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
    /// 要改的片段在锁定的轨道上。
    #[error("轨道已锁定，先在轨道头解锁")]
    TrackLocked,
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
        ClipContent::Adjustment => kind == TrackKind::Video,
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
            |a| clip.start + (a.source_limit() - *source_in).scaled(1.0 / f64::from(clip.speed)),
        ),
        ClipContent::Subtitle { .. } | ClipContent::Adjustment => Time(i64::MAX / 4),
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
        adjust: crate::ColorAdjust::NONE,
        disabled: false,
        transition_in: None,
        speed: 1.0,
        keep_pitch: true,
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

/// 只要素材的声音（剪映「音频提取」）：追加到第一条音频轨末尾。视频文件也行，放上去的只有声音。
///
/// # Errors
/// 素材不存在、没有声音。
pub fn append_audio(project: &mut Project, asset: AssetId) -> Result<ClipId, EditError> {
    let a = project
        .assets
        .get(&asset)
        .ok_or(EditError::AssetNotFound(asset))?;
    if a.audio.is_none() {
        return Err(EditError::WrongTrackKind);
    }
    let track = project
        .timeline
        .track_of_kind(TrackKind::Audio)
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
            ClipContent::Media { source_in, .. } => {
                c.start - source_in.scaled(1.0 / f64::from(c.speed))
            }
            ClipContent::Subtitle { .. } | ClipContent::Adjustment => Time::ZERO,
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
        let speed = f64::from(c.speed);
        if let ClipContent::Media { source_in, .. } = &mut c.content {
            *source_in += (start - c.start).scaled(speed);
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

/// `clips` 里跨过 `at`（起点 < at < 终点）的片段；任一片段不存在就报错。
fn crossing(project: &Project, clips: &[ClipId], at: Time) -> Result<Vec<ClipId>, EditError> {
    let mut out = Vec::new();
    for &id in clips {
        let (_, c) = project
            .timeline
            .find_clip(id)
            .ok_or(EditError::ClipNotFound(id))?;
        if c.start < at && at < c.end() {
            out.push(id);
        }
    }
    Ok(out)
}

/// 向左裁剪（剪映 / FCP X 的 Q）：`clips` 里跨过 `at` 的片段，剪掉 `at` 左边的部分，返回剪了几个。
///
/// # Errors
/// 任一片段不存在（这时什么都不改）。
pub fn trim_left_at(project: &mut Project, clips: &[ClipId], at: Time) -> Result<usize, EditError> {
    let targets = crossing(project, clips, at)?;
    for &c in &targets {
        trim_start(project, c, at)?;
    }
    Ok(targets.len())
}

/// 向右裁剪（W）：`clips` 里跨过 `at` 的片段，剪掉 `at` 右边的部分，返回剪了几个。
///
/// # Errors
/// 任一片段不存在（这时什么都不改）。
pub fn trim_right_at(
    project: &mut Project,
    clips: &[ClipId],
    at: Time,
) -> Result<usize, EditError> {
    let targets = crossing(project, clips, at)?;
    for &c in &targets {
        trim_end(project, c, at)?;
    }
    Ok(targets.len())
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
    // 切开的地方是硬切：右半段不带原来的入场转场
    right.transition_in = None;
    right.start = at;
    right.duration = original.end() - at;
    right.fade_in = Time::ZERO;
    let speed = f64::from(right.speed);
    if let ClipContent::Media { source_in, .. } = &mut right.content {
        *source_in += left.duration.scaled(speed);
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
        .filter(|t| !t.locked)
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
/// 用改好的片段（同一个 id）替换原来的，经过不变式检查。
pub(crate) fn replace_clip(project: &mut Project, clip: Clip) -> Result<(), EditError> {
    update_clip(project, clip.id, |slot| {
        *slot = clip;
        Ok(())
    })
}

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
    // 没有同类轨道时：视频轨放在字幕轨后面，音频轨放最后
    let at = at.unwrap_or_else(|| match kind {
        TrackKind::Video => tracks
            .iter()
            .position(|t| t.kind != TrackKind::Subtitle)
            .unwrap_or(tracks.len()),
        _ => tracks.len(),
    });
    let id = project.ids.track();
    project
        .timeline
        .tracks
        .insert(at, crate::Track::new(id, kind));
    Ok(id)
}

/// 某类轨道可以新插入的位置（轨道列表下标，含两端）：同类轨道要挨在一起。
/// 字幕轨只有一条，返回 `None`；还没有这类轨道时也返回 `None`。
#[must_use]
pub fn insert_slots(
    timeline: &Timeline,
    kind: TrackKind,
) -> Option<std::ops::RangeInclusive<usize>> {
    if kind == TrackKind::Subtitle {
        return None;
    }
    let first = timeline.tracks.iter().position(|t| t.kind == kind)?;
    let last = timeline.tracks.iter().rposition(|t| t.kind == kind)?;
    Some(first..=last + 1)
}

/// 把片段拖进两条轨道之间的缝（剪映：拖到轨道之间自动新建轨道）：在 `slot` 处新建一条
/// 和片段所在轨道同类型的轨道，把片段放进去、起点 `start`。返回新轨道。
///
/// # Errors
/// 片段不存在；`slot` 不在这类轨道能插入的范围内（或片段在字幕轨上）时返回
/// [`EditError::WrongTrackKind`]。失败时工程不变。
pub fn move_clip_to_new_track(
    project: &mut Project,
    clip: ClipId,
    slot: usize,
    start: Time,
) -> Result<TrackId, EditError> {
    let (track, _) = project
        .timeline
        .find_clip(clip)
        .ok_or(EditError::ClipNotFound(clip))?;
    let kind = project
        .timeline
        .track(track)
        .ok_or(EditError::TrackNotFound(track))?
        .kind;
    if !insert_slots(&project.timeline, kind).is_some_and(|r| r.contains(&slot)) {
        return Err(EditError::WrongTrackKind);
    }
    let mut next = project.clone();
    let id = next.ids.track();
    next.timeline
        .tracks
        .insert(slot, crate::Track::new(id, kind));
    move_clip(&mut next, clip, id, start)?;
    *project = next;
    Ok(id)
}

/// 切换画面比例（剪映播放器上的「比例」）：保持短边不变，按比例算长边，宽高取偶数；
/// `None` 是「原始」，回到第一个视频素材的尺寸（没有视频素材时不变）。
/// 画面按「完整放进画布」摆放，所以换比例后各图层自动适配（留黑边）。
///
/// # Errors
/// 比例里有 0。
pub fn set_aspect(project: &mut Project, ratio: Option<(u32, u32)>) -> Result<(), EditError> {
    let s = &mut project.settings;
    let Some((rw, rh)) = ratio else {
        let first = project
            .assets
            .values()
            .find_map(|a| a.video.filter(|v| !v.still));
        if let Some(v) = first {
            s.width = (v.width & !1).max(2);
            s.height = (v.height & !1).max(2);
        }
        return Ok(());
    };
    if rw == 0 || rh == 0 {
        return Err(EditError::InvalidValue);
    }
    let short = f64::from(s.width.min(s.height));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "画面边长为正且远小于 u32 上限"
    )]
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    let (w, h) = if rw >= rh {
        (even(short * f64::from(rw) / f64::from(rh)), even(short))
    } else {
        (even(short), even(short * f64::from(rh) / f64::from(rw)))
    };
    s.width = w;
    s.height = h;
    Ok(())
}

/// 改分辨率（剪映草稿参数）：保持比例，短边设成 `short`，宽高取偶数。
///
/// # Errors
/// `short` 为 0。
pub fn set_resolution(project: &mut Project, short: u32) -> Result<(), EditError> {
    if short == 0 {
        return Err(EditError::InvalidValue);
    }
    let s = &mut project.settings;
    let (w, h) = (f64::from(s.width), f64::from(s.height));
    let k = f64::from(short) / w.min(h);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "画面边长为正且远小于 u32 上限"
    )]
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    s.width = even(w * k);
    s.height = even(h * k);
    Ok(())
}

