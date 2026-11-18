//! 预览上直接操作画面：点选图层、拖动移动（吸附到画布和其他图层）、拖角缩放、拖手柄旋转，
//! 选中的图层按实际角度画出边框、角手柄和旋转手柄，吸上时画对齐参考线。
//!
//! 几何都按画布像素算（和合成器同一套 `layer_geometry`），只在进出屏幕时换算一次。

use crate::preview::fit_rect;
use crate::preview_handles::{
    Edge, Guides, HandleHit, cursor_for, drag_move, drag_rotate, drag_scale, drag_scale_edge, hit,
    rotate_handle,
};
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyElement, Bounds, Context, CursorStyle, Hsla, IntoElement as _, MouseDownEvent,
    MouseMoveEvent, PathBuilder, Pixels, Styled as _, Window, canvas, fill, point, px, size,
};
use ve_core::compose::resolve_layers;
use ve_core::edit::apply_transform_at;
use ve_core::{ClipId, LayerGeometry, Time, Transform, layer_geometry};

/// 吸附和手柄判定的范围（屏幕像素）。
const SNAP_PX: f32 = 8.0;
/// 角上小方块的边长（屏幕像素）。
const HANDLE_PX: f32 = 8.0;
/// 旋转手柄离画面上边的距离（屏幕像素）。
const ROTATE_PX: f32 = 24.0;
/// 角手柄外面再往外这么远算旋转区（屏幕像素）。
const ROTATE_ZONE_PX: f32 = 18.0;

/// 一个图层：片段、画面属性、几何。
type Layer = (ClipId, Transform, LayerGeometry);

/// 正在进行的拖动。
#[derive(Debug, Clone)]
pub(crate) struct PreviewDrag {
    clip: ClipId,
    start: Transform,
    geom: LayerGeometry,
    mode: HandleHit,
    /// 按下的位置（画布像素）。
    origin: (f32, f32),
    /// 移动时可吸附的线（按下时其他图层的位置）。
    guides: Guides,
    /// 当前吸上的竖线、横线（画布像素），画参考线用。
    lines: (Option<f32>, Option<f32>),
    /// 播放头相对片段起点的时间：有动画时在这里新建 / 更新关键帧。
    at: Time,
}

/// 画布（工程像素）和屏幕之间的换算：屏幕 = 原点 + 画布 × 比例。
#[derive(Debug, Clone, Copy)]
struct Mapping {
    x: f32,
    y: f32,
    scale: f32,
    canvas: (u32, u32),
}

impl Mapping {
    fn to_canvas(self, p: (f32, f32)) -> (f32, f32) {
        ((p.0 - self.x) / self.scale, (p.1 - self.y) / self.scale)
    }

    fn to_screen(self, p: (f32, f32)) -> (f32, f32) {
        (self.x + p.0 * self.scale, self.y + p.1 * self.scale)
    }
}

/// 选中框的绘制数据（屏幕坐标），给画布闭包用。
struct Overlay {
    corners: [(f32, f32); 4],
    /// 下边中点（连到下方的旋转手柄）。
    bottom_mid: (f32, f32),
    /// 四条边的中点（缩放手柄）。
    edges: [(f32, f32); 4],
    rotate: (f32, f32),
    /// 参考线：竖线的 x、横线的 y。
    lines: (Option<f32>, Option<f32>),
    /// 画面区域（x, y, 宽, 高），参考线只画在画面范围内。
    video: (f32, f32, f32, f32),
    color: Hsla,
    handle_bg: Hsla,
}

impl Workspace {
    fn mapping(&self) -> Option<Mapping> {
        let s = self.doc.project.settings;
        let video = fit_rect(self.preview.bounds, s.width, s.height);
        let w = f32::from(video.size.width);
        if s.width == 0 || w <= 0.0 {
            return None;
        }
        #[expect(clippy::cast_precision_loss, reason = "画面宽度远小于 2^23")]
        let scale = w / s.width as f32;
        Some(Mapping {
            x: f32::from(video.origin.x),
            y: f32::from(video.origin.y),
            scale,
            canvas: (s.width, s.height),
        })
    }

