//! 视频解码：精确定位到某一时刻的帧，输出缩放到预览尺寸的 BGRA。
//!
//! 性能要点：
//! - 帧多线程解码；
//! - 定位时跳过的帧不做颜色转换，只转换最终交出去的那一帧；
//! - 小幅向前定位（2 秒内）直接往前解，不回关键帧；
//! - 直接缩放到预览尺寸，减少后续上传显卡的数据量。

use crate::MediaError;
use crate::probe::{open_input, time_to_ts, ts_to_time};
use ffmpeg_next as ff;
use std::fmt;
use std::path::Path;
use ve_core::{FrameRate, Time};

/// 小于这个距离的向前定位直接往前解。
const FORWARD_DECODE_WINDOW: Time = Time::from_secs(2);

/// 一帧 BGRA 图像（紧密排列，每行 `width * 4` 字节）。
#[derive(Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// 素材内时间。
    pub pts: Time,
    /// 宽。
    pub width: u32,
    /// 高。
    pub height: u32,
    /// BGRA 像素。
    pub data: Vec<u8>,
}

impl fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VideoFrame")
            .field("pts", &self.pts)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.data.len())
            .finish()
    }
}

/// 输出尺寸上限（保持宽高比、不放大）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScaleTarget {
    /// 最大宽。
    pub max_width: u32,
    /// 最大高。
    pub max_height: u32,
}

/// 等比缩放到不超过 `target`、不放大，宽高取偶数（至少 2）。
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "结果不超过源尺寸，且为正"
)]
pub fn fit_size(src_w: u32, src_h: u32, target: ScaleTarget) -> (u32, u32) {
    let scale = (f64::from(target.max_width) / f64::from(src_w.max(1)))
        .min(f64::from(target.max_height) / f64::from(src_h.max(1)))
        .min(1.0);
    let even = |v: f64| ((v.floor() as u32) & !1).max(2);
    (
        even(f64::from(src_w) * scale),
        even(f64::from(src_h) * scale),
    )
}

/// 视频解码器。
pub struct VideoDecoder {
    input: ff::format::context::Input,
    stream: usize,
    tb: ff::Rational,
    decoder: ff::decoder::Video,
    scaler: Option<ff::software::scaling::Context>,
    scaled: ff::frame::Video,
    out: (u32, u32),
    src: (u32, u32),
    rate: FrameRate,
    /// 静态图片。
    is_still: bool,
    /// 静态图片解出的那一帧。
    still_frame: Option<VideoFrame>,
    /// 已解出但还没交出去的帧（定位时多解的那一帧）。
    pending: Option<ff::frame::Video>,
    /// 最近交出去的帧（原始格式），同一时刻再次请求时直接复用。
    last: Option<(Time, ff::frame::Video)>,
    sent_eof: bool,
    /// 文件起始时间戳（对外时间 = 时间戳 − 它）。
    offset: Time,
}

impl fmt::Debug for VideoDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VideoDecoder")
            .field("src", &self.src)
            .field("out", &self.out)
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

impl VideoDecoder {
    /// 打开文件的最佳视频流。
    ///
    /// # Errors
    /// 打不开文件或没有视频流。
    pub fn open(path: &Path, target: ScaleTarget) -> Result<Self, MediaError> {
        let info = crate::probe(path)?;
        let video = info.video.ok_or_else(|| MediaError::NoStream {
            path: path.to_path_buf(),
            kind: "视频流",
        })?;
        let input = open_input(path)?;
        let offset = crate::probe::start_offset(&input);
        let stream = crate::probe::video_stream(&input).ok_or_else(|| MediaError::NoStream {
            path: path.to_path_buf(),
            kind: "视频流",
        })?;
        let (index, tb) = (stream.index(), stream.time_base());
        let mut ctx = ff::codec::context::Context::from_parameters(stream.parameters())?;
        ctx.set_threading(ff::codec::threading::Config {
            kind: ff::codec::threading::Type::Frame,
            count: 0,
        });
        let decoder = ctx.decoder().video()?;
        let src = (decoder.width(), decoder.height());
        Ok(Self {
            input,
            stream: index,
            tb,
            decoder,
            scaler: None,
            scaled: ff::frame::Video::empty(),
            out: fit_size(src.0, src.1, target),
            src,
            rate: video.frame_rate,
            is_still: video.still,
            still_frame: None,
            pending: None,
            last: None,
            sent_eof: false,
            offset,
        })
    }

