//! 画面的「外观」（剪映属性面板「画面」里除了位置大小以外的东西）：混合模式、圆角、描边、阴影、
//! 蒙版、色度抠图。都在显卡合成时逐像素做（`ve-render`），预览和导出同一个合成器。
//!
//! 长度单位：圆角、描边、阴影的大小和距离都按画布短边的比例（0.01 = 短边的 1%），
//! 换分辨率导出时看起来一样；蒙版的位置和大小按图层显示部分的比例。

use serde::{Deserialize, Serialize};

/// 混合模式（剪映「混合」组的「混合模式」）。公式见 W3C Compositing and Blending。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    /// 正常。
    #[default]
    Normal,
    /// 变暗。
    Darken,
    /// 滤色。
    Screen,
    /// 叠加。
    Overlay,
    /// 正片叠底。
    Multiply,
    /// 变亮。
    Lighten,
    /// 强光。
    HardLight,
    /// 柔光。
    SoftLight,
    /// 线性加深。
    LinearBurn,
    /// 颜色加深。
    ColorBurn,
    /// 颜色减淡。
    ColorDodge,
}

impl BlendMode {
    /// 全部（剪映下拉框里的顺序）。
    pub const ALL: [Self; 11] = [
        Self::Normal,
        Self::Darken,
        Self::Screen,
        Self::Overlay,
        Self::Multiply,
        Self::Lighten,
        Self::HardLight,
        Self::SoftLight,
        Self::LinearBurn,
        Self::ColorBurn,
        Self::ColorDodge,
    ];

    /// 名字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "正常",
            Self::Darken => "变暗",
            Self::Screen => "滤色",
            Self::Overlay => "叠加",
            Self::Multiply => "正片叠底",
            Self::Lighten => "变亮",
            Self::HardLight => "强光",
            Self::SoftLight => "柔光",
            Self::LinearBurn => "线性加深",
            Self::ColorBurn => "颜色加深",
            Self::ColorDodge => "颜色减淡",
        }
    }

    /// 着色器里的编号。
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Darken => 1,
            Self::Screen => 2,
            Self::Overlay => 3,
            Self::Multiply => 4,
            Self::Lighten => 5,
            Self::HardLight => 6,
            Self::SoftLight => 7,
            Self::LinearBurn => 8,
            Self::ColorBurn => 9,
            Self::ColorDodge => 10,
        }
    }

    /// 一个通道的混合结果 B(底色, 图层色)（不预乘，0–1），和着色器同一套公式，供测试对照。
    #[must_use]
    pub fn channel(self, cb: f32, cs: f32) -> f32 {
        let hard = |b: f32, s: f32| {
            if s <= 0.5 {
                b * 2.0 * s
            } else {
                let s2 = 2.0f32.mul_add(s, -1.0);
                b + s2 - b * s2
            }
        };
        match self {
            Self::Normal => cs,
            Self::Darken => cb.min(cs),
            Self::Screen => cb + cs - cb * cs,
            Self::Overlay => hard(cs, cb),
            Self::Multiply => cb * cs,
            Self::Lighten => cb.max(cs),
            Self::HardLight => hard(cb, cs),
            Self::SoftLight => {
                if cs <= 0.5 {
                    cb - (2.0f32.mul_add(-cs, 1.0)) * cb * (1.0 - cb)
                } else {
                    let d = if cb <= 0.25 {
                        (16.0f32.mul_add(cb, -12.0).mul_add(cb, 4.0)) * cb
                    } else {
                        cb.sqrt()
                    };
                    2.0f32.mul_add(cs, -1.0).mul_add(d - cb, cb)
                }
            }
            Self::LinearBurn => (cb + cs - 1.0).max(0.0),
            Self::ColorBurn => {
                if cb >= 1.0 {
                    1.0
                } else if cs <= 0.0 {
                    0.0
                } else {
                    1.0 - ((1.0 - cb) / cs).min(1.0)
                }
            }
            Self::ColorDodge => {
                if cb <= 0.0 {
                    0.0
                } else if cs >= 1.0 {
                    1.0
                } else {
                    (cb / (1.0 - cs)).min(1.0)
                }
            }
        }
    }
}

