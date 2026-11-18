//! 转场（剪映的「转场」）：挂在后一段片段上，作用在它和同一条轨上紧挨着的前一段之间。
//!
//! 转场以切点为中心，占 `[切点 - d/2, 切点 + d/2]`。不需要片段两头多留素材：切点之后前一段停在
//! 最后一帧、切点之前后一段停在第一帧，两段按进度 `p`（0 → 1）混合。

use crate::Transform;
use serde::{Deserialize, Serialize};

/// 转场效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransitionKind {
    /// 叠化：后一段渐渐盖上来。
    Dissolve,
    /// 闪黑：前一段暗下去，再从黑里亮出后一段。
    FadeBlack,
    /// 滑动：后一段从右边滑进来，盖住前一段。
    Slide,
    /// 推：后一段从右边把前一段推出去。
    Push,
    /// 推近：后一段从稍大缩回原样、同时渐显。
    ZoomIn,
}

impl TransitionKind {
    /// 全部效果（素材面板里的顺序）。
    pub const ALL: [Self; 5] = [
        Self::Dissolve,
        Self::FadeBlack,
        Self::Slide,
        Self::Push,
        Self::ZoomIn,
    ];

    /// 名字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dissolve => "叠化",
            Self::FadeBlack => "闪黑",
            Self::Slide => "滑动",
            Self::Push => "推",
            Self::ZoomIn => "推近",
        }
    }

    /// 进度 `p`（0–1）时，前一段和后一段的画面属性（在各自原来的属性上改）。
    #[must_use]
    pub fn blend(self, p: f32, outgoing: Transform, incoming: Transform) -> (Transform, Transform) {
        let p = p.clamp(0.0, 1.0);
        let (mut out, mut inc) = (outgoing, incoming);
        match self {
            Self::Dissolve => inc.opacity *= p,
            Self::FadeBlack => {
                out.opacity *= 2.0f32.mul_add(-p, 1.0).max(0.0);
                inc.opacity *= 2.0f32.mul_add(p, -1.0).max(0.0);
            }
            Self::Slide => inc.x += 1.0 - p,
            Self::Push => {
                out.x -= p;
                inc.x += 1.0 - p;
            }
            Self::ZoomIn => {
                inc.scale = (inc.scale * 0.3f32.mul_add(1.0 - p, 1.0))
                    .clamp(Transform::SCALE_RANGE.0, Transform::SCALE_RANGE.1);
                inc.opacity *= p;
            }
        }
        (out, inc)
    }
}

/// 一个转场：效果和时长。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    /// 效果。
    pub kind: TransitionKind,
    /// 时长（> 0）。
    pub duration: crate::Time,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blends_start_from_the_outgoing_and_end_on_the_incoming() {
        let base = Transform::default();
        for kind in TransitionKind::ALL {
            let (out, inc) = kind.blend(1.0, base, base);
            assert!(inc.is_valid() && out.is_valid(), "{kind:?}");
            assert!(
                (inc.opacity - 1.0).abs() < 1e-6 && inc.x.abs() < 1e-6,
                "{kind:?} 结束时后一段原样"
            );
            assert!((inc.scale - 1.0).abs() < 1e-6);
            assert_ne!(kind.label(), "");
        }
        let (_, start) = TransitionKind::Dissolve.blend(0.0, base, base);
        assert_eq!(start.opacity, 0.0, "叠化开始时后一段完全透明");
        let (dark_out, dark_in) = TransitionKind::FadeBlack.blend(0.5, base, base);
        assert_eq!(
            (dark_out.opacity, dark_in.opacity),
            (0.0, 0.0),
            "闪黑正中间全黑"
        );
        let (pushed, entering) = TransitionKind::Push.blend(0.25, base, base);
        assert!((pushed.x + 0.25).abs() < 1e-6 && (entering.x - 0.75).abs() < 1e-6);
    }
}
