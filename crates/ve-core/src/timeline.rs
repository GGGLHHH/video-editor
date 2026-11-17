//! 时间轴：轨道、片段，以及模型不变式检查。

use crate::{Asset, AssetId, ClipId, IdGen, Time, TrackId, Transform};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use thiserror::Error;

/// 轨道类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackKind {
    /// 视频（带素材原声）。
    Video,
    /// 音频。
    Audio,
    /// 字幕。
    Subtitle,
}

/// 片段内容。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipContent {
    /// 引用素材的一段，从 `source_in` 开始。
    Media {
        /// 素材。
        asset: AssetId,
        /// 素材内起点。
        source_in: Time,
    },
    /// 字幕文字。
    Subtitle {
        /// 文字，可含换行。
        text: String,
    },
}

/// 时间轴上的一个片段。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// id。
    pub id: ClipId,
    /// 时间轴上的起点。
    pub start: Time,
    /// 长度（> 0）。
    pub duration: Time,
    /// 内容。
    pub content: ClipContent,
    /// 音量增益（dB）。
    pub gain_db: f32,
    /// 淡入长度。
    pub fade_in: Time,
    /// 淡出长度。
    pub fade_out: Time,
    /// 画面属性（只对视频轨上的媒体片段起作用）。第 1 版工程文件里没有这个字段，取默认值。
    #[serde(default)]
    pub transform: Transform,
    /// 画面属性的关键帧动画（时间相对片段起点）；为空时不写进文件。第 4 版工程文件加的。
    #[serde(default, skip_serializing_if = "crate::Animation::is_empty")]
    pub animation: crate::Animation,
}

impl Clip {
    /// 片段内 `offset` 时刻（相对片段起点）的画面属性：有关键帧的属性按动画求值，其余用固定值。
    #[must_use]
    pub fn transform_at(&self, offset: Time) -> Transform {
        self.animation.apply(&self.transform, offset)
    }

    /// 时间轴上的终点（不含）。
    #[must_use]
    pub fn end(&self) -> Time {
        self.start + self.duration
    }

    /// 引用的素材（字幕返回 `None`）。
    #[must_use]
    pub const fn asset(&self) -> Option<AssetId> {
        match &self.content {
            ClipContent::Media { asset, .. } => Some(*asset),
            ClipContent::Subtitle { .. } => None,
        }
    }

    /// 素材内起点（字幕为 0）。
    #[must_use]
    pub const fn source_in(&self) -> Time {
        match &self.content {
            ClipContent::Media { source_in, .. } => *source_in,
            ClipContent::Subtitle { .. } => Time::ZERO,
        }
    }

    /// 字幕文字。
    #[must_use]
    pub fn subtitle_text(&self) -> Option<&str> {
        match &self.content {
            ClipContent::Subtitle { text } => Some(text),
            ClipContent::Media { .. } => None,
        }
    }
}

/// 一条轨道：片段按 `start` 升序、互不重叠。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// id。
    pub id: TrackId,
    /// 类型。
    pub kind: TrackKind,
    /// 片段。
    pub clips: Vec<Arc<Clip>>,
}

impl Track {
    /// 空轨道。
    #[must_use]
    pub const fn new(id: TrackId, kind: TrackKind) -> Self {
        Self {
            id,
            kind,
            clips: Vec::new(),
        }
    }

    /// 轨道终点（最后一个片段的终点）。
    #[must_use]
    pub fn end(&self) -> Time {
        self.clips.last().map_or(Time::ZERO, |c| c.end())
    }

    /// 片段下标。
    #[must_use]
    pub fn index_of(&self, clip: ClipId) -> Option<usize> {
        self.clips.iter().position(|c| c.id == clip)
    }

    /// 时间点 `t` 处的片段（`start <= t < end`）。
    #[must_use]
    pub fn clip_at(&self, t: Time) -> Option<&Arc<Clip>> {
        let idx = self.clips.partition_point(|c| c.start <= t);
        idx.checked_sub(1)
            .and_then(|i| self.clips.get(i))
            .filter(|c| t < c.end())
    }

    /// 区间 `[start, end)` 是否空闲（忽略 `ignore` 里的片段）。
    #[must_use]
    pub fn is_free(&self, start: Time, end: Time, ignore: &[ClipId]) -> bool {
        self.clips
            .iter()
            .filter(|c| !ignore.contains(&c.id))
            .all(|c| c.end() <= start || end <= c.start)
    }

    /// 插入并保持排序（不检查重叠）。
    pub fn insert_sorted(&mut self, clip: Arc<Clip>) {
        let idx = self.clips.partition_point(|c| c.start <= clip.start);
        self.clips.insert(idx, clip);
    }
}

