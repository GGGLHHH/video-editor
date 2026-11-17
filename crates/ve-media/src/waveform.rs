//! 波形：按时间分桶的最小 / 最大振幅，用于时间轴上画音频波形。

use crate::{AudioDecoder, MediaError, OUTPUT_CHANNELS, OUTPUT_SAMPLE_RATE};
use std::path::Path;

/// 解码整条音频，每 `1 / buckets_per_second` 秒一桶，返回各桶 `(最小, 最大)`（两声道合并）。
/// `buckets_per_second` 为 0 时返回空。
///
/// # Errors
/// 打不开文件、没有音频流、解码失败。
pub fn waveform_peaks(path: &Path, buckets_per_second: u32) -> Result<Vec<(f32, f32)>, MediaError> {
    if buckets_per_second == 0 {
        return Ok(Vec::new());
    }
    let bucket = (OUTPUT_SAMPLE_RATE / buckets_per_second).max(1) as usize * OUTPUT_CHANNELS;
    let mut decoder = AudioDecoder::open(path)?;
    let mut peaks = Vec::new();
    let mut buf = vec![0.0f32; bucket];
    let mut acc: Option<(f32, f32)> = None;
    let mut filled = 0;
    loop {
        let frames = decoder.read(&mut buf[..bucket - filled])?;
        if frames == 0 {
            break;
        }
        for &v in &buf[..frames * OUTPUT_CHANNELS] {
            acc = Some(acc.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v))));
        }
        filled += frames * OUTPUT_CHANNELS;
        if filled >= bucket {
            peaks.extend(acc.take());
            filled = 0;
        }
    }
    peaks.extend(acc);
    Ok(peaks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, fixture};

    #[test]
    fn peaks_of_tone_near_amplitude() {
        let peaks = waveform_peaks(&fixture(Fixture::ToneStereo48k), 100).unwrap();
        assert_eq!(peaks.len(), 200);
        for &(lo, hi) in &peaks {
            assert!((0.48..=0.52).contains(&hi), "{hi}");
            assert!((-0.52..=-0.48).contains(&lo), "{lo}");
        }
    }

    #[test]
    fn zero_buckets_per_second_is_rejected() {
        assert_eq!(
            waveform_peaks(&fixture(Fixture::ToneStereo48k), 0).unwrap(),
            []
        );
    }
}
