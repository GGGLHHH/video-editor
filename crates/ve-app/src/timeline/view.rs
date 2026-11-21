//! 时间轴视图：画布绘制（标尺、轨道、片段缩略图条 / 波形 / 字幕、播放头、吸附线）与鼠标交互。

use super::drop::{BinDrag, BinItem, DropPlan, DropTarget};
use super::geometry::SCROLLBAR_HEIGHT;
use super::{Drag, Hit, TimelineGeometry};
use crate::import::PEAKS_PER_SECOND;
use crate::theme::palette;
use crate::workspace::{MediaThumbs, Workspace};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{
    App, BorderStyle, Bounds, ContentMask, Context, Corners, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, Point, ScrollEvent, StatefulInteractiveElement as _, Styled as _, TextAlign, Window,
    canvas, div, fill, outline, point, px, size,
};
use std::collections::HashMap;
use std::sync::Arc;
use ve_core::{AssetId, ClipContent, ClipId, Project, Time, TrackKind};

/// 按下后鼠标挪开这么远（像素）才算拖起来，开始贴边自动滚。
const DRAG_START_PX: f32 = 4.0;

/// 时间轴视图状态。
#[derive(Debug, Default)]
pub struct TimelineUi {
    /// 几何参数。
    pub geometry: TimelineGeometry,
    /// 视图在窗口里的位置（上一帧布局结果）。
    pub bounds: Bounds<Pixels>,
    /// 进行中的拖拽。
    pub drag: Option<Drag>,
    /// 吸附提示线。
    pub snap_line: Option<Time>,
    /// 右键菜单弹出的位置（窗口坐标）。
    pub context_menu: Option<(f32, f32)>,
    /// 松手会在这条缝新建轨道（画白色横线）、片段放在这个起点。
    pub insert_line: Option<(usize, Time)>,
    /// 进行中的框选。
    pub marquee: Option<Marquee>,
    /// 按在多选里的某个片段上：（片段，按下时的修改计数）；松开时没拖动就改成只选它。
    pub pending_single: Option<(ClipId, u64)>,
    /// 鼠标悬停的片段。
    pub hover: Option<ClipId>,
    /// 移动拖动中，被拖片段的原位置（轨道下标、起点、终点）。
    pub ghosts: Vec<(usize, Time, Time)>,
    /// 鼠标样式（悬停 / 拖动时按位置变）。
    pub cursor: gpui_kit::CursorStyle,
    /// 鼠标最后的位置（时间轴本地坐标），松开后据此恢复悬停样式。
    pub pointer: (f32, f32),
    /// 按下的位置（时间轴本地坐标）：按下后鼠标还没真的挪开（几像素以内）时不贴边自动滚，
    /// 免得在边缘单击一下片段就被挪走；挪开过一次就清掉。
    pub pressed_at: Option<(f32, f32)>,
    /// 播放时跟着播放头翻页（用户自己滚走时不拉回）。
    pub follow: super::geometry::PlayFollow,
    /// 正在拖横向滚动条：（按下时的横坐标，按下时的横向滚动）。
    pub thumb_drag: Option<(f32, f32)>,
    /// 鼠标模式（选择 / 分割）。
    pub tool: super::tools::MouseTool,
    /// 鼠标模式菜单展开着。
    pub tool_menu: bool,
    /// 预览轴开着（S）。
    pub preview_axis: bool,
    /// 鼠标悬停处的时刻（在轨道区域里时）。
    pub hover_time: Option<Time>,
    /// 预览轴正在预览：真正的播放头位置（鼠标离开时回到这里）。
    pub axis_anchor: Option<Time>,
    /// 主轨「封面」菜单展开着。
    pub cover_menu: bool,
    /// 拖动中最后一次鼠标移动时按着的修饰键（贴边自动滚时照样按 Alt 决定吸附）。
    pub drag_modifiers: gpui_kit::Modifiers,
    /// 正从素材面板往时间轴拖：拖的东西和现在会落在哪（`None` = 落不下）。
    pub bin_drop: Option<(BinItem, Option<DropPlan>)>,
    /// 右键空白处弹出的菜单。
    pub blank_menu: Option<super::context_menu::BlankMenu>,
}

/// 框选。
#[derive(Debug, Clone)]
pub struct Marquee {
    /// 起点：（时间，内容坐标的纵向位置 = 屏幕 y + 纵向滚动），滚动后起点仍贴着内容。
    origin: (Time, f32),
    /// 当前鼠标位置（时间轴本地坐标）。
    current: (f32, f32),
    /// 框选前就保留的选择（按住 Ctrl / Shift 时是原有选择，否则为空）。
    base: Vec<ClipId>,
    /// 按下前的选择（Esc 时还原）。
    before: Vec<ClipId>,
}

/// 绘制用的快照（画布闭包不能借用视图）。
struct Snapshot {
    project: Arc<Project>,
    geo: TimelineGeometry,
    playhead: Time,
    selection: Vec<ClipId>,
    media: HashMap<AssetId, MediaThumbs>,
    snap: Option<Time>,
    insert: Option<usize>,
    hover: Option<ClipId>,
    /// 框选矩形的两个角（时间轴本地坐标）。
    marquee: Option<((f32, f32), (f32, f32))>,
    ghosts: Vec<(usize, Time, Time)>,
    /// 鼠标处的竖线（分割模式：切开的位置；预览轴：预览的位置）和颜色。
    hover_line: Option<(Time, Hsla)>,
    /// 找不到文件的素材（片段标红）。
    missing: std::collections::BTreeSet<AssetId>,
    /// 从素材面板拖进来会落在哪。
    drop: Option<DropTarget>,
}

impl Workspace {
    /// 按当前轨道数、时长和时间轴大小把滚动夹回范围（删轨、撤销、缩放、改面板大小以后）。
    pub(crate) fn clamp_timeline_scroll(&mut self) {
        let tracks = self.doc.project.timeline.tracks.len();
        let viewport = self.track_viewport();
        let geo = &mut self.timeline.geometry;
        geo.clamp_scroll_y(tracks, viewport);
        geo.clamp_scroll_x(self.doc.project.timeline.duration());
    }

    /// 轨道行可用的高度（底边留给横向滚动条）。
    fn track_viewport(&self) -> f32 {
        f32::from(self.timeline.bounds.size.height) - SCROLLBAR_HEIGHT
    }

