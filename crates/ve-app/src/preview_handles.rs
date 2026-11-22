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

/// 四条边的中点（按画面自己的方向）。剪映的选中框除了四个角还有四个边中点手柄。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// 上边。
    Top,
    /// 下边。
    Bottom,
    /// 左边。
    Left,
    /// 右边。
    Right,
}

impl Edge {
    /// 全部四条边。
    pub const ALL: [Self; 4] = [Self::Top, Self::Bottom, Self::Left, Self::Right];

    /// 这条边的中点在（转之前的）矩形上的位置。
    #[must_use]
    pub fn point(self, r: Rect) -> (f32, f32) {
        match self {
            Self::Top => (r.x + r.w / 2.0, r.y),
            Self::Bottom => (r.x + r.w / 2.0, r.y + r.h),
            Self::Left => (r.x, r.y + r.h / 2.0),
            Self::Right => (r.x + r.w, r.y + r.h / 2.0),
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
    /// 边中点上：拖动缩放（等比，和剪映「等比缩放」开着时一样）。
    Edge(Edge),
    /// 旋转手柄上，或者角的外侧一点：拖动旋转。
    Rotate,
    /// 框外。
    None,
}

/// 旋转手柄的位置：画面下边中点再往「画面的下方」`offset` 像素（剪映的位置），跟着画面一起转。
#[must_use]
pub fn rotate_handle(geom: &LayerGeometry, offset: f32) -> (f32, f32) {
    let s = geom.shown;
    geom.rotate((s.x + s.w / 2.0, s.y + s.h + offset))
}

/// 命中测试：旋转手柄 > 角 > 边中点 > 框内 > 角的外侧（旋转区）。`tolerance` 是手柄的判定半径，
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
    if let Some(e) = Edge::ALL
        .into_iter()
        .find(|e| near(geom.rotate(e.point(geom.shown))))
    {
        return HandleHit::Edge(e);
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
    axis_cursor(base + geom.angle.to_degrees())
}

/// 边中点上的缩放箭头：上下边是上下箭头、左右边是左右箭头，再加上画面的旋转角。
#[must_use]
pub fn edge_cursor(geom: &LayerGeometry, edge: Edge) -> CursorStyle {
    let base = match edge {
        Edge::Top | Edge::Bottom => 90.0,
        Edge::Left | Edge::Right => 0.0,
    };
    axis_cursor(base + geom.angle.to_degrees())
}

/// 沿 `degrees` 方向（屏幕 y 向下：45° 指向右下，即「\」）最接近的双向箭头。
fn axis_cursor(degrees: f32) -> CursorStyle {
    let axis = degrees.rem_euclid(180.0);
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
        HandleHit::Edge(e) => edge_cursor(geom, e),
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
    let x = start.x + dx / canvas.0.max(1.0);
    let y = start.y + dy / canvas.1.max(1.0);
    Moved {
        transform: Transform {
            // 吸住的轴去掉浮点尾巴：吸到中线就是精确的 0（像素换算会留下 1e-8 级的误差）
            x: if sx.is_some() { tidy(x) } else { x },
            y: if sy.is_some() { tidy(y) } else { y },
            ..*start
        },
        lines: (sx.map(|(_, l)| l), sy.map(|(_, l)| l)),
    }
}

/// 属性面板「对齐」按钮（剪映的六个对齐）：把图层（转过的按外接框）贴到画布的边或中线。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// 左对齐。
    Left,
    /// 水平居中。
    CenterX,
    /// 右对齐。
    Right,
    /// 顶对齐。
    Top,
    /// 垂直居中。
    CenterY,
    /// 底对齐。
    Bottom,
}

impl Align {
    /// 从左到右的按钮顺序。
    pub const ALL: [Self; 6] = [
        Self::Left,
        Self::CenterX,
        Self::Right,
        Self::Top,
        Self::CenterY,
        Self::Bottom,
    ];
}

/// 对齐后的画面属性：只改位置，`canvas` 是画布像素尺寸。
#[must_use]
pub fn align(t: &Transform, geom: &LayerGeometry, canvas: (f32, f32), how: Align) -> Transform {
    let b = geom.bounds();
    let (cw, ch) = (canvas.0.max(1.0), canvas.1.max(1.0));
    let (dx, dy) = match how {
        Align::Left => (-b.x, 0.0),
        Align::CenterX => (cw / 2.0 - (b.x + b.w / 2.0), 0.0),
        Align::Right => (cw - (b.x + b.w), 0.0),
        Align::Top => (0.0, -b.y),
        Align::CenterY => (0.0, ch / 2.0 - (b.y + b.h / 2.0)),
        Align::Bottom => (0.0, ch - (b.y + b.h)),
    };
    Transform {
        x: tidy(t.x + dx / cw),
        y: tidy(t.y + dy / ch),
        ..*t
    }
}

/// 取到 1e-6 的整数倍（1920 宽的画布上约 0.002 像素）。
fn tidy(v: f32) -> f32 {
    (v * 1e6).round() / 1e6
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

/// 拖边中点单向拉伸（剪映）：以整幅画面框的中心为基准，只拉这条边的方向——左右边只改宽、上下边只改高。
/// 拉伸比 = 指针在「中心 → 这条边中点」方向上的投影 / 这条边中点原来到中心的距离（沿边拖动不变）。
#[must_use]
pub fn drag_scale_edge(
    start: &Transform,
    geom: &LayerGeometry,
    edge: Edge,
    p: (f32, f32),
) -> Transform {
    let c = geom.center;
    let m = geom.rotate(edge.point(geom.shown));
    let n = (m.0 - c.0, m.1 - c.1);
    let len2 = n.0.mul_add(n.0, n.1 * n.1);
    let along = (p.0 - c.0).mul_add(n.0, (p.1 - c.1) * n.1);
    let ratio = if len2 > 0.0 { along / len2 } else { 1.0 };
    let (lo, hi) = Transform::STRETCH_RANGE;
    match edge {
        Edge::Left | Edge::Right => {
            // 宽乘 ratio、高不变：scale 变，stretch 反过来补
            let height = start.scale * start.stretch;
            let scale =
                (start.scale * ratio).clamp(Transform::SCALE_RANGE.0, Transform::SCALE_RANGE.1);
            let stretch = (height / scale).clamp(lo, hi);
            Transform {
                scale,
                stretch,
                ..*start
            }
        }
        Edge::Top | Edge::Bottom => Transform {
            stretch: (start.stretch * ratio).clamp(lo, hi),
            ..*start
        },
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
        // 框是 (250, 125) 到 (750, 375)；旋转手柄在下边中点再往下 30：(500, 405)（剪映的位置）
        assert_eq!(hit(&g, (502.0, 403.0), 8.0, 30.0, 20.0), HandleHit::Rotate);
        assert_eq!(
            hit(&g, (500.0, 95.0), 8.0, 30.0, 20.0),
            HandleHit::None,
            "上方不再有手柄"
        );
        // 四条边的中点
        assert_eq!(
            hit(&g, (500.0, 128.0), 8.0, 30.0, 20.0),
            HandleHit::Edge(Edge::Top)
        );
        assert_eq!(
            hit(&g, (500.0, 372.0), 8.0, 30.0, 20.0),
            HandleHit::Edge(Edge::Bottom)
        );
        assert_eq!(
            hit(&g, (253.0, 250.0), 8.0, 30.0, 20.0),
            HandleHit::Edge(Edge::Left)
        );
        assert_eq!(
            hit(&g, (747.0, 250.0), 8.0, 30.0, 20.0),
            HandleHit::Edge(Edge::Right)
        );
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
        // 旋转手柄（原本在下方）跟着转到左边：(345, 250)
        assert_eq!(hit(&g, (345.0, 250.0), 8.0, 30.0, 20.0), HandleHit::Rotate);
        let handle = rotate_handle(&g, 30.0);
        assert!((handle.0 - 345.0).abs() < 1e-3 && (handle.1 - 250.0).abs() < 1e-3);
        // 上边中点转到了右边：(625, 250)，箭头也转成左右
        assert_eq!(
            hit(&g, (622.0, 250.0), 8.0, 30.0, 20.0),
            HandleHit::Edge(Edge::Top)
        );
        assert_eq!(edge_cursor(&g, Edge::Top), CursorStyle::ResizeLeftRight);
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
    fn snapping_back_to_center_lands_exactly_on_zero() {
        // 先偏到一个不整的位置，再拖回中线附近：吸住后位置应是精确的 0，不留浮点尾巴
        for start_x in [0.0137_f32, -0.0291, 0.1, 0.333] {
            let t = Transform {
                scale: 0.5,
                x: start_x,
                ..Transform::default()
            };
            let g = layer_geometry(CANVAS, CANVAS, &t);
            let back = -start_x * 1000.0 + 3.3;
            let out = drag_move(&t, (1000.0, 500.0), (back, 0.0), 8.0, &g, &canvas_only());
            assert_eq!(out.lines.0, Some(500.0), "{out:?}");
            assert_eq!(out.transform.x, 0.0, "起点 {start_x}");
        }
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

    #[test]
    fn edge_drag_stretches_one_direction_only() {
        let (t, g) = half();
        // 右边中点 (750, 250) 拖到 (875, 300)：沿法线方向是中心外 375（原来 250），宽变 1.5 倍、高不变
        let out = drag_scale_edge(&t, &g, Edge::Right, (875.0, 300.0));
        assert!((out.scale - 0.75).abs() < 1e-6, "{out:?}");
        assert!(
            (out.scale * out.stretch - 0.5).abs() < 1e-6,
            "高不变 {out:?}"
        );
        // 上边中点 (500, 125) 往下拖到 (520, 187.5)：高只剩一半、宽不变
        let out = drag_scale_edge(&t, &g, Edge::Top, (520.0, 187.5));
        assert!((out.scale - 0.5).abs() < 1e-6, "{out:?}");
        assert!((out.stretch - 0.5).abs() < 1e-6, "{out:?}");
        assert_eq!(edge_cursor(&g, Edge::Top), CursorStyle::ResizeUpDown);
        assert_eq!(edge_cursor(&g, Edge::Left), CursorStyle::ResizeLeftRight);
        assert_eq!(
            cursor_for(HandleHit::Edge(Edge::Bottom), &g, false),
            CursorStyle::ResizeUpDown
        );
        // 拖过中心：缩到最小，不翻转
        let flipped = drag_scale_edge(&t, &g, Edge::Bottom, (500.0, 100.0));
        assert_eq!(flipped.stretch, Transform::STRETCH_RANGE.0);
    }

    #[test]
    fn align_snaps_the_bounding_box_to_canvas_edges_and_centers() {
        let (t, g) = half();
        let canvas = (1000.0, 500.0);
        // 框是 (250, 125) 到 (750, 375)
        assert_eq!(align(&t, &g, canvas, Align::Left).x, -0.25);
        assert_eq!(align(&t, &g, canvas, Align::Right).x, 0.25);
        assert_eq!(align(&t, &g, canvas, Align::Top).y, -0.25);
        assert_eq!(align(&t, &g, canvas, Align::Bottom).y, 0.25);
        let left = align(&t, &g, canvas, Align::Left);
        assert_eq!(left.y, t.y, "左对齐不动纵向");
        // 偏到一边后再居中：回到 0
        let off = Transform {
            x: 0.137,
            y: -0.21,
            ..t
        };
        let og = layer_geometry(CANVAS, CANVAS, &off);
        assert_eq!(align(&off, &og, canvas, Align::CenterX).x, 0.0);
        assert_eq!(align(&off, &og, canvas, Align::CenterY).y, 0.0);
        // 转 90° 的图层按外接框贴边：竖着的框宽 250，左对齐后框的左边在 0
        let (rt, rg) = turned(90.0);
        let aligned = align(&rt, &rg, canvas, Align::Left);
        let after = layer_geometry(CANVAS, CANVAS, &aligned).bounds();
        assert!(after.x.abs() < 1e-3, "{after:?}");
    }
}
