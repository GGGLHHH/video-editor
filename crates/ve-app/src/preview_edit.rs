//! 预览上直接操作画面：点选图层、拖动移动、拖角缩放，选中的图层画出边框。
//!
//! 几何都按画布像素算（和合成器同一套 `layer_geometry`），只在进出屏幕时换算一次。

use crate::preview::fit_rect;
use crate::preview_handles::{Corner, HandleHit, drag_move, drag_scale, hit};
use crate::workspace::Workspace;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyElement, Context, IntoElement as _, MouseDownEvent, MouseMoveEvent, Styled as _, Window,
    div, px,
};
use ve_core::compose::resolve_layers;
use ve_core::edit::set_transform;
use ve_core::{ClipId, LayerGeometry, Rect, Transform, layer_geometry};

/// 吸附和拖角判定的范围（屏幕像素）。
const SNAP_PX: f32 = 8.0;
/// 角上小方块的边长（屏幕像素）。
const HANDLE_PX: f32 = 8.0;

/// 正在进行的拖动。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PreviewDrag {
    clip: ClipId,
    start: Transform,
    geom: LayerGeometry,
    mode: HandleHit,
    /// 按下的位置（画布像素）。
    origin: (f32, f32),
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

    fn to_screen(self, r: Rect) -> Rect {
        Rect {
            x: self.x + r.x * self.scale,
            y: self.y + r.y * self.scale,
            w: r.w * self.scale,
            h: r.h * self.scale,
        }
    }
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

    /// 选中的片段（单选）在播放头处可见时，它的画面属性和几何。
    fn selected_layer(&self, canvas: (u32, u32)) -> Option<(ClipId, Transform, LayerGeometry)> {
        let [selected] = self.doc.selection() else {
            return None;
        };
        self.layers_here(canvas)
            .into_iter()
            .find(|(clip, ..)| clip == selected)
    }

    pub(crate) fn preview_mouse_down(
        &mut self,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(m) = self.mapping() else { return };
        let p = m.to_canvas((f32::from(e.position.x), f32::from(e.position.y)));
        // 先后顺序：选中图层的角手柄 > 鼠标下最上面的图层 > 选中图层边缘附近（吸附范围内）。
        // 不能让「选中图层的身体」优先：铺满画面的底层选中后，会把上面的小画面全挡住、点不到。
        let selected = self.selected_layer(m.canvas);
        let tolerance = SNAP_PX / m.scale;
        let corner = selected.and_then(|layer| match hit(layer.2.shown, p, tolerance) {
            h @ HandleHit::Corner(_) => Some((layer, h)),
            HandleHit::Body | HandleHit::None => None,
        });
        let target = corner
            .or_else(|| {
                let top = self
                    .layers_here(m.canvas)
                    .into_iter()
                    .rev()
                    .find(|(.., g)| g.shown.contains(p.0, p.1))?;
                if selected.is_none_or(|s| s.0 != top.0) {
                    self.doc.select(top.0, false);
                    self.sync_inspector(window, cx);
                }
                Some((top, HandleHit::Body))
            })
            .or_else(|| {
                selected.and_then(|layer| {
                    (hit(layer.2.shown, p, tolerance) == HandleHit::Body)
                        .then_some((layer, HandleHit::Body))
                })
            });
        let Some(((clip, start, geom), mode)) = target else {
            return;
        };
        self.doc.begin_gesture();
        self.preview.drag = Some(PreviewDrag {
            clip,
            start,
            geom,
            mode,
            origin: p,
        });
        cx.notify();
    }

    pub(crate) fn preview_mouse_move(
        &mut self,
        e: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let (Some(drag), Some(m)) = (self.preview.drag, self.mapping()) else {
            return;
        };
        let p = m.to_canvas((f32::from(e.position.x), f32::from(e.position.y)));
        #[expect(clippy::cast_precision_loss, reason = "画布尺寸远小于 2^23")]
        let canvas = (m.canvas.0 as f32, m.canvas.1 as f32);
        let t = match drag.mode {
            HandleHit::Corner(corner) => drag_scale(&drag.start, &drag.geom, corner, p),
            HandleHit::Body | HandleHit::None => drag_move(
                &drag.start,
                canvas,
                (p.0 - drag.origin.0, p.1 - drag.origin.1),
                // 按住 Alt：临时不吸附，松开就恢复
                if e.modifiers.alt {
                    0.0
                } else {
                    SNAP_PX / m.scale
                },
                &drag.geom,
            ),
        };
        if self
            .doc
            .edit(|proj| set_transform(proj, drag.clip, t))
            .is_ok()
        {
            self.after_change(window, cx);
            let now = self.doc.selection().first().copied();
            if now == Some(drag.clip) {
                self.fill_transform(&t, window, cx);
            }
        }
    }

    pub(crate) fn preview_mouse_up(&mut self, cx: &mut Context<'_, Self>) {
        if self.preview.drag.take().is_some() {
            self.doc.end_gesture();
            cx.notify();
        }
    }

    /// 选中图层的边框和四个角（在播放头处可见时）。
    pub(crate) fn render_selection_box(&self, cx: &Context<'_, Self>) -> Vec<AnyElement> {
        let Some(m) = self.mapping() else {
            return Vec::new();
        };
        let Some((.., geom)) = self.selected_layer(m.canvas) else {
            return Vec::new();
        };
        let r = m.to_screen(geom.shown);
        let (ox, oy) = (
            f32::from(self.preview.bounds.origin.x),
            f32::from(self.preview.bounds.origin.y),
        );
        let color = crate::theme::palette::selection();
        let frame = div()
            .absolute()
            .left(px(r.x - ox))
            .top(px(r.y - oy))
            .w(px(r.w))
            .h(px(r.h))
            .border_1()
            .border_color(color)
            .into_any_element();
        let handles = Corner::ALL.into_iter().map(|c| {
            let (x, y) = c.point(r);
            div()
                .absolute()
                .left(px(x - ox - HANDLE_PX / 2.0))
                .top(px(y - oy - HANDLE_PX / 2.0))
                .size(px(HANDLE_PX))
                .bg(cx.theme().background)
                .border_1()
                .border_color(color)
                .into_any_element()
        });
        std::iter::once(frame).chain(handles).collect()
    }
}