    /// 在横向滚动条上按下：按在滑块上就开始拖；按在空槽里先把滑块中心跳到那里，再接着拖。
    fn scrollbar_mouse_down(
        &mut self,
        e: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.stop_propagation();
        let (x, _) = self.local(e.position);
        let duration = self.doc.project.timeline.duration();
        let width = f32::from(self.timeline.bounds.size.width);
        let geo = &mut self.timeline.geometry;
        if let Some(thumb) = geo.h_scrollbar(duration, width)
            && !(thumb.x..thumb.x + thumb.width).contains(&x)
        {
            let per = geo.scroll_per_thumb_px(duration, width);
            geo.scroll_by((x - (thumb.x + thumb.width / 2.0)) * per);
            geo.clamp_scroll_x(duration);
        }
        self.timeline.thumb_drag = Some((x, self.timeline.geometry.scroll_x));
        cx.notify();
    }

    /// 横向滚动条（能滚时才有）。
    fn scrollbar(&self, cx: &Context<'_, Self>) -> Option<impl IntoElement> {
        let geo = self.timeline.geometry;
        let thumb = geo.h_scrollbar(
            self.doc.project.timeline.duration(),
            f32::from(self.timeline.bounds.size.width),
        )?;
        let active = self.timeline.thumb_drag.is_some();
        Some(
            div()
                .id("timeline-hscroll")
                .test_support()
                .absolute()
                .bottom_0()
                .left(px(geo.header_width))
                .right_0()
                .h(px(SCROLLBAR_HEIGHT))
                .cursor(gpui_kit::CursorStyle::Arrow)
                .on_mouse_down(MouseButton::Left, cx.listener(Self::scrollbar_mouse_down))
                .child(
                    div()
                        .id("timeline-hscroll-thumb")
                        .test_support()
                        .absolute()
                        .top(px(2.0))
                        .bottom(px(2.0))
                        .left(px(thumb.x - geo.header_width))
                        .w(px(thumb.width))
                        .rounded(px(3.0))
                        .bg(if active {
                            palette::text()
                        } else {
                            palette::tick()
                        }),
                ),
        )
    }

    fn local(&self, p: Point<Pixels>) -> (f32, f32) {
        let o = self.timeline.bounds.origin;
        (f32::from(p.x - o.x), f32::from(p.y - o.y))
    }

    fn timeline_mouse_down(
        &mut self,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.focus.focus(window, cx);
        self.end_preview_axis();
        let (x, y) = self.local(e.position);
        let geo = self.timeline.geometry;
        if y >= geo.ruler_height
            && self.blade_click(
                geo.hit_test(x, y, &self.doc.project.timeline),
                x,
                window,
                cx,
            )
        {
            return;
        }
        let toggle = e.modifiers.control || e.modifiers.platform;
        let range = e.modifiers.shift;
        match geo.hit_test(x, y, &self.doc.project.timeline) {
            Hit::Clip { clip, .. } => {
                if toggle {
                    self.doc.select(clip, true);
                } else if range {
                    self.doc.select_range(clip);
                } else if self.doc.is_selected(clip) {
                    // 点在已选中的片段上：先保留整组（可能要一起拖），松开时没拖动再改成只选它
                    self.timeline.pending_single = Some((clip, self.doc.revision()));
                } else {
                    self.doc.select(clip, false);
                }
            }
            Hit::Track(_) | Hit::Empty if y >= geo.ruler_height => {
                // 空白处按下：开始框选；按住 Ctrl / Shift 时在原有选择上加
                let before = self.doc.selection().to_vec();
                let base = if toggle || range {
                    before.clone()
                } else {
                    Vec::new()
                };
                self.doc.set_selection(base.clone());
                self.timeline.marquee = Some(Marquee {
                    origin: (geo.x_to_time(x), y + geo.scroll_y),
                    current: (x, y),
                    base,
                    before,
                });
            }
            Hit::Track(_) | Hit::Empty | Hit::Ruler | Hit::Header(_) => {}
        }
        // Ctrl 点击减选掉的片段不跟着拖
        let drag = Drag::begin(&self.doc, &geo, x, y, self.playhead)
            .filter(|d| d.clip().is_none_or(|c| self.doc.is_selected(c)))
            .and_then(|d| self.tool_drag(d, geo.x_to_time(x)));
        if drag.is_some_and(|d| d.clip().is_some()) {
            self.doc.begin_gesture();
        }
        self.timeline.ghosts = drag.map_or_else(Vec::new, |d| self.ghosts_for(&d));
        self.timeline.drag = drag;
        self.timeline.pointer = (x, y);
        self.timeline.pressed_at = Some((x, y));
        self.timeline.cursor = super::interaction::cursor_for(
            geo.hit_test(x, y, &self.doc.project.timeline),
            drag.as_ref(),
            self.timeline.marquee.is_some(),
        );
        if let Some(mut d) = drag
            && matches!(d, Drag::Scrub)
        {
            let out = d.update(&mut self.doc, &geo, x, y, self.playhead, true);
            if let Some(t) = out.seek {
                self.seek(t, cx);
            }
        }
        self.after_change(window, cx);
    }

    /// 移动拖动开始时被拖片段的原位置（轨道下标、起点、终点），拖动中画成影子。
    fn ghosts_for(&self, drag: &Drag) -> Vec<(usize, Time, Time)> {
        if !matches!(drag, Drag::Move { .. }) {
            return Vec::new();
        }
        let mut clips = drag.group(&self.doc);
        if clips.is_empty() {
            clips.extend(drag.clip());
        }
        let tl = &self.doc.project.timeline;
        tl.tracks
            .iter()
            .enumerate()
            .flat_map(|(i, t)| {
                t.clips
                    .iter()
                    .filter(|c| clips.contains(&c.id))
                    .map(move |c| (i, c.start, c.end()))
            })
            .collect()
    }

