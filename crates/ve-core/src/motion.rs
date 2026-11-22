//! 入场 / 出场 / 组合动画（剪映属性面板「动画」）。
//!
//! 片段上存的是 [`ClipMotion`]：选了哪个动画、多长，播放和导出时在关键帧算出的画面属性上再叠加，
//! 所以能随时改时长、换一个或去掉（不会留下关键帧）。组合动画铺满整段，和入场 / 出场二选一。
//!
//! 下面的 [`apply_motion`] 是早先的做法：把预设打成关键帧，套用后能在关键帧上接着改。
//!
//! 一个预设只动一个属性（渐显动不透明度、放大缩小动缩放、滑入动位置）。入场在片段开头打两帧：
//! 起点是「从哪来」的值、`duration` 处是原来的值，缓出；出场在片段末尾反过来，缓入。
//! 套用前先清掉这个属性在片段前半段（入场）或后半段（出场）的关键帧，所以换一个预设就是替换。

use crate::edit::EditError;
use crate::{ClipId, Easing, Project, Prop, Time, Transform};
use serde::{Deserialize, Serialize};

/// 动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// 选了的入场或出场动画。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotionClip {
    /// 动作。
    pub motion: Motion,
    /// 时长（超过片段长度时按片段长度）。
    pub duration: Time,
}

/// 组合动画：铺满整段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Combo {
    /// 旋转：整段转一圈。
    Spin,
    /// 摇摆：左右来回摆（每秒一个来回）。
    Swing,
    /// 呼吸：一大一小（每 1.2 秒一次）。
    Pulse,
}

impl Combo {
    /// 全部（面板里的顺序）。
    pub const ALL: [Self; 3] = [Self::Spin, Self::Swing, Self::Pulse];

    /// 名字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Spin => "旋转",
            Self::Swing => "摇摆",
            Self::Pulse => "呼吸",
        }
    }
}

/// 摇摆的幅度（度）。
const SWING_DEG: f32 = 8.0;
/// 呼吸放大的幅度。
const PULSE: f32 = 0.08;

/// 片段的动画：入场、出场（可以都有），或者一个组合动画。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ClipMotion {
    /// 入场。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<MotionClip>,
    /// 出场。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outro: Option<MotionClip>,
    /// 组合。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combo: Option<Combo>,
}

impl ClipMotion {
    /// 没有动画。
    pub const NONE: Self = Self {
        intro: None,
        outro: None,
        combo: None,
    };

    /// 没有动画（工程文件里不写）。
    #[must_use]
    pub const fn is_none(&self) -> bool {
        self.intro.is_none() && self.outro.is_none() && self.combo.is_none()
    }

