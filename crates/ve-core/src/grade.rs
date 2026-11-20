//! 进阶调色（剪映「调整」的 HSL、曲线、色轮、LUT）：这些都不是线性的，不能并进
//! [`crate::ColorMatrix`]。合成时把一个图层的整条调色链（基础调整矩阵 → 进阶调色，再接上压在它上面的
//! 调节片段）烘焙成一个 3D 查找表，显卡逐像素查表（`ve-render`）。
//!
//! 顺序：HSL → 曲线 → 色轮 → LUT（按强度和原色混合）。颜色都是 sRGB 0–1、不预乘。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// HSL 的八个色相（剪映的八个色环：红橙黄绿青蓝紫品）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HslBand {
    /// 红。
    Red,
    /// 橙。
    Orange,
    /// 黄。
    Yellow,
    /// 绿。
    Green,
    /// 青。
    Cyan,
    /// 蓝。
    Blue,
    /// 紫。
    Purple,
    /// 品红。
    Magenta,
}

impl HslBand {
    /// 全部（剪映里的顺序）。
    pub const ALL: [Self; 8] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Cyan,
        Self::Blue,
        Self::Purple,
        Self::Magenta,
    ];

    /// 中心色相（度）。
    #[must_use]
    pub const fn hue(self) -> f32 {
        match self {
            Self::Red => 0.0,
            Self::Orange => 30.0,
            Self::Yellow => 60.0,
            Self::Green => 120.0,
            Self::Cyan => 180.0,
            Self::Blue => 240.0,
            Self::Purple => 270.0,
            Self::Magenta => 300.0,
        }
    }

    /// 名字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Red => "红",
            Self::Orange => "橙",
            Self::Yellow => "黄",
            Self::Green => "绿",
            Self::Cyan => "青",
            Self::Blue => "蓝",
            Self::Purple => "紫",
            Self::Magenta => "品红",
        }
    }

    /// 下标。
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Orange => 1,
            Self::Yellow => 2,
            Self::Green => 3,
            Self::Cyan => 4,
            Self::Blue => 5,
            Self::Purple => 6,
            Self::Magenta => 7,
        }
    }
}

/// 一个色相的调整，各 -1–1。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HslAdjust {
    /// 色相（±1 = ±30°）。
    pub hue: f32,
    /// 饱和度。
    pub saturation: f32,
    /// 亮度。
    pub lightness: f32,
}

/// 曲线：（输入，输出）点，按输入排好，两头默认 (0,0)、(1,1)。
pub type CurvePoints = Vec<(f32, f32)>;

/// 曲线调色：亮度（三个通道一起）和红、绿、蓝各一条。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Curves {
    /// 亮度曲线（三个通道一起）。
    pub luma: CurvePoints,
    /// 红。
    pub red: CurvePoints,
    /// 绿。
    pub green: CurvePoints,
    /// 蓝。
    pub blue: CurvePoints,
}

/// 色轮（一级校色）：暗部 / 中灰 / 亮部 / 偏移，每个是 RGB 三个通道的偏移（-1–1）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wheels {
    /// 暗部（lift）。
    pub shadows: [f32; 3],
    /// 中灰（gamma）。
    pub midtones: [f32; 3],
    /// 亮部（gain）。
    pub highlights: [f32; 3],
    /// 偏移（整体加减）。
    pub offset: [f32; 3],
}

/// 套用的 LUT 文件（.cube）和强度。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LutRef {
    /// 文件。
    pub path: PathBuf,
    /// 强度（0–1）。
    pub intensity: f32,
}

/// 一个片段的进阶调色；默认值是什么都不做（不写进工程文件）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grade {
    /// HSL：八个色相，按 [`HslBand::index`] 排。
    pub hsl: [HslAdjust; 8],
    /// 曲线。
    pub curves: Curves,
    /// 色轮。
    pub wheels: Wheels,
    /// LUT。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lut: Option<LutRef>,
}

/// 一条曲线是不是没动过（没有点，或者只有两头的对角线）。
fn curve_is_identity(c: &CurvePoints) -> bool {
    c.iter().all(|(x, y)| (x - y).abs() < 1e-6)
}

