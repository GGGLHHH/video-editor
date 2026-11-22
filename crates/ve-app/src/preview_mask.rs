//! 蒙版在播放器上直接拖（剪映：属性面板切到「蒙版」时，播放器上显示蒙版的轮廓和手柄）：
//! 拖里面移动，拖右边 / 上边的方块改宽 / 高，拖下面的圆点旋转，拖上面的箭头改羽化。
//!
//! 几何按画布像素算：蒙版的位置大小是图层显示部分的比例（[`ve_core::Mask`]），
//! 和合成器（`ve-render` 的 `look_params`）同一套换算。

use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::{
    AnyElement, Bounds, Context, CursorStyle, Hsla, IntoElement as _, PathBuilder, Styled as _,
    Window, canvas, fill, point, px, size,
};
use ve_core::edit::apply_look_at;
use ve_core::{ClipId, LayerGeometry, Mask, MaskShape, Time};

/// 手柄判定范围（屏幕像素）。
const HIT_PX: f32 = 8.0;
/// 旋转圆点离蒙版下边的距离（屏幕像素）。
const ROTATE_PX: f32 = 28.0;
/// 羽化箭头离蒙版上边的最小距离（屏幕像素），羽化越大离得越远。
const FEATHER_PX: f32 = 20.0;
/// 线性 / 镜面蒙版的线画多长（画布像素，够穿过整个画面）。
const LINE_HALF: f32 = 20_000.0;

/// 蒙版在画布上的位置：中心、半宽半高（画布像素）、朝向（图层的角度 + 蒙版自己的角度，弧度）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MaskFrame {
    /// 中心（画布像素）。
    pub(crate) center: (f32, f32),
    /// 半宽。
    pub(crate) hw: f32,
    /// 半高。
    pub(crate) hh: f32,
    /// 朝向（弧度，顺时针）。
    pub(crate) angle: f32,
    /// 羽化宽度（画布像素）。
    pub(crate) feather: f32,
    /// 形状。
    pub(crate) shape: MaskShape,
}

impl MaskFrame {
    /// 图层几何 + 蒙版 → 画布上的蒙版。
    #[must_use]
    pub(crate) fn new(g: &LayerGeometry, m: &Mask) -> Self {
        let s = g.shown;
        let local = (s.x + s.w * (0.5 + m.x), s.y + s.h * (0.5 + m.y));
        Self {
            center: g.rotate(local),
            hw: m.width * s.w / 2.0,
            hh: m.height * s.h / 2.0,
            angle: g.angle + m.rotation.to_radians(),
            feather: m.feather * s.w.min(s.h) / 2.0,
            shape: m.shape,
        }
    }

    /// 画布上的点 → 蒙版自己的坐标（中心为原点、没转之前）。
    #[must_use]
    pub(crate) fn in_mask(&self, p: (f32, f32)) -> (f32, f32) {
        let (dx, dy) = (p.0 - self.center.0, p.1 - self.center.1);
        let (sin, cos) = self.angle.sin_cos();
        (dx * cos + dy * sin, -dx * sin + dy * cos)
    }

    /// 蒙版自己的坐标 → 画布。
    #[must_use]
    pub(crate) fn on_canvas(&self, q: (f32, f32)) -> (f32, f32) {
        let (sin, cos) = self.angle.sin_cos();
        (
            self.center.0 + q.0 * cos - q.1 * sin,
            self.center.1 + q.0 * sin + q.1 * cos,
        )
    }

    /// 有宽度手柄（线性、镜面只有一条线 / 一条带子，不用宽）。
    const fn has_width(&self) -> bool {
        !matches!(self.shape, MaskShape::Linear | MaskShape::Mirror)
    }

    /// 有高度手柄（线性没有）。
    const fn has_height(&self) -> bool {
        !matches!(self.shape, MaskShape::Linear)
    }

    /// 上边（线性就是那条线）离中心的距离。
    const fn top(&self) -> f32 {
        if self.has_height() { self.hh } else { 0.0 }
    }

    /// 各手柄在画布上的位置（`unit` = 一个屏幕像素是多少画布像素）。
    #[must_use]
    pub(crate) fn handles(&self, unit: f32) -> Handles {
        let bottom = if self.has_height() { self.hh } else { 0.0 };
        Handles {
            width: self.has_width().then(|| self.on_canvas((self.hw, 0.0))),
            height: self.has_height().then(|| self.on_canvas((0.0, -self.hh))),
            rotate: self.on_canvas((0.0, bottom + ROTATE_PX * unit)),
            feather: self.on_canvas((0.0, -self.top() - FEATHER_PX * unit - self.feather)),
        }
    }

