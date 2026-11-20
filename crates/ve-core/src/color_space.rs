//! 色彩管理（达芬奇 / Premiere 的「输入色彩空间」；剪映没有）：HDR（HLG、PQ）和各家 Log 素材
//! 换成 Rec.709 SDR 再调色。按片段指定，放在调色链最前面一起烘焙进查找表，预览、导出一样。
//!
//! 每种都是：解码成线性光 → 换色域到 Rec.709 原色 → 色调映射（高光压进 0–1）→ Rec.709 编码。
//! 公式按各家公开的白皮书（BT.2100、Sony S-Log3、Panasonic V-Log、ARRI `LogC3` EI 800）。

#![expect(
    clippy::excessive_precision,
    reason = "常数照抄各家白皮书的位数，方便对照"
)]

use serde::{Deserialize, Serialize};

/// 片段素材的色彩空间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InputSpace {
    /// HDR · HLG（BT.2100，BT.2020 色域）。
    Hlg,
    /// HDR · PQ（SMPTE ST 2084，BT.2020 色域）。
    Pq,
    /// Sony S-Log3 / S-Gamut3.Cine。
    SLog3,
    /// Panasonic V-Log / V-Gamut。
    VLog,
    /// ARRI LogC3（EI 800）/ ALEXA Wide Gamut。
    LogC3,
}

type Matrix = [[f32; 3]; 3];

/// HLG 1000 nit 峰值按参考白 203 nit 归一后的亮度。
const HLG_PEAK: f32 = 1000.0 / 203.0;

/// BT.2020 → BT.709。
const BT2020_TO_709: Matrix = [
    [1.660_491, -0.587_641, -0.072_850],
    [-0.124_550, 1.132_900, -0.008_349],
    [-0.018_151, -0.100_579, 1.118_730],
];
/// S-Gamut3.Cine → BT.709。
const SGAMUT3_CINE_TO_709: Matrix = [
    [1.626_947, -0.540_139, -0.086_809],
    [-0.178_516, 1.417_941, -0.239_425],
    [-0.044_436, -0.195_920, 1.240_356],
];
/// V-Gamut → BT.709。
const VGAMUT_TO_709: Matrix = [
    [1.806_576, -0.695_697, -0.110_879],
    [-0.170_090, 1.305_955, -0.135_865],
    [-0.025_206, -0.154_468, 1.179_674],
];
/// ALEXA Wide Gamut 3 → BT.709。
const AWG3_TO_709: Matrix = [
    [1.617_523, -0.537_287, -0.080_237],
    [-0.070_573, 1.334_613, -0.264_040],
    [-0.021_102, -0.226_954, 1.248_056],
];

fn mul(m: &Matrix, c: [f32; 3]) -> [f32; 3] {
    m.map(|row| row[0] * c[0] + row[1] * c[1] + row[2] * c[2])
}

/// HLG 反 OETF：信号 → 场景线性（0–1）。
fn hlg_to_linear(e: f32) -> f32 {
    const A: f32 = 0.178_832_77;
    const B: f32 = 0.284_668_92;
    const C: f32 = 0.559_910_73;
    if e <= 0.5 {
        e * e / 3.0
    } else {
        (((e - C) / A).exp() + B) / 12.0
    }
}

/// PQ EOTF：信号 → 亮度，按 SDR 参考白 203 nit 归一（203 nit = 1）。
fn pq_to_linear(e: f32) -> f32 {
    const M1: f32 = 0.159_301_76;
    const M2: f32 = 78.843_75;
    const C1: f32 = 0.835_937_5;
    const C2: f32 = 18.851_563;
    const C3: f32 = 18.6875;
    let p = e.max(0.0).powf(1.0 / M2);
    let y = ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1);
    y * 10_000.0 / 203.0
}

fn slog3_to_linear(x: f32) -> f32 {
    let code = x * 1023.0;
    if code >= 171.210_3 {
        10f32.powf((code - 420.0) / 261.5) * (0.18 + 0.01) - 0.01
    } else {
        (code - 95.0) * 0.01125 / (171.210_3 - 95.0)
    }
}

fn vlog_to_linear(x: f32) -> f32 {
    if x < 0.181 {
        (x - 0.125) / 5.6
    } else {
        10f32.powf((x - 0.598_206) / 0.241_514) - 0.008_73
    }
}

fn logc3_to_linear(x: f32) -> f32 {
    const CUT: f32 = 0.010_591;
    const A: f32 = 5.555_556;
    const B: f32 = 0.052_272;
    const C: f32 = 0.247_190;
    const D: f32 = 0.385_537;
    const E: f32 = 5.367_655;
    const F: f32 = 0.092_809;
    if x > E * CUT + F {
        (10f32.powf((x - D) / C) - B) / A
    } else {
        (x - F) / E
    }
}

