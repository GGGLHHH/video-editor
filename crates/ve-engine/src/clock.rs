//! 播放时钟：以声卡实际取走的样本数为准，两次回调之间按系统时间插值。
//!
//! 声卡欠载时样本数不涨，时钟随之停住，画面就会等声音，不会音画漂移。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use ve_core::Time;

/// 两次声卡回调之间最多插值这么久（防止声卡停了时钟还一直走）。
pub const MAX_INTERPOLATION: Time = Time::from_millis(40);

/// 锚点状态：一起读写，避免读到两次 `start` 混在一起的值。
#[derive(Debug, Clone, Copy)]
struct Anchor {
    /// 本次开始（或停住）的位置。
    at: Time,
    playing: bool,
    /// 本次开始时样本计数器的值。
    base_played: u64,
    /// 本次开始后报出过的最大位置（保证单调）。
    reported: Time,
}

/// 播放时钟。
#[derive(Debug)]
pub struct PlayClock {
    sample_rate: u32,
    epoch: Instant,
    anchor: Mutex<Anchor>,
    /// 声卡累计取走的样本帧数（只增不减；输出回调里只做原子加）。
    played: AtomicU64,
    last_update_ns: AtomicU64,
}

impl PlayClock {
    /// 新时钟，停在 0。
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            epoch: Instant::now(),
            anchor: Mutex::new(Anchor {
                at: Time::ZERO,
                playing: false,
                base_played: 0,
                reported: Time::ZERO,
            }),
            played: AtomicU64::new(0),
            last_update_ns: AtomicU64::new(0),
        }
    }

    /// 采样率。
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn now_ns(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Anchor> {
        self.anchor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 是否在走。
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.lock().playing
    }

    /// 当前位置（播放中单调不减）。
    #[must_use]
    pub fn position(&self) -> Time {
        let mut a = self.lock();
        if !a.playing {
            return a.at;
        }
        let played = self
            .played
            .load(Ordering::Acquire)
            .saturating_sub(a.base_played);
        let since = self
            .now_ns()
            .saturating_sub(self.last_update_ns.load(Ordering::Acquire));
        let interp =
            Time::from_secs_f64(Duration::from_nanos(since).as_secs_f64()).min(MAX_INTERPOLATION);
        let p = a.at
            + Time::from_samples(i64::try_from(played).unwrap_or(i64::MAX), self.sample_rate)
            + interp;
        a.reported = a.reported.max(p);
        let out = a.reported;
        drop(a);
        out
    }

    /// 从 `at` 开始走。
    pub fn start(&self, at: Time) {
        let base_played = self.played.load(Ordering::Acquire);
        self.last_update_ns.store(self.now_ns(), Ordering::Release);
        *self.lock() = Anchor {
            at,
            playing: true,
            base_played,
            reported: at,
        };
    }

    /// 停住，返回停住的位置。
    pub fn stop(&self) -> Time {
        let at = self.position();
        let mut a = self.lock();
        a.playing = false;
        a.at = at;
        at
    }

    /// 跳到 `t`：走着就从 `t` 接着走，停着就停在 `t`。
    pub fn set_position(&self, t: Time) {
        if self.is_playing() {
            self.start(t);
        } else {
            self.lock().at = t;
        }
    }

    /// 声卡取走了 `frames` 个样本帧（由输出回调调用，只用原子操作）。
    pub fn advance(&self, frames: u64) {
        self.played.fetch_add(frames, Ordering::AcqRel);
        self.last_update_ns.store(self.now_ns(), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::thread::sleep;

    fn ms(v: i64) -> Time {
        Time::from_millis(v)
    }

    fn near(a: Time, b: Time, tol_ms: i64) -> bool {
        (a - b).0.abs() <= ms(tol_ms).0
    }

    #[test]
    fn paused_clock_is_frozen() {
        let c = PlayClock::new(48_000);
        c.set_position(ms(1500));
        sleep(Duration::from_millis(20));
        assert_eq!(c.position(), ms(1500));
        assert!(!c.is_playing());
    }

    #[test]
    fn clock_advances_with_consumed_samples() {
        let c = PlayClock::new(48_000);
        c.start(ms(1000));
        assert!(c.is_playing());
        c.advance(48_000);
        assert!(near(c.position(), ms(2000), 5), "{:?}", c.position());
    }

    #[test]
    fn interpolation_is_capped_when_samples_stop() {
        let c = PlayClock::new(48_000);
        c.start(Time::ZERO);
        sleep(Duration::from_millis(120));
        // 没有样本被取走：只插值到上限
        assert!(
            c.position() <= MAX_INTERPOLATION + ms(1),
            "{:?}",
            c.position()
        );
    }

    #[test]
    fn position_never_goes_backwards_while_playing() {
        let c = PlayClock::new(48_000);
        c.start(Time::ZERO);
        sleep(Duration::from_millis(15));
        let a = c.position();
        c.advance(48); // 1ms 的样本，比插值出来的少
        assert!(c.position() >= a);
    }

    #[test]
    fn concurrent_readers_never_resurrect_old_position() {
        // 读者线程不停读时钟，主线程反复跳回更早的位置：跳完之后不应读到旧的大值
        let c = std::sync::Arc::new(PlayClock::new(48_000));
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let readers: Vec<_> = (0..3)
            .map(|_| {
                let (c, stop) = (std::sync::Arc::clone(&c), std::sync::Arc::clone(&stop));
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let _ = c.position();
                    }
                })
            })
            .collect();
        let mut bad = 0;
        for _ in 0..20_000 {
            c.start(Time::from_secs(100));
            c.advance(48_000);
            c.start(Time::from_secs(1));
            if c.position() > Time::from_secs(2) {
                bad += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        for r in readers {
            r.join().unwrap();
        }
        assert_eq!(bad, 0, "跳回后读到旧位置 {bad} 次");
    }

    #[test]
    fn stop_freezes_and_restart_moves() {
        let c = PlayClock::new(48_000);
        c.start(ms(100));
        c.advance(4_800);
        let at = c.stop();
        assert!(near(at, ms(200), 5), "{at:?}");
        assert_eq!(c.position(), at);
        c.start(ms(5000));
        assert!(near(c.position(), ms(5000), 5));
        c.set_position(ms(7000));
        assert!(near(c.position(), ms(7000), 5));
        assert_eq!(c.sample_rate(), 48_000);
    }
}