    fn timeline_mouse_move(
        &mut self,
        e: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let (x, y) = self.local(e.position);
        let geo = self.timeline.geometry;
        self.timeline.pointer = (x, y);
        let pressed = e.pressed_button == Some(MouseButton::Left);
        if let Some((x0, scroll0)) = self.timeline.thumb_drag {
            if pressed {
                let duration = self.doc.project.timeline.duration();
                let width = f32::from(self.timeline.bounds.size.width);
                let geo = &mut self.timeline.geometry;
                geo.scroll_x = scroll0 + (x - x0) * geo.scroll_per_thumb_px(duration, width);
                geo.clamp_scroll_x(duration);
            } else {
                self.timeline.thumb_drag = None;
            }
            cx.notify();
            return;
        }
        if self.timeline.marquee.is_some() || self.timeline.drag.is_some() {
            if !pressed {
                // 在窗口外松开时收不到 mouse up：移动时发现左键已松开，就当作松开处理
                self.timeline_mouse_up(window, cx);
                return;
            }
        } else {
            // 没在拖：更新悬停提示和鼠标样式
            let hit = geo.hit_test(x, y, &self.doc.project.timeline);
            let hover = match hit {
                Hit::Clip { clip, .. } => Some(clip),
                _ => None,
            };
            let cursor = if self.timeline.tool == super::tools::MouseTool::Blade
                && matches!(hit, Hit::Clip { .. })
            {
                gpui_kit::CursorStyle::Crosshair
            } else {
                super::interaction::cursor_for(hit, None, false)
            };
            self.timeline_hover_at(x, if y >= geo.ruler_height { y } else { -1.0 }, cx);
            if hover != self.timeline.hover || cursor != self.timeline.cursor {
                self.timeline.hover = hover;
                self.timeline.cursor = cursor;
                cx.notify();
            }
            return;
        }
        self.timeline.drag_modifiers = e.modifiers;
        self.drag_to(x, y, window, cx);
    }

    /// 框选 / 拖动跟到时间轴本地坐标 `(x, y)`（鼠标移动、贴边自动滚以后都走这里）。
    fn drag_to(&mut self, x: f32, y: f32, window: &mut Window, cx: &mut Context<'_, Self>) {
        let geo = self.timeline.geometry;
        if let Some(m) = self.timeline.marquee.as_mut() {
            m.current = (x, y);
            let corner = (geo.time_to_x(m.origin.0), m.origin.1 - geo.scroll_y);
            let mut sel = m.base.clone();
            sel.extend(geo.clips_in_rect(&self.doc.project.timeline, corner, m.current));
            self.doc.set_selection(sel);
            self.after_change(window, cx);
            return;
        }
        let Some(mut drag) = self.timeline.drag else {
            return;
        };
        let revision = self.doc.revision();
        // 吸附开关关着时只吸到帧；按住 Alt 临时取反，松开就恢复
        let snapping = self.snapping_for(self.timeline.drag_modifiers.alt);
        let out = drag.update(&mut self.doc, &geo, x, y, self.playhead, snapping);
        self.timeline.drag = Some(drag);
        self.timeline.snap_line = out.snap;
        self.timeline.insert_line = out.insert;
        if let Some(t) = out.seek {
            self.seek(t, cx);
        }
        if self.doc.revision() != revision {
            self.after_change(window, cx);
        }
        cx.notify();
    }

    /// 每帧调一次：播放时时间轴跟着播放头翻页；拖动、修剪、框选时鼠标贴着左右边缘就自动滚，
    /// 滚完按同一个鼠标位置把拖动重新算一遍（鼠标不动也接着滚）。
    pub(crate) fn auto_scroll_timeline(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let width = f32::from(self.timeline.bounds.size.width);
        if width <= 0.0 || self.timeline.thumb_drag.is_some() {
            return;
        }
        let duration = self.doc.project.timeline.duration();
        // 从素材面板拖的东西已经放下或者不拖了：收掉预览
        if self.timeline.bin_drop.is_some() && !cx.has_active_drag() {
            self.clear_bin_drop();
        }
        let from_bin = self.timeline.bin_drop.map(|(item, _)| item);
        let dragging =
            self.timeline.marquee.is_some() || self.timeline.drag.is_some() || from_bin.is_some();
        if !dragging {
            if !self.engine.is_playing() {
                self.timeline.follow.stop();
                return;
            }
            let (t, forward) = (self.engine.position(), self.engine.rate() >= 0.0);
            let geo = &mut self.timeline.geometry;
            if self.timeline.follow.step(geo, t, width, forward) {
                geo.clamp_scroll_x(duration);
            }
            return;
        }
        let (x, y) = self.timeline.pointer;
        // 按下后还没真的拖起来（单击）：不滚
        if from_bin.is_none()
            && let Some((px, py)) = self.timeline.pressed_at
        {
            if (x - px).hypot(y - py) < DRAG_START_PX {
                return;
            }
            self.timeline.pressed_at = None;
        }
        let geo = &mut self.timeline.geometry;
        let before = geo.scroll_x;
        // 往右拖出结尾时片段跟着往后挪、时间线变长，能接着滚
        geo.scroll_by(geo.edge_scroll(x, width));
        geo.clamp_scroll_x(duration);
        if (geo.scroll_x - before).abs() > f32::EPSILON {
            if let Some(item) = from_bin {
                self.timeline.bin_drop = Some((item, self.plan_bin_drop(item)));
                self.timeline.snap_line = self.timeline.bin_drop.and_then(|(_, p)| p?.snap);
            } else {
                self.drag_to(x, y, window, cx);
            }
            window.request_animation_frame();
        }
    }

    fn timeline_mouse_up(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.timeline.thumb_drag = None;
        self.timeline.pressed_at = None;
        self.timeline.marquee = None;
        let insert = self.timeline.insert_line.take();
        if let Some(drag) = self.timeline.drag.take()
            && let Some(clip) = drag.clip()
        {
            // 松手在缝上：在那里新建一条同类轨道，把片段放进去（和拖动算同一步撤销）
            if let (Drag::Move { .. }, Some((slot, start))) = (drag, insert) {
                let _ = self
                    .doc
                    .edit(|p| ve_core::edit::move_clip_to_new_track(p, clip, slot, start));
            }
            self.doc.end_gesture();
        }
        // 点在多选里的一个片段上、没拖动就松开：只选它
        if let Some((clip, revision)) = self.timeline.pending_single.take()
            && revision == self.doc.revision()
        {
            self.doc.select(clip, false);
        }
        self.timeline.ghosts.clear();
        self.timeline.snap_line = None;
        self.restore_timeline_cursor();
        self.after_change(window, cx);
    }

    /// 拖动结束或取消后，按鼠标所在位置恢复悬停时的样式。
    pub(crate) fn restore_timeline_cursor(&mut self) {
        let (x, y) = self.timeline.pointer;
        let hit = self
            .timeline
            .geometry
            .hit_test(x, y, &self.doc.project.timeline);
        self.timeline.cursor = super::interaction::cursor_for(hit, None, false);
    }

