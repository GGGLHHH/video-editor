//! 时间轴拖拽：拖标尺定位、拖片段移动（可换轨、吸附）、拖边缘修剪。纯逻辑。
//!
//! 位置都吸附到帧；靠近播放头或其他片段边缘（8 像素内）时吸过去（按住 Alt 时只吸到帧）。
//! 拖的是多选里的一个片段时，整组在各自轨道上一起平移。

use super::{Hit, TimelineGeometry, Zone};
use crate::document::Document;
use ve_core::edit::{insert_slots, move_clip, move_clips, trim_end, trim_start};
use ve_core::snap::{snap, snap_targets};
use ve_core::{ClipId, FrameRate, Time};

/// 吸附距离（像素）。
pub const SNAP_PX: f32 = 8.0;

/// 进行中的拖拽。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drag {
    /// 拖标尺：定位播放头。
    Scrub,
    /// 移动片段。
    Move {
        /// 片段。
        clip: ClipId,
        /// 抓取点相对片段起点的偏移。
        grab: Time,
        /// 片段长度。
        duration: Time,
    },
    /// 修剪起点。
    TrimStart {
        /// 片段。
        clip: ClipId,
    },
    /// 修剪终点。
    TrimEnd {
        /// 片段。
        clip: ClipId,
    },
    /// 滑移 / 滑动 / 滚动（[`ve_core::trim_tools`]）；滚动时 `clip` 是剪辑点左边那段。
    Tool {
        /// 哪种。
        kind: TrimKind,
        /// 片段。
        clip: ClipId,
        /// 按下时鼠标对着的时刻。
        origin: Time,
        /// 已经挪了多少。
        applied: Time,
    },
}

/// 三种剪辑点修剪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimKind {
    /// 滑移。
    Slip,
    /// 滑动。
    Slide,
    /// 滚动。
    Roll,
}

/// 一次拖拽更新的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DragOutcome {
    /// 要定位到的时刻（拖标尺时）。
    pub seek: Option<Time>,
    /// 吸附线位置（用于画提示竖线）。
    pub snap: Option<Time>,
    /// 松手会在这条缝新建轨道（轨道列表下标，画白色横线提示）、片段放在这个起点。
    pub insert: Option<(usize, Time)>,
}

impl Drag {
    /// 按按下位置开始拖拽；空白处返回 `None`。
    #[must_use]
    pub fn begin(
        doc: &Document,
        g: &TimelineGeometry,
        x: f32,
        y: f32,
        _playhead: Time,
    ) -> Option<Self> {
        match g.hit_test(x, y, &doc.project.timeline) {
            Hit::Ruler => Some(Self::Scrub),
            Hit::Clip { clip, zone } => {
                let (_, c) = doc.project.timeline.find_clip(clip)?;
                Some(match zone {
                    Zone::Body => Self::Move {
                        clip,
                        grab: g.x_to_time(x) - c.start,
                        duration: c.duration,
                    },
                    Zone::LeftEdge => Self::TrimStart { clip },
                    Zone::RightEdge => Self::TrimEnd { clip },
                })
            }
            Hit::Header(_) | Hit::Track(_) | Hit::Empty => None,
        }
    }

    /// 拖拽涉及的片段。
    #[must_use]
    pub const fn clip(&self) -> Option<ClipId> {
        match self {
            Self::Scrub => None,
            Self::Move { clip, .. }
            | Self::TrimStart { clip }
            | Self::TrimEnd { clip }
            | Self::Tool { clip, .. } => Some(*clip),
        }
    }

    /// 拖的片段在多选里时，整组一起平移；否则只动它自己（返回空）。
    #[must_use]
    pub fn group(&self, doc: &Document) -> Vec<ClipId> {
        match *self {
            Self::Move { clip, .. } if doc.is_selected(clip) && doc.selection().len() > 1 => {
                doc.selection().to_vec()
            }
            _ => Vec::new(),
        }
    }