/// 改工程帧率。片段时间用 flicks 存，不受影响；导出、逐帧按新帧率。
///
/// # Errors
/// 帧率分子或分母为 0。
pub const fn set_frame_rate(
    project: &mut Project,
    rate: crate::FrameRate,
) -> Result<(), EditError> {
    if rate.num == 0 || rate.den == 0 {
        return Err(EditError::InvalidValue);
    }
    project.settings.frame_rate = rate;
    Ok(())
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

/// 轨道头的开关（剪映的 🔒 👁 🔊）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackFlag {
    /// 锁定：片段不能选中、不能改。
    Locked,
    /// 隐藏：画面、字幕不进预览和导出。
    Hidden,
    /// 静音：声音不进预览和导出。
    Muted,
}

/// 打开 / 关掉轨道头的一个开关。
///
/// # Errors
/// 轨道不存在。
pub fn set_track_flag(
    project: &mut Project,
    track: TrackId,
    flag: TrackFlag,
    on: bool,
) -> Result<(), EditError> {
    let idx = track_index(&project.timeline, track)?;
    let t = &mut project.timeline.tracks[idx];
    match flag {
        TrackFlag::Locked => t.locked = on,
        TrackFlag::Hidden => t.hidden = on,
        TrackFlag::Muted => t.muted = on,
    }
    Ok(())
}

