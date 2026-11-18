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
    /// 下一块应该从哪个素材时刻接着放（对不上就是跳了，要重新定位）。
    next: Time,
    /// 变速且保持音调时的拉伸器（解码器读在它前面，`next` 按输出算）。
    stretch: Option<crate::stretch::Stretcher>,
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
    /// 穿梭播放（倍速、倒放）的状态；原速时没有。
    shuttle: Option<Shuttle>,
}

impl Mixer {
    /// 把时间轴从 `pos` 起、`mix` 那么长的所有音频段解码、混进 `mix`（先清零，最后限幅）。
    fn render(&mut self, project: &Project, pos: Time, mix: &mut [f32]) {
        let rate = self.rate;
        let one_sample = Time::from_samples(1, rate).max(Time(1));
        let mix_frames = mix.len() / 2;
        let span = Time::from_samples(i64::try_from(mix_frames).unwrap_or(0), rate);
        mix.fill(0.0);
        for seg in resolve_audio(project, pos, span) {
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
                            stretch: None,
                        })
                        .into_mut(),
                    Err(e) => {
                        log::warn!("打不开音频 {}：{e}", asset.path.display());
                        self.failed.insert(seg.asset, asset.path.clone());
                        continue;
                    }
                },
            };
            // 变速时素材时间按倍数换算，相邻两块之间会差一两个样本：容差跟着倍数放大，免得每块都重新定位
            let speed = f64::from(seg.speed);
            let tolerance = one_sample.scaled(2.0 * speed.max(1.0));
            #[expect(clippy::float_cmp, reason = "原速是精确的 1.0")]
            let stretching = seg.keep_pitch && seg.speed != 1.0;
            let jumped = (entry.next - seg.source_start).0.abs() > tolerance.0;
            let stale = entry
                .stretch
                .as_ref()
                .is_some_and(|s| !stretching || (s.speed() - seg.speed).abs() > 1e-6);
            if jumped || stale || (stretching && entry.stretch.is_none()) {
                // 跳了、换了倍数或开关：从这块该放的地方重新开始（拉伸器开着时解码器读在前面）
                entry.stretch = None;
                if let Err(e) = entry.decoder.seek_to(seg.source_start) {
                    log::warn!("音频定位失败：{e}");
                    continue;
                }
                entry.stretch = stretching.then(|| crate::stretch::Stretcher::new(seg.speed));
            }
            let frames = usize::try_from(seg.duration.to_samples(rate))
                .unwrap_or(0)
                .min(mix_frames);
            let out_time = Time::from_samples(i64::try_from(frames).unwrap_or(0), rate);
            if let Some(stretch) = entry.stretch.as_mut() {
                // 变速不变调：喂到够这一块用，再取 frames 帧
                while stretch.available() < frames && !stretch.is_finished() {
                    self.tmp.resize(CHUNK * 2, 0.0);
                    match entry.decoder.read(&mut self.tmp) {
                        Ok(got) if got > 0 => stretch.push(&self.tmp[..got * 2]),
                        _ => stretch.finish(),
                    }
                }
                let out = stretch.pop(frames);
                entry.next = seg.source_start + out_time.scaled(speed);
                mix_into(mix, &out, &seg, pos, rate);
                continue;
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss,
                reason = "一块只有一千来帧"
            )]
            let src_frames = (frames as f64 * speed).ceil() as usize;
            self.tmp.resize(src_frames * 2, 0.0);
            let got = entry.decoder.read(&mut self.tmp).unwrap_or(0);
            entry.next =
                seg.source_start + Time::from_samples(i64::try_from(got).unwrap_or(0), rate);
            #[expect(clippy::float_cmp, reason = "原速是精确的 1.0")]
            if seg.speed == 1.0 {
                mix_into(mix, &self.tmp[..got * 2], &seg, pos, rate);
            } else {
                // 变速且变调：拉伸 / 压缩到时间轴上的长度（和导出的 asetrate 一致）
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::cast_precision_loss,
                    reason = "一块只有一千来帧"
                )]
                let out_frames = ((got as f64 / speed).round() as usize).min(frames);
                let stretched = crate::resample_linear(&self.tmp[..got * 2], out_frames);
                mix_into(mix, &stretched, &seg, pos, rate);
            }
        }
        clamp_mix(mix);
    }
}

