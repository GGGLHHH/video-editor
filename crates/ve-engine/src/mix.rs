//! 混音：按片段增益和淡入淡出包络把各段音频累加到输出缓冲。
//!
//! 所有缓冲都是交错立体声 f32。

use crate::resolve::AudioSegment;
use ve_core::Time;

const CHANNELS: usize = 2;

/// 时间轴时刻 `t` 处这段音频的增益（片段增益或音量关键帧 × 淡入淡出线性包络）。
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "包络在 0..=1 之间")]
pub fn gain_at(seg: &AudioSegment, t: Time) -> f32 {
    let gain = ve_core::keyframe::value_of(&seg.volume, t - seg.clip_start)
        .map_or(seg.gain, crate::resolve::db_to_gain);
    let mut env = 1.0f64;
    if seg.fade_in > Time::ZERO {
        env = env.min((t - seg.sound_start).as_secs_f64() / seg.fade_in.as_secs_f64());
    }
    if seg.fade_out > Time::ZERO {
        env = env.min((seg.sound_end - t).as_secs_f64() / seg.fade_out.as_secs_f64());
    }
    gain * env.clamp(0.0, 1.0) as f32
}

/// 把 `samples`（从 `seg.timeline_start` 开始）乘增益后累加进 `out`（从 `out_start` 开始），
/// 超出 `out` 的部分丢弃。不做限幅，全部累加完后调用 [`clamp_mix`]。
pub fn mix_into(
    out: &mut [f32],
    samples: &[f32],
    seg: &AudioSegment,
    out_start: Time,
    sample_rate: u32,
) {
    let offset = (seg.timeline_start - out_start).to_samples(sample_rate);
    let out_frames = i64::try_from(out.len() / CHANNELS).unwrap_or(i64::MAX);
    // 只遍历和输出重叠的那部分样本帧
    let first = (-offset).max(0);
    let last = (out_frames - offset).min(i64::try_from(samples.len() / CHANNELS).unwrap_or(0));
    let flat = seg.fade_in == Time::ZERO && seg.fade_out == Time::ZERO && seg.volume.is_empty();
    for i in first..last {
        let gain = if flat {
            seg.gain
        } else {
            gain_at(seg, seg.timeline_start + Time::from_samples(i, sample_rate))
        };
        let (Ok(src), Ok(dst)) = (usize::try_from(i), usize::try_from(i + offset)) else {
            continue;
        };
        for c in 0..CHANNELS {
            out[dst * CHANNELS + c] += samples[src * CHANNELS + c] * gain;
        }
    }
}

/// 把交错立体声 `src` 线性插值拉伸 / 压缩成 `out_frames` 帧（变速：音调跟着变，和导出的
/// `asetrate` 一样）。
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "帧数很小、下标非负"
)]
pub fn resample_linear(src: &[f32], out_frames: usize) -> Vec<f32> {
    let in_frames = src.len() / CHANNELS;
    let mut out = vec![0.0; out_frames * CHANNELS];
    if in_frames == 0 || out_frames == 0 {
        return out;
    }
    let step = in_frames as f64 / out_frames as f64;
    for frame in 0..out_frames {
        let pos = frame as f64 * step;
        let lo = (pos.floor() as usize).min(in_frames - 1);
        let hi = (lo + 1).min(in_frames - 1);
        let weight = (pos - lo as f64) as f32;
        for ch in 0..CHANNELS {
            let (from, to) = (src[lo * CHANNELS + ch], src[hi * CHANNELS + ch]);
            out[frame * CHANNELS + ch] = (to - from).mul_add(weight, from);
        }
    }
    out
}

