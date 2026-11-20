//! 播放引擎对外接口：设置工程、播放 / 暂停 / 定位，取当前该显示的帧。
//!
//! 线程：视频工作线程（解码、提前排 3 帧）、音频工作线程（解码混音写环形缓冲）、
//! 输出（声卡回调或空输出线程）。UI 线程只读帧队列和时钟，不做解码。
//!
//! 「代号」（generation）：每次播放 / 定位 / 换工程加一，工作线程据此丢弃旧状态，
//! 输出端据此清掉缓冲里的旧样本。

use crate::output::{DeviceOutput, OutputCtl, ring, spawn_null_output};
use crate::{PlayClock, audio_worker, video_worker};
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thiserror::Error;
use ve_core::{Project, Time};
use ve_media::{ScaleTarget, VideoFrame};

/// 音频输出方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioBackend {
    /// 默认声卡（打不开时自动退回 [`AudioBackend::Null`]）。
    Device,
    /// 不出声，按真实时间消耗样本（时钟照常走）。
    Null,
    /// 同 `Null`，并记录播出的样本（测试用）。
    Capture,
}

/// 引擎配置。
#[derive(Debug, Clone, Copy)]
pub struct EngineConfig {
    /// 预览画面尺寸上限（解码时直接缩放到这么大）。
    pub preview_max: ScaleTarget,
    /// 音频输出。
    pub audio: AudioBackend,
    /// 帧缓存字节数上限。
    pub cache_bytes: usize,
    /// 合成器配置（测试里用软件显卡）。
    pub render: ve_render::RenderConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            preview_max: ScaleTarget {
                max_width: 1920,
                max_height: 1080,
            },
            audio: AudioBackend::Device,
            cache_bytes: 512 * 1024 * 1024,
            render: ve_render::RenderConfig::default(),
        }
    }
}

/// 引擎事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineEvent {
    /// 暂停状态下有新帧可取（定位 / 换工程后）。
    FrameReady,
    /// 播到结尾，已自动暂停。
    Ended,
}

/// 一次要显示的画面；`frame` 为 `None` 表示该处没有视频（黑场）。
#[derive(Debug, Clone)]
pub struct FrameUpdate {
    /// 时间轴时刻。
    pub time: Time,
    /// 画面。
    pub frame: Option<Arc<VideoFrame>>,
}

/// 统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineStats {
    /// 真正解码过的帧数（不含缓存命中）。
    pub decoded_frames: u64,
    /// 解码器真正定位（往回或往前跳远，要从关键帧解起）的次数。
    pub seeks: u64,
}

/// 引擎错误。
#[derive(Debug, Error)]
pub enum EngineError {
    /// 起线程失败。
    #[error("启动工作线程失败：{0}")]
    Thread(#[from] std::io::Error),
}

pub(crate) struct Control {
    pub(crate) project: Arc<Project>,
    pub(crate) playing: bool,
    pub(crate) generation: u64,
    /// 暂停时要渲染的位置（拖动时只保留最新一个）。
    pub(crate) seek: Option<Time>,
    /// 本代开始播放的位置（音频线程从这里开始出）。
    pub(crate) start: Time,
    /// 工程修订号：播放中换工程只加它（不换代、不清音频缓冲、不重启时钟）。
    pub(crate) revision: u64,
    /// 播放倍数（J / K / L；负数倒放）。
    pub(crate) rate: f32,
    /// 预览画面尺寸上限（剪映的「原画质 / 清晰画质」切换会改它）。
    pub(crate) preview_max: ScaleTarget,
}

pub(crate) struct Queued {
    pub(crate) time: Time,
    pub(crate) frame: Option<Arc<VideoFrame>>,
    pub(crate) seq: u64,
}

#[derive(Default)]
pub(crate) struct FrameQueue {
    pub(crate) items: VecDeque<Queued>,
    pub(crate) next_seq: u64,
    pub(crate) last_taken: Option<u64>,
}

impl FrameQueue {
    pub(crate) fn push(&mut self, time: Time, frame: Option<Arc<VideoFrame>>) {
        self.next_seq += 1;
        self.items.push_back(Queued {
            time,
            frame,
            seq: self.next_seq,
        });
    }
}

pub(crate) struct Shared {
    pub(crate) control: Mutex<Control>,
    pub(crate) wake: Condvar,
    pub(crate) clock: Arc<PlayClock>,
    pub(crate) out: Arc<OutputCtl>,
    pub(crate) frames: Mutex<FrameQueue>,
    pub(crate) events: smol::channel::Sender<EngineEvent>,
    pub(crate) shutdown: Arc<AtomicBool>,
    pub(crate) decoded: AtomicU64,
    pub(crate) seeks: AtomicU64,
    pub(crate) preview_max: ScaleTarget,
    pub(crate) cache_bytes: usize,
    pub(crate) render: ve_render::RenderConfig,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    pub(crate) fn control(&self) -> MutexGuard<'_, Control> {
        lock(&self.control)
    }

