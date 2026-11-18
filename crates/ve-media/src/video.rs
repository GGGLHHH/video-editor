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
    scaler: Option<Scaler>,
    scaled: ff::frame::Video,
    /// 带透明的素材：先在原尺寸转成 BGRA 并预乘，再缩放（先预乘再插值，边缘才不会混进透明处的颜色）。
    native_scaler: Option<Scaler>,
    native: ff::frame::Video,
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
    /// 真正定位过几次（统计用）。
    seeks: u64,
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
            native_scaler: None,
            native: ff::frame::Video::empty(),
            out: fit_size(src.0, src.1, target),
            src,
            rate: video.frame_rate,
            is_still: video.still,
            still_frame: None,
            pending: None,
            last: None,
            sent_eof: false,
            offset,
            seeks: 0,
        })
    }

    /// 真正定位过几次（往回跳、往前跳远都要从关键帧解起，[`Self::seek_to`] 往后一点点时接着解，不算）。
    #[must_use]
    pub const fn seeks(&self) -> u64 {
        self.seeks
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

    /// 颜色转换 + 缩放成紧密排列的 BGRA（预乘透明）。
    fn convert(&mut self, raw: &ff::frame::Video, pts: Time) -> Result<VideoFrame, MediaError> {
        let (w, h) = self.out;
        let alpha = has_alpha(raw.format());
        if alpha {
            // 第一步：原尺寸转 BGRA，预乘
            let (sw, sh) = (raw.width(), raw.height());
            ensure_scaler(
                &mut self.native_scaler,
                raw,
                raw.format(),
                (sw, sh),
                (sw, sh),
            )?;
            if let Some(s) = self.native_scaler.as_mut() {
                s.0.run(raw, &mut self.native)?;
            }
            premultiply_frame(&mut self.native, sw, sh);
            if (sw, sh) != (w, h) {
                // 第二步：预乘过的 BGRA 再缩放
                let native = &self.native;
                ensure_scaler(
                    &mut self.scaler,
                    native,
                    ff::format::Pixel::BGRA,
                    (sw, sh),
                    (w, h),
                )?;
                if let Some(s) = self.scaler.as_mut() {
                    s.0.run(native, &mut self.scaled)?;
                }
            }
        } else {
            ensure_scaler(
                &mut self.scaler,
                raw,
                raw.format(),
                (raw.width(), raw.height()),
                (w, h),
            )?;
            if let Some(s) = self.scaler.as_mut() {
                s.0.run(raw, &mut self.scaled)?;
            }
        }
        let src = if alpha && (raw.width(), raw.height()) == (w, h) {
            &self.native
        } else {
            &self.scaled
        };
        let row = w as usize * 4;
        let stride = src.stride(0);
        let plane = src.data(0);
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
            self.seeks += 1;
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

/// swscale 上下文。ffmpeg-next 没给它实现 `Send`，但 `SwsContext` 没有线程亲和性：
/// 只要同一时刻只有一个线程在用，就可以在线程之间移动。预览要把解码器交给别的线程并行解码。
struct Scaler(ff::software::scaling::Context);

#[expect(
    unsafe_code,
    clippy::non_send_fields_in_send_ty,
    reason = "ffmpeg-next 没给 swscale 上下文实现 Send；理由见下面的 SAFETY"
)]
// SAFETY: `SwsContext` 不依赖创建它的线程；`Scaler` 只能通过 `VideoDecoder` 的 `&mut self` 访问，
// 不会被两个线程同时使用。
unsafe impl Send for Scaler {}

/// 像素格式的描述标志（`AV_PIX_FMT_FLAG_*`）。
fn pixel_flags(format: ff::format::Pixel) -> u64 {
    format.descriptor().map_or(0, |d| {
        // SAFETY: `descriptor()` 只在 FFmpeg 的静态描述表里有这一项时才返回，指针指向静态数据，永远有效。
        #[expect(unsafe_code, reason = "ffmpeg-next 没有暴露像素格式标志")]
        unsafe {
            (*d.as_ptr()).flags
        }
    })
}

