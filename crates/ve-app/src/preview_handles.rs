//! 预览上直接操作画面：命中测试、移动（吸附到画布和其他图层的边缘 / 中线）、拖角缩放、拖手柄旋转。
//! 纯函数，坐标都是画布像素；转过的图层按它实际的四个角算。

use gpui_kit::CursorStyle;
use ve_core::{LayerGeometry, Rect, Transform, wrap_degrees};

/// 旋转吸附：离 0°/90°/180°/270° 不到这么多度时吸过去。
const ANGLE_SNAP_DEG: f32 = 4.0;
/// 按住 Shift 旋转时的一档。
const ANGLE_STEP_DEG: f32 = 15.0;

/// 四个角（按画面自己的方向）。
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

    /// 这个角在（转之前的）矩形上的位置。
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
    /// 旋转手柄上，或者角的外侧一点：拖动旋转。
    Rotate,
    /// 框外。
    None,
}

/// 旋转手柄的位置：画面上边中点再往「画面的上方」`offset` 像素，跟着画面一起转。
#[must_use]
pub fn rotate_handle(geom: &LayerGeometry, offset: f32) -> (f32, f32) {
    let s = geom.shown;
    geom.rotate((s.x + s.w / 2.0, s.y - offset))
}

/// 命中测试：旋转手柄 > 角 > 框内 > 角的外侧（旋转区）。`tolerance` 是手柄的判定半径，
/// `offset` 是旋转手柄离上边的距离，`zone` 是角手柄外面再往外多远算旋转区（和 Figma、
/// Photoshop 一样：鼠标移到角外侧一点就能转）。
#[must_use]
pub fn hit(
    geom: &LayerGeometry,
    p: (f32, f32),
    tolerance: f32,
    offset: f32,
    zone: f32,
) -> HandleHit {
    let near = |q: (f32, f32)| (p.0 - q.0).abs() <= tolerance && (p.1 - q.1).abs() <= tolerance;
    if near(rotate_handle(geom, offset)) {
        return HandleHit::Rotate;
    }
    let corners = Corner::ALL.map(|c| (c, geom.rotate(c.point(geom.shown))));
    if let Some((c, _)) = corners.iter().find(|(_, q)| near(*q)) {
        return HandleHit::Corner(*c);
    }
    if geom.contains(p.0, p.1) {
        return HandleHit::Body;
    }
    let reach = tolerance + zone;
    if corners
        .iter()
        .any(|(_, q)| (p.0 - q.0).hypot(p.1 - q.1) <= reach)
    {
        HandleHit::Rotate
    } else {
        HandleHit::None
    }
}

/// 角上的缩放箭头：沿这个角的对角线方向（左上 / 右下是「\」，右上 / 左下是「/」），
/// 再加上画面的旋转角，取最接近的四种箭头之一。
#[must_use]
pub fn corner_cursor(geom: &LayerGeometry, corner: Corner) -> CursorStyle {
    let base = match corner {
        Corner::TopLeft | Corner::BottomRight => 45.0,
        Corner::TopRight | Corner::BottomLeft => 135.0,
    };
    // 屏幕 y 向下：45° 指向右下，即「\」
    let axis = (base + geom.angle.to_degrees()).rem_euclid(180.0);
    match axis {
        a if a < 22.5 || a >= 157.5 => CursorStyle::ResizeLeftRight,
        a if a < 67.5 => CursorStyle::ResizeUpLeftDownRight,
        a if a < 112.5 => CursorStyle::ResizeUpDown,
        _ => CursorStyle::ResizeUpRightDownLeft,
    }
}

/// 鼠标样式：提示按下去会做什么。框内张开的手（拖动中握拳），角上缩放箭头，
/// 旋转手柄上是手指（没有旋转光标可用），别处箭头。
#[must_use]
pub fn cursor_for(hit: HandleHit, geom: &LayerGeometry, dragging: bool) -> CursorStyle {
    match hit {
        HandleHit::Body if dragging => CursorStyle::ClosedHand,
        HandleHit::Body => CursorStyle::OpenHand,
        HandleHit::Corner(c) => corner_cursor(geom, c),
        HandleHit::Rotate => CursorStyle::PointingHand,
        HandleHit::None => CursorStyle::Arrow,
    }
}

