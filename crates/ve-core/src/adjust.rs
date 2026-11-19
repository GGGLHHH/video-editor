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

    /// 同一个调色写成颜色矩阵（不含最后的截断）。调节层要把几次调色连起来做，矩阵才能相乘。
    #[must_use]
    pub fn matrix(&self) -> ColorMatrix {
        // 提亮、对比度对每个通道一样：g·c + k；饱和度是 S = s·I + (1−s)·1·Lᵀ（S·1 = 1，常数项不变）；
        // 最后加色温、色调的偏移
        const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
        let g = 1.0 + self.contrast;
        let s = 1.0 + self.saturation;
        let k = (self.brightness * 0.25 - 0.5).mul_add(g, 0.5);
        let shift = [
            self.temperature * 0.1 + self.tint * 0.05,
            -self.tint * 0.1,
            self.tint.mul_add(0.05, -self.temperature * 0.1),
        ];
        let row = |i: usize| {
            let m = |j: usize| g * (1.0 - s).mul_add(LUMA[j], if i == j { s } else { 0.0 });
            [m(0), m(1), m(2), k + shift[i]]
        };
        ColorMatrix([row(0), row(1), row(2)])
    }
}

/// 颜色的仿射变换：输出 = M·rgb + 常数（每行 `[r 系数, g 系数, b 系数, 常数]`），sRGB 0–1、不预乘。
/// 合成器按它给每层调色；调节层把自己的矩阵接在下面各层的后面。
///
/// 仿射变换和透明叠加可以交换次序（`f(a·x + (1−a)·y) = a·f(x) + (1−a)·f(y)`），所以「先叠好再整体调色」
/// 等于「每层先调色再叠」，只要底色也调。差别只在中途超出 0–1 被截断的地方。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorMatrix(pub [[f32; 4]; 3]);

impl Default for ColorMatrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl ColorMatrix {
    /// 不调色。
    pub const IDENTITY: Self = Self([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ]);

    /// 是不是不调色（合成时跳过，免得反预乘再预乘带来误差）。
    #[must_use]
    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// 对一个颜色做变换，再截断到 0–1（和着色器一样）。
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        self.0.map(|[r, g, b, c]| {
            r.mul_add(rgb[0], g.mul_add(rgb[1], b.mul_add(rgb[2], c)))
                .clamp(0.0, 1.0)
        })
    }

    /// 先做 `self` 再做 `next`。
    #[must_use]
    pub fn then(&self, next: &Self) -> Self {
        let (a, b) = (&self.0, &next.0);
        Self(std::array::from_fn(|i| {
            let mut row = [0.0f32; 4];
            for (j, out) in row.iter_mut().enumerate() {
                let carry = if j == 3 { b[i][3] } else { 0.0 };
                *out = (0..3).fold(carry, |acc, k| b[i][k].mul_add(a[k][j], acc));
            }
            row
        }))
    }
}

/// 一个滤镜预设（剪映素材面板「滤镜」的本地版）：名字和一组调色参数。套用就是把片段的调色换成它，
/// 之后还能在「调整」里接着改。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Filter {
    /// 名字。
    pub name: &'static str,
    /// 调色参数。
    pub adjust: ColorAdjust,
}

const fn filter(
    name: &'static str,
    brightness: f32,
    contrast: f32,
    saturation: f32,
    temperature: f32,
    tint: f32,
) -> Filter {
    Filter {
        name,
        adjust: ColorAdjust {
            brightness,
            contrast,
            saturation,
            temperature,
            tint,
        },
    }
}

/// 滤镜预设，第一个「原图」是去掉调色。
pub const FILTERS: [Filter; 12] = [
    filter("原图", 0.0, 0.0, 0.0, 0.0, 0.0),
    filter("鲜艳", 0.0, 0.15, 0.45, 0.0, 0.0),
    filter("清新", 0.15, -0.1, 0.1, -0.15, 0.0),
    filter("暖阳", 0.05, 0.05, 0.1, 0.5, 0.0),
    filter("冷调", 0.0, 0.05, -0.05, -0.5, 0.0),
    filter("复古", 0.05, -0.1, -0.35, 0.35, 0.0),
    filter("胶片", 0.0, 0.15, -0.15, 0.15, 0.1),
    filter("青橙", 0.0, 0.2, 0.2, 0.25, -0.2),
    filter("粉调", 0.08, 0.0, 0.05, 0.05, 0.35),
    filter("暗调", -0.2, 0.25, -0.1, 0.0, 0.0),
    filter("淡雅", 0.1, -0.25, -0.3, 0.0, 0.0),
    filter("黑白", 0.0, 0.1, -1.0, 0.0, 0.0),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_matches_the_step_by_step_formula() {
        let adjust = ColorAdjust {
            brightness: 0.3,
            contrast: -0.4,
            saturation: 0.6,
            temperature: -0.2,
            tint: 0.5,
        };
        let m = adjust.matrix();
        for rgb in [
            [0.0, 0.0, 0.0],
            [1.0, 0.5, 0.25],
            [0.3, 0.9, 0.6],
            [0.5, 0.5, 0.5],
        ] {
            let (want, got) = (adjust.apply(rgb), m.apply(rgb));
            for k in 0..3 {
                assert!(
                    (want[k] - got[k]).abs() < 1e-5,
                    "{rgb:?}: {want:?} vs {got:?}"
                );
            }
        }
        assert!(ColorAdjust::NONE.matrix().is_identity());
    }

    #[test]
    fn chained_matrices_equal_doing_both_in_turn() {
        let first = ColorAdjust {
            saturation: -1.0,
            ..ColorAdjust::NONE
        }
        .matrix();
        let second = ColorAdjust {
            temperature: 0.5,
            contrast: 0.2,
            ..ColorAdjust::NONE
        }
        .matrix();
        let both = first.then(&second);
        for rgb in [[0.9, 0.2, 0.1], [0.1, 0.4, 0.7]] {
            let want = second.apply(first.apply(rgb));
            let got = both.apply(rgb);
            for k in 0..3 {
                assert!((want[k] - got[k]).abs() < 1e-5, "{want:?} vs {got:?}");
            }
        }
        assert_eq!(ColorMatrix::IDENTITY.then(&second), second);
        assert_eq!(second.then(&ColorMatrix::IDENTITY), second);
    }

    #[test]
    fn filters_are_valid_distinct_and_start_with_the_original() {
        assert!(FILTERS[0].adjust.is_identity());
        for (i, f) in FILTERS.iter().enumerate() {
            assert!(f.adjust.is_valid(), "{}", f.name);
            assert!(
                FILTERS[..i]
                    .iter()
                    .all(|g| g.name != f.name && g.adjust != f.adjust),
                "{} 重复",
                f.name
            );
        }
        // 黑白：彩色变成灰
        let [r, g, b] = FILTERS[11].adjust.apply([0.9, 0.2, 0.1]);
        assert!((r - g).abs() < 1e-6 && (g - b).abs() < 1e-6);
    }

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
