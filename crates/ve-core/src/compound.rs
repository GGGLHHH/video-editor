//! 复合片段（剪映「新建复合片段」）：选中的几段打包成时间轴上的一段，里面保留各自的轨道；
//! 可以整段移动、裁剪、分割，也能「解除复合片段」放回原样。
//!
//! 播放和导出不认识复合片段：交给它们之前先 [`flatten`]——把每段复合片段里面的轨道按它在时间轴上的
//! 位置和裁剪范围摊开（画面轨插在它所在的那条视频轨上面，声音轨放到最后，字幕轨放到最前面），
//! 合成、混音、字幕都照常算。
//!
//! 第一版：复合片段自己的画面属性、调色、音量这些不生效（属性面板也不给改），只按时间放里面的东西。

use crate::edit::EditError;
use crate::{Clip, ClipContent, ClipId, Project, Time, Track, TrackId, TrackKind};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

/// 复合片段里面的东西：几条轨道（和时间轴一样从上到下），时间从 0 开始。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Compound {
    /// 名字（时间轴上显示）。
    pub name: String,
    /// 轨道。
    pub tracks: Vec<Track>,
}

impl Compound {
    /// 里面最后一段的终点。
    #[must_use]
    pub fn duration(&self) -> Time {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.end()))
            .max()
            .unwrap_or(Time::ZERO)
    }

    /// 里面有画面（素材、调节、特效）：复合片段放在视频轨上，否则放音频轨。
    #[must_use]
    pub fn has_picture(&self) -> bool {
        self.tracks
            .iter()
            .any(|t| t.kind == TrackKind::Video && !t.clips.is_empty())
    }
}

/// 一段新建的复合片段（默认属性）。
fn compound_clip(id: ClipId, start: Time, inner: Arc<Compound>) -> Clip {
    let duration = inner.duration();
    Clip {
        id,
        start,
        duration,
        content: ClipContent::Compound {
            inner,
            source_in: Time::ZERO,
        },
        gain_db: 0.0,
        fade_in: Time::ZERO,
        fade_out: Time::ZERO,
        transform: crate::Transform::default(),
        animation: crate::Animation::default(),
        adjust: crate::ColorAdjust::NONE,
        disabled: false,
        transition_in: None,
        speed: 1.0,
        keep_pitch: true,
        label: None,
        look: crate::Look::NONE,
        reversed: None,
        curve: None,
        grade: None,
        audio_fx: crate::AudioFx::NONE,
        frame_blend: false,
        stabilized: None,
        input_space: None,
    }
}

/// 新建复合片段：选中的片段（至少要有一段素材）按原来的轨道上下关系打包，放在最上面一条
/// 放得下的同类轨道上（视频优先），返回新片段。
///
/// # Errors
/// 没选素材片段；有片段在锁着的轨道上。
pub fn make_compound(project: &mut Project, clips: &[ClipId]) -> Result<ClipId, EditError> {
    let chosen: HashSet<ClipId> = clips.iter().copied().collect();
    let picked: Vec<(usize, Arc<Clip>)> = project
        .timeline
        .tracks
        .iter()
        .enumerate()
        .flat_map(|(ti, t)| {
            t.clips
                .iter()
                .filter(|c| chosen.contains(&c.id))
                .map(move |c| (ti, Arc::clone(c)))
        })
        .collect();
    if !picked
        .iter()
        .any(|(_, c)| c.asset().is_some() || c.compound().is_some())
    {
        return Err(EditError::WrongTrackKind);
    }
    if picked
        .iter()
        .any(|&(ti, _)| project.timeline.tracks[ti].locked)
    {
        return Err(EditError::TrackLocked);
    }
    let start = picked
        .iter()
        .map(|(_, c)| c.start)
        .min()
        .unwrap_or(Time::ZERO);
    let end = picked
        .iter()
        .map(|(_, c)| c.end())
        .max()
        .unwrap_or(Time::ZERO);
    let mut next = project.clone();
    // 里面的轨道：原来有选中片段的轨道，各一条，上下顺序不变
    let mut inner_tracks = Vec::new();
    for (ti, t) in project.timeline.tracks.iter().enumerate() {
        let mine: Vec<Arc<Clip>> = picked
            .iter()
            .filter(|(i, _)| *i == ti)
            .map(|(_, c)| {
                let mut moved = (**c).clone();
                moved.start = c.start - start;
                Arc::new(moved)
            })
            .collect();
        if mine.is_empty() {
            continue;
        }
        let mut track = Track::new(next.ids.track(), t.kind);
        track.clips = mine;
        inner_tracks.push(track);
    }
    for t in &mut next.timeline.tracks {
        t.clips.retain(|c| !chosen.contains(&c.id));
    }
    let n = project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.compound().is_some())
        .count();
    let inner = Arc::new(Compound {
        name: format!("复合片段{}", n + 1),
        tracks: inner_tracks,
    });
    let kind = if inner.has_picture() {
        TrackKind::Video
    } else {
        TrackKind::Audio
    };
    // 放回原来那几条里最上面一条放得下的，没有就新建一条
    let host = picked
        .iter()
        .map(|&(ti, _)| ti)
        .chain(0..next.timeline.tracks.len())
        .find(|&ti| {
            let t = &next.timeline.tracks[ti];
            t.kind == kind && !t.locked && t.is_free(start, end, &[])
        });
    let ti = index_or_add(&mut next, host, kind)?;
    let id = next.ids.clip();
    next.timeline.tracks[ti].insert_sorted(Arc::new(compound_clip(id, start, inner)));
    *project = next;
    Ok(id)
}

