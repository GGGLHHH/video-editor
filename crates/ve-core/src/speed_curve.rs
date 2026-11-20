//! 曲线变速（剪映「变速 › 曲线变速」）：片段里各处的快慢不一样。
//!
//! 曲线是若干个点（片段内位置 0–1，那一刻的倍数），点之间直线连起来。片段的平均倍数仍然存在
//! [`crate::Clip::speed`] 里（素材用掉的长度 = 时间轴长度 × 平均倍数，修剪、分割、素材上限都照旧），
//! 曲线只决定「时间轴上走到某处时，素材走到了哪」：素材进度 = 倍数从 0 积到这里 ÷ 积到 1。

use serde::{Deserialize, Serialize};

/// 曲线上的一个点。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeedPoint {
    /// 片段内位置（0–1）。
    pub at: f32,
    /// 这一刻的倍数（0.1–10）。
    pub speed: f32,
}

/// 速度曲线：点按位置排好，第一个在 0、最后一个在 1。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpeedCurve {
    points: Vec<SpeedPoint>,
}

/// 曲线变速的预设（剪映的卡片）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CurvePreset {
    /// 自定义（五个点，初始都是 1 倍）。
    Custom,
    /// 蒙太奇：快—慢—快。
    Montage,
    /// 英雄时刻：中间突然很慢。
    Hero,
    /// 子弹时间：快进来、慢镜头、快出去。
    Bullet,
    /// 跳接：中间一下子跳过去。
    JumpCut,
    /// 闪进：开头很快，然后正常。
    FlashIn,
    /// 闪出：正常，然后很快。
    FlashOut,
}

impl CurvePreset {
    /// 全部（剪映里的顺序）。
    pub const ALL: [Self; 7] = [
        Self::Custom,
        Self::Montage,
        Self::Hero,
        Self::Bullet,
        Self::JumpCut,
        Self::FlashIn,
        Self::FlashOut,
    ];

    /// 名字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Custom => "自定义",
            Self::Montage => "蒙太奇",
            Self::Hero => "英雄时刻",
            Self::Bullet => "子弹时间",
            Self::JumpCut => "跳接",
            Self::FlashIn => "闪进",
            Self::FlashOut => "闪出",
        }
    }

    /// 这个预设的曲线。
    #[must_use]
    pub fn curve(self) -> SpeedCurve {
        let pts: &[(f32, f32)] = match self {
            Self::Custom => &[(0.0, 1.0), (0.25, 1.0), (0.5, 1.0), (0.75, 1.0), (1.0, 1.0)],
            Self::Montage => &[(0.0, 3.0), (0.35, 0.4), (0.65, 0.4), (1.0, 3.0)],
            Self::Hero => &[(0.0, 1.0), (0.4, 1.0), (0.5, 0.2), (0.6, 1.0), (1.0, 1.0)],
            Self::Bullet => &[
                (0.0, 4.0),
                (0.3, 4.0),
                (0.4, 0.3),
                (0.6, 0.3),
                (0.7, 4.0),
                (1.0, 4.0),
            ],
            Self::JumpCut => &[(0.0, 1.0), (0.45, 1.0), (0.5, 8.0), (0.55, 1.0), (1.0, 1.0)],
            Self::FlashIn => &[(0.0, 5.0), (0.4, 5.0), (0.5, 1.0), (1.0, 1.0)],
            Self::FlashOut => &[(0.0, 1.0), (0.5, 1.0), (0.6, 5.0), (1.0, 5.0)],
        };
        SpeedCurve {
            points: pts
                .iter()
                .map(|&(at, speed)| SpeedPoint { at, speed })
                .collect(),
        }
    }
}

/// 倍数的范围。
pub const CURVE_SPEED_RANGE: (f32, f32) = (0.1, 10.0);

impl SpeedCurve {
    /// 用一组点建曲线；不合法时 `None`（见 [`Self::is_valid`]）。
    #[must_use]
    pub fn new(points: Vec<SpeedPoint>) -> Option<Self> {
        let c = Self { points };
        c.is_valid().then_some(c)
    }

    /// 各点。
    #[must_use]
    pub fn points(&self) -> &[SpeedPoint] {
        &self.points
    }