/// 检查一次编辑有没有动到锁定轨道上的片段：`before` 里锁着的轨道，片段必须原样不动
/// （轨道本身也不能被删）。
///
/// # Errors
/// 动到了：[`EditError::TrackLocked`]。
pub fn check_locked(before: &Timeline, after: &Timeline) -> Result<(), EditError> {
    for old in before.tracks.iter().filter(|t| t.locked) {
        if after.track(old.id).is_none_or(|t| t.clips != old.clips) {
            return Err(EditError::TrackLocked);
        }
    }
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

/// 设置片段的调色（剪映「调整」）。
///
/// # Errors
/// 片段不存在、不是视频轨上的片段、有超出 -1–1 或不是有限数的项。
pub fn set_adjust(
    project: &mut Project,
    clip: ClipId,
    adjust: crate::ColorAdjust,
) -> Result<(), EditError> {
    if !adjust.is_valid() {
        return Err(EditError::InvalidValue);
    }
    ensure_video(project, clip)?;
    update_clip(project, clip, |c| {
        c.adjust = adjust;
        Ok(())
    })
}

/// 复制下来的片段：（原来的轨道，片段）；片段起点是相对最早那段的偏移。
pub type Copied = Vec<(TrackId, Clip)>;

/// 复制片段（Ctrl+C）。
///
/// # Errors
/// 任一片段不存在。
pub fn copy_clips(project: &Project, clips: &[ClipId]) -> Result<Copied, EditError> {
    let mut out: Copied = Vec::with_capacity(clips.len());
    for &id in clips {
        let (track, c) = project
            .timeline
            .find_clip(id)
            .ok_or(EditError::ClipNotFound(id))?;
        out.push((track, (**c).clone()));
    }
    let first = out.iter().map(|(_, c)| c.start).min().unwrap_or(Time::ZERO);
    for (_, c) in &mut out {
        c.start -= first;
    }
    Ok(out)
}

/// 粘贴（Ctrl+V）：按复制时的相对位置放到 `at` 开始，各回原来的轨道（轨道没了就放到同类的第一条）。
/// 主轨磁吸开着时，进主轨的片段按位置插队。返回新片段。
///
/// # Errors
/// 位置被占了（这时什么都不改）、没有能放的轨道、素材已经不在了。
pub fn paste_clips(
    project: &mut Project,
    copied: &[(TrackId, Clip)],
    at: Time,
) -> Result<Vec<ClipId>, EditError> {
    let mut next = project.clone();
    let main = crate::magnet::main_track(&next.timeline);
    let mut ids = Vec::with_capacity(copied.len());
    for (track, clip) in copied {
        if let Some(asset) = clip.asset()
            && !next.assets.contains_key(&asset)
        {
            return Err(EditError::AssetNotFound(asset));
        }
        let kind_of = |tl: &Timeline, id: TrackId| tl.track(id).map(|t| t.kind);
        let wanted = project.timeline.track(*track).map(|t| t.kind).or_else(|| {
            // 轨道删掉了：按内容猜类型
            Some(if clip.subtitle_text().is_some() {
                TrackKind::Subtitle
            } else {
                TrackKind::Video
            })
        });
        let target = if kind_of(&next.timeline, *track).is_some() {
            *track
        } else {
            next.timeline
                .tracks
                .iter()
                .find(|t| Some(t.kind) == wanted)
                .map(|t| t.id)
                .ok_or(EditError::WrongTrackKind)?
        };
        let id = next.ids.clip();
        let mut c = clip.clone();
        c.id = id;
        c.start = at + clip.start;
        let magnet = next.settings.main_magnet && Some(target) == main;
        let tl = &mut next.timeline;
        let t = tl
            .track_mut(target)
            .ok_or(EditError::TrackNotFound(target))?;
        if magnet {
            // 先放到主轨末尾，再按位置插队
            let end = t.end();
            let wanted_start = c.start;
            c.start = end;
            t.insert_sorted(Arc::new(c));
            crate::magnet::insert_into_main(&mut next, id, wanted_start)?;
        } else {
            if !t.is_free(c.start, c.end(), &[]) {
                return Err(EditError::Overlap);
            }
            t.insert_sorted(Arc::new(c));
        }
        ids.push(id);
    }
    next.timeline
        .check_invariants(&next.assets)
        .map_err(|_| EditError::WrongTrackKind)?;
    *project = next;
    Ok(ids)
}

/// 复制下来的属性（剪映「复制属性」）：画面属性、关键帧动画、调色。
#[derive(Debug, Clone, PartialEq)]
pub struct Attributes {
    /// 画面属性。
    pub transform: Transform,
    /// 关键帧动画。
    pub animation: crate::Animation,
    /// 调色。
    pub adjust: crate::ColorAdjust,
}

/// 复制属性（Ctrl+Shift+C）。
///
/// # Errors
/// 片段不存在、不在视频轨上。
pub fn copy_attributes(project: &Project, clip: ClipId) -> Result<Attributes, EditError> {
    ensure_video(project, clip)?;
    let (_, c) = project
        .timeline
        .find_clip(clip)
        .ok_or(EditError::ClipNotFound(clip))?;
    Ok(Attributes {
        transform: c.transform,
        animation: c.animation.clone(),
        adjust: c.adjust,
    })
}

/// 粘贴属性（Ctrl+Shift+V）：套到这些视频片段上（不在视频轨上的跳过），返回套了几个。
///
/// # Errors
/// 任一片段不存在。
pub fn paste_attributes(
    project: &mut Project,
    clips: &[ClipId],
    attrs: &Attributes,
) -> Result<usize, EditError> {
    let mut n = 0;
    let mut next = project.clone();
    for &id in clips {
        match ensure_video(&next, id) {
            Ok(()) => {}
            Err(EditError::WrongTrackKind) => continue,
            Err(e) => return Err(e),
        }
        update_clip(&mut next, id, |c| {
            c.transform = attrs.transform;
            c.animation = attrs.animation.clone();
            c.adjust = attrs.adjust;
            Ok(())
        })?;
        n += 1;
    }
    *project = next;
    Ok(n)
}

/// 改字幕样式（应用到全部字幕）。
///
/// # Errors
/// 样式里有超出范围的值。
pub fn set_subtitle_style(
    project: &mut Project,
    style: crate::SubtitleStyle,
) -> Result<(), EditError> {
    if !style.is_valid() {
        return Err(EditError::InvalidValue);
    }
    project.subtitle_style = style;
    Ok(())
}

/// 变速（剪映「变速 › 常规变速」）：素材用的那一段不变，时间轴上的长度 = 原来素材长度 / 倍数。
/// 关键帧跟着内容一起伸缩。片段变长撞到后一段时：主轨磁吸开着就把后面的整体往后推，否则报错。
///
/// # Errors
/// 片段不存在、是字幕、倍数超出 0.1–10、变长后和后面的片段重叠（不在磁吸的主轨上时）。
pub fn set_speed(project: &mut Project, clip: ClipId, speed: f32) -> Result<(), EditError> {
    if !(crate::timeline::SPEED_RANGE.0..=crate::timeline::SPEED_RANGE.1).contains(&speed) {
        return Err(EditError::InvalidValue);
    }
    let shortest = min_len(project);
    let main_track = crate::magnet::main_track(&project.timeline);
    let magnet = project.settings.main_magnet;
    with_timeline(project, |_, tl| {
        let (ti, ci) = locate(tl, clip)?;
        let mut c = (*tl.tracks[ti].clips[ci]).clone();
        // 只有素材片段能变速
        if c.asset().is_none() {
            return Err(EditError::WrongTrackKind);
        }
        let span = c.source_span();
        let old = (c.duration, c.speed);
        // 往下取整：新的素材长度不会超过原来的（不越过素材结尾）
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss,
            reason = "时长远小于 2^53"
        )]
        let duration = Time((span.0 as f64 / f64::from(speed)).floor() as i64).max(shortest);
        let ratio = f64::from(old.1) / f64::from(speed);
        c.speed = speed;
        c.duration = duration;
        c.animation.scale_times(ratio);
        clamp_fades(&mut c);
        let delta = duration - old.0;
        let track = &mut tl.tracks[ti];
        let next_start = track.clips.get(ci + 1).map(|n| n.start);
        if next_start.is_some_and(|s| c.end() > s) {
            if magnet && Some(track.id) == main_track {
                // 磁吸的主轨：后面的整体往后推
                for later in &mut track.clips[ci + 1..] {
                    Arc::make_mut(later).start += delta;
                }
            } else {
                return Err(EditError::Overlap);
            }
        }
        track.clips[ci] = Arc::new(c);
        Ok(())
    })
}

/// 变速时保持音调（剪映「声音变调」关着）还是让音调跟着快慢变。
///
/// # Errors
/// 片段不存在、是字幕。
pub fn set_keep_pitch(project: &mut Project, clip: ClipId, keep: bool) -> Result<(), EditError> {
    update_clip(project, clip, |c| {
        if c.asset().is_none() {
            return Err(EditError::WrongTrackKind);
        }
        c.keep_pitch = keep;
        Ok(())
    })
}

/// 设置进入这一段时的转场（`None` 去掉）。转场只在它和前一段紧挨着时起作用。
///
/// # Errors
/// 片段不存在、不在视频轨上、时长不是正数。
pub fn set_transition(
    project: &mut Project,
    clip: ClipId,
    transition: Option<crate::Transition>,
) -> Result<(), EditError> {
    if transition.is_some_and(|t| t.duration <= Time::ZERO) {
        return Err(EditError::ZeroLength);
    }
    ensure_video(project, clip)?;
    update_clip(project, clip, |c| {
        c.transition_in = transition;
        Ok(())
    })
}

/// 定格的默认长度（剪映是 3 秒）。
pub const FREEZE_LENGTH: Time = Time::from_secs(3);