/// 找到的轨道下标；没找到就新建一条 `kind` 轨道，返回它的下标。
fn index_or_add(
    project: &mut Project,
    found: Option<usize>,
    kind: TrackKind,
) -> Result<usize, EditError> {
    if let Some(ti) = found {
        return Ok(ti);
    }
    let id = crate::edit::add_track(project, kind)?;
    project
        .timeline
        .tracks
        .iter()
        .position(|t| t.id == id)
        .ok_or(EditError::TrackNotFound(id))
}

/// 一条轨道上落在 `[from, to)` 里的部分（跨边界的片段在边界处切开），起点平移 `shift`；
/// 留下的每段的 id 由 `id_of(原 id, 第几段)` 给。
fn window(
    track: &Track,
    from: Time,
    to: Time,
    shift: Time,
    id_of: &mut dyn FnMut(ClipId, usize) -> ClipId,
) -> Vec<Arc<Clip>> {
    let mut tl = crate::Timeline {
        tracks: vec![track.clone()],
        markers: Vec::new(),
        range: None,
        cover: None,
    };
    // 切开用的临时 id（切完马上换掉）
    let mut tmp = u64::MAX;
    for at in [from, to] {
        let crossing: Vec<ClipId> = tl.tracks[0]
            .clips
            .iter()
            .filter(|c| c.start < at && at < c.end())
            .map(|c| c.id)
            .collect();
        for c in crossing {
            tmp -= 1;
            let _ = crate::edit::split_in(&mut tl, c, at, ClipId(tmp));
        }
    }
    let mut out = Vec::new();
    for (i, c) in tl.tracks[0]
        .clips
        .iter()
        .filter(|c| c.start >= from && c.end() <= to)
        .enumerate()
    {
        let original = if c.id.0 > u64::MAX - 1_000_000 {
            // 切出来的右半段：沿用切之前那段的 id 计算
            track
                .clips
                .iter()
                .find(|o| o.start < c.start && c.start < o.end())
                .map_or(c.id, |o| o.id)
        } else {
            c.id
        };
        let mut piece = (**c).clone();
        piece.id = id_of(original, i);
        piece.start = c.start + shift;
        out.push(Arc::new(piece));
    }
    out
}