    /// 第一个点在 0、最后一个在 1、位置严格递增，倍数都在范围里。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let p = &self.points;
        p.len() >= 2
            && p.first().is_some_and(|f| f.at == 0.0)
            && p.last().is_some_and(|l| (l.at - 1.0).abs() < f32::EPSILON)
            && p.windows(2).all(|w| w[0].at < w[1].at)
            && p.iter()
                .all(|q| (CURVE_SPEED_RANGE.0..=CURVE_SPEED_RANGE.1).contains(&q.speed))
    }

    /// 位置 `u`（0–1）处的倍数。
    #[must_use]
    pub fn speed_at(&self, u: f32) -> f32 {
        let pos = u.clamp(0.0, 1.0);
        let pts = &self.points;
        let next = pts.partition_point(|q| q.at <= pos).clamp(1, pts.len() - 1);
        let (from, to) = (pts[next - 1], pts[next]);
        let frac = ((pos - from.at) / (to.at - from.at)).clamp(0.0, 1.0);
        (to.speed - from.speed).mul_add(frac, from.speed)
    }

    /// 倍数从 0 积到 `u`（梯形，点之间是直线所以是精确的）。
    fn integral(&self, u: f32) -> f64 {
        let u = f64::from(u.clamp(0.0, 1.0));
        let mut sum = 0.0;
        for w in self.points.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (a0, b0) = (f64::from(a.at), f64::from(b.at));
            if u <= a0 {
                break;
            }
            let end = u.min(b0);
            let va = f64::from(a.speed);
            let vb = f64::from(b.speed);
            let v_end = (vb - va).mul_add((end - a0) / (b0 - a0), va);
            sum += f64::midpoint(va, v_end) * (end - a0);
        }
        sum
    }

    /// 平均倍数（片段的 [`crate::Clip::speed`]）。
    #[must_use]
    pub fn mean(&self) -> f32 {
        #[expect(clippy::cast_possible_truncation, reason = "倍数在 0.1–10")]
        let m = self.integral(1.0) as f32;
        m
    }

    /// 时间轴上走到位置 `u`（0–1）时，素材走到了它要用的那一段的多少（0–1）。
    #[must_use]
    pub fn progress(&self, u: f32) -> f64 {
        let total = self.integral(1.0);
        if total <= 0.0 {
            return f64::from(u);
        }
        (self.integral(u) / total).clamp(0.0, 1.0)
    }

    /// 在位置 `at`（0–1）处切成两段，各自换算回 0–1（分割片段用）。
    #[must_use]
    pub fn split(&self, at: f32) -> (Self, Self) {
        let at = at.clamp(0.0, 1.0);
        let cut = SpeedPoint {
            at,
            speed: self.speed_at(at),
        };
        let mut left: Vec<SpeedPoint> = self.points.iter().copied().filter(|q| q.at < at).collect();
        left.push(cut);
        let mut right = vec![cut];
        right.extend(self.points.iter().copied().filter(|q| q.at > at));
        let norm = |pts: Vec<SpeedPoint>, from: f32, len: f32| Self {
            points: pts
                .into_iter()
                .map(|q| SpeedPoint {
                    at: ((q.at - from) / len).clamp(0.0, 1.0),
                    speed: q.speed,
                })
                .collect(),
        };
        (
            norm(left, 0.0, at.max(1e-6)),
            norm(right, at, (1.0 - at).max(1e-6)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_curve_is_linear() {
        let c = CurvePreset::Custom.curve();
        assert!((c.mean() - 1.0).abs() < 1e-6);
        for u in [0.0, 0.2, 0.5, 0.9, 1.0] {
            assert!((c.progress(u) - f64::from(u)).abs() < 1e-6);
        }
    }

    #[test]
    fn presets_are_valid_and_progress_is_monotonic() {
        for p in CurvePreset::ALL {
            let c = p.curve();
            assert!(c.is_valid(), "{p:?}");
            let mut last = -1.0;
            for i in 0..=100 {
                #[expect(clippy::cast_precision_loss, reason = "测试")]
                let v = c.progress(i as f32 / 100.0);
                assert!(v >= last, "{p:?}");
                last = v;
            }
            assert!((c.progress(1.0) - 1.0).abs() < 1e-9);
        }
        // 闪进：前 40% 是 5 倍，素材进度跑在时间前面
        let f = CurvePreset::FlashIn.curve();
        assert!(f.progress(0.2) > 0.3, "前 20% 的时间里走了三成多的素材");
        assert!((f.speed_at(0.2) - 5.0).abs() < 1e-6);
        assert!((f.speed_at(0.45) - 3.0).abs() < 1e-5);
    }

    #[test]
    fn mean_is_the_exact_integral() {
        let c = SpeedCurve::new(vec![
            SpeedPoint {
                at: 0.0,
                speed: 1.0,
            },
            SpeedPoint {
                at: 1.0,
                speed: 3.0,
            },
        ])
        .unwrap();
        assert!((c.mean() - 2.0).abs() < 1e-6);
        // 前一半积分 = (1 + 2) / 2 × 0.5 = 0.75，占总共 2 的 37.5%
        assert!((c.progress(0.5) - 0.375).abs() < 1e-9);
        assert!(
            SpeedCurve::new(vec![SpeedPoint {
                at: 0.0,
                speed: 1.0
            }])
            .is_none()
        );
        assert!(
            SpeedCurve::new(vec![
                SpeedPoint {
                    at: 0.0,
                    speed: 1.0
                },
                SpeedPoint {
                    at: 1.0,
                    speed: 20.0
                },
            ])
            .is_none()
        );
    }

    #[test]
    fn split_keeps_the_shape_on_both_sides() {
        let c = CurvePreset::Montage.curve();
        let (l, r) = c.split(0.5);
        assert!(l.is_valid() && r.is_valid());
        assert!((l.speed_at(0.0) - c.speed_at(0.0)).abs() < 1e-5);
        assert!((l.speed_at(1.0) - c.speed_at(0.5)).abs() < 1e-5);
        assert!((r.speed_at(0.5) - c.speed_at(0.75)).abs() < 1e-5);
    }
}