impl Grade {
    /// 什么都没调。
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.hsl.iter().all(|h| *h == HslAdjust::default())
            && curve_is_identity(&self.curves.luma)
            && curve_is_identity(&self.curves.red)
            && curve_is_identity(&self.curves.green)
            && curve_is_identity(&self.curves.blue)
            && self.wheels == Wheels::default()
            && self.lut.is_none()
    }

    /// 各项都在范围里。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let unit = |v: f32| (-1.0..=1.0).contains(&v);
        let curve_ok = |c: &CurvePoints| {
            c.iter()
                .all(|(x, y)| (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y))
                && c.windows(2).all(|w| w[0].0 < w[1].0)
        };
        self.hsl
            .iter()
            .all(|h| unit(h.hue) && unit(h.saturation) && unit(h.lightness))
            && [
                &self.curves.luma,
                &self.curves.red,
                &self.curves.green,
                &self.curves.blue,
            ]
            .into_iter()
            .all(curve_ok)
            && [
                self.wheels.shadows,
                self.wheels.midtones,
                self.wheels.highlights,
                self.wheels.offset,
            ]
            .iter()
            .flatten()
            .all(|v| unit(*v))
            && self
                .lut
                .as_ref()
                .is_none_or(|l| (0.0..=1.0).contains(&l.intensity))
    }

    /// 对一个颜色做进阶调色（LUT 文件读不出来时跳过 LUT）。
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut c = apply_hsl(&self.hsl, rgb);
        c = apply_curves(&self.curves, c);
        c = apply_wheels(&self.wheels, c);
        if let Some(l) = &self.lut
            && let Some(lut) = load_lut(&l.path)
        {
            let t = lut.sample(c);
            let k = l.intensity.clamp(0.0, 1.0);
            c = [0, 1, 2].map(|i| (t[i] - c[i]).mul_add(k, c[i]));
        }
        c.map(|v| v.clamp(0.0, 1.0))
    }
}

// ---- HSL ----

#[expect(
    clippy::many_single_char_names,
    reason = "颜色公式里的 r/g/b、h/s/l 是通行写法"
)]
fn rgb_to_hsl([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = f32::midpoint(max, min);
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() < f32::EPSILON {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

#[expect(
    clippy::many_single_char_names,
    reason = "颜色公式里的 r/g/b、h/s/l 是通行写法"
)]
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    if s <= 0.0 {
        return [l; 3];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0f32.mul_add(l, -q);
    let hk = h.rem_euclid(360.0) / 360.0;
    let ch = |t: f32| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            ((q - p) * 6.0).mul_add(t, p)
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            ((q - p) * 6.0).mul_add(2.0 / 3.0 - t, p)
        } else {
            p
        }
    };
    [ch(hk + 1.0 / 3.0), ch(hk), ch(hk - 1.0 / 3.0)]
}

/// 色相差（度，0–180）。
fn hue_distance(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

#[expect(
    clippy::many_single_char_names,
    reason = "颜色公式里的 r/g/b、h/s/l 是通行写法"
)]
fn apply_hsl(bands: &[HslAdjust; 8], rgb: [f32; 3]) -> [f32; 3] {
    if bands.iter().all(|b| *b == HslAdjust::default()) {
        return rgb;
    }
    let (h, s, l) = rgb_to_hsl(rgb);
    if s < 1e-4 {
        return rgb;
    }
    let (mut dh, mut ds, mut dl) = (0.0, 0.0, 0.0);
    for band in HslBand::ALL {
        let adj = bands[band.index()];
        // 每个色相管它到两边相邻色相之间的那一段（三角形权重）
        let i = band.index();
        let prev = HslBand::ALL[(i + 7) % 8].hue();
        let next = HslBand::ALL[(i + 1) % 8].hue();
        let center = band.hue();
        let reach = |other: f32| hue_distance(center, other).max(1.0);
        let d = hue_distance(h, center);
        let side = if (h - center).rem_euclid(360.0) < 180.0 {
            reach(next)
        } else {
            reach(prev)
        };
        let w = (1.0 - d / side).max(0.0);
        dh += w * adj.hue * 30.0;
        ds += w * adj.saturation;
        dl += w * adj.lightness;
    }
    // 灰的地方几乎不动（按原来的饱和度加权）
    let weight = s.min(1.0);
    let s2 = (s * (1.0 + ds)).clamp(0.0, 1.0);
    let l2 = (dl * 0.3).mul_add(weight, l).clamp(0.0, 1.0);
    hsl_to_rgb(h + dh, s2, l2)
}

// ---- 曲线 ----