/// 解除复合片段：把里面的片段按现在的位置和裁剪放回时间轴（同类轨道放得下就放，否则新建轨道），
/// 返回放回去的片段。
///
/// # Errors
/// 不是复合片段；轨道锁着；字幕放不下（字幕轨只有一条）。
pub fn break_compound(project: &mut Project, clip: ClipId) -> Result<Vec<ClipId>, EditError> {
    let (track_id, c) = project
        .timeline
        .find_clip(clip)
        .ok_or(EditError::ClipNotFound(clip))?;
    let Some((inner, source_in)) = c.compound() else {
        return Err(EditError::WrongTrackKind);
    };
    let inner = Arc::clone(inner);
    let (start, duration) = (c.start, c.duration);
    if project.timeline.track(track_id).is_some_and(|t| t.locked) {
        return Err(EditError::TrackLocked);
    }
    let mut next = project.clone();
    for t in &mut next.timeline.tracks {
        t.clips.retain(|x| x.id != clip);
    }
    let mut placed = Vec::new();
    for it in &inner.tracks {
        let mut ids = next.ids.clone();
        let pieces = window(
            it,
            source_in,
            source_in + duration,
            start - source_in,
            &mut |_, _| ids.clip(),
        );
        next.ids = ids;
        if pieces.is_empty() {
            continue;
        }
        let fits = |t: &Track| {
            t.kind == it.kind
                && !t.locked
                && pieces.iter().all(|p| t.is_free(p.start, p.end(), &[]))
        };
        // 原来那条（复合片段所在的）优先，再从上往下找
        let found = next
            .timeline
            .tracks
            .iter()
            .position(|t| t.id == track_id && fits(t))
            .or_else(|| next.timeline.tracks.iter().position(fits));
        let ti = index_or_add(&mut next, found, it.kind)?;
        for p in pieces {
            placed.push(p.id);
            next.timeline.tracks[ti].insert_sorted(p);
        }
    }
    *project = next;
    Ok(placed)
}

/// 合成两个编号：摊开的片段 / 轨道要有稳定又不和真实编号撞的 id（解码器按片段 id 缓存）。
const fn synth(outer: u64, inner: u64) -> u64 {
    0x4000_0000_0000_0000
        | ((outer.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ inner) & 0x3FFF_FFFF_FFFF_FFFF)
}

/// 有没有复合片段。
#[must_use]
pub fn has_compound(project: &Project) -> bool {
    project
        .timeline
        .tracks
        .iter()
        .any(|t| t.clips.iter().any(|c| c.compound().is_some()))
}

/// 摊开一组轨道（从上到下）：复合片段换成里面的轨道。
fn flatten_tracks(tracks: &[Track]) -> Vec<Track> {
    let mut subtitles = Vec::new();
    let mut middle = Vec::new();
    let mut audio = Vec::new();
    for t in tracks {
        let mut host = t.clone();
        host.clips.retain(|c| c.compound().is_none());
        let mut above = Vec::new();
        for c in t.clips.iter().filter(|c| c.enabled()) {
            let Some((inner, source_in)) = c.compound() else {
                continue;
            };
            let outer = c.id.0;
            for it in flatten_tracks(&inner.tracks) {
                let pieces = window(
                    &it,
                    source_in,
                    source_in + c.duration,
                    c.start - source_in,
                    // 一段原片段在范围里只留一块：按原 id 合成，裁剪时 id 不变
                    &mut |id, _| ClipId(synth(outer, id.0)),
                );
                let mut track = it.clone();
                track.id = TrackId(synth(outer, it.id.0));
                track.clips = pieces;
                track.hidden |= t.hidden;
                track.muted |= t.muted;
                track.locked = false;
                match it.kind {
                    TrackKind::Video => above.push(track),
                    TrackKind::Audio => audio.push(track),
                    TrackKind::Subtitle => subtitles.push(track),
                }
            }
        }
        middle.extend(above);
        middle.push(host);
    }
    // 同类轨道挨在一起：字幕最前，声音最后
    let (subs_here, rest): (Vec<Track>, Vec<Track>) = middle
        .into_iter()
        .partition(|t| t.kind == TrackKind::Subtitle);
    let (audio_here, video): (Vec<Track>, Vec<Track>) =
        rest.into_iter().partition(|t| t.kind == TrackKind::Audio);
    subtitles
        .into_iter()
        .chain(subs_here)
        .chain(video)
        .chain(audio_here)
        .chain(audio)
        .collect()
}

