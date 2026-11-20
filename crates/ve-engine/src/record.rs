//! 录音（剪映工具栏的 🎤）：从默认麦克风录，停下后写成 16 位 WAV。

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::io::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use ve_core::Time;

/// 录到的样本（交错）和最近一小段的峰值（界面上的电平）。
#[derive(Debug, Default)]
struct Captured {
    samples: Vec<f32>,
    peak: f32,
}

/// 正在进行的录音。
pub struct Recorder {
    stream: cpal::Stream,
    captured: Arc<Mutex<Captured>>,
    rate: u32,
    channels: u16,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("rate", &self.rate)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

fn push(captured: &Mutex<Captured>, data: impl Iterator<Item = f32>) {
    let mut c = captured.lock().unwrap_or_else(PoisonError::into_inner);
    let mut peak = 0.0f32;
    for s in data {
        peak = peak.max(s.abs());
        c.samples.push(s);
    }
    // 电平慢慢落下来（看着不跳）
    c.peak = peak.max(c.peak * 0.85);
}

impl Recorder {
    /// 从默认麦克风开始录。
    ///
    /// # Errors
    /// 没有麦克风、取不到配置、格式不支持、启动失败。
    pub fn start_default() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "没有找到麦克风".to_owned())?;
        let config = device.default_input_config().map_err(|e| e.to_string())?;
        let rate = config.sample_rate();
        let channels = config.channels();
        let captured = Arc::new(Mutex::new(Captured::default()));
        let sink = Arc::clone(&captured);
        let err = |e| log::warn!("录音出错：{e}");
        let stream_config: cpal::StreamConfig = config.config();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| push(&sink, data.iter().copied()),
                err,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                stream_config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    push(&sink, data.iter().map(|s| f32::from(*s) / 32_768.0));
                },
                err,
                None,
            ),
            other => return Err(format!("不支持的麦克风格式 {other}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            stream,
            captured,
            rate,
            channels,
        })
    }

    /// 已经录了多长。
    #[must_use]
    pub fn elapsed(&self) -> Time {
        let len = self
            .captured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .samples
            .len();
        let frames = len / usize::from(self.channels.max(1));
        Time::from_samples(i64::try_from(frames).unwrap_or(i64::MAX), self.rate)
    }

    /// 最近的峰值（0–1，界面上的电平）。
    #[must_use]
    pub fn level(&self) -> f32 {
        self.captured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .peak
            .min(1.0)
    }

    /// 停下来，写成 WAV。返回录了多长。
    ///
    /// # Errors
    /// 一点都没录到、写文件失败。
    pub fn finish(self, path: &std::path::Path) -> Result<Time, String> {
        drop(self.stream.pause());
        drop(self.stream);
        let c = std::mem::take(&mut *self.captured.lock().unwrap_or_else(PoisonError::into_inner));
        if c.samples.is_empty() {
            return Err("没有录到声音".to_owned());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        write_wav(path, &c.samples, self.rate, self.channels).map_err(|e| e.to_string())?;
        let frames = c.samples.len() / usize::from(self.channels.max(1));
        Ok(Time::from_samples(
            i64::try_from(frames).unwrap_or(i64::MAX),
            self.rate,
        ))
    }
}

/// 交错的 f32 样本写成 16 位 PCM 的 WAV。
///
/// # Errors
/// 写文件失败。
pub fn write_wav(
    path: &std::path::Path,
    samples: &[f32],
    rate: u32,
    channels: u16,
) -> std::io::Result<()> {
    let data_len = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX);
    let block = channels * 2;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * u32::from(block)).to_le_bytes())?;
    f.write_all(&block.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        #[expect(clippy::cast_possible_truncation, reason = "夹到 16 位范围")]
        let v = (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
        f.write_all(&v.to_le_bytes())?;
    }
    f.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_has_the_right_header_and_length() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        // 0.5 秒 48k 立体声
        let samples: Vec<f32> = (0..48_000u16)
            .map(|i| (f32::from(i) * 0.01).sin() * 0.5)
            .collect();
        write_wav(&path, &samples, 48_000, 2).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(bytes.len(), 44 + 48_000 * 2);
        let r = ve_media::probe(&path).unwrap();
        let a = r.audio.unwrap();
        assert_eq!((a.sample_rate, a.channels), (48_000, 2));
        assert!((r.duration - Time::from_millis(500)).0.abs() <= Time::from_millis(5).0);
        // 满幅和越界的样本夹到 16 位
        write_wav(&path, &[2.0, -2.0], 48_000, 1).unwrap();
        let clipped = std::fs::read(&path).unwrap();
        assert_eq!(i16::from_le_bytes([clipped[44], clipped[45]]), 32_767);
        assert_eq!(i16::from_le_bytes([clipped[46], clipped[47]]), -32_767);
    }
}