    /// 播放头处各图层（从下往上）的片段、画面属性和几何。
    fn layers_here(&self, canvas: (u32, u32)) -> Vec<(ClipId, Transform, LayerGeometry)> {
        let project = &self.doc.project;
        resolve_layers(project, self.playhead)
            .into_iter()
            .filter_map(|l| {
                let info = project.assets.get(&l.asset)?.video?;
                // 素材宽高缺失时（极少见）合成按解码出的帧尺寸算，这里拿不到，干脆不画框、不让拖，免得框和画面对不上
                if info.width == 0 || info.height == 0 {
                    return None;
                }
                let geom = layer_geometry(canvas, (info.width, info.height), &l.transform);
                Some((l.clip, l.transform, geom))
            })
            .collect()
    }

    /// 吸附开关（Alt 临时取反）下，这次操作是否吸附。
    pub(crate) const fn snapping_for(&self, alt: bool) -> bool {
        self.snapping != alt
    }

    /// 选中的片段（单选）在播放头处可见时，它的画面属性和几何。
    fn selected_layer(&self, canvas: (u32, u32)) -> Option<(ClipId, Transform, LayerGeometry)> {
        let [selected] = self.doc.selection() else {
            return None;
        };
        self.layers_here(canvas)
            .into_iter()
            .find(|(clip, ..)| clip == selected)
    }

    /// 画布上的点 `p` 按下去会作用在哪个图层的哪里（不改任何状态）。
    /// 先后顺序：选中图层的旋转手柄、角手柄 > 鼠标下最上面的图层。
    /// 不能让「选中图层的身体」优先：铺满画面的底层选中后，会把上面的小画面全挡住、点不到。
    fn pick(&self, m: Mapping, p: (f32, f32)) -> Option<(Layer, HandleHit)> {
        let selected = self.selected_layer(m.canvas);
        let (tolerance, offset) = (SNAP_PX / m.scale, ROTATE_PX / m.scale);
        let zone = ROTATE_ZONE_PX / m.scale;
        selected
            .and_then(|layer| match hit(&layer.2, p, tolerance, offset, zone) {
                h @ (HandleHit::Corner(_) | HandleHit::Edge(_) | HandleHit::Rotate) => {
                    Some((layer, h))
                }
                HandleHit::Body | HandleHit::None => None,
            })
            .or_else(|| {
                let layers = self.layers_here(m.canvas);
                let top = *layers.iter().rev().find(|(.., g)| g.contains(p.0, p.1))?;
                Some((top, HandleHit::Body))
            })
    }

    pub(crate) fn preview_mouse_down(
        &mut self,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(m) = self.mapping() else { return };
        let p = m.to_canvas((f32::from(e.position.x), f32::from(e.position.y)));
        let Some(((clip, start, geom), mode)) = self.pick(m, p) else {
            return;
        };
        Self::rotate_cursor(
            (mode == HandleHit::Rotate).then_some(geom.center),
            p,
            window,
        );
        if self.doc.selection() != [clip] {
            self.doc.select(clip, false);
            self.sync_inspector(window, cx);
        }
        let layers = self.layers_here(m.canvas);
        let at = self
            .doc
            .project
            .timeline
            .find_clip(clip)
            .map_or(Time::ZERO, |(_, c)| self.playhead - c.start);
        let others: Vec<LayerGeometry> = layers
            .iter()
            .filter(|(c, ..)| *c != clip)
            .map(|(.., g)| *g)
            .collect();
        #[expect(clippy::cast_precision_loss, reason = "画布尺寸远小于 2^23")]
        let guides = Guides::new((m.canvas.0 as f32, m.canvas.1 as f32), &others);
        self.doc.begin_gesture();
        self.preview.cursor = cursor_for(mode, &geom, true);
        self.preview.drag = Some(PreviewDrag {
            clip,
            start,
            geom,
            mode,
            origin: p,
            guides,
            lines: (None, None),
            at,
        });
        cx.notify();
    }

