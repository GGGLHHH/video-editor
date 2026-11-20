//! 三种剪辑点修剪（Premiere / 达芬奇的工具；剪映没有）：
//!
//! - 滑移（slip）：片段位置、长度不动，换用素材里的另一段（素材内起点前后挪）。
//! - 滑动（slide）：片段内容不变、整段左右挪，紧挨着的前一段跟着变长 / 变短、后一段反过来，总长不变。
//! - 滚动（roll）：两段紧挨着的片段之间的剪辑点左右挪，前一段变长多少后一段就变短多少。
//!
//! 每个操作给的是「再挪多少」，返回实际挪了多少（碰到素材头尾、相邻片段太短时夹住）。

use crate::edit::{EditError, min_len, source_end_limit, trim_end, trim_start};
use crate::{Clip, ClipContent, ClipId, Project, Time};
use std::sync::Arc;

/// 片段在轨道上的位置和它紧挨着的前后片段。
fn neighbours(project: &Project, clip: ClipId) -> Result<(usize, usize), EditError> {
    project
        .timeline
        .tracks
        .iter()
        .enumerate()
        .find_map(|(ti, t)| t.index_of(clip).map(|ci| (ti, ci)))
        .ok_or(EditError::ClipNotFound(clip))
}

/// 片段往前（起点往左）最多还能露出多少素材（时间轴时间）。
fn head_room(c: &Clip) -> Time {
    match &c.content {
        ClipContent::Media { source_in, .. } => source_in.scaled(1.0 / f64::from(c.speed)),
        ClipContent::Compound { source_in, .. } => *source_in,
        _ => Time(i64::MAX / 4),
    }
}

/// 片段往后（终点往右）最多还能露出多少素材。
fn tail_room(project: &Project, c: &Clip) -> Time {
    source_end_limit(project, c) - c.end()
}

fn check_unlocked(project: &Project, ti: usize) -> Result<(), EditError> {
    if project.timeline.tracks[ti].locked {
        Err(EditError::TrackLocked)
    } else {
        Ok(())
    }
}

/// 滑移：片段换用素材里往后 `delta`（时间轴时间，负的往前）的那一段。
///
/// # Errors
/// 片段不存在、不是素材 / 复合片段、轨道锁着。
pub fn slip(project: &mut Project, clip: ClipId, delta: Time) -> Result<Time, EditError> {
    let (ti, ci) = neighbours(project, clip)?;
    check_unlocked(project, ti)?;
    let c = (*project.timeline.tracks[ti].clips[ci]).clone();
    if !matches!(
        c.content,
        ClipContent::Media { .. } | ClipContent::Compound { .. }
    ) {
        return Err(EditError::WrongTrackKind);
    }
    let d = delta.clamp(Time::ZERO - head_room(&c), tail_room(project, &c));
    if d == Time::ZERO {
        return Ok(Time::ZERO);
    }
    let mut next = c;
    match &mut next.content {
        ClipContent::Media { source_in, .. } => *source_in += d.scaled(f64::from(next.speed)),
        ClipContent::Compound { source_in, .. } => *source_in += d,
        _ => {}
    }
    project.timeline.tracks[ti].clips[ci] = Arc::new(next);
    Ok(d)
}

