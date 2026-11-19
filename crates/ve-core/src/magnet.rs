//! 主轨磁吸与联动（剪映的两个默认开关）。
//!
//! - **主轨**：最下面一条视频轨。
//! - **主轨磁吸**（[`ProjectSettings::main_magnet`](crate::ProjectSettings)）：主轨上的片段永远从 0 开始、
//!   首尾相接、不留空隙。删掉、剪短主轨片段，后面的自动补上；把片段拖进主轨，按位置插队。
//! - **联动**（[`ProjectSettings::linkage`](crate::ProjectSettings)）：主轨片段挪了，压在它上面的字幕跟着挪同样的距离。
//!
//! 做法：编辑前用 [`Layout::of`] 记下主轨和字幕的位置，编辑后调用 [`settle`]。

use crate::edit::EditError;
use crate::{ClipId, Project, Time, Timeline, TrackId, TrackKind};
use std::sync::Arc;

/// 主轨：最下面一条视频轨。
#[must_use]
pub fn main_track(timeline: &Timeline) -> Option<TrackId> {
    timeline
        .tracks
        .iter()
        .rev()
        .find(|t| t.kind == TrackKind::Video)
        .map(|t| t.id)
}

/// 编辑前的位置记录。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    /// 主轨片段：（片段，起点，终点）。
    main: Vec<(ClipId, Time, Time)>,
    /// 跟着主轨走的片段（字幕）：（片段，起点）。
    linked: Vec<(ClipId, Time)>,
}

impl Layout {
    /// 记下当前的主轨和字幕位置。
    #[must_use]
    pub fn of(timeline: &Timeline) -> Self {
        let main = main_track(timeline)
            .and_then(|id| timeline.track(id))
            .map(|t| t.clips.iter().map(|c| (c.id, c.start, c.end())).collect())
            .unwrap_or_default();
        let linked = timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Subtitle && !t.locked)
            .flat_map(|t| t.clips.iter().map(|c| (c.id, c.start)))
            .collect();
        Self { main, linked }
    }
}

/// 主轨紧密排列：按现在的先后顺序，从 0 开始首尾相接。
pub fn pack(project: &mut Project) {
    let Some(main) = main_track(&project.timeline) else {
        return;
    };
    let Some(track) = project.timeline.track_mut(main) else {
        return;
    };
    // 锁着的主轨原样不动
    if track.locked {
        return;
    }
    let mut at = Time::ZERO;
    for clip in &mut track.clips {
        if clip.start != at {
            Arc::make_mut(clip).start = at;
        }
        at = clip.end();
    }
}

/// 编辑后收尾：磁吸开着就把主轨排紧；联动开着就让字幕跟着它下面的主轨片段挪。
/// `before` 是编辑前的 [`Layout::of`]。
pub fn settle(project: &mut Project, before: &Layout) {
    if project.settings.main_magnet {
        pack(project);
    }
    if project.settings.linkage {
        follow_main(project, before);
    }
}

/// 字幕跟着主轨片段挪：编辑本身没动过的字幕，按编辑前压着的那个主轨片段挪了多少就挪多少。
/// 挪过去会和别的字幕重叠的，留在原处。
fn follow_main(project: &mut Project, before: &Layout) {
    let Some(main) = main_track(&project.timeline) else {
        return;
    };
    let now: Vec<(ClipId, Time)> = project
        .timeline
        .track(main)
        .map(|t| t.clips.iter().map(|c| (c.id, c.start)).collect())
        .unwrap_or_default();
    let delta_of = |at: Time| -> Option<Time> {
        let &(host, start, _) = before
            .main
            .iter()
            .find(|&&(_, start, end)| start <= at && at < end)?;
        let &(_, new_start) = now.iter().find(|&&(id, _)| id == host)?;
        Some(new_start - start)
    };
    let mut moves: Vec<(ClipId, Time)> = before
        .linked
        .iter()
        .filter_map(|&(id, old)| {
            let (_, clip) = project.timeline.find_clip(id)?;
            // 编辑本身挪过的（比如就是在拖这条字幕）不再跟着走
            if clip.start != old {
                return None;
            }
            let delta = delta_of(old).filter(|d| *d != Time::ZERO)?;
            Some((id, (old + delta).max(Time::ZERO)))
        })
        .collect();
    // 往前挪的从前往后挪、往后挪的从后往前挪，同方向的一串不会互相挡住
    moves.sort_by_key(|&(id, to)| {
        let from = project
            .timeline
            .find_clip(id)
            .map_or(Time::ZERO, |(_, c)| c.start);
        if to < from { to.0 } else { -to.0 }
    });
    for (id, to) in moves {
        if let Some((track, _)) = project.timeline.find_clip(id) {
            let _ = crate::edit::move_clip(project, id, track, to);
        }
    }
}

