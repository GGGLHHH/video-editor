//! 时间轴拖拽：拖标尺定位、拖片段移动（可换轨、吸附）、拖边缘修剪。纯逻辑。
//!
//! 位置都吸附到帧；靠近播放头或其他片段边缘（8 像素内）时吸过去。

use super::{Hit, TimelineGeometry, Zone};
use crate::document::Document;
use ve_core::edit::{move_clip, trim_end, trim_start};
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
}

/// 一次拖拽更新的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DragOutcome {
    /// 要定位到的时刻（拖标尺时）。
    pub seek: Option<Time>,
    /// 吸附线位置（用于画提示竖线）。
    pub snap: Option<Time>,
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
            Self::Move { clip, .. } | Self::TrimStart { clip } | Self::TrimEnd { clip } => {
                Some(*clip)
            }
        }
    }

    /// 鼠标移动到 `(x, y)`：更新工程（编辑失败则保持上一个合法状态）。
    pub fn update(
        &mut self,
        doc: &mut Document,
        geo: &TimelineGeometry,
        x: f32,
        y: f32,
        playhead: Time,
    ) -> DragOutcome {
        let pointer = geo.x_to_time(x).max(Time::ZERO);
        let snapper = Snapper::new(doc, geo, playhead, self.clip());
        match *self {
            Self::Scrub => DragOutcome {
                seek: Some(snapper.rate.snap(pointer)),
                snap: None,
            },
            Self::Move {
                clip,
                grab,
                duration,
            } => {
                let (start, line) = snapper.place_range((pointer - grab).max(Time::ZERO), duration);
                move_to_hovered_track(doc, geo, clip, y, start);
                DragOutcome {
                    seek: None,
                    snap: line,
                }
            }
            Self::TrimStart { clip } => {
                let (to, line) = snapper.place_point(pointer);
                let _ = doc.edit(|p| trim_start(p, clip, to));
                DragOutcome {
                    seek: None,
                    snap: line,
                }
            }
            Self::TrimEnd { clip } => {
                let (to, line) = snapper.place_point(pointer);
                let _ = doc.edit(|p| trim_end(p, clip, to));
                DragOutcome {
                    seek: None,
                    snap: line,
                }
            }
        }
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
        exclude: Option<ClipId>,
    ) -> Self {
        let exclude: Vec<ClipId> = exclude.into_iter().collect();
        Self {
            targets: snap_targets(&doc.project.timeline, playhead, &exclude),
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
    for track in hovered.into_iter().chain(current) {
        if doc.edit(|p| move_clip(p, clip, track, start)).is_ok() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use ve_core::testing::sample_project;

    fn setup() -> (Document, ve_core::testing::SampleIds, TimelineGeometry) {
        let (p, ids) = sample_project();
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
        let out = drag.update(&mut d, &g, x(&g, 3.2), 5.0, Time::ZERO);
        assert_eq!(out.seek, Some(Time::from_millis(3200)));
        // 拖出左边也夹到 0
        let left_of_zero = drag.update(&mut d, &g, g.header_width - 100.0, 5.0, Time::ZERO);
        assert_eq!(left_of_zero.seek, Some(Time::ZERO));
    }

    #[test]
    fn drag_clip_moves_with_snap() {
        let (mut d, ids, g) = setup();
        d.begin_gesture();
        // 抓住 v2（5–8 秒）中间 6.5 秒处
        let mut drag = Drag::begin(&d, &g, x(&g, 6.5), row(&g, 0.0), Time::ZERO).unwrap();
        assert!(matches!(drag, Drag::Move { .. }));
        // 拖到 12.06 秒：起点 10.56，不靠近任何目标 → 吸到最近的帧（第 317 帧）
        drag.update(&mut d, &g, x(&g, 12.06), row(&g, 0.0), Time::ZERO);
        assert_eq!(
            start_of(&d, ids.v2),
            FrameRate::new(30, 1).time_of_frame(317)
        );
        // 拖到起点 8.04 秒：离 a1 终点（8 秒）4px，吸过去
        let out = drag.update(&mut d, &g, x(&g, 9.54), row(&g, 0.0), Time::ZERO);
        assert_eq!(start_of(&d, ids.v2), Time::from_secs(8));
        assert_eq!(out.snap, Some(Time::from_secs(8)));
        // 终点吸到播放头（20 秒）：起点 16.97 → 终点 19.97
        drag.update(&mut d, &g, x(&g, 18.47), row(&g, 0.0), Time::from_secs(20));
        assert_eq!(start_of(&d, ids.v2), Time::from_secs(17));
        d.end_gesture();
        assert!(d.undo());
        assert_eq!(start_of(&d, ids.v2), Time::from_secs(5));
    }

    #[test]
    fn drag_into_overlap_keeps_last_valid_position() {
        let (mut d, ids, g) = setup();
        let mut drag = Drag::begin(&d, &g, x(&g, 6.0), row(&g, 0.0), Time::ZERO).unwrap();
        drag.update(&mut d, &g, x(&g, 21.0), row(&g, 0.0), Time::ZERO);
        let ok = start_of(&d, ids.v2);
        // 拖到和 v1 重叠：保持上一个合法位置
        drag.update(&mut d, &g, x(&g, 3.0), row(&g, 0.0), Time::ZERO);
        assert_eq!(start_of(&d, ids.v2), ok);
    }

    #[test]
    fn drag_to_audio_track_changes_track_and_wrong_kind_stays() {
        let (mut d, ids, g) = setup();
        let mut drag = Drag::begin(&d, &g, x(&g, 6.0), row(&g, 0.0), Time::ZERO).unwrap();
        drag.update(&mut d, &g, x(&g, 21.0), row(&g, 1.0), Time::ZERO);
        assert_eq!(
            d.project.timeline.find_clip(ids.v2).unwrap().0,
            ids.audio_track
        );
        // 拖到字幕轨：类型不符，留在原轨（音频轨）
        drag.update(&mut d, &g, x(&g, 25.0), row(&g, 2.0), Time::ZERO);
        assert_eq!(
            d.project.timeline.find_clip(ids.v2).unwrap().0,
            ids.audio_track
        );
    }

    #[test]
    fn drag_edge_trims() {
        let (mut d, ids, g) = setup();
        let mut left = Drag::begin(&d, &g, x(&g, 5.0) + 2.0, row(&g, 0.0), Time::ZERO).unwrap();
        assert!(matches!(left, Drag::TrimStart { .. }));
        left.update(&mut d, &g, x(&g, 6.0), row(&g, 0.0), Time::ZERO);
        let c = std::sync::Arc::clone(d.project.timeline.find_clip(ids.v2).unwrap().1);
        assert_eq!((c.start, c.end()), (Time::from_secs(6), Time::from_secs(8)));
        let mut right = Drag::begin(&d, &g, x(&g, 8.0) - 2.0, row(&g, 0.0), Time::ZERO).unwrap();
        assert!(matches!(right, Drag::TrimEnd { .. }));
        right.update(&mut d, &g, x(&g, 9.0), row(&g, 0.0), Time::ZERO);
        assert_eq!(
            d.project.timeline.find_clip(ids.v2).unwrap().1.end(),
            Time::from_secs(9)
        );
    }

    #[test]
    fn empty_area_starts_nothing_and_selection_follows_click() {
        let (d, ids, g) = setup();
        assert!(Drag::begin(&d, &g, x(&g, 30.0), row(&g, 5.0), Time::ZERO).is_none());
        assert!(Drag::begin(&d, &g, x(&g, 30.0), row(&g, 0.0), Time::ZERO).is_none());
        let drag = Drag::begin(&d, &g, x(&g, 2.0), row(&g, 1.0), Time::ZERO).unwrap();
        assert_eq!(drag.clip(), Some(ids.a1));
        assert_eq!(Drag::Scrub.clip(), None);
    }
}
