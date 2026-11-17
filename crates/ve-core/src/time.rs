//! 时间：以 flicks（1/705 600 000 秒）为单位的整数时间，以及帧率换算。
//!
//! flicks 能被常见帧率（24、25、30、48、50、60、90、100、120 以及 NTSC 的 /1001）
//! 和常见采样率（8k～192k）整除，用 `i64` 存可以精确表示，不会有浮点累积误差。

use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

/// 每秒的 flicks 数。
pub const FLICKS_PER_SECOND: i64 = 705_600_000;

/// 时间点或时长，单位 flicks。
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Time(pub i64);

impl Time {
    /// 零。
    pub const ZERO: Self = Self(0);

    /// 整秒。
    #[must_use]
    pub const fn from_secs(secs: i64) -> Self {
        Self(secs * FLICKS_PER_SECOND)
    }

    /// 毫秒。
    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis * (FLICKS_PER_SECOND / 1000))
    }

    /// 浮点秒（就近取整）。
    #[must_use]
    #[expect(clippy::cast_possible_truncation, reason = "时间范围远小于 i64 上限")]
    pub fn from_secs_f64(secs: f64) -> Self {
        Self((secs * 705_600_000.0).round() as i64)
    }

    /// 转浮点秒。
    #[must_use]
    #[expect(clippy::cast_precision_loss, reason = "仅用于显示和浮点计算")]
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / 705_600_000.0
    }

    /// 转毫秒（向下取整）。
    #[must_use]
    pub const fn as_millis(self) -> i64 {
        self.0.div_euclid(FLICKS_PER_SECOND / 1000)
    }

    /// 由样本数和采样率构造（向下取整）。
    #[must_use]
    pub fn from_samples(samples: i64, sample_rate: u32) -> Self {
        Self(mul_div_floor(
            samples,
            FLICKS_PER_SECOND,
            i64::from(sample_rate),
        ))
    }

    /// 转样本数（向下取整）。
    #[must_use]
    pub fn to_samples(self, sample_rate: u32) -> i64 {
        mul_div_floor(self.0, i64::from(sample_rate), FLICKS_PER_SECOND)
    }
}

