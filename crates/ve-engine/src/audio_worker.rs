//! 音频工作线程：按播放位置解码各片段音频、混音，写入环形缓冲。

use crate::engine::Shared;
use crate::{clamp_mix, mix_into, resolve_audio};
use rtrb::Producer;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ve_core::{AssetId, ClipId, Project, Time};
use ve_media::AudioDecoder;

/// 每次混音的样本帧数（48k 下约 21ms）。
const CHUNK: usize = 1024;

struct ClipAudio {
    asset: AssetId,
    /// 打开的文件（片段 / 素材编号可能被重复使用，要连路径一起比对）。
    path: PathBuf,
    decoder: AudioDecoder,
    /// 解码器下一次 `read` 会给出的素材时刻。
    next: Time,
}

/// 丢掉已删除的片段、换了素材或文件的解码器和失败记录。
fn prune(
    project: &Project,
    clips: &mut HashMap<ClipId, ClipAudio>,
    failed: &mut HashMap<AssetId, PathBuf>,
) {
    clips.retain(|clip, a| {
        project
            .timeline
            .find_clip(*clip)
            .and_then(|(_, c)| c.asset())
            == Some(a.asset)
            && project
                .assets
                .get(&a.asset)
                .is_some_and(|x| x.path == a.path)
    });
    failed.retain(|id, path| project.assets.get(id).is_some_and(|a| a.path == *path));
}

/// 每个片段的解码器 + 混音缓冲。
struct Mixer {
    rate: u32,
    chunk_time: Time,
    clips: HashMap<ClipId, ClipAudio>,
    failed: HashMap<AssetId, PathBuf>,
    tmp: Vec<f32>,
}

impl Mixer {
    /// 把时间轴 `[pos, pos + chunk)` 的所有音频段解码、混进 `mix`（先清零，最后限幅）。
    fn render(&mut self, project: &Project, pos: Time, mix: &mut [f32]) {
        let rate = self.rate;
        let one_sample = Time::from_samples(1, rate).max(Time(1));
        mix.fill(0.0);
        for seg in resolve_audio(project, pos, self.chunk_time) {
            let Some(asset) = project.assets.get(&seg.asset) else {
                continue;
            };
            if self
                .failed
                .get(&seg.asset)
                .is_some_and(|p| *p == asset.path)
            {
                continue;
            }
            let entry = match self.clips.get_mut(&seg.clip) {
                Some(e) if e.asset == seg.asset && e.path == asset.path => e,
                _ => match AudioDecoder::open_with_rate(&asset.path, rate) {
                    Ok(decoder) => self
                        .clips
                        .entry(seg.clip)
                        .insert_entry(ClipAudio {
                            asset: seg.asset,
                            path: asset.path.clone(),
                            decoder,
                            next: Time::ZERO,
                        })
                        .into_mut(),
                    Err(e) => {
                        log::warn!("打不开音频 {}：{e}", asset.path.display());
                        self.failed.insert(seg.asset, asset.path.clone());
                        continue;
                    }
                },
            };
            if (entry.next - seg.source_start).0.abs() > one_sample.0
                && let Err(e) = entry.decoder.seek_to(seg.source_start)
            {
                log::warn!("音频定位失败：{e}");
                continue;
            }
            let frames = usize::try_from(seg.duration.to_samples(rate))
                .unwrap_or(0)
                .min(CHUNK);
            self.tmp.resize(frames * 2, 0.0);
            let got = entry.decoder.read(&mut self.tmp).unwrap_or(0);
            entry.next =
                seg.source_start + Time::from_samples(i64::try_from(got).unwrap_or(0), rate);
            mix_into(mix, &self.tmp[..got * 2], &seg, pos, rate);
        }
        clamp_mix(mix);
    }
}

pub(crate) fn run(shared: &Shared, mut producer: Producer<f32>) {
    let rate = shared.clock.sample_rate();
    let chunk_time = Time::from_samples(i64::try_from(CHUNK).unwrap_or(0), rate);
    let mut mixer = Mixer {
        rate,
        chunk_time,
        clips: HashMap::new(),
        failed: HashMap::new(),
        tmp: Vec::with_capacity(CHUNK * 2),
    };
    let mut generation = None;
    let mut revision = None;
    let mut pos = Time::ZERO;
    let mut mix = vec![0.0f32; CHUNK * 2];

    while !shared.is_shutdown() {
        let (project, playing, current_gen, start, current_rev) = {
            let c = shared.control();
            (
                Arc::clone(&c.project),
                c.playing,
                c.generation,
                c.start,
                c.revision,
            )
        };
        if revision != Some(current_rev) {
            // 播放中换了工程：接着当前位置用新工程混音，只清理用不到的解码器
            revision = Some(current_rev);
            prune(&project, &mut mixer.clips, &mut mixer.failed);
        }
        if generation != Some(current_gen) {
            generation = Some(current_gen);
            pos = start;
            prune(&project, &mut mixer.clips, &mut mixer.failed);
            // 等输出端把旧样本清掉再写新的，否则新样本可能被一起清掉
            let deadline = Instant::now() + Duration::from_millis(100);
            while !shared.out.flushed(current_gen)
                && Instant::now() < deadline
                && !shared.is_shutdown()
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            continue;
        }
        if !playing {
            let c = shared.control();
            if c.generation == current_gen && !c.playing {
                shared.wait(c, Duration::from_millis(50));
            } else {
                drop(c);
            }
            continue;
        }
        if producer.slots() < CHUNK * 2 {
            std::thread::sleep(Duration::from_millis(3));
            continue;
        }

        mixer.render(&project, pos, &mut mix);
        if shared.control().generation != current_gen {
            continue;
        }
        if producer.push_entire_slice(&mix).is_ok() {
            pos += chunk_time;
        }
    }
}
