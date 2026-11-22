//! 时间轴几何：时间 ↔ 像素、命中检测、缩放。纯函数，不依赖界面框架。
//!
//! 坐标都是时间轴视图内的局部坐标：最左是轨道头，最上是标尺。

use ve_core::{ClipId, Time, Timeline, TrackId};

/// 最小缩放（像素 / 秒）。
pub const MIN_PX_PER_SECOND: f32 = 2.0;
/// 最大缩放（像素 / 秒）。
pub const MAX_PX_PER_SECOND: f32 = 2000.0;
/// 缩放滑块的位置（0–100，按对数均匀）→ 像素 / 秒。
#[must_use]
pub fn px_for_zoom_level(level: f32) -> f32 {
    let t = (level / 100.0).clamp(0.0, 1.0);
    MIN_PX_PER_SECOND * (MAX_PX_PER_SECOND / MIN_PX_PER_SECOND).powf(t)
}

/// 像素 / 秒 → 缩放滑块的位置（0–100）。
#[must_use]
pub fn zoom_level(px_per_second: f32) -> f32 {
    let px = px_per_second.clamp(MIN_PX_PER_SECOND, MAX_PX_PER_SECOND);
    100.0 * (px / MIN_PX_PER_SECOND).ln() / (MAX_PX_PER_SECOND / MIN_PX_PER_SECOND).ln()
}

/// 刚好放下整条时间线的缩放（剪映「全局预览缩放」Shift+Z）：总长占轨道区的九成。
/// 空时间轴返回 `None`。
#[must_use]
pub fn fit_px_per_second(duration: Time, lane_width: f32) -> Option<f32> {
    let secs = duration.as_secs_f64();
    if secs <= 0.0 || lane_width <= 0.0 {
        return None;
    }
    #[expect(clippy::cast_possible_truncation, reason = "缩放值很小")]
    let px = (f64::from(lane_width) * 0.9 / secs) as f32;
    Some(px.clamp(MIN_PX_PER_SECOND, MAX_PX_PER_SECOND))
}

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

/// 音频片段淡入 / 淡出拖点离片段上沿多远（像素）。
pub const FADE_HANDLE_Y: f32 = 5.0;
/// 淡入 / 淡出拖点的点击范围（像素，横竖各这么远）。
pub const FADE_HIT_PX: f32 = 6.0;
/// 没有淡入淡出时拖点离片段边缘多远画（不和修剪的边缘挤在一起，像素）。
pub const FADE_INSET: f32 = 4.0;

/// 关键帧小菱形的点击范围（像素，横竖各这么远）。
pub const KEY_HIT_PX: f32 = 7.0;
/// 片段两头的关键帧离片段边缘至少这么远画（整个菱形画在片段里，像素）。
pub const KEY_INSET: f32 = 6.5;

/// 轨道分界上下多少像素内算「缝」（拖进去新建轨道）。
pub const INSERT_BAND: f32 = 6.0;

/// 播放头离右边这么近（像素）就翻页。
const FOLLOW_MARGIN: f32 = 16.0;
/// 拖动时离片段区左右边缘这么近（像素）就开始自动滚。
pub const EDGE_SCROLL_ZONE: f32 = 40.0;
/// 自动滚最快每帧多少像素。
const EDGE_SCROLL_MAX: f32 = 24.0;

/// 横向滚动条的高度（贴在时间轴底边）。
pub const SCROLLBAR_HEIGHT: f32 = 10.0;
/// 滚动条滑块最短多少像素（内容很长时也抓得住）。
const MIN_THUMB: f32 = 24.0;

/// 横向滚动条的滑块（时间轴本地坐标）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollThumb {
    /// 左边。
    pub x: f32,
    /// 宽度。
    pub width: f32,
}

/// 只有特效 / 调节片段的视频轨高度是普通轨的几成（剪映的特效轨是细条）。
pub const THIN_ROW: f32 = 0.5;
/// 文字轨高度是普通轨的几成。
pub const TEXT_ROW: f32 = 0.75;

