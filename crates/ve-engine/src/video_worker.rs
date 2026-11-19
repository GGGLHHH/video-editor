//! 视频工作线程：暂停时渲染定位目标，播放时按时钟提前排 3 帧。

use crate::engine::{EngineEvent, Shared, lock};
use crate::{FrameCache, LayerSource, preview_canvas, resolve_layers};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use ve_core::{AssetId, Project, Time, TrackId};
use ve_media::{MediaError, VideoDecoder, VideoFrame};
use ve_render::{Compositor, FramePixels, Layer};

/// 播放时最多提前排几帧。
const AHEAD: usize = 3;

/// 倒放时每段往回顺着解几帧（按取帧的间隔算）。
const REVERSE_BLOCK: i64 = 24;

/// 倒放时取帧的间隔：时钟每秒走过的帧数超过 60 时隔几帧取一帧。
fn reverse_stride(play_rate: f64, rate: ve_core::FrameRate) -> i64 {
    let per_second = play_rate.abs() * rate.as_f64();
    #[expect(clippy::cast_possible_truncation, reason = "倍数最多 8、帧率几十")]
    let stride = (per_second / 60.0).ceil() as i64;
    stride.max(1)
}

/// 倒放时往下提前解码的进度（工程帧号）。
#[derive(Debug, Clone, Copy)]
struct ReverseAhead {
    /// 从这帧往上（到当前）都已经解好放进缓存了。
    low: i64,
    /// 正在解的一段 `[from, to)`，下一个要解的是 `next`。
    from: i64,
    next: i64,
    to: i64,
}

impl ReverseAhead {
    /// 刚解出 `frame`：从它下面开始一段段往下解。
    const fn below(frame: i64) -> Self {
        Self {
            low: frame,
            from: frame,
            next: frame,
            to: frame,
        }
    }
}

struct Worker<'a> {
    shared: &'a Shared,
    /// 解码器及其打开的文件（素材编号可能被重复使用，要连路径一起比对）。
    /// 按「轨道 + 素材」各一个解码器：同一素材在两条轨上同时出现时互不抢着定位。
    /// 解码器按（轨道，素材，交替位）分：转场时同一条轨上前后两段同时解码，各用各的。
    decoders: HashMap<(TrackId, AssetId, u8), (PathBuf, VideoDecoder)>,
    /// 打不开的素材及其路径。
    failed: HashMap<AssetId, PathBuf>,
    /// 帧缓存里每个素材对应的路径。
    cached: HashMap<AssetId, PathBuf>,
    cache: FrameCache,
    generation: Option<u64>,
    revision: Option<u64>,
    next_time: Option<Time>,
    /// 倒放时往下提前解码到哪了。
    reverse: Option<ReverseAhead>,
    /// 合成器（第一次用到时创建）；创建失败后退回只显示最上层。
    compositor: Option<Compositor>,
    compositor_failed: bool,
}

pub(crate) fn run(shared: &Shared) {
    let mut w = Worker {
        shared,
        decoders: HashMap::new(),
        failed: HashMap::new(),
        cached: HashMap::new(),
        cache: FrameCache::new(shared.cache_bytes),
        generation: None,
        revision: None,
        next_time: None,
        reverse: None,
        compositor: None,
        compositor_failed: false,
    };
    while !shared.is_shutdown() {
        w.step();
    }
}