/// 倒放时每次往回取多长（帧）：取一段顺着解、再整段倒过来，免得每一小块都让解码器往回跳。
const REVERSE_BLOCK: usize = 8192;

/// 穿梭播放（J / L 的倍速、倒放）时的输入状态。
struct Shuttle {
    rate: f32,
    /// 下一段输入从时间轴哪里接着取：正放是这段的起点，倒放是这段的终点。
    cursor: Time,
    /// 倒放：已经倒过来、还没用掉的样本（交错立体声）。
    reversed: Vec<f32>,
    /// 倍数不是 1 时的变速不变调（和片段变速同一个 WSOLA）。
    stretch: Option<crate::stretch::Stretcher>,
}

impl Mixer {
    /// 按播放倍数出一块（`mix` 长 [`CHUNK`] 帧），返回时钟走了多少（倒放为负）。倍速：
    /// 按倍数取一段再变速不变调压成一块；倒放：往回取一段、倒过来放，倍数不是 1 时同样不变调。
    fn render_at_rate(
        &mut self,
        project: &Project,
        pos: Time,
        play_rate: f32,
        mix: &mut [f32],
    ) -> Time {
        let chunk_time = self.chunk_time;
        if (play_rate - 1.0).abs() < 1e-6 {
            self.shuttle = None;
            self.render(project, pos, mix);
            return chunk_time;
        }
        let mut sh = self
            .shuttle
            .take()
            .filter(|s| (s.rate - play_rate).abs() < 1e-6)
            .unwrap_or_else(|| Shuttle {
                rate: play_rate,
                cursor: pos,
                reversed: Vec::new(),
                stretch: ((play_rate.abs() - 1.0).abs() > 1e-6)
                    .then(|| crate::stretch::Stretcher::new(play_rate.abs())),
            });
        if let Some(mut stretch) = sh.stretch.take() {
            while stretch.available() < CHUNK {
                let input = self.shuttle_input(project, &mut sh, CHUNK);
                stretch.push(&input);
            }
            mix.copy_from_slice(&stretch.pop(CHUNK));
            sh.stretch = Some(stretch);
        } else {
            mix.copy_from_slice(&self.shuttle_input(project, &mut sh, CHUNK));
        }
        self.shuttle = Some(sh);
        chunk_time.scaled(f64::from(play_rate))
    }

    /// 穿梭播放的下一段输入（`frames` 帧，按播放方向排好）。倒到开头之前是静音。
    fn shuttle_input(&mut self, project: &Project, sh: &mut Shuttle, frames: usize) -> Vec<f32> {
        let rate = self.rate;
        let len = |n: usize| Time::from_samples(i64::try_from(n).unwrap_or(0), rate);
        if sh.rate > 0.0 {
            let mut out = vec![0.0; frames * 2];
            self.render(project, sh.cursor, &mut out);
            sh.cursor += len(frames);
            return out;
        }
        while sh.reversed.len() < frames * 2 {
            if sh.cursor <= Time::ZERO {
                sh.reversed.resize(frames * 2, 0.0);
                break;
            }
            let start = (sh.cursor - len(REVERSE_BLOCK)).max(Time::ZERO);
            let n = usize::try_from((sh.cursor - start).to_samples(rate))
                .unwrap_or(0)
                .max(1);
            let mut block = vec![0.0; n * 2];
            self.render(project, start, &mut block);
            sh.reversed
                .extend(block.as_chunks::<2>().0.iter().rev().flatten());
            sh.cursor = start;
        }
        sh.reversed.drain(..frames * 2).collect()
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
        shuttle: None,
    };
    let mut generation = None;
    let mut revision = None;
    let mut pos = Time::ZERO;
    let mut mix = vec![0.0f32; CHUNK * 2];