    /// 时长都是正数。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        [self.intro, self.outro]
            .iter()
            .flatten()
            .all(|m| m.duration > Time::ZERO)
    }

    /// 动画最多把画面放大到原来的几倍（渲染时按这个留够分辨率）。
    #[must_use]
    pub fn max_scale(&self) -> f32 {
        let one = |m: Option<MotionClip>| -> f32 {
            match m.map(|m| m.motion) {
                Some(Motion::ZoomOut) => 1.4,
                _ => 1.0,
            }
        };
        let combo = if self.combo == Some(Combo::Pulse) {
            1.0 + PULSE
        } else {
            1.0
        };
        one(self.intro).max(one(self.outro)).max(combo)
    }

    /// 在关键帧算出的画面属性 `t` 上叠加动画：片段内 `offset` 时刻，片段长 `len`。
    #[must_use]
    pub fn apply(&self, mut t: Transform, offset: Time, len: Time) -> Transform {
        if self.is_none() || len <= Time::ZERO {
            return t;
        }
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "进度"
        )]
        let progress = |a: Time, d: Time| (a.0 as f64 / d.0.max(1) as f64).clamp(0.0, 1.0) as f32;
        if let Some(m) = self.intro {
            let d = m.duration.min(len);
            if offset < d {
                let prop = m.motion.prop();
                let (base, away) = (prop.get(&t), m.motion.away(&t));
                let e = Easing::EaseOut.apply(progress(offset, d));
                prop.set(&mut t, away + (base - away) * e);
            }
        }
        if let Some(m) = self.outro {
            let d = m.duration.min(len);
            let from = len - d;
            if offset > from {
                let prop = m.motion.prop();
                let (base, away) = (prop.get(&t), m.motion.away(&t));
                let e = Easing::EaseIn.apply(progress(offset - from, d));
                prop.set(&mut t, base + (away - base) * e);
            }
        }
        if let Some(c) = self.combo {
            let secs = offset.as_secs_f64();
            #[expect(clippy::cast_possible_truncation, reason = "角度、比例")]
            match c {
                Combo::Spin => t.rotation += 360.0 * progress(offset, len),
                Combo::Swing => {
                    t.rotation += SWING_DEG * (std::f64::consts::TAU * secs).sin() as f32;
                }
                Combo::Pulse => {
                    let k = 1.0 + PULSE * (std::f64::consts::TAU * secs / 1.2).sin() as f32;
                    t.scale =
                        (t.scale * k).clamp(Transform::SCALE_RANGE.0, Transform::SCALE_RANGE.1);
                }
            }
        }
        t
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

    #[test]
    fn clip_motion_animates_without_keyframes_and_can_be_retimed_or_removed() {
        let (mut p, ids) = sample_project();
        let len = p.timeline.find_clip(ids.v1).unwrap().1.duration;
        let fade = MotionClip {
            motion: Motion::Fade,
            duration: secs(1.0),
        };
        let m = ClipMotion {
            intro: Some(fade),
            outro: Some(MotionClip {
                motion: Motion::SlideLeft,
                duration: secs(0.5),
            }),
            combo: None,
        };
        crate::edit::set_clip_motion(&mut p, ids.v1, m).unwrap();
        let clip_of = |proj: &Project| (**proj.timeline.find_clip(ids.v1).unwrap().1).clone();
        let c = clip_of(&p);
        assert!(c.animation.is_empty(), "不打关键帧");
        assert_eq!(c.transform_at(Time::ZERO).opacity, 0.0);
        assert!(
            c.transform_at(secs(0.5)).opacity > 0.5,
            "缓出：过半就超过一半"
        );
        assert_eq!(c.transform_at(secs(1.0)).opacity, 1.0);
        assert!(
            (c.transform_at(len).x + 1.0).abs() < 1e-6,
            "滑出到左边画面外"
        );
        // 改时长
        let longer = ClipMotion {
            intro: Some(MotionClip {
                duration: secs(2.0),
                ..fade
            }),
            ..m
        };
        crate::edit::set_clip_motion(&mut p, ids.v1, longer).unwrap();
        assert!(clip_of(&p).transform_at(secs(1.0)).opacity < 1.0);
        // 组合动画：转一圈；去掉
        let spin = ClipMotion {
            combo: Some(Combo::Spin),
            ..ClipMotion::default()
        };
        crate::edit::set_clip_motion(&mut p, ids.v1, spin).unwrap();
        let rotated = clip_of(&p);
        let half = Time(len.0 / 2);
        assert!((rotated.transform_at(half).rotation - 180.0).abs() < 1.0);
        assert!(rotated.motion.max_scale() >= 1.0);
        crate::edit::set_clip_motion(&mut p, ids.v1, ClipMotion::default()).unwrap();
        let plain = clip_of(&p);
        assert!(plain.motion.is_none());
        assert_eq!(plain.transform_at(Time::ZERO), plain.transform);
        // 时长不是正数不行
        let bad = ClipMotion {
            intro: Some(MotionClip {
                duration: Time::ZERO,
                ..fade
            }),
            ..ClipMotion::default()
        };
        assert!(crate::edit::set_clip_motion(&mut p, ids.v1, bad).is_err());
    }

    #[test]
    fn editing_during_an_animation_does_not_bake_it_in() {
        // 渐显到一半时拖动画面：只改位置，不透明度别被写成半透明
        let (mut p, ids) = sample_project();
        let m = ClipMotion {
            intro: Some(MotionClip {
                motion: Motion::Fade,
                duration: secs(1.0),
            }),
            ..ClipMotion::NONE
        };
        crate::edit::set_clip_motion(&mut p, ids.v1, m).unwrap();
        let c = p.timeline.find_clip(ids.v1).unwrap().1;
        assert!(c.transform_at(secs(0.5)).opacity < 1.0);
        let mut t = c.keyed_transform_at(secs(0.5));
        assert_eq!(t.opacity, 1.0, "编辑用的是不含动画的值");
        t.x = 0.2;
        crate::edit::apply_transform_at(&mut p, ids.v1, secs(0.5), t).unwrap();
        crate::edit::add_keyframe(&mut p, ids.v1, secs(0.5)).unwrap();
        let after = p.timeline.find_clip(ids.v1).unwrap().1;
        assert_eq!(after.transform.opacity, 1.0);
        assert_eq!(
            after.keyed_transform_at(secs(0.5)).opacity,
            1.0,
            "关键帧也没记成半透明"
        );
        assert!((after.transform_at(secs(0.5)).x - 0.2).abs() < 1e-6);
    }

    #[test]
    fn splitting_keeps_the_intro_on_the_left_and_the_outro_on_the_right() {
        let (mut p, ids) = sample_project();
        let m = ClipMotion {
            intro: Some(MotionClip {
                motion: Motion::Fade,
                duration: secs(0.5),
            }),
            outro: Some(MotionClip {
                motion: Motion::Fade,
                duration: secs(0.5),
            }),
            combo: None,
        };
        crate::edit::set_clip_motion(&mut p, ids.v1, m).unwrap();
        let start = p.timeline.find_clip(ids.v1).unwrap().1.start;
        let right = crate::edit::split_at(&mut p, ids.v1, start + secs(1.0)).unwrap();
        let l = p.timeline.find_clip(ids.v1).unwrap().1.motion;
        let r = p.timeline.find_clip(right).unwrap().1.motion;
        assert!(l.intro.is_some() && l.outro.is_none());
        assert!(r.intro.is_none() && r.outro.is_some());
        // 存进工程文件再读回来
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<ClipMotion>(&json).unwrap(), m);
    }

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
