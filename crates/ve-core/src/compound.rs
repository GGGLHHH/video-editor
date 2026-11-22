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

/// 复合片段能不能放在 `kind` 轨道上：有画面的只能放视频轨（放音频轨摊开后画面会跑到最底下），
/// 只有声音的视频轨、音频轨都行。
#[must_use]
pub fn fits_track(inner: &Compound, kind: TrackKind) -> bool {
    match kind {
        TrackKind::Video => true,
        TrackKind::Audio => !inner.has_picture(),
        TrackKind::Subtitle => false,
    }
}

/// 对 `tracks` 里的每一段（包括复合片段里面的，一层层往下）调用 `f`。
pub fn for_each_clip<'a>(tracks: &'a [Track], f: &mut dyn FnMut(&'a Clip)) {
    for c in tracks.iter().flat_map(|t| t.clips.iter()) {
        f(c);
        if let Some((inner, _)) = c.compound() {
            for_each_clip(&inner.tracks, f);
        }
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
        mask_text: None,
        parent: None,
        audio_split: false,
        motion: crate::motion::ClipMotion::NONE,
        group: None,
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
        // 隐藏 / 静音跟着走（不然从静音轨上打包后就有声音了）
        track.hidden = t.hidden;
        track.muted = t.muted;
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

/// 复合片段里面的时间换到外面：里面 `from` 对着外面 `origin`，里面走 `speed` 秒外面走 1 秒
/// （复合片段变速）。
#[derive(Clone, Copy, Debug)]
struct Retime {
    from: Time,
    origin: Time,
    speed: f32,
    /// 复合片段变了速时，它自己的「保持音调」管里面所有的声音（两层快慢乘在一起，分不开）。
    keep_pitch: Option<bool>,
}

impl Retime {
    /// 复合片段 `c`（里面从 `source_in` 开始用）的换算。
    fn of(c: &Clip, source_in: Time) -> Self {
        #[expect(clippy::float_cmp, reason = "和 1 倍精确比较")]
        let sped = c.speed != 1.0;
        Self {
            from: source_in,
            origin: c.start,
            speed: c.speed,
            keep_pitch: sped.then_some(c.keep_pitch),
        }
    }

    fn at(self, t: Time) -> Time {
        self.origin + (t - self.from).scaled(1.0 / f64::from(self.speed))
    }

    /// 一段片段换到外面：位置、长度、变速倍数、淡入淡出、关键帧、转场和入场 / 出场动画的时长；
    /// 外面套着的几层复合片段（[`Clip::parent`]）一样换。
    fn clip(self, c: &mut Clip) {
        let k = 1.0 / f64::from(self.speed);
        let (start, end) = (self.at(c.start), self.at(c.end()));
        c.start = start;
        c.duration = end - start;
        if c.asset().is_some() || c.compound().is_some() {
            c.speed *= self.speed;
            if let Some(keep) = self.keep_pitch {
                c.keep_pitch = keep;
            }
        }
        c.fade_in = c.fade_in.scaled(k);
        c.fade_out = c.fade_out.scaled(k);
        c.animation.scale_times(k);
        if let Some(t) = &mut c.transition_in {
            t.duration = t.duration.scaled(k);
        }
        for m in [&mut c.motion.intro, &mut c.motion.outro]
            .into_iter()
            .flatten()
        {
            m.duration = m.duration.scaled(k);
        }
        if let Some(p) = &c.parent {
            let mut p = (**p).clone();
            self.clip(&mut p);
            c.parent = Some(Arc::new(p));
        }
    }
}

/// 复合片段里面所有素材片段和复合片段（摊开后）的变速倍数的范围；里面什么都没有时 `None`。
fn inner_speeds(inner: &Compound) -> Option<(f32, f32)> {
    let direct = inner.tracks.iter().flat_map(|t| t.clips.iter());
    let flat = flatten_tracks(&inner.tracks);
    let deep = flat.iter().flat_map(|t| t.clips.iter());
    direct
        .chain(deep)
        .filter(|c| c.asset().is_some() || c.compound().is_some())
        .map(|c| c.speed)
        .fold(None, |acc, s| {
            Some(acc.map_or((s, s), |(lo, hi): (f32, f32)| (lo.min(s), hi.max(s))))
        })
}

/// 复合片段 `c` 能不能变成 `speed` 倍速：里面每段乘上去都还在变速范围里。
pub(crate) fn speed_fits(c: &Clip, speed: f32) -> bool {
    let Some((inner, _)) = c.compound() else {
        return true;
    };
    let (lo, hi) = crate::timeline::SPEED_RANGE;
    let eps = 1e-4;
    inner_speeds(inner).is_none_or(|(a, b)| a * speed >= lo - eps && b * speed <= hi + eps)
}

/// 一条轨道上落在 `[from, to)`（里面的时间）里的部分（跨边界的片段在边界处切开），按 `retime`
/// 换到外面；留下的每段的 id 由 `id_of(原 id, 第几段)` 给。
fn window(
    track: &Track,
    from: Time,
    to: Time,
    retime: Retime,
    id_of: &mut dyn FnMut(ClipId, usize) -> ClipId,
) -> Vec<Arc<Clip>> {
    let mut tl = crate::Timeline {
        tracks: vec![track.clone()],
        markers: Vec::new(),
        marker_colors: Vec::new(),
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
        retime.clip(&mut piece);
        out.push(Arc::new(piece));
    }
    out
}

/// 解除复合片段：把里面的片段按现在的位置和裁剪放回时间轴（同类轨道放得下就放，否则新建轨道），
/// 返回放回去的片段。
///
/// # Errors
/// 不是复合片段；轨道锁着。
pub fn break_compound(project: &mut Project, clip: ClipId) -> Result<Vec<ClipId>, EditError> {
    let (track_id, c) = project
        .timeline
        .find_clip(clip)
        .ok_or(EditError::ClipNotFound(clip))?;
    let Some((inner, source_in)) = c.compound() else {
        return Err(EditError::WrongTrackKind);
    };
    let inner = Arc::clone(inner);
    let (retime, span) = (Retime::of(c, source_in), c.source_span());
    if project.timeline.track(track_id).is_some_and(|t| t.locked) {
        return Err(EditError::TrackLocked);
    }
    let (host_hidden, host_muted) = project
        .timeline
        .track(track_id)
        .map_or((false, false), |t| (t.hidden, t.muted));
    let mut next = project.clone();
    for t in &mut next.timeline.tracks {
        t.clips.retain(|x| x.id != clip);
    }
    let mut placed = Vec::new();
    // 每类轨道上一条（更靠下的）放到了哪条轨道：从下往上放，上下顺序才不会乱
    let mut below: Vec<(TrackKind, TrackId)> = Vec::new();
    for it in inner.tracks.iter().rev() {
        let mut ids = next.ids.clone();
        let pieces = window(it, source_in, source_in + span, retime, &mut |_, _| {
            ids.clip()
        });
        next.ids = ids;
        if pieces.is_empty() {
            continue;
        }
        // 放回去后还是原来的样子：所在轨道的隐藏 / 静音要和它在里面时一样
        let (hidden, muted) = (it.hidden || host_hidden, it.muted || host_muted);
        let fits = |t: &Track| {
            t.kind == it.kind
                && !t.locked
                && t.hidden == hidden
                && t.muted == muted
                && pieces.iter().all(|p| t.is_free(p.start, p.end(), &[]))
        };
        let tracks = &next.timeline.tracks;
        let floor = below
            .iter()
            .find(|(k, _)| *k == it.kind)
            .and_then(|(_, id)| tracks.iter().position(|t| t.id == *id));
        let ti = if let Some(floor) = floor {
            // 在下面那条的上方找最近的；没有就紧贴着它上面新建一条
            if let Some(i) = (0..floor).rev().find(|&i| fits(&tracks[i])) {
                i
            } else {
                let id = next.ids.track();
                next.timeline.tracks.insert(floor, Track::new(id, it.kind));
                floor
            }
        } else {
            // 最下面一条：原来那条（复合片段所在的）优先，再往上找最近的，再找别的
            let host = tracks.iter().position(|t| t.id == track_id);
            let found = host
                .filter(|&h| fits(&tracks[h]))
                .or_else(|| host.and_then(|h| (0..h).rev().find(|&i| fits(&tracks[i]))))
                .or_else(|| tracks.iter().position(fits));
            index_or_add(&mut next, found, it.kind)?
        };
        let target = &mut next.timeline.tracks[ti];
        target.hidden = hidden;
        target.muted = muted;
        below.retain(|(k, _)| *k != it.kind);
        below.push((it.kind, target.id));
        for p in pieces {
            placed.push(p.id);
            target.insert_sorted(p);
        }
    }
    *project = next;
    Ok(placed)
}

/// 进入复合片段编辑完回来（剪映「返回上一层」）：里面换成 `tracks`（空轨道丢掉），名字不变；
/// 里面变短了，复合片段跟着截短；里面什么都不剩了，这段复合片段删掉。
///
/// # Errors
/// 片段不存在、不是复合片段；轨道锁着。
pub fn set_inner(project: &mut Project, clip: ClipId, tracks: Vec<Track>) -> Result<(), EditError> {
    let (track, c) = project
        .timeline
        .find_clip(clip)
        .ok_or(EditError::ClipNotFound(clip))?;
    let Some((inner, source_in)) = c.compound() else {
        return Err(EditError::WrongTrackKind);
    };
    if project.timeline.track(track).is_some_and(|t| t.locked) {
        return Err(EditError::TrackLocked);
    }
    let name = inner.name.clone();
    let speed = f64::from(c.speed);
    let tracks: Vec<Track> = tracks.into_iter().filter(|t| !t.clips.is_empty()).collect();
    let next = Arc::new(Compound { name, tracks });
    let room = (next.duration() - source_in).scaled(1.0 / speed);
    if room <= Time::ZERO {
        return crate::edit::delete_clips(project, &[clip]);
    }
    let ti = project
        .timeline
        .tracks
        .iter()
        .position(|t| t.id == track)
        .ok_or(EditError::TrackNotFound(track))?;
    let tr = &mut project.timeline.tracks[ti];
    let i = tr.index_of(clip).ok_or(EditError::ClipNotFound(clip))?;
    let slot = Arc::make_mut(&mut tr.clips[i]);
    slot.content = ClipContent::Compound {
        inner: next,
        source_in,
    };
    slot.duration = slot.duration.min(room);
    slot.fade_in = slot.fade_in.min(slot.duration);
    slot.fade_out = slot.fade_out.min(slot.duration - slot.fade_in);
    Ok(())
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

/// 把复合片段的音量 `db` 叠到里面一段上：固定值和每个音量关键帧都加上，夹在音量范围里。
fn stack_gain(piece: &mut Clip, db: f32) {
    let (lo, hi) = crate::keyframe::VOLUME_RANGE_DB;
    piece.gain_db = (piece.gain_db + db).clamp(lo, hi);
    let keys: Vec<crate::Key> = piece.animation.keys(crate::Prop::Volume).to_vec();
    for k in keys {
        piece
            .animation
            .upsert(crate::Prop::Volume, k.at, (k.value + db).clamp(lo, hi));
    }
}

/// 摊开的片段记下外面那段复合片段 `parent`（合成时套上它的画面属性）；里面本来就有外层
/// （复合片段套复合片段）时，接到最外面（那几层已经跟着片段一起换到了外面的时间）。
fn adopt(piece: &mut Clip, parent: &Arc<Clip>) {
    piece.parent = Some(piece.parent.take().map_or_else(
        || Arc::clone(parent),
        |inner| {
            let mut inner = (*inner).clone();
            adopt(&mut inner, parent);
            Arc::new(inner)
        },
    ));
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
            let retime = Retime::of(c, source_in);
            let parent = Arc::clone(c);
            for it in flatten_tracks(&inner.tracks) {
                let mut pieces = window(
                    &it,
                    source_in,
                    source_in + c.source_span(),
                    retime,
                    // 一段原片段在范围里只留一块：按原 id 合成，裁剪时 id 不变
                    &mut |id, _| ClipId(synth(outer, id.0)),
                );
                for piece in &mut pieces {
                    let piece = Arc::make_mut(piece);
                    adopt(piece, &parent);
                    // 复合片段的音量叠到里面的声音上（音频轨和视频的原声都算；有音量关键帧的，
                    // 每个关键帧也一起挪）
                    if it.kind != TrackKind::Subtitle && parent.gain_db != 0.0 {
                        stack_gain(piece, parent.gain_db);
                    }
                }
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
    fn a_sped_up_compound_plays_everything_inside_faster() {
        let (mut p, ids) = sample_project();
        let half = |t: Time| Time(t.0 / 2);
        // 0–8 秒打包，两倍速 → 4 秒
        let cc = make_compound(&mut p, &[ids.v1, ids.v2, ids.a1, ids.s1]).unwrap();
        crate::edit::set_speed(&mut p, cc, 2.0).unwrap();
        assert_eq!(p.timeline.find_clip(cc).unwrap().1.duration, secs(4));
        let flat = flatten(&p).unwrap();
        // 1 秒 = 里面 2 秒（v1 素材 2 秒）；3 秒 = 里面 6 秒（v2 素材 6 秒）
        let at = |t| crate::compose::resolve_layers(&flat, t)[0].source_time;
        assert_eq!((at(secs(1)), at(secs(3))), (secs(2), secs(6)));
        let pieces: Vec<(Time, Time, f32)> = flat
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(|c| (c.start, c.duration, c.speed))
            .collect();
        // 字幕 1–3 秒 → 0.5–1.5 秒；声音 0–8 → 0–4 两倍速；v2 5–8 → 2.5–4
        assert!(
            pieces.contains(&(half(secs(1)), secs(1), 1.0)),
            "{pieces:?}"
        );
        assert!(pieces.contains(&(Time::ZERO, secs(4), 2.0)), "{pieces:?}");
        assert!(
            pieces.contains(&(half(secs(5)), half(secs(3)), 2.0)),
            "{pieces:?}"
        );
        // 剪掉开头 1 秒（= 里面 2 秒）：1 秒处是里面 2 秒
        let mut trimmed = p.clone();
        crate::edit::trim_start(&mut trimmed, cc, secs(1)).unwrap();
        let trimmed_flat = flatten(&trimmed).unwrap();
        assert_eq!(
            crate::compose::resolve_layers(&trimmed_flat, secs(1))[0].source_time,
            secs(2)
        );
        // 不能再往前拖：里面开头就是 0
        crate::edit::trim_start(&mut trimmed, cc, Time::ZERO).unwrap();
        assert_eq!(trimmed.timeline.find_clip(cc).unwrap().1.start, Time::ZERO);
        // 解除：放回去的片段保持现在的快慢和位置
        let mut broken = p.clone();
        break_compound(&mut broken, cc).unwrap();
        let back: Vec<(Time, Time, f32)> = broken
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(|c| (c.start, c.duration, c.speed))
            .collect();
        assert!(
            back.contains(&(half(secs(5)), half(secs(3)), 2.0)),
            "{back:?}"
        );
        // 写回里面：里面 8 秒，两倍速最多 4 秒
        let inner = p
            .timeline
            .find_clip(cc)
            .unwrap()
            .1
            .compound()
            .unwrap()
            .0
            .tracks
            .clone();
        set_inner(&mut p, cc, inner).unwrap();
        assert_eq!(p.timeline.find_clip(cc).unwrap().1.duration, secs(4));
    }

    #[test]
    fn compound_speed_keeps_every_inner_speed_in_range() {
        let (mut p, ids) = sample_project();
        crate::edit::set_speed(&mut p, ids.v1, 8.0).unwrap();
        let cc = make_compound(&mut p, &[ids.v1]).unwrap();
        // 里面 8 倍速再两倍就是 16 倍，超出 0.1–10
        assert_eq!(
            crate::edit::set_speed(&mut p, cc, 2.0),
            Err(EditError::InvalidValue)
        );
        assert!(crate::edit::set_speed(&mut p, cc, 1.2).is_ok());
        // 复合片段不能曲线变速
        assert_eq!(
            crate::edit::set_speed_curve(&mut p, cc, Some(crate::CurvePreset::Hero.curve())),
            Err(EditError::WrongTrackKind)
        );
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
    fn breaking_keeps_the_inner_video_tracks_in_order() {
        let (mut p, ids) = sample_project();
        // 上面一条视频轨放画中画 pip（0–4），和主轨 v1（0–5）一起打包
        let top = crate::edit::add_track(&mut p, TrackKind::Video).unwrap();
        let pip = p.ids.clip();
        let clip = crate::testing::media_clip(pip, ids.video_asset, Time::ZERO, secs(4));
        p.timeline
            .track_mut(top)
            .unwrap()
            .insert_sorted(Arc::new(clip));
        let cc = make_compound(&mut p, &[pip, ids.v1]).unwrap();
        // 复合片段挪回主轨：上面那条空了
        crate::edit::move_clip(&mut p, cc, ids.video_track, Time::ZERO).unwrap();
        break_compound(&mut p, cc).unwrap();
        // 从下往上：全屏那段（5 秒）在下，画中画（4 秒）在上
        let order: Vec<Time> = p
            .timeline
            .video_tracks_bottom_up()
            .filter_map(|t| {
                t.clips
                    .iter()
                    .find(|c| c.start == Time::ZERO)
                    .map(|c| c.duration)
            })
            .collect();
        assert_eq!(order, [secs(5), secs(4)], "画中画还在全屏画面上面");
        p.timeline.check_invariants(&p.assets).unwrap();
    }

    #[test]
    fn hidden_and_muted_tracks_stay_that_way() {
        let (mut p, ids) = sample_project();
        crate::edit::set_track_flag(&mut p, ids.audio_track, crate::edit::TrackFlag::Muted, true)
            .unwrap();
        let cc = make_compound(&mut p, &[ids.v1, ids.a1]).unwrap();
        let (_, c) = p.timeline.find_clip(cc).unwrap();
        let inner = c.compound().unwrap().0;
        assert!(
            inner
                .tracks
                .iter()
                .any(|t| t.kind == TrackKind::Audio && t.muted)
        );
        let flat = flatten(&p).unwrap();
        assert!(
            flat.timeline
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Audio && !t.clips.is_empty())
                .all(|t| t.muted),
            "摊开后还是静音"
        );
        break_compound(&mut p, cc).unwrap();
        let audio = p
            .timeline
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Audio && !t.clips.is_empty())
            .unwrap();
        assert!(audio.muted, "解除后还在静音轨上");
    }

    #[test]
    fn a_compound_with_picture_stays_on_video_tracks_and_its_inside_is_checked() {
        let (mut p, ids) = sample_project();
        let cc = make_compound(&mut p, &[ids.v1, ids.v2]).unwrap();
        assert_eq!(
            crate::edit::move_clip(&mut p, cc, ids.audio_track, secs(20)),
            Err(EditError::WrongTrackKind)
        );
        // 里面坏了：整个工程不合法
        let (track, _) = p.timeline.find_clip(cc).unwrap();
        let t = p.timeline.track_mut(track).unwrap();
        let i = t.index_of(cc).unwrap();
        let outer = Arc::make_mut(&mut t.clips[i]);
        let ClipContent::Compound { inner, .. } = &mut outer.content else {
            panic!("不是复合片段");
        };
        Arc::make_mut(&mut Arc::make_mut(inner).tracks[0].clips[0]).gain_db = f32::NAN;
        assert!(p.timeline.check_invariants(&p.assets).is_err());
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

    #[test]
    fn the_compounds_own_picture_and_volume_reach_whats_inside() {
        let (mut p, ids) = sample_project();
        let cc = make_compound(&mut p, &[ids.v1, ids.v2, ids.a1]).unwrap();
        let group = crate::Transform {
            x: 0.1,
            scale: 0.5,
            opacity: 0.5,
            ..crate::Transform::default()
        };
        crate::edit::set_transform(&mut p, cc, group).unwrap();
        crate::edit::set_gain(&mut p, cc, -6.0).unwrap();
        let flat = flatten(&p).unwrap();
        let frame = crate::compose::resolve_frame(&flat, secs(1));
        assert_eq!(frame.layers.len(), 1);
        let shown = frame.layers[0].transform;
        assert!((shown.scale - 0.5).abs() < 1e-6 && (shown.x - 0.1).abs() < 1e-6);
        // 不透明度按整组算
        assert!((shown.opacity - 1.0).abs() < 1e-6);
        assert_eq!(frame.groups.len(), 1);
        assert!((frame.groups[0].opacity - 0.5).abs() < 1e-6);
        let gain = flat
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .flat_map(|t| t.clips.iter())
            .map(|c| c.gain_db)
            .next()
            .unwrap();
        assert!((gain + 6.0).abs() < 1e-6, "复合片段的音量叠到里面");
    }

    #[test]
    fn nested_compounds_stack_their_pictures_at_the_right_time() {
        let (mut p, ids) = sample_project();
        let inner = make_compound(&mut p, &[ids.v1, ids.v2]).unwrap();
        let half = crate::Transform {
            scale: 0.5,
            ..crate::Transform::default()
        };
        crate::edit::set_transform(&mut p, inner, half).unwrap();
        let outer = make_compound(&mut p, &[inner]).unwrap();
        crate::edit::set_transform(&mut p, outer, half).unwrap();
        // 外面那段挪到 3 秒：里面那层的起点也要跟着挪，6 秒处是 v1 的第 3 秒
        crate::edit::move_clip(&mut p, outer, ids.video_track, secs(3)).unwrap();
        let flat = flatten(&p).unwrap();
        let layers = crate::compose::resolve_layers(&flat, secs(6));
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].source_time, secs(3));
        assert!(
            (layers[0].transform.scale - 0.25).abs() < 1e-6,
            "两层各缩一半"
        );
        assert_eq!(crate::compose::resolve_layers(&flat, secs(2)), []);
    }

    #[test]
    fn the_compound_volume_also_reaches_video_sound_and_volume_keys() {
        let (mut p, ids) = sample_project();
        // a1 有音量关键帧：0 秒 -10 dB、4 秒 0 dB
        crate::edit::add_keyframe_props(&mut p, ids.a1, secs(0), &[crate::Prop::Volume]).unwrap();
        crate::edit::apply_gain_at(&mut p, ids.a1, secs(0), -10.0).unwrap();
        crate::edit::apply_gain_at(&mut p, ids.a1, secs(4), 0.0).unwrap();
        let cc = make_compound(&mut p, &[ids.v1, ids.a1]).unwrap();
        crate::edit::set_gain(&mut p, cc, -6.0).unwrap();
        let flat = flatten(&p).unwrap();
        let all: Vec<(TrackKind, Arc<Clip>)> = flat
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(move |c| (t.kind, Arc::clone(c))))
            .collect();
        let video = &all.iter().find(|(k, _)| *k == TrackKind::Video).unwrap().1;
        assert!(
            (video.gain_db + 6.0).abs() < 1e-6,
            "视频的原声也跟着小 6 dB"
        );
        let audio = &all.iter().find(|(k, _)| *k == TrackKind::Audio).unwrap().1;
        assert!(
            (audio.gain_db_at(secs(0)) + 16.0).abs() < 1e-4,
            "{}",
            audio.gain_db_at(secs(0))
        );
        assert!((audio.gain_db_at(secs(4)) + 6.0).abs() < 1e-4);
    }

    #[test]
    fn a_translucent_compound_fades_as_one_group() {
        let (mut p, ids) = sample_project();
        // v2 挪到上面一条视频轨的 0 秒：和 v1 叠在一起
        let upper = crate::edit::add_track(&mut p, TrackKind::Video).unwrap();
        crate::edit::move_clip(&mut p, ids.v2, upper, secs(0)).unwrap();
        let cc = make_compound(&mut p, &[ids.v1, ids.v2]).unwrap();
        let half = crate::Transform {
            opacity: 0.5,
            ..crate::Transform::default()
        };
        crate::edit::set_transform(&mut p, cc, half).unwrap();
        let flat = flatten(&p).unwrap();
        let frame = crate::compose::resolve_frame(&flat, secs(1));
        // 里面两层各自不透明，整组一起半透明（不是每层各乘一半，那样重叠处会透出下面那层）
        assert_eq!(frame.layers.len(), 2);
        assert!(
            frame
                .layers
                .iter()
                .all(|l| (l.transform.opacity - 1.0).abs() < 1e-6)
        );
        assert_eq!(
            frame.groups,
            [crate::compose::GroupPass {
                start: 0,
                end: 2,
                effects_end: 0,
                opacity: 0.5,
                shape: None,
            }]
        );
    }

    #[test]
    fn a_compounds_mask_and_corners_cut_the_whole_group() {
        let (mut p, ids) = sample_project();
        let cc = make_compound(&mut p, &[ids.v1, ids.v2]).unwrap();
        // 没有外观、完全不透明：不用分组
        let plain = flatten(&p).unwrap();
        assert_eq!(crate::compose::resolve_frame(&plain, secs(1)).groups, []);
        let moved = crate::Transform {
            x: 0.2,
            ..crate::Transform::default()
        };
        crate::edit::set_transform(&mut p, cc, moved).unwrap();
        let look = crate::Look {
            mask: Some(crate::Mask::new(crate::MaskShape::Circle)),
            radius: 0.3,
            ..crate::Look::NONE
        };
        crate::edit::set_look(&mut p, cc, look).unwrap();
        let flat = flatten(&p).unwrap();
        let frame = crate::compose::resolve_frame(&flat, secs(1));
        assert_eq!(frame.groups.len(), 1);
        let pass = frame.groups[0];
        assert!((pass.opacity - 1.0).abs() < 1e-6);
        let shape = pass.shape.expect("有蒙版的复合片段整组裁");
        assert_eq!(shape.look, look);
        assert!((shape.transform.x - 0.2).abs() < 1e-6);
        // 套在外面一层的复合片段里：形状的位置是两层叠起来的
        let outer = make_compound(&mut p, &[cc]).unwrap();
        crate::edit::set_transform(&mut p, outer, moved).unwrap();
        let nested = flatten(&p).unwrap();
        let nested_frame = crate::compose::resolve_frame(&nested, secs(1));
        let inner = nested_frame.groups.iter().find_map(|g| g.shape).unwrap();
        assert!(
            (inner.transform.x - 0.4).abs() < 1e-5,
            "{}",
            inner.transform.x
        );
    }

    #[test]
    fn a_compounds_own_grade_comes_after_the_adjustments_inside_it() {
        let (mut p, ids) = sample_project();
        // 里面：v1 上面压一层调节片段（提亮）
        let upper = crate::edit::add_track(&mut p, TrackKind::Video).unwrap();
        let adj = crate::edit::add_adjustment_on(&mut p, upper, secs(0), secs(5)).unwrap();
        let bright = crate::ColorAdjust {
            brightness: 0.3,
            ..crate::ColorAdjust::default()
        };
        crate::edit::set_adjust(&mut p, adj, bright).unwrap();
        let cc = make_compound(&mut p, &[ids.v1, adj]).unwrap();
        // 复合片段自己：加对比度
        let contrast = crate::ColorAdjust {
            contrast: 0.5,
            ..crate::ColorAdjust::default()
        };
        crate::edit::set_adjust(&mut p, cc, contrast).unwrap();
        let flat = flatten(&p).unwrap();
        let layers = crate::compose::resolve_layers(&flat, secs(1));
        assert_eq!(layers.len(), 1);
        // 先里面的提亮，再整组的对比度（反过来结果不一样）
        let want = bright
            .matrix()
            .then(&contrast.matrix())
            .apply([0.2, 0.2, 0.2]);
        let wrong = contrast
            .matrix()
            .then(&bright.matrix())
            .apply([0.2, 0.2, 0.2]);
        assert!((want[0] - wrong[0]).abs() > 1e-3, "两个顺序要能分出来");
        let got = layers[0].color.apply([0.2, 0.2, 0.2]);
        assert!((got[0] - want[0]).abs() < 1e-4, "{got:?} vs {want:?}");
    }
}
