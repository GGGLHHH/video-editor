//! 变速不变调（WSOLA）：把素材按倍数压缩 / 拉长，音调不变（剪映「声音变调」关着的效果）。
//!
//! 每次从素材里取一段 `FRAME` 帧、加汉宁窗、以 `HOP` 帧的间隔叠加输出；取样位置按倍数前进，
//! 再在 ±`SEEK` 帧里找和上一段「自然接续」最像的位置，免得叠出来的波形相位对不上发颤。
//! 交错立体声，按流式喂入：`push` 素材、`pop` 输出，中间有一帧多的延迟。

use std::collections::VecDeque;

const CHANNELS: usize = 2;
/// 一段的长度（48k 下 32ms）。
const FRAME: usize = 1536;
/// 输出间隔（半段：汉宁窗 50% 叠加正好加起来是 1）。
const HOP: usize = FRAME / 2;
/// 找最像位置的范围。
const SEEK: usize = 256;

/// 流式的变速不变调处理器。
#[derive(Debug)]
pub struct Stretcher {
    speed: f32,
    /// 还要用到的素材（交错立体声），第 0 帧是绝对位置 `base`。
    input: Vec<f32>,
    base: usize,
    /// 下一段理想的取样位置（绝对帧号）。
    pos: f64,
    /// 上一段实际取样的位置。
    prev: Option<usize>,
    /// 叠加缓冲（一段长）。
    acc: Vec<f32>,
    out: VecDeque<f32>,
    window: Vec<f32>,
    finished: bool,
}

impl Stretcher {
    /// `speed`：素材时间 / 输出时间。
    #[must_use]
    #[expect(clippy::cast_precision_loss, reason = "窗口长度很小")]
    pub fn new(speed: f32) -> Self {
        let window = (0..FRAME)
            .map(|i| {
                let phase = std::f32::consts::TAU * i as f32 / FRAME as f32;
                0.5f32.mul_add(-phase.cos(), 0.5)
            })
            .collect();
        Self {
            speed,
            input: Vec::new(),
            base: 0,
            pos: 0.0,
            prev: None,
            acc: vec![0.0; FRAME * CHANNELS],
            out: VecDeque::new(),
            window,
            finished: false,
        }
    }

    /// 倍数。
    #[must_use]
    pub const fn speed(&self) -> f32 {
        self.speed
    }

    /// 喂素材（交错立体声）。
    pub fn push(&mut self, samples: &[f32]) {
        self.input.extend_from_slice(samples);
        self.process();
    }

