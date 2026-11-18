//! 入场 / 出场动画预设（剪映属性面板「动画」）：用关键帧实现，套用后还能在关键帧上接着改。
//!
//! 一个预设只动一个属性（渐显动不透明度、放大缩小动缩放、滑入动位置）。入场在片段开头打两帧：
//! 起点是「从哪来」的值、`duration` 处是原来的值，缓出；出场在片段末尾反过来，缓入。
//! 套用前先清掉这个属性在片段前半段（入场）或后半段（出场）的关键帧，所以换一个预设就是替换。

use crate::edit::EditError;
use crate::{ClipId, Easing, Project, Prop, Time, Transform};

/// 动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// 渐显 / 渐隐。
    Fade,
    /// 放大（入场从小变大；出场变大消失）。
    ZoomIn,
    /// 缩小（入场从大变小；出场变小消失）。
    ZoomOut,
    /// 从左边滑入 / 向左滑出。
    SlideLeft,
    /// 从右边滑入 / 向右滑出。
    SlideRight,
    /// 从上边滑入 / 向上滑出。
    SlideUp,
    /// 从下边滑入 / 向下滑出。
    SlideDown,
}

impl Motion {
    /// 全部动作（面板里的顺序）。
    pub const ALL: [Self; 7] = [
        Self::Fade,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::SlideLeft,
        Self::SlideRight,
        Self::SlideUp,
        Self::SlideDown,
    ];

    /// 动的是哪个属性。
    #[must_use]
    pub const fn prop(self) -> Prop {
        match self {
            Self::Fade => Prop::Opacity,
            Self::ZoomIn | Self::ZoomOut => Prop::Scale,
            Self::SlideLeft | Self::SlideRight => Prop::X,
            Self::SlideUp | Self::SlideDown => Prop::Y,
        }
    }

    /// 画面不在原位时那一端的值（入场的起点、出场的终点）。
    fn away(self, base: &Transform) -> f32 {
        let clamp = |s: f32| s.clamp(Transform::SCALE_RANGE.0, Transform::SCALE_RANGE.1);
        match self {
            Self::Fade => 0.0,
            Self::ZoomIn => clamp(base.scale * 0.6),
            Self::ZoomOut => clamp(base.scale * 1.4),
            // 偏出一整个画面宽 / 高：完全在画面外
            Self::SlideLeft => base.x - 1.0,
            Self::SlideRight => base.x + 1.0,
            Self::SlideUp => base.y - 1.0,
            Self::SlideDown => base.y + 1.0,
        }
    }
}

/// 入场还是出场。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 入场：片段开头。
    In,
    /// 出场：片段末尾。
    Out,
}

/// 给视频轨片段套用动画预设。`duration` 超过片段长度时按片段长度。
///
/// # Errors
/// 片段不存在、不在视频轨上，或 `duration` 不是正数。
pub fn apply_motion(
    project: &mut Project,
    clip: ClipId,
    phase: Phase,
    motion: Motion,
    duration: Time,
) -> Result<(), EditError> {
    if duration <= Time::ZERO {
        return Err(EditError::ZeroLength);
    }
    let (track, c) = project
        .timeline
        .find_clip(clip)
        .ok_or(EditError::ClipNotFound(clip))?;
    if project.timeline.track(track).map(|t| t.kind) != Some(crate::TrackKind::Video) {
        return Err(EditError::WrongTrackKind);
    }
    let len = c.duration;
    let d = duration.min(len);
    let half = Time(len.0 / 2);
    let prop = motion.prop();
    let (window, rest_at, away_at) = match phase {
        Phase::In => ((Time::ZERO, half.max(d)), d, Time::ZERO),
        Phase::Out => ((half.min(len - d), len), len - d, len),
    };
    let mut next = (**c).clone();
    // 先清掉旧预设，再取「原位」的值（否则会取到旧预设动到一半的值）
    next.animation.remove_between(prop, window.0, window.1);
    let base = next.transform_at(rest_at);
    next.animation.upsert(prop, rest_at, prop.get(&base));
    next.animation.upsert(prop, away_at, motion.away(&base));
    let (first, ease) = match phase {
        Phase::In => (away_at, Easing::EaseOut),
        Phase::Out => (rest_at, Easing::EaseIn),
    };
    next.animation.set_easing(first, ease);
    if !next.animation.is_valid() {
        return Err(EditError::InvalidValue);
    }
    crate::edit::replace_clip(project, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::sample_project;

    fn secs(s: f64) -> Time {
        Time::from_secs_f64(s)
    }

    fn at(p: &Project, id: ClipId, t: f64) -> Transform {
        p.timeline.find_clip(id).unwrap().1.transform_at(secs(t))
    }

    #[test]
    fn fade_in_goes_from_transparent_to_normal() {
        let (mut p, ids) = sample_project();
        apply_motion(&mut p, ids.v1, Phase::In, Motion::Fade, secs(0.5)).unwrap();
        assert_eq!(at(&p, ids.v1, 0.0).opacity, 0.0);
        let mid = at(&p, ids.v1, 0.25).opacity;
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert_eq!(at(&p, ids.v1, 0.5).opacity, 1.0);
        assert_eq!(at(&p, ids.v1, 3.0).opacity, 1.0);
        p.timeline.check_invariants(&p.assets).unwrap();
    }

    #[test]
    fn slide_out_leaves_the_frame_at_the_end() {
        let (mut p, ids) = sample_project();
        // v1 是 0–5 秒
        apply_motion(&mut p, ids.v1, Phase::Out, Motion::SlideLeft, secs(1.0)).unwrap();
        assert_eq!(at(&p, ids.v1, 3.0).x, 0.0);
        assert_eq!(at(&p, ids.v1, 4.0).x, 0.0);
        assert_eq!(at(&p, ids.v1, 5.0).x, -1.0, "最后一刻完全滑出画面");
        let c = p.timeline.find_clip(ids.v1).unwrap().1;
        assert_eq!(c.animation.easing_at(secs(4.0)), Some(Easing::EaseIn));
    }

    #[test]
    fn a_new_preset_replaces_the_old_one_and_duration_is_clamped() {
        let (mut p, ids) = sample_project();
        apply_motion(&mut p, ids.v1, Phase::In, Motion::ZoomIn, secs(2.0)).unwrap();
        apply_motion(&mut p, ids.v1, Phase::In, Motion::ZoomOut, secs(0.5)).unwrap();
        let c = p.timeline.find_clip(ids.v1).unwrap().1;
        assert_eq!(
            c.animation.keys(Prop::Scale).len(),
            2,
            "旧预设 2 秒处的帧清掉了"
        );
        assert!((at(&p, ids.v1, 0.0).scale - 1.4).abs() < 1e-6);
        // v2 只有 3 秒：要 10 秒的入场也只占满片段
        apply_motion(&mut p, ids.v2, Phase::In, Motion::Fade, secs(10.0)).unwrap();
        let c2 = p.timeline.find_clip(ids.v2).unwrap().1;
        assert_eq!(
            c2.animation.times_of(&[Prop::Opacity]),
            [Time::ZERO, secs(3.0)]
        );
        // 字幕不行、时长要为正
        assert_eq!(
            apply_motion(&mut p, ids.s1, Phase::In, Motion::Fade, secs(1.0)),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(
            apply_motion(&mut p, ids.v1, Phase::In, Motion::Fade, Time::ZERO),
            Err(EditError::ZeroLength)
        );
    }
}
