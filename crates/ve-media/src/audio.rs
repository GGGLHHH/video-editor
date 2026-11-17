//! 音频解码：统一输出 f32 交错立体声 48 kHz。

use crate::MediaError;
use crate::probe::{open_input, time_to_ts, ts_to_time};
use ffmpeg_next as ff;
use std::collections::VecDeque;
use std::fmt;
use std::path::Path;
use ve_core::Time;

/// 输出采样率。
pub const OUTPUT_SAMPLE_RATE: u32 = 48_000;
/// 输出声道数（交错）。
pub const OUTPUT_CHANNELS: usize = 2;

/// 音频解码器。
pub struct AudioDecoder {
    input: ff::format::context::Input,
    stream: usize,
    tb: ff::Rational,
    decoder: ff::decoder::Audio,
    resampler: Option<ff::software::resampling::Context>,
    resampled: ff::frame::Audio,
    /// 已解码、待读出的交错样本。
    buf: VecDeque<f32>,
    sent_eof: bool,
    drained: bool,
    /// 输出采样率。
    rate: u32,
    /// 定位目标：下一帧解出来时据此计算要丢弃 / 补静音的样本数。
    target: Option<Time>,
    /// 还要丢弃的样本帧数。
    skip: u64,
    /// 文件起始时间戳（对外时间 = 时间戳 − 它）。
    offset: Time,
}

impl fmt::Debug for AudioDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioDecoder")
            .field("buffered", &self.buf.len())
            .finish_non_exhaustive()
    }
}

impl AudioDecoder {
    /// 打开文件的最佳音频流，输出 [`OUTPUT_SAMPLE_RATE`]。
    ///
    /// # Errors
    /// 打不开文件或没有音频流。
    pub fn open(path: &Path) -> Result<Self, MediaError> {
        Self::open_with_rate(path, OUTPUT_SAMPLE_RATE)
    }

    /// 打开文件的最佳音频流，输出采样率 `rate`（跟随声卡）。
    ///
    /// # Errors
    /// 打不开文件或没有音频流。
    pub fn open_with_rate(path: &Path, rate: u32) -> Result<Self, MediaError> {
        let input = open_input(path)?;
        let offset = crate::probe::start_offset(&input);
        let stream = input
            .streams()
            .best(ff::media::Type::Audio)
            .ok_or_else(|| MediaError::NoStream {
                path: path.to_path_buf(),
                kind: "音频流",
            })?;
        let (index, tb) = (stream.index(), stream.time_base());
        let ctx = ff::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = ctx.decoder().audio()?;
        Ok(Self {
            input,
            stream: index,
            tb,
            decoder,
            resampler: None,
            resampled: ff::frame::Audio::empty(),
            buf: VecDeque::new(),
            sent_eof: false,
            drained: false,
            rate: rate.max(1),
            target: Some(Time::ZERO),
            skip: 0,
            offset,
        })
    }

    /// 输出采样率。
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// 定位到 `t`（样本级精确）。
    ///
    /// # Errors
    /// 定位失败。
    pub fn seek_to(&mut self, t: Time) -> Result<(), MediaError> {
        let t = t.max(Time::ZERO);
        let us = time_to_ts(t + self.offset, ff::Rational::new(1, ff::ffi::AV_TIME_BASE));
        self.input.seek(us, ..us)?;
        self.decoder.flush();
        self.resampler = None;
        self.buf.clear();
        self.sent_eof = false;
        self.drained = false;
        self.target = Some(t);
        self.skip = 0;
        Ok(())
    }

    /// 读出最多 `out.len() / 2` 个样本帧（交错立体声），返回实际帧数；0 表示结束。
    ///
    /// # Errors
    /// 解码失败。
    pub fn read(&mut self, out: &mut [f32]) -> Result<usize, MediaError> {
        let want = out.len() / OUTPUT_CHANNELS;
        while self.buf.len() < want * OUTPUT_CHANNELS && self.fill()? {}
        let frames = (self.buf.len() / OUTPUT_CHANNELS).min(want);
        for (dst, src) in out
            .iter_mut()
            .zip(self.buf.drain(..frames * OUTPUT_CHANNELS))
        {
            *dst = src;
        }
        Ok(frames)
    }