/// 描边（画面外沿一圈）。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    /// 颜色 0xRRGGBB。
    pub color: u32,
    /// 粗细（画布短边的比例，0–0.1）。
    pub width: f32,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            color: 0xFF_FFFF,
            width: 0.01,
        }
    }
}

/// 阴影（画面下面一块偏移、模糊的影子）。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    /// 颜色 0xRRGGBB。
    pub color: u32,
    /// 不透明度（0–1）。
    pub opacity: f32,
    /// 模糊半径（画布短边的比例，0–0.1）。
    pub blur: f32,
    /// 距离（画布短边的比例，0–0.1）。
    pub distance: f32,
    /// 方向（度，0 = 向右，顺时针；剪映默认往右下）。
    pub angle: f32,
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            color: 0x00_0000,
            opacity: 0.6,
            blur: 0.02,
            distance: 0.015,
            angle: 45.0,
        }
    }
}

/// 蒙版形状（剪映「蒙版」）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MaskShape {
    /// 线性：一条线把画面分成两半，留一边。
    #[default]
    Linear,
    /// 镜面：一条带子，留中间。
    Mirror,
    /// 圆形（椭圆）。
    Circle,
    /// 矩形（可圆角）。
    Rectangle,
    /// 爱心。
    Heart,
    /// 五角星。
    Star,
    /// 文字：字的形状（字写在片段的 [`crate::Clip::mask_text`] 里）。
    Text,
}

impl MaskShape {
    /// 全部（剪映里的顺序）。
    pub const ALL: [Self; 7] = [
        Self::Linear,
        Self::Mirror,
        Self::Circle,
        Self::Rectangle,
        Self::Heart,
        Self::Star,
        Self::Text,
    ];

    /// 名字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Linear => "线性",
            Self::Mirror => "镜面",
            Self::Circle => "圆形",
            Self::Rectangle => "矩形",
            Self::Heart => "爱心",
            Self::Star => "星形",
            Self::Text => "文字",
        }
    }

    /// 着色器里的编号。
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::Linear => 0,
            Self::Mirror => 1,
            Self::Circle => 2,
            Self::Rectangle => 3,
            Self::Heart => 4,
            Self::Star => 5,
            Self::Text => 6,
        }
    }
}

/// 蒙版。位置、大小按图层显示部分的比例（中心 0,0；宽高 1 = 和画面一样大）。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    /// 形状。
    pub shape: MaskShape,
    /// 中心（相对画面中心，-0.5–0.5 是画面内）。
    pub x: f32,
    /// 中心 y（向下为正）。
    pub y: f32,
    /// 宽（画面宽的比例；线性 / 镜面不用宽）。
    pub width: f32,
    /// 高（画面高的比例；镜面是带子的宽度）。
    pub height: f32,
    /// 旋转（度，顺时针）。
    pub rotation: f32,
    /// 羽化（0–1：边缘渐变的宽度，按画面短边的一半算）。
    pub feather: f32,
    /// 矩形的圆角（0–1）。
    pub round: f32,
    /// 反转：留外面、去掉里面。
    pub invert: bool,
}

impl Mask {
    /// 新建某个形状的蒙版（剪映的默认大小：居中，约画面的一半）。
    #[must_use]
    pub const fn new(shape: MaskShape) -> Self {
        let (width, height) = match shape {
            MaskShape::Linear => (1.0, 1.0),
            MaskShape::Mirror => (1.0, 0.3),
            MaskShape::Circle | MaskShape::Heart | MaskShape::Star => (0.5, 0.5),
            MaskShape::Rectangle => (0.6, 0.5),
            MaskShape::Text => (0.8, 0.4),
        };
        Self {
            shape,
            x: 0.0,
            y: 0.0,
            width,
            height,
            rotation: 0.0,
            feather: 0.0,
            round: 0.0,
            invert: false,
        }
    }
}

