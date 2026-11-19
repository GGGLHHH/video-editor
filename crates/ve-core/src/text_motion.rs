//! 字幕的入场 / 出场动画（剪映文字「动画」的常用几种）。
//!
//! 预览和导出（ASS）都按这里算：入场、出场各占 [`motion_len`]，期间透明度、上下位移、缩放都是线性变化
//! （ASS 的 `\fad`、`\move`、`\t` 也是线性的），两边一致。

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
    pub const ALL: [Self; 4] = [Self::None, Self::FadeIn, Self::SlideUp, Self::ZoomIn];

    /// 名字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::FadeIn => "渐显",
            Self::SlideUp => "向上滑入",
            Self::ZoomIn => "放大",
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
}

impl TextPose {
    /// 原样。
    pub const STILL: Self = Self {
        alpha: 1.0,
        dy: 0.0,
        scale: 1.0,
    };
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
            TextIn::None => {}
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
    pose
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
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
        };
        assert!(close(
            text_pose(fade, Time::from_millis(1999), d).alpha,
            0.002
        ));
        let zoom = TextMotion {
            intro: TextIn::ZoomIn,
            outro: TextOut::SlideDown,
        };
        assert!(close(text_pose(zoom, Time::ZERO, d).scale, ZOOM_FROM));
        assert!(close(
            text_pose(zoom, Time::from_millis(1750), d).dy,
            SLIDE / 2.0
        ));
        assert!(TextMotion::default().is_none());
    }
}
