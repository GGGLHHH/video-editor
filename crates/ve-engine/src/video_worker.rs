//! 视频工作线程：暂停时渲染定位目标，播放时按时钟提前排 3 帧。

use crate::engine::{EngineEvent, Shared, lock};
use crate::{FrameCache, resolve_video};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use ve_core::{AssetId, Project, Time};
use ve_media::{VideoDecoder, VideoFrame};

/// 播放时最多提前排几帧。
const AHEAD: usize = 3;

struct Worker<'a> {
    shared: &'a Shared,
    /// 解码器及其打开的文件（素材编号可能被重复使用，要连路径一起比对）。
    decoders: HashMap<AssetId, (PathBuf, VideoDecoder)>,
    /// 打不开的素材及其路径。
    failed: HashMap<AssetId, PathBuf>,
    /// 帧缓存里每个素材对应的路径。
    cached: HashMap<AssetId, PathBuf>,
    cache: FrameCache,
    generation: Option<u64>,
    revision: Option<u64>,
    next_time: Option<Time>,
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
    };
    while !shared.is_shutdown() {
        w.step();
    }
}

impl Worker<'_> {
    fn step(&mut self) {
        let (project, playing, generation, seek, revision) = {
            let mut c = self.shared.control();
            (
                Arc::clone(&c.project),
                c.playing,
                c.generation,
                c.seek.take(),
                c.revision,
            )
        };
        if self.generation != Some(generation) || self.revision != Some(revision) {
            self.generation = Some(generation);
            self.revision = Some(revision);
            self.prune(&project);
            self.next_time = None;
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
        self.play_step(&project, generation);
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
        self.decoders.retain(|id, (path, _)| current(id, path));
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
    fn render(&mut self, project: &Project, t: Time) -> Option<Arc<VideoFrame>> {
        let src = resolve_video(project, t)?;
        let asset = project.assets.get(&src.asset)?;
        let info = asset.video?;
        let index = if info.still {
            0
        } else {
            info.frame_rate.frame_of(src.source_time)
        };
        if self
            .cached
            .get(&src.asset)
            .is_some_and(|p| *p == asset.path)
            && let Some(f) = self.cache.get((src.asset, index))
        {
            return Some(f);
        }
        if self
            .failed
            .get(&src.asset)
            .is_some_and(|p| *p == asset.path)
        {
            return None;
        }
        if self
            .decoders
            .get(&src.asset)
            .is_none_or(|(p, _)| *p != asset.path)
        {
            match VideoDecoder::open(&asset.path, self.shared.preview_max) {
                Ok(d) => {
                    self.decoders.insert(src.asset, (asset.path.clone(), d));
                }
                Err(e) => {
                    log::warn!("打不开视频 {}：{e}", asset.path.display());
                    self.failed.insert(src.asset, asset.path.clone());
                    return None;
                }
            }
        }
        if self.cached.get(&src.asset).is_none_or(|p| *p != asset.path) {
            self.cache.remove_asset(src.asset);
            self.cached.insert(src.asset, asset.path.clone());
        }
        let (_, decoder) = self.decoders.get_mut(&src.asset)?;
        match decoder.seek_to(src.source_time) {
            Ok(f) => {
                self.shared.decoded.fetch_add(1, Ordering::Relaxed);
                let f = Arc::new(f);
                self.cache.insert((src.asset, index), Arc::clone(&f));
                Some(f)
            }
            Err(e) => {
                log::warn!("解码失败 {}：{e}", asset.path.display());
                None
            }
        }
    }
}