    /// Esc 取消时间轴上进行中的操作：框选回到框之前的选择，拖动交给撤回手势。
    /// 返回是否有操作被取消；`true` 且是拖动时，调用方负责撤回手势。
    pub(crate) fn cancel_timeline_gesture(&mut self) -> (bool, bool) {
        self.timeline.pending_single = None;
        self.restore_timeline_cursor();
        self.timeline.ghosts.clear();
        self.timeline.snap_line = None;
        self.timeline.insert_line = None;
        if let Some(m) = self.timeline.marquee.take() {
            self.doc.set_selection(m.before);
            return (true, false);
        }
        let dragging = self
            .timeline
            .drag
            .take()
            .is_some_and(|d| d.clip().is_some());
        (dragging, dragging)
    }

    fn timeline_scroll(
        &mut self,
        e: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let d = e.delta.pixel_delta(window.line_height());
        let m = e.modifiers;
        let tracks = self.doc.project.timeline.tracks.len();
        let viewport = self.track_viewport();
        let overflow = self.timeline.geometry.content_height(tracks) > viewport;
        match super::interaction::wheel_action(f32::from(d.x), f32::from(d.y), m, overflow) {
            super::interaction::Wheel::Zoom(zoom_in) => {
                let (x, _) = self.local(e.position);
                self.timeline
                    .geometry
                    .zoom_around(x, if zoom_in { 1.15 } else { 1.0 / 1.15 });
            }
            super::interaction::Wheel::Scroll { dx, dy } => {
                self.timeline.geometry.scroll_by(dx);
                self.timeline
                    .geometry
                    .scroll_tracks_by(dy, tracks, viewport);
            }
        }
        self.clamp_timeline_scroll();
        cx.notify();
    }

    /// 从素材面板拖着经过时间轴：算出会落在哪，画出来。
    fn timeline_drag_move(
        &mut self,
        e: &gpui_kit::DragMoveEvent<BinDrag>,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let item = e.drag(cx).item;
        let inside = e.bounds.contains(&e.event.position);
        self.timeline.pointer = self.local(e.event.position);
        self.timeline.drag_modifiers = e.event.modifiers;
        let before = self.timeline.bin_drop;
        self.timeline.bin_drop = inside.then(|| (item, self.plan_bin_drop(item)));
        if self.timeline.bin_drop != before {
            self.timeline.snap_line = self.timeline.bin_drop.and_then(|(_, p)| p?.snap);
            cx.notify();
        }
    }

    /// 鼠标在 `self.timeline.pointer` 时 `item` 会落在哪。
    fn plan_bin_drop(&self, item: BinItem) -> Option<DropPlan> {
        super::drop::plan(
            &self.doc,
            &self.timeline.geometry,
            item,
            self.timeline.pointer,
            self.playhead,
            self.snapping_for(self.timeline.drag_modifiers.alt),
        )
    }

    /// 拖完松手（或者拖到一半不拖了）：把拖动预览收掉。
    pub(crate) const fn clear_bin_drop(&mut self) {
        if self.timeline.bin_drop.take().is_some() {
            self.timeline.snap_line = None;
        }
    }

    fn timeline_drop(&mut self, drag: &BinDrag, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.timeline.pointer = self.local(window.mouse_position());
        let plan = self.plan_bin_drop(drag.item);
        self.clear_bin_drop();
        let Some(plan) = plan else {
            cx.notify();
            return;
        };
        let transition = Time::from_secs_f64(self.transition_secs);
        let item = drag.item;
        if let Some(clip) = self.apply(window, cx, |p| {
            super::drop::apply(p, item, plan.target, transition)
        }) {
            self.doc.select(clip, false);
            self.after_change(window, cx);
        }
    }

    pub(crate) fn render_timeline(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let snapshot = Snapshot {
            project: Arc::clone(&self.project),
            geo: self.timeline.geometry,
            playhead: self.playhead,
            selection: self.doc.selection().to_vec(),
            media: self.media.clone(),
            snap: self.timeline.snap_line,
            insert: self.timeline.insert_line.map(|(slot, _)| slot),
            hover: self.timeline.hover,
            marquee: self.timeline.marquee.as_ref().map(|m| {
                let g = &self.timeline.geometry;
                (
                    (g.time_to_x(m.origin.0), m.origin.1 - g.scroll_y),
                    m.current,
                )
            }),
            ghosts: self.timeline.ghosts.clone(),
            hover_line: self.timeline.hover_time.and_then(|t| {
                if self.timeline.tool == super::tools::MouseTool::Blade {
                    Some((t, palette::selection()))
                } else if self.timeline.axis_anchor.is_some() {
                    Some((t, palette::preview_axis()))
                } else {
                    None
                }
            }),
            missing: self.missing.clone(),
            drop: self
                .timeline
                .bin_drop
                .and_then(|(_, plan)| plan.map(|p| p.target)),
        };
        let this = cx.entity();
        div()
            .id("timeline")
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.timeline_hover_end(cx);
                }
            }))
            .test_support()
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .overflow_hidden()
            .bg(palette::timeline_bg())
            .cursor(self.timeline.cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::timeline_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::timeline_right_down))
            .on_mouse_move(cx.listener(Self::timeline_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, w, cx| this.timeline_mouse_up(w, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, w, cx| this.timeline_mouse_up(w, cx)),
            )
            .on_scroll(cx.listener(Self::timeline_scroll))
            .on_drag_move(cx.listener(Self::timeline_drag_move))
            .on_drop(cx.listener(Self::timeline_drop))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let changed = this.update(cx, |w, _| {
                            let changed =
                                std::mem::replace(&mut w.timeline.bounds, bounds) != bounds;
                            // 面板拉高了也要重新夹一次
                            w.clamp_timeline_scroll();
                            changed
                        });
                        // 轨道头按钮等按旧尺寸摆的：尺寸变了再画一帧
                        if changed {
                            window.request_animation_frame();
                        }
                    },
                    move |bounds, (), window, cx| paint(&snapshot, bounds, window, cx),
                )
                .size_full(),
            )
            .child(
                // 轨道头这一列：滚轮在这里上下滚轨道（测试也从这里滚）；不拦点击
                div()
                    .id("track-headers")
                    .test_support()
                    .absolute()
                    .left_0()
                    .top(px(self.timeline.geometry.ruler_height))
                    .w(px(self.timeline.geometry.header_width))
                    .bottom_0(),
            )
            .children(self.track_buttons(cx))
            .children(self.scrollbar(cx))
    }
}

