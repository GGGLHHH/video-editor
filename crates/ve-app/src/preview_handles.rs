//! 预览上直接拖动画面：命中测试、移动（带吸附）、拖角缩放。纯函数，坐标都是画布像素。

use ve_core::{LayerGeometry, Rect, Transform};

/// 四个角。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    /// 左上。
    TopLeft,
    /// 右上。
    TopRight,
    /// 左下。
    BottomLeft,
    /// 右下。
    BottomRight,
}

impl Corner {
    /// 全部四个角。
    pub const ALL: [Self; 4] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    /// 这个角在矩形上的位置。
    #[must_use]
    pub fn point(self, r: Rect) -> (f32, f32) {
        match self {
            Self::TopLeft => (r.x, r.y),
            Self::TopRight => (r.x + r.w, r.y),
            Self::BottomLeft => (r.x, r.y + r.h),
            Self::BottomRight => (r.x + r.w, r.y + r.h),
        }
    }
}

/// 点在画面框的哪里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleHit {
    /// 框内：拖动移动。
    Body,
    /// 角上：拖动缩放。
    Corner(Corner),
    /// 框外。
    None,
}

/// 命中测试：角优先（角的判定范围 `corner` 像素，可以略出框外）。
#[must_use]
pub fn hit(shown: Rect, p: (f32, f32), corner: f32) -> HandleHit {
    for c in Corner::ALL {
        let (cx, cy) = c.point(shown);
        if (p.0 - cx).abs() <= corner && (p.1 - cy).abs() <= corner {
            return HandleHit::Corner(c);
        }
    }
    if shown.contains(p.0, p.1) {
        HandleHit::Body
    } else {
        HandleHit::None
    }
}

/// 一个方向上的吸附：画面的这几条线（左边、中线、右边）里，离画布的这几条线（0、中线、满）
/// 最近的一对距离小于 `snap` 时，返回把它们对齐要补的位移。
fn snap_axis(edges: [f32; 3], targets: [f32; 3], snap: f32) -> f32 {
    let mut best: Option<f32> = None;
    for e in edges {
        for t in targets {
            let d = t - e;
            if d.abs() < snap && best.is_none_or(|b| d.abs() < b.abs()) {
                best = Some(d);
            }
        }
    }
    best.unwrap_or(0.0)
}

/// 拖动移动：`delta` 是从按下开始的总位移（画布像素）。显示的框的边缘或中线
/// 离画布边缘或中线不到 `snap` 像素时吸过去。
#[must_use]
pub fn drag_move(
    start: &Transform,
    canvas: (f32, f32),
    delta: (f32, f32),
    snap: f32,
    geom: &LayerGeometry,
) -> Transform {
    let s = geom.shown;
    let (x0, y0) = (s.x + delta.0, s.y + delta.1);
    let dx = delta.0
        + snap_axis(
            [x0, x0 + s.w / 2.0, x0 + s.w],
            [0.0, canvas.0 / 2.0, canvas.0],
            snap,
        );
    let dy = delta.1
        + snap_axis(
            [y0, y0 + s.h / 2.0, y0 + s.h],
            [0.0, canvas.1 / 2.0, canvas.1],
            snap,
        );
    Transform {
        x: start.x + dx / canvas.0.max(1.0),
        y: start.y + dy / canvas.1.max(1.0),
        ..*start
    }
}

/// 拖角缩放：以整幅画面框的中心为基准等比缩放，缩放比 = 指针到中心的距离 / 这个角原来到中心的距离。
#[must_use]
pub fn drag_scale(
    start: &Transform,
    geom: &LayerGeometry,
    corner: Corner,
    p: (f32, f32),
) -> Transform {
    let f = geom.full;
    let center = (f.x + f.w / 2.0, f.y + f.h / 2.0);
    let (cx, cy) = corner.point(geom.shown);
    let from = (cx - center.0).hypot(cy - center.1);
    let to = (p.0 - center.0).hypot(p.1 - center.1);
    let ratio = if from > 0.0 { to / from } else { 1.0 };
    Transform {
        scale: (start.scale * ratio).clamp(Transform::SCALE_RANGE.0, Transform::SCALE_RANGE.1),
        ..*start
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::layer_geometry;

    const CANVAS: (u32, u32) = (1000, 500);

    fn half() -> (Transform, LayerGeometry) {
        let t = Transform {
            scale: 0.5,
            ..Transform::default()
        };
        (t, layer_geometry(CANVAS, CANVAS, &t))
    }

    #[test]
    fn hit_prefers_corners_then_body() {
        let (_, g) = half();
        // 框是 (250, 125) 到 (750, 375)
        assert_eq!(
            hit(g.shown, (255.0, 128.0), 8.0),
            HandleHit::Corner(Corner::TopLeft)
        );
        assert_eq!(
            hit(g.shown, (756.0, 380.0), 8.0),
            HandleHit::Corner(Corner::BottomRight)
        );
        assert_eq!(hit(g.shown, (500.0, 250.0), 8.0), HandleHit::Body);
        assert_eq!(hit(g.shown, (100.0, 100.0), 8.0), HandleHit::None);
    }

    #[test]
    fn move_snaps_to_center() {
        let (t, g) = half();
        // 中线离画布中线 5 像素：吸回去
        let near = drag_move(&t, (1000.0, 500.0), (5.0, 0.0), 8.0, &g);
        assert!(near.x.abs() < 1e-6, "{near:?}");
        // 9 像素：不吸
        let far = drag_move(&t, (1000.0, 500.0), (9.0, 0.0), 8.0, &g);
        assert!((far.x - 0.009).abs() < 1e-6, "{far:?}");
        // 右边贴到画布右边缘：框右边 750 + 246 = 996，差 4 像素吸到 1000
        let edge = drag_move(&t, (1000.0, 500.0), (246.0, 0.0), 8.0, &g);
        assert!((edge.x - 0.25).abs() < 1e-6, "{edge:?}");
        // 其他属性不变
        assert_eq!(edge.scale, t.scale);
    }

    #[test]
    fn corner_drag_scales_uniformly() {
        let (t, g) = half();
        // 右下角 (750, 375) 离中心 (500, 250) 的 1.5 倍处
        let bigger = drag_scale(&t, &g, Corner::BottomRight, (875.0, 437.5));
        assert!((bigger.scale - 0.75).abs() < 1e-5, "{bigger:?}");
        assert_eq!((bigger.x, bigger.y), (t.x, t.y));
        // 往里拖：变小
        let smaller = drag_scale(&t, &g, Corner::TopLeft, (375.0, 187.5));
        assert!((smaller.scale - 0.25).abs() < 1e-5, "{smaller:?}");
    }

    #[test]
    fn scale_clamped() {
        let (t, g) = half();
        let tiny = drag_scale(&t, &g, Corner::BottomRight, (500.0, 250.0));
        assert_eq!(tiny.scale, Transform::SCALE_RANGE.0);
        let huge = drag_scale(&t, &g, Corner::BottomRight, (1e6, 1e6));
        assert_eq!(huge.scale, Transform::SCALE_RANGE.1);
    }
}
