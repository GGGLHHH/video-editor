//! 片段的画面属性（位置、缩放、旋转、裁剪、不透明度）和由它算出的图层几何。
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
    /// 旋转角度（度，顺时针为正），绕整幅画面（未裁剪）的中心转。
    pub rotation: f32,
    /// 左右镜像（剪映工具栏的「镜像」）。裁剪按镜像后看到的左右算。不做关键帧。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub mirror: bool,
}

impl Transform {
    /// 把 `child`（一组里的一层）放进整组的变换 `self` 里，画布 `w` × `h`：位置跟着整组镜像、缩放、
    /// 旋转（绕画布中心）、平移；大小相乘、角度相加、不透明度相乘、镜像叠加。裁剪只用这一层自己的。
    #[must_use]
    pub fn compose(&self, child: &Self, w: u32, h: u32) -> Self {
        let (w, h) = (f64::from(w.max(1)), f64::from(h.max(1)));
        let mut cx = f64::from(child.x) * w;
        let cy = f64::from(child.y) * h;
        let mut rotation = child.rotation;
        if self.mirror {
            cx = -cx;
            rotation = -rotation;
        }
        let s = f64::from(self.scale);
        let (sin, cos) = f64::from(self.rotation).to_radians().sin_cos();
        let (px, py) = (cx * s, cy * s);
        let (rx, ry) = (px.mul_add(cos, -py * sin), px.mul_add(sin, py * cos));
        #[expect(clippy::cast_possible_truncation, reason = "画布单位里的小数")]
        Self {
            x: (rx / w) as f32 + self.x,
            y: (ry / h) as f32 + self.y,
            scale: self.scale * child.scale,
            rotation: rotation + self.rotation,
            opacity: self.opacity * child.opacity,
            mirror: self.mirror != child.mirror,
            // 镜像只把画面左右翻：裁掉的左边翻过去成了右边
            crop: if self.mirror {
                Crop {
                    left: child.crop.right,
                    right: child.crop.left,
                    ..child.crop
                }
            } else {
                child.crop
            },
        }
    }
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
        rotation: 0.0,
        mirror: false,
    };

    /// `scale` 的取值范围。
    pub const SCALE_RANGE: (f32, f32) = (0.01, 10.0);

    /// 是否满足不变式：都是有限数、缩放在范围内、不透明度 0–1、裁剪不越界。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let c = self.crop;
        let all = [
            self.x,
            self.y,
            self.scale,
            self.opacity,
            self.rotation,
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

/// 一个图层在画布上的几何。`full`、`shown` 是转之前的框；实际画面是它们绕 `center`
/// 顺时针转 `angle` 弧度后的样子（见 [`LayerGeometry::corners`]）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayerGeometry {
    /// 未裁剪的整幅画面框（转之前）。
    pub full: Rect,
    /// 裁剪后实际显示的部分（转之前）。
    pub shown: Rect,
    /// `shown` 对应的纹理坐标（0–1）。
    pub uv: Rect,
    /// 旋转中心：整幅画面框的中心。
    pub center: (f32, f32),
    /// 旋转角（弧度，顺时针为正）。
    pub angle: f32,
}

impl LayerGeometry {
    /// 把转之前的点绕中心转到画布上。
    #[must_use]
    pub fn rotate(&self, p: (f32, f32)) -> (f32, f32) {
        turn(p, self.center, self.angle)
    }

    /// 把画布上的点转回转之前的坐标（命中测试用）。
    #[must_use]
    pub fn to_local(&self, p: (f32, f32)) -> (f32, f32) {
        turn(p, self.center, -self.angle)
    }

    /// 显示部分在画布上的四个角：左上、右上、右下、左下（按画面自己的方向）。
    #[must_use]
    pub fn corners(&self) -> [(f32, f32); 4] {
        let s = self.shown;
        [
            (s.x, s.y),
            (s.x + s.w, s.y),
            (s.x + s.w, s.y + s.h),
            (s.x, s.y + s.h),
        ]
        .map(|p| self.rotate(p))
    }