    /// 鼠标移动到 `(x, y)`：更新工程（编辑失败则保持上一个合法状态）。
    /// `snapping` 为假（按住 Alt）时不吸附播放头和片段边缘，只吸到帧。
    pub fn update(
        &mut self,
        doc: &mut Document,
        geo: &TimelineGeometry,
        x: f32,
        y: f32,
        playhead: Time,
        snapping: bool,
    ) -> DragOutcome {
        let pointer = geo.x_to_time(x).max(Time::ZERO);
        let group = self.group(doc);
        let exclude = if group.is_empty() {
            self.clip().into_iter().collect()
        } else {
            group.clone()
        };
        let snapper = Snapper::new(doc, geo, playhead, &exclude, snapping);
        // 修剪工具：按帧算总共要挪多少，减去已经挪的，再挪剩下的（碰到边界时实际挪得少）
        if let Self::Tool {
            kind,
            clip,
            origin,
            applied,
        } = self
        {
            let want = snapper.rate.snap(pointer) - snapper.rate.snap(*origin) - *applied;
            let (kind, clip) = (*kind, *clip);
            let got = doc
                .edit(|p| match kind {
                    TrimKind::Slip => ve_core::trim_tools::slip(p, clip, want),
                    TrimKind::Slide => ve_core::trim_tools::slide(p, clip, want),
                    TrimKind::Roll => ve_core::trim_tools::roll(p, clip, want),
                })
                .unwrap_or(Time::ZERO);
            *applied += got;
            return DragOutcome::default();
        }
        match *self {
            Self::Scrub => DragOutcome {
                seek: Some(snapper.rate.snap(pointer)),
                ..DragOutcome::default()
            },
            Self::Move {
                clip,
                grab,
                duration,
            } => {
                let (start, line) = snapper.place_range((pointer - grab).max(Time::ZERO), duration);
                let mut insert = None;
                if group.is_empty() {
                    // 在缝上：片段先不动，松手时在那里新建轨道、放到 start
                    insert = insert_slot_for(doc, geo, clip, y).map(|slot| (slot, start));
                    if insert.is_none() {
                        move_to_hovered_track(doc, geo, clip, y, start);
                    }
                } else if let Some((_, c)) = doc.project.timeline.find_clip(clip) {
                    let delta = start - c.start;
                    if delta != Time::ZERO {
                        let _ = doc.edit(|p| move_clips(p, &group, delta));
                    }
                }
                DragOutcome {
                    seek: None,
                    snap: line,
                    insert,
                }
            }
            Self::TrimStart { clip } => {
                let (to, line) = snapper.place_point(pointer);
                let _ = doc.edit(|p| trim_start(p, clip, to));
                DragOutcome {
                    snap: line,
                    ..DragOutcome::default()
                }
            }
            Self::TrimEnd { clip } => {
                let (to, line) = snapper.place_point(pointer);
                let _ = doc.edit(|p| trim_end(p, clip, to));
                DragOutcome {
                    snap: line,
                    ..DragOutcome::default()
                }
            }
            // 上面已经处理过
            Self::Tool { .. } => DragOutcome::default(),
        }
    }
}

/// 鼠标样式：提示按下去会做什么。拖动中按拖的类型；框选中是十字；
/// 悬停在片段中间是张开的手（可拖动），在片段边缘是左右箭头（可修剪）。
#[must_use]
pub const fn cursor_for(hit: Hit, drag: Option<&Drag>, marquee: bool) -> gpui_kit::CursorStyle {
    use gpui_kit::CursorStyle;
    if marquee {
        return CursorStyle::Crosshair;
    }
    match (drag, hit) {
        (Some(Drag::Move { .. }), _) => CursorStyle::ClosedHand,
        (Some(Drag::TrimStart { .. } | Drag::TrimEnd { .. } | Drag::Tool { .. }), _)
        | (
            None,
            Hit::Clip {
                zone: Zone::LeftEdge | Zone::RightEdge,
                ..
            },
        ) => CursorStyle::ResizeLeftRight,
        (
            None,
            Hit::Clip {
                zone: Zone::Body, ..
            },
        ) => CursorStyle::OpenHand,
        _ => CursorStyle::Arrow,
    }
}