/// 单调三次插值（Fritsch–Carlson），两头补 (0,0)、(1,1)。
#[expect(
    clippy::many_single_char_names,
    reason = "插值公式里的 x/y/n/h/t 是通行写法"
)]
fn eval_curve(points: &CurvePoints, x: f32) -> f32 {
    if curve_is_identity(points) {
        return x;
    }
    let mut pts: Vec<(f32, f32)> = points.clone();
    if pts.first().is_none_or(|p| p.0 > 0.0) {
        pts.insert(0, (0.0, 0.0));
    }
    if pts.last().is_none_or(|p| p.0 < 1.0) {
        pts.push((1.0, 1.0));
    }
    let n = pts.len();
    let x = x.clamp(0.0, 1.0);
    let k = pts.partition_point(|p| p.0 <= x).clamp(1, n - 1);
    let slope = |i: usize| (pts[i + 1].1 - pts[i].1) / (pts[i + 1].0 - pts[i].0).max(1e-6);
    let tangent = |i: usize| {
        if i == 0 {
            slope(0)
        } else if i == n - 1 {
            slope(n - 2)
        } else {
            let (a, b) = (slope(i - 1), slope(i));
            if a * b <= 0.0 {
                0.0
            } else {
                2.0 * a * b / (a + b)
            }
        }
    };
    let (x0, y0) = pts[k - 1];
    let (x1, y1) = pts[k];
    let h = (x1 - x0).max(1e-6);
    let t = (x - x0) / h;
    let (m0, m1) = (tangent(k - 1) * h, tangent(k) * h);
    let t2 = t * t;
    let t3 = t2 * t;
    let y = (2.0 * t3 - 3.0 * t2 + 1.0) * y0
        + (t3 - 2.0 * t2 + t) * m0
        + (-2.0 * t3 + 3.0 * t2) * y1
        + (t3 - t2) * m1;
    y.clamp(0.0, 1.0)
}

/// 曲线在 `x` 处的值（界面画曲线用，和调色同一个公式）。
#[must_use]
pub fn curve_value(points: &CurvePoints, x: f32) -> f32 {
    eval_curve(points, x)
}

/// 一个色相的代表色（界面上的色环），0–255。
#[must_use]
pub fn hsl_preview(band: HslBand) -> [u8; 3] {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "0–255"
    )]
    hsl_to_rgb(band.hue(), 0.85, 0.5).map(|v| (v * 255.0).round() as u8)
}

fn apply_curves(c: &Curves, rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(|v| eval_curve(&c.luma, v));
    [
        eval_curve(&c.red, r),
        eval_curve(&c.green, g),
        eval_curve(&c.blue, b),
    ]
}

// ---- 色轮 ----

fn apply_wheels(w: &Wheels, rgb: [f32; 3]) -> [f32; 3] {
    if *w == Wheels::default() {
        return rgb;
    }
    [0, 1, 2].map(|i| {
        let x = rgb[i];
        let lift = w.shadows[i] * 0.25;
        let gain = 0.5f32.mul_add(w.highlights[i], 1.0);
        let gamma = 0.5f32.mul_add(w.midtones[i], 1.0).max(0.1);
        let v = (lift.mul_add(1.0 - x, x) * gain).clamp(0.0, 1.0);
        w.offset[i].mul_add(0.25, v.powf(1.0 / gamma))
    })
}

// ---- LUT ----

/// 读出来的 3D LUT（.cube）。
#[derive(Debug, Clone, PartialEq)]
pub struct Lut {
    /// 每边格数。
    pub size: usize,
    /// 颜色，红变得最快（.cube 的顺序）。
    pub data: Vec<[f32; 3]>,
}

