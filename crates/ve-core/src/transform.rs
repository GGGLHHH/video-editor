//! 片段的画面属性（位置、缩放、裁剪、不透明度）和由它算出的图层几何。
//!
//! 几何是纯函数，预览合成、导出合成、预览上的拖拽框三处共用，保证三处一致。

use serde::{Deserialize, Serialize};

/// 四边各裁掉源画面的比例（0–1）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crop {
    /// 左边。
    pub left: f32,
    /// 上边。
    pub top: f32,
    /// 右边。
    pub right: f32,
    /// 下边。
    pub bottom: f32,
}

/// 片段的画面属性。默认值下的画面和 MVP 一样：等比放进画布、居中。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transform {
    /// 画面中心相对画布中心的水平偏移，单位是画布宽度。
    pub x: f32,
    /// 垂直偏移，单位是画布高度（向下为正）。
    pub y: f32,
    /// 相对「完整放进画布」时的大小。
    pub scale: f32,
    /// 裁剪；只切掉边，不改变画面的位置和大小。
    pub crop: Crop,
    /// 不透明度（0–1）。
    pub opacity: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Transform {
    /// 默认值（常量版本，供 `const fn` 使用）。
    pub const DEFAULT: Self = Self {
        x: 0.0,
        y: 0.0,
        scale: 1.0,
        crop: Crop {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        },
        opacity: 1.0,
    };

    /// `scale` 的取值范围。
    pub const SCALE_RANGE: (f32, f32) = (0.01, 10.0);

    /// 是否满足不变式：有限、缩放在范围内、不透明度 0–1、裁剪不越界。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let c = self.crop;
        let all = [
            self.x,
            self.y,
            self.scale,
            self.opacity,
            c.left,
            c.top,
            c.right,
            c.bottom,
        ];
        all.iter().all(|v| v.is_finite())
            && (Self::SCALE_RANGE.0..=Self::SCALE_RANGE.1).contains(&self.scale)
            && (0.0..=1.0).contains(&self.opacity)
            && [c.left, c.top, c.right, c.bottom].iter().all(|v| *v >= 0.0)
            && c.left + c.right < 1.0
            && c.top + c.bottom < 1.0
    }

    /// 是否是默认值。
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// 矩形（画布像素坐标，或 0–1 纹理坐标）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// 左。
    pub x: f32,
    /// 上。
    pub y: f32,
    /// 宽。
    pub w: f32,
    /// 高。
    pub h: f32,
}

impl Rect {
    /// 是否包含点 `(px, py)`。
    #[must_use]
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
}

/// 一个图层在画布上的几何。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayerGeometry {
    /// 未裁剪的整幅画面框。
    pub full: Rect,
    /// 裁剪后实际显示的部分。
    pub shown: Rect,
    /// `shown` 对应的纹理坐标（0–1）。
    pub uv: Rect,
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "画布坐标远小于 f32 的精度上限"
)]
const fn to_f32(v: f64) -> f32 {
    v as f32
}