/// 文字蒙版的字（剪映「蒙版 › 文字」）：字按蒙版的宽高等比放进去，留下字的形状。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MaskText {
    /// 字（可以多行）。
    pub text: String,
    /// 字体名（空 = 系统默认的无衬线字体）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub font: String,
}

impl MaskText {
    /// 新选文字蒙版时的字。
    pub const DEFAULT_TEXT: &'static str = "文字";
    /// 最多几个字。
    pub const MAX_CHARS: usize = 50;

    /// 有字、不太长。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.text.trim().is_empty() && self.text.chars().count() <= Self::MAX_CHARS
    }
}

/// 色度抠图（剪映「抠像 › 色度抠图」）：把接近某个颜色的像素抠成透明。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChromaKey {
    /// 要抠掉的颜色 0xRRGGBB。
    pub color: u32,
    /// 强度（0–1）：越大抠掉的颜色范围越宽。
    pub intensity: f32,
    /// 阴影（0–1）：越大边缘过渡越软，保留的半透明阴影越多。
    pub shadow: f32,
}

impl Default for ChromaKey {
    fn default() -> Self {
        Self {
            color: 0x00_FF00,
            intensity: 0.4,
            shadow: 0.2,
        }
    }
}

impl ChromaKey {
    /// 一个像素（sRGB 0–1、不预乘）抠完之后留下的不透明度（0–1），和着色器同一套公式：
    /// 按 BT.709 的色度（Cb、Cr）距离算，强度定阈值，阴影定过渡宽度。
    #[must_use]
    pub fn keep(&self, rgb: [f32; 3]) -> f32 {
        let key = rgb_unit(self.color);
        let d = chroma(rgb).0 - chroma(key).0;
        let e = chroma(rgb).1 - chroma(key).1;
        let dist = d.hypot(e);
        let (lo, hi) = self.thresholds();
        ((dist - lo) / (hi - lo)).clamp(0.0, 1.0)
    }

    /// 色度距离的两个阈值：小于 `lo` 全透明，大于 `hi` 全保留。
    #[must_use]
    pub fn thresholds(&self) -> (f32, f32) {
        let lo = self.intensity.clamp(0.0, 1.0) * 0.4;
        let hi = lo + self.shadow.clamp(0.0, 1.0).mul_add(0.3, 0.01);
        (lo, hi)
    }
}

/// BT.709 色度（Cb、Cr），各在 -0.5–0.5。
fn chroma([r, g, b]: [f32; 3]) -> (f32, f32) {
    let y = 0.0722f32.mul_add(b, 0.2126f32.mul_add(r, 0.7152 * g));
    ((b - y) / 1.8556, (r - y) / 1.5748)
}

/// 0xRRGGBB → 0–1。
#[must_use]
pub fn rgb_unit(c: u32) -> [f32; 3] {
    let ch = |shift: u32| f32::from(u8::try_from((c >> shift) & 0xFF).unwrap_or(0)) / 255.0;
    [ch(16), ch(8), ch(0)]
}

/// 一个片段最多几个蒙版。
pub const MAX_MASKS: usize = 4;

fn no_extra_masks(m: &[Option<Mask>; MAX_MASKS - 1]) -> bool {
    m.iter().all(Option::is_none)
}

/// 蒙版的数值都在范围里。
fn mask_ok(m: &Mask) -> bool {
    [m.x, m.y, m.rotation].iter().all(|v| v.is_finite())
        && (0.01..=4.0).contains(&m.width)
        && (0.01..=4.0).contains(&m.height)
        && (0.0..=1.0).contains(&m.feather)
        && (0.0..=1.0).contains(&m.round)
}