/// 滑动：片段整段往后挪 `delta`（负的往前），紧挨着的前后两段跟着伸缩。
///
/// # Errors
/// 片段不存在、轨道锁着。
pub fn slide(project: &mut Project, clip: ClipId, delta: Time) -> Result<Time, EditError> {
    let (ti, ci) = neighbours(project, clip)?;
    check_unlocked(project, ti)?;
    let min = min_len(project);
    let track = &project.timeline.tracks[ti];
    let c = Arc::clone(&track.clips[ci]);
    let prev = ci.checked_sub(1).map(|i| Arc::clone(&track.clips[i]));
    let after = track.clips.get(ci + 1).map(Arc::clone);
    // 紧挨着才跟着伸缩；隔着空隙的只当边界
    let left = prev.as_ref().filter(|p| p.end() == c.start);
    let right = after.as_ref().filter(|n| n.start == c.end());
    let far = Time(i64::MAX / 4);
    let up = right.map_or_else(
        || after.as_ref().map_or(far, |n| n.start - c.end()),
        |n| n.duration - min,
    );
    let up = up.min(left.map_or(far, |p| tail_room(project, p)));
    let down = left.map_or_else(
        || c.start - prev.as_ref().map_or(Time::ZERO, |p| p.end()),
        |p| p.duration - min,
    );
    let down = down.min(right.map_or(far, |n| head_room(n)));
    let d = delta.clamp(Time::ZERO - down.max(Time::ZERO), up.max(Time::ZERO));
    if d == Time::ZERO {
        return Ok(Time::ZERO);
    }
    let (left, right) = (left.map(|p| p.id), right.map(|n| n.id));
    let mut next = project.clone();
    let moved = |p: &mut Project| {
        let (mt, mc) = neighbours(p, clip)?;
        let mut m = (*p.timeline.tracks[mt].clips[mc]).clone();
        m.start += d;
        p.timeline.tracks[mt].clips[mc] = Arc::new(m);
        Ok::<(), EditError>(())
    };
    // 先让出地方再挪，免得重叠
    if d > Time::ZERO {
        if let Some(n) = right {
            trim_start(&mut next, n, c.end() + d)?;
        }
        moved(&mut next)?;
        if let Some(p) = left {
            trim_end(&mut next, p, c.start + d)?;
        }
    } else {
        if let Some(p) = left {
            trim_end(&mut next, p, c.start + d)?;
        }
        moved(&mut next)?;
        if let Some(n) = right {
            trim_start(&mut next, n, c.end() + d)?;
        }
    }
    *project = next;
    Ok(d)
}