/// 移动时可以吸附的线：画布的边和中线，加上其他图层（外接框）的边和中线。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Guides {
    /// 竖线的横坐标。
    pub xs: Vec<f32>,
    /// 横线的纵坐标。
    pub ys: Vec<f32>,
}

impl Guides {
    /// 由画布尺寸和其他图层的几何生成。
    #[must_use]
    pub fn new(canvas: (f32, f32), others: &[LayerGeometry]) -> Self {
        let mut xs = vec![0.0, canvas.0 / 2.0, canvas.0];
        let mut ys = vec![0.0, canvas.1 / 2.0, canvas.1];
        for g in others {
            let b = g.bounds();
            xs.extend([b.x, b.x + b.w / 2.0, b.x + b.w]);
            ys.extend([b.y, b.y + b.h / 2.0, b.y + b.h]);
        }
        Self { xs, ys }
    }
}

/// 一次移动的结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moved {
    /// 新的画面属性。
    pub transform: Transform,
    /// 吸上的竖线、横线（画对齐参考线用）。
    pub lines: (Option<f32>, Option<f32>),
}

/// 一个方向上的吸附：画面的这几条线（左边、中线、右边）里，离某条目标线最近的一对距离小于 `snap`
/// 时，返回（要补的位移，那条目标线）。
fn snap_axis(edges: [f32; 3], targets: &[f32], snap: f32) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32)> = None;
    for e in edges {
        for &t in targets {
            let d = t - e;
            if d.abs() < snap && best.is_none_or(|(b, _)| d.abs() < b.abs()) {
                best = Some((d, t));
            }
        }
    }
    best
}

/// 拖动移动：`delta` 是从按下开始的总位移（画布像素）。画面（转过的按外接框）的边或中线
/// 离某条吸附线不到 `snap` 像素时吸过去；`snap` 为 0 就是不吸。
#[must_use]
pub fn drag_move(
    start: &Transform,
    canvas: (f32, f32),
    delta: (f32, f32),
    snap: f32,
    geom: &LayerGeometry,
    guides: &Guides,
) -> Moved {
    let b = geom.bounds();
    let (x0, y0) = (b.x + delta.0, b.y + delta.1);
    let sx = snap_axis([x0, x0 + b.w / 2.0, x0 + b.w], &guides.xs, snap);
    let sy = snap_axis([y0, y0 + b.h / 2.0, y0 + b.h], &guides.ys, snap);
    let dx = delta.0 + sx.map_or(0.0, |(d, _)| d);
    let dy = delta.1 + sy.map_or(0.0, |(d, _)| d);
    Moved {
        transform: Transform {
            x: start.x + dx / canvas.0.max(1.0),
            y: start.y + dy / canvas.1.max(1.0),
            ..*start
        },
        lines: (sx.map(|(_, l)| l), sy.map(|(_, l)| l)),
    }
}

/// 拖角缩放：以整幅画面框的中心为基准等比缩放，缩放比 = 指针到中心的距离 / 这个角原来到中心的距离。
/// 距离和方向无关，所以转过的图层也一样算。
#[must_use]
pub fn drag_scale(
    start: &Transform,
    geom: &LayerGeometry,
    corner: Corner,
    p: (f32, f32),
) -> Transform {
    let center = geom.center;
    let (cx, cy) = corner.point(geom.shown);
    let from = (cx - center.0).hypot(cy - center.1);
    let to = (p.0 - center.0).hypot(p.1 - center.1);
    let ratio = if from > 0.0 { to / from } else { 1.0 };
    Transform {
        scale: (start.scale * ratio).clamp(Transform::SCALE_RANGE.0, Transform::SCALE_RANGE.1),
        ..*start
    }
}