    /// 源尺寸。
    #[must_use]
    pub const fn source_size(&self) -> (u32, u32) {
        self.src
    }

    /// 输出尺寸。
    #[must_use]
    pub const fn output_size(&self) -> (u32, u32) {
        self.out
    }

    /// 帧率。
    #[must_use]
    pub const fn frame_rate(&self) -> FrameRate {
        self.rate
    }

    /// 解出下一帧原始数据；`None` 表示到结尾。
    fn decode_raw(&mut self) -> Result<Option<ff::frame::Video>, MediaError> {
        loop {
            let mut frame = ff::frame::Video::empty();
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => return Ok(Some(frame)),
                Err(ff::Error::Eof) => return Ok(None),
                Err(ff::Error::Other { errno }) if errno == ff::error::EAGAIN => {}
                Err(e) => return Err(e.into()),
            }
            if self.sent_eof {
                return Ok(None);
            }
            let mut packet = ff::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() == self.stream => self.decoder.send_packet(&packet)?,
                Ok(()) => {}
                Err(ff::Error::Eof) => {
                    self.decoder.send_eof()?;
                    self.sent_eof = true;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn pts_of(&self, frame: &ff::frame::Video) -> Time {
        frame.timestamp().or_else(|| frame.pts()).map_or_else(
            || {
                self.last
                    .as_ref()
                    .map_or(Time::ZERO, |(t, _)| *t + self.rate.frame_duration())
            },
            |ts| ts_to_time(ts, self.tb) - self.offset,
        )
    }

    /// 颜色转换 + 缩放成紧密排列的 BGRA。
    fn convert(&mut self, raw: &ff::frame::Video, pts: Time) -> Result<VideoFrame, MediaError> {
        let (w, h) = self.out;
        let rebuild = self.scaler.as_ref().is_none_or(|s| {
            let i = s.input();
            i.format != raw.format() || i.width != raw.width() || i.height != raw.height()
        });
        if rebuild {
            self.scaler = Some(ff::software::scaling::Context::get(
                raw.format(),
                raw.width(),
                raw.height(),
                ff::format::Pixel::BGRA,
                w,
                h,
                ff::software::scaling::Flags::BILINEAR,
            )?);
        }
        if let Some(scaler) = self.scaler.as_mut() {
            scaler.run(raw, &mut self.scaled)?;
        }
        let row = w as usize * 4;
        let stride = self.scaled.stride(0);
        let plane = self.scaled.data(0);
        let mut data = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            data.extend_from_slice(&plane[y * stride..y * stride + row]);
        }
        Ok(VideoFrame {
            pts,
            width: w,
            height: h,
            data,
        })
    }

    fn deliver(&mut self, raw: ff::frame::Video, pts: Time) -> Result<VideoFrame, MediaError> {
        let frame = self.convert(&raw, pts)?;
        self.last = Some((pts, raw));
        Ok(frame)
    }

    /// 静态图片：解一次，之后一直返回同一帧。
    fn decode_still(&mut self, t: Time) -> Result<VideoFrame, MediaError> {
        if let Some(f) = &self.still_frame {
            return Ok(VideoFrame {
                pts: t,
                ..f.clone()
            });
        }
        let raw = self
            .decode_raw()?
            .ok_or(MediaError::Decode(ff::Error::InvalidData))?;
        let f = self.convert(&raw, Time::ZERO)?;
        self.still_frame = Some(f.clone());
        Ok(VideoFrame { pts: t, ..f })
    }

    /// 按顺序解下一帧；`None` 表示到结尾。
    ///
    /// # Errors
    /// 解码失败。
    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        if self.is_still {
            return self.decode_still(Time::ZERO).map(Some);
        }
        let raw = match self.pending.take() {
            Some(raw) => Some(raw),
            None => self.decode_raw()?,
        };
        raw.map(|raw| {
            let pts = self.pts_of(&raw);
            self.deliver(raw, pts)
        })
        .transpose()
    }

