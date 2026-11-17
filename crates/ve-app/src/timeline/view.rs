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
}

/// 绘制用的快照（画布闭包不能借用视图）。
struct Snapshot {
    project: Arc<Project>,
    geo: TimelineGeometry,
    playhead: Time,
    selection: Vec<ClipId>,
    media: HashMap<AssetId, MediaThumbs>,
    snap: Option<Time>,
}

impl Workspace {
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
        match geo.hit_test(x, y, &self.doc.project.timeline) {
            Hit::Clip { clip, .. } => self.doc.select(clip, e.modifiers.shift),
            Hit::Track(_) | Hit::Empty => self.doc.clear_selection(),
            Hit::Ruler | Hit::Header(_) => {}
        }
        let drag = Drag::begin(&self.doc, &geo, x, y, self.playhead);
        if drag.is_some_and(|d| d.clip().is_some()) {
            self.doc.begin_gesture();
        }
        self.timeline.drag = drag;
        if let Some(mut d) = drag
            && matches!(d, Drag::Scrub)
        {
            let out = d.update(&mut self.doc, &geo, x, y, self.playhead);
            if let Some(t) = out.seek {
                self.seek(t, cx);
            }
        }
        self.after_change(window, cx);
    }

    fn timeline_mouse_move(
        &mut self,
        e: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(mut drag) = self.timeline.drag else {
            return;
        };
        if e.pressed_button != Some(MouseButton::Left) {
            // 在窗口外松开时收不到 mouse up：移动时发现左键已松开，就当作松开处理
            self.timeline_mouse_up(window, cx);
            return;
        }
        let (x, y) = self.local(e.position);
        let geo = self.timeline.geometry;
        let revision = self.doc.revision();
        let out = drag.update(&mut self.doc, &geo, x, y, self.playhead);
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
        if let Some(drag) = self.timeline.drag.take()
            && drag.clip().is_some()
        {
            self.doc.end_gesture();
        }
        self.timeline.snap_line = None;
        self.after_change(window, cx);
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
        };
        let this = cx.entity();
        div()
            .id("timeline")
            .test_support()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(palette::timeline_bg())
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
                    move |bounds, _, cx| this.update(cx, |w, _| w.timeline.bounds = bounds),
                    move |bounds, (), window, cx| paint(&snapshot, bounds, window, cx),
                )
                .size_full(),
            )
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
    for (i, track) in snap.project.timeline.tracks.iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "轨道数很少")]
        let y = oy + geo.ruler_height + geo.track_height * i as f32;
        window.paint_quad(fill(
            rect(ox, y, w, geo.track_height - 1.0),
            palette::track_bg(i % 2 == 0),
        ));
        window.paint_quad(fill(
            rect(ox, y, geo.header_width, geo.track_height - 1.0),
            palette::ruler_bg(),
        ));
        let name = match track.kind {
            TrackKind::Video => "视频",
            TrackKind::Audio => "音频",
            TrackKind::Subtitle => "字幕",
        };
        paint_text(
            window,
            cx,
            name,
            ox + 12.0,
            y + geo.track_height / 2.0 - 8.0,
            12.0,
            palette::text(),
        );
    }

    // 片段（裁掉轨道头区域）
    let area = rect(ox + geo.header_width, oy, w - geo.header_width, h);
    window.with_content_mask(Some(ContentMask { bounds: area }), |window| {
        for (i, track) in snap.project.timeline.tracks.iter().enumerate() {
            #[expect(clippy::cast_precision_loss, reason = "轨道数很少")]
            let y = oy + geo.ruler_height + geo.track_height * i as f32 + 2.0;
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
        paint_text(
            window,
            cx,
            &label,
            x0 + 5.0,
            y0 + 1.0,
            11.0,
            palette::text(),
        );
    });
    if snap.selection.contains(&clip.id) {
        window
            .paint_quad(outline(r, palette::selection(), BorderStyle::Solid).corner_radii(px(3.0)));
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