/// 当前裁剪区域的左右边界（嵌套的裁剪会自动求交，就是片段在屏幕上可见的部分）。
fn mask_span(window: &Window) -> (f32, f32) {
    let m = window.content_mask().bounds;
    (f32::from(m.origin.x), f32::from(m.origin.x + m.size.width))
}

/// 从 `x0` 起宽 `width`、每格 `cell` 的格子里，落在 `[left, right]` 内的格子下标。
/// 长片段放大后只画屏幕上那几百格，不遍历整段。
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "已夹到非负，格数受屏幕宽度限制"
)]
fn visible_cells(x0: f32, width: f32, cell: f32, left: f32, right: f32) -> std::ops::Range<usize> {
    if cell <= 0.0 || width <= 0.0 {
        return 0..0;
    }
    let total = (width / cell).ceil();
    let first = ((left - x0) / cell).floor().clamp(0.0, total);
    let last = ((right - x0) / cell).ceil().clamp(0.0, total);
    if first >= last {
        0..0
    } else {
        first as usize..last as usize
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(w.max(0.0)), px(h.max(0.0))))
}

fn paint_text(
    window: &mut Window,
    cx: &mut App,
    text: &str,
    x: f32,
    y: f32,
    font: f32,
    color: Hsla,
) {
    let text = text.replace('\n', " ");
    if text.is_empty() {
        return;
    }
    let mut style = window.text_style();
    style.color = color;
    let run = style.to_run(text.len());
    let line = window
        .text_system()
        .shape_line(text.into(), px(font), &[run], None);
    let _ignored = line.paint(
        point(px(x), px(y)),
        px(font * 1.3),
        TextAlign::Left,
        None,
        window,
        cx,
    );
}

/// 播放头颜色：平时白色，有东西吸到它上面时变成吸附青。
fn playhead_color(snapped: bool) -> Hsla {
    if snapped {
        palette::snap()
    } else {
        palette::playhead()
    }
}

