//! 片段的声音处理（剪映「音频」标签的「音频降噪」，加上均衡和压缩）。
//!
//! 预览和导出都用同一串 ffmpeg 音频滤镜（[`AudioFx::filter_spec`]）：预览把解出来的声音过一遍
//! 滤镜图，导出写进滤镜图里，两边听起来一样。降噪（`afftdn`）有固定 25 毫秒的延迟，两边都按
//! [`AudioFx::latency_samples`] 补回来，声音不会比画面晚。

use serde::{Deserialize, Serialize};

/// 均衡三段的中心频率（Hz）：低音搁架、中音峰值、高音搁架。
pub const EQ_FREQS: [u32; 3] = [100, 1000, 8000];
/// 均衡每段的增益范围（dB）。
pub const EQ_RANGE_DB: (f32, f32) = (-12.0, 12.0);
/// 均衡三段的名字。
pub const EQ_LABELS: [&str; 3] = ["低音", "中音", "高音"];

/// 片段的声音处理。全零 = 不处理。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct AudioFx {
    /// 降噪强度 0–1（0 = 关）。
    #[serde(default)]
    pub denoise: f32,
    /// 均衡：低音、中音、高音的增益（dB）。
    #[serde(default)]
    pub eq: [f32; 3],
    /// 压缩强度 0–1（0 = 关）：大声的地方压下来、整体再提一点，听起来更平稳。
    #[serde(default)]
    pub compress: f32,
}

impl AudioFx {
    /// 不处理。
    pub const NONE: Self = Self {
        denoise: 0.0,
        eq: [0.0; 3],
        compress: 0.0,
    };

    /// 没有任何处理（工程文件里不写）。
    #[must_use]
    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }

    /// 数值都在范围里（不是 NaN）。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        (0.0..=1.0).contains(&self.denoise)
            && (0.0..=1.0).contains(&self.compress)
            && self
                .eq
                .iter()
                .all(|g| (EQ_RANGE_DB.0..=EQ_RANGE_DB.1).contains(g))
    }

    /// ffmpeg 音频滤镜串（不含延迟补偿）；不处理时为 `None`。
    #[must_use]
    pub fn filter_spec(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.denoise > 0.0 {
            // 降噪量 6–30 dB；噪声底按 -50 dB 起估
            parts.push(format!(
                "afftdn=nr={:.2}:nf=-50",
                6.0 + 24.0 * self.denoise.clamp(0.0, 1.0)
            ));
        }
        let [low, mid, high] = self.eq;
        if low != 0.0 {
            parts.push(format!("bass=g={low:.2}:f={}", EQ_FREQS[0]));
        }
        if mid != 0.0 {
            parts.push(format!("equalizer=f={}:t=q:w=1:g={mid:.2}", EQ_FREQS[1]));
        }
        if high != 0.0 {
            parts.push(format!("treble=g={high:.2}:f={}", EQ_FREQS[2]));
        }
        if self.compress > 0.0 {
            let c = self.compress.clamp(0.0, 1.0);
            // 门限 -12 → -30 dB，压缩比 2 → 8，再补回一部分音量
            let threshold = 10f32.powf((-12.0 - 18.0 * c) / 20.0);
            parts.push(format!(
                "acompressor=threshold={threshold:.5}:ratio={:.2}:attack=5:release=120:makeup={:.3}",
                2.0 + 6.0 * c,
                1.0 + 2.0 * c
            ));
        }
        (!parts.is_empty()).then(|| parts.join(","))
    }

    /// 滤镜串在采样率 `rate` 下推迟了多少个样本（降噪 `afftdn` 固定 25 毫秒，其余没有延迟）。
    #[must_use]
    pub fn latency_samples(&self, rate: u32) -> usize {
        if self.denoise > 0.0 {
            usize::try_from(rate).unwrap_or(0) / 40
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_do_means_no_filter() {
        assert!(AudioFx::NONE.is_none());
        assert_eq!(AudioFx::NONE.filter_spec(), None);
        assert_eq!(AudioFx::NONE.latency_samples(48_000), 0);
    }

    #[test]
    fn each_part_adds_its_filter_in_order() {
        let fx = AudioFx {
            denoise: 0.5,
            eq: [3.0, 0.0, -2.0],
            compress: 1.0,
        };
        let spec = fx.filter_spec().unwrap();
        let names: Vec<&str> = spec
            .split(',')
            .map(|f| f.split('=').next().unwrap())
            .collect();
        assert_eq!(names, ["afftdn", "bass", "treble", "acompressor"]);
        assert!(spec.contains("nr=18.00"), "{spec}");
        assert!(spec.contains("ratio=8.00"), "{spec}");
        assert_eq!(fx.latency_samples(48_000), 1200);
        assert_eq!(fx.latency_samples(44_100), 1102);
    }

    #[test]
    fn values_out_of_range_are_invalid() {
        let mut fx = AudioFx::NONE;
        fx.eq[1] = 13.0;
        assert!(!fx.is_valid());
        fx.eq[1] = 0.0;
        fx.denoise = f32::NAN;
        assert!(!fx.is_valid());
        fx.denoise = 1.0;
        assert!(fx.is_valid());
    }
}
