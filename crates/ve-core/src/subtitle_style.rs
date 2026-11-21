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
    /// 斜体。
    pub italic: bool,
    /// 下划线。
    pub underline: bool,
    /// 字间距（以 1080 高为基准的像素）。
    pub spacing: f32,
    /// 文字（连同描边）的不透明度，0–1。
    pub opacity: f32,
    /// 阴影离文字多远（以 1080 高为基准的像素，往右下；0 是没有阴影）。
    pub shadow: f32,
    /// 阴影颜色 0xRRGGBB。
    pub shadow_color: u32,
    /// 阴影不透明度，0–1。
    pub shadow_opacity: f32,
    /// 背景框颜色 0xRRGGBB。
    pub background_color: u32,
    /// 背景框不透明度，0–1（0 是没有背景框）。有背景框时不画描边（导出的 ASS 也是这样）。
    pub background_opacity: f32,
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
            italic: false,
            underline: false,
            spacing: 0.0,
            opacity: 1.0,
            shadow: 0.0,
            shadow_color: 0x00_0000,
            shadow_opacity: 0.6,
            background_color: 0x00_0000,
            background_opacity: 0.0,
        }
    }
}

impl SubtitleStyle {
    /// 字号比例的范围。
    pub const SIZE_RANGE: (f32, f32) = (0.02, 0.2);
    /// 底边距比例的范围。
    pub const MARGIN_RANGE: (f32, f32) = (0.0, 0.9);
    /// 描边宽度的范围（1080 基准像素）。
    pub const OUTLINE_RANGE: (f32, f32) = (0.0, 20.0);
    /// 字间距的范围（1080 基准像素）。
    pub const SPACING_RANGE: (f32, f32) = (0.0, 60.0);
    /// 阴影距离的范围（1080 基准像素）。
    pub const SHADOW_RANGE: (f32, f32) = (0.0, 30.0);

    /// 是不是默认样式（默认时不写进工程文件）。
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 各项都在范围内、颜色是 24 位、字体名不空。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let within = |v: f32, (a, b): (f32, f32)| (a..=b).contains(&v);
        within(self.size_ratio, Self::SIZE_RANGE)
            && within(self.outline, Self::OUTLINE_RANGE)
            && within(self.margin_ratio, Self::MARGIN_RANGE)
            && within(self.spacing, Self::SPACING_RANGE)
            && within(self.shadow, Self::SHADOW_RANGE)
            && within(self.opacity, (0.0, 1.0))
            && within(self.shadow_opacity, (0.0, 1.0))
            && within(self.background_opacity, (0.0, 1.0))
            && [
                self.color,
                self.outline_color,
                self.shadow_color,
                self.background_color,
            ]
            .iter()
            .all(|c| *c <= 0xFF_FFFF)
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

    /// 在高 `height` 像素的画面上的字间距。
    #[must_use]
    pub fn spacing_px(&self, height: u32) -> f64 {
        f64::from(self.spacing) * f64::from(height) / 1080.0
    }

    /// 在高 `height` 像素的画面上的阴影距离。
    #[must_use]
    pub fn shadow_px(&self, height: u32) -> f64 {
        f64::from(self.shadow) * f64::from(height) / 1080.0
    }

    /// 有背景框。
    #[must_use]
    pub fn has_background(&self) -> bool {
        self.background_opacity > 0.0
    }

    /// 背景框比字四周多出来的边（字号的两成）。
    #[must_use]
    pub fn padding_px(&self, height: u32) -> f64 {
        (self.font_px(height) * 0.2).round()
    }

    /// 在高 `height` 像素的画面上的底边距。
    #[must_use]
    pub fn margin_px(&self, height: u32) -> f64 {
        (f64::from(height) * f64::from(self.margin_ratio)).round()
    }

    /// ASS 的颜色写法：`&H00BBGGRR`。
    #[must_use]
    pub fn ass_color(rgb: u32) -> String {
        Self::ass_color_alpha(rgb, 1.0)
    }

    /// 带不透明度的 ASS 颜色：`&HAABBGGRR`（AA 是透明度，00 不透明、FF 全透明）。
    #[must_use]
    pub fn ass_color_alpha(rgb: u32, opacity: f32) -> String {
        let (r, g, b) = ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF);
        format!(
            "&H{:02X}{b:02X}{g:02X}{r:02X}",
            Self::ass_alpha_byte(opacity)
        )
    }

    /// 不透明度 → ASS 的透明度字节（00 不透明、FF 全透明）。
    #[must_use]
    pub fn ass_alpha_byte(opacity: f32) -> u8 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "0–255"
        )]
        let a = ((1.0 - opacity.clamp(0.0, 1.0)) * 255.0).round() as u8;
        a
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