    /// 蒙版的轮廓（画布坐标的折线，每条单独画）。
    #[must_use]
    pub(crate) fn outline(&self) -> Vec<Vec<(f32, f32)>> {
        let line = |y: f32| {
            vec![
                self.on_canvas((-LINE_HALF, y)),
                self.on_canvas((LINE_HALF, y)),
            ]
        };
        match self.shape {
            MaskShape::Linear => vec![line(0.0)],
            MaskShape::Mirror => vec![line(-self.hh), line(self.hh)],
            MaskShape::Circle => {
                let n = 48;
                #[expect(clippy::cast_precision_loss, reason = "分段数很小")]
                let ring = (0..=n)
                    .map(|i| {
                        let a = i as f32 / n as f32 * std::f32::consts::TAU;
                        self.on_canvas((self.hw * a.cos(), self.hh * a.sin()))
                    })
                    .collect();
                vec![ring]
            }
            // 矩形、爱心、星形：画外接框
            MaskShape::Rectangle | MaskShape::Heart | MaskShape::Star => {
                let (w, h) = (self.hw, self.hh);
                vec![
                    [(-w, -h), (w, -h), (w, h), (-w, h), (-w, -h)]
                        .iter()
                        .map(|q| self.on_canvas(*q))
                        .collect(),
                ]
            }
        }
    }
}

/// 手柄位置（画布像素）；这种形状没有的手柄为 `None`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Handles {
    /// 右边中点：改宽。
    pub(crate) width: Option<(f32, f32)>,
    /// 上边中点：改高。
    pub(crate) height: Option<(f32, f32)>,
    /// 下面的圆点：旋转。
    pub(crate) rotate: (f32, f32),
    /// 上面的箭头：羽化。
    pub(crate) feather: (f32, f32),
}

/// 按在蒙版的哪里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaskHit {
    /// 移动。
    Move,
    /// 改宽。
    Width,
    /// 改高。
    Height,
    /// 旋转。
    Rotate,
    /// 改羽化。
    Feather,
}

/// 画布上的点 `p` 按下去作用在蒙版的哪里：先手柄，再蒙版里面（线性 / 镜面：画面里哪里都算移动）。
#[must_use]
#[expect(clippy::many_single_char_names, reason = "几何量：蒙版、图层、点")]
pub(crate) fn mask_hit(
    f: &MaskFrame,
    g: &LayerGeometry,
    p: (f32, f32),
    unit: f32,
) -> Option<MaskHit> {
    let h = f.handles(unit);
    let near = |q: (f32, f32)| (q.0 - p.0).hypot(q.1 - p.1) <= HIT_PX * unit;
    if near(h.rotate) {
        return Some(MaskHit::Rotate);
    }
    if near(h.feather) {
        return Some(MaskHit::Feather);
    }
    if h.width.is_some_and(near) {
        return Some(MaskHit::Width);
    }
    if h.height.is_some_and(near) {
        return Some(MaskHit::Height);
    }
    let q = f.in_mask(p);
    let inside = match f.shape {
        MaskShape::Linear | MaskShape::Mirror => g.contains(p.0, p.1),
        _ => q.0.abs() <= f.hw && q.1.abs() <= f.hh,
    };
    inside.then_some(MaskHit::Move)
}

/// 正在拖的蒙版。
#[derive(Debug, Clone, Copy)]
pub(crate) struct MaskDrag {
    clip: ClipId,
    /// 按下时的蒙版。
    start: Mask,
    /// 按下时的蒙版位置。
    frame: MaskFrame,
    /// 图层几何。
    geom: LayerGeometry,
    hit: MaskHit,
    /// 按下的位置（画布像素）。
    origin: (f32, f32),
    /// 播放头相对片段起点：有关键帧时在这里打。
    at: Time,
    /// 一个屏幕像素是多少画布像素。
    unit: f32,
}