/// 由画布尺寸、源画面尺寸和画面属性算出图层几何。
///
/// 完整源画面等比放进画布再乘 `scale`，中心放在画布中心偏移 `(x·w, y·h)` 处；
/// 裁剪从这个框的四边往里切，纹理坐标同步切。尺寸为零时返回空矩形。
#[must_use]
pub fn layer_geometry(canvas: (u32, u32), source: (u32, u32), t: &Transform) -> LayerGeometry {
    let (canvas_w, canvas_h) = (f64::from(canvas.0), f64::from(canvas.1));
    let (src_w, src_h) = (f64::from(source.0), f64::from(source.1));
    if canvas_w <= 0.0 || canvas_h <= 0.0 || src_w <= 0.0 || src_h <= 0.0 {
        return LayerGeometry::default();
    }
    let factor = (canvas_w / src_w).min(canvas_h / src_h) * f64::from(t.scale);
    let (full_w, full_h) = (src_w * factor, src_h * factor);
    let left = canvas_w / 2.0 + f64::from(t.x) * canvas_w - full_w / 2.0;
    let top = canvas_h / 2.0 + f64::from(t.y) * canvas_h - full_h / 2.0;
    let crop = t.crop;
    let (cut_l, cut_t) = (f64::from(crop.left), f64::from(crop.top));
    let keep_w = 1.0 - cut_l - f64::from(crop.right);
    let keep_h = 1.0 - cut_t - f64::from(crop.bottom);
    let make = |x: f64, y: f64, width: f64, height: f64| Rect {
        x: to_f32(x),
        y: to_f32(y),
        w: to_f32(width),
        h: to_f32(height),
    };
    LayerGeometry {
        full: make(left, top, full_w, full_h),
        shown: make(
            cut_l.mul_add(full_w, left),
            cut_t.mul_add(full_h, top),
            full_w * keep_w,
            full_h * keep_h,
        ),
        uv: make(cut_l, cut_t, keep_w, keep_h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn close(a: Rect, b: Rect) -> bool {
        (a.x - b.x).abs() < 1e-3
            && (a.y - b.y).abs() < 1e-3
            && (a.w - b.w).abs() < 1e-3
            && (a.h - b.h).abs() < 1e-3
    }

    #[test]
    fn default_fits_and_centers() {
        let g = layer_geometry((1920, 1080), (1280, 720), &Transform::default());
        assert!(close(g.full, rect(0.0, 0.0, 1920.0, 1080.0)), "{g:?}");
        assert_eq!(g.shown, g.full);
        assert_eq!(g.uv, rect(0.0, 0.0, 1.0, 1.0));
        // 方形素材：左右留黑
        let sq = layer_geometry((1920, 1080), (1080, 1080), &Transform::default());
        assert!(close(sq.full, rect(420.0, 0.0, 1080.0, 1080.0)), "{sq:?}");
    }

    #[test]
    fn scale_and_offset() {
        let t = Transform {
            x: 0.25,
            y: -0.1,
            scale: 0.5,
            ..Transform::default()
        };
        let g = layer_geometry((1000, 500), (1000, 500), &t);
        // 中心 (500 + 250, 250 - 50) = (750, 200)，尺寸 500×250
        assert!(close(g.full, rect(500.0, 75.0, 500.0, 250.0)), "{g:?}");
    }

    #[test]
    fn crop_keeps_unclipped_edges() {
        let t = Transform {
            crop: Crop {
                left: 0.25,
                top: 0.1,
                ..Crop::default()
            },
            ..Transform::default()
        };
        let g = layer_geometry((1920, 1080), (1920, 1080), &t);
        assert!(close(g.full, rect(0.0, 0.0, 1920.0, 1080.0)));
        assert!(
            close(g.shown, rect(480.0, 108.0, 1440.0, 972.0)),
            "{:?}",
            g.shown
        );
        assert!(close(g.uv, rect(0.25, 0.1, 0.75, 0.9)), "{:?}", g.uv);
    }

    #[test]
    fn invalid_transforms_rejected() {
        assert!(Transform::default().is_valid());
        let bad = [
            Transform {
                x: f32::NAN,
                ..Transform::default()
            },
            Transform {
                scale: 0.0,
                ..Transform::default()
            },
            Transform {
                scale: 10.5,
                ..Transform::default()
            },
            Transform {
                opacity: 1.5,
                ..Transform::default()
            },
            Transform {
                crop: Crop {
                    left: 0.6,
                    right: 0.5,
                    ..Crop::default()
                },
                ..Transform::default()
            },
            Transform {
                crop: Crop {
                    top: -0.1,
                    ..Crop::default()
                },
                ..Transform::default()
            },
        ];
        for t in bad {
            assert!(!t.is_valid(), "{t:?}");
        }
    }

    #[test]
    fn degenerate_sizes_give_empty_rect() {
        let no_canvas = layer_geometry((0, 1080), (1920, 1080), &Transform::default());
        assert_eq!(no_canvas, LayerGeometry::default());
        let no_source = layer_geometry((1920, 1080), (0, 0), &Transform::default());
        assert_eq!(no_source, LayerGeometry::default());
    }

    proptest! {
        #[test]
        fn geometry_invariants(
            cw in 16u32..4000, ch in 16u32..4000, sw in 16u32..4000, sh in 16u32..4000,
            x in -2.0f32..2.0, y in -2.0f32..2.0, scale in 0.01f32..10.0,
            l in 0.0f32..0.49, r in 0.0f32..0.49, t in 0.0f32..0.49, b in 0.0f32..0.49,
        ) {
            let tf = Transform { x, y, scale, crop: Crop { left: l, top: t, right: r, bottom: b }, opacity: 1.0 };
            prop_assert!(tf.is_valid());
            let geom = layer_geometry((cw, ch), (sw, sh), &tf);
            let (full, shown) = (geom.full, geom.shown);
            let eps = 1e-3 * full.w.max(full.h).max(1.0);
            prop_assert!(shown.x >= full.x - eps && shown.y >= full.y - eps);
            prop_assert!(shown.x + shown.w <= full.x + full.w + eps);
            prop_assert!(shown.y + shown.h <= full.y + full.h + eps);
            let src_ratio = f64::from(sw) / f64::from(sh);
            let full_ratio = f64::from(full.w) / f64::from(full.h);
            prop_assert!((full_ratio / src_ratio - 1.0).abs() < 1e-4);
            // scale = 1 时恰好放进画布（至少一边贴满）
            let fit = layer_geometry((cw, ch), (sw, sh), &Transform::default()).full;
            let (cwf, chf) = (f64::from(cw), f64::from(ch));
            let (fw, fh) = (f64::from(fit.w), f64::from(fit.h));
            let fills = (fw - cwf).abs() < 1e-2 || (fh - chf).abs() < 1e-2;
            prop_assert!(fills && fw <= cwf + 1e-2 && fh <= chf + 1e-2);
        }
    }
}
