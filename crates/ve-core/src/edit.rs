//! 编辑操作。全部是 `fn(&mut Project, ...) -> Result<_, EditError>`：失败时工程保持不变。
//!
//! 实现方式：在时间轴副本上改（片段是 `Arc`，复制只是引用计数），成功才写回。

use crate::{
    AssetId, Clip, ClipContent, ClipId, Project, Time, Timeline, TrackId, TrackKind, Transform,
};
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
    /// 这是该类型的最后一条轨道，不能删。
    #[error("至少要留一条这种轨道")]
    LastTrack,
    /// 这个时刻没有关键帧。
    #[error("这里没有关键帧")]
    NoKeyframe,
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
        transform: Transform::default(),
        animation: crate::Animation::default(),
    })
}

/// 把素材追加到对应轨道末尾：有视频进最下层的视频轨（主画面），纯音频进第一条音频轨。
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
    // 上层视频轨是叠加用的（台标、画中画），追加的正片接在主画面后面
    let track = match kind {
        TrackKind::Video => project.timeline.video_tracks_bottom_up().next(),
        _ => project.timeline.track_of_kind(kind),
    }
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

/// 把一组片段在各自轨道上一起平移 `delta`（可为负）。往前挪过头时整组停在最早的片段到 0 为止，
/// 相对位置不变；撞上组外的片段则整组不动。
///
/// # Errors
/// 片段不存在、与组外片段重叠。
pub fn move_clips(project: &mut Project, clips: &[ClipId], delta: Time) -> Result<(), EditError> {
    let mut clips = clips.to_vec();
    clips.sort_unstable();
    clips.dedup();
    with_timeline(project, |_, tl| {
        let mut found = Vec::with_capacity(clips.len());
        for &clip in &clips {
            let (ti, ci) = locate(tl, clip)?;
            found.push((ti, Arc::clone(&tl.tracks[ti].clips[ci])));
        }
        let earliest = found
            .iter()
            .map(|(_, c)| c.start)
            .min()
            .unwrap_or(Time::ZERO);
        let delta = delta.max(Time::ZERO - earliest);
        for (ti, _) in &found {
            tl.tracks[*ti].clips.retain(|c| !clips.contains(&c.id));
        }
        for (ti, c) in found {
            let mut moved = (*c).clone();
            moved.start = c.start + delta;
            if !tl.tracks[ti].is_free(moved.start, moved.end(), &[]) {
                return Err(EditError::Overlap);
            }
            tl.tracks[ti].insert_sorted(Arc::new(moved));
        }
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
        // 关键帧时间相对片段起点：起点后移多少，关键帧就前移多少，仍对着原来的内容
        c.animation.shift(c.start - start);
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
    let (left_anim, right_anim) = original.animation.split(at - original.start);
    left.animation = left_anim;
    right.animation = right_anim;
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

/// 加一条轨道：视频轨放在所有视频轨最上面，音频轨放在最后一条音频轨下面。
///
/// # Errors
/// 字幕轨（这一版只有一条）。
pub fn add_track(project: &mut Project, kind: TrackKind) -> Result<TrackId, EditError> {
    let tracks = &project.timeline.tracks;
    let at = match kind {
        TrackKind::Video => tracks.iter().position(|t| t.kind == TrackKind::Video),
        TrackKind::Audio => tracks
            .iter()
            .rposition(|t| t.kind == TrackKind::Audio)
            .map(|i| i + 1),
        TrackKind::Subtitle => return Err(EditError::WrongTrackKind),
    };
    // 没有同类轨道时：视频轨放最前，音频轨放在字幕轨之前
    let at = at.unwrap_or_else(|| match kind {
        TrackKind::Video => 0,
        _ => tracks
            .iter()
            .position(|t| t.kind == TrackKind::Subtitle)
            .unwrap_or(tracks.len()),
    });
    let id = project.ids.track();
    project
        .timeline
        .tracks
        .insert(at, crate::Track::new(id, kind));
    Ok(id)
}

/// 删除一条轨道和它上面的片段。
///
/// # Errors
/// 轨道不存在，或它是该类型的最后一条。
pub fn remove_track(project: &mut Project, track: TrackId) -> Result<(), EditError> {
    let tl = &mut project.timeline;
    let idx = track_index(tl, track)?;
    let kind = tl.tracks[idx].kind;
    if tl.tracks.iter().filter(|t| t.kind == kind).count() <= 1 {
        return Err(EditError::LastTrack);
    }
    tl.tracks.remove(idx);
    Ok(())
}

/// 设置画面属性（位置、缩放、裁剪、不透明度）。
///
/// # Errors
/// 片段不存在、不是视频轨上的媒体片段、数值违反不变式。
pub fn set_transform(project: &mut Project, clip: ClipId, t: Transform) -> Result<(), EditError> {
    if !t.is_valid() {
        return Err(EditError::InvalidValue);
    }
    ensure_video(project, clip)?;
    update_clip(project, clip, |c| {
        c.transform = t;
        Ok(())
    })
}

/// 片段要在视频轨上（画面属性和关键帧只对视频轨有意义）。
fn ensure_video(project: &Project, clip: ClipId) -> Result<(), EditError> {
    let (track, _) = project
        .timeline
        .find_clip(clip)
        .ok_or(EditError::ClipNotFound(clip))?;
    if project.timeline.track(track).map(|tr| tr.kind) == Some(TrackKind::Video) {
        Ok(())
    } else {
        Err(EditError::WrongTrackKind)
    }
}

/// 重置画面属性：回到默认值，清掉关键帧动画。
///
/// # Errors
/// 片段不存在、不是视频轨上的片段。
pub fn reset_transform(project: &mut Project, clip: ClipId) -> Result<(), EditError> {
    ensure_video(project, clip)?;
    update_clip(project, clip, |c| {
        c.transform = Transform::default();
        c.animation = crate::Animation::default();
        Ok(())
    })
}

/// 在片段内 `at` 时刻（相对片段起点）打关键帧：把那一刻所有画面属性都记下来。
///
/// # Errors
/// 片段不存在、不是视频轨上的片段。
pub fn add_keyframe(project: &mut Project, clip: ClipId, at: Time) -> Result<(), EditError> {
    ensure_video(project, clip)?;
    update_clip(project, clip, |c| {
        let now = c.transform_at(at);
        for prop in crate::Prop::TRANSFORM {
            c.animation.upsert(prop, at, prop.get(&now));
        }
        Ok(())
    })
}

/// 界面改画面属性统一走这里：片段没有动画时改固定值；有动画时，有关键帧的属性在 `at`
/// （相对片段起点）新建或更新关键帧，其余属性改固定值。
///
/// # Errors
/// 片段不存在、不是视频轨上的片段、数值违反不变式。
pub fn apply_transform_at(
    project: &mut Project,
    clip: ClipId,
    at: Time,
    t: Transform,
) -> Result<(), EditError> {
    if !t.is_valid() {
        return Err(EditError::InvalidValue);
    }
    ensure_video(project, clip)?;
    update_clip(project, clip, |c| {
        for prop in crate::Prop::TRANSFORM {
            let value = prop.get(&t);
            if c.animation.keys(prop).is_empty() {
                prop.set(&mut c.transform, value);
            } else {
                c.animation.upsert(prop, at, value);
            }
        }
        Ok(())
    })
}

/// 删掉片段内 `at` 时刻的关键帧。删完一个都不剩时，固定值取删之前那一刻的值，画面不跳。
///
/// # Errors
/// 片段不存在、那一刻没有关键帧。
pub fn remove_keyframe(project: &mut Project, clip: ClipId, at: Time) -> Result<(), EditError> {
    update_clip(project, clip, |c| {
        let now = c.transform_at(at);
        if !c.animation.remove_at(at) {
            return Err(EditError::NoKeyframe);
        }
        if c.animation.is_empty() {
            c.transform = now;
        }
        Ok(())
    })
}

/// 设置片段内 `at` 时刻关键帧的缓动（作用在它到下一个关键帧这一段）。
///
/// # Errors
/// 片段不存在、那一刻没有关键帧。
pub fn set_keyframe_easing(
    project: &mut Project,
    clip: ClipId,
    at: Time,
    easing: crate::Easing,
) -> Result<(), EditError> {
    update_clip(project, clip, |c| {
        if c.animation.set_easing(at, easing) {
            Ok(())
        } else {
            Err(EditError::NoKeyframe)
        }
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
        transform: Transform::default(),
        animation: crate::Animation::default(),
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

    fn starts(p: &Project, ids: &[ClipId]) -> Vec<Time> {
        ids.iter()
            .map(|c| p.timeline.find_clip(*c).unwrap().1.start)
            .collect()
    }

    #[test]
    fn move_clips_shifts_group_on_their_tracks() {
        let (mut p, ids) = sample_project();
        // v1 0–5、v2 5–8（V1），a1 0–8（A1）一起往后 2 秒
        // 同一个片段给了两次也只挪一次
        move_clips(
            &mut p,
            &[ids.v1, ids.v2, ids.a1, ids.v1],
            Time::from_secs(2),
        )
        .unwrap();
        assert_eq!(
            starts(&p, &[ids.v1, ids.v2, ids.a1]),
            vec![Time::from_secs(2), Time::from_secs(7), Time::from_secs(2)]
        );
        assert_eq!(p.timeline.find_clip(ids.a1).unwrap().0, ids.audio_track);
        assert!(p.timeline.check_invariants(&p.assets).is_ok());
    }

    #[test]
    fn move_clips_stops_at_zero_and_refuses_overlap() {
        let (mut p, ids) = sample_project();
        move_clips(&mut p, &[ids.v2], Time::from_secs(10)).unwrap();
        // 往前挪太多：最早的片段停在 0，整体相对位置不变
        move_clips(&mut p, &[ids.v1, ids.a1], Time::from_secs(-3)).unwrap();
        assert_eq!(starts(&p, &[ids.v1, ids.a1]), vec![Time::ZERO, Time::ZERO]);
        // 撞上没选中的片段：整组不动
        let before = p.clone();
        assert_eq!(
            move_clips(&mut p, &[ids.v1], Time::from_secs(12)),
            Err(EditError::Overlap)
        );
        assert_eq!(p, before);
        assert_eq!(
            move_clips(&mut p, &[ClipId(999)], Time::from_secs(1)),
            Err(EditError::ClipNotFound(ClipId(999)))
        );
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

    fn kinds(p: &Project) -> Vec<TrackKind> {
        p.timeline.tracks.iter().map(|t| t.kind).collect()
    }

    #[test]
    fn add_video_track_goes_on_top() {
        let (mut p, ids) = sample_project();
        let new = add_track(&mut p, TrackKind::Video).unwrap();
        assert_eq!(
            kinds(&p),
            [
                TrackKind::Video,
                TrackKind::Video,
                TrackKind::Audio,
                TrackKind::Subtitle
            ]
        );
        assert_eq!(p.timeline.tracks[0].id, new);
        assert_eq!(p.timeline.tracks[1].id, ids.video_track);
        assert_eq!(
            add_track(&mut p, TrackKind::Subtitle),
            Err(EditError::WrongTrackKind)
        );
        assert_valid(&p);
    }

    #[test]
    fn append_goes_to_bottom_video_track_after_adding_tracks() {
        // 加了上层视频轨（放台标用）之后，「添加到时间轴」仍然接在主画面（最下层）后面
        let (mut p, ids) = sample_project();
        let top = add_track(&mut p, TrackKind::Video).unwrap();
        let clip = append_asset(&mut p, ids.video_asset).unwrap();
        assert_eq!(p.timeline.find_clip(clip).unwrap().0, ids.video_track);
        assert_eq!(p.timeline.track(top).unwrap().clips.len(), 0);
    }

    #[test]
    fn add_audio_track_goes_below_audio() {
        let (mut p, ids) = sample_project();
        let new = add_track(&mut p, TrackKind::Audio).unwrap();
        assert_eq!(
            kinds(&p),
            [
                TrackKind::Video,
                TrackKind::Audio,
                TrackKind::Audio,
                TrackKind::Subtitle
            ]
        );
        assert_eq!(p.timeline.tracks[1].id, ids.audio_track);
        assert_eq!(p.timeline.tracks[2].id, new);
    }

    #[test]
    fn remove_track_drops_clips_and_refuses_last() {
        let (mut p, ids) = sample_project();
        assert_eq!(
            remove_track(&mut p, ids.video_track),
            Err(EditError::LastTrack)
        );
        let top = add_track(&mut p, TrackKind::Video).unwrap();
        // 把 v2 挪到新轨，再删旧轨：旧轨上的 v1 一起没了
        move_clip(&mut p, ids.v2, top, secs(5)).unwrap();
        remove_track(&mut p, ids.video_track).unwrap();
        assert!(p.timeline.find_clip(ids.v1).is_none());
        assert_eq!(p.timeline.find_clip(ids.v2).unwrap().0, top);
        assert_eq!(
            remove_track(&mut p, ids.video_track),
            Err(EditError::TrackNotFound(ids.video_track))
        );
        assert_eq!(
            remove_track(&mut p, ids.subtitle_track),
            Err(EditError::LastTrack)
        );
        assert_valid(&p);
    }

    #[test]
    fn bottom_up_order() {
        let (mut p, ids) = sample_project();
        let top = add_track(&mut p, TrackKind::Video).unwrap();
        let order: Vec<TrackId> = p.timeline.video_tracks_bottom_up().map(|t| t.id).collect();
        assert_eq!(order, [ids.video_track, top]);
    }

    #[test]
    fn set_transform_validates_and_targets_video_only() {
        let (mut p, ids) = sample_project();
        let t = Transform {
            x: 0.25,
            scale: 0.5,
            opacity: 0.8,
            ..Transform::default()
        };
        set_transform(&mut p, ids.v1, t).unwrap();
        assert_eq!(clip(&p, ids.v1).transform, t);
        assert_eq!(
            set_transform(&mut p, ids.a1, t),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(
            set_transform(&mut p, ids.s1, t),
            Err(EditError::WrongTrackKind)
        );
        let bad = Transform {
            scale: 0.0,
            ..Transform::default()
        };
        assert_eq!(
            set_transform(&mut p, ids.v1, bad),
            Err(EditError::InvalidValue)
        );
        assert_eq!(clip(&p, ids.v1).transform, t, "失败时不变");
        assert_valid(&p);
        // 不变式能拦住无效的画面属性
        let tr = p.timeline.track_of_kind_mut(TrackKind::Video).unwrap();
        let mut c = (*tr.clips[0]).clone();
        c.transform = bad;
        tr.clips[0] = Arc::new(c);
        assert!(matches!(
            p.timeline.check_invariants(&p.assets),
            Err(crate::InvariantError::BadTransform { .. })
        ));
    }

    fn scale_at(p: &Project, id: ClipId, offset: Time) -> f32 {
        clip(p, id).transform_at(offset).scale
    }

    #[test]
    fn add_keyframe_records_every_property_and_edits_upsert() {
        let (mut p, ids) = sample_project();
        let half = Transform {
            scale: 0.5,
            ..Transform::default()
        };
        // 没有动画：apply_transform_at 就是改固定值
        apply_transform_at(&mut p, ids.v1, Time::from_secs(1), half).unwrap();
        assert_eq!(clip(&p, ids.v1).transform, half);
        assert!(clip(&p, ids.v1).animation.is_empty());
        // 打关键帧：所有画面属性都记下来
        add_keyframe(&mut p, ids.v1, Time::ZERO).unwrap();
        let a = &clip(&p, ids.v1).animation;
        assert_eq!(a.props().count(), crate::Prop::TRANSFORM.len());
        assert_eq!(a.times(), vec![Time::ZERO]);
        // 有动画后在别处改属性：在那里新建关键帧
        let full = Transform::default();
        apply_transform_at(&mut p, ids.v1, Time::from_secs(2), full).unwrap();
        assert_eq!(
            clip(&p, ids.v1).animation.times(),
            vec![Time::ZERO, Time::from_secs(2)]
        );
        assert!((scale_at(&p, ids.v1, Time::from_secs(2)) - 1.0).abs() < 1e-6);
        // 中间是缓入缓出：正中间正好一半
        assert!((scale_at(&p, ids.v1, Time::from_secs(1)) - 0.75).abs() < 1e-6);
        // 同一处再改：更新而不是新增
        apply_transform_at(&mut p, ids.v1, Time::from_secs(2), half).unwrap();
        assert_eq!(clip(&p, ids.v1).animation.times().len(), 2);
        assert!((scale_at(&p, ids.v1, Time::from_secs(2)) - 0.5).abs() < 1e-6);
        assert_valid(&p);
        // 不合法的值、不是视频轨的片段
        let bad = Transform {
            scale: 0.0,
            ..Transform::default()
        };
        assert_eq!(
            apply_transform_at(&mut p, ids.v1, Time::ZERO, bad),
            Err(EditError::InvalidValue)
        );
        assert_eq!(
            add_keyframe(&mut p, ids.a1, Time::ZERO),
            Err(EditError::WrongTrackKind)
        );
    }

    #[test]
    fn removing_the_last_keyframe_keeps_the_picture() {
        let (mut p, ids) = sample_project();
        let half = Transform {
            scale: 0.5,
            ..Transform::default()
        };
        add_keyframe(&mut p, ids.v1, Time::ZERO).unwrap();
        apply_transform_at(&mut p, ids.v1, Time::from_secs(2), half).unwrap();
        remove_keyframe(&mut p, ids.v1, Time::ZERO).unwrap();
        assert_eq!(clip(&p, ids.v1).animation.times(), vec![Time::from_secs(2)]);
        assert_eq!(
            remove_keyframe(&mut p, ids.v1, Time::ZERO),
            Err(EditError::NoKeyframe)
        );
        // 最后一个也删掉：动画清空，固定值取删之前那一刻的值，画面不跳
        remove_keyframe(&mut p, ids.v1, Time::from_secs(2)).unwrap();
        assert!(clip(&p, ids.v1).animation.is_empty());
        assert!((clip(&p, ids.v1).transform.scale - 0.5).abs() < 1e-6);
        assert_valid(&p);
    }

    #[test]
    fn keyframe_easing_and_reset() {
        let (mut p, ids) = sample_project();
        add_keyframe(&mut p, ids.v1, Time::ZERO).unwrap();
        set_keyframe_easing(&mut p, ids.v1, Time::ZERO, crate::Easing::Linear).unwrap();
        assert_eq!(
            clip(&p, ids.v1).animation.easing_at(Time::ZERO),
            Some(crate::Easing::Linear)
        );
        assert_eq!(
            set_keyframe_easing(&mut p, ids.v1, Time::from_secs(3), crate::Easing::Linear),
            Err(EditError::NoKeyframe)
        );
        // 重置：画面属性回默认，动画清掉
        reset_transform(&mut p, ids.v1).unwrap();
        assert!(clip(&p, ids.v1).animation.is_empty());
        assert!(clip(&p, ids.v1).transform.is_default());
    }

    /// 时间轴上 `t` 时刻，`ids` 里正在播的那个片段的画面属性。
    fn picture_at(p: &Project, ids: &[ClipId], t: Time) -> Transform {
        let c = ids
            .iter()
            .map(|id| clip(p, *id))
            .find(|c| c.start <= t && t < c.end())
            .unwrap();
        c.transform_at(t - c.start)
    }

    fn animate_linear(p: &mut Project, id: ClipId, from: Time, to: Time, a: f32, b: f32) {
        let at = |s: f32| Transform {
            x: s,
            ..Transform::default()
        };
        add_keyframe(p, id, from).unwrap();
        apply_transform_at(p, id, from, at(a)).unwrap();
        apply_transform_at(p, id, to, at(b)).unwrap();
        set_keyframe_easing(p, id, from, crate::Easing::Linear).unwrap();
    }

    #[test]
    fn trim_start_keeps_keyframes_on_the_content() {
        let (mut p, ids) = sample_project();
        // v2 在 5–8 秒；动画在片段内 0–2 秒，x 从 0 到 0.4
        animate_linear(&mut p, ids.v2, Time::ZERO, Time::from_secs(2), 0.0, 0.4);
        let times = [6.0, 6.5, 7.0, 7.5].map(Time::from_secs_f64);
        let before = times.map(|t| picture_at(&p, &[ids.v2], t).x);
        trim_start(&mut p, ids.v2, Time::from_secs(6)).unwrap();
        let after = times.map(|t| picture_at(&p, &[ids.v2], t).x);
        for (b, a) in before.iter().zip(&after) {
            assert!((b - a).abs() < 1e-6, "{before:?} / {after:?}");
        }
        assert_valid(&p);
    }

    #[test]
    fn split_keeps_the_motion_on_both_halves() {
        let (mut p, ids) = sample_project();
        // v1 在 0–5 秒；x 从 0 到 0.4（片段内 0–4 秒，匀速）
        animate_linear(&mut p, ids.v1, Time::ZERO, Time::from_secs(4), 0.0, 0.4);
        let times = [0.5, 1.0, 1.9, 2.0, 2.5, 3.9, 4.5].map(Time::from_secs_f64);
        let before = times.map(|t| picture_at(&p, &[ids.v1], t).x);
        let right = split_at(&mut p, ids.v1, Time::from_secs(2)).unwrap();
        let after = times.map(|t| picture_at(&p, &[ids.v1, right], t).x);
        for (b, a) in before.iter().zip(&after) {
            assert!((b - a).abs() < 1e-5, "{before:?} / {after:?}");
        }
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
