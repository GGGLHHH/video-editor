//! 编辑操作。全部是 `fn(&mut Project, ...) -> Result<_, EditError>`：失败时工程保持不变。
//!
//! 实现方式：在时间轴副本上改（片段是 `Arc`，复制只是引用计数），成功才写回。

use crate::{AssetId, Clip, ClipContent, ClipId, Project, Time, Timeline, TrackId, TrackKind};
use std::sync::Arc;
use thiserror::Error;

/// 编辑失败的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EditError {
    /// 片段不存在。
    #[error("片段不存在")]
    ClipNotFound(ClipId),
    /// 轨道不存在。
    #[error("轨道不存在")]
    TrackNotFound(TrackId),
    /// 素材不存在。
    #[error("素材不存在")]
    AssetNotFound(AssetId),
    /// 位置被别的片段占了。
    #[error("这个位置已有片段")]
    Overlap,
    /// 片段内容和轨道类型不匹配。
    #[error("不能放到这条轨道上")]
    WrongTrackKind,
    /// 会产生长度为零的片段。
    #[error("片段长度不能为零")]
    ZeroLength,
    /// 数值无效（如 NaN）。
    #[error("数值无效")]
    InvalidValue,
}

/// 增益范围（dB）。
pub const GAIN_RANGE_DB: (f32, f32) = (-60.0, 12.0);

/// 在时间轴副本上执行 `f`，成功后写回。
fn with_timeline<T>(
    project: &mut Project,
    f: impl FnOnce(&Project, &mut Timeline) -> Result<T, EditError>,
) -> Result<T, EditError> {
    let mut timeline = project.timeline.clone();
    let out = f(project, &mut timeline)?;
    project.timeline = timeline;
    Ok(out)
}

/// 片段位置：（轨道下标，片段下标）。
fn locate(timeline: &Timeline, clip: ClipId) -> Result<(usize, usize), EditError> {
    timeline
        .tracks
        .iter()
        .enumerate()
        .find_map(|(ti, t)| t.index_of(clip).map(|ci| (ti, ci)))
        .ok_or(EditError::ClipNotFound(clip))
}

fn track_index(timeline: &Timeline, track: TrackId) -> Result<usize, EditError> {
    timeline
        .tracks
        .iter()
        .position(|t| t.id == track)
        .ok_or(EditError::TrackNotFound(track))
}

/// 内容能否放到该类型轨道上。
fn fits(project: &Project, content: &ClipContent, kind: TrackKind) -> bool {
    match content {
        ClipContent::Subtitle { .. } => kind == TrackKind::Subtitle,
        ClipContent::Media { asset, .. } => project.assets.get(asset).is_some_and(|a| match kind {
            TrackKind::Video => a.video.is_some(),
            TrackKind::Audio => a.audio.is_some(),
            TrackKind::Subtitle => false,
        }),
    }
}

/// 最短片段：一帧。
fn min_len(project: &Project) -> Time {
    project.settings.frame_rate.frame_duration().max(Time(1))
}

/// 片段在素材里最多能延伸到的时间轴终点。
fn source_end_limit(project: &Project, clip: &Clip) -> Time {
    match &clip.content {
        ClipContent::Media { asset, source_in } => project.assets.get(asset).map_or_else(
            || clip.end(),
            |a| clip.start + (a.source_limit() - *source_in),
        ),
        ClipContent::Subtitle { .. } => Time(i64::MAX / 4),
    }
}

/// 淡入淡出夹到片段长度内（淡入优先）。
fn clamp_fades(clip: &mut Clip) {
    clip.fade_in = clip.fade_in.clamp(Time::ZERO, clip.duration);
    clip.fade_out = clip
        .fade_out
        .clamp(Time::ZERO, clip.duration - clip.fade_in);
}

