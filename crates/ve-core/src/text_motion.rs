//! 字幕的入场 / 出场 / 循环动画（剪映文字「动画」的常用几种），以及字幕某一刻的样子。
//!
//! 预览和导出（ASS）都按这里算：入场、出场各占 [`motion_len`]，期间透明度、上下位移、缩放都是线性变化；
//! 逐字的入场（打字机、逐字渐显、逐字放大）一个字一个字地出来；循环动画（呼吸、晃动、闪烁、跳动）
//! 在整条字幕上反复。导出时 ASS 按时间切成小段、每段线性过渡（见 `ve-export` 的 `ass`），两边一致。

use crate::Time;
use serde::{Deserialize, Serialize};

/// 入场。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextIn {
    /// 无。
    #[default]
    None,
    /// 渐显。
    FadeIn,
    /// 向上滑入（从下面一点升上来，同时渐显）。
    SlideUp,
    /// 放大（从一半大小放大到原样）。
    ZoomIn,
    /// 打字机：一个字一个字地出现。
    Typewriter,
    /// 逐字渐显。
    CharFade,
    /// 逐字放大。
    CharZoom,
}

/// 出场。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextOut {
    /// 无。
    #[default]
    None,
    /// 渐隐。
    FadeOut,
    /// 向下滑出（往下沉，同时渐隐）。
    SlideDown,
    /// 缩小（缩到一半大小）。
    ZoomOut,
}

impl TextIn {
    /// 全部（界面上的顺序）。
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::FadeIn,
        Self::SlideUp,
        Self::ZoomIn,
        Self::Typewriter,
        Self::CharFade,
        Self::CharZoom,
    ];

    /// 名字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::FadeIn => "渐显",
            Self::SlideUp => "向上滑入",
            Self::ZoomIn => "放大",
            Self::Typewriter => "打字机",
            Self::CharFade => "逐字渐显",
            Self::CharZoom => "逐字放大",
        }
    }

    /// 是不是逐字的入场。
    #[must_use]
    pub const fn per_char(self) -> bool {
        matches!(self, Self::Typewriter | Self::CharFade | Self::CharZoom)
    }
}

/// 循环动画（整条字幕上一直反复）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextLoop {
    /// 无。
    #[default]
    None,
    /// 呼吸：一大一小。
    Breathe,
    /// 晃动：左右摆。
    Shake,
    /// 闪烁：一明一暗。
    Blink,
    /// 跳动：一跳一跳。
    Bounce,
}

impl TextLoop {
    /// 全部（界面上的顺序）。
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::Breathe,
        Self::Shake,
        Self::Blink,
        Self::Bounce,
    ];

    /// 名字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::Breathe => "呼吸",
            Self::Shake => "晃动",
            Self::Blink => "闪烁",
            Self::Bounce => "跳动",
        }
    }

    /// 一个周期多长。
    #[must_use]
    pub const fn period(self) -> Time {
        match self {
            Self::None | Self::Breathe => Time::from_millis(1200),
            Self::Shake => Time::from_millis(500),
            Self::Blink => Time::from_millis(800),
            Self::Bounce => Time::from_millis(600),
        }
    }
}

impl TextOut {
    /// 全部（界面上的顺序）。
    pub const ALL: [Self; 4] = [Self::None, Self::FadeOut, Self::SlideDown, Self::ZoomOut];

    /// 名字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::FadeOut => "渐隐",
            Self::SlideDown => "向下滑出",
            Self::ZoomOut => "缩小",
        }
    }
}

/// 一条字幕的动画。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextMotion {
    /// 入场。
    pub intro: TextIn,
    /// 出场。
    pub outro: TextOut,
    /// 循环。
    pub looping: TextLoop,
}

impl TextMotion {
    /// 没有动画（不写进工程文件）。
    #[must_use]
    pub fn is_none(&self) -> bool {
        *self == Self::default()
    }
}

/// 入场 / 出场各自的长度：0.5 秒，字幕太短时最多占一半。
#[must_use]
pub fn motion_len(duration: Time) -> Time {
    Time::from_millis(500).min(Time(duration.0 / 2))
}

/// 滑入 / 滑出移动的距离：画面高度的 5%。
pub const SLIDE: f32 = 0.05;

/// 缩放动画的起点 / 终点：一半大小。
pub const ZOOM_FROM: f32 = 0.5;

/// 字幕某一刻的样子。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextPose {
    /// 不透明度（0–1）。
    pub alpha: f32,
    /// 往下偏移，单位是画面高度（正数往下）。
    pub dy: f32,
    /// 字的缩放（以底边中点为中心）。
    pub scale: f32,
    /// 往右偏移，单位是画面宽度。
    pub dx: f32,
}