    /// 再解一些数据进缓冲；没有更多数据时返回 `false`。
    fn fill(&mut self) -> Result<bool, MediaError> {
        loop {
            let mut frame = ff::frame::Audio::empty();
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    self.push_frame(&mut frame)?;
                    return Ok(true);
                }
                Err(ff::Error::Eof) => return self.drain_resampler(),
                Err(ff::Error::Other { errno }) if errno == ff::error::EAGAIN => {}
                Err(e) => return Err(e.into()),
            }
            if self.sent_eof {
                return self.drain_resampler();
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

    fn drain_resampler(&mut self) -> Result<bool, MediaError> {
        if self.drained {
            return Ok(false);
        }
        self.drained = true;
        if let Some(r) = self.resampler.as_mut() {
            self.resampled = Self::output_frame(4096);
            r.flush(&mut self.resampled)?;
            self.append_resampled();
        }
        Ok(!self.buf.is_empty())
    }

    fn push_frame(&mut self, frame: &mut ff::frame::Audio) -> Result<(), MediaError> {
        if let Some(target) = self.target.take() {
            let pts = frame
                .timestamp()
                .or_else(|| frame.pts())
                .map_or(target, |ts| ts_to_time(ts, self.tb) - self.offset);
            if pts < target {
                self.skip = u64::try_from((target - pts).to_samples(self.rate)).unwrap_or(0);
            } else {
                let pad = usize::try_from((pts - target).to_samples(self.rate)).unwrap_or(0);
                self.buf
                    .extend(std::iter::repeat_n(0.0, pad * OUTPUT_CHANNELS));
            }
        }
        // 布局顺序为 UNSPEC（如 wav）时 swr_init 会把自己那份改成默认布局，之后每一帧都会被判为
        // 「输入变了」。这里先把帧的布局规范成同样的默认布局。
        if frame.channel_layout().0.order == ff::ffi::AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC {
            frame.set_channel_layout(ff::ChannelLayout::default(i32::from(frame.channels())));
        }
        let layout = frame.channel_layout();
        let rebuild = self.resampler.as_ref().is_none_or(|r| {
            let i = r.input();
            i.format != frame.format() || i.rate != frame.rate()
        });
        if rebuild {
            self.resampler = Some(ff::software::resampling::Context::get(
                frame.format(),
                layout,
                frame.rate(),
                ff::format::Sample::F32(ff::format::sample::Type::Packed),
                ff::ChannelLayout::default(2),
                self.rate,
            )?);
        }
        // 每次新分配输出帧：容量按采样率换算再留余量（复用旧帧会把容量缩成上一次的输出长度）
        let capacity = frame.samples() * self.rate as usize / frame.rate().max(1) as usize + 256;
        self.resampled = Self::output_frame(capacity);
        if let Some(r) = self.resampler.as_mut() {
            r.run(frame, &mut self.resampled)?;
        }
        self.append_resampled();
        Ok(())
    }

    fn output_frame(capacity: usize) -> ff::frame::Audio {
        ff::frame::Audio::new(
            ff::format::Sample::F32(ff::format::sample::Type::Packed),
            capacity,
            ff::ChannelLayout::default(2),
        )
    }

    fn append_resampled(&mut self) {
        let frames = self.resampled.samples();
        let bytes = frames * OUTPUT_CHANNELS * 4;
        let data = self.resampled.data(0);
        let data = data.get(..bytes).unwrap_or(data);
        let mut skip_values = usize::try_from(self.skip)
            .unwrap_or(usize::MAX)
            .saturating_mul(OUTPUT_CHANNELS);
        let values = data.len() / 4;
        let dropped = skip_values.min(values);
        skip_values -= dropped;
        self.skip = (skip_values / OUTPUT_CHANNELS) as u64;
        self.buf.extend(
            data.as_chunks::<4>()
                .0
                .iter()
                .skip(dropped)
                .map(|b| f32::from_ne_bytes(*b)),
        );
    }
}

#[cfg(test)]
#[expect(clippy::cast_precision_loss, reason = "测试里样本数、毫秒数都很小")]
mod tests {
    use super::*;
    use crate::testing::{Fixture, fixture};
    use std::f64::consts::TAU;