/// 把片段放进主轨（磁吸开着时拖进主轨用）：按 `at` 插到主轨片段之间（越过某个片段的中点就排到它后面），
/// 再整条排紧。
///
/// # Errors
/// 片段不存在；片段不能放在视频轨上；没有主轨。失败时工程不变。
pub fn insert_into_main(project: &mut Project, clip: ClipId, at: Time) -> Result<(), EditError> {
    let main = main_track(&project.timeline).ok_or(EditError::WrongTrackKind)?;
    let mut next = project.clone();
    // 先挪进主轨的末尾（顺带检查类型），再按位置排进去
    let end = next.timeline.track(main).map_or(Time::ZERO, |t| {
        t.clips
            .iter()
            .filter(|c| c.id != clip)
            .map(|c| c.end())
            .max()
            .unwrap_or(Time::ZERO)
    });
    crate::edit::move_clip(&mut next, clip, main, end)?;
    let track = next
        .timeline
        .track_mut(main)
        .ok_or(EditError::TrackNotFound(main))?;
    let index = track.index_of(clip).ok_or(EditError::ClipNotFound(clip))?;
    let moving = track.clips.remove(index);
    let slot = track
        .clips
        .iter()
        .take_while(|c| c.start + Time(c.duration.0 / 2) < at)
        .count();
    track.clips.insert(slot, moving);
    pack(&mut next);
    *project = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{delete_clips, move_clip};
    use crate::testing::sample_project;

    fn secs(s: i64) -> Time {
        Time::from_secs(s)
    }

    fn span(p: &Project, id: ClipId) -> (Time, Time) {
        let (_, c) = p.timeline.find_clip(id).unwrap();
        (c.start, c.end())
    }

    fn with_switches(magnet: bool, linkage: bool) -> (Project, crate::testing::SampleIds) {
        let (mut p, ids) = sample_project();
        p.settings.main_magnet = magnet;
        p.settings.linkage = linkage;
        (p, ids)
    }

    #[test]
    fn main_track_is_the_bottom_video_track() {
        let (mut p, ids) = sample_project();
        assert_eq!(main_track(&p.timeline), Some(ids.video_track));
        crate::edit::add_track(&mut p, TrackKind::Video).unwrap();
        assert_eq!(
            main_track(&p.timeline),
            Some(ids.video_track),
            "新视频轨在上面"
        );
    }

    #[test]
    fn magnet_closes_gaps_after_delete() {
        let (mut p, ids) = with_switches(true, false);
        let before = Layout::of(&p.timeline);
        delete_clips(&mut p, &[ids.v1]).unwrap();
        settle(&mut p, &before);
        assert_eq!(span(&p, ids.v2), (secs(0), secs(3)), "后面的补上来");
        assert_eq!(span(&p, ids.s1), (secs(1), secs(3)), "不联动时字幕不动");
        p.timeline.check_invariants(&p.assets).unwrap();
    }

    #[test]
    fn magnet_off_leaves_gaps() {
        let (mut p, ids) = with_switches(false, false);
        let before = Layout::of(&p.timeline);
        delete_clips(&mut p, &[ids.v1]).unwrap();
        settle(&mut p, &before);
        assert_eq!(span(&p, ids.v2), (secs(5), secs(8)));
    }

    #[test]
    fn linked_subtitles_follow_their_main_clip() {
        let (mut p, ids) = with_switches(true, true);
        // 字幕挪到 v2（5–8）上面：6–8
        move_clip(&mut p, ids.s1, ids.subtitle_track, secs(6)).unwrap();
        let before = Layout::of(&p.timeline);
        delete_clips(&mut p, &[ids.v1]).unwrap();
        settle(&mut p, &before);
        assert_eq!(span(&p, ids.v2), (secs(0), secs(3)));
        assert_eq!(
            span(&p, ids.s1),
            (secs(1), secs(3)),
            "跟着 v2 往前挪了 5 秒"
        );
        p.timeline.check_invariants(&p.assets).unwrap();
    }

    #[test]
    fn locked_tracks_stay_put() {
        use crate::edit::{TrackFlag, set_track_flag};
        let (mut p, ids) = with_switches(false, true);
        move_clip(&mut p, ids.s1, ids.subtitle_track, secs(6)).unwrap();
        // 主轨留个空隙（磁吸关着时做的），再锁上主轨、打开磁吸
        move_clip(&mut p, ids.v2, ids.video_track, secs(6)).unwrap();
        set_track_flag(&mut p, ids.video_track, TrackFlag::Locked, true).unwrap();
        p.settings.main_magnet = true;
        let before = Layout::of(&p.timeline);
        settle(&mut p, &before);
        assert_eq!(span(&p, ids.v2), (secs(6), secs(9)), "锁着的主轨不排紧");
        // 锁着的字幕不跟着主轨走
        set_track_flag(&mut p, ids.video_track, TrackFlag::Locked, false).unwrap();
        set_track_flag(&mut p, ids.subtitle_track, TrackFlag::Locked, true).unwrap();
        let unlocked_main = Layout::of(&p.timeline);
        settle(&mut p, &unlocked_main);
        assert_eq!(span(&p, ids.v2), (secs(5), secs(8)));
        assert_eq!(span(&p, ids.s1), (secs(6), secs(8)), "锁着的字幕留在原处");
    }

    #[test]
    fn subtitle_moved_by_the_edit_itself_does_not_follow() {
        let (mut p, ids) = with_switches(true, true);
        let before = Layout::of(&p.timeline);
        // 同一次编辑里既删了 v1、又亲手把字幕挪到 10 秒：字幕留在 10 秒
        delete_clips(&mut p, &[ids.v1]).unwrap();
        move_clip(&mut p, ids.s1, ids.subtitle_track, secs(10)).unwrap();
        settle(&mut p, &before);
        assert_eq!(span(&p, ids.s1).0, secs(10));
    }

    #[test]
    fn dragging_into_main_reorders_by_position() {
        let (mut p, ids) = with_switches(true, true);
        // v2（5–8）拖到 1 秒：越过不了 v1 的中点（2.5），排到它前面
        insert_into_main(&mut p, ids.v2, secs(1)).unwrap();
        assert_eq!(span(&p, ids.v2), (secs(0), secs(3)));
        assert_eq!(span(&p, ids.v1), (secs(3), secs(8)));
        // 再拖到 9 秒：排到最后
        insert_into_main(&mut p, ids.v2, secs(9)).unwrap();
        assert_eq!(span(&p, ids.v1), (secs(0), secs(5)));
        assert_eq!(span(&p, ids.v2), (secs(5), secs(8)));
        // 字幕、音频不能进主轨
        let before = p.clone();
        assert_eq!(
            insert_into_main(&mut p, ids.s1, secs(0)),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(p, before);
        p.timeline.check_invariants(&p.assets).unwrap();
    }
}
