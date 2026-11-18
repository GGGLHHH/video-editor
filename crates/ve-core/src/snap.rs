//! 吸附：拖动时贴到播放头、片段边缘、时间轴起点。

use crate::{ClipId, Time, Timeline};

/// 吸附目标：0、播放头、标记、所有片段（除 `exclude`）的起点和终点，升序去重。
#[must_use]
pub fn snap_targets(timeline: &Timeline, playhead: Time, exclude: &[ClipId]) -> Vec<Time> {
    let mut targets = vec![Time::ZERO, playhead];
    targets.extend(&timeline.markers);
    for track in &timeline.tracks {
        for clip in track.clips.iter().filter(|c| !exclude.contains(&c.id)) {
            targets.push(clip.start);
            targets.push(clip.end());
        }
    }
    targets.sort_unstable();
    targets.dedup();
    targets
}

/// `t` 吸附到最近的目标（距离不超过 `tolerance`），没有则 `None`。
#[must_use]
pub fn snap(t: Time, targets: &[Time], tolerance: Time) -> Option<Time> {
    targets
        .iter()
        .copied()
        .map(|target| (target, (target - t).0.abs()))
        .filter(|&(_, d)| d <= tolerance.0)
        .min_by_key(|&(_, d)| d)
        .map(|(target, _)| target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::sample_project;

    #[test]
    fn targets_include_edges_playhead_and_zero_without_excluded() {
        let (p, ids) = sample_project();
        // 排除 v2（5–8 秒）：5 仍是 v1 的终点、8 仍是 a1 的终点
        let t = snap_targets(&p.timeline, Time::from_millis(4321), &[ids.v2]);
        let secs = |s| Time::from_secs(s);
        assert_eq!(
            t,
            vec![
                Time::ZERO,
                secs(1),
                secs(3),
                Time::from_millis(4321),
                secs(5),
                secs(8)
            ]
        );
        let none = snap_targets(&p.timeline, Time::ZERO, &[ids.v1, ids.v2, ids.a1, ids.s1]);
        assert_eq!(none, vec![Time::ZERO]);
    }

    #[test]
    fn snap_picks_nearest_within_tolerance() {
        let targets = [Time(0), Time(100), Time(200)];
        assert_eq!(snap(Time(95), &targets, Time(10)), Some(Time(100)));
        assert_eq!(snap(Time(160), &targets, Time(50)), Some(Time(200)));
        assert_eq!(snap(Time(150), &targets, Time(10)), None);
        assert_eq!(snap(Time(5), &[], Time(10)), None);
    }

    #[test]
    fn markers_are_snap_targets() {
        let (mut p, _) = sample_project();
        p.timeline.toggle_marker(Time::from_secs(12));
        assert!(snap_targets(&p.timeline, Time::ZERO, &[]).contains(&Time::from_secs(12)));
    }
}