/// 时间轴：若干轨道。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    /// 轨道，从上到下。
    pub tracks: Vec<Track>,
}

/// 模型不变式被破坏。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvariantError {
    /// 同轨片段重叠或未排序。
    #[error("片段 {clip:?} 与前一个片段重叠")]
    Overlap {
        /// 轨道。
        track: TrackId,
        /// 后一个片段。
        clip: ClipId,
    },
    /// 起点为负。
    #[error("片段 {clip:?} 起点为负")]
    NegativeStart {
        /// 片段。
        clip: ClipId,
    },
    /// 长度不为正。
    #[error("片段 {clip:?} 长度为零")]
    ZeroLength {
        /// 片段。
        clip: ClipId,
    },
    /// 超出素材范围。
    #[error("片段 {clip:?} 超出素材范围")]
    OutOfSource {
        /// 片段。
        clip: ClipId,
    },
    /// 引用的素材不存在。
    #[error("片段 {clip:?} 引用的素材 {asset:?} 不存在")]
    MissingAsset {
        /// 片段。
        clip: ClipId,
        /// 素材。
        asset: AssetId,
    },
    /// 片段内容与轨道类型不符。
    #[error("片段 {clip:?} 放错了轨道")]
    WrongTrackKind {
        /// 片段。
        clip: ClipId,
    },
    /// 淡入淡出为负或超过片段长度。
    #[error("片段 {clip:?} 淡入淡出无效")]
    BadFade {
        /// 片段。
        clip: ClipId,
    },
    /// 画面属性违反不变式。
    #[error("片段 {clip:?} 画面属性无效")]
    BadTransform {
        /// 片段。
        clip: ClipId,
    },
    /// 增益不是有限数。
    #[error("片段 {clip:?} 增益无效")]
    BadGain {
        /// 片段。
        clip: ClipId,
    },
}

impl Timeline {
    /// MVP 布局：视频、音频、字幕各一条。
    #[must_use]
    pub fn mvp(ids: &mut IdGen) -> Self {
        Self {
            tracks: vec![
                Track::new(ids.track(), TrackKind::Video),
                Track::new(ids.track(), TrackKind::Audio),
                Track::new(ids.track(), TrackKind::Subtitle),
            ],
        }
    }

    /// 视频轨，从最下层到最上层（合成顺序）。轨道列表的顺序就是界面上从上到下的顺序。
    pub fn video_tracks_bottom_up(&self) -> impl Iterator<Item = &Track> {
        self.tracks
            .iter()
            .rev()
            .filter(|t| t.kind == TrackKind::Video)
    }

    /// 按 id 找轨道。
    #[must_use]
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// 按 id 找轨道（可变）。
    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// 第一条该类型的轨道。
    #[must_use]
    pub fn track_of_kind(&self, kind: TrackKind) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == kind)
    }

    /// 第一条该类型的轨道（可变）。
    pub fn track_of_kind_mut(&mut self, kind: TrackKind) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.kind == kind)
    }

    /// 找片段及其所在轨道。
    #[must_use]
    pub fn find_clip(&self, id: ClipId) -> Option<(TrackId, &Arc<Clip>)> {
        self.tracks
            .iter()
            .find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t.id, c)))
    }

    /// 时间轴总长（所有轨道终点的最大值）。
    #[must_use]
    pub fn duration(&self) -> Time {
        self.tracks
            .iter()
            .map(Track::end)
            .max()
            .unwrap_or(Time::ZERO)
    }

    /// 检查所有不变式。
    ///
    /// # Errors
    /// 返回第一个被破坏的不变式。
    pub fn check_invariants(
        &self,
        assets: &BTreeMap<AssetId, Asset>,
    ) -> Result<(), InvariantError> {
        for track in &self.tracks {
            let mut prev_end: Option<Time> = None;
            for clip in &track.clips {
                check_clip(track.kind, clip, assets)?;
                if prev_end.is_some_and(|end| clip.start < end) {
                    return Err(InvariantError::Overlap {
                        track: track.id,
                        clip: clip.id,
                    });
                }
                prev_end = Some(clip.end());
            }
        }
        Ok(())
    }
}