    while !shared.is_shutdown() {
        let (project, playing, current_gen, start, current_rev, play_rate) = {
            let c = shared.control();
            (
                Arc::clone(&c.project),
                c.playing,
                c.generation,
                c.start,
                c.revision,
                c.rate,
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
            mixer.shuttle = None;
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

        let step = mixer.render_at_rate(&project, pos, play_rate, &mut mix);
        if shared.control().generation != current_gen {
            continue;
        }
        if producer.push_entire_slice(&mix).is_ok() {
            pos += step;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::{Asset, edit};
    use ve_media::testing::{Fixture, fixture};

    /// 一段 440Hz 音调（2 秒）的工程。
    fn tone_project() -> (Project, ClipId) {
        let mut p = Project::new();
        let path = fixture(Fixture::ToneStereo48k);
        let r = ve_media::probe(&path).unwrap();
        let id = p.add_asset(Asset {
            id: AssetId(0),
            name: "tone".into(),
            path,
            duration: r.duration,
            video: r.video,
            audio: r.audio,
        });
        let clip = edit::append_asset(&mut p, id).unwrap();
        (p, clip)
    }

    fn mixer() -> Mixer {
        let rate = 48_000;
        Mixer {
            rate,
            chunk_time: Time::from_samples(i64::try_from(CHUNK).unwrap(), rate),
            clips: HashMap::new(),
            failed: HashMap::new(),
            tmp: Vec::new(),
            shuttle: None,
        }
    }

    /// 按播放倍数从 `from` 起连着出 `blocks` 块（左声道）。
    fn shuttle(p: &Project, from: Time, play_rate: f32, blocks: usize) -> Vec<f32> {
        let mut m = mixer();
        let mut mix = vec![0.0; CHUNK * 2];
        let mut pos = from;
        let mut out = Vec::new();
        for _ in 0..blocks {
            pos += m.render_at_rate(p, pos, play_rate, &mut mix);
            out.extend(mix.iter().step_by(2));
        }
        out
    }

    /// 数过零点估频率。
    fn hz(left: &[f32]) -> f64 {
        let crossings = left
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        #[expect(clippy::cast_precision_loss, reason = "测试数据")]
        let hz = crossings as f64 / (left.len() as f64 / 48_000.0);
        hz
    }

    /// 2 倍速的 440Hz 音调，混音器连着出 0.6 秒，数频率。
    fn rendered_hz(keep_pitch: bool) -> f64 {
        let (mut p, clip) = tone_project();
        edit::set_speed(&mut p, clip, 2.0).unwrap();
        edit::set_keep_pitch(&mut p, clip, keep_pitch).unwrap();
        let mut mixer = mixer();
        let mut out = Vec::new();
        let mut mix = vec![0.0; CHUNK * 2];
        let mut pos = Time::ZERO;
        while pos < Time::from_millis(600) {
            mixer.render(&p, pos, &mut mix);
            out.extend_from_slice(&mix);
            pos += mixer.chunk_time;
        }
        // 去掉开头（拉伸器起步那一点淡入）
        let left: Vec<f32> = out.iter().step_by(2).skip(4800).copied().collect();
        hz(&left)
    }

    #[test]
    fn double_speed_keeps_or_shifts_the_pitch() {
        let kept = rendered_hz(true);
        assert!((kept - 440.0).abs() < 15.0, "保持音调：{kept} Hz");
        let shifted = rendered_hz(false);
        assert!((shifted - 880.0).abs() < 25.0, "变调：{shifted} Hz");
    }

    #[test]
    fn shuttle_keeps_the_pitch_forward_and_backward() {
        let (p, _) = tone_project();
        for play_rate in [2.0, 4.0, -1.0, -2.0] {
            let from = if play_rate > 0.0 {
                Time::ZERO
            } else {
                Time::from_millis(1990)
            };
            // 出 0.2 秒，去掉开头拉伸器起步的那点
            let left = shuttle(&p, from, play_rate, 10);
            let f = hz(&left[2400..]);
            assert!((f - 440.0).abs() < 20.0, "{play_rate} 倍：{f} Hz");
        }
    }

    #[test]
    fn reverse_shuttle_plays_the_sound_backwards() {
        let (mut p, clip) = tone_project();
        // 整段淡入：越往后越响，倒着放就越来越轻
        edit::set_fades(&mut p, clip, Time::from_secs(2), Time::ZERO).unwrap();
        let left = shuttle(&p, Time::from_millis(1900), -1.0, 60);
        let rms: Vec<f32> = left
            .chunks(4800)
            .map(|w| (w.iter().map(|s| s * s).sum::<f32>() / 4800.0).sqrt())
            .collect();
        assert!(rms[0] > 0.2, "倒放有声音：{rms:?}");
        assert!(rms.windows(2).all(|w| w[1] < w[0]), "越来越轻：{rms:?}");
        // 倒到开头之后是静音（时钟停在 0）
        let tail = shuttle(&p, Time::from_millis(50), -1.0, 10);
        assert!(tail[4800..].iter().all(|s| s.abs() < 1e-6));
    }
}