/// 定格（剪映工具栏的「定格」）：在 `at` 处把视频片段 `clip` 切开，中间插入一段 `length` 长的
/// 静态图片 `still`（`at` 那一帧取出来的画面），同轨后面的片段整体往后挪 `length`。
/// 图片沿用原片段在 `at` 那一刻的画面属性（固定值，不带关键帧）和调色。返回定格片段的 id。
///
/// # Errors
/// 片段 / 素材不存在、片段不是视频轨上的媒体片段、`at` 不在片段内、`length` 不是正数。
pub fn insert_freeze(
    project: &mut Project,
    clip: ClipId,
    at: Time,
    still: AssetId,
    length: Time,
) -> Result<ClipId, EditError> {
    ensure_video(project, clip)?;
    if length <= Time::ZERO {
        return Err(EditError::ZeroLength);
    }
    let (track, source) = project
        .timeline
        .find_clip(clip)
        .map(|(t, c)| (t, Arc::clone(c)))
        .ok_or(EditError::ClipNotFound(clip))?;
    if source.asset().is_none() || at < source.start || at >= source.end() {
        return Err(EditError::InvalidValue);
    }
    let mut next = project.clone();
    if at > source.start {
        split_at(&mut next, clip, at)?;
    }
    let mut ids = next.ids.clone();
    let id = ids.clip();
    let mut frozen = new_media_clip(&next, id, still, at)?;
    frozen.duration = length;
    frozen.transform = source.transform_at(at - source.start);
    frozen.adjust = source.adjust;
    with_timeline(&mut next, |_, tl| {
        let ti = track_index(tl, track)?;
        let t = &mut tl.tracks[ti];
        // 从 at 起的片段整体后挪（挪的是同一个量，先后顺序不变）
        for c in t.clips.iter_mut().filter(|c| c.start >= at) {
            Arc::make_mut(c).start = c.start + length;
        }
        t.insert_sorted(Arc::new(frozen));
        Ok(())
    })?;
    next.ids = ids;
    *project = next;
    Ok(id)
}

/// 左右镜像（剪映工具栏的「镜像」）：给每个视频片段打开 / 关掉镜像。
///
/// # Errors
/// 任一片段不存在或不是视频轨上的片段（这时什么都不改）。
pub fn set_mirror(project: &mut Project, clips: &[ClipId], on: bool) -> Result<(), EditError> {
    for &id in clips {
        ensure_video(project, id)?;
    }
    with_timeline(project, |_, tl| {
        for &id in clips {
            let (ti, ci) = locate(tl, id)?;
            let mut c = (*tl.tracks[ti].clips[ci]).clone();
            c.transform.mirror = on;
            tl.tracks[ti].clips[ci] = Arc::new(c);
        }
        Ok(())
    })
}

/// 顺时针转 90°（剪映工具栏的「旋转」）：在片段内 `at` 时刻的角度上加 90°（落在 -180°–180°），
/// 有旋转关键帧时在 `at` 打帧。
///
/// # Errors
/// 片段不存在、不是视频轨上的片段。
pub fn rotate_quarter(project: &mut Project, clip: ClipId, at: Time) -> Result<(), EditError> {
    ensure_video(project, clip)?;
    let (_, c) = project
        .timeline
        .find_clip(clip)
        .ok_or(EditError::ClipNotFound(clip))?;
    let mut t = c.transform_at(at);
    // 和属性面板一样落在 (-180°, 180°]；先取整到 0.001° 免得累积误差
    t.rotation = crate::wrap_degrees(((t.rotation + 90.0) * 1000.0).round() / 1000.0);
    apply_transform_at(project, clip, at, t)
}