fn ruler_label(t: Time, step: Time) -> String {
    let ms = t.as_millis().max(0);
    let (m, s) = (ms / 60_000, ms / 1000 % 60);
    // 剪映式：分:秒都补两位（00:01），细刻度再带十分之一秒
    if step < Time::from_secs(1) {
        format!("{m:02}:{s:02}.{}", ms / 100 % 10)
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 空工程：主轨那一行提示把素材拖进来（剪映「素材拖拽到这里，开始你的大作吧~」）。
fn paint_empty_hint(snap: &Snapshot, ox: f32, oy: f32, window: &mut Window, cx: &mut App) {
    let (geo, timeline) = (snap.geo, &snap.project.timeline);
    if timeline.tracks.iter().any(|t| !t.clips.is_empty()) {
        return;
    }
    let Some(main) = ve_core::magnet::main_track(timeline) else {
        return;
    };
    let Some(i) = timeline.tracks.iter().position(|t| t.id == main) else {
        return;
    };
    paint_text(
        window,
        cx,
        "素材拖拽到这里，开始你的大作吧~",
        ox + geo.header_width + 16.0,
        oy + geo.track_top(i) + geo.track_height / 2.0 - 8.0,
        12.0,
        palette::tick(),
    );
}

fn paint(snap: &Snapshot, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let geo = snap.geo;
    let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    window.paint_quad(fill(bounds, palette::timeline_bg()));

    // 轨道底色和轨道头
    let names = super::tracks::track_names(&snap.project.timeline);
    for (i, label) in names.iter().enumerate() {
        let y = oy + geo.track_top(i);
        window.paint_quad(fill(
            rect(ox, y, w, geo.track_height - 1.0),
            palette::track_bg(i % 2 == 0),
        ));
        window.paint_quad(fill(
            rect(ox, y, geo.header_width, geo.track_height - 1.0),
            palette::ruler_bg(),
        ));
        let ty = y + geo.track_height / 2.0 - 8.0;
        paint_text(window, cx, label, ox + 10.0, ty, 12.0, palette::text());
    }

    paint_empty_hint(snap, ox, oy, window, cx);

    // 片段（裁掉轨道头区域）
    let area = rect(ox + geo.header_width, oy, w - geo.header_width, h);
    window.with_content_mask(Some(ContentMask { bounds: area }), |window| {
        paint_ghosts(snap, ox, oy, window);
        for (i, track) in snap.project.timeline.tracks.iter().enumerate() {
            let y = oy + geo.track_top(i) + 2.0;
            for clip in &track.clips {
                let x0 = ox + geo.time_to_x(clip.start);
                let x1 = ox + geo.time_to_x(clip.end());
                if x1 < area.origin.x.into() || x0 > ox + w {
                    continue;
                }
                let r = rect(x0, y, x1 - x0, geo.track_height - 5.0);
                paint_clip(snap, clip, track.kind, r, ox, window, cx);
                // 整条轨不起作用（隐藏的画面 / 字幕轨、静音的音频轨）或锁着：片段压暗
                let silent = track.kind == TrackKind::Audio && track.muted;
                if track.hidden || silent {
                    window.paint_quad(fill(r, palette::disabled_veil()));
                } else if track.locked {
                    window.paint_quad(fill(r, palette::locked_veil()));
                }
            }
        }
        // 播放头和吸附线
        let line = |window: &mut Window, t: Time, width: f32, color: Hsla| {
            let x = ox + geo.time_to_x(t);
            window.paint_quad(fill(rect(x - width / 2.0, oy, width, h), color));
        };
        // 剪映：吸到播放头时播放头本身变青；吸到别处时在那里画一条青线
        let on_playhead = snap.snap == Some(snap.playhead);
        if let Some(t) = snap.snap.filter(|_| !on_playhead) {
            line(window, t, 1.0, palette::snap());
        }
        line(window, snap.playhead, 2.0, playhead_color(on_playhead));
        if let Some((t, color)) = snap.hover_line {
            line(window, t, 1.0, color);
        }
        paint_marquee(snap, ox, oy, window);
        paint_drop(snap, ox, oy, w, window);
        // 拖到缝上：一条贯穿的白色横线，标出松手后新轨道的位置（剪映）
        if let Some(slot) = snap.insert {
            let y = oy + geo.track_top(slot);
            window.paint_quad(fill(rect(ox, y - 1.0, w, 2.0), palette::selection()));
        }
    });

    // 标尺
    window.paint_quad(fill(rect(ox, oy, w, geo.ruler_height), palette::ruler_bg()));
    let step = geo.tick_step();
    let minor = Time(step.0 / 5).max(Time(1));
    let first = geo.x_to_time(geo.header_width).max(Time::ZERO);
    let start = Time(first.0 - first.0.rem_euclid(minor.0));
    let ticks = (geo.x_to_time(w) - start).0 / minor.0 + 1;
    for i in 0..=ticks.max(0) {
        let t = start + Time(minor.0 * i);
        let x = ox + geo.time_to_x(t);
        if x >= ox + geo.header_width {
            let major = t.0.rem_euclid(step.0) == 0;
            let tick_h = if major { 10.0 } else { 4.0 };
            window.paint_quad(fill(
                rect(x, oy + geo.ruler_height - tick_h, 1.0, tick_h),
                palette::tick(),
            ));
            if major {
                paint_text(
                    window,
                    cx,
                    &ruler_label(t, step),
                    x + 3.0,
                    oy + 3.0,
                    10.0,
                    palette::tick(),
                );
            }
        }
    }
    paint_range(snap, ox, oy, w, window);
    paint_markers(snap, ox, oy, w, window);
    paint_transitions(snap, ox, oy, window);
    let px_head = ox + geo.time_to_x(snap.playhead);
    if px_head >= ox + geo.header_width {
        window.paint_quad(fill(
            rect(px_head - 5.0, oy + geo.ruler_height - 10.0, 10.0, 10.0),
            playhead_color(snap.snap == Some(snap.playhead)),
        ));
    }
}

/// 转场：切点上一个青色小方块（压在前后两段之间），宽度按转场时长。
fn paint_transitions(snap: &Snapshot, ox: f32, oy: f32, window: &mut Window) {
    let geo = snap.geo;
    for (i, track) in snap.project.timeline.tracks.iter().enumerate() {
        let y = oy + geo.track_top(i) + geo.track_height / 2.0 - 7.0;
        // 滚到标尺下面去的轨道不画（这里画在标尺之后，否则会压在标尺上）
        if y < oy + geo.ruler_height {
            continue;
        }
        for pair in track.clips.windows(2) {
            let (prev, next) = (&pair[0], &pair[1]);
            let Some(tr) = next.transition_in.filter(|_| prev.end() == next.start) else {
                continue;
            };
            let half = Time(tr.duration.0 / 2);
            let x0 = (ox + geo.time_to_x(next.start - half)).max(ox + geo.header_width);
            let x1 = ox + geo.time_to_x(next.start + half);
            let r = rect(x0, y, (x1 - x0).max(10.0), 14.0);
            window.paint_quad(fill(r, palette::transition()).corner_radii(px(3.0)));
            window.paint_quad(
                outline(r, palette::accent(), BorderStyle::Solid).corner_radii(px(3.0)),
            );
        }
    }
}

/// 入出点区域：标尺上一条半透明的青色带，两头各一条竖线（剪映）。
fn paint_range(snap: &Snapshot, ox: f32, oy: f32, w: f32, window: &mut Window) {
    let geo = snap.geo;
    let Some((a, b)) = snap.project.timeline.range else {
        return;
    };
    let left = ox + geo.header_width;
    let x0 = (ox + geo.time_to_x(a)).max(left);
    let x1 = (ox + geo.time_to_x(b)).min(ox + w);
    if x1 <= x0 {
        return;
    }
    window.paint_quad(fill(
        rect(x0, oy, x1 - x0, geo.ruler_height),
        palette::accent().opacity(0.22),
    ));
    for (t, x) in [(a, x0), (b, x1)] {
        if ox + geo.time_to_x(t) >= left {
            window.paint_quad(fill(
                rect(x - 1.0, oy, 2.0, geo.ruler_height),
                palette::accent(),
            ));
        }
    }
}

/// 标记：标尺顶上的青色小旗（剪映）。
fn paint_markers(snap: &Snapshot, ox: f32, oy: f32, w: f32, window: &mut Window) {
    let geo = snap.geo;
    for &m in &snap.project.timeline.markers {
        let x = ox + geo.time_to_x(m);
        if x >= ox + geo.header_width && x <= ox + w {
            window.paint_quad(
                fill(rect(x - 4.0, oy + 1.0, 8.0, 7.0), palette::accent()).corner_radii(px(2.0)),
            );
            window.paint_quad(fill(
                rect(x - 0.5, oy + 1.0, 1.0, geo.ruler_height - 1.0),
                palette::accent(),
            ));
        }
    }
}

/// 选中片段的描边粗细（像素）。
const SELECTION_BORDER: f32 = 2.0;

/// 片段底部画出关键帧位置（小菱形）。
fn paint_keyframes(
    clip: &ve_core::Clip,
    ox: f32,
    geo: &TimelineGeometry,
    r: Bounds<Pixels>,
    window: &mut Window,
) {
    const HALF: f32 = 4.5;
    let y = f32::from(r.origin.y + r.size.height) - HALF - 3.0;
    // 片段两端的关键帧也整个画在片段里面，不被边缘切掉一半
    let (left, right) = (
        f32::from(r.origin.x) + HALF + 2.0,
        f32::from(r.origin.x + r.size.width) - HALF - 2.0,
    );
    for t in clip.animation.times() {
        let x = (ox + geo.time_to_x(clip.start + t)).clamp(left, right.max(left));
        let diamond = |half: f32| {
            let mut b = gpui_kit::PathBuilder::fill();
            b.move_to(point(px(x), px(y - half)));
            b.line_to(point(px(x + half), px(y)));
            b.line_to(point(px(x), px(y + half)));
            b.line_to(point(px(x - half), px(y)));
            b.close();
            b.build()
        };
        if let (Ok(back), Ok(front)) = (diamond(HALF + 1.5), diamond(HALF)) {
            window.paint_path(back, palette::keyframe_edge());
            window.paint_path(front, palette::keyframe());
        }
    }
}

/// 拖动中：被拖片段原来的位置留个影子。
fn paint_ghosts(snap: &Snapshot, ox: f32, oy: f32, window: &mut Window) {
    let geo = snap.geo;
    for &(i, start, end) in &snap.ghosts {
        let (x0, x1) = (ox + geo.time_to_x(start), ox + geo.time_to_x(end));
        let r = rect(
            x0,
            oy + geo.track_top(i) + 2.0,
            x1 - x0,
            geo.track_height - 5.0,
        );
        window.paint_quad(fill(r, palette::ghost()).corner_radii(px(3.0)));
        window.paint_quad(outline(r, palette::hover(), BorderStyle::Dashed).corner_radii(px(3.0)));
    }
}

/// 从素材面板拖进来会落在哪：新片段的位置（半透明框）、新轨道的缝（白线）、主轨插队的剪辑点（竖条）、
/// 要套滤镜 / 转场的片段（描边）。
fn paint_drop(snap: &Snapshot, ox: f32, oy: f32, w: f32, window: &mut Window) {
    let geo = snap.geo;
    let ghost = |window: &mut Window, top: f32, start: Time, duration: Time| {
        let (x0, x1) = (
            ox + geo.time_to_x(start),
            ox + geo.time_to_x(start + duration),
        );
        let r = rect(x0, oy + top + 2.0, x1 - x0, geo.track_height - 5.0);
        window.paint_quad(fill(r, palette::drop_preview()).corner_radii(px(3.0)));
        window
            .paint_quad(outline(r, palette::selection(), BorderStyle::Solid).corner_radii(px(3.0)));
    };
    match snap.drop {
        Some(DropTarget::Track {
            track,
            start,
            duration,
        }) => ghost(window, geo.track_top(track), start, duration),
        Some(DropTarget::NewTrack {
            slot,
            start,
            duration,
        }) => {
            let y = geo.track_top(slot);
            window.paint_quad(fill(rect(ox, oy + y - 1.0, w, 2.0), palette::selection()));
            ghost(window, y - geo.track_height / 2.0, start, duration);
        }
        Some(DropTarget::Main { track, at }) => {
            let x = ox + geo.time_to_x(at);
            let y = oy + geo.track_top(track);
            window.paint_quad(fill(
                rect(x - 2.0, y, 4.0, geo.track_height),
                palette::selection(),
            ));
        }
        Some(DropTarget::Clip(clip)) => {
            for (i, t) in snap.project.timeline.tracks.iter().enumerate() {
                if let Some(c) = t.clips.iter().find(|c| c.id == clip) {
                    let (x0, x1) = (ox + geo.time_to_x(c.start), ox + geo.time_to_x(c.end()));
                    let r = rect(
                        x0,
                        oy + geo.track_top(i) + 2.0,
                        x1 - x0,
                        geo.track_height - 5.0,
                    );
                    window.paint_quad(
                        outline(r, palette::selection(), BorderStyle::Solid).corner_radii(px(3.0)),
                    );
                }
            }
        }
        None => {}
    }
}

/// 框选矩形。
fn paint_marquee(snap: &Snapshot, ox: f32, oy: f32, window: &mut Window) {
    if let Some((a, b)) = snap.marquee {
        let r = rect(
            ox + a.0.min(b.0),
            oy + a.1.min(b.1),
            (a.0 - b.0).abs(),
            (a.1 - b.1).abs(),
        );
        window.paint_quad(fill(r, palette::marquee()));
        window.paint_quad(outline(r, palette::selection(), BorderStyle::Solid));
    }
}

/// 剪映式标题：字幕显示全文；媒体片段显示「名字  精确到帧的时长」（变过速的再标倍数）。
fn clip_title(snap: &Snapshot, clip: &ve_core::Clip, asset: Option<&ve_core::Asset>) -> String {
    clip.subtitle_text().map_or_else(
        || {
            let name = if clip.is_adjustment() {
                "调节".to_owned()
            } else if let Some(e) = clip.effect() {
                e.name().to_owned()
            } else if let Some((inner, _)) = clip.compound() {
                format!("▣ {}", inner.name)
            } else {
                asset.map(|a| a.name.clone()).unwrap_or_default()
            };
            let rate = snap.project.settings.frame_rate;
            let time = ve_core::format_timecode(clip.duration, rate);
            // 变过速的片段标出倍数（剪映在片段上显示「2.0x」）
            if (clip.speed - 1.0).abs() > 1e-4 {
                format!("{name}  {time}  {}x", clip.speed)
            } else {
                format!("{name}  {time}")
            }
        },
        str::to_owned,
    )
}

fn paint_clip(
    snap: &Snapshot,
    clip: &ve_core::Clip,
    kind: TrackKind,
    r: Bounds<Pixels>,
    ox: f32,
    window: &mut Window,
    cx: &mut App,
) {
    let geo = snap.geo;
    let (x0, y0) = (f32::from(r.origin.x), f32::from(r.origin.y));
    let selected = snap.selection.contains(&clip.id);
    let color = match kind {
        TrackKind::Video if clip.is_adjustment() => palette::adjustment_clip(),
        TrackKind::Video if clip.effect().is_some() => palette::effect_clip(),
        _ if clip.compound().is_some() => palette::compound_clip(),
        TrackKind::Video => palette::video_clip(),
        TrackKind::Audio => palette::audio_clip(),
        TrackKind::Subtitle => palette::subtitle_clip(),
    };
    window.paint_quad(fill(r, color).corner_radii(px(3.0)));
    window.with_content_mask(Some(ContentMask { bounds: r }), |window| {
        let asset = clip.asset().and_then(|a| snap.project.assets.get(&a));
        let media = clip.asset().and_then(|a| snap.media.get(&a));
        match (&clip.content, asset, media) {
            // 音频轨上只画波形（视频文件的声音放到音频轨时也一样）
            (ClipContent::Media { source_in, .. }, Some(asset), Some(m))
                if !m.thumbnails.is_empty() && kind != TrackKind::Audio =>
            {
                paint_thumbnails(&geo, clip, *source_in, asset.duration, m, r, ox, window);
            }
            (ClipContent::Media { source_in, .. }, Some(_), Some(m)) if !m.peaks.is_empty() => {
                paint_waveform(&geo, clip, *source_in, m, r, ox, window);
            }
            _ => {}
        }
        let label = clip_title(snap, clip, asset);
        // 选中的片段：白色描边（剪映式）再蒙一层很淡的白，压在花哨的缩略图上也看得出来
        if selected {
            window.paint_quad(fill(r, palette::selection_tint()));
        }
        // 媒体丢失（剪映）：压一层红，标题下面注明
        if clip.asset().is_some_and(|a| snap.missing.contains(&a)) {
            window.paint_quad(fill(r, palette::missing_veil()));
            paint_text(
                window,
                cx,
                "媒体丢失",
                x0 + 5.0,
                y0 + 16.0,
                11.0,
                palette::text(),
            );
        }
        // 停用的片段：先压一层深色（标题画在上面，仍看得清），标题下面注明（剪映的「停用片段」）
        if clip.disabled {
            window.paint_quad(fill(r, palette::disabled_veil()));
            paint_text(
                window,
                cx,
                "已停用",
                x0 + 5.0,
                y0 + 16.0,
                11.0,
                palette::text(),
            );
        }
        // 颜色标签（剪映）：标题那一条换成标签色
        if let Some(l) = clip.label {
            let w = f32::from(r.size.width);
            window.paint_quad(fill(
                rect(x0, y0, w, 15.0),
                crate::clip_label::label_color(l).opacity(0.85),
            ));
        }
        paint_text(
            window,
            cx,
            &label,
            x0 + 5.0,
            y0 + 1.0,
            11.0,
            palette::text(),
        );
        paint_keyframes(clip, ox, &geo, r, window);
    });
    if !selected && snap.hover == Some(clip.id) {
        window.paint_quad(outline(r, palette::hover(), BorderStyle::Solid).corner_radii(px(3.0)));
    }
    if selected {
        window.paint_quad(
            outline(r, palette::selection(), BorderStyle::Solid)
                .border_widths(px(SELECTION_BORDER))
                .corner_radii(px(3.0)),
        );
    }
}

/// 缩略图条：每格取对应素材时刻的那张。`ox` 是时间轴原点横坐标（窗口坐标）。
#[expect(clippy::too_many_arguments, reason = "绘制参数，拆成结构体反而难读")]
fn paint_thumbnails(
    geo: &TimelineGeometry,
    clip: &ve_core::Clip,
    source_in: Time,
    asset_duration: Time,
    m: &MediaThumbs,
    r: Bounds<Pixels>,
    ox: f32,
    window: &mut Window,
) {
    let (x0, y0) = (f32::from(r.origin.x), f32::from(r.origin.y));
    let (cw, ch) = (f32::from(r.size.width), f32::from(r.size.height));
    let Some(first) = m.thumbnails.first() else {
        return;
    };
    let thumb_h = ch - 16.0;
    let sz = first.size(0);
    #[expect(clippy::cast_precision_loss, reason = "缩略图尺寸很小")]
    let tw = (thumb_h * sz.width.0 as f32 / sz.height.0.max(1) as f32).max(8.0);
    let (left, right) = mask_span(window);
    for k in visible_cells(x0, cw, tw, left, right) {
        #[expect(clippy::cast_precision_loss, reason = "格数很少")]
        let x = x0 + tw * k as f32;
        let t = geo.x_to_time(x + tw / 2.0 - ox).max(clip.start);
        let source = source_in + (t - clip.start).scaled(f64::from(clip.speed));
        let ratio =
            (source.as_secs_f64() / asset_duration.as_secs_f64().max(1e-6)).clamp(0.0, 0.999);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "下标很小"
        )]
        let idx = ((ratio * m.thumbnails.len() as f64) as usize).min(m.thumbnails.len() - 1);
        let tile = rect(x, y0 + 14.0, tw, thumb_h);
        let _ignored = window.paint_image(
            tile,
            tile,
            Corners::default(),
            Arc::clone(&m.thumbnails[idx]),
            0,
            false,
        );
    }
}

