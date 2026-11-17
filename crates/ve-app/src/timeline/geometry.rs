//! 时间轴几何：时间 ↔ 像素、命中检测、缩放。纯函数，不依赖界面框架。
//!
//! 坐标都是时间轴视图内的局部坐标：最左是轨道头，最上是标尺。

use ve_core::{ClipId, Time, Timeline, TrackId};

/// 最小缩放（像素 / 秒）。
pub const MIN_PX_PER_SECOND: f32 = 2.0;
/// 最大缩放（像素 / 秒）。
pub const MAX_PX_PER_SECOND: f32 = 2000.0;
/// 片段边缘可拖动修剪的宽度（像素）。
pub const EDGE_PX: f32 = 6.0;

/// 片段上的区域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// 中间：拖动移动。
    Body,
    /// 左边缘：修剪起点。
    LeftEdge,
    /// 右边缘：修剪终点。
    RightEdge,
}

/// 命中结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// 标尺。
    Ruler,
    /// 轨道头。
    Header(TrackId),
    /// 片段。
    Clip {
        /// 片段。
        clip: ClipId,
        /// 区域。
        zone: Zone,
    },
    /// 轨道空白处。
    Track(TrackId),
    /// 轨道以外。
    Empty,
}

/// 时间轴视图的几何参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelineGeometry {
    /// 缩放：每秒多少像素。
    pub px_per_second: f32,
    /// 横向滚动（像素）。
    pub scroll_x: f32,
    /// 纵向滚动（像素）：轨道多了放不下时，轨道行在标尺下面上下滚。
    pub scroll_y: f32,
    /// 每条轨道高。
    pub track_height: f32,
    /// 标尺高。
    pub ruler_height: f32,
    /// 轨道头宽。
    pub header_width: f32,
}

impl Default for TimelineGeometry {
    fn default() -> Self {
        Self {
            px_per_second: 50.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            track_height: 56.0,
            ruler_height: 28.0,
            header_width: 72.0,
        }
    }
}