/// 停用 / 启用片段（剪映「停用片段」V）。
///
/// # Errors
/// 任一片段不存在（这时什么都不改）。
pub fn set_disabled(
    project: &mut Project,
    clips: &[ClipId],
    disabled: bool,
) -> Result<(), EditError> {
    with_timeline(project, |_, tl| {
        for &id in clips {
            let (ti, ci) = locate(tl, id)?;
            let mut c = (*tl.tracks[ti].clips[ci]).clone();
            c.disabled = disabled;
            tl.tracks[ti].clips[ci] = Arc::new(c);
        }
        Ok(())
    })
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

/// 只给几个属性在 `at`（相对片段起点）打关键帧（剪映属性面板每一行的 ◇），值取那一刻的值。
///
/// # Errors
/// 片段不存在、不是视频轨上的片段。
pub fn add_keyframe_props(
    project: &mut Project,
    clip: ClipId,
    at: Time,
    props: &[crate::Prop],
) -> Result<(), EditError> {
    ensure_video(project, clip)?;
    update_clip(project, clip, |c| {
        let now = c.transform_at(at);
        for prop in props {
            c.animation.upsert(*prop, at, prop.get(&now));
        }
        Ok(())
    })
}

/// 删掉几个属性在 `at` 处的关键帧。某个属性的关键帧删光了，它的固定值取删之前那一刻的值，画面不跳。
///
/// # Errors
/// 片段不存在、这些属性在那一刻都没有关键帧。
pub fn remove_keyframe_props(
    project: &mut Project,
    clip: ClipId,
    at: Time,
    props: &[crate::Prop],
) -> Result<(), EditError> {
    update_clip(project, clip, |c| {
        let now = c.transform_at(at);
        if !c.animation.remove_props_at(props, at) {
            return Err(EditError::NoKeyframe);
        }
        for prop in props {
            if c.animation.keys(*prop).is_empty() {
                prop.set(&mut c.transform, prop.get(&now));
            }
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

/// 调节片段的默认长度（剪映「自定义调节」是 3 秒）。
pub const ADJUSTMENT_LENGTH: Time = Time::from_secs(3);

/// 加一段调节（剪映「自定义调节」）：放在最上面的视频轨上（那里 `at` 起空着、而且不是主轨时），
/// 否则在最上面新建一条视频轨。调节作用于它下面所有轨道的画面。返回片段 id。
///
/// # Errors
/// 长度不为正。
pub fn add_adjustment(
    project: &mut Project,
    at: Time,
    duration: Time,
) -> Result<ClipId, EditError> {
    if duration <= Time::ZERO {
        return Err(EditError::ZeroLength);
    }
    let start = at.max(Time::ZERO);
    let main = crate::magnet::main_track(&project.timeline);
    let top = project
        .timeline
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Video)
        .filter(|t| Some(t.id) != main && !t.locked && t.is_free(start, start + duration, &[]))
        .map(|t| t.id);
    let mut next = project.clone();
    let track = match top {
        Some(t) => t,
        None => add_track(&mut next, TrackKind::Video)?,
    };
    let id = next.ids.clip();
    let clip = Clip {
        id,
        start,
        duration,
        content: ClipContent::Adjustment,
        gain_db: 0.0,
        fade_in: Time::ZERO,
        fade_out: Time::ZERO,
        transform: Transform::default(),
        animation: crate::Animation::default(),
        adjust: crate::ColorAdjust::NONE,
        disabled: false,
        transition_in: None,
        speed: 1.0,
        keep_pitch: true,
    };
    let ti = track_index(&next.timeline, track)?;
    next.timeline.tracks[ti].insert_sorted(Arc::new(clip));
    *project = next;
    Ok(id)
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
        content: ClipContent::Subtitle {
            text,
            motion: crate::TextMotion::default(),
        },
        gain_db: 0.0,
        fade_in: Time::ZERO,
        fade_out: Time::ZERO,
        transform: Transform::default(),
        animation: crate::Animation::default(),
        adjust: crate::ColorAdjust::NONE,
        disabled: false,
        transition_in: None,
        speed: 1.0,
        keep_pitch: true,
    };
    project.timeline.tracks[ti].insert_sorted(Arc::new(clip));
    Ok(id)
}

/// 设置字幕的入场 / 出场动画（剪映文字「动画」）。
///
/// # Errors
/// 任一片段不存在或不是字幕（这时什么都不改）。
pub fn set_text_motion(
    project: &mut Project,
    clips: &[ClipId],
    motion: crate::TextMotion,
) -> Result<(), EditError> {
    with_timeline(project, |_, tl| {
        for &id in clips {
            let (ti, ci) = locate(tl, id)?;
            let mut c = (*tl.tracks[ti].clips[ci]).clone();
            match &mut c.content {
                ClipContent::Subtitle { motion: m, .. } => *m = motion,
                _ => return Err(EditError::WrongTrackKind),
            }
            tl.tracks[ti].clips[ci] = Arc::new(c);
        }
        Ok(())
    })
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
        ClipContent::Subtitle { text: t, .. } => {
            *t = text;
            Ok(())
        }
        ClipContent::Media { .. } | ClipContent::Adjustment => Err(EditError::WrongTrackKind),
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
    fn append_audio_puts_only_the_sound_on_the_audio_track() {
        let (mut p, ids) = sample_project();
        // 视频素材的声音：接在音频轨 a1（0–8）后面
        let c = append_audio(&mut p, ids.video_asset).unwrap();
        assert_eq!(p.timeline.find_clip(c).unwrap().0, ids.audio_track);
        assert_eq!(clip(&p, c).start, secs(8));
        // 没有声音的素材不行
        let still = still_asset(&mut p);
        assert_eq!(append_audio(&mut p, still), Err(EditError::WrongTrackKind));
        assert_eq!(
            append_audio(&mut p, AssetId(999)),
            Err(EditError::AssetNotFound(AssetId(999)))
        );
    }

    #[test]
    fn split_all_at_skips_locked_tracks() {
        let (mut p, ids) = sample_project();
        set_track_flag(&mut p, ids.audio_track, TrackFlag::Locked, true).unwrap();
        let new = split_all_at(&mut p, secs(2));
        assert_eq!(new.len(), 2, "只切 v1、s1");
        assert_eq!(clip(&p, ids.a1).end(), secs(8));
    }

    #[test]
    fn locked_tracks_reject_edits_that_touch_them() {
        let (mut p, ids) = sample_project();
        set_track_flag(&mut p, ids.audio_track, TrackFlag::Locked, true).unwrap();
        let before = p.timeline.clone();
        // 动别的轨道没问题
        let mut other = p.clone();
        delete_clips(&mut other, &[ids.v1]).unwrap();
        assert_eq!(check_locked(&before, &other.timeline), Ok(()));
        // 动锁着的轨道（删片段、删轨道）都不行
        let mut touched = p.clone();
        delete_clips(&mut touched, &[ids.a1]).unwrap();
        assert_eq!(
            check_locked(&before, &touched.timeline),
            Err(EditError::TrackLocked)
        );
        let mut removed = p.clone();
        add_track(&mut removed, TrackKind::Audio).unwrap();
        remove_track(&mut removed, ids.audio_track).unwrap();
        assert_eq!(
            check_locked(&before, &removed.timeline),
            Err(EditError::TrackLocked)
        );
        // 解锁本身不算改动片段
        set_track_flag(&mut p, ids.audio_track, TrackFlag::Locked, false).unwrap();
        assert_eq!(check_locked(&before, &p.timeline), Ok(()));
        assert_eq!(
            set_track_flag(&mut p, TrackId(999), TrackFlag::Muted, true),
            Err(EditError::TrackNotFound(TrackId(999)))
        );
    }

    fn still_asset(p: &mut Project) -> AssetId {
        p.add_asset(Asset {
            id: AssetId(0),
            path: "C:/drafts/freeze.png".into(),
            name: "freeze.png".into(),
            duration: Time::from_secs(5),
            video: Some(VideoInfo {
                still: true,
                ..crate::testing::video_info()
            }),
            audio: None,
        })
    }

    #[test]
    fn freeze_splits_inserts_a_still_and_pushes_later_clips() {
        let (mut p, ids) = sample_project();
        p.settings.main_magnet = false;
        let still = still_asset(&mut p);
        let moved = Transform {
            scale: 0.5,
            mirror: true,
            ..Transform::default()
        };
        set_transform(&mut p, ids.v1, moved).unwrap();
        let frozen = insert_freeze(&mut p, ids.v1, secs(2), still, FREEZE_LENGTH).unwrap();
        let v: Vec<_> = p.timeline.tracks[1]
            .clips
            .iter()
            .map(|c| (c.start, c.end()))
            .collect();
        // v1 0–2 | 定格 2–5 | v1 右半 5–8 | v2 8–11
        assert_eq!(
            v,
            [
                (secs(0), secs(2)),
                (secs(2), secs(5)),
                (secs(5), secs(8)),
                (secs(8), secs(11))
            ]
        );
        let f = clip(&p, frozen);
        assert_eq!(f.asset(), Some(still));
        assert_eq!(f.transform, moved, "沿用原片段的画面属性");
        assert_eq!(clip(&p, ids.a1).start, secs(0), "别的轨不动");
        assert_valid(&p);
        // 在片段开头定格：不切，原片段往后挪
        let at_start = insert_freeze(&mut p, ids.v2, secs(8), still, FREEZE_LENGTH).unwrap();
        assert_eq!(clip(&p, at_start).start, secs(8));
        assert_eq!(clip(&p, ids.v2).start, secs(11));
        // 播放头不在片段上、不是视频片段
        let before = p.clone();
        assert_eq!(
            insert_freeze(&mut p, ids.v2, secs(30), still, FREEZE_LENGTH),
            Err(EditError::InvalidValue)
        );
        assert_eq!(
            insert_freeze(&mut p, ids.a1, secs(1), still, FREEZE_LENGTH),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(p, before);
    }

    #[test]
    fn adjustments_go_above_the_main_track() {
        let (mut p, ids) = sample_project();
        let a = add_adjustment(&mut p, secs(1), ADJUSTMENT_LENGTH).unwrap();
        // 只有主轨：在它上面新建一条视频轨
        let (track, c) = p.timeline.find_clip(a).unwrap();
        assert_ne!(track, ids.video_track);
        assert!(c.is_adjustment() && c.asset().is_none());
        assert_eq!((c.start, c.end()), (secs(1), secs(4)));
        let videos = p
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video)
            .count();
        // 那条轨后面空着：第二段接着放上去，不再新建
        let b = add_adjustment(&mut p, secs(5), ADJUSTMENT_LENGTH).unwrap();
        assert_eq!(p.timeline.find_clip(b).unwrap().0, track);
        // 重叠：再建一条
        add_adjustment(&mut p, secs(2), ADJUSTMENT_LENGTH).unwrap();
        assert_eq!(
            p.timeline
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Video)
                .count(),
            videos + 1
        );
        assert_valid(&p);
        // 调节片段不能变速、不能放到音频轨
        assert_eq!(set_speed(&mut p, a, 2.0), Err(EditError::WrongTrackKind));
        assert_eq!(
            move_clip(&mut p, a, ids.audio_track, secs(20)),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(
            add_adjustment(&mut p, secs(0), Time::ZERO),
            Err(EditError::ZeroLength)
        );
    }

    #[test]
    fn text_motion_only_on_subtitles() {
        let (mut p, ids) = sample_project();
        let m = crate::TextMotion {
            intro: crate::TextIn::FadeIn,
            outro: crate::TextOut::SlideDown,
        };
        set_text_motion(&mut p, &[ids.s1], m).unwrap();
        assert_eq!(clip(&p, ids.s1).text_motion(), Some(m));
        // 改文字不丢动画
        set_subtitle_text(&mut p, ids.s1, "新的".into()).unwrap();
        assert_eq!(clip(&p, ids.s1).text_motion(), Some(m));
        let before = p.clone();
        assert_eq!(
            set_text_motion(&mut p, &[ids.s1, ids.v1], crate::TextMotion::default()),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(p, before);
    }

    #[test]
    fn mirror_and_quarter_turns() {
        let (mut p, ids) = sample_project();
        set_mirror(&mut p, &[ids.v1, ids.v2], true).unwrap();
        assert!(clip(&p, ids.v1).transform.mirror && clip(&p, ids.v2).transform.mirror);
        assert_eq!(
            set_mirror(&mut p, &[ids.v1, ids.a1], false),
            Err(EditError::WrongTrackKind)
        );
        assert!(clip(&p, ids.v1).transform.mirror, "出错时什么都不改");
        for want in [90.0, 180.0, -90.0, 0.0] {
            rotate_quarter(&mut p, ids.v1, Time::ZERO).unwrap();
            assert!((clip(&p, ids.v1).transform.rotation - want).abs() < 1e-3);
        }
        assert_eq!(
            rotate_quarter(&mut p, ids.s1, Time::ZERO),
            Err(EditError::WrongTrackKind)
        );
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
                TrackKind::Subtitle,
                TrackKind::Video,
                TrackKind::Video,
                TrackKind::Audio
            ]
        );
        // 剪映：新视频轨在已有视频轨上面（主轨在最下），文字轨始终在最上
        assert_eq!(p.timeline.tracks[1].id, new);
        assert_eq!(p.timeline.tracks[2].id, ids.video_track);
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
                TrackKind::Subtitle,
                TrackKind::Video,
                TrackKind::Audio,
                TrackKind::Audio
            ]
        );
        assert_eq!(p.timeline.tracks[2].id, ids.audio_track);
        assert_eq!(p.timeline.tracks[3].id, new);
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

    #[test]
    fn trim_left_cuts_what_is_before_the_playhead() {
        let (mut p, ids) = sample_project();
        let all = [ids.v1, ids.v2, ids.a1, ids.s1];
        // 2 秒：v1（0–5）、a1（0–8）、s1（1–3）跨过它；v2（5–8）不跨，不动
        assert_eq!(trim_left_at(&mut p, &all, secs(2)), Ok(3));
        let v1 = clip(&p, ids.v1);
        assert_eq!((v1.start, v1.duration), (secs(2), secs(3)));
        assert!(matches!(v1.content, ClipContent::Media { source_in, .. } if source_in == secs(2)));
        assert_eq!(clip(&p, ids.s1).start, secs(2));
        assert_eq!(clip(&p, ids.v2).start, secs(5));
        assert_valid(&p);
    }

    #[test]
    fn trim_right_cuts_what_is_after_the_playhead() {
        let (mut p, ids) = sample_project();
        assert_eq!(trim_right_at(&mut p, &[ids.v2, ids.s1], secs(6)), Ok(1));
        let v2 = clip(&p, ids.v2);
        assert_eq!((v2.start, v2.end()), (secs(5), secs(6)));
        assert_eq!(clip(&p, ids.s1).end(), secs(3), "不跨过播放头的不动");
        // 一个都不跨：什么也不改
        let before = p.clone();
        assert_eq!(trim_right_at(&mut p, &[ids.s1], secs(6)), Ok(0));
        assert_eq!(p, before);
        assert_eq!(
            trim_right_at(&mut p, &[ClipId(999)], secs(6)),
            Err(EditError::ClipNotFound(ClipId(999)))
        );
    }

    #[test]
    fn insert_slots_keep_each_kind_together() {
        let (mut p, _) = sample_project();
        add_track(&mut p, TrackKind::Video).unwrap();
        // [S, V, V, A]：视频轨能插在 1、2、3（视频块的上沿、中间、下沿），音频轨能插在 3、4
        assert_eq!(insert_slots(&p.timeline, TrackKind::Video), Some(1..=3));
        assert_eq!(insert_slots(&p.timeline, TrackKind::Audio), Some(3..=4));
        assert_eq!(insert_slots(&p.timeline, TrackKind::Subtitle), None);
    }

    #[test]
    fn dragging_into_a_slot_creates_a_track_there() {
        let (mut p, ids) = sample_project();
        // 把 v2（5–8）拖到字幕轨和视频轨之间（第 1 个缝）、放在 2 秒：新视频轨在视频轨最上面
        let track = move_clip_to_new_track(&mut p, ids.v2, 1, secs(2)).unwrap();
        assert_eq!(p.timeline.tracks[1].id, track);
        assert_eq!(p.timeline.tracks[1].kind, TrackKind::Video);
        let (on, c) = p.timeline.find_clip(ids.v2).unwrap();
        assert_eq!((on, c.start), (track, secs(2)));
        assert_valid(&p);
        // 音频片段拖到最下面：新音频轨在最后
        let n = p.timeline.tracks.len();
        let audio = move_clip_to_new_track(&mut p, ids.a1, n, secs(0)).unwrap();
        assert_eq!(p.timeline.tracks.last().unwrap().id, audio);
        // 插到别的类型中间：拒绝，工程不变
        let before = p.clone();
        assert_eq!(
            move_clip_to_new_track(&mut p, ids.v1, 0, secs(0)),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(
            move_clip_to_new_track(&mut p, ids.s1, 0, secs(0)),
            Err(EditError::WrongTrackKind),
            "字幕轨只有一条"
        );
        assert_eq!(p, before);
    }

    #[test]
    fn aspect_keeps_the_short_side() {
        let (mut p, _) = sample_project();
        let size = |proj: &Project| (proj.settings.width, proj.settings.height);
        assert_eq!(size(&p), (1920, 1080));
        set_aspect(&mut p, Some((9, 16))).unwrap();
        assert_eq!(size(&p), (1080, 1920), "竖屏");
        set_aspect(&mut p, Some((1, 1))).unwrap();
        assert_eq!(size(&p), (1080, 1080));
        set_aspect(&mut p, Some((4, 3))).unwrap();
        assert_eq!(size(&p), (1440, 1080));
        // 宽高都取偶数（yuv420p 的要求）
        set_aspect(&mut p, Some((235, 100))).unwrap();
        assert_eq!(size(&p), (2538, 1080));
        // 原始：回到第一个视频素材的比例（示例素材是 1920×1080）
        set_aspect(&mut p, None).unwrap();
        assert_eq!(size(&p), (1920, 1080));
        assert_eq!(
            set_aspect(&mut p, Some((0, 9))),
            Err(EditError::InvalidValue)
        );
    }

    #[test]
    fn keyframes_per_property_group() {
        use crate::{KeyGroup, Prop};
        let (mut p, ids) = sample_project();
        // 只给「缩放」打关键帧：别的属性仍是固定值
        add_keyframe_props(&mut p, ids.v1, secs(0), KeyGroup::Scale.props()).unwrap();
        let anim = &clip(&p, ids.v1).animation;
        assert_eq!(anim.props().collect::<Vec<_>>(), [Prop::Scale]);
        // 2 秒处改位置和缩放：缩放有关键帧 → 打新帧；位置没有 → 改固定值
        let moved = Transform {
            x: 0.2,
            scale: 0.5,
            ..Transform::default()
        };
        apply_transform_at(&mut p, ids.v1, secs(2), moved).unwrap();
        let c1 = clip(&p, ids.v1);
        assert_eq!(c1.animation.keys(Prop::Scale).len(), 2);
        assert_eq!(c1.animation.keys(Prop::X), []);
        assert!((c1.transform.x - 0.2).abs() < 1e-6);
        // 位置一行打帧：X、Y 一起
        add_keyframe_props(&mut p, ids.v1, secs(1), KeyGroup::Position.props()).unwrap();
        let c2 = clip(&p, ids.v1);
        assert_eq!(c2.animation.times_of(KeyGroup::Position.props()), [secs(1)]);
        assert_eq!(c2.animation.keys(Prop::Y).len(), 1);
        // 删掉缩放在 2 秒的帧：位置的帧不受影响
        remove_keyframe_props(&mut p, ids.v1, secs(2), KeyGroup::Scale.props()).unwrap();
        let c3 = clip(&p, ids.v1);
        assert_eq!(c3.animation.times_of(KeyGroup::Scale.props()), [secs(0)]);
        assert_eq!(c3.animation.times_of(KeyGroup::Position.props()), [secs(1)]);
        // 删光缩放的帧：固定值取删之前那一刻的值（1.0），画面不跳
        remove_keyframe_props(&mut p, ids.v1, secs(0), KeyGroup::Scale.props()).unwrap();
        let c4 = clip(&p, ids.v1);
        assert_eq!(c4.animation.keys(Prop::Scale), []);
        assert!((c4.transform.scale - 1.0).abs() < 1e-6);
        assert_eq!(
            remove_keyframe_props(&mut p, ids.v1, secs(0), KeyGroup::Scale.props()),
            Err(EditError::NoKeyframe)
        );
        assert_valid(&p);
    }

    #[test]
    fn resolution_keeps_ratio_and_frame_rate_is_free() {
        let (mut p, _) = sample_project();
        let size = |proj: &Project| (proj.settings.width, proj.settings.height);
        set_resolution(&mut p, 720).unwrap();
        assert_eq!(size(&p), (1280, 720));
        set_aspect(&mut p, Some((9, 16))).unwrap();
        set_resolution(&mut p, 2160).unwrap();
        assert_eq!(size(&p), (2160, 3840), "竖屏：短边是宽");
        assert_eq!(set_resolution(&mut p, 0), Err(EditError::InvalidValue));
        set_frame_rate(&mut p, crate::FrameRate::new(60, 1)).unwrap();
        assert_eq!(p.settings.frame_rate, crate::FrameRate::new(60, 1));
        assert_eq!(
            set_frame_rate(&mut p, crate::FrameRate::new(0, 1)),
            Err(EditError::InvalidValue)
        );
    }

    #[test]
    fn adjust_only_on_video_clips_and_validated() {
        let (mut p, ids) = sample_project();
        let warm = crate::ColorAdjust {
            temperature: 0.5,
            ..crate::ColorAdjust::NONE
        };
        set_adjust(&mut p, ids.v1, warm).unwrap();
        assert_eq!(clip(&p, ids.v1).adjust, warm);
        assert_eq!(
            set_adjust(&mut p, ids.a1, warm),
            Err(EditError::WrongTrackKind)
        );
        let bad = crate::ColorAdjust {
            contrast: 2.0,
            ..crate::ColorAdjust::NONE
        };
        assert_eq!(
            set_adjust(&mut p, ids.v1, bad),
            Err(EditError::InvalidValue)
        );
        assert_valid(&p);
    }

    #[test]
    fn disabled_clips_stay_but_do_not_render() {
        let (mut p, ids) = sample_project();
        set_disabled(&mut p, &[ids.v1, ids.s1], true).unwrap();
        assert!(clip(&p, ids.v1).disabled && clip(&p, ids.s1).disabled);
        assert!(
            crate::compose::resolve_layers(&p, secs(1)).is_empty(),
            "停用的画面不合成"
        );
        assert_eq!(
            crate::compose::resolve_layers(&p, secs(6)).len(),
            1,
            "别的片段照常"
        );
        assert!(crate::srt::export_srt(&p).is_empty(), "停用的字幕不导出");
        set_disabled(&mut p, &[ids.v1], false).unwrap();
        assert!(!clip(&p, ids.v1).disabled);
        assert_eq!(
            set_disabled(&mut p, &[ClipId(999)], true),
            Err(EditError::ClipNotFound(ClipId(999)))
        );
        assert_valid(&p);
    }

    #[test]
    fn copy_paste_keeps_relative_layout_and_tracks() {
        let (mut p, ids) = sample_project();
        p.settings.main_magnet = false;
        // v2（视频 5–8）和 s1（字幕 1–3）：相对最早的 s1 是 4 秒和 0 秒
        let copied = copy_clips(&p, &[ids.v2, ids.s1]).unwrap();
        let pasted = paste_clips(&mut p, &copied, secs(10)).unwrap();
        assert_eq!(pasted.len(), 2);
        let (vt, v) = p.timeline.find_clip(pasted[0]).unwrap();
        assert_eq!(
            (vt, v.start, v.end()),
            (ids.video_track, secs(14), secs(17))
        );
        let (st, s) = p.timeline.find_clip(pasted[1]).unwrap();
        assert_eq!(
            (st, s.start, s.subtitle_text()),
            (ids.subtitle_track, secs(10), Some("你好"))
        );
        // 放不下：整次失败，什么都不改
        let before = p.clone();
        assert_eq!(
            paste_clips(&mut p, &copied, secs(0)),
            Err(EditError::Overlap)
        );
        assert_eq!(p, before);
        assert_valid(&p);
    }

    #[test]
    fn paste_into_main_track_with_magnet_inserts_by_position() {
        let (mut p, ids) = sample_project();
        assert!(p.settings.main_magnet);
        let copied = copy_clips(&p, &[ids.v2]).unwrap();
        // 播放头在 1 秒（v1 中点 2.5 之前）：插到最前面，主轨仍首尾相接
        let pasted = paste_clips(&mut p, &copied, secs(1)).unwrap();
        let main = p.timeline.track(ids.video_track).unwrap();
        let order: Vec<_> = main.clips.iter().map(|c| (c.id, c.start)).collect();
        assert_eq!(
            order,
            [(pasted[0], secs(0)), (ids.v1, secs(3)), (ids.v2, secs(8))]
        );
    }

    #[test]
    fn attributes_copy_to_other_video_clips_only() {
        let (mut p, ids) = sample_project();
        let look = Transform {
            scale: 0.5,
            x: 0.2,
            ..Transform::default()
        };
        set_transform(&mut p, ids.v1, look).unwrap();
        let warm = crate::ColorAdjust {
            temperature: 0.3,
            ..crate::ColorAdjust::NONE
        };
        set_adjust(&mut p, ids.v1, warm).unwrap();
        let attrs = copy_attributes(&p, ids.v1).unwrap();
        // 字幕不是视频片段：跳过
        assert_eq!(paste_attributes(&mut p, &[ids.v2, ids.s1], &attrs), Ok(1));
        let v2 = clip(&p, ids.v2);
        assert_eq!((v2.transform, v2.adjust), (look, warm));
        assert_eq!(copy_attributes(&p, ids.s1), Err(EditError::WrongTrackKind));
    }

    #[test]
    fn speed_keeps_the_content_and_changes_the_length() {
        let (mut p, ids) = sample_project();
        p.settings.main_magnet = false;
        // v2：时间轴 5–8，素材 5–8。2 倍速后只占 1.5 秒，素材还是 5–8
        set_speed(&mut p, ids.v2, 2.0).unwrap();
        let v2 = clip(&p, ids.v2);
        assert_eq!((v2.start, v2.duration), (secs(5), Time::from_secs_f64(1.5)));
        assert_eq!(
            v2.source_at(Time::from_secs_f64(0.75)),
            Time::from_secs_f64(6.5)
        );
        assert_eq!(v2.source_span(), secs(3));
        assert_valid(&p);
        // 变慢：v1（0–5）0.5 倍要 10 秒，撞上 v2：磁吸关着就报错
        assert_eq!(set_speed(&mut p, ids.v1, 0.5), Err(EditError::Overlap));
        assert_eq!(
            set_speed(&mut p, ids.v1, 20.0),
            Err(EditError::InvalidValue)
        );
        assert_eq!(
            set_speed(&mut p, ids.s1, 2.0),
            Err(EditError::WrongTrackKind)
        );
        // 切开变速后的片段：右半段的素材起点按倍数算
        let right = split_at(&mut p, ids.v2, Time::from_secs_f64(5.5)).unwrap();
        assert_eq!(clip(&p, right).source_in(), secs(6));
        assert_valid(&p);
    }

    #[test]
    fn slowing_down_on_the_magnet_main_track_pushes_later_clips() {
        let (mut p, ids) = sample_project();
        assert!(p.settings.main_magnet);
        set_speed(&mut p, ids.v1, 0.5).unwrap();
        assert_eq!(clip(&p, ids.v1).duration, secs(10));
        assert_eq!(clip(&p, ids.v2).start, secs(10), "后面的整体往后推");
        // 修剪按倍数换算素材：剪掉前 2 秒时间轴 = 1 秒素材
        trim_start(&mut p, ids.v1, secs(2)).unwrap();
        assert_eq!(clip(&p, ids.v1).source_in(), secs(1));
        assert_valid(&p);
    }

    #[test]
    fn keyframes_stretch_with_speed() {
        let (mut p, ids) = sample_project();
        p.settings.main_magnet = false;
        add_keyframe(&mut p, ids.v2, secs(2)).unwrap();
        set_speed(&mut p, ids.v2, 2.0).unwrap();
        assert_eq!(clip(&p, ids.v2).animation.times(), [secs(1)]);
    }

    #[test]
    fn pitch_switch_is_per_clip() {
        let (mut p, ids) = sample_project();
        assert!(clip(&p, ids.v1).keep_pitch, "默认保持音调");
        set_keep_pitch(&mut p, ids.v1, false).unwrap();
        assert!(!clip(&p, ids.v1).keep_pitch);
        assert_eq!(
            set_keep_pitch(&mut p, ids.s1, false),
            Err(EditError::WrongTrackKind)
        );
    }
}