impl Lut {
    /// 解析 .cube 文本（`LUT_3D_SIZE`，`DOMAIN_MIN / MAX` 只认默认的 0–1）。
    ///
    /// # Errors
    /// 没有尺寸、数据个数不对、数解析不了。
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut size = None;
        let mut data = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("LUT_3D_SIZE") {
                size = Some(rest.trim().parse::<usize>().map_err(|e| e.to_string())?);
                continue;
            }
            if line.starts_with(|c: char| c.is_ascii_alphabetic()) {
                continue; // TITLE、DOMAIN_MIN 之类
            }
            let v: Vec<f32> = line
                .split_whitespace()
                .map(str::parse)
                .collect::<Result<_, _>>()
                .map_err(|e: std::num::ParseFloatError| e.to_string())?;
            if v.len() != 3 {
                return Err(format!("这一行不是三个数：{line}"));
            }
            data.push([v[0], v[1], v[2]]);
        }
        let size = size.ok_or("缺少 LUT_3D_SIZE（只支持 3D LUT）")?;
        if !(2..=128).contains(&size) || data.len() != size * size * size {
            return Err(format!("数据个数 {} 和尺寸 {size} 对不上", data.len()));
        }
        Ok(Self { size, data })
    }

    fn at(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.data[(b * self.size + g) * self.size + r]
    }

    /// 三线性插值取色。
    #[must_use]
    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        #[expect(clippy::cast_precision_loss, reason = "格数很小")]
        let max = (self.size - 1) as f32;
        let pos = rgb.map(|v| v.clamp(0.0, 1.0) * max);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "0–size"
        )]
        let lo = pos.map(|p| (p.floor() as usize).min(self.size - 2));
        #[expect(clippy::cast_precision_loss, reason = "格数很小")]
        let f = [0, 1, 2].map(|i| pos[i] - lo[i] as f32);
        let mut out = [0.0f32; 3];
        for (dr, dg, db) in [
            (0, 0, 0),
            (1, 0, 0),
            (0, 1, 0),
            (1, 1, 0),
            (0, 0, 1),
            (1, 0, 1),
            (0, 1, 1),
            (1, 1, 1),
        ] {
            let w = (if dr == 1 { f[0] } else { 1.0 - f[0] })
                * (if dg == 1 { f[1] } else { 1.0 - f[1] })
                * (if db == 1 { f[2] } else { 1.0 - f[2] });
            let c = self.at(lo[0] + dr, lo[1] + dg, lo[2] + db);
            for i in 0..3 {
                out[i] = c[i].mul_add(w, out[i]);
            }
        }
        out
    }
}

/// 读过的 LUT 文件（按路径；同一个文件只读一次）。
fn cache() -> &'static Mutex<std::collections::HashMap<PathBuf, Option<Arc<Lut>>>> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<PathBuf, Option<Arc<Lut>>>>> =
        OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// 读 LUT 文件（有缓存）；读不出来时 `None`。
#[must_use]
pub fn load_lut(path: &Path) -> Option<Arc<Lut>> {
    let mut c = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    c.entry(path.to_path_buf())
        .or_insert_with(|| {
            let text = std::fs::read_to_string(path).ok()?;
            Lut::parse(&text).ok().map(Arc::new)
        })
        .clone()
}

/// 一步调色：先过调色矩阵，再（有的话）过进阶调色。
#[derive(Clone, Debug, PartialEq)]
pub struct GradeStep {
    /// 先把素材从这个色彩空间换成 Rec.709（HDR / Log 素材；[`crate::color_space`]）。
    pub input: Option<crate::color_space::InputSpace>,
    /// 基础调整（亮度、对比度…）的矩阵。
    pub matrix: crate::ColorMatrix,
    /// 进阶调色。
    pub grade: Option<Arc<Grade>>,
}

/// 一串调色（片段自己的，再接上压在它上面的调节片段）。
pub type GradeChain = Vec<GradeStep>;

/// 整条调色链对一个颜色的结果。
#[must_use]
pub fn apply_chain(chain: &[GradeStep], rgb: [f32; 3]) -> [f32; 3] {
    chain.iter().fold(rgb, |c, step| {
        let c = step.input.map_or(c, |s| s.to_rec709(c));
        let m = step.matrix.apply(c).map(|v| v.clamp(0.0, 1.0));
        step.grade.as_ref().map_or(m, |g| g.apply(m))
    })
}

/// 烘焙查找表的每边格数。
pub const BAKE_SIZE: usize = 33;