/// 片段的外观：没设的都是默认（不写进工程文件）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    /// 混合模式。
    pub blend: BlendMode,
    /// 圆角（0–1：1 = 短边的一半，变成胶囊 / 圆）。
    pub radius: f32,
    /// 描边。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Stroke>,
    /// 阴影。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
    /// 蒙版（第一个；关键帧只作用在它上面）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask: Option<Mask>,
    /// 另外加的蒙版（剪映「添加蒙版」，最多一共 [`MAX_MASKS`] 个）：和第一个取并集，
    /// 哪个蒙版留下的地方都显示。前面的格子先用，不留空格。
    #[serde(skip_serializing_if = "no_extra_masks")]
    pub extra_masks: [Option<Mask>; MAX_MASKS - 1],
    /// 色度抠图。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chroma: Option<ChromaKey>,
}

impl Look {
    /// 默认外观（常量版本）。
    pub const NONE: Self = Self {
        blend: BlendMode::Normal,
        radius: 0.0,
        stroke: None,
        shadow: None,
        mask: None,
        extra_masks: [None; MAX_MASKS - 1],
        chroma: None,
    };

    /// 有文字蒙版（第一个或另外的蒙版里有文字形状）。
    #[must_use]
    pub fn uses_text(&self) -> bool {
        self.masks().any(|m| m.shape == MaskShape::Text)
    }