/// 时间轴视图的几何参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelineGeometry {
    /// 缩放：每秒多少像素。
    pub px_per_second: f32,
    /// 横向滚动（像素）。
    pub scroll_x: f32,
    /// 纵向滚动（像素）：轨道多了放不下时，轨道行在标尺下面上下滚。
    pub scroll_y: f32,
    /// 普通轨道（视频、音频）的高；文字轨、特效轨按比例矮一些（[`Self::row_height`]）。
    pub track_height: f32,
    /// 第 i 位是 1：第 i 条轨是细条（只有特效 / 调节片段的视频轨）。每帧按时间轴重算（[`Self::sync_rows`]）。
    pub thin_rows: u64,
    /// 第 i 位是 1：第 i 条轨是文字轨。
    pub text_rows: u64,
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
            thin_rows: 0,
            text_rows: 0,
            ruler_height: 28.0,
            header_width: 132.0,
        }
    }
}

impl TimelineGeometry {
    /// 按时间轴记下哪几条轨是细条、哪几条是文字轨（只管前 64 条，再往后都按普通高度）。
    pub fn sync_rows(&mut self, timeline: &Timeline) {
        let (mut thin, mut text) = (0u64, 0u64);
        for (i, t) in timeline.tracks.iter().enumerate().take(64) {
            let bit = 1u64 << i;
            match t.kind {
                ve_core::TrackKind::Subtitle => text |= bit,
                ve_core::TrackKind::Video
                    if !t.clips.is_empty() && t.clips.iter().all(|c| c.is_overlay_tool()) =>
                {
                    thin |= bit;
                }
                _ => {}
            }
        }
        self.thin_rows = thin;
        self.text_rows = text;
    }

    /// 第 `i` 条轨道的高。
    #[must_use]
    pub fn row_height(&self, i: usize) -> f32 {
        let bit = if i < 64 { 1u64 << i } else { 0 };
        if self.thin_rows & bit != 0 {
            self.track_height * THIN_ROW
        } else if self.text_rows & bit != 0 {
            self.track_height * TEXT_ROW
        } else {
            self.track_height
        }
    }

    /// 前 `n` 条轨道加起来多高。
    #[must_use]
    pub fn rows_height(&self, n: usize) -> f32 {
        (0..n).map(|i| self.row_height(i)).sum()
    }

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
    pub fn track_index_at(&self, y: f32, track_count: usize) -> Option<usize> {
        if y < self.ruler_height {
            return None;
        }
        let mut rest = y - self.ruler_height + self.scroll_y;
        for i in 0..track_count {
            let h = self.row_height(i);
            if rest < h {
                return Some(i);
            }
            rest -= h;
        }
        None
    }

    /// 音频片段上沿的淡入 / 淡出拖点在哪（本地坐标的横坐标）：（淡入，淡出）。
    #[must_use]
    pub fn fade_handles(&self, clip: &ve_core::Clip) -> (f32, f32) {
        let (x0, x1) = (self.time_to_x(clip.start), self.time_to_x(clip.end()));
        let fin = self
            .time_to_x(clip.start + clip.fade_in)
            .max(x0 + FADE_INSET);
        let fout = self
            .time_to_x(clip.end() - clip.fade_out)
            .min(x1 - FADE_INSET);
        (fin.min(x1), fout.max(x0))
    }