/// 滚动：片段 `left` 和紧挨着它的后一段之间的剪辑点往后挪 `delta`（负的往前）。
///
/// # Errors
/// 片段不存在、后面没有紧挨着的片段（[`EditError::Overlap`] 之外的情况都用 `InvalidValue`）、轨道锁着。
pub fn roll(project: &mut Project, left: ClipId, delta: Time) -> Result<Time, EditError> {
    let (ti, ci) = neighbours(project, left)?;
    check_unlocked(project, ti)?;
    let min = min_len(project);
    let track = &project.timeline.tracks[ti];
    let a = Arc::clone(&track.clips[ci]);
    let b = track
        .clips
        .get(ci + 1)
        .filter(|n| n.start == a.end())
        .map(Arc::clone)
        .ok_or(EditError::InvalidValue)?;
    let up = tail_room(project, &a).min(b.duration - min).max(Time::ZERO);
    let down = (a.duration - min).min(head_room(&b)).max(Time::ZERO);
    let d = delta.clamp(Time::ZERO - down, up);
    if d == Time::ZERO {
        return Ok(Time::ZERO);
    }
    let cut = a.end() + d;
    let mut next = project.clone();
    if d > Time::ZERO {
        trim_start(&mut next, b.id, cut)?;
        trim_end(&mut next, a.id, cut)?;
    } else {
        trim_end(&mut next, a.id, cut)?;
        trim_start(&mut next, b.id, cut)?;
    }
    *project = next;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::sample_project;

    fn secs(s: i64) -> Time {
        Time::from_secs(s)
    }

    fn span(p: &Project, id: ClipId) -> (Time, Time, Time) {
        let (_, c) = p.timeline.find_clip(id).unwrap();
        (c.start, c.end(), c.source_in())
    }

    #[test]
    fn slip_changes_only_the_source_and_stops_at_its_ends() {
        let (mut p, ids) = sample_project();
        // v1：时间轴 0–5，素材 0–5；素材 10 秒长
        assert_eq!(slip(&mut p, ids.v1, secs(2)).unwrap(), secs(2));
        assert_eq!(span(&p, ids.v1), (secs(0), secs(5), secs(2)));
        // 往后最多到素材结尾（还能挪 3 秒）
        assert_eq!(slip(&mut p, ids.v1, secs(9)).unwrap(), secs(3));
        // 往前最多到素材开头
        assert_eq!(slip(&mut p, ids.v1, secs(-20)).unwrap(), secs(-5));
        assert_eq!(span(&p, ids.v1).2, Time::ZERO);
        assert_eq!(
            slip(&mut p, ids.s1, secs(1)),
            Err(EditError::WrongTrackKind)
        );
        p.timeline.check_invariants(&p.assets).unwrap();
    }

    #[test]
    fn slide_moves_the_clip_and_its_neighbours_follow() {
        let (mut p, ids) = sample_project();
        // v1 0–5、v2 5–8（素材 5–8）。v2 往前滑 1 秒：v1 变成 0–4，v2 4–7，内容不变
        assert_eq!(slide(&mut p, ids.v2, secs(-1)).unwrap(), secs(-1));
        assert_eq!(span(&p, ids.v1), (secs(0), secs(4), secs(0)));
        assert_eq!(span(&p, ids.v2), (secs(4), secs(7), secs(5)));
        // 后面是空的：往后最多回到… 前一段 v1 能伸到素材结尾（10 秒），后面空地无限
        assert_eq!(slide(&mut p, ids.v2, secs(2)).unwrap(), secs(2));
        assert_eq!(span(&p, ids.v1), (secs(0), secs(6), secs(0)));
        assert_eq!(span(&p, ids.v2), (secs(6), secs(9), secs(5)));
        // v1 往前：前面没东西、在 0 秒，挪不动
        assert_eq!(slide(&mut p, ids.v1, secs(-1)).unwrap(), Time::ZERO);
        p.timeline.check_invariants(&p.assets).unwrap();
    }

    #[test]
    fn roll_moves_the_cut_between_two_clips() {
        let (mut p, ids) = sample_project();
        // v1 | v2 的剪辑点在 5 秒：往后 1 秒，v1 0–6、v2 6–8（素材从 6 起）
        assert_eq!(roll(&mut p, ids.v1, secs(1)).unwrap(), secs(1));
        assert_eq!(span(&p, ids.v1), (secs(0), secs(6), secs(0)));
        assert_eq!(span(&p, ids.v2), (secs(6), secs(8), secs(6)));
        // 往前挪 3 秒：v2 素材最早从 5 起，所以 v2 往左最多伸回 1 秒…
        // 但现在 v2 的素材从 6 起，往前最多 6 秒；v1 至少留一帧：能挪 3 秒
        assert_eq!(roll(&mut p, ids.v1, secs(-3)).unwrap(), secs(-3));
        assert_eq!(span(&p, ids.v1), (secs(0), secs(3), secs(0)));
        assert_eq!(span(&p, ids.v2), (secs(3), secs(8), secs(3)));
        // 后面没有紧挨着的片段
        assert_eq!(roll(&mut p, ids.v2, secs(1)), Err(EditError::InvalidValue));
        p.timeline.check_invariants(&p.assets).unwrap();
    }

    #[test]
    fn locked_tracks_refuse_all_three() {
        let (mut p, ids) = sample_project();
        crate::edit::set_track_flag(
            &mut p,
            ids.video_track,
            crate::edit::TrackFlag::Locked,
            true,
        )
        .unwrap();
        assert_eq!(slip(&mut p, ids.v1, secs(1)), Err(EditError::TrackLocked));
        assert_eq!(slide(&mut p, ids.v2, secs(1)), Err(EditError::TrackLocked));
        assert_eq!(roll(&mut p, ids.v1, secs(1)), Err(EditError::TrackLocked));
    }
}