/// 拖到 `p`（画布像素）之后的蒙版。按住 Shift 转的时候 15° 一档。
#[must_use]
#[expect(clippy::many_single_char_names, reason = "几何量：蒙版、图层、点")]
pub(crate) fn drag_mask(
    start: &Mask,
    f: &MaskFrame,
    g: &LayerGeometry,
    hit: MaskHit,
    (origin, p): ((f32, f32), (f32, f32)),
    unit: f32,
    shift: bool,
) -> Mask {
    let s = g.shown;
    let mut m = *start;
    let q = f.in_mask(p);
    match hit {
        MaskHit::Move => {
            let (a, b) = (g.to_local(origin), g.to_local(p));
            m.x = start.x + (b.0 - a.0) / s.w;
            m.y = start.y + (b.1 - a.1) / s.h;
        }
        MaskHit::Width => m.width = (q.0.abs() * 2.0 / s.w).clamp(0.01, 4.0),
        MaskHit::Height => m.height = (q.1.abs() * 2.0 / s.h).clamp(0.01, 4.0),
        MaskHit::Rotate => {
            // 圆点在蒙版下方（+y）：鼠标相对中心的方向减去「正下方」就是蒙版朝向
            let (dx, dy) = (p.0 - f.center.0, p.1 - f.center.1);
            let total = dy.atan2(dx).to_degrees() - 90.0;
            let mut deg = total - g.angle.to_degrees();
            if shift {
                deg = (deg / 15.0).round() * 15.0;
            }
            m.rotation = (deg + 180.0).rem_euclid(360.0) - 180.0;
        }
        MaskHit::Feather => {
            let beyond = -q.1 - f.top() - FEATHER_PX * unit;
            m.feather = (beyond.max(0.0) / (s.w.min(s.h) / 2.0)).clamp(0.0, 1.0);
        }
    }
    m
}

/// 画蒙版轮廓和手柄用的屏幕坐标。
struct Overlay {
    lines: Vec<Vec<(f32, f32)>>,
    width: Option<(f32, f32)>,
    height: Option<(f32, f32)>,
    rotate: (f32, f32),
    feather: (f32, f32),
    /// 画面区域（x, y, 宽, 高）：线性 / 镜面的长线只画在这里面。
    clip: (f32, f32, f32, f32),
    color: Hsla,
    handle_bg: Hsla,
}

impl Workspace {
    /// 属性面板正在「画面 › 蒙版」、选中的图层在播放头处可见且有蒙版时：
    /// （片段、这一刻的蒙版、图层几何、播放头相对片段起点）。
    fn mask_editing(&self, canvas: (u32, u32)) -> Option<(ClipId, Mask, LayerGeometry, Time)> {
        if self.inspector.tab != crate::inspector::InspectorTab::Picture
            || self.inspector.look.seg != crate::inspector_look::PictureSeg::Mask
        {
            return None;
        }
        let (clip, _, geom) = self.selected_layer(canvas)?;
        let (_, c) = self.doc.project.timeline.find_clip(clip)?;
        let at = self.key_offset(c);
        let look = crate::inspector_look::view_mask(c.look_at(at), self.inspector.look.mask_index);
        Some((clip, look.mask?, geom, at))
    }

    /// 蒙版模式下按下：按在蒙版上就开始拖蒙版。在蒙版模式里返回 `true`（按在蒙版外面也不选图层、
    /// 不拖图层——这时图层的选中框和手柄都没画，按下去动了图层用户看不出来）。
    pub(crate) fn mask_mouse_down(&mut self, p: (f32, f32), canvas: (u32, u32), unit: f32) -> bool {
        let Some((clip, start, geom, at)) = self.mask_editing(canvas) else {
            return false;
        };
        let frame = MaskFrame::new(&geom, &start);
        let Some(hit) = mask_hit(&frame, &geom, p, unit) else {
            return true;
        };
        self.doc.begin_gesture();
        self.preview.mask_drag = Some(MaskDrag {
            clip,
            start,
            frame,
            geom,
            hit,
            origin: p,
            at,
            unit,
        });
        self.preview.cursor = mask_cursor(hit);
        true
    }

