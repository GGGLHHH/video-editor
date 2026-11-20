//! 电平表的数据：音频线程每混出一块，记下（这块从时间轴哪里开始，左右声道峰值）；
//! 界面按播放头取最近一小段里的峰值。声音在缓冲里要排一会儿才放出来，按时间对上才不会提前跳。

use std::collections::VecDeque;
use ve_core::Time;

/// 最多记多少块（48k 下一块约 21 毫秒，够缓冲里排着的几百毫秒）。
const KEEP: usize = 128;

/// 最近混出来的各块峰值。
#[derive(Debug, Default)]
pub struct Levels {
    recent: VecDeque<(Time, Time, [f32; 2])>,
}

impl Levels {
    /// 记一块：从时间轴 `at` 开始、长 `len`、交错立体声样本 `mix`。
    pub fn push(&mut self, at: Time, len: Time, mix: &[f32]) {
        let mut peak = [0.0f32; 2];
        for [l, r] in mix.as_chunks::<2>().0 {
            peak[0] = peak[0].max(l.abs());
            peak[1] = peak[1].max(r.abs());
        }
        if self.recent.len() >= KEEP {
            self.recent.pop_front();
        }
        self.recent.push_back((at, len, peak));
    }

    /// 清空（跳了位置、停了）。
    pub fn clear(&mut self) {
        self.recent.clear();
    }

    /// 时间轴 `t` 之前 `window` 那么长里的左右峰值（0–1）；没有数据时为零。
    #[must_use]
    pub fn at(&self, t: Time, window: Time) -> [f32; 2] {
        let from = t - window;
        self.recent
            .iter()
            .filter(|&&(start, len, _)| start <= t && start + len > from)
            .fold([0.0f32; 2], |m, &(_, _, p)| {
                [m[0].max(p[0]), m[1].max(p[1])]
            })
    }
}

/// 线性峰值换成电平表上的位置（0–1）：-60 dB 以下为 0，0 dB 为 1。
#[must_use]
pub fn meter_fraction(peak: f32) -> f32 {
    if peak <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * peak.log10();
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_are_looked_up_by_timeline_position() {
        let mut l = Levels::default();
        let len = Time::from_millis(20);
        l.push(Time::ZERO, len, &[0.5, -0.25, 0.1, 0.1]);
        l.push(len, len, &[0.0, 0.0, -0.8, 0.9]);
        // 第 10 毫秒：只有第一块
        assert_eq!(
            l.at(Time::from_millis(10), Time::from_millis(5)),
            [0.5, 0.25]
        );
        // 第 30 毫秒、往前看 30 毫秒：两块都算
        assert_eq!(
            l.at(Time::from_millis(30), Time::from_millis(30)),
            [0.8, 0.9]
        );
        // 还没混到的地方：没有数据
        assert_eq!(l.at(Time::from_secs(5), Time::from_millis(30)), [0.0, 0.0]);
        l.clear();
        assert_eq!(
            l.at(Time::from_millis(10), Time::from_millis(5)),
            [0.0, 0.0]
        );
    }

    #[test]
    fn meter_scale_is_sixty_decibels() {
        assert_eq!(meter_fraction(0.0), 0.0);
        assert!((meter_fraction(1.0) - 1.0).abs() < 1e-6);
        assert!((meter_fraction(0.001) - 0.0).abs() < 1e-6);
        assert!((meter_fraction(0.1) - 2.0 / 3.0).abs() < 1e-4, "-20 dB");
    }
}