/// 限幅到 [-1, 1]。
pub fn clamp_mix(out: &mut [f32]) {
    for v in out {
        *v = v.clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::AudioSegment;
    use ve_core::{AssetId, ClipId, Time};

    fn seg(
        start_ms: i64,
        dur_ms: i64,
        gain: f32,
        fade_in_ms: i64,
        fade_out_ms: i64,
    ) -> AudioSegment {
        let ms = Time::from_millis;
        AudioSegment {
            asset: AssetId(1),
            clip: ClipId(1),
            source_start: Time::ZERO,
            timeline_start: ms(start_ms),
            duration: ms(dur_ms),
            gain,
            fade_in: ms(fade_in_ms),
            fade_out: ms(fade_out_ms),
            sound_start: ms(start_ms),
            sound_end: ms(start_ms + dur_ms),
            speed: 1.0,
            keep_pitch: true,
            clip_start: ms(start_ms),
            volume: Vec::new(),
            fx: ve_core::AudioFx::NONE,
        }
    }

    #[test]
    fn volume_keyframes_shape_the_gain() {
        let mut s = seg(1000, 2000, 1.0, 0, 0);
        // 片段内 0 秒 0 dB，匀速降到 1 秒 -20 dB
        s.volume = vec![
            ve_core::Key {
                at: Time::ZERO,
                value: 0.0,
                easing: ve_core::Easing::Linear,
            },
            ve_core::Key {
                at: Time::from_secs(1),
                value: -20.0,
                easing: ve_core::Easing::Linear,
            },
        ];
        let ms = Time::from_millis;
        assert!((gain_at(&s, ms(1000)) - 1.0).abs() < 1e-6);
        assert!(
            (gain_at(&s, ms(1500)) - 10f32.powf(-0.5)).abs() < 1e-4,
            "中间 -10 dB"
        );
        assert!(
            (gain_at(&s, ms(2500)) - 0.1).abs() < 1e-5,
            "最后一帧之后保持 -20 dB"
        );
        let samples = vec![1.0f32; 2 * 96_000];
        let mut out = vec![0.0f32; 2 * 48_000];
        mix_into(&mut out, &samples, &s, ms(2000), 48_000);
        assert!(
            (out[0] - 0.1).abs() < 1e-4,
            "混音也按关键帧走，不是固定增益"
        );
    }

    #[test]
    fn fade_envelope_endpoints() {
        let s = seg(0, 1000, 1.0, 100, 200);
        let ms = Time::from_millis;
        assert!(gain_at(&s, ms(0)).abs() < 1e-6);
        assert!((gain_at(&s, ms(50)) - 0.5).abs() < 1e-3);
        assert!((gain_at(&s, ms(100)) - 1.0).abs() < 1e-6);
        assert!((gain_at(&s, ms(500)) - 1.0).abs() < 1e-6);
        assert!((gain_at(&s, ms(900)) - 0.5).abs() < 1e-3);
        assert!(gain_at(&s, ms(1000)).abs() < 1e-6);
        let half = seg(0, 1000, 0.5, 0, 0);
        assert!((gain_at(&half, ms(10)) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn mix_places_samples_at_offset_with_gain() {
        let mut out = vec![0.0f32; 20]; // 10 帧
        let s = seg(0, 1000, 0.5, 0, 0);
        let samples = vec![1.0f32; 6]; // 3 帧
        // 输出从时间轴 0 开始，片段样本从时间轴 0 开始；在 48k 下偏移 2 帧 → 让片段从第 2 帧开始
        let mut s2 = s;
        s2.timeline_start = Time::from_samples(2, 48_000);
        mix_into(&mut out, &samples, &s2, Time::ZERO, 48_000);
        assert_eq!(&out[..4], &[0.0, 0.0, 0.0, 0.0]);
        assert_eq!(&out[4..10], &[0.5; 6]);
        assert_eq!(&out[10..], &[0.0; 10]);
    }

    #[test]
    fn mix_clips_segment_outside_output() {
        let mut out = vec![0.0f32; 4]; // 2 帧，时间轴 [1 帧, 3 帧)
        let s = seg(0, 1000, 1.0, 0, 0);
        mix_into(
            &mut out,
            &[1.0; 8],
            &s,
            Time::from_samples(1, 48_000),
            48_000,
        );
        assert_eq!(out, vec![1.0; 4]);
        let mut later = vec![0.0f32; 4];
        mix_into(&mut later, &[1.0; 8], &s, Time::from_secs(5), 48_000);
        assert_eq!(later, vec![0.0; 4]);
    }

    #[test]
    fn mix_sums_and_clamps() {
        let mut out = vec![0.0f32; 4];
        let s = seg(0, 1000, 1.0, 0, 0);
        mix_into(&mut out, &[0.7; 4], &s, Time::ZERO, 48_000);
        mix_into(&mut out, &[0.7; 4], &s, Time::ZERO, 48_000);
        assert!((out[0] - 1.4).abs() < 1e-6);
        clamp_mix(&mut out);
        assert_eq!(out, vec![1.0; 4]);
        let mut neg = vec![-3.0f32, 0.25];
        clamp_mix(&mut neg);
        assert_eq!(neg, vec![-1.0, 0.25]);
    }

    #[test]
    fn resample_stretches_and_squeezes() {
        // 4 帧斜坡（左右声道一样）
        let src: Vec<f32> = [0.0, 1.0, 2.0, 3.0].iter().flat_map(|&v| [v, v]).collect();
        assert_eq!(resample_linear(&src, 4), src, "同样长度原样");
        // 2 倍速：一半的帧，取 0、2
        assert_eq!(resample_linear(&src, 2), [0.0, 0.0, 2.0, 2.0]);
        // 慢放到 8 帧：中间插值
        let slow = resample_linear(&src, 8);
        assert_eq!(slow.len(), 16);
        assert!((slow[2] - 0.5).abs() < 1e-6, "{slow:?}");
        assert!(resample_linear(&[], 3).iter().all(|v| *v == 0.0));
    }
}