    /// 定位到 `t`：返回 `pts <= t` 的最后一帧；`t` 早于第一帧时返回第一帧，晚于结尾时返回最后一帧。
    ///
    /// # Errors
    /// 定位或解码失败。
    pub fn seek_to(&mut self, t: Time) -> Result<VideoFrame, MediaError> {
        if self.is_still {
            return self.decode_still(t);
        }
        let near_ahead = self
            .last
            .as_ref()
            .is_some_and(|(last, _)| *last <= t && t - *last < FORWARD_DECODE_WINDOW);
        if !near_ahead {
            let us = time_to_ts(
                t.max(Time::ZERO) + self.offset,
                ff::Rational::new(1, ff::ffi::AV_TIME_BASE),
            );
            self.input.seek(us, ..us)?;
            self.decoder.flush();
            self.pending = None;
            self.last = None;
            self.sent_eof = false;
        }
        let mut best: Option<(Time, ff::frame::Video)> = None;
        loop {
            let raw = match self.pending.take() {
                Some(raw) => raw,
                None => match self.decode_raw()? {
                    Some(raw) => raw,
                    None => break,
                },
            };
            let pts = self.pts_of(&raw);
            if pts <= t || (best.is_none() && self.last.is_none()) {
                // 第一帧就晚于 t（t 在开头之前）时也先收下
                let first_after = pts > t;
                best = Some((pts, raw));
                if first_after {
                    break;
                }
            } else {
                self.pending = Some(raw);
                break;
            }
        }
        match best.or_else(|| self.last.take()) {
            Some((pts, raw)) => self.deliver(raw, pts),
            None => Err(MediaError::Decode(ff::Error::InvalidData)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, expected_gray, fixture};
    use ve_core::FrameRate;

    const RATE: FrameRate = FrameRate::new(30, 1);

    /// 帧中心像素的灰度（BGRA 的 B 通道）。
    fn center_gray(f: &VideoFrame) -> f64 {
        let (x, y) = (f.width / 2, f.height / 2);
        let i = ((y * f.width + x) * 4) as usize;
        f64::from(f.data[i])
    }

    fn assert_frame(f: &VideoFrame, n: i64) {
        let gray = center_gray(f);
        assert!(
            (gray - expected_gray(n)).abs() <= 2.0,
            "期望第 {n} 帧（灰度 {:.1}），实际灰度 {gray}",
            expected_gray(n)
        );
        assert_eq!(RATE.frame_of(f.pts), n, "pts {:?}", f.pts);
    }

    fn open(target: ScaleTarget) -> VideoDecoder {
        VideoDecoder::open(&fixture(Fixture::FrameIndexed30), target).unwrap()
    }

    const FULL: ScaleTarget = ScaleTarget {
        max_width: 1920,
        max_height: 1080,
    };

    #[test]
    fn seek_exact_frame() {
        let mut d = open(FULL);
        // 打乱顺序：前后来回跳
        for n in [
            0, 59, 17, 16, 15, 14, 30, 2, 44, 45, 1, 58, 29, 31, 7, 52, 0, 23, 38, 11,
        ] {
            let t = RATE.time_of_frame(n) + Time::from_millis(10);
            let f = d.seek_to(t).unwrap();
            assert_frame(&f, n);
        }
    }

    #[test]
    fn sequential_after_seek() {
        let mut d = open(FULL);
        assert_frame(&d.seek_to(Time::from_secs(1)).unwrap(), 30);
        for n in 31..36 {
            assert_frame(&d.next_frame().unwrap().unwrap(), n);
        }
    }

    #[test]
    fn small_forward_seek_continues_decoding() {
        let mut d = open(FULL);
        assert_frame(&d.seek_to(RATE.time_of_frame(10)).unwrap(), 10);
        assert_frame(&d.seek_to(RATE.time_of_frame(12)).unwrap(), 12);
        assert_frame(&d.seek_to(RATE.time_of_frame(12)).unwrap(), 12);
        assert_frame(&d.seek_to(RATE.time_of_frame(13)).unwrap(), 13);
        assert_frame(&d.seek_to(RATE.time_of_frame(3)).unwrap(), 3);
    }

    #[test]
    fn next_frame_reaches_end() {
        let mut d = open(FULL);
        let mut n = 0;
        while let Some(f) = d.next_frame().unwrap() {
            assert_frame(&f, n);
            n += 1;
        }
        assert_eq!(n, 60);
        assert!(d.next_frame().unwrap().is_none());
    }

    #[test]
    fn scales_to_target() {
        let mut d = open(ScaleTarget {
            max_width: 32,
            max_height: 32,
        });
        assert_eq!(d.output_size(), (32, 18));
        let f = d.seek_to(Time::ZERO).unwrap();
        assert_eq!((f.width, f.height, f.data.len()), (32, 18, 32 * 18 * 4));
        assert_eq!(d.source_size(), (64, 36));
        assert_eq!(d.frame_rate(), RATE);
    }

    #[test]
    fn never_upscales_and_keeps_even_size() {
        assert_eq!(fit_size(64, 36, FULL), (64, 36));
        assert_eq!(
            fit_size(
                1920,
                1080,
                ScaleTarget {
                    max_width: 1001,
                    max_height: 1001
                }
            ),
            (1000, 562)
        );
        assert_eq!(
            fit_size(
                1080,
                1920,
                ScaleTarget {
                    max_width: 1920,
                    max_height: 1080
                }
            ),
            (606, 1080)
        );
        assert_eq!(
            fit_size(
                3,
                3,
                ScaleTarget {
                    max_width: 1,
                    max_height: 1
                }
            ),
            (2, 2)
        );
    }

    #[test]
    fn seek_past_end_returns_last_frame() {
        let mut d = open(FULL);
        assert_frame(&d.seek_to(Time::from_secs(10)).unwrap(), 59);
        assert_frame(&d.seek_to(Time::from_secs(-1)).unwrap(), 0);
    }

    #[test]
    fn still_image_returns_same_frame_anywhere() {
        let mut d = VideoDecoder::open(&fixture(Fixture::Image), FULL).unwrap();
        for t in [Time::ZERO, Time::from_secs(3), Time::from_secs(100)] {
            let f = d.seek_to(t).unwrap();
            assert_eq!((f.width, f.height), (64, 36));
            let px = &f.data[0..4];
            assert!(px[2] > 240 && px[0] < 15 && px[1] < 15, "应为红色：{px:?}");
        }
    }

    #[test]
    fn ts_start_offset_is_hidden_from_callers() {
        // TS 的时间戳从 1.4 秒左右开始；素材内时间要从 0 算，和 ffmpeg 导出时的 -ss 一致
        let mut d = VideoDecoder::open(&fixture(Fixture::TsWithOffset), FULL).unwrap();
        assert_frame(&d.seek_to(Time::ZERO).unwrap(), 0);
        // 这个文件里视频比音频晚 21ms 开始（文件本身的音画同步关系，导出时 ffmpeg 也这样算），
        // 所以第 N 帧在 21ms + N/30 秒处
        assert_frame(
            &d.seek_to(RATE.time_of_frame(30) + Time::from_millis(30))
                .unwrap(),
            30,
        );
        assert_frame(&d.next_frame().unwrap().unwrap(), 31);
    }

    #[test]
    fn open_audio_only_file_errors() {
        let err = VideoDecoder::open(&fixture(Fixture::ToneStereo48k), FULL).unwrap_err();
        assert!(matches!(err, MediaError::NoStream { .. }), "{err:?}");
    }
}