impl Add for Time {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl Sub for Time {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl Neg for Time {
    type Output = Self;
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl AddAssign for Time {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl SubAssign for Time {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

/// `a * b / c` 向下取整，中间用 i128 防溢出；结果饱和到 i64。
fn mul_div_floor(value: i64, mul: i64, div: i64) -> i64 {
    saturate((i128::from(value) * i128::from(mul)).div_euclid(i128::from(div)))
}

/// `a * b / c` 向上取整。
fn mul_div_ceil(value: i64, mul: i64, div: i64) -> i64 {
    let product = i128::from(value) * i128::from(mul);
    let div = i128::from(div);
    saturate(product.div_euclid(div) + i128::from(product.rem_euclid(div) != 0))
}

fn saturate(wide: i128) -> i64 {
    i64::try_from(wide).unwrap_or(if wide < 0 { i64::MIN } else { i64::MAX })
}

/// 帧率 `num / den`（如 30000/1001）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameRate {
    /// 分子。
    pub num: u32,
    /// 分母。
    pub den: u32,
}

impl FrameRate {
    /// 构造；`num`、`den` 都必须大于 0。
    #[must_use]
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    /// 浮点帧率。
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    fn den_flicks(self) -> i64 {
        FLICKS_PER_SECOND * i64::from(self.den)
    }

    /// 一帧的时长（向下取整）。
    #[must_use]
    pub fn frame_duration(self) -> Time {
        Time(mul_div_floor(1, self.den_flicks(), i64::from(self.num)))
    }

    /// 时间所在的帧号（向下取整）。
    #[must_use]
    pub fn frame_of(self, t: Time) -> i64 {
        mul_div_floor(t.0, i64::from(self.num), self.den_flicks())
    }

    /// 第 `frame` 帧的起始时间（向上取整，保证 `frame_of(time_of_frame(n)) == n`）。
    #[must_use]
    pub fn time_of_frame(self, frame: i64) -> Time {
        Time(mul_div_ceil(frame, self.den_flicks(), i64::from(self.num)))
    }

    /// 吸附到最近的帧起点。
    #[must_use]
    pub fn snap(self, t: Time) -> Time {
        let f = self.frame_of(t);
        let a = self.time_of_frame(f);
        let b = self.time_of_frame(f + 1);
        if t - a <= b - t { a } else { b }
    }
}

/// 格式化为非丢帧时间码 `HH:MM:SS:FF`，负数前加 `-`。
#[must_use]
pub fn format_timecode(t: Time, rate: FrameRate) -> String {
    let neg = t.0 < 0;
    let frames = rate.frame_of(Time(t.0.abs()));
    let fps = i64::from(rate.num.div_ceil(rate.den)).max(1);
    let ff = frames % fps;
    let total_secs = frames / fps;
    let mut s = String::with_capacity(12);
    if neg {
        s.push('-');
    }
    let _ = write!(
        s,
        "{:02}:{:02}:{:02}:{:02}",
        total_secs / 3600,
        total_secs / 60 % 60,
        total_secs % 60,
        ff
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const RATES: [FrameRate; 5] = [
        FrameRate::new(30, 1),
        FrameRate::new(25, 1),
        FrameRate::new(24, 1),
        FrameRate::new(24_000, 1001),
        FrameRate::new(60, 1),
    ];

    #[test]
    fn frame_round_trip_30_25_24_23976_60() {
        for rate in RATES {
            for t in [
                0,
                1,
                12_345_678,
                FLICKS_PER_SECOND,
                3 * FLICKS_PER_SECOND + 7,
            ] {
                let t = Time(t);
                let back = rate.time_of_frame(rate.frame_of(t));
                assert!(back <= t, "{rate:?} {t:?} -> {back:?}");
                assert!(
                    t - back < rate.frame_duration(),
                    "{rate:?} {t:?} -> {back:?}"
                );
            }
            for n in [0, 1, 29, 1000, 123_457] {
                assert_eq!(
                    rate.frame_of(rate.time_of_frame(n)),
                    n,
                    "{rate:?} frame {n}"
                );
            }
        }
    }

    #[test]
    fn samples_48000_exact() {
        assert_eq!(Time::from_samples(48_000, 48_000), Time(FLICKS_PER_SECOND));
        assert_eq!(Time(FLICKS_PER_SECOND).to_samples(44_100), 44_100);
        assert_eq!(Time::from_samples(1, 48_000).to_samples(48_000), 1);
    }

    #[test]
    fn secs_conversions() {
        assert_eq!(Time::from_secs(2), Time(2 * FLICKS_PER_SECOND));
        assert_eq!(Time::from_millis(1500), Time(FLICKS_PER_SECOND * 3 / 2));
        assert_eq!(Time::from_secs_f64(0.5), Time(FLICKS_PER_SECOND / 2));
        assert!((Time::from_secs(3).as_secs_f64() - 3.0).abs() < 1e-12);
        assert_eq!(Time::from_secs(1).as_millis(), 1000);
    }

    #[test]
    fn arithmetic() {
        let mut t = Time::from_secs(1) + Time::from_secs(2) - Time::from_secs(1);
        assert_eq!(t, Time::from_secs(2));
        t += Time::from_secs(1);
        t -= Time::from_secs(2);
        assert_eq!(t, Time::from_secs(1));
        assert_eq!(-t, Time::from_secs(-1));
        assert_eq!(Time::ZERO.max(-t), Time::ZERO);
    }

    #[test]
    fn timecode_formats() {
        let rate = FrameRate::new(30, 1);
        let t = Time::from_secs(3600 + 2 * 60 + 3) + rate.time_of_frame(4);
        assert_eq!(format_timecode(t, rate), "01:02:03:04");
        assert_eq!(format_timecode(Time::ZERO, rate), "00:00:00:00");
        assert_eq!(format_timecode(-Time::from_secs(1), rate), "-00:00:01:00");
        let ntsc = FrameRate::new(30_000, 1001);
        assert_eq!(format_timecode(ntsc.time_of_frame(31), ntsc), "00:00:01:01");
    }

    #[test]
    fn frame_rate_as_f64() {
        assert!((FrameRate::new(30_000, 1001).as_f64() - 29.970_029).abs() < 1e-5);
    }

    proptest! {
        #[test]
        fn snap_is_idempotent(t in -10_000_000_000_i64..10_000_000_000, idx in 0usize..5) {
            let rate = RATES[idx];
            let s = rate.snap(Time(t));
            prop_assert_eq!(rate.snap(s), s);
            prop_assert!((s - Time(t)).0.abs() <= rate.frame_duration().0 / 2 + 1);
        }
    }
}