/// 给播放和导出用的工程：复合片段都摊开（没有复合片段时返回 `None`，直接用原来的）。
#[must_use]
pub fn flatten(project: &Project) -> Option<Project> {
    if !has_compound(project) {
        return None;
    }
    let mut out = project.clone();
    out.timeline.tracks = flatten_tracks(&project.timeline.tracks);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::sample_project;

    fn secs(s: i64) -> Time {
        Time::from_secs(s)
    }

    #[test]
    fn making_and_breaking_a_compound_round_trips() {
        let (mut p, ids) = sample_project();
        let before = p.clone();
        // v1（视频 0–5）、a1（音频 0–8）、s1（字幕 1–3）打包
        let cc = make_compound(&mut p, &[ids.v1, ids.a1, ids.s1]).unwrap();
        let (track, c) = p.timeline.find_clip(cc).unwrap();
        assert_eq!(p.timeline.track(track).unwrap().kind, TrackKind::Video);
        assert_eq!((c.start, c.duration), (Time::ZERO, secs(8)));
        let (inner, _) = c.compound().unwrap();
        assert_eq!(inner.tracks.len(), 3, "字幕、视频、音频各一条");
        assert!(p.timeline.find_clip(ids.v1).is_none());
        assert!(p.timeline.find_clip(ids.a1).is_none());
        p.timeline.check_invariants(&p.assets).unwrap();
        // 解除：放回原处（id 是新的），内容和位置一样
        let back = break_compound(&mut p, cc).unwrap();
        assert_eq!(back.len(), 3);
        let spans = |proj: &Project| {
            let mut v: Vec<(TrackKind, Time, Time)> = proj
                .timeline
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter().map(move |x| (t.kind, x.start, x.end())))
                .collect();
            v.sort_by_key(|x| (x.1, x.2));
            v
        };
        assert_eq!(spans(&p), spans(&before));
        p.timeline.check_invariants(&p.assets).unwrap();
    }

    #[test]
    fn only_subtitles_cannot_be_a_compound_and_locked_tracks_refuse() {
        let (mut p, ids) = sample_project();
        assert_eq!(
            make_compound(&mut p, &[ids.s1]),
            Err(EditError::WrongTrackKind)
        );
        crate::edit::set_track_flag(
            &mut p,
            ids.audio_track,
            crate::edit::TrackFlag::Locked,
            true,
        )
        .unwrap();
        assert_eq!(
            make_compound(&mut p, &[ids.v1, ids.a1]),
            Err(EditError::TrackLocked)
        );
        let cc = make_compound(&mut p, &[ids.v1, ids.v2]).unwrap();
        assert_eq!(
            break_compound(&mut p, ids.a1),
            Err(EditError::WrongTrackKind)
        );
        assert!(break_compound(&mut p, cc).is_ok());
    }

    #[test]
    fn flatten_puts_the_inner_tracks_where_the_compound_sits() {
        let (mut p, ids) = sample_project();
        assert!(flatten(&p).is_none(), "没有复合片段就不摊");
        let cc = make_compound(&mut p, &[ids.v1, ids.v2, ids.a1]).unwrap();
        // 挪到 2 秒、剪掉里面开头 1 秒
        crate::edit::move_clip(&mut p, cc, ids.video_track, secs(2)).unwrap();
        crate::edit::trim_start(&mut p, cc, secs(3)).unwrap();
        let flat = flatten(&p).unwrap();
        assert!(!has_compound(&flat));
        // 3 秒处 = 里面 1 秒：v1 的第 1 秒
        let layers = crate::compose::resolve_layers(&flat, secs(3));
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].source_time, secs(1));
        // 8 秒处 = 里面 6 秒：v2（素材 5–8 秒）的第 1 秒 → 素材 6 秒
        let later = crate::compose::resolve_layers(&flat, secs(8));
        assert_eq!(later[0].source_time, secs(6));
        // 复合片段之前没有画面；声音也跟着挪
        assert_eq!(crate::compose::resolve_layers(&flat, secs(2)), []);
        let audio: Vec<(Time, Time)> = flat
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .flat_map(|t| t.clips.iter().map(|c| (c.start, c.source_in())))
            .collect();
        assert_eq!(audio, [(secs(3), secs(1))]);
        // 摊开后 id 稳定（解码器按它缓存）
        let again = flatten(&p).unwrap();
        assert_eq!(again, flat);
    }

    #[test]
    fn a_split_compound_flattens_both_halves_with_distinct_ids() {
        let (mut p, ids) = sample_project();
        let cc = make_compound(&mut p, &[ids.v1, ids.v2]).unwrap();
        let right = crate::edit::split_at(&mut p, cc, secs(4)).unwrap();
        let (_, r) = p.timeline.find_clip(right).unwrap();
        assert_eq!(r.compound().unwrap().1, secs(4));
        let flat = flatten(&p).unwrap();
        let layers_left = crate::compose::resolve_layers(&flat, secs(3));
        let layers_right = crate::compose::resolve_layers(&flat, secs(4));
        assert_eq!(layers_left[0].source_time, secs(3));
        assert_eq!(layers_right[0].source_time, secs(4));
        assert_ne!(layers_left[0].clip, layers_right[0].clip);
        p.timeline.check_invariants(&p.assets).unwrap();
    }
}