    /// 拖蒙版中：改蒙版（有关键帧时在播放头处打）。不在拖蒙版时返回 `false`。
    pub(crate) fn mask_mouse_move(
        &mut self,
        p: (f32, f32),
        (pressed, shift): (bool, bool),
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(d) = self.preview.mask_drag else {
            return false;
        };
        // 松开没收到（比如在窗口外松开）：鼠标键已经不按着了，结束这次拖动
        if !pressed {
            self.mask_mouse_up();
            cx.notify();
            return true;
        }
        let m = drag_mask(
            &d.start,
            &d.frame,
            &d.geom,
            d.hit,
            (d.origin, p),
            d.unit,
            shift,
        );
        let index = self.inspector.look.mask_index;
        let edited = self.doc.edit(|proj| {
            let (_, c) = proj
                .timeline
                .find_clip(d.clip)
                .ok_or(ve_core::EditError::ClipNotFound(d.clip))?;
            let real = c.look_at(d.at);
            let mut view = crate::inspector_look::view_mask(real, index);
            view.mask = Some(m);
            let look = crate::inspector_look::unview_mask(view, real, index);
            apply_look_at(proj, d.clip, d.at, look)
        });
        if edited.is_ok() {
            self.after_change(window, cx);
        }
        cx.notify();
        true
    }

    /// 蒙版模式下没在拖时的鼠标样式；不在蒙版模式返回 `None`。
    pub(crate) fn mask_hover_cursor(
        &self,
        p: (f32, f32),
        canvas: (u32, u32),
        unit: f32,
    ) -> Option<CursorStyle> {
        let (_, m, geom, _) = self.mask_editing(canvas)?;
        let f = MaskFrame::new(&geom, &m);
        Some(mask_hit(&f, &geom, p, unit).map_or(CursorStyle::Arrow, mask_cursor))
    }

    /// 松开：结束拖蒙版（一步撤销）。没在拖蒙版时返回 `false`。
    pub(crate) fn mask_mouse_up(&mut self) -> bool {
        if self.preview.mask_drag.take().is_none() {
            return false;
        }
        self.doc.end_gesture();
        true
    }

    /// 蒙版模式下画蒙版的轮廓和手柄（代替选中框）。不在蒙版模式返回 `None`。
    pub(crate) fn render_mask_overlay(
        &self,
        to_screen: impl Fn((f32, f32)) -> (f32, f32),
        canvas_px: (u32, u32),
        unit: f32,
        video: (f32, f32, f32, f32),
        handle_bg: Hsla,
    ) -> Option<AnyElement> {
        let (_, m, geom, _) = self.mask_editing(canvas_px)?;
        let f = MaskFrame::new(&geom, &m);
        let h = f.handles(unit);
        let overlay = Overlay {
            lines: f
                .outline()
                .into_iter()
                .map(|l| l.into_iter().map(&to_screen).collect())
                .collect(),
            width: h.width.map(&to_screen),
            height: h.height.map(&to_screen),
            rotate: to_screen(h.rotate),
            feather: to_screen(h.feather),
            clip: video,
            color: palette::selection(),
            handle_bg,
        };
        Some(
            canvas(
                |_, _, _| {},
                move |_, (), window, _| paint_mask(&overlay, window),
            )
            .absolute()
            .size_full()
            .into_any_element(),
        )
    }
}

const fn mask_cursor(hit: MaskHit) -> CursorStyle {
    match hit {
        MaskHit::Move => CursorStyle::OpenHand,
        MaskHit::Width => CursorStyle::ResizeLeftRight,
        MaskHit::Height | MaskHit::Feather => CursorStyle::ResizeUpDown,
        MaskHit::Rotate => CursorStyle::PointingHand,
    }
}

fn square(c: (f32, f32), side: f32) -> Bounds<gpui_kit::Pixels> {
    Bounds::new(
        point(px(c.0 - side / 2.0), px(c.1 - side / 2.0)),
        size(px(side), px(side)),
    )
}