    pub(crate) fn preview_mouse_move(
        &mut self,
        e: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(m) = self.mapping() else { return };
        let snapping = self.snapping_for(e.modifiers.alt);
        let p = m.to_canvas((f32::from(e.position.x), f32::from(e.position.y)));
        let Some(drag) = self.preview.drag.as_mut() else {
            // 没在拖：鼠标样式提示按下去会做什么
            let picked = self.pick(m, p);
            let cursor = picked.map_or(CursorStyle::Arrow, |((.., g), h)| cursor_for(h, &g, false));
            let rotating =
                picked.and_then(|((.., g), h)| (h == HandleHit::Rotate).then_some(g.center));
            Self::rotate_cursor(rotating, p, window);
            if cursor != self.preview.cursor {
                self.preview.cursor = cursor;
                cx.notify();
            }
            return;
        };
        #[expect(clippy::cast_precision_loss, reason = "画布尺寸远小于 2^23")]
        let canvas = (m.canvas.0 as f32, m.canvas.1 as f32);
        let rotating = (drag.mode == HandleHit::Rotate).then_some(drag.geom.center);
        let t = match drag.mode {
            HandleHit::Corner(corner) => drag_scale(&drag.start, &drag.geom, corner, p),
            HandleHit::Edge(edge) => drag_scale_edge(&drag.start, &drag.geom, edge, p),
            // 按住 Shift：15° 一档
            HandleHit::Rotate => drag_rotate(
                &drag.start,
                &drag.geom,
                drag.origin,
                p,
                snapping,
                e.modifiers.shift,
            ),
            HandleHit::Body | HandleHit::None => {
                let moved = drag_move(
                    &drag.start,
                    canvas,
                    (p.0 - drag.origin.0, p.1 - drag.origin.1),
                    if snapping { SNAP_PX / m.scale } else { 0.0 },
                    &drag.geom,
                    &drag.guides,
                );
                drag.lines = moved.lines;
                moved.transform
            }
        };
        let (clip, at) = (drag.clip, drag.at);
        // 转的时候光标跟着鼠标绕中心转
        Self::rotate_cursor(rotating, p, window);
        if self
            .doc
            .edit(|proj| apply_transform_at(proj, clip, at, t))
            .is_ok()
        {
            self.after_change(window, cx);
            let now = self.doc.selection().first().copied();
            if now == Some(clip) {
                self.fill_transform(&t, window, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn preview_mouse_up(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(drag) = self.preview.drag.take() {
            self.doc.end_gesture();
            // 可能在预览外面松开（之后收不到预览的悬停事件）：先交还光标，下次移动再按位置定
            crate::platform_cursor::clear();
            // 拖完鼠标还在图层上：握拳的手松开
            if drag.mode == HandleHit::Body {
                self.preview.cursor = CursorStyle::OpenHand;
            }
            cx.notify();
        }
    }

    /// 旋转光标：`center` 有值时显示，朝向是鼠标 `p` 相对图层中心的方向；没值时交还给 GPUI。
    fn rotate_cursor(center: Option<(f32, f32)>, p: (f32, f32), window: &Window) {
        match center {
            Some(c) => {
                let angle = (p.1 - c.1).atan2(p.0 - c.0).to_degrees();
                crate::platform_cursor::show_rotate(angle, window.scale_factor());
            }
            None => crate::platform_cursor::clear(),
        }
    }

    /// 鼠标离开预览：不在拖动时交还光标（否则旋转光标会留在别的面板上）。
    pub(crate) fn preview_hover(&mut self, hovered: bool, cx: &mut Context<'_, Self>) {
        if !hovered && self.preview.drag.is_none() {
            crate::platform_cursor::clear();
            self.preview.cursor = CursorStyle::Arrow;
            cx.notify();
        }
    }

    /// 选中图层的边框、四个角、旋转手柄（在播放头处可见时），以及拖动中吸上的参考线。
    pub(crate) fn render_selection_box(&self, cx: &Context<'_, Self>) -> Vec<AnyElement> {
        let Some(m) = self.mapping() else {
            return Vec::new();
        };
        let Some((.., geom)) = self.selected_layer(m.canvas) else {
            return Vec::new();
        };
        let s = geom.shown;
        let lines = self.preview.drag.as_ref().map_or((None, None), |d| d.lines);
        #[expect(clippy::cast_precision_loss, reason = "画布尺寸远小于 2^23")]
        let (cw, ch) = (m.canvas.0 as f32, m.canvas.1 as f32);
        let overlay = Overlay {
            corners: geom.corners().map(|c| m.to_screen(c)),
            bottom_mid: m.to_screen(geom.rotate((s.x + s.w / 2.0, s.y + s.h))),
            edges: Edge::ALL.map(|e| m.to_screen(geom.rotate(e.point(s)))),
            rotate: m.to_screen(rotate_handle(&geom, ROTATE_PX / m.scale)),
            lines: (
                lines.0.map(|x| m.to_screen((x, 0.0)).0),
                lines.1.map(|y| m.to_screen((0.0, y)).1),
            ),
            video: (m.x, m.y, cw * m.scale, ch * m.scale),
            color: palette::selection(),
            handle_bg: cx.theme().background,
        };
        vec![
            canvas(
                |_, _, _| {},
                move |_, (), window, _| paint_overlay(&overlay, window),
            )
            .absolute()
            .size_full()
            .into_any_element(),
        ]
    }
}

fn square(c: (f32, f32), side: f32) -> Bounds<Pixels> {
    Bounds::new(
        point(px(c.0 - side / 2.0), px(c.1 - side / 2.0)),
        size(px(side), px(side)),
    )
}

/// 画选中框：按实际角度的边框、角和边中点的小方块、下边中点连到旋转手柄的线和圆点、对齐参考线。
fn paint_overlay(o: &Overlay, window: &mut Window) {
    let at = |p: (f32, f32)| point(px(p.0), px(p.1));
    let (vx, vy, vw, vh) = o.video;
    if let Some(x) = o.lines.0 {
        window.paint_quad(fill(
            Bounds::new(point(px(x - 0.5), px(vy)), size(px(1.0), px(vh))),
            palette::snap(),
        ));
    }
    if let Some(y) = o.lines.1 {
        window.paint_quad(fill(
            Bounds::new(point(px(vx), px(y - 0.5)), size(px(vw), px(1.0))),
            palette::snap(),
        ));
    }
    let mut frame = PathBuilder::stroke(px(1.5));
    frame.move_to(at(o.corners[0]));
    for c in &o.corners[1..] {
        frame.line_to(at(*c));
    }
    frame.close();
    let mut stem = PathBuilder::stroke(px(1.0));
    stem.move_to(at(o.bottom_mid));
    stem.line_to(at(o.rotate));
    for path in [frame, stem] {
        if let Ok(path) = path.build() {
            window.paint_path(path, o.color);
        }
    }
    for c in o.corners {
        window.paint_quad(fill(square(c, HANDLE_PX), o.color));
        window.paint_quad(fill(square(c, HANDLE_PX - 2.0), o.handle_bg));
    }
    // 边中点：小一号的方块
    for e in o.edges {
        window.paint_quad(fill(square(e, HANDLE_PX - 2.0), o.color));
        window.paint_quad(fill(square(e, HANDLE_PX - 4.0), o.handle_bg));
    }
    window.paint_quad(fill(square(o.rotate, HANDLE_PX + 2.0), o.color).corner_radii(px(HANDLE_PX)));
    window.paint_quad(
        fill(square(o.rotate, HANDLE_PX - 1.0), o.handle_bg).corner_radii(px(HANDLE_PX)),
    );
}