    /// 素材喂完了：补静音把剩下的都处理掉。只做一次。
    pub fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            self.input.extend(std::iter::repeat_n(
                0.0,
                (FRAME + 2 * SEEK + HOP) * CHANNELS,
            ));
            self.process();
        }
    }

    /// 素材是不是已经喂完（`finish` 过）。
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished
    }

    /// 已经能取走的输出帧数。
    #[must_use]
    pub fn available(&self) -> usize {
        self.out.len() / CHANNELS
    }

    /// 取走最多 `frames` 帧输出。
    pub fn pop(&mut self, frames: usize) -> Vec<f32> {
        let n = frames.min(self.available()) * CHANNELS;
        self.out.drain(..n).collect()
    }

    const fn end(&self) -> usize {
        self.base + self.input.len() / CHANNELS
    }

    /// 绝对帧 `at` 起 `len` 帧的单声道（左右相加），给找位置用。
    fn mono(&self, at: usize, len: usize) -> impl Iterator<Item = f32> + '_ {
        let from = (at - self.base) * CHANNELS;
        self.input[from..from + len * CHANNELS]
            .as_chunks::<CHANNELS>()
            .0
            .iter()
            .map(|f| f.iter().sum())
    }

    /// 在 `[lo, hi]` 里找开头最像 `natural` 处的位置（归一化互相关最大）：先每 4 帧粗找，
    /// 再在最好的附近逐帧细找，计算量少四倍。
    fn best_match(&self, natural: usize, lo: usize, hi: usize) -> usize {
        let target: Vec<f32> = self.mono(natural, HOP).collect();
        let score = |cand: usize| {
            let (mut dot, mut energy) = (0.0f32, 1e-9f32);
            for (x, y) in self.mono(cand, HOP).zip(&target) {
                dot = x.mul_add(*y, dot);
                energy = x.mul_add(x, energy);
            }
            dot / energy.sqrt()
        };
        let pick = |range: &mut dyn Iterator<Item = usize>| {
            range
                .map(|c| (score(c), c))
                .fold(
                    (f32::MIN, lo),
                    |best, s| if s.0 > best.0 { s } else { best },
                )
                .1
        };
        let coarse = pick(&mut (lo..=hi).step_by(4));
        pick(&mut (coarse.saturating_sub(3).max(lo)..=(coarse + 3).min(hi)))
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "位置非负、远小于 2^52"
    )]
    fn process(&mut self) {
        loop {
            let ideal = (self.pos.round() as usize).max(self.base);
            let lo = ideal.saturating_sub(SEEK).max(self.base);
            let hi = ideal + SEEK;
            let need = self
                .prev
                .map_or(ideal + FRAME, |p| (hi + FRAME).max(p + HOP + HOP));
            if need > self.end() {
                break;
            }
            let start = self
                .prev
                .map_or(ideal, |p| self.best_match(p + HOP, lo, hi));
            let from = (start - self.base) * CHANNELS;
            for (i, w) in self.window.iter().enumerate() {
                for c in 0..CHANNELS {
                    let k = i * CHANNELS + c;
                    self.acc[k] = self.input[from + k].mul_add(*w, self.acc[k]);
                }
            }
            self.out.extend(&self.acc[..HOP * CHANNELS]);
            self.acc.copy_within(HOP * CHANNELS.., 0);
            self.acc[(FRAME - HOP) * CHANNELS..].fill(0.0);
            self.prev = Some(start);
            self.pos += HOP as f64 * f64::from(self.speed);
            // 丢掉以后用不到的素材：下一段最早从 pos - SEEK 或 start + HOP 开始看
            let keep = (self.pos.round() as usize)
                .saturating_sub(SEEK)
                .min(start + HOP)
                .max(self.base);
            self.input.drain(..(keep - self.base) * CHANNELS);
            self.base = keep;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn sine(hz: f32, secs: f32) -> Vec<f32> {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "测试数据"
        )]
        let n = (RATE * secs) as usize;
        #[expect(clippy::cast_precision_loss, reason = "测试数据")]
        (0..n)
            .flat_map(|i| {
                let v = (std::f32::consts::TAU * hz * i as f32 / RATE).sin() * 0.5;
                [v, v]
            })
            .collect()
    }

    /// 中间一段（去掉头尾）的频率：数过零点。
    #[expect(clippy::cast_precision_loss, reason = "测试数据")]
    fn frequency(samples: &[f32]) -> f32 {
        let left: Vec<f32> = samples.iter().step_by(CHANNELS).copied().collect();
        let mid = &left[left.len() / 4..left.len() * 3 / 4];
        let crossings = mid
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        crossings as f32 / 2.0 / (mid.len() as f32 / RATE)
    }

    fn stretch(speed: f32, input: &[f32]) -> Vec<f32> {
        let mut s = Stretcher::new(speed);
        // 分小块喂，模拟解码器
        for block in input.chunks(1000 * CHANNELS) {
            s.push(block);
        }
        s.finish();
        let n = s.available();
        s.pop(n)
    }

    #[test]
    fn keeps_the_pitch_and_changes_the_length() {
        let input = sine(440.0, 2.0);
        for speed in [2.0f32, 0.5, 1.5] {
            let out = stretch(speed, &input);
            #[expect(clippy::cast_precision_loss, reason = "测试数据")]
            let secs = out.len() as f32 / CHANNELS as f32 / RATE;
            let expected = 2.0 / speed;
            // 末尾补的静音会多出一点
            assert!(
                secs > expected - 0.05 && secs < expected + 0.1,
                "{speed}x：{secs} 秒"
            );
            let hz = frequency(&out);
            assert!((hz - 440.0).abs() < 440.0 * 0.03, "{speed}x：{hz} Hz");
            // 中间段的音量和原来差不多（叠加没发虚、没爆）
            let mid = &out[out.len() / 4..out.len() / 2];
            let peak = mid.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((peak - 0.5).abs() < 0.08, "{speed}x：峰值 {peak}");
        }
    }

    #[test]
    fn pop_is_limited_to_what_is_ready_and_finish_is_once() {
        let mut s = Stretcher::new(2.0);
        assert_eq!(s.available(), 0);
        assert_eq!(s.pop(10), [] as [f32; 0]);
        s.push(&sine(440.0, 0.01));
        assert!(!s.is_finished());
        s.finish();
        let ready = s.available();
        s.finish();
        assert_eq!(s.available(), ready, "第二次 finish 不再补静音");
        assert_eq!(s.pop(3).len(), 3 * CHANNELS);
        assert!((s.speed() - 2.0).abs() < 1e-6);
    }
}