/// 把整条调色链烘焙成 [`BAKE_SIZE`]³ 的查找表（RGBA8，红变得最快），给显卡用。
#[must_use]
pub fn bake(chain: &[GradeStep]) -> Vec<u8> {
    let n = BAKE_SIZE;
    #[expect(clippy::cast_precision_loss, reason = "格数很小")]
    let max = (n - 1) as f32;
    let mut out = Vec::with_capacity(n * n * n * 4);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                #[expect(clippy::cast_precision_loss, reason = "格数很小")]
                let c = apply_chain(chain, [r as f32 / max, g as f32 / max, b as f32 / max]);
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "0–255"
                )]
                out.extend(c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
                out.push(255);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
    }

    #[test]
    fn identity_grade_changes_nothing() {
        let g = Grade::default();
        assert!(g.is_identity() && g.is_valid());
        for c in [
            [0.0, 0.0, 0.0],
            [0.2, 0.5, 0.9],
            [1.0, 1.0, 1.0],
            [0.7, 0.1, 0.3],
        ] {
            assert!(near(g.apply(c), c, 1e-5), "{c:?} -> {:?}", g.apply(c));
        }
    }

    #[test]
    fn hsl_shifts_only_the_chosen_hue() {
        let mut g = Grade::default();
        g.hsl[HslBand::Red.index()].saturation = -1.0;
        let red = g.apply([0.9, 0.1, 0.1]);
        assert!((red[0] - red[1]).abs() < 0.05, "红变灰：{red:?}");
        let blue = g.apply([0.1, 0.1, 0.9]);
        assert!(near(blue, [0.1, 0.1, 0.9], 1e-3), "蓝不受影响：{blue:?}");
        let gray = g.apply([0.5, 0.5, 0.5]);
        assert!(near(gray, [0.5, 0.5, 0.5], 1e-5));
        // 色相：绿往青转
        let mut hue = Grade::default();
        hue.hsl[HslBand::Green.index()].hue = 1.0;
        let c = hue.apply([0.1, 0.9, 0.1]);
        assert!(c[2] > 0.2, "{c:?}");
    }

    #[test]
    fn curves_are_monotonic_and_hit_their_points() {
        let pts = vec![(0.25, 0.1), (0.75, 0.9)];
        let mut last = -1.0;
        for i in 0..=100 {
            #[expect(clippy::cast_precision_loss, reason = "测试")]
            let y = eval_curve(&pts, i as f32 / 100.0);
            assert!(y >= last - 1e-6);
            last = y;
        }
        assert!((eval_curve(&pts, 0.25) - 0.1).abs() < 1e-5);
        assert!((eval_curve(&pts, 0.75) - 0.9).abs() < 1e-5);
        let mut g = Grade::default();
        g.curves.red = vec![(0.5, 0.8)];
        let c = g.apply([0.5, 0.5, 0.5]);
        assert!(c[0] > 0.75 && (c[1] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn wheels_lift_shadows_and_gain_highlights() {
        let mut g = Grade::default();
        g.wheels.shadows = [0.4, 0.4, 0.4];
        assert!(g.apply([0.0, 0.0, 0.0])[0] > 0.05, "暗部被提亮");
        assert!((g.apply([1.0, 1.0, 1.0])[0] - 1.0).abs() < 1e-5, "白不变");
        let mut warm = Grade::default();
        warm.wheels.highlights = [0.0, 0.0, -0.6];
        let c = warm.apply([0.8, 0.8, 0.8]);
        assert!(c[2] < c[0], "亮部偏黄：{c:?}");
    }

    #[test]
    fn cube_lut_parses_and_samples() {
        use std::fmt::Write as _;
        // 2³ 的反色 LUT
        let mut text = String::from("TITLE \"invert\"\nLUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    let _ = writeln!(text, "{} {} {}", 1 - r, 1 - g, 1 - b);
                }
            }
        }
        let lut = Lut::parse(&text).unwrap();
        assert!(near(lut.sample([0.25, 0.5, 1.0]), [0.75, 0.5, 0.0], 1e-5));
        assert!(Lut::parse("LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(Lut::parse("0 0 0\n").is_err());
        // 文件 + 强度
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inv.cube");
        std::fs::write(&path, &text).unwrap();
        let g = Grade {
            lut: Some(LutRef {
                path,
                intensity: 0.5,
            }),
            ..Grade::default()
        };
        assert!(near(g.apply([0.0, 0.0, 0.0]), [0.5, 0.5, 0.5], 1e-5));
        // 烘焙：33³，单位链就是原色
        let baked = bake(&[]);
        assert_eq!(baked.len(), BAKE_SIZE * BAKE_SIZE * BAKE_SIZE * 4);
        assert_eq!(&baked[..4], &[0, 0, 0, 255]);
        let last = baked.len() - 4;
        assert_eq!(&baked[last..], &[255, 255, 255, 255]);
    }

    #[test]
    fn chain_applies_steps_in_order() {
        let half = crate::ColorAdjust {
            brightness: -1.0,
            ..crate::ColorAdjust::NONE
        };
        let mut g = Grade::default();
        g.curves.luma = vec![(0.5, 1.0)];
        let chain = vec![GradeStep {
            input: None,
            matrix: half.matrix(),
            grade: Some(Arc::new(g)),
        }];
        let c = apply_chain(&chain, [0.75, 0.75, 0.75]);
        // 先暗 0.25（0.75 → 0.5），再曲线把 0.5 拉到 1
        assert!((c[0] - 1.0).abs() < 1e-3, "{c:?}");
    }
}
