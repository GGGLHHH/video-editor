//! 音频输出：环形缓冲 → 声卡回调（或测试用的空输出线程）。
//!
//! 回调里不加锁、不分配（只在缓冲不够大时扩一次）。

use crate::PlayClock;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Consumer, RingBuffer};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
#[cfg(test)]
use ve_core::Time;

/// 输出端控制：时钟、清空请求、测试用录音。
#[derive(Debug)]
pub struct OutputCtl {
    /// 播放时钟（输出端推进）。
    pub clock: Arc<PlayClock>,
    flush_gen: AtomicU64,
    ack_gen: AtomicU64,
    capture: Option<Mutex<Vec<f32>>>,
}

impl OutputCtl {
    /// 新建；`capture` 为真时把真正播出的样本记下来（测试用）。
    #[must_use]
    pub fn new(clock: Arc<PlayClock>, capture: bool) -> Self {
        Self {
            clock,
            flush_gen: AtomicU64::new(0),
            ack_gen: AtomicU64::new(0),
            capture: capture.then(|| Mutex::new(Vec::new())),
        }
    }

    /// 请求输出端丢掉缓冲里所有旧样本（代号 `generation`）。
    pub fn request_flush(&self, generation: u64) {
        self.flush_gen.store(generation, Ordering::Release);
    }

    /// 输出端是否已处理到代号 `generation` 的清空请求。
    #[must_use]
    pub fn flushed(&self, generation: u64) -> bool {
        self.ack_gen.load(Ordering::Acquire) >= generation
    }

    /// 取出录到的样本（交错立体声）。
    #[must_use]
    pub fn take_captured(&self) -> Vec<f32> {
        self.capture.as_ref().map_or_else(Vec::new, |c| {
            std::mem::take(&mut *c.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
        })
    }
}

/// 从环形缓冲（交错立体声）取样本填进声卡缓冲（`channels` 声道交错），返回真正播出的帧数。
/// 暂停时输出静音且不取样本；缓冲不够时不足部分补静音。
pub fn fill_output(
    cons: &mut Consumer<f32>,
    out: &mut [f32],
    channels: usize,
    ctl: &OutputCtl,
) -> usize {
    let wanted_gen = ctl.flush_gen.load(Ordering::Acquire);
    if ctl.ack_gen.load(Ordering::Acquire) != wanted_gen {
        if let Ok(chunk) = cons.read_chunk(cons.slots()) {
            chunk.commit_all();
        }
        ctl.ack_gen.store(wanted_gen, Ordering::Release);
    }
    out.fill(0.0);
    if channels == 0 || !ctl.clock.is_playing() {
        return 0;
    }
    let frames = (out.len() / channels).min(cons.slots() / 2);
    let Ok(chunk) = cons.read_chunk(frames * 2) else {
        return 0;
    };
    let (a, b) = chunk.as_slices();
    let mut src = a.iter().chain(b.iter()).copied();
    let mut captured = ctl
        .capture
        .as_ref()
        .map(|c| c.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    for frame in out.chunks_exact_mut(channels).take(frames) {
        let (l, r) = (src.next().unwrap_or(0.0), src.next().unwrap_or(0.0));
        if let [only] = frame {
            *only = f32::midpoint(l, r);
        } else if let [fl, fr, ..] = frame {
            *fl = l;
            *fr = r;
        }
        if let Some(c) = captured.as_mut() {
            c.extend([l, r]);
        }
    }
    drop(captured);
    chunk.commit_all();
    ctl.clock.advance(frames as u64);
    frames
}

/// 没有声卡（或测试）时的输出：一个按真实时间消耗样本的线程。
///
/// # Errors
/// 起线程失败。
pub fn spawn_null_output(
    mut cons: Consumer<f32>,
    ctl: Arc<OutputCtl>,
    shutdown: Arc<AtomicBool>,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("ve-null-audio".into())
        .spawn(move || {
            let rate = f64::from(ctl.clock.sample_rate());
            let mut last = Instant::now();
            let mut owed = 0.0f64;
            let mut buf = Vec::new();
            while !shutdown.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
                let now = Instant::now();
                owed = now.duration_since(last).as_secs_f64().mul_add(rate, owed);
                last = now;
                let whole = owed.floor();
                owed -= whole;
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "几毫秒的样本数"
                )]
                let frames = whole as usize;
                buf.resize(frames * 2, 0.0);
                fill_output(&mut cons, &mut buf, 2, &ctl);
            }
        })
}

/// 声卡设备与配置。
pub struct DeviceOutput {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
}