/// 吸附：先吸目标（播放头、片段边缘），没有就吸到帧。
struct Snapper {
    targets: Vec<Time>,
    tolerance: Time,
    rate: FrameRate,
}

impl Snapper {
    fn new(
        doc: &Document,
        geo: &TimelineGeometry,
        playhead: Time,
        exclude: &[ClipId],
        snapping: bool,
    ) -> Self {
        Self {
            targets: if snapping {
                snap_targets(&doc.project.timeline, playhead, exclude)
            } else {
                Vec::new()
            },
            tolerance: Time::from_secs_f64(f64::from(SNAP_PX / geo.px_per_second)),
            rate: doc.project.settings.frame_rate,
        }
    }

    /// 单点：返回（位置，吸附线）。
    fn place_point(&self, at: Time) -> (Time, Option<Time>) {
        snap(at, &self.targets, self.tolerance)
            .map_or_else(|| (self.rate.snap(at), None), |hit| (hit, Some(hit)))
    }

    /// 区间：起点或终点谁离目标近吸谁，返回（起点，吸附线）。
    fn place_range(&self, start: Time, len: Time) -> (Time, Option<Time>) {
        let end = start + len;
        let by_start =
            snap(start, &self.targets, self.tolerance).map(|hit| (hit, (hit - start).0.abs()));
        let by_end =
            snap(end, &self.targets, self.tolerance).map(|hit| (hit - len, (hit - end).0.abs()));
        let end_wins = match (by_start, by_end) {
            (Some((_, from_start)), Some((_, from_end))) => from_end < from_start,
            (None, Some(_)) => true,
            _ => false,
        };
        match (end_wins, by_start, by_end) {
            (true, _, Some((placed, _))) => (placed, Some(placed + len)),
            (_, Some((placed, _)), _) => (placed, Some(placed)),
            _ => (self.rate.snap(start), None),
        }
    }
}

/// 鼠标在一条缝上、且这条缝能插入片段所在类型的轨道时，返回缝的下标。
fn insert_slot_for(doc: &Document, geo: &TimelineGeometry, clip: ClipId, y: f32) -> Option<usize> {
    let tl = &doc.project.timeline;
    let (track, _) = tl.find_clip(clip)?;
    let kind = tl.track(track)?.kind;
    let slot = geo.insert_slot(y, tl.tracks.len())?;
    insert_slots(tl, kind)
        .is_some_and(|r| r.contains(&slot))
        .then_some(slot)
}

/// 移到鼠标所在轨道；类型不符或重叠时留在原轨道；都不行则不动。
fn move_to_hovered_track(
    doc: &mut Document,
    geo: &TimelineGeometry,
    clip: ClipId,
    y: f32,
    start: Time,
) {
    let current = doc.project.timeline.find_clip(clip).map(|(track, _)| track);
    let hovered = geo
        .track_index_at(y, doc.project.timeline.tracks.len())
        .and_then(|i| doc.project.timeline.tracks.get(i))
        .map(|track| track.id);
    // 主轨磁吸开着、拖到主轨上：按位置插队（主轨始终首尾相接，不能放到空隙里）
    if doc.project.settings.main_magnet
        && hovered.is_some()
        && hovered == ve_core::magnet::main_track(&doc.project.timeline)
        && doc
            .edit(|p| ve_core::magnet::insert_into_main(p, clip, start))
            .is_ok()
    {
        return;
    }
    for track in hovered.into_iter().chain(current) {
        if doc.edit(|p| move_clip(p, clip, track, start)).is_ok() {
            break;
        }
    }
}

