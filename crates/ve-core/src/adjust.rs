//! 基础调色（剪映属性面板「调整」）：亮度、对比度、饱和度、色温、色调，在显卡合成时逐像素做。

use serde::{Deserialize, Serialize};

/// 一个片段的调色参数，每项 -1 到 1，0 是不调（界面上显示成 -100 到 100）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorAdjust {
    /// 亮度。
    pub brightness: f32,
    /// 对比度。
    pub contrast: f32,
    /// 饱和度（-1 是黑白）。
    pub saturation: f32,
    /// 色温（正数偏暖、负数偏冷）。
    pub temperature: f32,
    /// 色调（正数偏品红、负数偏绿）。
    pub tint: f32,
}

/// 调色的五项（界面上的顺序）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdjustItem {
    /// 亮度。
    Brightness,
    /// 对比度。
    Contrast,
    /// 饱和度。
    Saturation,
    /// 色温。
    Temperature,
    /// 色调。
    Tint,
}

impl AdjustItem {
    /// 全部五项。
    pub const ALL: [Self; 5] = [
        Self::Brightness,
        Self::Contrast,
        Self::Saturation,
        Self::Temperature,
        Self::Tint,
    ];

    /// 名字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Brightness => "亮度",
            Self::Contrast => "对比度",
            Self::Saturation => "饱和度",
            Self::Temperature => "色温",
            Self::Tint => "色调",
        }
    }

    /// 读这一项。
    #[must_use]
    pub const fn get(self, a: &ColorAdjust) -> f32 {
        match self {
            Self::Brightness => a.brightness,
            Self::Contrast => a.contrast,
            Self::Saturation => a.saturation,
            Self::Temperature => a.temperature,
            Self::Tint => a.tint,
        }
    }

    /// 写这一项（夹到 -1–1）。
    pub const fn set(self, a: &mut ColorAdjust, value: f32) {
        let v = value.clamp(-1.0, 1.0);
        match self {
            Self::Brightness => a.brightness = v,
            Self::Contrast => a.contrast = v,
            Self::Saturation => a.saturation = v,
            Self::Temperature => a.temperature = v,
            Self::Tint => a.tint = v,
        }
    }
}

impl ColorAdjust {
    /// 不调色。
    pub const NONE: Self = Self {
        brightness: 0.0,
        contrast: 0.0,
        saturation: 0.0,
        temperature: 0.0,
        tint: 0.0,
    };

    /// 是不是没调（不写进工程文件、合成时跳过）。
    #[must_use]
    pub fn is_identity(&self) -> bool {
        *self == Self::NONE
    }

    /// 每项都是 -1 到 1 的有限数。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        AdjustItem::ALL
            .iter()
            .all(|i| (-1.0..=1.0).contains(&i.get(self)))
    }

    /// 对一个颜色（sRGB 0–1、不预乘）调色；和合成器着色器里的公式一样，供测试和 CPU 端对照。
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let lifted = rgb.map(|c| c + self.brightness * 0.25);
        let gain = 1.0 + self.contrast;
        let contrasted = lifted.map(|c| (c - 0.5).mul_add(gain, 0.5));
        let [red, green, blue] = contrasted;
        let luma = 0.0722f32.mul_add(blue, 0.2126f32.mul_add(red, 0.7152 * green));
        let sat = 1.0 + self.saturation;
        let [r2, g2, b2] = contrasted.map(|c| (c - luma).mul_add(sat, luma));
        [
            r2 + self.temperature * 0.1 + self.tint * 0.05,
            g2 - self.tint * 0.1,
            b2 + self.tint.mul_add(0.05, -self.temperature * 0.1),
        ]
        .map(|c| c.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_leaves_colors_alone() {
        let c = [0.2, 0.5, 0.9];
        let out = ColorAdjust::NONE.apply(c);
        for (a, b) in out.iter().zip(c) {
            assert!((a - b).abs() < 1e-6);
        }
        assert!(ColorAdjust::default().is_identity());
    }

    #[test]
    fn each_knob_moves_colors_the_expected_way() {
        let with = |item: AdjustItem, v: f32| {
            let mut a = ColorAdjust::NONE;
            item.set(&mut a, v);
            a
        };
        let gray = [0.5, 0.5, 0.5];
        assert!(with(AdjustItem::Brightness, 0.4).apply(gray)[0] > 0.55);
        let mono = with(AdjustItem::Saturation, -1.0).apply([1.0, 0.0, 0.0]);
        assert!(
            (mono[0] - mono[1]).abs() < 1e-6 && (mono[1] - mono[2]).abs() < 1e-6,
            "黑白"
        );
        let warm = with(AdjustItem::Temperature, 1.0).apply(gray);
        assert!(warm[0] > warm[2], "偏暖：红多蓝少");
        let green = with(AdjustItem::Tint, -1.0).apply(gray);
        assert!(green[1] > green[0], "偏绿");
        let punchy = with(AdjustItem::Contrast, 1.0).apply([0.3, 0.7, 0.5]);
        assert!(punchy[0] < 0.3 && punchy[1] > 0.7, "对比度拉开");
        // 夹到 -1–1；非法值
        let mut clamped = with(AdjustItem::Contrast, 9.0);
        assert_eq!(clamped.contrast, 1.0);
        assert!(clamped.is_valid());
        clamped.tint = f32::NAN;
        assert!(!clamped.is_valid());
        assert_eq!(AdjustItem::Tint.label(), "色调");
    }
}