    /// 本地坐标 `(x, y)` 按在哪个音频片段的淡入（`true`）/ 淡出（`false`）拖点上。
    #[must_use]
    pub fn fade_handle_at(&self, x: f32, y: f32, timeline: &Timeline) -> Option<(ClipId, bool)> {
        let i = self.track_index_at(y, timeline.tracks.len())?;
        let track = &timeline.tracks[i];
        if track.kind != ve_core::TrackKind::Audio || track.locked {
            return None;
        }
        if (y - (self.track_top(i) + FADE_HANDLE_Y)).abs() > FADE_HIT_PX {
            return None;
        }
        track.clips.iter().find_map(|c| {
            let (fin, fout) = self.fade_handles(c);
            if (x - fin).abs() <= FADE_HIT_PX {
                Some((c.id, true))
            } else if (x - fout).abs() <= FADE_HIT_PX {
                Some((c.id, false))
            } else {
                None
            }
        })
    }

    /// 选中的片段（`selected`）中部的关键帧小菱形：本地坐标 `(x, y)` 按在哪个上面就返回
    /// （片段，关键帧相对片段起点的时间）；离得最近的那个，横竖各 [`KEY_HIT_PX`] 像素以内。
    #[must_use]
    pub fn keyframe_at(
        &self,
        x: f32,
        y: f32,
        timeline: &Timeline,
        selected: &[ClipId],
    ) -> Option<(ClipId, Time)> {
        let i = self.track_index_at(y, timeline.tracks.len())?;
        let mid = self.track_top(i) + self.row_height(i) / 2.0;
        if (y - mid).abs() > KEY_HIT_PX {
            return None;
        }
        timeline.tracks[i]
            .clips
            .iter()
            .filter(|c| selected.contains(&c.id))
            .flat_map(|c| {
                let (left, right) = (self.time_to_x(c.start), self.time_to_x(c.end()));
                c.animation.times().into_iter().map(move |t| {
                    let kx = self
                        .time_to_x(c.start + t)
                        .clamp(left + KEY_INSET, (right - KEY_INSET).max(left + KEY_INSET));
                    ((kx - x).abs(), c.id, t)
                })
            })
            .filter(|(d, ..)| *d <= KEY_HIT_PX)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id, t)| (id, t))
    }

    /// 纵坐标落在哪条「缝」上：第 `i` 条缝是第 `i - 1` 和第 `i` 条轨道的分界（0 是第一条轨道的上沿，
    /// `track_count` 是最后一条的下沿）。分界上下 [`INSERT_BAND`] 像素内、或最后一条轨道下面的空白，
    /// 都算在缝上；拖片段到缝上松手会在那里新建轨道。
    #[must_use]
    pub fn insert_slot(&self, y: f32, track_count: usize) -> Option<usize> {
        if y < self.ruler_height {
            return None;
        }
        let at = y - self.ruler_height + self.scroll_y;
        if at >= self.rows_height(track_count) {
            return Some(track_count);
        }
        // 离得最近的那条缝（第 k 条缝在前 k 条轨道的下沿）
        (0..=track_count)
            .map(|k| (k, (self.rows_height(k) - at).abs()))
            .filter(|&(_, d)| d <= INSERT_BAND)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(k, _)| k)
    }

    /// 内容总高（标尺 + 轨道）。
    #[must_use]
    pub fn content_height(&self, track_count: usize) -> f32 {
        self.ruler_height + self.rows_height(track_count)
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
                y0 < bottom && y0 + self.row_height(*i) > top
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
    pub fn track_top(&self, i: usize) -> f32 {
        self.ruler_height + self.rows_height(i) - self.scroll_y
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

    /// 横向滚动（不小于 0；右边由 [`Self::clamp_scroll_x`] 夹）。
    pub fn scroll_by(&mut self, dx: f32) {
        self.scroll_x = (self.scroll_x + dx).max(0.0);
    }

    /// 片段区（去掉轨道头）的宽度。
    fn lane_width(&self, view_width: f32) -> f32 {
        (view_width - self.header_width).max(1.0)
    }

    /// 播放头 `t` 在视图里看得见（片段区内、离右边留一点）。
    #[must_use]
    pub fn shows(&self, t: Time, view_width: f32) -> bool {
        let x = self.time_to_x(t);
        x >= self.header_width && x <= view_width - FOLLOW_MARGIN
    }

    /// 播放时让播放头 `t` 留在视图里（剪映的「翻页」）：出了视图就翻页，正放（`forward`）
    /// 翻到片段区一成处、倒放翻到九成处（循环播放跳回起点也按正放落在左边）。滚动了返回 `true`。
    pub fn follow(&mut self, t: Time, view_width: f32, forward: bool) -> bool {
        if self.shows(t, view_width) {
            return false;
        }
        let lane = self.lane_width(view_width);
        let x = self.time_to_x(t);
        let target = self.header_width + lane * if forward { 0.1 } else { 0.9 };
        let before = self.scroll_x;
        self.scroll_by(x - target);
        (self.scroll_x - before).abs() > f32::EPSILON
    }

    /// 拖动时鼠标在横坐标 `x`：贴近片段区左右边缘（[`EDGE_SCROLL_ZONE`] 像素内或出了边）时，
    /// 每帧该横向滚多少像素（越靠边越快，出了边按最快）；不在边上为 0。
    #[must_use]
    pub fn edge_scroll(&self, x: f32, view_width: f32) -> f32 {
        let left = self.header_width + EDGE_SCROLL_ZONE;
        let right = view_width - EDGE_SCROLL_ZONE;
        let depth = if x > right {
            x - right
        } else if x < left {
            x - left
        } else {
            return 0.0;
        };
        (depth / EDGE_SCROLL_ZONE).clamp(-1.0, 1.0) * EDGE_SCROLL_MAX
    }

    /// 横向最多滚多远：时长为 `duration` 的内容最多滚到结尾贴着轨道头（后面留出一整屏，
    /// 方便往后接片段；内容短时也能滚，放大时播放头才能留在原处）；空时间轴不能滚。
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "屏幕坐标范围远小于 f32 上限"
    )]
    pub fn max_scroll_x(&self, duration: Time) -> f32 {
        ((duration.as_secs_f64() * f64::from(self.px_per_second)) as f32).max(0.0)
    }

    /// 把横向滚动夹回 `[0, max_scroll_x]`（缩放、删片段、改面板大小以后）。
    pub fn clamp_scroll_x(&mut self, duration: Time) {
        self.scroll_x = self.scroll_x.clamp(0.0, self.max_scroll_x(duration));
    }

    /// 横向滚动条的滑块；不能滚时没有。
    #[must_use]
    pub fn h_scrollbar(&self, duration: Time, view_width: f32) -> Option<ScrollThumb> {
        let max = self.max_scroll_x(duration);
        if max <= 0.0 {
            return None;
        }
        let lane = self.lane_width(view_width);
        // 滑块占槽的比例 = 看得见的 / 能看的总长
        let width = (lane * (lane / (max + lane))).max(MIN_THUMB).min(lane);
        Some(ScrollThumb {
            x: self.header_width + (lane - width) * (self.scroll_x / max).clamp(0.0, 1.0),
            width,
        })
    }

    /// 滑块每拖 1 像素，内容滚多少像素。
    #[must_use]
    pub fn scroll_per_thumb_px(&self, duration: Time, view_width: f32) -> f32 {
        let max = self.max_scroll_x(duration);
        let lane = self.lane_width(view_width);
        self.h_scrollbar(duration, view_width)
            .map_or(0.0, |thumb| max / (lane - thumb.width).max(1.0))
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

/// 播放时要不要跟着播放头翻页。用户在播放中自己滚动、缩放把播放头挪出视图时不拉回去
/// （想往后看一眼），播放头自己走回视图后再接着跟；停下再播重新跟。
#[derive(Debug, Clone, Copy)]
pub struct PlayFollow {
    /// 跟着翻页。
    armed: bool,
    /// 上一帧处理完时的（横向滚动，缩放）：对不上说明用户自己滚过或缩放过。
    seen: Option<(f32, f32)>,
}

impl Default for PlayFollow {
    fn default() -> Self {
        Self {
            armed: true,
            seen: None,
        }
    }
}

impl PlayFollow {
    /// 播放中的一帧：播放头在 `t`，`forward` 是正放。翻页了返回 `true`。
    pub fn step(
        &mut self,
        geo: &mut TimelineGeometry,
        t: Time,
        view_width: f32,
        forward: bool,
    ) -> bool {
        let visible = geo.shows(t, view_width);
        let moved_by_user = self.seen.is_some_and(|(x, zoom)| {
            (x - geo.scroll_x).abs() > 0.5 || (zoom - geo.px_per_second).abs() > f32::EPSILON
        });
        if moved_by_user {
            self.armed = visible;
        } else if visible {
            self.armed = true;
        }
        let paged = self.armed && geo.follow(t, view_width, forward);
        self.seen = Some((geo.scroll_x, geo.px_per_second));
        paged
    }

    /// 停下了：下次播放重新跟。
    pub const fn stop(&mut self) {
        self.armed = true;
        self.seen = None;
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
    fn effect_only_and_text_tracks_are_shorter_rows() {
        let (mut p, _) = sample_project();
        ve_core::edit::add_adjustment(&mut p, Time::ZERO, Time::from_secs(2)).unwrap();
        let mut g = geo();
        g.sync_rows(&p.timeline);
        let tl = &p.timeline;
        let h = g.track_height;
        let heights: Vec<f32> = (0..tl.tracks.len()).map(|i| g.row_height(i)).collect();
        for (i, t) in tl.tracks.iter().enumerate() {
            let want = if t.kind == ve_core::TrackKind::Subtitle {
                h * TEXT_ROW
            } else if t.kind == ve_core::TrackKind::Video
                && t.clips.iter().all(|c| c.is_overlay_tool())
            {
                h * THIN_ROW
            } else {
                h
            };
            assert!(
                (heights[i] - want).abs() < 1e-3,
                "第 {i} 条 {:?}：{heights:?}",
                t.kind
            );
        }
        assert!(
            heights.iter().any(|&x| (x - h * THIN_ROW).abs() < 1e-3),
            "调节轨是细条"
        );
        // 行的位置、点中哪条、总高都按各自的高度算
        let mut top = g.ruler_height;
        for (i, &rh) in heights.iter().enumerate() {
            assert!((g.track_top(i) - top).abs() < 1e-3);
            assert_eq!(g.track_index_at(top + rh - 1.0, heights.len()), Some(i));
            top += rh;
        }
        assert!((g.content_height(heights.len()) - top).abs() < 1e-3);
        assert_eq!(g.insert_slot(g.track_top(1) + 1.0, heights.len()), Some(1));
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
        // 行：字幕 0、视频 1、音频 2。视频、音频两行中间，6–7 秒：v2（5–8）、a1（0–8）
        let mut got = g.clips_in_rect(&p.timeline, (x(6.0), row(&g, 1.5)), (x(7.0), row(&g, 2.5)));
        got.sort_unstable();
        assert_eq!(got, vec![ids.v2, ids.a1]);
        // 反着拖一样；只扫到字幕行 3.5–4 秒：s1 是 1–3，扫不到
        let none = g.clips_in_rect(&p.timeline, (x(4.0), row(&g, 0.9)), (x(3.5), row(&g, 0.1)));
        assert_eq!(none, Vec::<ClipId>::new());
        // 纵向滚动后按屏幕位置算：滚掉两行，屏幕第 0 行是音频轨
        g.scroll_y = g.track_height * 2.0;
        assert_eq!(
            g.clips_in_rect(&p.timeline, (x(1.0), row(&g, 0.2)), (x(2.0), row(&g, 0.8))),
            vec![ids.a1]
        );
    }

    #[test]
    fn hit_test_finds_clip_body_and_edges() {
        let (p, ids) = sample_project();
        let g = geo();
        let y_video = g.ruler_height + g.track_height * 1.5;
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
        let y_subs = g.ruler_height + g.track_height * 0.5;
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
        let y = g.ruler_height + g.track_height * 0.5;
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
    fn horizontal_scroll_stops_when_the_end_reaches_the_left_edge() {
        let mut g = geo(); // 100 像素 / 秒
        // 20 秒内容 2000 像素：最多滚到结尾贴着轨道头
        let secs = Time::from_secs;
        assert!((g.max_scroll_x(secs(20)) - 2000.0).abs() < 1e-3);
        g.scroll_by(9999.0);
        g.clamp_scroll_x(secs(20));
        assert!((g.scroll_x - 2000.0).abs() < 1e-3);
        // 内容短也能滚（放大时播放头才能留在原处）；空时间轴不能滚
        assert!((g.max_scroll_x(secs(3)) - 300.0).abs() < 1e-3);
        g.clamp_scroll_x(secs(3));
        assert!((g.scroll_x - 300.0).abs() < 1e-3);
        assert!(g.max_scroll_x(Time::ZERO).abs() < f32::EPSILON);
    }

    #[test]
    fn zoom_slider_is_logarithmic_and_fit_fills_ninety_percent() {
        assert!((px_for_zoom_level(0.0) - MIN_PX_PER_SECOND).abs() < 1e-3);
        assert!((px_for_zoom_level(100.0) - MAX_PX_PER_SECOND).abs() < 1e-1);
        // 中间是几何平均：2 × 1000^0.5 ≈ 63.2
        assert!((px_for_zoom_level(50.0) - 63.245).abs() < 1e-2);
        for level in [0.0, 13.0, 50.0, 99.0] {
            assert!((zoom_level(px_for_zoom_level(level)) - level).abs() < 1e-3);
        }
        assert!((zoom_level(1e9) - 100.0).abs() < 1e-3, "越界夹住");
        // 20 秒放进 1000 像素：900 / 20 = 45
        let fit = fit_px_per_second(Time::from_secs(20), 1000.0).unwrap();
        assert!((fit - 45.0).abs() < 1e-3);
        assert_eq!(fit_px_per_second(Time::ZERO, 1000.0), None);
        // 很短的也夹在最大缩放以内
        let short = fit_px_per_second(Time::from_millis(10), 1000.0).unwrap();
        assert!((short - MAX_PX_PER_SECOND).abs() < 1e-3);
    }

    #[test]
    fn scrollbar_thumb_tracks_the_visible_part() {
        let mut g = geo();
        let (secs, width) = (Time::from_secs, 1132.0);
        // 能滚 2000：总长 3000，滑块占 1000 / 3000
        let thumb = g.h_scrollbar(secs(20), width).unwrap();
        assert!((thumb.x - 132.0).abs() < 1e-3 && (thumb.width - 1000.0 / 3.0).abs() < 1e-2);
        g.scroll_x = 2000.0;
        let thumb = g.h_scrollbar(secs(20), width).unwrap();
        assert!(
            (thumb.x + thumb.width - 1132.0).abs() < 1e-2,
            "滚到头滑块贴右边：{thumb:?}"
        );
        // 滑块拖 1 像素 = 内容滚 2000 / (1000 − 333.3) = 3 像素
        assert!((g.scroll_per_thumb_px(secs(20), width) - 3.0).abs() < 1e-2);
        // 空时间轴不显示
        assert!(g.h_scrollbar(Time::ZERO, width).is_none());
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

    #[test]
    fn insert_slots_are_thin_bands_on_row_boundaries() {
        let g = TimelineGeometry::default();
        let boundary = |i: f32| g.ruler_height + g.track_height * i;
        // 第 1 条缝：上下各几像素都算
        assert_eq!(g.insert_slot(boundary(1.0) - 3.0, 3), Some(1));
        assert_eq!(g.insert_slot(boundary(1.0) + 3.0, 3), Some(1));
        // 行中间不是缝
        assert_eq!(g.insert_slot(boundary(1.5), 3), None);
        // 最下面一条轨下方的空白：插在最后
        assert_eq!(g.insert_slot(boundary(3.0) + 40.0, 3), Some(3));
        // 标尺上不算
        assert_eq!(g.insert_slot(g.ruler_height - 1.0, 3), None);
    }

    #[test]
    fn playback_pages_the_view_when_the_playhead_leaves_it() {
        // 视图 1132 宽：轨道头 132 + 片段区 1000（50 像素 / 秒，看得见 20 秒）
        let mut g = TimelineGeometry::default();
        let width = 1132.0;
        assert!(!g.follow(Time::from_secs(10), width, true), "看得见不动");
        assert!(g.follow(Time::from_secs(25), width, true));
        // 翻页后播放头在片段区一成处
        assert!((g.time_to_x(Time::from_secs(25)) - (132.0 + 100.0)).abs() < 0.5);
        // 倒放走出左边：翻到九成处
        assert!(g.follow(Time::from_secs(20), width, false));
        assert!((g.time_to_x(Time::from_secs(20)) - (132.0 + 900.0)).abs() < 0.5);
        // 已经滚到头了：开头那段再往左也不能滚
        g.scroll_x = 0.0;
        assert!(!g.follow(Time::ZERO, width, false));
    }

    #[test]
    fn dragging_near_an_edge_scrolls_faster_the_closer_it_gets() {
        let g = TimelineGeometry::default();
        let width = 1132.0;
        assert!(g.edge_scroll(600.0, width).abs() < f32::EPSILON, "中间不滚");
        let near = g.edge_scroll(width - 20.0, width);
        let at = g.edge_scroll(width - 1.0, width);
        let out = g.edge_scroll(width + 300.0, width);
        assert!(0.0 < near && near < at && at <= out, "{near} {at} {out}");
        assert!(g.edge_scroll(132.0 + 5.0, width) < 0.0, "左边往回滚");
        assert!(g.edge_scroll(0.0, width) < 0.0);
    }

    #[test]
    fn playback_follows_until_the_user_scrolls_away_and_loops_land_at_the_left() {
        let mut g = TimelineGeometry {
            px_per_second: 100.0,
            ..TimelineGeometry::default()
        };
        let width = 1000.0;
        let lane = width - g.header_width;
        let mut f = PlayFollow::default();
        let t = |s: f64| Time::from_secs_f64(s);
        // 正常播放：走出右边就翻页
        assert!(!f.step(&mut g, t(1.0), width, true));
        assert!(f.step(&mut g, t(9.0), width, true));
        assert!((g.time_to_x(t(9.0)) - (g.header_width + lane * 0.1)).abs() < 1.0);
        // 用户自己往后滚，播放头不在视图里了：不拉回
        g.scroll_by(3000.0);
        let away = g.scroll_x;
        assert!(!f.step(&mut g, t(9.1), width, true));
        assert!(!f.step(&mut g, t(9.2), width, true));
        assert!((g.scroll_x - away).abs() < f32::EPSILON, "没被拉回");
        // 播放头自己走进视图后恢复跟随
        let back = g.x_to_time(g.header_width + lane * 0.5);
        assert!(!f.step(&mut g, back, width, true));
        let later = g.x_to_time(width + 10.0);
        assert!(f.step(&mut g, later, width, true));
        // 循环播放跳回起点（在视图左边外面）：正放落在一成处，不是九成处
        assert!(f.step(&mut g, t(20.0), width, true));
        assert!((g.time_to_x(t(20.0)) - (g.header_width + lane * 0.1)).abs() < 1.0);
        // 停下再播：重新跟随
        f.stop();
        g.scroll_by(5000.0);
        assert!(f.step(&mut g, t(0.6), width, true));
    }
}