    /// 显示部分转过之后的外接矩形（对齐吸附用）。
    #[must_use]
    pub fn bounds(&self) -> Rect {
        if self.angle == 0.0 {
            return self.shown;
        }
        let c = self.corners();
        let (mut x0, mut y0, mut x1, mut y1) = (c[0].0, c[0].1, c[0].0, c[0].1);
        for (x, y) in c {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }

    /// 画布上的点是否落在显示部分里（算上旋转）。
    #[must_use]
    pub fn contains(&self, px: f32, py: f32) -> bool {
        let (x, y) = self.to_local((px, py));
        self.shown.contains(x, y)
    }
}

/// 角度换算到 (-180°, 180°]：450° → 90°，-190° → 170°，-180° → 180°。
#[must_use]
pub fn wrap_degrees(d: f32) -> f32 {
    let r = d.rem_euclid(360.0);
    if r > 180.0 { r - 360.0 } else { r }
}

/// 点 `p` 绕 `c` 顺时针转 `angle` 弧度（屏幕坐标 y 向下）。
fn turn(p: (f32, f32), c: (f32, f32), angle: f32) -> (f32, f32) {
    if angle == 0.0 {
        return p;
    }
    let (sin, cos) = angle.sin_cos();
    let (dx, dy) = (p.0 - c.0, p.1 - c.1);
    (
        c.0 + dx.mul_add(cos, -(dy * sin)),
        c.1 + dx.mul_add(sin, dy * cos),
    )
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
        // 镜像：纹理横向倒着取（从右往左），显示的位置和裁剪都不变
        uv: if t.mirror {
            make(1.0 - cut_l, cut_t, -keep_w, keep_h)
        } else {
            make(cut_l, cut_t, keep_w, keep_h)
        },
        center: (to_f32(left + full_w / 2.0), to_f32(top + full_h / 2.0)),
        angle: t.rotation.to_radians(),
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
    fn mirror_reads_the_texture_right_to_left() {
        let t = Transform {
            mirror: true,
            crop: Crop {
                left: 0.1,
                right: 0.2,
                ..Crop::default()
            },
            ..Transform::default()
        };
        let plain = layer_geometry(
            (1920, 1080),
            (1920, 1080),
            &Transform { mirror: false, ..t },
        );
        let g = layer_geometry((1920, 1080), (1920, 1080), &t);
        assert_eq!(g.shown, plain.shown, "位置、大小不变");
        // 看到的左边裁掉 10%：最左边显示的是源画面 90% 处，往左读到 20% 处
        assert!(close(g.uv, rect(0.9, 0.0, -0.7, 1.0)), "{g:?}");
        assert!(close(plain.uv, rect(0.1, 0.0, 0.7, 1.0)), "{plain:?}");
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

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-2 && (a.1 - b.1).abs() < 1e-2
    }

    #[test]
    fn rotation_turns_corners_clockwise_about_center() {
        let t = Transform {
            scale: 0.5,
            rotation: 90.0,
            ..Transform::default()
        };
        // 1000×500 画布里 500×250 的框，中心 (500, 250)；顺时针转 90°
        let g = layer_geometry((1000, 500), (1000, 500), &t);
        assert_eq!(g.center, (500.0, 250.0));
        let [tl, tr, br, bl] = g.corners();
        assert!(near(tl, (625.0, 0.0)), "{tl:?}");
        assert!(near(tr, (625.0, 500.0)), "{tr:?}");
        assert!(near(br, (375.0, 500.0)), "{br:?}");
        assert!(near(bl, (375.0, 0.0)), "{bl:?}");
        let b = g.bounds();
        assert!(
            near((b.x, b.y), (375.0, 0.0)) && near((b.w, b.h), (250.0, 500.0)),
            "{b:?}"
        );
        // 转过以后：原来框外、现在框内的点算在里面，反之不算
        assert!(g.contains(500.0, 50.0));
        assert!(!g.contains(300.0, 250.0));
        // 不转时 corners 就是 shown 的四个角
        let flat = layer_geometry((1000, 500), (1000, 500), &Transform::default());
        assert_eq!(flat.corners()[0], (0.0, 0.0));
        assert_eq!(flat.bounds(), flat.shown);
    }

    #[test]
    fn cropped_rotation_pivots_on_full_frame_center() {
        let t = Transform {
            rotation: 180.0,
            crop: Crop {
                left: 0.5,
                ..Crop::default()
            },
            ..Transform::default()
        };
        // 裁掉左半边后转 180°：剩下的右半边绕整幅画面中心转到左边
        let g = layer_geometry((1000, 500), (1000, 500), &t);
        let b = g.bounds();
        assert!(
            near((b.x, b.y), (0.0, 0.0)) && near((b.w, b.h), (500.0, 500.0)),
            "{b:?}"
        );
    }

    #[test]
    fn degrees_wrap_to_half_turn() {
        for (d, want) in [
            (0.0, 0.0),
            (450.0, 90.0),
            (-190.0, 170.0),
            (-180.0, 180.0),
            (180.0, 180.0),
            (359.5, -0.5),
        ] {
            assert!(
                (wrap_degrees(d) - want).abs() < 1e-4,
                "{d} → {}",
                wrap_degrees(d)
            );
        }
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
                rotation: f32::INFINITY,
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
            let tf = Transform { x, y, scale, crop: Crop { left: l, top: t, right: r, bottom: b }, opacity: 1.0, rotation: 0.0, mirror: false };
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

    #[test]
    fn a_group_transform_moves_scales_turns_and_fades_its_layers() {
        let child = Transform {
            x: 0.25,
            y: 0.0,
            scale: 0.5,
            opacity: 0.8,
            rotation: 10.0,
            ..Transform::default()
        };
        let same = Transform::default().compose(&child, 1920, 1080);
        assert!((same.x - 0.25).abs() < 1e-6 && (same.scale - 0.5).abs() < 1e-6);
        // 整组缩一半、往右挪 0.1：位置跟着缩
        let half = Transform {
            x: 0.1,
            scale: 0.5,
            opacity: 0.5,
            ..Transform::default()
        };
        let c = half.compose(&child, 1920, 1080);
        assert!((c.x - (0.125 + 0.1)).abs() < 1e-6);
        assert!((c.scale - 0.25).abs() < 1e-6);
        assert!((c.opacity - 0.4).abs() < 1e-6);
        // 整组顺时针转 90°：右边 480 像素的图层转到下面 480 像素（画布 1920×1080）
        let turned = Transform {
            rotation: 90.0,
            ..Transform::default()
        };
        let t = turned.compose(&child, 1920, 1080);
        assert!(t.x.abs() < 1e-5, "{}", t.x);
        assert!((t.y - 480.0 / 1080.0).abs() < 1e-5, "{}", t.y);
        assert!((t.rotation - 100.0).abs() < 1e-5);
        // 整组镜像：位置左右翻、角度反过来、镜像叠加
        let flipped = Transform {
            mirror: true,
            ..Transform::default()
        };
        let m = flipped.compose(&child, 1920, 1080);
        assert!((m.x + 0.25).abs() < 1e-6 && (m.rotation + 10.0).abs() < 1e-6 && m.mirror);
    }

    #[test]
    fn a_mirrored_group_swaps_its_layers_left_and_right_crop() {
        let child = Transform {
            crop: Crop {
                left: 0.5,
                top: 0.1,
                right: 0.0,
                bottom: 0.2,
            },
            ..Transform::default()
        };
        let flipped = Transform {
            mirror: true,
            ..Transform::default()
        };
        let c = flipped.compose(&child, 1920, 1080).crop;
        assert!((c.right - 0.5).abs() < 1e-6 && c.left.abs() < 1e-6, "{c:?}");
        assert!((c.top - 0.1).abs() < 1e-6 && (c.bottom - 0.2).abs() < 1e-6);
    }
}