    /// 全部蒙版（第一个和另外的）。
    pub fn masks(&self) -> impl Iterator<Item = Mask> + '_ {
        self.mask
            .into_iter()
            .chain(self.extra_masks.iter().flatten().copied())
    }

    /// 加一个矩形蒙版，返回它是第几个（0 = 第一个）；满了返回 `None`。
    pub fn add_mask(&mut self) -> Option<usize> {
        let new = Mask::new(MaskShape::Rectangle);
        if self.mask.is_none() {
            self.mask = Some(new);
            return Some(0);
        }
        let free = self.extra_masks.iter().position(Option::is_none)?;
        self.extra_masks[free] = Some(new);
        Some(free + 1)
    }

    /// 删掉第 `i` 个蒙版（0 = 第一个），后面的往前挪。
    pub fn remove_mask(&mut self, i: usize) {
        let mut all: Vec<Mask> = self.masks().collect();
        if i < all.len() {
            all.remove(i);
        }
        let mut rest = all.into_iter();
        self.mask = rest.next();
        self.extra_masks = [None; MAX_MASKS - 1];
        for (slot, m) in self.extra_masks.iter_mut().zip(rest) {
            *slot = Some(m);
        }
    }

    /// 什么都没设。
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 各项都在范围里、都是有限数。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let unit = |v: f32| (0.0..=1.0).contains(&v);
        let small = |v: f32| (0.0..=0.1).contains(&v);
        unit(self.radius)
            && self
                .stroke
                .is_none_or(|s| s.color <= 0xFF_FFFF && small(s.width))
            && self.shadow.is_none_or(|s| {
                s.color <= 0xFF_FFFF
                    && unit(s.opacity)
                    && small(s.blur)
                    && small(s.distance)
                    && s.angle.is_finite()
            })
            && self.masks().all(|m| mask_ok(&m))
            && (self.mask.is_some() || no_extra_masks(&self.extra_masks))
            && self
                .chroma
                .is_none_or(|c| c.color <= 0xFF_FFFF && unit(c.intensity) && unit(c.shadow))
    }

    /// 画面外要多画出来的宽度（画布短边的比例）：描边粗细和阴影的距离 + 模糊。
    #[must_use]
    pub fn outset(&self) -> f32 {
        let stroke = self.stroke.map_or(0.0, |s| s.width);
        let shadow = self.shadow.map_or(0.0, |s| s.distance + s.blur * 2.0);
        stroke.max(shadow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_to_four_masks_can_be_added_and_removed() {
        let mut l = Look::NONE;
        assert_eq!(l.masks().count(), 0);
        // 第一个放主蒙版，之后放进另外三格，满了加不了
        assert_eq!(l.add_mask(), Some(0));
        assert_eq!(l.add_mask(), Some(1));
        assert_eq!(l.add_mask(), Some(2));
        assert_eq!(l.add_mask(), Some(3));
        assert_eq!(l.add_mask(), None);
        assert_eq!(l.masks().count(), MAX_MASKS);
        assert!(l.is_valid());
        // 删第一个：后面的往前挪
        if let Some(m) = &mut l.extra_masks[0] {
            m.x = 0.3;
        }
        l.remove_mask(0);
        assert_eq!(l.masks().count(), 3);
        assert!(
            (l.mask.unwrap().x - 0.3).abs() < 1e-6,
            "原来的第二个成了第一个"
        );
        l.remove_mask(1);
        assert_eq!(l.masks().count(), 2);
        // 存文件：没有另外的蒙版时不写
        let json = serde_json::to_string(&Look::NONE).unwrap();
        assert!(!json.contains("extra_masks"), "{json}");
        let back: Look = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
        // 另外的蒙版数值不对也不行
        l.extra_masks[0] = Some(Mask {
            width: 0.0,
            ..Mask::new(MaskShape::Rectangle)
        });
        assert!(!l.is_valid());
    }

    #[test]
    fn blend_formulas_match_known_values() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(near(BlendMode::Multiply.channel(0.5, 0.5), 0.25));
        assert!(near(BlendMode::Screen.channel(0.5, 0.5), 0.75));
        assert!(near(BlendMode::Darken.channel(0.2, 0.7), 0.2));
        assert!(near(BlendMode::Lighten.channel(0.2, 0.7), 0.7));
        assert!(near(BlendMode::Overlay.channel(0.25, 1.0), 0.5));
        assert!(near(BlendMode::HardLight.channel(1.0, 0.25), 0.5));
        assert!(near(BlendMode::LinearBurn.channel(0.3, 0.4), 0.0));
        assert!(near(BlendMode::ColorDodge.channel(0.5, 0.5), 1.0));
        assert!(near(BlendMode::ColorBurn.channel(0.5, 0.5), 0.0));
        assert!(near(BlendMode::SoftLight.channel(0.5, 0.5), 0.5));
        // 白色叠在任何东西上「正片叠底」都不变，黑色「滤色」都不变
        for b in [0.0, 0.3, 1.0] {
            assert!(near(BlendMode::Multiply.channel(b, 1.0), b));
            assert!(near(BlendMode::Screen.channel(b, 0.0), b));
        }
        assert_eq!(BlendMode::ALL.len(), 11);
        assert_eq!(BlendMode::ALL.map(BlendMode::code)[10], 10);
    }

    #[test]
    fn chroma_key_removes_the_key_colour_and_keeps_others() {
        let k = ChromaKey::default();
        assert!(k.keep([0.0, 1.0, 0.0]) < 1e-6, "纯绿抠掉");
        assert!(k.keep([0.1, 0.9, 0.1]) < 0.5, "接近绿的也抠掉大半");
        assert!((k.keep([1.0, 0.8, 0.7]) - 1.0).abs() < 1e-6, "肤色留着");
        assert!((k.keep([1.0, 1.0, 1.0]) - 1.0).abs() < 1e-6, "白色留着");
        // 强度越大，抠掉的越多
        let strong = ChromaKey {
            intensity: 0.9,
            ..k
        };
        assert!(strong.keep([0.4, 0.7, 0.3]) <= k.keep([0.4, 0.7, 0.3]));
    }

    #[test]
    fn look_validation_and_outset() {
        let mut l = Look::default();
        assert!(l.is_default() && l.is_valid());
        l.stroke = Some(Stroke::default());
        l.shadow = Some(Shadow::default());
        assert!(l.is_valid());
        assert!((l.outset() - (0.015 + 0.04)).abs() < 1e-6);
        l.radius = 2.0;
        assert!(!l.is_valid());
        l.radius = 0.2;
        l.mask = Some(Mask {
            width: 0.0,
            ..Mask::new(MaskShape::Circle)
        });
        assert!(!l.is_valid());
        l.mask = Some(Mask::new(MaskShape::Star));
        assert!(l.is_valid());
        assert_eq!(rgb_unit(0xFF_8000), [1.0, 128.0 / 255.0, 0.0]);
    }
}