/// 拖手柄旋转：转过的角度 = 指针相对中心的方向变化（从按下的 `origin` 到 `p`）。
/// `snap` 时靠近 0°/90°/180°/270° 吸过去；`step`（按住 Shift）时按 15° 一档。
#[must_use]
pub fn drag_rotate(
    start: &Transform,
    geom: &LayerGeometry,
    origin: (f32, f32),
    p: (f32, f32),
    snap: bool,
    step: bool,
) -> Transform {
    let c = geom.center;
    let from = (origin.1 - c.1).atan2(origin.0 - c.0);
    let to = (p.1 - c.1).atan2(p.0 - c.0);
    let mut deg = start.rotation + (to - from).to_degrees();
    if step {
        deg = (deg / ANGLE_STEP_DEG).round() * ANGLE_STEP_DEG;
    } else if snap {
        let right = (deg / 90.0).round() * 90.0;
        if (deg - right).abs() < ANGLE_SNAP_DEG {
            deg = right;
        }
    }
    Transform {
        rotation: wrap_degrees(deg),
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

    fn turned(deg: f32) -> (Transform, LayerGeometry) {
        let t = Transform {
            scale: 0.5,
            rotation: deg,
            ..Transform::default()
        };
        (t, layer_geometry(CANVAS, CANVAS, &t))
    }

    fn canvas_only() -> Guides {
        Guides::new((1000.0, 500.0), &[])
    }

    #[test]
    fn hit_prefers_rotate_handle_then_corners_then_body() {
        let (_, g) = half();
        // 框是 (250, 125) 到 (750, 375)；旋转手柄在上边中点再往上 30：(500, 95)
        assert_eq!(hit(&g, (502.0, 97.0), 8.0, 30.0, 20.0), HandleHit::Rotate);
        assert_eq!(
            hit(&g, (255.0, 128.0), 8.0, 30.0, 20.0),
            HandleHit::Corner(Corner::TopLeft)
        );
        assert_eq!(
            hit(&g, (756.0, 380.0), 8.0, 30.0, 20.0),
            HandleHit::Corner(Corner::BottomRight)
        );
        assert_eq!(hit(&g, (500.0, 250.0), 8.0, 30.0, 20.0), HandleHit::Body);
        assert_eq!(hit(&g, (100.0, 100.0), 8.0, 30.0, 20.0), HandleHit::None);
    }

    #[test]
    fn just_outside_a_corner_rotates() {
        let (_, g) = half();
        // 左上角 (250, 125) 外侧 14 像素（超出角手柄 8 的范围、在旋转区 8+20 以内）：旋转
        assert_eq!(hit(&g, (240.0, 115.0), 8.0, 30.0, 20.0), HandleHit::Rotate);
        // 太远：什么都不是
        assert_eq!(hit(&g, (220.0, 95.0), 8.0, 30.0, 20.0), HandleHit::None);
        // 框外、但靠近边的中间（不靠近角）：不是旋转
        assert_eq!(hit(&g, (400.0, 115.0), 8.0, 30.0, 20.0), HandleHit::None);
        // 角的内侧仍是画面本身（拖动移动）
        assert_eq!(hit(&g, (265.0, 140.0), 8.0, 30.0, 20.0), HandleHit::Body);
        // 转 90° 后，旋转区跟着角走：左上角转到 (625, 0)
        let (_, r) = turned(90.0);
        assert_eq!(hit(&r, (640.0, -10.0), 8.0, 30.0, 20.0), HandleHit::Rotate);
    }

    #[test]
    fn hit_follows_rotation() {
        // 顺时针转 90°：框变成竖的 (375, 0) 到 (625, 500)，左上角转到 (625, 0)
        let (_, g) = turned(90.0);
        assert_eq!(hit(&g, (500.0, 30.0), 8.0, 30.0, 20.0), HandleHit::Body);
        assert_eq!(hit(&g, (300.0, 250.0), 8.0, 30.0, 20.0), HandleHit::None);
        assert_eq!(
            hit(&g, (623.0, 3.0), 8.0, 30.0, 20.0),
            HandleHit::Corner(Corner::TopLeft)
        );
        // 旋转手柄跟着转到右边：(655, 250)
        assert_eq!(hit(&g, (655.0, 250.0), 8.0, 30.0, 20.0), HandleHit::Rotate);
        let handle = rotate_handle(&g, 30.0);
        assert!((handle.0 - 655.0).abs() < 1e-3 && (handle.1 - 250.0).abs() < 1e-3);
    }

    #[test]
    fn move_snaps_to_canvas_center_and_edges() {
        let (t, g) = half();
        // 中线离画布中线 5 像素：吸回去，并报出吸到的竖线
        let near = drag_move(&t, (1000.0, 500.0), (5.0, 0.0), 8.0, &g, &canvas_only());
        assert!(near.transform.x.abs() < 1e-6, "{near:?}");
        assert_eq!(near.lines, (Some(500.0), Some(250.0)));
        // 9 像素：不吸（横向不报线；纵向没动，仍贴着中线）
        let far = drag_move(&t, (1000.0, 500.0), (9.0, 0.0), 8.0, &g, &canvas_only());
        assert!((far.transform.x - 0.009).abs() < 1e-6, "{far:?}");
        assert_eq!(far.lines.0, None);
        // 右边贴到画布右边缘：框右边 750 + 246 = 996，差 4 像素吸到 1000
        let edge = drag_move(&t, (1000.0, 500.0), (246.0, 0.0), 8.0, &g, &canvas_only());
        assert!((edge.transform.x - 0.25).abs() < 1e-6, "{edge:?}");
        // 框宽正好半个画布：左边同时贴上画布中线（都差 4 像素），报先找到的那条
        assert_eq!(edge.lines.0, Some(500.0));
        assert_eq!(edge.transform.scale, t.scale);
        // 吸附关掉（范围 0）：一点不吸，也不报线
        let off = drag_move(&t, (1000.0, 500.0), (5.0, 0.0), 0.0, &g, &canvas_only());
        assert!((off.transform.x - 0.005).abs() < 1e-6);
        assert_eq!(off.lines, (None, None));
    }

    #[test]
    fn move_snaps_to_other_layers_edges() {
        let (t, g) = half();
        // 另一层占 (0, 50) 到 (200, 150)：拖动的框左边从 250 往左 46 → 204，吸到它的右边 200
        let other = layer_geometry(
            CANVAS,
            CANVAS,
            &Transform {
                x: -0.4,
                y: -0.3,
                scale: 0.2,
                ..Transform::default()
            },
        );
        let guides = Guides::new((1000.0, 500.0), &[other]);
        let out = drag_move(&t, (1000.0, 500.0), (-46.0, -37.0), 8.0, &g, &guides);
        assert!((out.transform.x - (-0.05)).abs() < 1e-5, "{out:?}");
        assert_eq!(out.lines.0, Some(200.0));
        // 纵向：框上边 125 - 37 = 88，离另一层下边 150、中线 100 都远于 8 → 不吸
        assert_eq!(out.lines.1, None);
    }

    #[test]
    fn rotated_layer_snaps_by_its_bounding_box() {
        // 转 90° 后外接框 (375, 0)–(625, 500)：右移 372 → 右边 997，吸到画布右边 1000
        let (t, g) = turned(90.0);
        let out = drag_move(&t, (1000.0, 500.0), (372.0, 0.0), 8.0, &g, &canvas_only());
        assert!((out.transform.x - 0.375).abs() < 1e-5, "{out:?}");
    }

    #[test]
    fn rotate_drag_follows_pointer_angle_and_snaps() {
        let (t, g) = half();
        let c = g.center;
        let at = |deg: f32| {
            let r = deg.to_radians();
            (c.0 + 100.0 * r.cos(), c.1 + 100.0 * r.sin())
        };
        // 从正右方拖到正下方：顺时针 90°
        let quarter = drag_rotate(&t, &g, at(0.0), at(90.0), false, false);
        assert!((quarter.rotation - 90.0).abs() < 1e-3, "{quarter:?}");
        // 87°：吸附打开时吸到 90°，关掉时保持 87°
        assert!((drag_rotate(&t, &g, at(0.0), at(87.0), true, false).rotation - 90.0).abs() < 1e-3);
        assert!(
            (drag_rotate(&t, &g, at(0.0), at(87.0), false, false).rotation - 87.0).abs() < 1e-3
        );
        // 按住 Shift：15° 一档
        assert!((drag_rotate(&t, &g, at(0.0), at(37.0), false, true).rotation - 30.0).abs() < 1e-3);
        // 跨过 180° 换算回 (-180, 180]
        let start = Transform {
            rotation: 170.0,
            ..t
        };
        let wrapped = drag_rotate(&start, &g, at(0.0), at(30.0), false, false);
        assert!((wrapped.rotation - (-160.0)).abs() < 1e-3, "{wrapped:?}");
        assert_eq!(wrapped.scale, t.scale);
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
        // 转过的图层：按到中心的距离算，和方向无关
        let (rt, rg) = turned(45.0);
        let r = drag_scale(
            &rt,
            &rg,
            Corner::BottomRight,
            (500.0, 250.0 + 1.5 * 279.508_5),
        );
        assert!((r.scale - 0.75).abs() < 1e-3, "{r:?}");
    }

    #[test]
    fn corner_cursor_follows_rotation() {
        use CursorStyle::{
            ResizeLeftRight, ResizeUpDown, ResizeUpLeftDownRight, ResizeUpRightDownLeft,
        };
        let (_, g) = half();
        assert_eq!(corner_cursor(&g, Corner::TopLeft), ResizeUpLeftDownRight);
        assert_eq!(
            corner_cursor(&g, Corner::BottomRight),
            ResizeUpLeftDownRight
        );
        assert_eq!(corner_cursor(&g, Corner::TopRight), ResizeUpRightDownLeft);
        assert_eq!(corner_cursor(&g, Corner::BottomLeft), ResizeUpRightDownLeft);
        // 转 90°：左上角转到右上，箭头跟着换方向
        assert_eq!(
            corner_cursor(&turned(90.0).1, Corner::TopLeft),
            ResizeUpRightDownLeft
        );
        // 转 45°：左上角转到正上方，右上角转到正右方
        assert_eq!(
            corner_cursor(&turned(45.0).1, Corner::TopLeft),
            ResizeUpDown
        );
        assert_eq!(
            corner_cursor(&turned(45.0).1, Corner::TopRight),
            ResizeLeftRight
        );
        assert_eq!(
            corner_cursor(&turned(-45.0).1, Corner::TopLeft),
            ResizeLeftRight
        );
    }

    #[test]
    fn cursor_matches_what_a_drag_would_do() {
        let (_, g) = half();
        assert_eq!(cursor_for(HandleHit::None, &g, false), CursorStyle::Arrow);
        assert_eq!(
            cursor_for(HandleHit::Body, &g, false),
            CursorStyle::OpenHand
        );
        assert_eq!(
            cursor_for(HandleHit::Body, &g, true),
            CursorStyle::ClosedHand
        );
        assert_eq!(
            cursor_for(HandleHit::Rotate, &g, false),
            CursorStyle::PointingHand
        );
        assert_eq!(
            cursor_for(HandleHit::Rotate, &g, true),
            CursorStyle::PointingHand
        );
        let corner = HandleHit::Corner(Corner::TopRight);
        assert_eq!(
            cursor_for(corner, &g, false),
            CursorStyle::ResizeUpRightDownLeft
        );
        assert_eq!(
            cursor_for(corner, &g, true),
            CursorStyle::ResizeUpRightDownLeft
        );
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