/// 波形：每 2 像素一列。
fn paint_waveform(
    geo: &TimelineGeometry,
    clip: &ve_core::Clip,
    source_in: Time,
    m: &MediaThumbs,
    r: Bounds<Pixels>,
    ox: f32,
    window: &mut Window,
) {
    let (x0, y0) = (f32::from(r.origin.x), f32::from(r.origin.y));
    let (cw, ch) = (f32::from(r.size.width), f32::from(r.size.height));
    let mid = y0 + 14.0 + (ch - 14.0) / 2.0;
    let half = (ch - 18.0) / 2.0;
    let (left, right) = mask_span(window);
    for k in visible_cells(x0, cw, 2.0, left, right) {
        #[expect(clippy::cast_precision_loss, reason = "列数很少")]
        let x = x0 + 2.0 * k as f32;
        let source = source_in + (geo.x_to_time(x - ox) - clip.start).scaled(f64::from(clip.speed));
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "桶下标为正且不大"
        )]
        let bucket = (source.as_secs_f64().max(0.0) * f64::from(PEAKS_PER_SECOND)) as usize;
        if let Some(&(lo, hi)) = m.peaks.get(bucket) {
            let top = mid - hi.clamp(-1.0, 1.0) * half;
            let bottom = mid - lo.clamp(-1.0, 1.0) * half;
            window.paint_quad(fill(
                rect(x, top, 1.0, (bottom - top).max(1.0)),
                palette::waveform(),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_visible_cells_are_iterated() {
        // 1 小时的片段放大到 2000 px/s：720 万像素宽，只画屏幕上那 1000 像素
        let cells = visible_cells(-3_000_000.0, 7_200_000.0, 2.0, 0.0, 1000.0);
        assert!(cells.len() <= 502, "{cells:?}");
        assert_eq!(cells.start, 1_500_000);
        // 完全在屏幕外
        assert!(visible_cells(5000.0, 100.0, 2.0, 0.0, 1000.0).is_empty());
        // 部分可见：从片段开头算起
        assert_eq!(visible_cells(990.0, 100.0, 10.0, 0.0, 1000.0), 0..1);
        assert_eq!(visible_cells(0.0, 25.0, 10.0, 0.0, 1000.0), 0..3);
    }

    #[test]
    fn ruler_labels_pad_minutes_like_jianying() {
        assert_eq!(ruler_label(Time::from_secs(1), Time::from_secs(1)), "00:01");
        assert_eq!(
            ruler_label(Time::from_secs(75), Time::from_secs(5)),
            "01:15"
        );
        assert_eq!(
            ruler_label(Time::from_secs_f64(2.5), Time::from_secs_f64(0.5)),
            "00:02.5"
        );
    }

    #[test]
    fn playhead_turns_snap_color_when_snapped() {
        assert_eq!(playhead_color(true), palette::snap());
        assert_eq!(playhead_color(false), palette::playhead());
        assert_ne!(palette::snap(), palette::playhead());
    }
}