    fn read_all(d: &mut AudioDecoder) -> Vec<f32> {
        let mut out = Vec::new();
        let mut buf = vec![0.0f32; 4096];
        loop {
            let n = d.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n * 2]);
        }
        out
    }

    fn expected(t_secs: f64) -> f32 {
        #[expect(clippy::cast_possible_truncation, reason = "测试里比较用")]
        let v = (0.5 * (TAU * 440.0 * t_secs).sin()) as f32;
        v
    }

    #[test]
    fn tone_is_440hz() {
        let mut d = AudioDecoder::open(&fixture(Fixture::ToneStereo48k)).unwrap();
        let samples = read_all(&mut d);
        assert_eq!(samples.len() / 2, 96_000, "2 秒 48k");
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        let crossings = left
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        let hz = crossings as f64 / 2.0;
        assert!((hz - 440.0).abs() <= 2.0, "{hz} Hz");
        // 左右声道相同
        assert!(
            samples
                .as_chunks::<2>()
                .0
                .iter()
                .all(|c| (c[0] - c[1]).abs() < 1e-6)
        );
    }

    #[test]
    fn seek_audio_sample_accurate() {
        let mut d = AudioDecoder::open(&fixture(Fixture::ToneStereo48k)).unwrap();
        for start_ms in [1000_i64, 250, 1777, 0] {
            d.seek_to(Time::from_millis(start_ms)).unwrap();
            let mut buf = vec![0.0f32; 960];
            assert_eq!(d.read(&mut buf).unwrap(), 480);
            for (i, pair) in buf.as_chunks::<2>().0.iter().enumerate() {
                let t = start_ms as f64 / 1000.0 + i as f64 / 48_000.0;
                assert!(
                    (pair[0] - expected(t)).abs() < 0.01,
                    "{start_ms}ms 第 {i} 个样本"
                );
            }
        }
    }

    #[test]
    fn seek_past_end_reads_nothing() {
        let mut d = AudioDecoder::open(&fixture(Fixture::ToneStereo48k)).unwrap();
        d.seek_to(Time::from_secs(5)).unwrap();
        let mut buf = vec![0.0f32; 64];
        assert_eq!(d.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn aac_in_mp4_decodes_to_48k_stereo() {
        let mut d = AudioDecoder::open(&fixture(Fixture::VideoWithAudio)).unwrap();
        let samples = read_all(&mut d);
        let secs = samples.len() as f64 / 2.0 / 48_000.0;
        assert!((secs - 2.0).abs() < 0.05, "{secs}");
        assert!(samples.iter().any(|v| v.abs() > 0.1));
    }

    #[test]
    fn open_with_rate_resamples_to_44100() {
        let mut d = AudioDecoder::open_with_rate(&fixture(Fixture::ToneStereo48k), 44_100).unwrap();
        assert_eq!(d.sample_rate(), 44_100);
        let samples = read_all(&mut d);
        let frames = samples.len() / 2;
        assert!(frames.abs_diff(88_200) <= 64, "{frames}");
    }

    #[test]
    fn ts_audio_starts_at_zero_without_padding() {
        // TS 时间戳不从 0 开始：开头不应补一段等长的静音
        let mut d = AudioDecoder::open(&fixture(Fixture::TsWithOffset)).unwrap();
        let mut buf = vec![0.0f32; 4800];
        let n = d.read(&mut buf).unwrap();
        let peak = buf[..n * 2].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.3, "开头 50ms 是静音（峰值 {peak}）");
    }

    #[test]
    fn open_video_only_errors() {
        let err = AudioDecoder::open(&fixture(Fixture::FrameIndexed30)).unwrap_err();
        assert!(matches!(err, MediaError::NoStream { .. }), "{err:?}");
    }
}