    pub(crate) fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    /// 换代：工作线程丢旧状态，输出端丢旧样本。
    fn bump(&self, c: &mut Control, start: Time) {
        c.generation += 1;
        c.start = start;
        self.out.request_flush(c.generation);
        self.wake.notify_all();
    }

    /// 停在 `at`，并渲染该处画面。
    pub(crate) fn pause_at(&self, c: &mut Control, at: Time) {
        c.playing = false;
        self.clock.stop();
        self.clock.set_position(at);
        c.seek = Some(at);
        self.bump(c, at);
    }

    /// 等待唤醒或超时。
    pub(crate) fn wait(&self, guard: MutexGuard<'_, Control>, timeout: Duration) {
        drop(
            self.wake
                .wait_timeout(guard, timeout)
                .unwrap_or_else(PoisonError::into_inner),
        );
    }
}

/// 播放引擎。
pub struct Engine {
    shared: Arc<Shared>,
    events_rx: smol::channel::Receiver<EngineEvent>,
    threads: Vec<JoinHandle<()>>,
    _stream: Option<cpal::Stream>,
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("playing", &self.is_playing())
            .field("position", &self.position())
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// 启动引擎（空工程，停在 0）。
    ///
    /// # Errors
    /// 起线程失败。
    pub fn new(config: EngineConfig) -> Result<Self, EngineError> {
        let device = match config.audio {
            AudioBackend::Device => DeviceOutput::open_default()
                .inspect_err(|e| log::warn!("打不开声卡，改为静音播放：{e}"))
                .ok(),
            AudioBackend::Null | AudioBackend::Capture => None,
        };
        let rate = device
            .as_ref()
            .map_or(ve_media::OUTPUT_SAMPLE_RATE, DeviceOutput::sample_rate);
        let clock = Arc::new(PlayClock::new(rate));
        let out = Arc::new(OutputCtl::new(
            Arc::clone(&clock),
            config.audio == AudioBackend::Capture,
        ));
        let (events, events_rx) = smol::channel::unbounded();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            control: Mutex::new(Control {
                project: Arc::new(Project::new()),
                playing: false,
                generation: 0,
                seek: None,
                start: Time::ZERO,
                revision: 0,
                rate: 1.0,
                preview_max: config.preview_max,
            }),
            wake: Condvar::new(),
            clock,
            out: Arc::clone(&out),
            frames: Mutex::new(FrameQueue::default()),
            events,
            shutdown: Arc::clone(&shutdown),
            decoded: AtomicU64::new(0),
            seeks: AtomicU64::new(0),
            preview_max: config.preview_max,
            cache_bytes: config.cache_bytes,
            render: config.render,
        });

        let mut threads = Vec::new();
        let (mut producer, consumer) = ring(rate);
        let mut stream = None;
        let mut null_consumer = Some(consumer);
        if let Some(dev) = device
            && let Some(cons) = null_consumer.take()
        {
            match dev.start(cons, Arc::clone(&out)) {
                Ok(s) => stream = Some(s),
                Err(e) => {
                    log::warn!("声卡启动失败，改为静音播放：{e}");
                    let (p, c) = ring(rate);
                    producer = p;
                    null_consumer = Some(c);
                }
            }
        }
        if let Some(cons) = null_consumer {
            threads.push(spawn_null_output(
                cons,
                Arc::clone(&out),
                Arc::clone(&shutdown),
            )?);
        }
        let video_shared = Arc::clone(&shared);
        threads.push(
            thread::Builder::new()
                .name("ve-video".into())
                .spawn(move || video_worker::run(&video_shared))?,
        );
        let audio_shared = Arc::clone(&shared);
        threads.push(
            thread::Builder::new()
                .name("ve-audio".into())
                .spawn(move || audio_worker::run(&audio_shared, producer))?,
        );
        Ok(Self {
            shared,
            events_rx,
            threads,
            _stream: stream,
        })
    }

    /// 改预览画面尺寸上限（画质切换）：解码器按新尺寸重开，当前画面重新渲染。
    pub fn set_preview_max(&self, max: ScaleTarget) {
        let mut c = self.shared.control();
        if c.preview_max == max {
            return;
        }
        c.preview_max = max;
        c.revision += 1;
        if c.playing {
            self.shared.wake.notify_all();
        } else {
            let pos = self.shared.clock.position();
            c.seek = Some(pos);
            self.shared.bump(&mut c, pos);
        }
        drop(c);
    }

    /// 现在的预览画面尺寸上限。
    #[must_use]
    pub fn preview_max(&self) -> ScaleTarget {
        self.shared.control().preview_max
    }

    /// 换工程（编辑后调用）。播放中则从当前位置继续，暂停则重新渲染当前画面。
    /// 播放中：只换工程、加修订号——视频线程重排后面的帧，音频线程从已写到的位置接着用新工程混音，
    /// 不清空音频缓冲也不重启时钟，声音不断续（改动约 200ms 后听到）。
    pub fn set_project(&self, project: Arc<Project>) {
        let mut c = self.shared.control();
        c.project = project;
        c.revision += 1;
        if c.playing {
            self.shared.wake.notify_all();
        } else {
            let pos = self.shared.clock.position();
            c.seek = Some(pos);
            self.shared.bump(&mut c, pos);
        }
        drop(c);
    }

    /// 原速播放；已在结尾则从头开始。正在倍速 / 倒放时回到原速。
    pub fn play(&self) {
        if !self.is_playing() || (self.rate() - 1.0).abs() > f32::EPSILON {
            self.play_at_rate(1.0);
        }
    }

    /// 按倍数播放（剪映 J / L：负数倒放）。正放时已在结尾就从头开始，倒放时已在开头就从结尾开始。
    /// 倍速时声音变速不变调，倒放时声音倒着放。
    pub fn play_at_rate(&self, rate: f32) {
        let mut c = self.shared.control();
        let duration = c.project.timeline.duration();
        let mut pos = self.shared.clock.position().min(duration);
        if rate >= 0.0 && pos >= duration {
            pos = Time::ZERO;
        } else if rate < 0.0 && pos <= Time::ZERO {
            pos = duration;
        }
        c.playing = true;
        c.rate = rate;
        self.shared.clock.start_at_rate(pos, f64::from(rate));
        self.shared.bump(&mut c, pos);
        drop(c);
    }

    /// 当前播放倍数（暂停时是上一次的）。
    #[must_use]
    pub fn rate(&self) -> f32 {
        self.shared.control().rate
    }

    /// 暂停。
    pub fn pause(&self) {
        let mut c = self.shared.control();
        if c.playing {
            let at = self.shared.clock.position();
            self.shared.pause_at(&mut c, at);
        }
    }

    /// 是否在播放。
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.shared.control().playing
    }

    /// 定位（夹在 0 到时间轴总长之间）。拖动时可连续调用，暂停时只渲染最新的位置。
    pub fn seek(&self, t: Time) {
        let mut c = self.shared.control();
        let t = t.clamp(Time::ZERO, c.project.timeline.duration().max(Time::ZERO));
        if c.playing {
            self.shared.clock.start(t);
            self.shared.bump(&mut c, t);
        } else {
            self.shared.clock.set_position(t);
            c.seek = Some(t);
            self.shared.wake.notify_all();
        }
        drop(c);
    }

    /// 当前播放位置。
    #[must_use]
    pub fn position(&self) -> Time {
        let duration = self.duration();
        self.shared
            .clock
            .position()
            .clamp(Time::ZERO, duration.max(Time::ZERO))
    }

    /// 时间轴总长。
    #[must_use]
    pub fn duration(&self) -> Time {
        self.shared.control().project.timeline.duration()
    }

    /// 取当前该显示的帧；和上次相同则返回 `None`。UI 每帧调用。
    #[must_use]
    pub fn take_frame(&self) -> Option<FrameUpdate> {
        let playing = self.shared.clock.is_playing();
        let reverse = self.shared.clock.rate() < 0.0;
        let now = self.shared.clock.position();
        let mut q = lock(&self.shared.frames);
        // 正放：到点的最后一帧；暂停和倒放：最新的一帧（倒放的帧解好时时钟可能已经退过它了）
        let idx = if playing && !reverse {
            q.items.iter().rposition(|f| f.time <= now)?
        } else {
            q.items.len().checked_sub(1)?
        };
        q.items.drain(..idx);
        let front = q.items.front()?;
        if q.last_taken == Some(front.seq) {
            return None;
        }
        let update = FrameUpdate {
            time: front.time,
            frame: front.frame.clone(),
        };
        q.last_taken = Some(front.seq);
        drop(q);
        Some(update)
    }

    /// 事件流。
    #[must_use]
    pub fn events(&self) -> smol::channel::Receiver<EngineEvent> {
        self.events_rx.clone()
    }

    /// 统计。
    #[must_use]
    pub fn stats(&self) -> EngineStats {
        EngineStats {
            decoded_frames: self.shared.decoded.load(Ordering::Relaxed),
            seeks: self.shared.seeks.load(Ordering::Relaxed),
        }
    }

    /// 取出 [`AudioBackend::Capture`] 录到的样本（交错立体声）。
    #[must_use]
    pub fn captured_audio(&self) -> Vec<f32> {
        self.shared.out.take_captured()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.wake.notify_all();
        for t in self.threads.drain(..) {
            if t.join().is_err() {
                log::error!("引擎工作线程异常退出");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use ve_core::edit::{
        add_track, append_asset, delete_clips, insert_asset_at, set_transform, trim_end,
    };
    use ve_core::{Asset, AssetId, ClipId, FrameRate};
    use ve_media::testing::{Fixture, expected_gray, fixture};

    const RATE: FrameRate = FrameRate::new(30, 1);

    fn project_with(items: &[Fixture]) -> (Project, Vec<ClipId>) {
        // 和测试素材同尺寸：软件显卡合成小画布才快
        let mut p = small_project();
        let mut clips = Vec::new();
        for &f in items {
            let path = fixture(f);
            let r = ve_media::probe(&path).unwrap();
            let id = p.add_asset(Asset {
                id: AssetId(0),
                name: path.file_name().unwrap().to_string_lossy().into_owned(),
                path,
                duration: r.duration,
                video: r.video,
                audio: r.audio,
            });
            clips.push(append_asset(&mut p, id).unwrap());
        }
        (p, clips)
    }

    fn engine() -> Engine {
        Engine::new(EngineConfig {
            audio: AudioBackend::Capture,
            render: ve_render::RenderConfig { software: true },
            ..EngineConfig::default()
        })
        .unwrap()
    }

    fn wait_until(timeout_ms: u64, mut f: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        while std::time::Instant::now() < deadline {
            if f() {
                return true;
            }
            sleep(Duration::from_millis(2));
        }
        false
    }

    fn wait_frame(e: &Engine) -> FrameUpdate {
        let mut got = None;
        assert!(wait_until(3000, || {
            got = e.take_frame();
            got.is_some()
        }));
        got.unwrap()
    }

    /// 等到时刻 `t` 的那一帧（换工程本身也会先交一帧当前位置的画面，不能拿第一帧就当定位结果）。
    fn wait_frame_at(e: &Engine, t: Time) -> FrameUpdate {
        let mut got = None;
        assert!(wait_until(3000, || {
            got = e.take_frame().filter(|u| u.time == t);
            got.is_some()
        }));
        got.unwrap()
    }

    fn gray(f: &VideoFrame) -> f64 {
        let i = ((f.height / 2 * f.width + f.width / 2) * 4) as usize;
        f64::from(f.data[i])
    }

    #[test]
    fn preview_size_can_change_while_open() {
        let e = engine();
        e.set_project(Arc::new(project_with(&[Fixture::FrameIndexed30]).0));
        e.seek(Time::ZERO);
        let full = wait_frame_at(&e, Time::ZERO).frame.unwrap();
        assert_eq!((full.width, full.height), (64, 36));
        // 清晰画质：画面按一半大小解码、合成
        e.set_preview_max(ScaleTarget {
            max_width: 32,
            max_height: 18,
        });
        let small = wait_frame_at(&e, Time::ZERO).frame.unwrap();
        assert_eq!((small.width, small.height), (32, 18));
        assert!((gray(&small) - expected_gray(0)).abs() <= 2.0);
        assert_eq!(e.preview_max().max_width, 32);
    }

    #[test]
    fn seek_delivers_exact_frame() {
        let e = engine();
        e.set_project(Arc::new(project_with(&[Fixture::FrameIndexed30]).0));
        e.seek(RATE.time_of_frame(30) + Time::from_millis(10));
        let u = wait_frame_at(&e, RATE.time_of_frame(30) + Time::from_millis(10));
        let f = u.frame.unwrap();
        assert!((gray(&f) - expected_gray(30)).abs() <= 2.0, "{}", gray(&f));
        assert!(e.take_frame().is_none(), "同一帧只交一次");
        assert_eq!(e.duration(), Time::from_secs(2));
    }

    #[test]
    fn reverse_shows_earlier_frames_and_stops_at_start() {
        let e = engine();
        e.set_project(Arc::new(project_with(&[Fixture::FrameIndexed30]).0));
        let events = e.events();
        // 从 1.5 秒倒回去：并行跑测试时软件显卡要排队，留够出帧的时间
        e.seek(Time::from_millis(1500));
        e.play_at_rate(-1.0);
        assert!((e.rate() + 1.0).abs() < f32::EPSILON);
        let mut times = Vec::new();
        assert!(wait_until(4000, || {
            if let Some(u) = e.take_frame()
                && let Some(f) = u.frame
            {
                let index = RATE.frame_of(u.time);
                assert!(
                    (gray(&f) - expected_gray(index)).abs() <= 2.0,
                    "第 {index} 帧"
                );
                times.push(u.time);
            }
            matches!(events.try_recv(), Ok(EngineEvent::Ended))
        }));
        // 倒到开头自动暂停时会再交一次开头那帧（暂停画面）
        times.dedup();
        assert!(times.len() >= 3, "{times:?}");
        assert!(
            times.windows(2).all(|w| w[1] < w[0]),
            "时间往回走：{times:?}"
        );
        assert!(!e.is_playing());
        assert_eq!(e.position(), Time::ZERO);
        // 在开头按倒放：从结尾开始
        e.play_at_rate(-2.0);
        assert!(e.position() > Time::from_millis(1500));
        e.pause();
    }

    #[test]
    fn reverse_decodes_ahead_instead_of_seeking_every_frame() {
        // 往回跳要从关键帧解起：每帧都跳就又慢又卡。提前顺着解好一段放进缓存，
        // 2 秒倒完只该定位寥寥几次（每帧都跳是 60 次）
        let e = engine();
        e.set_project(Arc::new(project_with(&[Fixture::FrameIndexed30]).0));
        let events = e.events();
        e.seek(RATE.time_of_frame(59));
        drop(wait_frame_at(&e, RATE.time_of_frame(59)));
        let before = e.stats().seeks;
        e.play_at_rate(-1.0);
        let mut times = Vec::new();
        assert!(wait_until(5000, || {
            if let Some(u) = e.take_frame()
                && let Some(f) = u.frame
            {
                let index = RATE.frame_of(u.time);
                assert!(
                    (gray(&f) - expected_gray(index)).abs() <= 2.0,
                    "第 {index} 帧"
                );
                times.push(u.time);
            }
            matches!(events.try_recv(), Ok(EngineEvent::Ended))
        }));
        let seeks = e.stats().seeks - before;
        assert!(seeks <= 12, "定位了 {seeks} 次");
        times.dedup();
        assert!(times.len() >= 30, "倒放要连贯：只出了 {} 帧", times.len());
    }

    #[test]
    fn reverse_shows_a_late_frame_even_after_the_clock_passed_it() {
        // 倒放时解码一帧可能要很久（长 GOP 往回跳要从关键帧解起）：解好时时钟已经退到它前面了，
        // 这一帧还是要显示，不能因为「比现在晚」就扔掉
        let e = engine();
        e.shared.clock.start_at_rate(Time::from_millis(1000), -1.0);
        lock(&e.shared.frames).push(Time::from_millis(1500), None);
        let u = e.take_frame().expect("倒放时最新的一帧照样交出");
        assert_eq!(u.time, Time::from_millis(1500));
        e.shared.clock.stop();
    }

    #[test]
    fn fast_forward_runs_the_clock_faster_and_play_returns_to_normal() {
        let e = engine();
        e.set_project(Arc::new(
            project_with(&[Fixture::FrameIndexed30, Fixture::ToneStereo48k]).0,
        ));
        e.play_at_rate(4.0);
        sleep(Duration::from_millis(200));
        let fast = e.position();
        assert!(
            fast < Time::from_millis(1300),
            "不该比 4 倍快太多：{fast:?}"
        );
        assert!(
            fast > Time::from_millis(500),
            "4 倍速 0.2 秒应走过 0.8 秒左右：{fast:?}"
        );
        e.play();
        assert!((e.rate() - 1.0).abs() < f32::EPSILON);
        let at = e.position();
        sleep(Duration::from_millis(200));
        let walked = e.position() - at;
        assert!(walked < Time::from_millis(400), "原速：{walked:?}");
        e.pause();
    }

    #[test]
    fn play_advances_clock() {
        let e = engine();
        e.set_project(Arc::new(project_with(&[Fixture::FrameIndexed30]).0));
        e.play();
        assert!(e.is_playing());
        sleep(Duration::from_millis(300));
        let p = e.position();
        assert!(
            p > Time::from_millis(150) && p < Time::from_millis(600),
            "{p:?}"
        );
        e.pause();
        let p1 = e.position();
        sleep(Duration::from_millis(100));
        assert_eq!(e.position(), p1);
        assert!(!e.is_playing());
        assert!(wait_frame(&e).frame.is_some());
    }

    #[test]
    fn scrub_coalesces_to_latest() {
        let e = engine();
        e.set_project(Arc::new(project_with(&[Fixture::FrameIndexed30]).0));
        for n in 0..40 {
            e.seek(RATE.time_of_frame(n) + Time::from_millis(5));
        }
        let mut last = None;
        assert!(wait_until(3000, || {
            if let Some(u) = e.take_frame() {
                last = u.frame;
            }
            last.as_ref()
                .is_some_and(|f| (gray(f) - expected_gray(39)).abs() <= 2.0)
        }));
        assert!(e.stats().decoded_frames < 40, "{:?}", e.stats());
    }

    #[test]
    fn set_timeline_while_playing() {
        let e = engine();
        let (p, clips) = project_with(&[Fixture::FrameIndexed30, Fixture::ToneStereo48k]);
        e.set_project(Arc::new(p.clone()));
        e.play();
        sleep(Duration::from_millis(100));
        let mut without_video = p;
        delete_clips(&mut without_video, &[clips[0]]).unwrap();
        e.set_project(Arc::new(without_video));
        let before = e.position();
        // 视频片段没了：之后交出的帧应是黑场（None），播放继续
        assert!(wait_until(2000, || e
            .take_frame()
            .is_some_and(|u| u.frame.is_none())));
        assert!(e.is_playing());
        assert!(e.position() > before);
    }

    #[test]
    fn ended_event_at_timeline_end() {
        let e = engine();
        e.set_project(Arc::new(project_with(&[Fixture::FrameIndexed30]).0));
        let events = e.events();
        e.seek(Time::from_millis(1800));
        e.play();
        assert!(wait_until(3000, || matches!(
            events.try_recv(),
            Ok(EngineEvent::Ended)
        )));
        assert!(!e.is_playing());
        assert_eq!(e.position(), Time::from_secs(2));
        // 在结尾按播放：从头开始
        e.play();
        assert!(e.position() < Time::from_millis(500));
        e.pause();
    }

    #[test]
    fn audio_capture_plays_tone() {
        let e = engine();
        e.set_project(Arc::new(project_with(&[Fixture::ToneStereo48k]).0));
        e.play();
        sleep(Duration::from_millis(400));
        e.pause();
        let samples = e.captured_audio();
        assert!(samples.len() >= 48_000 / 4 * 2, "{}", samples.len());
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        let crossings = left
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        #[expect(clippy::cast_precision_loss, reason = "测试里样本数很小")]
        let hz = crossings as f64 / (left.len() as f64 / 48_000.0);
        assert!((hz - 440.0).abs() < 15.0, "{hz}");
        let peak = left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.05, "{peak}");
    }

    #[test]
    fn reused_asset_id_with_new_path_decodes_new_file() {
        // 撤销导入后再导入别的文件、或换工程时，素材编号会重复：必须按新路径重新解码
        let e = engine();
        let (first, _) = project_with(&[Fixture::FrameIndexed30]);
        e.set_project(Arc::new(first.clone()));
        e.seek(RATE.time_of_frame(30));
        let u = wait_frame_at(&e, RATE.time_of_frame(30));
        assert!((gray(&u.frame.unwrap()) - expected_gray(30)).abs() <= 2.0);
        let mut second = first;
        for a in second.assets.values_mut() {
            a.path = fixture(Fixture::Image);
            a.video = a.video.map(|v| ve_core::VideoInfo { still: true, ..v });
        }
        e.set_project(Arc::new(second));
        let f = wait_frame(&e).frame.unwrap();
        let px = &f.data[0..4];
        assert!(
            px[2] > 240 && px[0] < 15 && px[1] < 15,
            "应显示新文件（红图），实际 {px:?}"
        );
    }

    #[test]
    fn editing_while_playing_keeps_audio_continuous() {
        // 播放中每次编辑都清空约 200ms 的音频缓冲并重启时钟，会让声音断续（拖音量滑块时尤其明显）
        let e = engine();
        let (p, _) = project_with(&[Fixture::ToneStereo48k]);
        e.set_project(Arc::new(p.clone()));
        e.play();
        for _ in 0..5 {
            sleep(Duration::from_millis(80));
            e.set_project(Arc::new(p.clone()));
        }
        sleep(Duration::from_millis(100));
        e.pause();
        let left: Vec<f64> = e
            .captured_audio()
            .iter()
            .step_by(2)
            .map(|v| f64::from(*v))
            .collect();
        assert!(left.len() > 48_000 / 4, "{}", left.len());
        // 正弦波满足 x[n+1] = 2cos(w)x[n] − x[n−1]；有跳变就会大幅偏离
        let k = 2.0 * (std::f64::consts::TAU * 440.0 / 48_000.0).cos();
        let worst = left
            .windows(3)
            .map(|w| (w[2] - (k * w[1] - w[0])).abs())
            .fold(0.0f64, f64::max);
        assert!(worst < 0.01, "音频有跳变，最大偏差 {worst}");
    }

    #[test]
    fn empty_project_plays_to_end_immediately() {
        let e = engine();
        let events = e.events();
        e.play();
        assert!(wait_until(1000, || matches!(
            events.try_recv(),
            Ok(EngineEvent::Ended)
        )));
        assert!(!e.is_playing());
    }
    // ---------- 多层合成 ----------

    fn add(p: &mut Project, f: Fixture) -> AssetId {
        let path = fixture(f);
        let r = ve_media::probe(&path).unwrap();
        p.add_asset(Asset {
            id: AssetId(0),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            duration: r.duration,
            video: r.video,
            audio: r.audio,
        })
    }

    /// 64×36 的工程（测试素材也是 64×36，软件显卡合成快）。
    fn small_project() -> Project {
        let mut p = Project::new();
        p.settings.width = 64;
        p.settings.height = 36;
        p
    }

    fn half() -> ve_core::Transform {
        ve_core::Transform {
            scale: 0.5,
            ..ve_core::Transform::default()
        }
    }

    /// 在新的上层视频轨 `at` 处放素材，缩小一半居中。
    fn overlay(p: &mut Project, asset: AssetId, at: Time) -> ClipId {
        let track = add_track(p, ve_core::TrackKind::Video).unwrap();
        let clip = insert_asset_at(p, asset, track, at).unwrap();
        set_transform(p, clip, half()).unwrap();
        clip
    }

    fn bgr(f: &VideoFrame, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * f.width + x) * 4) as usize;
        [f.data[i], f.data[i + 1], f.data[i + 2]]
    }

    fn frame_at(p: Project, t: Time) -> Arc<VideoFrame> {
        let e = engine();
        e.set_project(Arc::new(p));
        e.seek(t);
        wait_frame_at(&e, t).frame.unwrap()
    }

    fn is_gray(px: [u8; 3], n: i64) -> bool {
        px.iter()
            .all(|c| (f64::from(*c) - expected_gray(n)).abs() <= 3.0)
    }

    const RED: [u8; 3] = [0, 0, 255];

    fn is_red(px: [u8; 3]) -> bool {
        px.iter().zip(RED).all(|(c, w)| c.abs_diff(w) <= 3)
    }

    #[test]
    fn two_layers_compose_pip() {
        let mut p = small_project();
        let gray = add(&mut p, Fixture::FrameIndexed30);
        let red = add(&mut p, Fixture::Image);
        append_asset(&mut p, gray).unwrap();
        overlay(&mut p, red, Time::ZERO);
        let f = frame_at(p, RATE.time_of_frame(15) + Time::from_millis(5));
        assert_eq!((f.width, f.height), (64, 36));
        assert!(is_red(bgr(&f, 32, 18)), "中心是上层：{:?}", bgr(&f, 32, 18));
        assert!(
            is_gray(bgr(&f, 2, 2), 15),
            "角上是下层：{:?}",
            bgr(&f, 2, 2)
        );
    }

    #[test]
    fn same_asset_two_tracks_independent() {
        let mut p = small_project();
        let gray = add(&mut p, Fixture::FrameIndexed30);
        append_asset(&mut p, gray).unwrap();
        overlay(&mut p, gray, Time::from_secs(1));
        // 1.5 秒：下层是素材第 45 帧，上层（从 1 秒开始）是素材第 15 帧
        let f = frame_at(p, RATE.time_of_frame(45) + Time::from_millis(5));
        assert!(is_gray(bgr(&f, 32, 18), 15), "上层：{:?}", bgr(&f, 32, 18));
        assert!(is_gray(bgr(&f, 2, 2), 45), "下层：{:?}", bgr(&f, 2, 2));
    }

    #[test]
    fn still_over_video_outlives_it() {
        let mut p = small_project();
        let gray = add(&mut p, Fixture::FrameIndexed30);
        let red = add(&mut p, Fixture::Image);
        append_asset(&mut p, gray).unwrap();
        overlay(&mut p, red, Time::ZERO);
        // 视频 2 秒就结束，图片（5 秒）还在：图片之外是黑底
        let f = frame_at(p, Time::from_secs(3));
        assert!(is_red(bgr(&f, 32, 18)), "{:?}", bgr(&f, 32, 18));
        assert_eq!(bgr(&f, 2, 2), [0, 0, 0]);
    }

    #[test]
    fn top_layer_ends_bottom_continues() {
        let mut p = small_project();
        let gray = add(&mut p, Fixture::FrameIndexed30);
        let red = add(&mut p, Fixture::Image);
        append_asset(&mut p, gray).unwrap();
        let top = overlay(&mut p, red, Time::ZERO);
        trim_end(&mut p, top, Time::from_secs(1)).unwrap();
        let f = frame_at(p, RATE.time_of_frame(45) + Time::from_millis(5));
        assert!(
            is_gray(bgr(&f, 32, 18), 45),
            "上层结束后只剩下层：{:?}",
            bgr(&f, 32, 18)
        );
    }

    #[test]
    fn identical_layers_decode_once() {
        // 同一素材、同一时刻叠两层（比如竖屏视频用自己的模糊版做背景）：只解一次
        let mut p = small_project();
        let gray = add(&mut p, Fixture::FrameIndexed30);
        append_asset(&mut p, gray).unwrap();
        overlay(&mut p, gray, Time::ZERO);
        let e = engine();
        e.set_project(Arc::new(p));
        let t = RATE.time_of_frame(10) + Time::from_millis(5);
        e.seek(t);
        let f = wait_frame_at(&e, t).frame.unwrap();
        assert!(is_gray(bgr(&f, 32, 18), 10));
        assert_eq!(e.stats().decoded_frames, 1, "{:?}", e.stats());
    }
}