/// 色调映射（扩展 Reinhard，白点 `white`）：线性亮度压进 0–1，暗部基本不动。
fn tone(x: f32, white: f32) -> f32 {
    let x = x.max(0.0);
    (x * (1.0 + x / (white * white)) / (1.0 + x)).min(1.0)
}

/// Rec.709 OETF：线性 → 信号。
fn rec709_oetf(l: f32) -> f32 {
    let l = l.clamp(0.0, 1.0);
    if l < 0.018 {
        4.5 * l
    } else {
        1.099 * l.powf(0.45) - 0.099
    }
}

impl InputSpace {
    /// 全部（属性面板里的顺序，「Rec.709」不算在内）。
    pub const ALL: [Self; 5] = [Self::Hlg, Self::Pq, Self::SLog3, Self::VLog, Self::LogC3];

    /// 名字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hlg => "HLG",
            Self::Pq => "PQ",
            Self::SLog3 => "S-Log3",
            Self::VLog => "V-Log",
            Self::LogC3 => "LogC3",
        }
    }

    /// 一个颜色（0–1 的信号值）换成 Rec.709 SDR。
    #[must_use]
    pub fn to_rec709(self, rgb: [f32; 3]) -> [f32; 3] {
        let (linear, gamut, white): (fn(f32) -> f32, &Matrix, f32) = match self {
            Self::Hlg => (hlg_to_linear, &BT2020_TO_709, HLG_PEAK),
            Self::Pq => (pq_to_linear, &BT2020_TO_709, 10.0),
            Self::SLog3 => (slog3_to_linear, &SGAMUT3_CINE_TO_709, 4.0),
            Self::VLog => (vlog_to_linear, &VGAMUT_TO_709, 4.0),
            Self::LogC3 => (logc3_to_linear, &AWG3_TO_709, 4.0),
        };
        let lin = mul(gamut, rgb.map(|v| linear(v.clamp(0.0, 1.0))));
        // HLG 是场景光：按 1000 nit 显示器的 1.2 系统伽马还原显示亮度，再按参考白 203 nit 归一
        let lin = if self == Self::Hlg {
            lin.map(|v| v.max(0.0).powf(1.2) * HLG_PEAK)
        } else {
            lin
        };
        lin.map(|v| rec709_oetf(tone(v, white)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(space: InputSpace, v: f32) -> f32 {
        space.to_rec709([v; 3])[1]
    }

    #[test]
    fn every_space_is_monotonic_bounded_and_keeps_gray_gray() {
        for s in InputSpace::ALL {
            let mut last = -1.0;
            for i in 0..=20 {
                #[expect(clippy::cast_precision_loss, reason = "测试")]
                let v = i as f32 / 20.0;
                let out = s.to_rec709([v; 3]);
                assert!(
                    out.iter().all(|c| (0.0..=1.0).contains(c)),
                    "{s:?} {v} → {out:?}"
                );
                // 灰色进灰色出（色域矩阵每行加起来是 1）
                assert!(
                    (out[0] - out[1]).abs() < 0.01 && (out[1] - out[2]).abs() < 0.01,
                    "{s:?} {out:?}"
                );
                assert!(out[1] >= last - 1e-4, "{s:?} 不是单调的：{v}");
                last = out[1];
            }
        }
    }

    #[test]
    fn log_middle_gray_lands_near_rec709_middle_gray() {
        // 各家 18% 灰的码值：S-Log3 420/1023、V-Log 0.423、LogC3 0.391
        for (s, code) in [
            (InputSpace::SLog3, 420.0 / 1023.0),
            (InputSpace::VLog, 0.423),
            (InputSpace::LogC3, 0.391),
        ] {
            let out = gray(s, code);
            assert!((0.33..=0.5).contains(&out), "{s:?} 中灰 → {out}");
        }
    }

    #[test]
    fn hdr_reference_white_is_bright_but_not_clipped() {
        // HLG 75% 和 PQ 58%（203 nit）都是参考白
        let hlg = gray(InputSpace::Hlg, 0.75);
        let pq = gray(InputSpace::Pq, 0.58);
        assert!((0.6..0.98).contains(&hlg), "HLG 参考白 {hlg}");
        assert!((0.6..0.98).contains(&pq), "PQ 参考白 {pq}");
        assert!(gray(InputSpace::Pq, 1.0) > pq, "高光还在");
    }
}
