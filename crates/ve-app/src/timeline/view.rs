//! 时间轴视图：画布绘制（标尺、轨道、片段缩略图条 / 波形 / 字幕、播放头、吸附线）与鼠标交互。

use super::{Drag, Hit, TimelineGeometry};
use crate::import::PEAKS_PER_SECOND;
use crate::media_bin::AssetDrag;
use crate::theme::palette;
use crate::workspace::{MediaThumbs, Workspace};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{
    App, BorderStyle, Bounds, ContentMask, Context, Corners, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, Point, ScrollEvent, Styled as _, TextAlign, Window, canvas, div, fill, outline, point,
    px, size,
};
use std::collections::HashMap;
use std::sync::Arc;
use ve_core::{AssetId, ClipContent, ClipId, Project, Time, TrackKind};

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
    hover: Option<ClipId>,
    /// 框选矩形的两个角（时间轴本地坐标）。
    marquee: Option<((f32, f32), (f32, f32))>,
    ghosts: Vec<(usize, Time, Time)>,
}

impl Workspace {
    /// 按当前轨道数和时间轴高度把纵向滚动夹回范围（删轨、撤销、改面板大小以后）。
    pub(crate) fn clamp_timeline_scroll(&mut self) {
        let tracks = self.doc.project.timeline.tracks.len();
        let viewport = f32::from(self.timeline.bounds.size.height);
        self.timeline.geometry.clamp_scroll_y(tracks, viewport);
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
        let (x, y) = self.local(e.position);
        let geo = self.timeline.geometry;
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
            .filter(|d| d.clip().is_none_or(|c| self.doc.is_selected(c)));
        if drag.is_some_and(|d| d.clip().is_some()) {
            self.doc.begin_gesture();
        }
        self.timeline.ghosts = drag.map_or_else(Vec::new, |d| self.ghosts_for(&d));
        self.timeline.drag = drag;
        self.timeline.pointer = (x, y);
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
            let cursor = super::interaction::cursor_for(hit, None, false);
            if hover != self.timeline.hover || cursor != self.timeline.cursor {
                self.timeline.hover = hover;
                self.timeline.cursor = cursor;
                cx.notify();
            }
            return;
        }
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
        let snapping = self.snapping_for(e.modifiers.alt);
        let out = drag.update(&mut self.doc, &geo, x, y, self.playhead, snapping);
        self.timeline.drag = Some(drag);
        self.timeline.snap_line = out.snap;
        if let Some(t) = out.seek {
            self.seek(t, cx);
        }
        if self.doc.revision() != revision {
            self.after_change(window, cx);
        }
        cx.notify();
    }

    fn timeline_mouse_up(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.timeline.marquee = None;
        if let Some(drag) = self.timeline.drag.take()
            && drag.clip().is_some()
        {
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
        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
        if e.modifiers.control {
            let (x, _) = self.local(e.position);
            self.timeline
                .geometry
                .zoom_around(x, if dy > 0.0 { 1.15 } else { 1.0 / 1.15 });
        } else if e.modifiers.shift
            || self.local(e.position).0 < self.timeline.geometry.header_width
        {
            // 轨道头上滚轮 / Shift+滚轮：上下滚轨道
            let tracks = self.doc.project.timeline.tracks.len();
            let viewport = f32::from(self.timeline.bounds.size.height);
            self.timeline
                .geometry
                .scroll_tracks_by(-(dy + dx), tracks, viewport);
        } else {
            self.timeline.geometry.scroll_by(-(dx + dy));
        }
        cx.notify();
    }

    fn timeline_drop(&mut self, drag: &AssetDrag, window: &mut Window, cx: &mut Context<'_, Self>) {
        let (x, y) = self.local(window.mouse_position());
        let geo = self.timeline.geometry;
        let at = self
            .doc
            .project
            .settings
            .frame_rate
            .snap(geo.x_to_time(x).max(Time::ZERO));
        let track = geo
            .track_index_at(y, self.doc.project.timeline.tracks.len())
            .and_then(|i| self.doc.project.timeline.tracks.get(i))
            .map(|t| t.id);
        let asset = drag.asset;
        let placed = match track {
            Some(track) => self.apply(window, cx, |p| {
                ve_core::edit::insert_asset_at(p, asset, track, at)
            }),
            None => self.apply(window, cx, |p| ve_core::edit::append_asset(p, asset)),
        };
        if let Some(clip) = placed {
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
            hover: self.timeline.hover,
            marquee: self.timeline.marquee.as_ref().map(|m| {
                let g = &self.timeline.geometry;
                (
                    (g.time_to_x(m.origin.0), m.origin.1 - g.scroll_y),
                    m.current,
                )
            }),
            ghosts: self.timeline.ghosts.clone(),
        };
        let this = cx.entity();
        div()
            .id("timeline")
            .test_support()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(palette::timeline_bg())
            .cursor(self.timeline.cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::timeline_mouse_down))
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
            .on_drop(cx.listener(Self::timeline_drop))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        this.update(cx, |w, _| {
                            w.timeline.bounds = bounds;
                            // 面板拉高了也要重新夹一次
                            w.clamp_timeline_scroll();
                        });
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

fn ruler_label(t: Time, step: Time) -> String {
    let ms = t.as_millis().max(0);
    let (m, s) = (ms / 60_000, ms / 1000 % 60);
    if step < Time::from_secs(1) {
        format!("{m}:{s:02}.{}", ms / 100 % 10)
    } else {
        format!("{m}:{s:02}")
    }
}

fn paint(snap: &Snapshot, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let geo = snap.geo;
    let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    window.paint_quad(fill(bounds, palette::timeline_bg()));

    // 轨道底色和轨道头
    let names = super::tracks::track_names(&snap.project.timeline);
    for (i, (track, label)) in snap.project.timeline.tracks.iter().zip(&names).enumerate() {
        let y = oy + geo.track_top(i);
        window.paint_quad(fill(
            rect(ox, y, w, geo.track_height - 1.0),
            palette::track_bg(i % 2 == 0),
        ));
        window.paint_quad(fill(
            rect(ox, y, geo.header_width, geo.track_height - 1.0),
            palette::ruler_bg(),
        ));
        let kind = match track.kind {
            TrackKind::Video => "视频",
            TrackKind::Audio => "音频",
            TrackKind::Subtitle => "字幕",
        };
        paint_text(
            window,
            cx,
            &format!("{label} {kind}"),
            ox + 12.0,
            y + geo.track_height / 2.0 - 8.0,
            12.0,
            palette::text(),
        );
    }

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
            }
        }
        // 播放头和吸附线
        let line = |window: &mut Window, t: Time, width: f32, color: Hsla| {
            let x = ox + geo.time_to_x(t);
            window.paint_quad(fill(rect(x - width / 2.0, oy, width, h), color));
        };
        if let Some(t) = snap.snap {
            line(window, t, 1.0, palette::snap());
        }
        line(window, snap.playhead, 2.0, palette::playhead());
        paint_marquee(snap, ox, oy, window);
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
    let px_head = ox + geo.time_to_x(snap.playhead);
    if px_head >= ox + geo.header_width {
        window.paint_quad(fill(
            rect(px_head - 5.0, oy + geo.ruler_height - 10.0, 10.0, 10.0),
            palette::playhead(),
        ));
    }
}