fn new_media_clip(
    project: &Project,
    id: ClipId,
    asset: AssetId,
    start: Time,
) -> Result<Clip, EditError> {
    let a = project
        .assets
        .get(&asset)
        .ok_or(EditError::AssetNotFound(asset))?;
    if a.duration <= Time::ZERO {
        return Err(EditError::ZeroLength);
    }
    Ok(Clip {
        id,
        start,
        duration: a.duration,
        content: ClipContent::Media {
            asset,
            source_in: Time::ZERO,
        },
        gain_db: 0.0,
        fade_in: Time::ZERO,
        fade_out: Time::ZERO,
    })
}

/// 把素材追加到对应轨道末尾：有视频进视频轨，纯音频进音频轨。
///
/// # Errors
/// 素材不存在，或素材既无视频也无音频。
pub fn append_asset(project: &mut Project, asset: AssetId) -> Result<ClipId, EditError> {
    let a = project
        .assets
        .get(&asset)
        .ok_or(EditError::AssetNotFound(asset))?;
    let kind = if a.video.is_some() {
        TrackKind::Video
    } else if a.audio.is_some() {
        TrackKind::Audio
    } else {
        return Err(EditError::WrongTrackKind);
    };
    let track = project
        .timeline
        .track_of_kind(kind)
        .ok_or(EditError::WrongTrackKind)?;
    let (track_id, start) = (track.id, track.end());
    insert_asset_at(project, asset, track_id, start)
}

/// 把素材放到指定轨道的指定位置。
///
/// # Errors
/// 素材 / 轨道不存在、类型不符、与已有片段重叠。
pub fn insert_asset_at(
    project: &mut Project,
    asset: AssetId,
    track: TrackId,
    at: Time,
) -> Result<ClipId, EditError> {
    let ti = track_index(&project.timeline, track)?;
    let mut ids = project.ids.clone();
    let id = ids.clip();
    let clip = new_media_clip(project, id, asset, at.max(Time::ZERO))?;
    with_timeline(project, |p, tl| {
        let t = &mut tl.tracks[ti];
        if !fits(p, &clip.content, t.kind) {
            return Err(EditError::WrongTrackKind);
        }
        if !t.is_free(clip.start, clip.end(), &[]) {
            return Err(EditError::Overlap);
        }
        t.insert_sorted(Arc::new(clip));
        Ok(())
    })?;
    project.ids = ids;
    Ok(id)
}

/// 移动片段（可换轨），起点夹到 ≥ 0。
///
/// # Errors
/// 片段 / 轨道不存在、类型不符、与已有片段重叠。
pub fn move_clip(
    project: &mut Project,
    clip: ClipId,
    to_track: TrackId,
    new_start: Time,
) -> Result<(), EditError> {
    with_timeline(project, |p, tl| {
        let (ti, ci) = locate(tl, clip)?;
        let to = track_index(tl, to_track)?;
        let mut c = (*tl.tracks[ti].clips[ci]).clone();
        if !fits(p, &c.content, tl.tracks[to].kind) {
            return Err(EditError::WrongTrackKind);
        }
        c.start = new_start.max(Time::ZERO);
        if !tl.tracks[to].is_free(c.start, c.end(), &[clip]) {
            return Err(EditError::Overlap);
        }
        tl.tracks[ti].clips.remove(ci);
        tl.tracks[to].insert_sorted(Arc::new(c));
        Ok(())
    })
}

/// 修剪起点（终点不动），夹在前一个片段终点、素材开头和「至少一帧」之间。
///
/// # Errors
/// 片段不存在。
pub fn trim_start(project: &mut Project, clip: ClipId, new_start: Time) -> Result<(), EditError> {
    let min = min_len(project);
    with_timeline(project, |_, tl| {
        let (ti, ci) = locate(tl, clip)?;
        let track = &tl.tracks[ti];
        let mut c = (*track.clips[ci]).clone();
        let prev_end = ci
            .checked_sub(1)
            .map_or(Time::ZERO, |i| track.clips[i].end());
        let source_floor = match &c.content {
            ClipContent::Media { source_in, .. } => c.start - *source_in,
            ClipContent::Subtitle { .. } => Time::ZERO,
        };
        let end = c.end();
        // 邻居和素材边界优先于「至少一帧」：片段本身不足一帧时不能因此压到前一个片段
        let lower = prev_end.max(source_floor).max(Time::ZERO);
        let upper = if end - min >= lower {
            end - min
        } else {
            c.start
        };
        let start = new_start.clamp(lower, upper);
        if let ClipContent::Media { source_in, .. } = &mut c.content {
            *source_in += start - c.start;
        }
        c.start = start;
        c.duration = end - start;
        clamp_fades(&mut c);
        tl.tracks[ti].clips[ci] = Arc::new(c);
        Ok(())
    })
}