/// 源像素格式带透明通道。
fn has_alpha(format: ff::format::Pixel) -> bool {
    let alpha = ff::ffi::AV_PIX_FMT_FLAG_ALPHA as u64;
    pixel_flags(format) & alpha != 0
}

/// YUV 素材该用的色彩矩阵（`SWS_CS_*`）：按素材标注；没标注时高度 ≥ 720 按 BT.709，否则按 BT.601。
const fn yuv_matrix(space: ff::color::Space, height: u32) -> std::ffi::c_int {
    use ff::color::Space;
    match space {
        Space::BT709 => ff::ffi::SWS_CS_ITU709,
        Space::BT470BG | Space::SMPTE170M => ff::ffi::SWS_CS_ITU601,
        Space::SMPTE240M => ff::ffi::SWS_CS_SMPTE240M,
        Space::FCC => ff::ffi::SWS_CS_FCC,
        Space::BT2020NCL | Space::BT2020CL => ff::ffi::SWS_CS_BT2020,
        _ if height >= 720 => ff::ffi::SWS_CS_ITU709,
        _ => ff::ffi::SWS_CS_ITU601,
    }
}

/// 按素材的色彩矩阵和范围设置 swscale（RGB 源不用设置）。
/// swscale 默认一律按 BT.601 有限范围处理，高清素材（BT.709）颜色会偏。
fn set_yuv_matrix(scaler: &mut ff::software::scaling::Context, raw: &ff::frame::Video) {
    let rgb = ff::ffi::AV_PIX_FMT_FLAG_RGB as u64;
    if pixel_flags(raw.format()) & rgb != 0 {
        return;
    }
    let full_range = raw.color_range() == ff::color::Range::JPEG
        || raw
            .format()
            .descriptor()
            .is_some_and(|d| d.name().starts_with("yuvj"));
    let matrix = yuv_matrix(raw.color_space(), raw.height());
    // SAFETY: `scaler` 是刚建好的有效上下文；`sws_getCoefficients` 返回指向 FFmpeg 静态表的指针，
    // `sws_setColorspaceDetails` 只读这两张表，不保留指针。
    #[expect(unsafe_code, reason = "ffmpeg-next 没有封装 sws_setColorspaceDetails")]
    let status = unsafe {
        let src = ff::ffi::sws_getCoefficients(matrix);
        let dst = ff::ffi::sws_getCoefficients(ff::ffi::SWS_CS_DEFAULT);
        ff::ffi::sws_setColorspaceDetails(
            scaler.as_mut_ptr(),
            src,
            std::ffi::c_int::from(full_range),
            dst,
            1,
            0,
            1 << 16,
            1 << 16,
        )
    };
    if status < 0 {
        log::warn!("设置色彩矩阵失败（{status}），按 swscale 默认处理");
    }
}

/// 输入格式或尺寸变了才重建缩放上下文；`frame` 用来取色彩矩阵和范围。
fn ensure_scaler(
    slot: &mut Option<Scaler>,
    frame: &ff::frame::Video,
    format: ff::format::Pixel,
    from: (u32, u32),
    to: (u32, u32),
) -> Result<(), MediaError> {
    let current = slot.as_ref().is_some_and(|s| {
        let (i, o) = (s.0.input(), s.0.output());
        i.format == format && (i.width, i.height) == from && (o.width, o.height) == to
    });
    if !current {
        let mut scaler = ff::software::scaling::Context::get(
            format,
            from.0,
            from.1,
            ff::format::Pixel::BGRA,
            to.0,
            to.1,
            ff::software::scaling::Flags::BILINEAR,
        )?;
        set_yuv_matrix(&mut scaler, frame);
        *slot = Some(Scaler(scaler));
    }
    Ok(())
}

/// BGRA 帧原地预乘（按行，跳过每行的对齐填充）。
fn premultiply_frame(frame: &mut ff::frame::Video, width: u32, height: u32) {
    let stride = frame.stride(0);
    let row = width as usize * 4;
    let data = frame.data_mut(0);
    for y in 0..height as usize {
        premultiply(&mut data[y * stride..y * stride + row]);
    }
}