/// 选中片段的描边粗细（像素）。
const SELECTION_BORDER: f32 = 2.0;
/// 选中片段标题栏的高度（像素）。
const LABEL_BAR: f32 = 14.0;

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
        TrackKind::Video => palette::video_clip(),
        TrackKind::Audio => palette::audio_clip(),
        TrackKind::Subtitle => palette::subtitle_clip(),
    };
    window.paint_quad(fill(r, color).corner_radii(px(3.0)));
    window.with_content_mask(Some(ContentMask { bounds: r }), |window| {
        let asset = clip.asset().and_then(|a| snap.project.assets.get(&a));
        let media = clip.asset().and_then(|a| snap.media.get(&a));
        match (&clip.content, asset, media) {
            (ClipContent::Media { source_in, .. }, Some(asset), Some(m))
                if !m.thumbnails.is_empty() =>
            {
                paint_thumbnails(&geo, clip, *source_in, asset.duration, m, r, ox, window);
            }
            (ClipContent::Media { source_in, .. }, Some(_), Some(m)) if !m.peaks.is_empty() => {
                paint_waveform(&geo, clip, *source_in, m, r, ox, window);
            }
            _ => {}
        }
        let label = clip.subtitle_text().map_or_else(
            || asset.map(|a| a.name.clone()).unwrap_or_default(),
            str::to_owned,
        );
        // 选中的片段：整体蒙一层亮色、标题栏换成亮底深字，压在花哨的缩略图上也一眼能看出来
        let label_color = if selected {
            window.paint_quad(fill(r, palette::selection_tint()));
            window.paint_quad(fill(
                rect(x0, y0, f32::from(r.size.width), LABEL_BAR),
                palette::selection(),
            ));
            palette::selection_text()
        } else {
            palette::text()
        };
        paint_text(window, cx, &label, x0 + 5.0, y0 + 1.0, 11.0, label_color);
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
        let source = source_in + (t - clip.start);
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
        let source = source_in + (geo.x_to_time(x - ox) - clip.start);
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
}