fn check_clip(
    kind: TrackKind,
    clip: &Clip,
    assets: &BTreeMap<AssetId, Asset>,
) -> Result<(), InvariantError> {
    let id = clip.id;
    if clip.start < Time::ZERO {
        return Err(InvariantError::NegativeStart { clip: id });
    }
    if clip.duration <= Time::ZERO {
        return Err(InvariantError::ZeroLength { clip: id });
    }
    if !clip.gain_db.is_finite() {
        return Err(InvariantError::BadGain { clip: id });
    }
    if !clip.transform.is_valid() || !clip.animation.is_valid() {
        return Err(InvariantError::BadTransform { clip: id });
    }
    if clip.fade_in < Time::ZERO
        || clip.fade_out < Time::ZERO
        || clip.fade_in + clip.fade_out > clip.duration
    {
        return Err(InvariantError::BadFade { clip: id });
    }
    match (&clip.content, kind) {
        (ClipContent::Subtitle { .. }, TrackKind::Subtitle) => Ok(()),
        (ClipContent::Media { asset, source_in }, TrackKind::Video | TrackKind::Audio) => {
            let a = assets.get(asset).ok_or(InvariantError::MissingAsset {
                clip: id,
                asset: *asset,
            })?;
            let fits = match kind {
                TrackKind::Video => a.video.is_some(),
                _ => a.audio.is_some(),
            };
            if !fits {
                return Err(InvariantError::WrongTrackKind { clip: id });
            }
            if *source_in < Time::ZERO || *source_in + clip.duration > a.source_limit() {
                return Err(InvariantError::OutOfSource { clip: id });
            }
            Ok(())
        }
        _ => Err(InvariantError::WrongTrackKind { clip: id }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{media_clip, sample_project};
    use crate::{Project, Time, VideoInfo};
    use std::sync::Arc;

    #[test]
    fn mvp_timeline_has_three_tracks() {
        let project = Project::new();
        let kinds: Vec<TrackKind> = project.timeline.tracks.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [TrackKind::Video, TrackKind::Audio, TrackKind::Subtitle]
        );
        assert!(project.timeline.track_of_kind(TrackKind::Audio).is_some());
        assert_eq!(project.timeline.duration(), Time::ZERO);
        project.timeline.check_invariants(&project.assets).unwrap();
    }

    #[test]
    fn transform_at_follows_animation_and_invalid_animation_rejected() {
        let (mut project, ids) = sample_project();
        let tr = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        let mut c = (*tr.clips[0]).clone();
        assert_eq!(
            c.transform_at(Time::from_secs(1)),
            c.transform,
            "没动画：固定值"
        );
        c.animation.upsert(crate::Prop::Opacity, Time::ZERO, 0.0);
        c.animation
            .upsert(crate::Prop::Opacity, Time::from_secs(2), 1.0);
        c.animation.set_easing(Time::ZERO, crate::Easing::Linear);
        assert!((c.transform_at(Time::from_secs(1)).opacity - 0.5).abs() < 1e-6);
        tr.clips[0] = Arc::new(c.clone());
        project.timeline.check_invariants(&project.assets).unwrap();
        // 关键帧值不合法：不变式拦住
        c.animation.upsert(crate::Prop::Scale, Time::ZERO, 0.0);
        let video = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        video.clips[0] = Arc::new(c);
        assert_eq!(
            project.timeline.check_invariants(&project.assets),
            Err(InvariantError::BadTransform { clip: ids.v1 })
        );
    }

    #[test]
    fn find_clip_and_duration() {
        let (project, ids) = sample_project();
        let (track, clip) = project.timeline.find_clip(ids.v1).unwrap();
        assert_eq!(
            project.timeline.track(track).unwrap().kind,
            TrackKind::Video
        );
        assert_eq!(clip.start, Time::ZERO);
        assert_eq!(project.timeline.duration(), Time::from_secs(8));
        assert!(project.timeline.find_clip(ClipId(9999)).is_none());
        project.timeline.check_invariants(&project.assets).unwrap();
    }

    #[test]
    fn invariants_reject_overlap() {
        let (mut project, ids) = sample_project();
        let v = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        let extra = media_clip(
            ClipId(500),
            ids.video_asset,
            Time::from_secs(1),
            Time::from_secs(2),
        );
        v.clips.push(Arc::new(extra));
        v.clips.sort_by_key(|c| c.start);
        let err = project
            .timeline
            .check_invariants(&project.assets)
            .unwrap_err();
        assert!(matches!(err, InvariantError::Overlap { .. }), "{err:?}");
    }

    #[test]
    fn invariants_reject_unsorted() {
        let (mut project, _) = sample_project();
        let v = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        v.clips.swap(0, 1);
        let err = project
            .timeline
            .check_invariants(&project.assets)
            .unwrap_err();
        assert!(matches!(err, InvariantError::Overlap { .. }), "{err:?}");
    }

    #[test]
    fn invariants_reject_source_out_of_bounds() {
        let (mut project, ids) = sample_project();
        let v = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        let mut clip = (*v.clips[0]).clone();
        clip.content = ClipContent::Media {
            asset: ids.video_asset,
            source_in: Time::from_secs(9),
        };
        v.clips[0] = Arc::new(clip);
        let err = project
            .timeline
            .check_invariants(&project.assets)
            .unwrap_err();
        assert!(matches!(err, InvariantError::OutOfSource { .. }), "{err:?}");
    }

    /// 改示例工程里某轨第一个片段后检查不变式。
    fn check_after(kind: TrackKind, edit: impl FnOnce(&mut Clip)) -> Result<(), InvariantError> {
        let (mut project, _) = sample_project();
        let track = project.timeline.track_of_kind_mut(kind).unwrap();
        let mut clip = (*track.clips[0]).clone();
        edit(&mut clip);
        track.clips[0] = Arc::new(clip);
        project.timeline.check_invariants(&project.assets)
    }

    #[test]
    fn invariants_reject_bad_fade() {
        let err = check_after(TrackKind::Video, |c| {
            c.fade_in = Time::from_secs(3);
            c.fade_out = Time::from_secs(3);
        });
        assert!(
            matches!(err, Err(InvariantError::BadFade { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn invariants_reject_zero_length() {
        let err = check_after(TrackKind::Audio, |c| c.duration = Time::ZERO);
        assert!(
            matches!(err, Err(InvariantError::ZeroLength { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn invariants_reject_negative_start() {
        let err = check_after(TrackKind::Audio, |c| c.start = Time::from_secs(-1));
        assert!(
            matches!(err, Err(InvariantError::NegativeStart { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn invariants_reject_nan_gain() {
        let err = check_after(TrackKind::Audio, |c| c.gain_db = f32::NAN);
        assert!(
            matches!(err, Err(InvariantError::BadGain { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn invariants_reject_media_on_subtitle_track() {
        let (mut project, ids) = sample_project();
        let s = project
            .timeline
            .track_of_kind_mut(TrackKind::Subtitle)
            .unwrap();
        let clip = media_clip(ClipId(501), ids.video_asset, Time::ZERO, Time::from_secs(1));
        s.clips.insert(0, Arc::new(clip));
        let err = project.timeline.check_invariants(&project.assets);
        assert!(
            matches!(err, Err(InvariantError::WrongTrackKind { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn invariants_reject_missing_asset() {
        let (mut project, ids) = sample_project();
        project.assets.remove(&ids.audio_asset);
        let err = project.timeline.check_invariants(&project.assets);
        assert!(
            matches!(err, Err(InvariantError::MissingAsset { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn still_image_has_no_source_limit() {
        let (mut project, ids) = sample_project();
        let asset = project.assets.get_mut(&ids.video_asset).unwrap();
        asset.video = asset.video.map(|v| VideoInfo { still: true, ..v });
        let track = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        let mut clip = (*track.clips[1]).clone();
        clip.duration = Time::from_secs(3600);
        track.clips[1] = Arc::new(clip);
        project.timeline.check_invariants(&project.assets).unwrap();
    }

    #[test]
    fn track_queries() {
        let (project, ids) = sample_project();
        let v = project.timeline.track(ids.video_track).unwrap();
        assert_eq!(v.clip_at(Time::from_secs(5)).map(|c| c.id), Some(ids.v2));
        assert_eq!(v.clip_at(Time::from_secs(8)).map(|c| c.id), None);
        assert!(v.is_free(Time::from_secs(8), Time::from_secs(9), &[]));
        assert!(!v.is_free(Time::from_secs(4), Time::from_secs(6), &[ids.v1]));
        assert!(v.is_free(Time::from_secs(4), Time::from_secs(5), &[ids.v1]));
        assert_eq!(v.index_of(ids.v2), Some(1));
        let s = &project.timeline.find_clip(ids.s1).unwrap().1;
        assert_eq!(s.subtitle_text(), Some("你好"));
        assert_eq!(s.asset(), None);
        assert_eq!(s.source_in(), Time::ZERO);
    }

    #[test]
    fn audio_only_asset_rejected_on_video_track() {
        let (mut project, ids) = sample_project();
        let v = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        let mut clip = (*v.clips[0]).clone();
        clip.content = ClipContent::Media {
            asset: ids.audio_asset,
            source_in: Time::ZERO,
        };
        v.clips[0] = Arc::new(clip);
        assert!(matches!(
            project.timeline.check_invariants(&project.assets),
            Err(InvariantError::WrongTrackKind { .. })
        ));
    }
}