/// 修剪终点（起点不动），夹在下一个片段起点、素材结尾和「至少一帧」之间。
///
/// # Errors
/// 片段不存在。
pub fn trim_end(project: &mut Project, clip: ClipId, new_end: Time) -> Result<(), EditError> {
    let min = min_len(project);
    with_timeline(project, |p, tl| {
        let (ti, ci) = locate(tl, clip)?;
        let track = &tl.tracks[ti];
        let mut c = (*track.clips[ci]).clone();
        let next_start = track
            .clips
            .get(ci + 1)
            .map_or(Time(i64::MAX / 4), |n| n.start);
        let upper = next_start.min(source_end_limit(p, &c));
        let lower = if c.start + min <= upper {
            c.start + min
        } else {
            c.end()
        };
        let end = new_end.clamp(lower, upper);
        c.duration = end - c.start;
        clamp_fades(&mut c);
        tl.tracks[ti].clips[ci] = Arc::new(c);
        Ok(())
    })
}

/// 在 `at` 处把片段切成两段，返回右半段的 id。
///
/// # Errors
/// 片段不存在；`at` 不在片段内部（含恰在边界）时返回 [`EditError::ZeroLength`]。
pub fn split_at(project: &mut Project, clip: ClipId, at: Time) -> Result<ClipId, EditError> {
    let mut ids = project.ids.clone();
    let right_id = ids.clip();
    with_timeline(project, |_, tl| split_in(tl, clip, at, right_id))?;
    project.ids = ids;
    Ok(right_id)
}

fn split_in(
    tl: &mut Timeline,
    clip: ClipId,
    at: Time,
    right_id: ClipId,
) -> Result<ClipId, EditError> {
    let (ti, ci) = locate(tl, clip)?;
    let original = &tl.tracks[ti].clips[ci];
    if at <= original.start || at >= original.end() {
        return Err(EditError::ZeroLength);
    }
    let mut left = (**original).clone();
    let mut right = (**original).clone();
    left.duration = at - left.start;
    left.fade_out = Time::ZERO;
    clamp_fades(&mut left);
    right.id = right_id;
    right.start = at;
    right.duration = original.end() - at;
    right.fade_in = Time::ZERO;
    if let ClipContent::Media { source_in, .. } = &mut right.content {
        *source_in += left.duration;
    }
    clamp_fades(&mut right);
    let clips = &mut tl.tracks[ti].clips;
    clips[ci] = Arc::new(left);
    clips.insert(ci + 1, Arc::new(right));
    Ok(right_id)
}

/// 在 `at` 处切开所有轨道上跨过它的片段，返回新片段 id。
pub fn split_all_at(project: &mut Project, at: Time) -> Vec<ClipId> {
    #[expect(
        clippy::needless_collect,
        reason = "split_at 要可变借用 project，必须先收集"
    )]
    let targets: Vec<ClipId> = project
        .timeline
        .tracks
        .iter()
        .filter_map(|t| t.clip_at(at))
        .filter(|c| c.start < at)
        .map(|c| c.id)
        .collect();
    targets
        .into_iter()
        .filter_map(|c| split_at(project, c, at).ok())
        .collect()
}

/// 删除片段（全部存在才删）。
///
/// # Errors
/// 任一片段不存在。
pub fn delete_clips(project: &mut Project, clips: &[ClipId]) -> Result<(), EditError> {
    with_timeline(project, |_, tl| {
        for &c in clips {
            locate(tl, c)?;
        }
        for t in &mut tl.tracks {
            t.clips.retain(|c| !clips.contains(&c.id));
        }
        Ok(())
    })
}