impl Worker<'_> {
    fn step(&mut self) {
        let (project, playing, generation, seek, revision, play_rate) = {
            let mut c = self.shared.control();
            (
                Arc::clone(&c.project),
                c.playing,
                c.generation,
                c.seek.take(),
                c.revision,
                c.rate,
            )
        };
        if self.generation != Some(generation) || self.revision != Some(revision) {
            self.generation = Some(generation);
            self.revision = Some(revision);
            self.prune(&project);
            self.next_time = None;
            self.reverse = None;
            if playing {
                lock(&self.shared.frames).items.clear();
            }
        }
        if !playing {
            if let Some(t) = seek {
                let shown = t.min(
                    project.timeline.duration() - project.settings.frame_rate.frame_duration(),
                );
                let frame = self.render(&project, shown.max(Time::ZERO));
                let mut q = lock(&self.shared.frames);
                q.items.clear();
                q.push(t, frame);
                drop(q);
                let _ = self.shared.events.try_send(EngineEvent::FrameReady);
            } else {
                let c = self.shared.control();
                if c.seek.is_none()
                    && c.generation == generation
                    && !c.playing
                    && !self.shared.is_shutdown()
                {
                    self.shared.wait(c, Duration::from_millis(100));
                }
            }
            return;
        }
        if play_rate < 0.0 {
            self.reverse_step(&project, generation);
        } else {
            self.play_step(&project, generation);
        }
    }

    /// 倒放（J）。解码器只能往后解，往回跳要从关键帧解起：每帧都跳又慢又卡。所以先把
    /// 时钟所在那帧直接解出来，再在它下面一段一段地「顺着」解（每段只定位一次），放进帧缓存，
    /// 时钟退到哪帧就从缓存里取。倍数高时隔几帧取一帧（屏幕也就刷新那么快）。倒到开头自动暂停。
    fn reverse_step(&mut self, project: &Project, generation: u64) {
        let rate = project.settings.frame_rate;
        let fd = rate.frame_duration();
        let now = self.shared.clock.position();
        if now <= Time::ZERO {
            let mut c = self.shared.control();
            if c.playing && c.generation == generation {
                self.shared.pause_at(&mut c, Time::ZERO);
                drop(c);
                let _ = self.shared.events.try_send(EngineEvent::Ended);
            }
            return;
        }
        let stride = reverse_stride(self.shared.clock.rate(), rate);
        let last = rate.frame_of((project.timeline.duration() - fd).max(Time::ZERO));
        let here = rate.frame_of(now).min(last);
        let here = here - here.rem_euclid(stride);
        let current = rate.time_of_frame(here);
        if self.next_time != Some(current) {
            // 正在往上解、还没解到这帧：等它解到，不另外往回跳
            let coming = self
                .reverse
                .as_ref()
                .is_some_and(|r| (r.next..r.to).contains(&here));
            if !coming || self.is_cached(project, current) {
                if !self.is_cached(project, current) {
                    // 缓存里没有（刚开始倒放，或者解码跟不上被时钟甩下了）：直接解这一帧，
                    // 从这里往下重新一段段解
                    self.reverse = Some(ReverseAhead::below(here));
                }
                let frame = self.render(project, current);
                if self.shared.control().generation != generation {
                    return;
                }
                let mut q = lock(&self.shared.frames);
                q.items.clear();
                q.push(current, frame);
                drop(q);
                self.next_time = Some(current);
                return;
            }
        }
        let Some(ahead) = self.reverse.as_mut() else {
            self.reverse = Some(ReverseAhead::below(here));
            return;
        };
        if ahead.next >= ahead.to {
            // 这段解完了；剩下的不到一段时接着解下面一段
            ahead.low = ahead.low.min(ahead.from);
            if ahead.low > 0 && here - ahead.low < REVERSE_BLOCK * stride {
                ahead.to = ahead.low;
                ahead.from = (ahead.low - REVERSE_BLOCK * stride).max(0);
                ahead.next = ahead.from;
            } else {
                let c = self.shared.control();
                if c.generation == generation {
                    self.shared.wait(c, Duration::from_millis(4));
                } else {
                    drop(c);
                }
                return;
            }
        }
        let t = rate.time_of_frame(ahead.next);
        ahead.next += stride;
        let layers = resolve_layers(project, t);
        drop(self.layer_frames(project, &layers));
    }

    /// 时间轴时刻 `t` 的各层画面是不是都已经在缓存里。
    fn is_cached(&self, project: &Project, t: Time) -> bool {
        resolve_layers(project, t).iter().all(|l| {
            let Some(asset) = project.assets.get(&l.asset) else {
                return true;
            };
            let Some(info) = asset.video else { return true };
            let index = if info.still {
                0
            } else {
                info.frame_rate.frame_of(l.source_time)
            };
            self.failed.get(&l.asset).is_some_and(|p| *p == asset.path)
                || (self.cached.get(&l.asset).is_some_and(|p| *p == asset.path)
                    && self.cache.contains((l.asset, index)))
        })
    }

    fn play_step(&mut self, project: &Project, generation: u64) {
        let rate = project.settings.frame_rate;
        let fd = rate.frame_duration();
        let duration = project.timeline.duration();
        let now = self.shared.clock.position();
        if now >= duration {
            let mut c = self.shared.control();
            if c.playing && c.generation == generation {
                self.shared.pause_at(&mut c, duration);
                drop(c);
                let _ = self.shared.events.try_send(EngineEvent::Ended);
            }
            return;
        }
        let ahead = {
            let mut q = lock(&self.shared.frames);
            // 只留一帧「当前」（≤ now 的最后一帧），更早的丢掉
            while q.items.get(1).is_some_and(|f| f.time <= now) {
                q.items.pop_front();
            }
            q.items.iter().filter(|f| f.time > now).count()
        };
        let current = rate.time_of_frame(rate.frame_of(now));
        let t = match self.next_time {
            // 解码跟不上：直接跳到当前帧（丢帧，不拖慢时钟）
            Some(t) if t + fd > now => t,
            _ => current,
        };
        if ahead >= AHEAD || t >= duration {
            let c = self.shared.control();
            if c.generation == generation {
                self.shared.wait(c, Duration::from_millis(4));
            } else {
                drop(c);
            }
            return;
        }
        let frame = self.render(project, t);
        if self.shared.control().generation != generation {
            return;
        }
        lock(&self.shared.frames).push(t, frame);
        self.next_time = Some(t + fd);
    }

    /// 换代时丢掉工程里已不存在、或换了文件的素材的解码器 / 失败记录 / 缓存。
    fn prune(&mut self, project: &Project) {
        let current =
            |id: &AssetId, path: &Path| project.assets.get(id).is_some_and(|a| a.path == path);
        self.decoders.retain(|(track, id, _), (path, _)| {
            current(id, path) && project.timeline.track(*track).is_some()
        });
        self.failed.retain(|id, path| current(id, path));
        let stale: Vec<AssetId> = self
            .cached
            .iter()
            .filter(|(id, p)| !current(id, p))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.cache.remove_asset(id);
            self.cached.remove(&id);
        }
    }

    /// 时间轴时刻 `t` 的画面（缓存优先）；没有视频或解码失败返回 `None`。
    /// 合成时间点 `t` 的预览画面；没有任何画面（空隙或全都解不出）、也没有调节片段把黑底调色时
    /// 返回 `None`。
    fn render(&mut self, project: &Project, t: Time) -> Option<Arc<VideoFrame>> {
        let ve_core::compose::Composition { layers, background } =
            ve_core::compose::resolve_frame(project, t);
        let frames = self.layer_frames(project, &layers);
        if frames.iter().all(Option::is_none) && background.is_identity() {
            return None;
        }
        let canvas = preview_canvas(&project.settings, self.shared.preview_max);
        if self.compositor.is_none() && !self.compositor_failed {
            match Compositor::new(&self.shared.render) {
                Ok(c) => {
                    log::info!("预览合成用显卡：{}", c.adapter_name());
                    self.compositor = Some(c);
                }
                Err(e) => {
                    log::error!("合成器创建失败，预览只显示最上层：{e}");
                    self.compositor_failed = true;
                }
            }
        }
        let top = || frames.iter().rev().flatten().next().cloned();
        let Some(compositor) = self.compositor.as_mut() else {
            return top();
        };
        let gpu_layers: Vec<Layer<'_>> = layers
            .iter()
            .zip(&frames)
            .filter_map(|(l, f)| {
                let f = f.as_ref()?;
                let info = project.assets.get(&l.asset)?.video?;
                let source = if info.width > 0 && info.height > 0 {
                    (info.width, info.height)
                } else {
                    (f.width, f.height)
                };
                Some(Layer {
                    pixels: FramePixels::Bgra {
                        width: f.width,
                        height: f.height,
                        data: &f.data,
                    },
                    geometry: ve_core::layer_geometry(canvas, source, &l.transform),
                    opacity: l.transform.opacity,
                    color: l.color,
                })
            })
            .collect();
        compositor.set_background(&background);
        let mut data = Vec::new();
        match compositor.compose_bgra(canvas, &gpu_layers, &mut data) {
            Ok(()) => Some(Arc::new(VideoFrame {
                pts: t,
                width: canvas.0,
                height: canvas.1,
                data,
            })),
            Err(e) => {
                log::warn!("合成失败，显示最上层：{e}");
                top()
            }
        }
    }

    /// 各层的源画面：先查缓存，缺的并行解码。
    fn layer_frames(
        &mut self,
        project: &Project,
        layers: &[LayerSource],
    ) -> Vec<Option<Arc<VideoFrame>>> {
        let mut frames = vec![None; layers.len()];
        let mut jobs = Vec::new();
        // 同一帧（素材 + 帧号）被几层同时用到时只解一次，结果分给这些层
        let mut wanted: Vec<((AssetId, i64), Vec<usize>)> = Vec::new();
        for (i, l) in layers.iter().enumerate() {
            let Some(asset) = project.assets.get(&l.asset) else {
                continue;
            };
            let Some(info) = asset.video else { continue };
            let index = if info.still {
                0
            } else {
                info.frame_rate.frame_of(l.source_time)
            };
            if self.cached.get(&l.asset).is_some_and(|p| *p == asset.path)
                && let Some(f) = self.cache.get((l.asset, index))
            {
                frames[i] = Some(f);
                continue;
            }
            if self.failed.get(&l.asset).is_some_and(|p| *p == asset.path) {
                continue;
            }
            if let Some((_, slots)) = wanted.iter_mut().find(|(k, _)| *k == (l.asset, index)) {
                slots.push(i);
                continue;
            }
            let key = (l.track, l.asset, l.lane);
            if self
                .decoders
                .get(&key)
                .is_none_or(|(p, _)| *p != asset.path)
            {
                match VideoDecoder::open(&asset.path, self.shared.preview_max) {
                    Ok(d) => {
                        self.decoders.insert(key, (asset.path.clone(), d));
                    }
                    Err(e) => {
                        log::warn!("打不开 {}：{e}", asset.path.display());
                        self.failed.insert(l.asset, asset.path.clone());
                        continue;
                    }
                }
            }
            if self.cached.get(&l.asset).is_none_or(|p| *p != asset.path) {
                self.cache.remove_asset(l.asset);
                self.cached.insert(l.asset, asset.path.clone());
            }
            // 同一个解码器同一帧只排一次（同一轨同一素材不会出现两次，这里只是保险）
            if let Some((path, decoder)) = self.decoders.remove(&key) {
                wanted.push(((l.asset, index), vec![i]));
                jobs.push(Job {
                    slot: i,
                    key,
                    index,
                    time: l.source_time,
                    path,
                    seeks_before: decoder.seeks(),
                    decoder,
                });
            }
        }
        for (job, result) in decode_all(jobs) {
            self.shared
                .seeks
                .fetch_add(job.decoder.seeks() - job.seeks_before, Ordering::Relaxed);
            match result {
                Ok(f) => {
                    self.shared.decoded.fetch_add(1, Ordering::Relaxed);
                    let f = Arc::new(f);
                    self.cache.insert((job.key.1, job.index), Arc::clone(&f));
                    let same = wanted
                        .iter()
                        .find(|(k, _)| *k == (job.key.1, job.index))
                        .map_or(&[][..], |(_, slots)| slots.as_slice());
                    for &slot in same {
                        frames[slot] = Some(Arc::clone(&f));
                    }
                    frames[job.slot] = Some(f);
                }
                Err(e) => log::warn!("解码失败 {}：{e}", job.path.display()),
            }
            self.decoders.insert(job.key, (job.path, job.decoder));
        }
        frames
    }
}

/// 一层的解码任务：解码器暂时从表里拿出来，解完再放回去。
struct Job {
    slot: usize,
    key: (TrackId, AssetId, u8),
    index: i64,
    time: Time,
    path: PathBuf,
    decoder: VideoDecoder,
    /// 解码前解码器定位过几次（解完相减就是这次定位了没有）。
    seeks_before: u64,
}

/// 解码各层；多于一层时每层一个线程并行。
fn decode_all(jobs: Vec<Job>) -> Vec<(Job, Result<VideoFrame, MediaError>)> {
    let decode = |mut job: Job| {
        let r = job.decoder.seek_to(job.time);
        (job, r)
    };
    if jobs.len() <= 1 {
        return jobs.into_iter().map(decode).collect();
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .into_iter()
            .map(|job| scope.spawn(move || decode(job)))
            .collect();
        handles
            .into_iter()
            .filter_map(|h| {
                h.join()
                    .map_err(|_| log::error!("解码线程出错，这一层这一帧不显示"))
                    .ok()
            })
            .collect()
    })
}