/// 画蒙版：轮廓线（限制在画面里）、改宽高的方块、旋转圆点、羽化的小三角。
fn paint_mask(o: &Overlay, window: &mut Window) {
    let at = |p: (f32, f32)| point(px(p.0), px(p.1));
    let (vx, vy, vw, vh) = o.clip;
    let bounds = Bounds::new(point(px(vx), px(vy)), size(px(vw), px(vh)));
    window.with_content_mask(Some(gpui_kit::ContentMask { bounds }), |window| {
        for l in &o.lines {
            let mut path = PathBuilder::stroke(px(1.5));
            if let Some(first) = l.first() {
                path.move_to(at(*first));
            }
            for p in &l[1..] {
                path.line_to(at(*p));
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, o.color);
            }
        }
    });
    for c in [o.width, o.height].into_iter().flatten() {
        window.paint_quad(fill(square(c, 9.0), o.color));
        window.paint_quad(fill(square(c, 5.0), o.handle_bg));
    }
    window.paint_quad(fill(square(o.rotate, 11.0), o.color).corner_radii(px(8.0)));
    window.paint_quad(fill(square(o.rotate, 6.0), o.handle_bg).corner_radii(px(8.0)));
    // 羽化：朝上的小三角
    let (x, y) = o.feather;
    let mut tri = PathBuilder::fill();
    tri.move_to(at((x, y - 6.0)));
    tri.line_to(at((x + 6.0, y + 4.0)));
    tri.line_to(at((x - 6.0, y + 4.0)));
    tri.close();
    if let Ok(path) = tri.build() {
        window.paint_path(path, o.color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::{Transform, layer_geometry};

    fn geom() -> LayerGeometry {
        layer_geometry((1920, 1080), (1920, 1080), &Transform::default())
    }

    #[test]
    fn frame_matches_the_compositor_layout() {
        let m = Mask {
            x: 0.25,
            ..Mask::new(MaskShape::Circle)
        };
        let f = MaskFrame::new(&geom(), &m);
        assert!((f.center.0 - 1440.0).abs() < 1e-3 && (f.center.1 - 540.0).abs() < 1e-3);
        assert!((f.hw - 480.0).abs() < 1e-3 && (f.hh - 270.0).abs() < 1e-3);
        let back = f.in_mask(f.on_canvas((10.0, -20.0)));
        assert!((back.0 - 10.0).abs() < 1e-3 && (back.1 + 20.0).abs() < 1e-3);
    }

    #[test]
    fn handles_are_hit_before_the_body_and_drags_change_the_right_field() {
        let g = geom();
        let m = Mask::new(MaskShape::Rectangle);
        let f = MaskFrame::new(&g, &m);
        let h = f.handles(1.0);
        assert_eq!(
            mask_hit(&f, &g, h.width.unwrap(), 1.0),
            Some(MaskHit::Width)
        );
        assert_eq!(
            mask_hit(&f, &g, h.height.unwrap(), 1.0),
            Some(MaskHit::Height)
        );
        assert_eq!(mask_hit(&f, &g, h.rotate, 1.0), Some(MaskHit::Rotate));
        assert_eq!(mask_hit(&f, &g, h.feather, 1.0), Some(MaskHit::Feather));
        assert_eq!(mask_hit(&f, &g, f.center, 1.0), Some(MaskHit::Move));
        assert_eq!(mask_hit(&f, &g, (5.0, 5.0), 1.0), None, "蒙版外面");
        // 往右拖 192 像素：中心挪画面宽的 0.1
        let moved = drag_mask(
            &m,
            &f,
            &g,
            MaskHit::Move,
            (f.center, (f.center.0 + 192.0, f.center.1)),
            1.0,
            false,
        );
        assert!((moved.x - 0.1).abs() < 1e-4 && moved.y.abs() < 1e-4);
        // 宽手柄拖到离中心 960：宽 = 整个画面
        let wide = drag_mask(
            &m,
            &f,
            &g,
            MaskHit::Width,
            (h.width.unwrap(), (960.0 + 960.0, 540.0)),
            1.0,
            false,
        );
        assert!((wide.width - 1.0).abs() < 1e-4);
        // 旋转圆点拖到右边：转 -90°（圆点原来在正下方）
        let turned = drag_mask(
            &m,
            &f,
            &g,
            MaskHit::Rotate,
            (h.rotate, (1500.0, 540.0)),
            1.0,
            false,
        );
        assert!((turned.rotation + 90.0).abs() < 1e-3, "{}", turned.rotation);
        // 羽化箭头往上拖 270 像素（画面短边一半的一半）：羽化 0.5
        let soft = drag_mask(
            &m,
            &f,
            &g,
            MaskHit::Feather,
            (h.feather, (h.feather.0, h.feather.1 - 270.0)),
            1.0,
            false,
        );
        assert!((soft.feather - 0.5).abs() < 1e-3, "{}", soft.feather);
    }

    #[test]
    fn a_linear_mask_moves_from_anywhere_on_the_layer_and_has_no_size_handles() {
        let g = geom();
        let m = Mask::new(MaskShape::Linear);
        let f = MaskFrame::new(&g, &m);
        let h = f.handles(1.0);
        assert_eq!((h.width, h.height), (None, None));
        assert_eq!(mask_hit(&f, &g, (100.0, 900.0), 1.0), Some(MaskHit::Move));
    }
}