/// 波纹删除：删掉片段，同轨后面的片段左移补上被删的长度。
///
/// # Errors
/// 任一片段不存在。
pub fn ripple_delete(project: &mut Project, clips: &[ClipId]) -> Result<(), EditError> {
    with_timeline(project, |_, tl| {
        for &c in clips {
            locate(tl, c)?;
        }
        for t in &mut tl.tracks {
            let mut shift = Time::ZERO;
            let mut kept = Vec::with_capacity(t.clips.len());
            for c in t.clips.drain(..) {
                if clips.contains(&c.id) {
                    shift += c.duration;
                } else if shift == Time::ZERO {
                    kept.push(c);
                } else {
                    let mut moved = (*c).clone();
                    moved.start -= shift;
                    kept.push(Arc::new(moved));
                }
            }
            t.clips = kept;
        }
        Ok(())
    })
}

/// 修改单个片段。
fn update_clip(
    project: &mut Project,
    clip: ClipId,
    f: impl FnOnce(&mut Clip) -> Result<(), EditError>,
) -> Result<(), EditError> {
    with_timeline(project, |_, tl| {
        let (ti, ci) = locate(tl, clip)?;
        let mut c = (*tl.tracks[ti].clips[ci]).clone();
        f(&mut c)?;
        tl.tracks[ti].clips[ci] = Arc::new(c);
        Ok(())
    })
}

/// 设置增益（dB），夹到 [`GAIN_RANGE_DB`]。
///
/// # Errors
/// 片段不存在或数值不是有限数。
pub fn set_gain(project: &mut Project, clip: ClipId, gain_db: f32) -> Result<(), EditError> {
    if !gain_db.is_finite() {
        return Err(EditError::InvalidValue);
    }
    update_clip(project, clip, |c| {
        c.gain_db = gain_db.clamp(GAIN_RANGE_DB.0, GAIN_RANGE_DB.1);
        Ok(())
    })
}

/// 设置淡入淡出，夹到 ≥ 0 且两者之和不超过片段长度（淡入优先）。
///
/// # Errors
/// 片段不存在。
pub fn set_fades(
    project: &mut Project,
    clip: ClipId,
    fade_in: Time,
    fade_out: Time,
) -> Result<(), EditError> {
    update_clip(project, clip, |c| {
        c.fade_in = fade_in;
        c.fade_out = fade_out;
        clamp_fades(c);
        Ok(())
    })
}

/// 在字幕轨加一条字幕。
///
/// # Errors
/// 长度不为正、与已有字幕重叠、没有字幕轨。
pub fn add_subtitle(
    project: &mut Project,
    at: Time,
    duration: Time,
    text: String,
) -> Result<ClipId, EditError> {
    if duration <= Time::ZERO {
        return Err(EditError::ZeroLength);
    }
    let ti = project
        .timeline
        .tracks
        .iter()
        .position(|t| t.kind == TrackKind::Subtitle)
        .ok_or(EditError::WrongTrackKind)?;
    let start = at.max(Time::ZERO);
    if !project.timeline.tracks[ti].is_free(start, start + duration, &[]) {
        return Err(EditError::Overlap);
    }
    let id = project.ids.clip();
    let clip = Clip {
        id,
        start,
        duration,
        content: ClipContent::Subtitle { text },
        gain_db: 0.0,
        fade_in: Time::ZERO,
        fade_out: Time::ZERO,
    };
    project.timeline.tracks[ti].insert_sorted(Arc::new(clip));
    Ok(id)
}