/// BGRA 原地预乘透明度。
fn premultiply(data: &mut [u8]) {
    for px in data.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        for c in &mut px[..3] {
            // 结果 ≤ 255，截断安全
            #[expect(clippy::cast_possible_truncation, reason = "c·a/255 ≤ 255")]
            let v = ((u16::from(*c) * a + 127) / 255) as u8;
            *c = v;
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

    /// 中心像素的 (B, G, R, A)。
    fn center_bgra(f: &VideoFrame) -> [u8; 4] {
        pixel(f, f.width / 2, f.height / 2)
    }

    fn pixel(f: &VideoFrame, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * f.width + x) * 4) as usize;
        [f.data[i], f.data[i + 1], f.data[i + 2], f.data[i + 3]]
    }

    fn assert_near(got: [u8; 4], want: [u8; 4], tol: u8, what: &str) {
        for (g, w) in got.iter().zip(want) {
            assert!(g.abs_diff(w) <= tol, "{what}：期望 {want:?}，实际 {got:?}");
        }
    }

    fn first_frame(kind: Fixture) -> VideoFrame {
        let mut d = VideoDecoder::open(&fixture(kind), FULL).unwrap();
        d.next_frame().unwrap().unwrap()
    }

    #[test]
    fn bt709_tagged_source_decodes_to_original_rgb() {
        let f = first_frame(Fixture::Bt709Red);
        // 文件里实际编码的是 YUV (72, 111, 197)，按 BT.709 有限范围精确换算是 RGB (189, 32, 29)；
        // 和原色 (192, 32, 32) 的差来自 8 位量化，不是解码的问题。按 BT.601 解会得到 (175, 14, 30)。
        assert_near(center_bgra(&f), [29, 32, 189, 255], 2, "BT.709 素材");
    }

    #[test]
    fn untagged_sd_uses_bt601() {
        let f = first_frame(Fixture::Bt601Red);
        assert_near(
            center_bgra(&f),
            [0x20, 0x20, 0xC0, 255],
            3,
            "未标注的标清素材",
        );
    }

    #[test]
    fn alpha_png_is_premultiplied() {
        let f = first_frame(Fixture::PngHalfAlpha);
        assert_near(pixel(&f, 8, 32), [0, 0, 127, 127], 1, "左半边");
        assert_eq!(pixel(&f, 56, 32), [0, 0, 0, 0], "右半边");
    }

    #[test]
    fn scaled_alpha_edges_have_no_halo() {
        // 缩小时要先预乘再插值：否则透明处存的白色会混进边缘，台标周围一圈发白
        let mut decoder = VideoDecoder::open(
            &fixture(Fixture::PngRedOnClearWhite),
            ScaleTarget {
                max_width: 16,
                max_height: 16,
            },
        )
        .unwrap();
        let frame = decoder.next_frame().unwrap().unwrap();
        assert_eq!((frame.width, frame.height), (16, 16));
        for x in 6..10 {
            let [blue, green, red, alpha] = pixel(&frame, x, 8);
            // 预乘后颜色不能超过透明度；红色台标的边缘不该有绿、蓝分量
            assert!(
                red <= alpha && green <= 8 && blue <= 8,
                "x={x} 边缘像素 {:?}",
                [blue, green, red, alpha]
            );
        }
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
        assert_eq!(d.seeks(), 0);
        assert_frame(&d.seek_to(RATE.time_of_frame(10)).unwrap(), 10);
        assert_eq!(d.seeks(), 1, "从头跳到第 10 帧要定位");
        assert_frame(&d.seek_to(RATE.time_of_frame(12)).unwrap(), 12);
        assert_frame(&d.seek_to(RATE.time_of_frame(12)).unwrap(), 12);
        assert_frame(&d.seek_to(RATE.time_of_frame(13)).unwrap(), 13);
        assert_eq!(d.seeks(), 1, "往后一点点：接着解，不定位");
        assert_frame(&d.seek_to(RATE.time_of_frame(3)).unwrap(), 3);
        assert_eq!(d.seeks(), 2, "往回跳要定位");
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