impl TimelineGeometry {
    /// 时间 → 横坐标。
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "屏幕坐标范围远小于 f32 上限"
    )]
    pub fn time_to_x(&self, t: Time) -> f32 {
        (f64::from(self.header_width) + t.as_secs_f64() * f64::from(self.px_per_second)
            - f64::from(self.scroll_x)) as f32
    }

    /// 横坐标 → 时间。
    #[must_use]
    pub fn x_to_time(&self, x: f32) -> Time {
        Time::from_secs_f64(
            (f64::from(x) - f64::from(self.header_width) + f64::from(self.scroll_x))
                / f64::from(self.px_per_second),
        )
    }

    /// 纵坐标所在的轨道下标。
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "已排除负数"
    )]
    pub fn track_index_at(&self, y: f32, track_count: usize) -> Option<usize> {
        if y < self.ruler_height {
            return None;
        }
        let i = ((y - self.ruler_height + self.scroll_y) / self.track_height).floor() as usize;
        (i < track_count).then_some(i)
    }

    /// 内容总高（标尺 + 轨道）。
    #[must_use]
    #[expect(clippy::cast_precision_loss, reason = "轨道数很少")]
    pub fn content_height(&self, track_count: usize) -> f32 {
        self.ruler_height + self.track_height * track_count as f32
    }

    /// 命中检测。
    #[must_use]
    pub fn hit_test(&self, x: f32, y: f32, timeline: &Timeline) -> Hit {
        if y < self.ruler_height {
            return Hit::Ruler;
        }
        let Some(track) = self
            .track_index_at(y, timeline.tracks.len())
            .and_then(|i| timeline.tracks.get(i))
        else {
            return Hit::Empty;
        };
        if x < self.header_width {
            return Hit::Header(track.id);
        }
        for clip in &track.clips {
            let (x0, x1) = (self.time_to_x(clip.start), self.time_to_x(clip.end()));
            if x >= x0 && x <= x1 {
                let edge = EDGE_PX.min((x1 - x0) / 4.0);
                let zone = if x - x0 <= edge {
                    Zone::LeftEdge
                } else if x1 - x <= edge {
                    Zone::RightEdge
                } else {
                    Zone::Body
                };
                return Hit::Clip {
                    clip: clip.id,
                    zone,
                };
            }
        }
        Hit::Track(track.id)
    }

    /// 框选：屏幕上 `a`、`b` 两点围成的矩形碰到的片段（按轨道从上到下、时间先后）。
    #[must_use]
    pub fn clips_in_rect(&self, timeline: &Timeline, a: (f32, f32), b: (f32, f32)) -> Vec<ClipId> {
        let (left, right) = (a.0.min(b.0), a.0.max(b.0));
        let (top, bottom) = (a.1.min(b.1), a.1.max(b.1));
        timeline
            .tracks
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                let y0 = self.track_top(*i);
                y0 < bottom && y0 + self.track_height > top
            })
            .flat_map(|(_, t)| t.clips.iter())
            .filter(|c| self.time_to_x(c.start) < right && self.time_to_x(c.end()) > left)
            .map(|c| c.id)
            .collect()
    }

    /// 以横坐标 `anchor_x` 为中心缩放（该处时间不动），缩放比夹在范围内。
    pub fn zoom_around(&mut self, anchor_x: f32, factor: f32) {
        let t = self.x_to_time(anchor_x);
        self.px_per_second =
            (self.px_per_second * factor).clamp(MIN_PX_PER_SECOND, MAX_PX_PER_SECOND);
        let x_now = self.time_to_x(t);
        self.scroll_by(x_now - anchor_x);
    }

    /// 第 `i` 条轨道顶边的纵坐标（算上纵向滚动）。
    #[must_use]
    #[expect(clippy::cast_precision_loss, reason = "轨道数很少")]
    pub fn track_top(&self, i: usize) -> f32 {
        self.ruler_height + self.track_height * i as f32 - self.scroll_y
    }

    /// 纵向滚动，夹在「刚好露出最后一条轨」以内；放得下时不滚。
    pub fn scroll_tracks_by(&mut self, dy: f32, track_count: usize, viewport_height: f32) {
        let max = (self.content_height(track_count) - viewport_height).max(0.0);
        self.scroll_y = (self.scroll_y + dy).clamp(0.0, max);
    }

    /// 轨道数或可视高度变了以后，把纵向滚动重新夹回范围内。
    pub fn clamp_scroll_y(&mut self, track_count: usize, viewport_height: f32) {
        self.scroll_tracks_by(0.0, track_count, viewport_height);
    }

    /// 横向滚动（不小于 0）。
    pub fn scroll_by(&mut self, dx: f32) {
        self.scroll_x = (self.scroll_x + dx).max(0.0);
    }

    /// 标尺大刻度间隔：保证刻度之间至少 80 像素。
    #[must_use]
    pub fn tick_step(&self) -> Time {
        const STEPS_MS: [i64; 14] = [
            100, 200, 500, 1000, 2000, 5000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000,
            600_000, 1_800_000,
        ];
        STEPS_MS
            .iter()
            .map(|&ms| Time::from_millis(ms))
            .find(|t| t.as_secs_f64() * f64::from(self.px_per_second) >= 80.0)
            .unwrap_or(Time::from_secs(3600))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::testing::sample_project;

    fn geo() -> TimelineGeometry {
        TimelineGeometry {
            px_per_second: 100.0,
            scroll_x: 0.0,
            ..TimelineGeometry::default()
        }
    }

    #[test]
    fn time_x_round_trip() {
        let mut g = geo();
        g.scroll_x = 250.0;
        for ms in [0, 1, 999, 5000, 123_456] {
            let t = Time::from_millis(ms);
            let back = g.x_to_time(g.time_to_x(t));
            assert!((back - t).0.abs() <= Time::from_millis(1).0, "{ms}");
        }
        assert!((g.time_to_x(Time::from_secs(3)) - (g.header_width + 300.0 - 250.0)).abs() < 1e-3);
    }

    #[test]
    fn rect_selects_clips_it_touches_on_rows_it_spans() {
        let (p, ids) = sample_project();
        let mut g = geo();
        let x = |s: f32| g.header_width + s * 100.0;
        let row = |g: &TimelineGeometry, i: f32| g.ruler_height + g.track_height * i;
        // 视频、音频两行中间，6–7 秒：v2（5–8）、a1（0–8）
        let mut got = g.clips_in_rect(&p.timeline, (x(6.0), row(&g, 0.5)), (x(7.0), row(&g, 1.5)));
        got.sort_unstable();
        assert_eq!(got, vec![ids.v2, ids.a1]);
        // 反着拖一样；只扫到字幕行 3.5–4 秒：s1 是 1–3，扫不到
        let none = g.clips_in_rect(&p.timeline, (x(4.0), row(&g, 2.9)), (x(3.5), row(&g, 2.1)));
        assert_eq!(none, Vec::<ClipId>::new());
        // 纵向滚动后按屏幕位置算：滚掉一行，屏幕第 0 行是音频轨
        g.scroll_y = g.track_height;
        assert_eq!(
            g.clips_in_rect(&p.timeline, (x(1.0), row(&g, 0.2)), (x(2.0), row(&g, 0.8))),
            vec![ids.a1]
        );
    }

    #[test]
    fn hit_test_finds_clip_body_and_edges() {
        let (p, ids) = sample_project();
        let g = geo();
        let y_video = g.ruler_height + g.track_height * 0.5;
        let x = |s: f32| g.header_width + s * 100.0;
        assert_eq!(
            g.hit_test(x(2.0), y_video, &p.timeline),
            Hit::Clip {
                clip: ids.v1,
                zone: Zone::Body
            }
        );
        assert_eq!(
            g.hit_test(x(0.0) + 2.0, y_video, &p.timeline),
            Hit::Clip {
                clip: ids.v1,
                zone: Zone::LeftEdge
            }
        );
        assert_eq!(
            g.hit_test(x(5.0) - 3.0, y_video, &p.timeline),
            Hit::Clip {
                clip: ids.v1,
                zone: Zone::RightEdge
            }
        );
        assert_eq!(
            g.hit_test(x(9.0), y_video, &p.timeline),
            Hit::Track(ids.video_track)
        );
        assert_eq!(g.hit_test(x(1.0), 3.0, &p.timeline), Hit::Ruler);
        assert_eq!(
            g.hit_test(5.0, y_video, &p.timeline),
            Hit::Header(ids.video_track)
        );
        let y_subs = g.ruler_height + g.track_height * 2.5;
        assert_eq!(
            g.hit_test(x(2.0), y_subs, &p.timeline),
            Hit::Clip {
                clip: ids.s1,
                zone: Zone::Body
            }
        );
        assert_eq!(
            g.hit_test(x(2.0), g.ruler_height + g.track_height * 9.0, &p.timeline),
            Hit::Empty
        );
    }

    #[test]
    fn short_clip_edges_do_not_swallow_body() {
        let (p, ids) = sample_project();
        let mut g = geo();
        g.px_per_second = 4.0; // 2 秒字幕只有 8px 宽
        let y = g.ruler_height + g.track_height * 2.5;
        let mid = g.time_to_x(Time::from_secs(2));
        assert_eq!(
            g.hit_test(mid, y, &p.timeline),
            Hit::Clip {
                clip: ids.s1,
                zone: Zone::Body
            }
        );
    }

    #[test]
    fn zoom_keeps_anchor_time_fixed_and_clamps() {
        let mut g = geo();
        g.scroll_x = 100.0;
        let anchor = 400.0;
        let before = g.x_to_time(anchor);
        g.zoom_around(anchor, 2.0);
        assert!((g.px_per_second - 200.0).abs() < 1e-3);
        assert!((g.x_to_time(anchor) - before).0.abs() <= Time::from_millis(1).0);
        g.zoom_around(anchor, 1e9);
        assert!((g.px_per_second - MAX_PX_PER_SECOND).abs() < 1e-3);
        g.zoom_around(anchor, 1e-9);
        assert!((g.px_per_second - MIN_PX_PER_SECOND).abs() < 1e-3);
        assert!(g.scroll_x >= 0.0);
    }

    #[test]
    fn scroll_never_negative_and_track_rows() {
        let mut g = geo();
        g.scroll_by(-50.0);
        assert!(g.scroll_x.abs() < f32::EPSILON);
        g.scroll_by(120.0);
        assert!((g.scroll_x - 120.0).abs() < 1e-3);
        assert_eq!(g.track_index_at(g.ruler_height + 1.0, 3), Some(0));
        assert_eq!(
            g.track_index_at(g.ruler_height + g.track_height * 3.0 + 1.0, 3),
            None
        );
        assert_eq!(g.track_index_at(1.0, 3), None);
        assert!((g.content_height(3) - (g.ruler_height + 3.0 * g.track_height)).abs() < 1e-3);
    }

    #[test]
    fn ruler_ticks_adapt_to_zoom() {
        let g = geo(); // 100px/秒 → 每秒一个大刻度
        assert_eq!(g.tick_step(), Time::from_secs(1));
        let dense = TimelineGeometry {
            px_per_second: 1000.0,
            ..geo()
        };
        assert!(dense.tick_step() < Time::from_secs(1));
        let sparse = TimelineGeometry {
            px_per_second: 5.0,
            ..geo()
        };
        assert!(sparse.tick_step() >= Time::from_secs(10));
    }

    #[test]
    fn vertical_scroll_shifts_rows_and_clamps() {
        let mut g = TimelineGeometry::default();
        // 6 条轨，视口 200 像素高：内容 28 + 6×56 = 364，最多滚 164
        g.scroll_tracks_by(100.0, 6, 200.0);
        assert!((g.scroll_y - 100.0).abs() < 1e-3);
        assert!((g.track_top(2) - (28.0 + 112.0 - 100.0)).abs() < 1e-3);
        // 标尺下面第一条可见的是第 1 行（第 0 行滚上去了）
        assert_eq!(g.track_index_at(30.0, 6), Some(1));
        g.scroll_tracks_by(1000.0, 6, 200.0);
        assert!((g.scroll_y - 164.0).abs() < 1e-3);
        g.scroll_tracks_by(-1000.0, 6, 200.0);
        assert_eq!(g.scroll_y, 0.0);
        // 放得下时不滚
        g.scroll_tracks_by(50.0, 2, 500.0);
        assert_eq!(g.scroll_y, 0.0);
    }
}
