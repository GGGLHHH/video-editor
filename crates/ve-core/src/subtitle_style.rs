//! 字幕样式（剪映属性面板「文本」，默认「应用到全部字幕」）：整个工程一份，预览和导出共用。

use serde::{Deserialize, Serialize};

/// 字幕样式。字号、边距按画面高的比例存，换分辨率、换比例后大小跟着走。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleStyle {
    /// 字体。
    pub font: String,
    /// 字号 = 画面高 × 该比例。
    pub size_ratio: f32,
    /// 描边宽度（以 1080 高为基准的像素，0 是不描边）。
    pub outline: f32,
    /// 底边距 = 画面高 × 该比例。
    pub margin_ratio: f32,
    /// 文字颜色 0xRRGGBB。
    pub color: u32,
    /// 描边颜色 0xRRGGBB。
    pub outline_color: u32,
    /// 粗体。
    pub bold: bool,
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            font: "Microsoft YaHei".into(),
            size_ratio: 0.055,
            outline: 2.5,
            margin_ratio: 0.05,
            color: 0xFF_FFFF,
            outline_color: 0x00_0000,
            bold: false,
        }
    }
}

impl SubtitleStyle {
    /// 字号比例的范围。
    pub const SIZE_RANGE: (f32, f32) = (0.02, 0.2);
    /// 底边距比例的范围。
    pub const MARGIN_RANGE: (f32, f32) = (0.0, 0.9);

    /// 是不是默认样式（默认时不写进工程文件）。
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 各项都在范围内、颜色是 24 位、字体名不空。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        (Self::SIZE_RANGE.0..=Self::SIZE_RANGE.1).contains(&self.size_ratio)
            && (0.0..=20.0).contains(&self.outline)
            && (Self::MARGIN_RANGE.0..=Self::MARGIN_RANGE.1).contains(&self.margin_ratio)
            && self.color <= 0xFF_FFFF
            && self.outline_color <= 0xFF_FFFF
            && !self.font.trim().is_empty()
    }

    /// 在高 `height` 像素的画面上的字号。
    #[must_use]
    pub fn font_px(&self, height: u32) -> f64 {
        (f64::from(height) * f64::from(self.size_ratio)).round()
    }

    /// 在高 `height` 像素的画面上的描边宽度。
    #[must_use]
    pub fn outline_px(&self, height: u32) -> f64 {
        f64::from(self.outline) * f64::from(height) / 1080.0
    }

    /// 在高 `height` 像素的画面上的底边距。
    #[must_use]
    pub fn margin_px(&self, height: u32) -> f64 {
        (f64::from(height) * f64::from(self.margin_ratio)).round()
    }

    /// ASS 的颜色写法：`&H00BBGGRR`。
    #[must_use]
    pub fn ass_color(rgb: u32) -> String {
        let (r, g, b) = ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF);
        format!("&H00{b:02X}{g:02X}{r:02X}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_scale_with_height_and_validate() {
        let s = SubtitleStyle::default();
        assert!(s.is_default() && s.is_valid());
        assert!((s.font_px(1080) - 59.0).abs() < 1e-9);
        assert!((s.outline_px(540) - 1.25).abs() < 1e-9);
        assert!((s.margin_px(1080) - 54.0).abs() < 1e-9);
        assert_eq!(SubtitleStyle::ass_color(0xFF_D700), "&H0000D7FF");
        let big = SubtitleStyle {
            size_ratio: 0.5,
            ..SubtitleStyle::default()
        };
        assert!(!big.is_valid());
        let blank = SubtitleStyle {
            font: " ".into(),
            ..SubtitleStyle::default()
        };
        assert!(!blank.is_valid());
    }
}