/// 改字幕文字。
///
/// # Errors
/// 片段不存在或不是字幕。
pub fn set_subtitle_text(
    project: &mut Project,
    clip: ClipId,
    text: String,
) -> Result<(), EditError> {
    update_clip(project, clip, |c| match &mut c.content {
        ClipContent::Subtitle { text: t } => {
            *t = text;
            Ok(())
        }
        ClipContent::Media { .. } => Err(EditError::WrongTrackKind),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{SampleIds, sample_project};
    use crate::{Asset, AssetId, VideoInfo};
    use proptest::prelude::*;

    fn secs(s: i64) -> Time {
        Time::from_secs(s)
    }

    fn clip(project: &Project, id: ClipId) -> Clip {
        (**project.timeline.find_clip(id).unwrap().1).clone()
    }

    fn assert_valid(project: &Project) {
        project.timeline.check_invariants(&project.assets).unwrap();
    }

    #[test]
    fn append_asset_goes_to_matching_track_end() {
        let (mut p, ids) = sample_project();
        let v = append_asset(&mut p, ids.video_asset).unwrap();
        let (track, c) = p.timeline.find_clip(v).unwrap();
        assert_eq!(track, ids.video_track);
        assert_eq!((c.start, c.duration), (secs(8), secs(10)));
        let a = append_asset(&mut p, ids.audio_asset).unwrap();
        assert_eq!(p.timeline.find_clip(a).unwrap().0, ids.audio_track);
        assert_eq!(
            append_asset(&mut p, AssetId(999)),
            Err(EditError::AssetNotFound(AssetId(999)))
        );
        assert_valid(&p);
    }

    #[test]
    fn zero_duration_asset_rejected() {
        let (mut p, ids) = sample_project();
        let empty = p.add_asset(Asset {
            id: AssetId(0),
            path: "raw.h264".into(),
            name: "raw.h264".into(),
            duration: Time::ZERO,
            video: Some(crate::testing::video_info()),
            audio: None,
        });
        let before = p.clone();
        assert_eq!(append_asset(&mut p, empty), Err(EditError::ZeroLength));
        assert_eq!(
            insert_asset_at(&mut p, empty, ids.video_track, secs(30)),
            Err(EditError::ZeroLength)
        );
        assert_eq!(p, before);
    }

    #[test]
    fn append_asset_without_streams_rejected() {
        let (mut p, _) = sample_project();
        let empty = p.add_asset(Asset {
            id: AssetId(0),
            path: "x".into(),
            name: "x".into(),
            duration: secs(1),
            video: None,
            audio: None,
        });
        assert_eq!(append_asset(&mut p, empty), Err(EditError::WrongTrackKind));
    }

    #[test]
    fn insert_asset_at_rejects_overlap_and_wrong_kind() {
        let (mut p, ids) = sample_project();
        let before = p.clone();
        assert_eq!(
            insert_asset_at(&mut p, ids.video_asset, ids.video_track, secs(3)),
            Err(EditError::Overlap)
        );
        assert_eq!(
            insert_asset_at(&mut p, ids.audio_asset, ids.video_track, secs(20)),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(
            insert_asset_at(&mut p, ids.video_asset, ids.subtitle_track, secs(20)),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(p, before);
        let id = insert_asset_at(&mut p, ids.video_asset, ids.audio_track, secs(9)).unwrap();
        assert_eq!(clip(&p, id).start, secs(9));
        assert_valid(&p);
    }

    #[test]
    fn move_clip_within_and_across_tracks() {
        let (mut p, ids) = sample_project();
        move_clip(&mut p, ids.v2, ids.video_track, secs(20)).unwrap();
        assert_eq!(clip(&p, ids.v2).start, secs(20));
        move_clip(&mut p, ids.v1, ids.video_track, secs(-5)).unwrap();
        assert_eq!(clip(&p, ids.v1).start, Time::ZERO);
        // 视频素材带音频，可以放到音频轨
        move_clip(&mut p, ids.v2, ids.audio_track, secs(30)).unwrap();
        assert_eq!(p.timeline.find_clip(ids.v2).unwrap().0, ids.audio_track);
        assert_valid(&p);
    }

    #[test]
    fn move_into_overlap_rejected_and_project_unchanged() {
        let (mut p, ids) = sample_project();
        let before = p.clone();
        assert_eq!(
            move_clip(&mut p, ids.v2, ids.video_track, secs(4)),
            Err(EditError::Overlap)
        );
        assert_eq!(
            move_clip(&mut p, ids.s1, ids.video_track, secs(40)),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(
            move_clip(&mut p, ids.a1, ids.video_track, secs(40)),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(
            move_clip(&mut p, ClipId(999), ids.video_track, secs(40)),
            Err(EditError::ClipNotFound(ClipId(999)))
        );
        assert_eq!(
            move_clip(&mut p, ids.v1, TrackId(999), secs(40)),
            Err(EditError::TrackNotFound(TrackId(999)))
        );
        assert_eq!(p, before);
    }

    #[test]
    fn trim_start_moves_source_and_keeps_end() {
        let (mut p, ids) = sample_project();
        trim_start(&mut p, ids.v2, secs(6)).unwrap();
        let mut c = clip(&p, ids.v2);
        assert_eq!(
            (c.start, c.end(), c.source_in()),
            (secs(6), secs(8), secs(6))
        );
        // 不能越过前一个片段，也不能早于素材开头
        trim_start(&mut p, ids.v2, secs(1)).unwrap();
        c = clip(&p, ids.v2);
        assert_eq!((c.start, c.source_in()), (secs(5), secs(5)));
        trim_start(&mut p, ids.v1, secs(-3)).unwrap();
        assert_eq!(clip(&p, ids.v1).start, Time::ZERO);
        // 至少留一帧
        trim_start(&mut p, ids.v2, secs(100)).unwrap();
        c = clip(&p, ids.v2);
        assert_eq!(c.duration, p.settings.frame_rate.frame_duration());
        assert_valid(&p);
    }

    #[test]
    fn trim_end_clamped_to_source() {
        let (mut p, ids) = sample_project();
        trim_end(&mut p, ids.v2, secs(100)).unwrap();
        // 素材 10 秒，片段从素材第 5 秒开始，所以最多到时间轴 10 秒
        assert_eq!(clip(&p, ids.v2).end(), secs(10));
        trim_end(&mut p, ids.v1, secs(9)).unwrap();
        // 不能盖住下一个片段
        assert_eq!(clip(&p, ids.v1).end(), secs(5));
        trim_end(&mut p, ids.v1, secs(2)).unwrap();
        assert_eq!(clip(&p, ids.v1).end(), secs(2));
        trim_end(&mut p, ids.s1, Time::ZERO).unwrap();
        assert_eq!(
            clip(&p, ids.s1).duration,
            p.settings.frame_rate.frame_duration()
        );
        assert_valid(&p);
    }

    #[test]
    fn trim_shrinks_fades() {
        let (mut p, ids) = sample_project();
        set_fades(&mut p, ids.v1, secs(2), secs(2)).unwrap();
        trim_end(&mut p, ids.v1, secs(3)).unwrap();
        let c = clip(&p, ids.v1);
        assert!(c.fade_in + c.fade_out <= c.duration);
        assert_valid(&p);
    }

    #[test]
    fn split_creates_two_continuous_parts() {
        let (mut p, ids) = sample_project();
        set_fades(&mut p, ids.v1, secs(1), secs(1)).unwrap();
        let right = split_at(&mut p, ids.v1, secs(2)).unwrap();
        let l = clip(&p, ids.v1);
        let r = clip(&p, right);
        assert_eq!(
            (l.start, l.end(), l.fade_in, l.fade_out),
            (Time::ZERO, secs(2), secs(1), Time::ZERO)
        );
        assert_eq!(
            (r.start, r.end(), r.source_in()),
            (secs(2), secs(5), secs(2))
        );
        assert_eq!((r.fade_in, r.fade_out), (Time::ZERO, secs(1)));
        assert_valid(&p);
    }

    #[test]
    fn split_at_clip_edge_is_noop() {
        let (mut p, ids) = sample_project();
        let before = p.clone();
        assert_eq!(
            split_at(&mut p, ids.v1, Time::ZERO),
            Err(EditError::ZeroLength)
        );
        assert_eq!(
            split_at(&mut p, ids.v1, secs(5)),
            Err(EditError::ZeroLength)
        );
        assert_eq!(
            split_at(&mut p, ids.v1, secs(7)),
            Err(EditError::ZeroLength)
        );
        assert_eq!(p, before);
    }

    #[test]
    fn split_all_at_splits_every_track() {
        let (mut p, ids) = sample_project();
        let new = split_all_at(&mut p, secs(2));
        // v1、a1、s1 都跨过 2 秒
        assert_eq!(new.len(), 3);
        assert_eq!(clip(&p, ids.s1).end(), secs(2));
        assert_eq!(split_all_at(&mut p, secs(2)), []);
        assert_valid(&p);
    }

    #[test]
    fn delete_clips_all_or_nothing() {
        let (mut p, ids) = sample_project();
        let before = p.clone();
        assert_eq!(
            delete_clips(&mut p, &[ids.v1, ClipId(999)]),
            Err(EditError::ClipNotFound(ClipId(999)))
        );
        assert_eq!(p, before);
        delete_clips(&mut p, &[ids.v1, ids.s1]).unwrap();
        assert!(p.timeline.find_clip(ids.v1).is_none());
        assert_eq!(clip(&p, ids.v2).start, secs(5));
        assert_valid(&p);
    }

    #[test]
    fn ripple_delete_shifts_following() {
        let (mut p, ids) = sample_project();
        let v3 = append_asset(&mut p, ids.video_asset).unwrap();
        ripple_delete(&mut p, &[ids.v1]).unwrap();
        assert_eq!(clip(&p, ids.v2).start, Time::ZERO);
        assert_eq!(clip(&p, v3).start, secs(3));
        // 其他轨道不动
        assert_eq!(clip(&p, ids.a1).start, Time::ZERO);
        assert_eq!(clip(&p, ids.s1).start, secs(1));
        ripple_delete(&mut p, &[ids.v2, v3]).unwrap();
        assert_eq!(p.timeline.track(ids.video_track).unwrap().clips, []);
        assert_eq!(
            ripple_delete(&mut p, &[ClipId(999)]),
            Err(EditError::ClipNotFound(ClipId(999)))
        );
        assert_valid(&p);
    }

    #[test]
    fn gain_clamped_and_nan_rejected() {
        let (mut p, ids) = sample_project();
        set_gain(&mut p, ids.a1, 40.0).unwrap();
        assert!((clip(&p, ids.a1).gain_db - 12.0).abs() < f32::EPSILON);
        set_gain(&mut p, ids.a1, -100.0).unwrap();
        assert!((clip(&p, ids.a1).gain_db + 60.0).abs() < f32::EPSILON);
        assert_eq!(
            set_gain(&mut p, ids.a1, f32::NAN),
            Err(EditError::InvalidValue)
        );
    }

    #[test]
    fn fades_clamped_to_length() {
        let (mut p, ids) = sample_project();
        set_fades(&mut p, ids.v2, secs(2), secs(5)).unwrap();
        let mut c = clip(&p, ids.v2);
        assert_eq!((c.fade_in, c.fade_out), (secs(2), secs(1)));
        set_fades(&mut p, ids.v2, secs(-1), secs(-1)).unwrap();
        c = clip(&p, ids.v2);
        assert_eq!((c.fade_in, c.fade_out), (Time::ZERO, Time::ZERO));
        assert_valid(&p);
    }

    #[test]
    fn subtitles_add_and_edit() {
        let (mut p, ids) = sample_project();
        let s = add_subtitle(&mut p, secs(4), secs(2), "第二句".into()).unwrap();
        assert_eq!(clip(&p, s).subtitle_text(), Some("第二句"));
        assert_eq!(
            add_subtitle(&mut p, secs(2), secs(1), "x".into()),
            Err(EditError::Overlap)
        );
        assert_eq!(
            add_subtitle(&mut p, secs(9), Time::ZERO, "x".into()),
            Err(EditError::ZeroLength)
        );
        set_subtitle_text(&mut p, ids.s1, "改了".into()).unwrap();
        assert_eq!(clip(&p, ids.s1).subtitle_text(), Some("改了"));
        assert_eq!(
            set_subtitle_text(&mut p, ids.v1, "x".into()),
            Err(EditError::WrongTrackKind)
        );
        assert_valid(&p);
    }

    #[test]
    fn still_image_appends_with_default_length() {
        let (mut p, _) = sample_project();
        let img = p.add_asset(Asset {
            id: AssetId(0),
            path: "a.png".into(),
            name: "a.png".into(),
            duration: secs(5),
            video: Some(VideoInfo {
                still: true,
                ..crate::testing::video_info()
            }),
            audio: None,
        });
        let c = append_asset(&mut p, img).unwrap();
        assert_eq!(clip(&p, c).duration, secs(5));
        trim_end(&mut p, c, secs(60)).unwrap();
        assert_eq!(clip(&p, c).end(), secs(60));
        assert_valid(&p);
    }

    #[derive(Debug, Clone)]
    enum Op {
        Append(bool),
        Move(usize, usize, i64),
        TrimStart(usize, i64),
        TrimEnd(usize, i64),
        Split(usize, i64),
        SplitAll(i64),
        Delete(usize),
        Ripple(usize),
        Fades(usize, i64, i64),
        Subtitle(i64, i64),
    }

    fn op() -> impl Strategy<Value = Op> {
        let t = -2_000i64..40_000; // 毫秒
        prop_oneof![
            any::<bool>().prop_map(Op::Append),
            (0usize..64, 0usize..3, t.clone()).prop_map(|(a, b, c)| Op::Move(a, b, c)),
            (0usize..64, t.clone()).prop_map(|(a, b)| Op::TrimStart(a, b)),
            (0usize..64, t.clone()).prop_map(|(a, b)| Op::TrimEnd(a, b)),
            (0usize..64, t.clone()).prop_map(|(a, b)| Op::Split(a, b)),
            t.clone().prop_map(Op::SplitAll),
            (0usize..64).prop_map(Op::Delete),
            (0usize..64).prop_map(Op::Ripple),
            (0usize..64, t.clone(), t.clone()).prop_map(|(a, b, c)| Op::Fades(a, b, c)),
            (t, 1i64..5_000).prop_map(|(a, b)| Op::Subtitle(a, b)),
        ]
    }

    fn all_clips(p: &Project) -> Vec<ClipId> {
        p.timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect()
    }

    fn apply(p: &mut Project, ids: &SampleIds, op: &Op) -> Result<(), EditError> {
        let pick = |proj: &Project, i: usize| {
            let all = all_clips(proj);
            if all.is_empty() {
                ClipId(0)
            } else {
                all[i % all.len()]
            }
        };
        let tracks = [ids.video_track, ids.audio_track, ids.subtitle_track];
        let ms = Time::from_millis;
        match *op {
            Op::Append(video) => append_asset(
                p,
                if video {
                    ids.video_asset
                } else {
                    ids.audio_asset
                },
            )
            .map(drop),
            Op::Move(c, t, at) => move_clip(p, pick(p, c), tracks[t], ms(at)),
            Op::TrimStart(c, at) => trim_start(p, pick(p, c), ms(at)),
            Op::TrimEnd(c, at) => trim_end(p, pick(p, c), ms(at)),
            Op::Split(c, at) => split_at(p, pick(p, c), ms(at)).map(drop),
            Op::SplitAll(at) => {
                split_all_at(p, ms(at));
                Ok(())
            }
            Op::Delete(c) => delete_clips(p, &[pick(p, c)]),
            Op::Ripple(c) => ripple_delete(p, &[pick(p, c)]),
            Op::Fades(c, a, b) => set_fades(p, pick(p, c), ms(a), ms(b)),
            Op::Subtitle(at, d) => add_subtitle(p, ms(at), ms(d), "x".into()).map(drop),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn random_edit_sequences_keep_invariants(ops in proptest::collection::vec(op(), 200)) {
            let (mut p, ids) = sample_project();
            for op in &ops {
                let before = p.clone();
                if apply(&mut p, &ids, op).is_err() {
                    prop_assert_eq!(&p, &before, "失败的操作改动了工程：{:?}", op);
                }
                prop_assert!(p.timeline.check_invariants(&p.assets).is_ok(), "{:?}", op);
            }
        }
    }
}