impl TextPose {
    /// 原样。
    pub const STILL: Self = Self {
        alpha: 1.0,
        dy: 0.0,
        scale: 1.0,
        dx: 0.0,
    };
}

/// 逐字入场时，每个字隔多久出来：入场时长平均分给每个字（最多 0.12 秒一个）。
#[must_use]
pub fn char_step(len: Time, chars: usize) -> Time {
    let n = i64::try_from(chars.max(1)).unwrap_or(i64::MAX);
    Time(len.0 / n).min(Time::from_millis(120))
}

/// 逐字入场的整段时长：每个字出来一次，最长 1.5 秒、最多占字幕的一半。
#[must_use]
pub fn char_len(duration: Time, chars: usize) -> Time {
    let n = i64::try_from(chars.max(1)).unwrap_or(i64::MAX);
    Time(Time::from_millis(120).0 * n)
        .min(Time::from_millis(1500))
        .min(Time(duration.0 / 2))
}

/// 一个字从开始出来到完全出来要多久（逐字渐显 / 放大；打字机是一下子出来）。
pub const CHAR_FADE: Time = Time::from_millis(250);

/// 逐字入场时第 `i` 个字（共 `n` 个）在片段内 `offset` 时刻的（不透明度，缩放）。
#[must_use]
pub fn char_reveal(intro: TextIn, i: usize, n: usize, offset: Time, duration: Time) -> (f32, f32) {
    if !intro.per_char() {
        return (1.0, 1.0);
    }
    let step = char_step(char_len(duration, n), n);
    let k = i64::try_from(i).unwrap_or(i64::MAX);
    let begin = Time(step.0.saturating_mul(k));
    if offset < begin {
        return (
            0.0,
            if intro == TextIn::CharZoom {
                ZOOM_FROM
            } else {
                1.0
            },
        );
    }
    #[expect(clippy::cast_precision_loss, reason = "动画进度只要大致精确")]
    let p = ((offset - begin).0 as f32 / CHAR_FADE.0 as f32).clamp(0.0, 1.0);
    match intro {
        TextIn::Typewriter => (1.0, 1.0),
        TextIn::CharFade => (p, 1.0),
        _ => (p, (1.0 - ZOOM_FROM).mul_add(p, ZOOM_FROM)),
    }
}

/// 循环动画在片段内 `offset` 时刻对样子的影响。
fn apply_loop(pose: &mut TextPose, looping: TextLoop, offset: Time) {
    if looping == TextLoop::None {
        return;
    }
    #[expect(clippy::cast_precision_loss, reason = "动画进度只要大致精确")]
    let phase = (offset.0.rem_euclid(looping.period().0) as f32) / looping.period().0 as f32;
    let wave = (phase * std::f32::consts::TAU).sin();
    match looping {
        TextLoop::None => {}
        TextLoop::Breathe => {
            pose.scale *= 0.04f32.mul_add(1.0 - (phase * std::f32::consts::TAU).cos(), 1.0);
        }
        TextLoop::Shake => pose.dx += 0.012 * wave,
        TextLoop::Blink => {
            pose.alpha *= 0.35f32.mul_add((phase * std::f32::consts::TAU).cos(), 0.65);
        }
        TextLoop::Bounce => pose.dy -= 0.025 * (phase * std::f32::consts::PI).sin(),
    }
}

/// 字幕在片段内 `offset` 时刻的样子（片段长 `duration`）。
#[must_use]
pub fn text_pose(motion: TextMotion, offset: Time, duration: Time) -> TextPose {
    let len = motion_len(duration);
    #[expect(clippy::cast_precision_loss, reason = "动画进度只要大致精确")]
    let progress = |t: Time| {
        if len <= Time::ZERO {
            1.0
        } else {
            (t.0 as f32 / len.0 as f32).clamp(0.0, 1.0)
        }
    };
    let mut pose = TextPose::STILL;
    if offset < len {
        let p = progress(offset);
        match motion.intro {
            // 逐字的入场按每个字算（[`char_reveal`]），整行不动
            TextIn::None | TextIn::Typewriter | TextIn::CharFade | TextIn::CharZoom => {}
            TextIn::FadeIn => pose.alpha = p,
            TextIn::SlideUp => {
                pose.alpha = p;
                pose.dy = SLIDE * (1.0 - p);
            }
            TextIn::ZoomIn => pose.scale = (1.0 - ZOOM_FROM).mul_add(p, ZOOM_FROM),
        }
    }
    let out_from = duration - len;
    if offset >= out_from {
        let p = progress(offset - out_from);
        match motion.outro {
            TextOut::None => {}
            TextOut::FadeOut => pose.alpha = 1.0 - p,
            TextOut::SlideDown => {
                pose.alpha = 1.0 - p;
                pose.dy = SLIDE * p;
            }
            TextOut::ZoomOut => pose.scale = (ZOOM_FROM - 1.0).mul_add(p, 1.0),
        }
    }
    apply_loop(&mut pose, motion.looping, offset);
    pose
}