/// 时间轴上滚一下滚轮做什么。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wheel {
    /// 缩放（`true` 放大）。
    Zoom(bool),
    /// 滚动（像素，正数往右 / 往下）。
    Scroll {
        /// 横向。
        dx: f32,
        /// 纵向（轨道）。
        dy: f32,
    },
}

/// 剪映的滚轮习惯：滚轮上下滚轨道，Alt / Shift + 滚轮左右滚，Ctrl + 滚轮缩放；
/// 触控板的横向滑动始终左右滚。`dx` / `dy` 是滚轮事件的像素增量（往上滚为正）。
/// 轨道都放得下（`tracks_overflow` 为假，没得上下滚）时，滚轮也左右滚，免得滚了没反应。
#[must_use]
pub fn wheel_action(dx: f32, dy: f32, mods: gpui_kit::Modifiers, tracks_overflow: bool) -> Wheel {
    if mods.control {
        Wheel::Zoom(dy > 0.0)
    } else if mods.alt || mods.shift || !tracks_overflow {
        Wheel::Scroll {
            dx: -(dx + dy),
            dy: 0.0,
        }
    } else {
        Wheel::Scroll { dx: -dx, dy: -dy }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use ve_core::testing::sample_project;

    /// 拖拽的基本行为在磁吸、联动都关着时测（主轨磁吸另有测试）。
    fn setup() -> (Document, ve_core::testing::SampleIds, TimelineGeometry) {
        let (mut p, ids) = sample_project();
        p.settings.main_magnet = false;
        p.settings.linkage = false;
        let g = TimelineGeometry {
            px_per_second: 100.0,
            scroll_x: 0.0,
            ..TimelineGeometry::default()
        };
        (Document::from_project(p, None), ids, g)
    }

    fn x(g: &TimelineGeometry, secs: f64) -> f32 {
        g.time_to_x(Time::from_secs_f64(secs))
    }

    /// 示例工程的轨道行：字幕在最上（剪映的排法）。
    const S: f32 = 0.0;
    const V: f32 = 1.0;
    const A: f32 = 2.0;

    fn row(g: &TimelineGeometry, i: f32) -> f32 {
        g.ruler_height + g.track_height * (i + 0.5)
    }

    fn start_of(d: &Document, c: ClipId) -> Time {
        d.project.timeline.find_clip(c).unwrap().1.start
    }

    #[test]
    fn ruler_click_seeks() {
        let (mut d, _, g) = setup();
        let mut drag = Drag::begin(&d, &g, x(&g, 3.2), 5.0, Time::ZERO).unwrap();
        assert!(matches!(drag, Drag::Scrub));
        let out = drag.update(&mut d, &g, x(&g, 3.2), 5.0, Time::ZERO, true);
        assert_eq!(out.seek, Some(Time::from_millis(3200)));
        // 拖出左边也夹到 0
        let left_of_zero = drag.update(&mut d, &g, g.header_width - 100.0, 5.0, Time::ZERO, true);
        assert_eq!(left_of_zero.seek, Some(Time::ZERO));
    }

    #[test]
    fn drag_clip_moves_with_snap() {
        let (mut d, ids, g) = setup();
        d.begin_gesture();
        // 抓住 v2（5–8 秒）中间 6.5 秒处
        let mut drag = Drag::begin(&d, &g, x(&g, 6.5), row(&g, V), Time::ZERO).unwrap();
        assert!(matches!(drag, Drag::Move { .. }));
        // 拖到 12.06 秒：起点 10.56，不靠近任何目标 → 吸到最近的帧（第 317 帧）
        drag.update(&mut d, &g, x(&g, 12.06), row(&g, V), Time::ZERO, true);
        assert_eq!(
            start_of(&d, ids.v2),
            FrameRate::new(30, 1).time_of_frame(317)
        );
        // 拖到起点 8.04 秒：离 a1 终点（8 秒）4px，吸过去
        let out = drag.update(&mut d, &g, x(&g, 9.54), row(&g, V), Time::ZERO, true);
        assert_eq!(start_of(&d, ids.v2), Time::from_secs(8));
        assert_eq!(out.snap, Some(Time::from_secs(8)));
        // 终点吸到播放头（20 秒）：起点 16.97 → 终点 19.97
        drag.update(
            &mut d,
            &g,
            x(&g, 18.47),
            row(&g, V),
            Time::from_secs(20),
            true,
        );
        assert_eq!(start_of(&d, ids.v2), Time::from_secs(17));
        d.end_gesture();
        assert!(d.undo());
        assert_eq!(start_of(&d, ids.v2), Time::from_secs(5));
    }

    #[test]
    fn drag_into_overlap_keeps_last_valid_position() {
        let (mut d, ids, g) = setup();
        let mut drag = Drag::begin(&d, &g, x(&g, 6.0), row(&g, V), Time::ZERO).unwrap();
        drag.update(&mut d, &g, x(&g, 21.0), row(&g, V), Time::ZERO, true);
        let ok = start_of(&d, ids.v2);
        // 拖到和 v1 重叠：保持上一个合法位置
        drag.update(&mut d, &g, x(&g, 3.0), row(&g, V), Time::ZERO, true);
        assert_eq!(start_of(&d, ids.v2), ok);
    }

    #[test]
    fn drag_to_audio_track_changes_track_and_wrong_kind_stays() {
        let (mut d, ids, g) = setup();
        let mut drag = Drag::begin(&d, &g, x(&g, 6.0), row(&g, V), Time::ZERO).unwrap();
        drag.update(&mut d, &g, x(&g, 21.0), row(&g, A), Time::ZERO, true);
        assert_eq!(
            d.project.timeline.find_clip(ids.v2).unwrap().0,
            ids.audio_track
        );
        // 拖到字幕轨：类型不符，留在原轨（音频轨）
        drag.update(&mut d, &g, x(&g, 25.0), row(&g, S), Time::ZERO, true);
        assert_eq!(
            d.project.timeline.find_clip(ids.v2).unwrap().0,
            ids.audio_track
        );
    }

    #[test]
    fn drag_edge_trims() {
        let (mut d, ids, g) = setup();
        let mut left = Drag::begin(&d, &g, x(&g, 5.0) + 2.0, row(&g, V), Time::ZERO).unwrap();
        assert!(matches!(left, Drag::TrimStart { .. }));
        left.update(&mut d, &g, x(&g, 6.0), row(&g, V), Time::ZERO, true);
        let c = std::sync::Arc::clone(d.project.timeline.find_clip(ids.v2).unwrap().1);
        assert_eq!((c.start, c.end()), (Time::from_secs(6), Time::from_secs(8)));
        let mut right = Drag::begin(&d, &g, x(&g, 8.0) - 2.0, row(&g, V), Time::ZERO).unwrap();
        assert!(matches!(right, Drag::TrimEnd { .. }));
        right.update(&mut d, &g, x(&g, 9.0), row(&g, V), Time::ZERO, true);
        assert_eq!(
            d.project.timeline.find_clip(ids.v2).unwrap().1.end(),
            Time::from_secs(9)
        );
    }

    #[test]
    fn dragging_a_selected_clip_moves_the_whole_selection() {
        let (mut d, ids, g) = setup();
        d.set_selection(vec![ids.v2, ids.a1]);
        let mut drag = Drag::begin(&d, &g, x(&g, 6.5), row(&g, V), Time::ZERO).unwrap();
        // 往后 10 秒，鼠标还拖到了音频行：多选时只平移，不换轨
        drag.update(&mut d, &g, x(&g, 16.5), row(&g, A), Time::ZERO, true);
        assert_eq!(start_of(&d, ids.v2), Time::from_secs(15));
        assert_eq!(start_of(&d, ids.a1), Time::from_secs(10));
        assert_eq!(
            d.project.timeline.find_clip(ids.v2).unwrap().0,
            ids.video_track
        );
        assert_eq!(start_of(&d, ids.v1), Time::ZERO, "没选中的不动");
    }

    #[test]
    fn dragging_an_unselected_clip_moves_only_it() {
        let (mut d, ids, g) = setup();
        d.set_selection(vec![ids.v1, ids.a1]);
        let mut drag = Drag::begin(&d, &g, x(&g, 6.5), row(&g, V), Time::ZERO).unwrap();
        drag.update(&mut d, &g, x(&g, 16.5), row(&g, V), Time::ZERO, true);
        assert_eq!(start_of(&d, ids.v2), Time::from_secs(15));
        assert_eq!(start_of(&d, ids.a1), Time::ZERO);
    }

    #[test]
    fn snapping_off_places_on_frames_only() {
        let (mut d, ids, g) = setup();
        let mut drag = Drag::begin(&d, &g, x(&g, 6.5), row(&g, V), Time::ZERO).unwrap();
        // 起点 8.04 秒离 a1 终点（8 秒）4 像素：吸附关掉时只吸到帧（第 241 帧）
        let out = drag.update(&mut d, &g, x(&g, 9.54), row(&g, V), Time::ZERO, false);
        assert_eq!(
            start_of(&d, ids.v2),
            FrameRate::new(30, 1).time_of_frame(241)
        );
        assert_eq!(out.snap, None);
    }

    #[test]
    fn cursor_shows_what_a_press_would_do() {
        use gpui_kit::CursorStyle;
        let (d, ids, g) = setup();
        let body = g.hit_test(x(&g, 6.5), row(&g, V), &d.project.timeline);
        let edge = g.hit_test(x(&g, 5.0) + 2.0, row(&g, V), &d.project.timeline);
        let empty = g.hit_test(x(&g, 30.0), row(&g, V), &d.project.timeline);
        assert_eq!(cursor_for(body, None, false), CursorStyle::OpenHand);
        assert_eq!(cursor_for(edge, None, false), CursorStyle::ResizeLeftRight);
        assert_eq!(cursor_for(empty, None, false), CursorStyle::Arrow);
        assert_eq!(cursor_for(Hit::Ruler, None, false), CursorStyle::Arrow);
        // 拖动中按拖的类型，不看鼠标下面是什么
        let moving = Drag::begin(&d, &g, x(&g, 6.5), row(&g, V), Time::ZERO).unwrap();
        assert_eq!(
            cursor_for(empty, Some(&moving), false),
            CursorStyle::ClosedHand
        );
        let trim = Drag::TrimEnd { clip: ids.v2 };
        assert_eq!(
            cursor_for(body, Some(&trim), false),
            CursorStyle::ResizeLeftRight
        );
        assert_eq!(
            cursor_for(body, Some(&Drag::Scrub), false),
            CursorStyle::Arrow
        );
        assert_eq!(
            cursor_for(body, None, true),
            CursorStyle::Crosshair,
            "框选中"
        );
    }

    #[test]
    fn empty_area_starts_nothing_and_selection_follows_click() {
        let (d, ids, g) = setup();
        assert!(Drag::begin(&d, &g, x(&g, 30.0), row(&g, 5.0), Time::ZERO).is_none());
        assert!(Drag::begin(&d, &g, x(&g, 30.0), row(&g, V), Time::ZERO).is_none());
        let drag = Drag::begin(&d, &g, x(&g, 2.0), row(&g, A), Time::ZERO).unwrap();
        assert_eq!(drag.clip(), Some(ids.a1));
        assert_eq!(Drag::Scrub.clip(), None);
    }

    /// 滚轮时按着的键。
    const fn m(control: bool, alt: bool, shift: bool) -> gpui_kit::Modifiers {
        gpui_kit::Modifiers {
            control,
            alt,
            shift,
            platform: false,
            function: false,
        }
    }

    #[test]
    fn wheel_follows_jianying_habits() {
        // 往下滚（dy 为负）：轨道放不下时轨道往下滚
        assert_eq!(
            wheel_action(0.0, -120.0, m(false, false, false), true),
            Wheel::Scroll { dx: 0.0, dy: 120.0 }
        );
        // 轨道都放得下（没得上下滚）：滚轮改成左右滚，免得滚了没反应
        assert_eq!(
            wheel_action(0.0, -120.0, m(false, false, false), false),
            Wheel::Scroll { dx: 120.0, dy: 0.0 }
        );
        // Alt / Shift：左右滚
        for (alt, shift) in [(true, false), (false, true)] {
            assert_eq!(
                wheel_action(0.0, -120.0, m(false, alt, shift), true),
                Wheel::Scroll { dx: 120.0, dy: 0.0 }
            );
        }
        // 触控板横向滑动
        assert_eq!(
            wheel_action(-30.0, 0.0, m(false, false, false), true),
            Wheel::Scroll { dx: 30.0, dy: 0.0 }
        );
        assert_eq!(
            wheel_action(0.0, 120.0, m(true, false, false), true),
            Wheel::Zoom(true)
        );
        assert_eq!(
            wheel_action(0.0, -120.0, m(true, true, false), true),
            Wheel::Zoom(false)
        );
    }

    #[test]
    fn dragging_onto_a_track_boundary_offers_a_new_track() {
        let (mut d, ids, g) = setup();
        let mut drag = Drag::begin(&d, &g, x(&g, 6.0), row(&g, V), Time::ZERO).unwrap();
        // 字幕轨和视频轨之间的缝（第 1 条）：视频片段可以插在这里
        let gap = g.ruler_height + g.track_height * V + 2.0;
        let out = drag.update(&mut d, &g, x(&g, 7.0), gap, Time::ZERO, false);
        assert_eq!(out.insert, Some((1, Time::from_secs(6))), "松手时放在 6 秒");
        // 拖动中片段先不动（主轨磁吸开着时也不会被排回去）
        assert_eq!(
            d.project.timeline.find_clip(ids.v2).unwrap().0,
            ids.video_track
        );
        assert_eq!(start_of(&d, ids.v2), Time::from_secs(5));
        // 视频轨和音频轨之间（第 2 条）也行；最下面（第 3 条，音频块下面）不行
        let between = g.ruler_height + g.track_height * A - 2.0;
        assert_eq!(
            drag.update(&mut d, &g, x(&g, 7.0), between, Time::ZERO, false)
                .insert,
            Some((2, Time::from_secs(6)))
        );
        let below = g.ruler_height + g.track_height * 3.0 + 30.0;
        assert_eq!(
            drag.update(&mut d, &g, x(&g, 7.0), below, Time::ZERO, false)
                .insert,
            None
        );
        // 回到轨道中间：不提示
        assert_eq!(
            drag.update(&mut d, &g, x(&g, 7.0), row(&g, V), Time::ZERO, false)
                .insert,
            None
        );
    }

    #[test]
    fn with_magnet_dragging_on_the_main_track_reorders() {
        let (p, ids) = sample_project();
        assert!(p.settings.main_magnet, "新工程默认开");
        let mut d = Document::from_project(p, None);
        let g = setup().2;
        // v2（5–8）往前拖到 1 秒附近：排到 v1 前面，主轨仍然首尾相接
        let mut drag = Drag::begin(&d, &g, x(&g, 6.0), row(&g, V), Time::ZERO).unwrap();
        drag.update(&mut d, &g, x(&g, 2.0), row(&g, V), Time::ZERO, false);
        assert_eq!(start_of(&d, ids.v2), Time::ZERO);
        assert_eq!(start_of(&d, ids.v1), Time::from_secs(3));
    }
}