impl std::fmt::Debug for DeviceOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceOutput")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl DeviceOutput {
    /// 默认输出设备及其默认配置；没有设备返回错误说明。
    ///
    /// # Errors
    /// 没有输出设备或取不到配置。
    pub fn open_default() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "没有音频输出设备".to_owned())?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        Ok(Self { device, config })
    }

    /// 采样率。
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate()
    }

    /// 开始输出。
    ///
    /// # Errors
    /// 设备不支持该格式或无法启动。
    pub fn start(self, cons: Consumer<f32>, ctl: Arc<OutputCtl>) -> Result<cpal::Stream, String> {
        let channels = usize::from(self.config.channels());
        let config: cpal::StreamConfig = self.config.config();
        let err = |e| log::warn!("音频输出出错：{e}");
        let stream = match self.config.sample_format() {
            cpal::SampleFormat::F32 => {
                let mut cons = cons;
                self.device.build_output_stream(
                    config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        fill_output(&mut cons, data, channels, &ctl);
                    },
                    err,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let mut cons = cons;
                let mut scratch = Vec::new();
                self.device.build_output_stream(
                    config,
                    move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        scratch.resize(data.len(), 0.0);
                        fill_output(&mut cons, &mut scratch, channels, &ctl);
                        for (d, s) in data.iter_mut().zip(&scratch) {
                            *d = cpal::Sample::from_sample(*s);
                        }
                    },
                    err,
                    None,
                )
            }
            other => return Err(format!("不支持的声卡格式 {other}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(stream)
    }
}

/// 环形缓冲：容量约 200ms。
#[must_use]
pub fn ring(sample_rate: u32) -> (rtrb::Producer<f32>, Consumer<f32>) {
    RingBuffer::new((sample_rate as usize / 5).max(1024) * 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctl() -> OutputCtl {
        OutputCtl::new(Arc::new(PlayClock::new(48_000)), false)
    }

    #[test]
    fn ring_buffer_underrun_outputs_silence() {
        let ctl = ctl();
        ctl.clock.start(Time::ZERO);
        let (mut prod, mut cons) = RingBuffer::<f32>::new(64);
        let _ = prod.push_partial_slice(&[0.5, -0.5, 0.25, -0.25]); // 2 帧
        let mut out = vec![9.0f32; 8]; // 4 帧立体声
        let real = fill_output(&mut cons, &mut out, 2, &ctl);
        assert_eq!(real, 2);
        assert_eq!(out, vec![0.5, -0.5, 0.25, -0.25, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn paused_outputs_silence_and_keeps_samples() {
        let ctl = ctl();
        let (mut prod, mut cons) = RingBuffer::<f32>::new(64);
        let _ = prod.push_partial_slice(&[0.5, 0.5]);
        let mut out = vec![1.0f32; 4];
        assert_eq!(fill_output(&mut cons, &mut out, 2, &ctl), 0);
        assert_eq!(out, vec![0.0; 4]);
        assert_eq!(cons.slots(), 2);
    }

    #[test]
    fn flush_drops_stale_samples_and_acks() {
        let ctl = ctl();
        ctl.clock.start(Time::ZERO);
        let (mut prod, mut cons) = RingBuffer::<f32>::new(64);
        let _ = prod.push_partial_slice(&[0.9; 10]);
        ctl.request_flush(7);
        let mut out = vec![1.0f32; 4];
        assert_eq!(fill_output(&mut cons, &mut out, 2, &ctl), 0);
        assert_eq!(out, vec![0.0; 4]);
        assert_eq!(cons.slots(), 0);
        assert!(ctl.flushed(7));
    }

    #[test]
    fn maps_stereo_to_mono_and_surround() {
        let ctl = ctl();
        ctl.clock.start(Time::ZERO);
        let (mut prod, mut cons) = RingBuffer::<f32>::new(64);
        let _ = prod.push_partial_slice(&[0.2, 0.4, 1.0, 0.0]);
        let mut mono = vec![0.0f32; 1];
        fill_output(&mut cons, &mut mono, 1, &ctl);
        assert!((mono[0] - 0.3).abs() < 1e-6);
        let mut six = vec![9.0f32; 6];
        fill_output(&mut cons, &mut six, 6, &ctl);
        assert_eq!(six, vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn capture_records_real_samples_and_advances_clock() {
        let ctl = OutputCtl::new(Arc::new(PlayClock::new(48_000)), true);
        ctl.clock.start(Time::ZERO);
        let (mut prod, mut cons) = RingBuffer::<f32>::new(64);
        let _ = prod.push_partial_slice(&[0.1, 0.2, 0.3, 0.4]);
        let mut out = vec![0.0f32; 8];
        fill_output(&mut cons, &mut out, 2, &ctl);
        assert_eq!(ctl.take_captured(), vec![0.1, 0.2, 0.3, 0.4]);
        assert!(ctl.clock.position() >= Time::from_samples(2, 48_000));
    }
}