/// 字幕某一刻的样子：入场 / 出场 / 循环动画，再加上片段的位置、缩放、不透明度（可以打关键帧）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextState {
    /// 往右偏移（画面宽度的比例）。
    pub x: f32,
    /// 往下偏移（画面高度的比例）。
    pub y: f32,
    /// 字的缩放。
    pub scale: f32,
    /// 不透明度。
    pub alpha: f32,
}

/// 字幕片段在片段内 `offset` 时刻的样子（不是字幕时按没有动画算）。
#[must_use]
pub fn text_state(clip: &crate::Clip, offset: Time) -> TextState {
    let motion = clip.text_motion().unwrap_or_default();
    let pose = text_pose(motion, offset, clip.duration);
    let t = clip.transform_at(offset);
    TextState {
        x: t.x + pose.dx,
        y: t.y + pose.dy,
        scale: t.scale * pose.scale,
        alpha: t.opacity * pose.alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn per_char_intros_reveal_one_by_one() {
        let d = Time::from_secs(4);
        let n = 5;
        let step = char_step(char_len(d, n), n);
        assert_eq!(step, Time::from_millis(120));
        // 打字机：第 3 个字在 2 × 步长之前不出现，之后马上全不透明
        let at = |i: usize, t: Time| char_reveal(TextIn::Typewriter, i, n, t, d);
        assert_eq!(at(2, Time(step.0 * 2 - 1)).0, 0.0);
        assert_eq!(at(2, Time(step.0 * 2)).0, 1.0);
        // 逐字渐显：渐渐出来
        let (a, _) = char_reveal(TextIn::CharFade, 0, n, Time::from_millis(125), d);
        assert!(close(a, 0.5));
        let (_, s) = char_reveal(TextIn::CharZoom, 0, n, Time::ZERO, d);
        assert!(close(s, ZOOM_FROM));
        assert_eq!(char_reveal(TextIn::FadeIn, 3, n, Time::ZERO, d), (1.0, 1.0));
    }

    #[test]
    fn loops_repeat_every_period() {
        let d = Time::from_secs(10);
        for l in TextLoop::ALL {
            let m = TextMotion {
                looping: l,
                ..TextMotion::default()
            };
            let a = text_pose(m, Time::from_millis(300), d);
            let b = text_pose(m, Time::from_millis(300) + l.period(), d);
            assert!(
                close(a.scale, b.scale)
                    && close(a.alpha, b.alpha)
                    && close(a.dx, b.dx)
                    && close(a.dy, b.dy),
                "{l:?}"
            );
        }
        let breathe = TextMotion {
            looping: TextLoop::Breathe,
            ..TextMotion::default()
        };
        assert!(text_pose(breathe, Time::from_millis(600), d).scale > 1.05);
        assert!(close(text_pose(breathe, Time::ZERO, d).scale, 1.0));
    }

    #[test]
    fn length_is_half_a_second_or_half_the_clip() {
        assert_eq!(motion_len(Time::from_secs(3)), Time::from_millis(500));
        assert_eq!(motion_len(Time::from_millis(600)), Time::from_millis(300));
    }

    #[test]
    fn poses_move_linearly_in_and_out() {
        let d = Time::from_secs(2);
        let m = TextMotion {
            intro: TextIn::SlideUp,
            outro: TextOut::ZoomOut,
            ..TextMotion::default()
        };
        let start = text_pose(m, Time::ZERO, d);
        assert!(close(start.alpha, 0.0) && close(start.dy, SLIDE));
        let half = text_pose(m, Time::from_millis(250), d);
        assert!(
            close(half.alpha, 0.5) && close(half.dy, SLIDE / 2.0),
            "{half:?}"
        );
        assert_eq!(text_pose(m, Time::from_secs(1), d), TextPose::STILL);
        let ending = text_pose(m, Time::from_millis(1750), d);
        assert!(
            close(ending.scale, 0.75) && close(ending.alpha, 1.0),
            "{ending:?}"
        );
        let fade = TextMotion {
            intro: TextIn::FadeIn,
            outro: TextOut::FadeOut,
            ..TextMotion::default()
        };
        assert!(close(
            text_pose(fade, Time::from_millis(1999), d).alpha,
            0.002
        ));
        let zoom = TextMotion {
            intro: TextIn::ZoomIn,
            outro: TextOut::SlideDown,
            ..TextMotion::default()
        };
        assert!(close(text_pose(zoom, Time::ZERO, d).scale, ZOOM_FROM));
        assert!(close(
            text_pose(zoom, Time::from_millis(1750), d).dy,
            SLIDE / 2.0
        ));
        assert!(TextMotion::default().is_none());
    }
}
