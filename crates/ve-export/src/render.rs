//! 导出画面：逐帧解码各层 → 显卡合成 NV12 → 写进 ffmpeg 的标准输入。
//!
//! 三段流水线各占一个线程：解码（多层时各层并行）、合成（调用方线程）、写管道。
//! 第 k 帧的时间是 `time_of_frame(k)`，取帧规则和预览一致：素材里时间不晚于目标的最后一帧。

use crate::{ExportError, ExportSettings};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use ve_core::compose::resolve_frame;
use ve_core::{ClipId, FrameRate, Project, Time, TrackId, Transform, layer_geometry};
use ve_media::{MediaError, ScaleTarget, VideoDecoder, VideoFrame};
use ve_render::{Compositor, FramePixels, Layer, MappedNv12};

/// 显卡上同时在途的帧数：显卡忙时单帧要等很久，保持几帧在途才能跑满吞吐。
const IN_FLIGHT: usize = 3;

/// 时间轴要导出多少帧：时间早于时长的帧都算。
#[must_use]
pub fn frame_count(project: &Project, rate: FrameRate) -> u64 {
    let duration = project.timeline.duration();
    if duration <= Time::ZERO {
        return 0;
    }
    u64::try_from(rate.frame_of(duration - Time(1)) + 1).unwrap_or(0)
}

/// 输出画面尺寸（NV12 / yuv420p 要求偶数）。
#[must_use]
pub const fn output_size(settings: &ExportSettings) -> (u32, u32) {
    (even(settings.width), even(settings.height))
}

const fn even(v: u32) -> u32 {
    let e = v & !1;
    if e < 2 { 2 } else { e }
}

/// 一层在某一帧要用的画面。
struct LayerFrame {
    frame: Arc<VideoFrame>,
    source: (u32, u32),
    transform: Transform,
    color: ve_core::ColorMatrix,
    look: ve_core::Look,
    grades: ve_core::GradeChain,
}

/// 一层的画面属性、调色、外观和调色链。
type Appearance = (
    Transform,
    ve_core::ColorMatrix,
    ve_core::Look,
    ve_core::GradeChain,
);

/// 一帧要合成的东西：各层画面（从下往上）和底色的调色。
/// 一帧要合成的东西：各层画面，底色调色，画面特效（`after` 已按画出来的层换算）。
type FrameLayers = (
    Vec<LayerFrame>,
    (ve_core::ColorMatrix, ve_core::GradeChain),
    Vec<ve_core::EffectPass>,
);

/// 按顺序读一个片段的画面：第一次定位，之后一直往后读。
struct Stepper {
    decoder: VideoDecoder,
    current: Option<Arc<VideoFrame>>,
    peek: Option<VideoFrame>,
    ended: bool,
    still: bool,
}

impl Stepper {
    /// 素材时间 `t` 上的画面：时间不晚于 `t` 的最后一帧；读完了就一直用最后一帧。
    fn frame_at(&mut self, t: Time) -> Result<Arc<VideoFrame>, MediaError> {
        let current = match self.current.take() {
            None => Arc::new(self.decoder.seek_to(t)?),
            Some(c) if self.still => c,
            Some(mut c) => {
                loop {
                    if self.peek.is_none() && !self.ended {
                        self.peek = self.decoder.next_frame()?;
                        self.ended = self.peek.is_none();
                    }
                    match self.peek.take() {
                        Some(p) if p.pts <= t => c = Arc::new(p),
                        other => {
                            self.peek = other;
                            break;
                        }
                    }
                }
                c
            }
        };
        self.current = Some(Arc::clone(&current));
        Ok(current)
    }

    /// [`Self::frame_at`]；`blend` 时再和下一帧混合（帧融合）。
    fn frame_or_blend(&mut self, t: Time, blend: bool) -> Result<Arc<VideoFrame>, MediaError> {
        let f = self.frame_at(t)?;
        if blend { self.blended(t, f) } else { Ok(f) }
    }

    /// 帧融合：[`Self::frame_at`] 取到的那一帧和它的下一帧按时间混合（和预览同一个函数）。
    fn blended(
        &mut self,
        t: Time,
        current: Arc<VideoFrame>,
    ) -> Result<Arc<VideoFrame>, MediaError> {
        if self.still {
            return Ok(current);
        }
        if self.peek.is_none() && !self.ended {
            self.peek = self.decoder.next_frame()?;
            self.ended = self.peek.is_none();
        }
        Ok(self
            .peek
            .as_ref()
            .and_then(|b| ve_media::blend_frames(&current, b, t))
            .map_or(current, Arc::new))
    }
}

type Key = (TrackId, ClipId);

/// 导出要的解码尺寸：画面在导出画布上显示的大小（放进画布的大小 × scale），夹在 2 像素到显卡纹理上限之间。
/// 缩小的画中画由解码器的缩放（带缩小滤波）缩到位，显卡只做接近 1:1 的采样，不会出锯齿；
/// 解码器本身不会放大到超过素材。
fn decode_target(canvas: (u32, u32), scale: f32, max_texture: u32) -> ScaleTarget {
    let factor = f64::from(scale.max(0.0));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "画布 × 至多 10 倍，远小于 u32 上限"
    )]
    let shown = |v: u32| ((f64::from(v) * factor).ceil() as u32).clamp(2, max_texture.max(2));
    ScaleTarget {
        max_width: shown(canvas.0),
        max_height: shown(canvas.1),
    }
}

/// 解码线程：每帧把各层画面（从下往上）送出去；出错时送错误并停下。
fn decode_frames(
    project: &Project,
    rate: FrameRate,
    canvas: (u32, u32),
    max_texture: u32,
    total: u64,
    tx: &SyncSender<Result<FrameLayers, ExportError>>,
    cancel: &AtomicBool,
) {
    let mut steppers: HashMap<Key, Stepper> = HashMap::new();
    for k in 0..total {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let t = rate.time_of_frame(i64::try_from(k).unwrap_or(i64::MAX));
        let result = layer_frames(project, canvas, max_texture, t, &mut steppers);
        let failed = result.is_err();
        if tx.send(result).is_err() || failed {
            return;
        }
    }
}

/// 时间 `t` 上各层的画面；片段换了的解码器随手关掉。
fn layer_frames(
    project: &Project,
    canvas: (u32, u32),
    max_texture: u32,
    t: Time,
    steppers: &mut HashMap<Key, Stepper>,
) -> Result<FrameLayers, ExportError> {
    let ve_core::compose::Composition {
        layers,
        background,
        background_grades,
        effects,
    } = resolve_frame(project, t);
    // 素材没有画面的层不画：特效压在哪几层上要按画出来的算
    let kept: Vec<bool> = layers
        .iter()
        .map(|l| {
            project
                .assets
                .get(&l.asset)
                .is_some_and(|a| a.video.is_some())
        })
        .collect();
    let effects = ve_core::EffectPass::remap(&effects, &kept);
    steppers.retain(|key, _| layers.iter().any(|l| (l.track, l.clip) == *key));
    let mut jobs = Vec::with_capacity(layers.len());
    for l in &layers {
        let Some(asset) = project.assets.get(&l.asset) else {
            continue;
        };
        let Some(info) = asset.video else { continue };
        let key = (l.track, l.clip);
        let stepper = match steppers.remove(&key) {
            Some(s) => s,
            None => Stepper {
                decoder: VideoDecoder::open(
                    &asset.path,
                    decode_target(canvas, l.max_scale, max_texture),
                )
                .map_err(|e| render_error(&asset.name, &e))?,
                current: None,
                peek: None,
                ended: false,
                still: info.still,
            },
        };
        let source = if info.width > 0 && info.height > 0 {
            (info.width, info.height)
        } else {
            canvas
        };
        jobs.push((
            key,
            stepper,
            l.source_time,
            source,
            (l.transform, l.color, l.look, l.grades.clone()),
            (asset.name.clone(), l.frame_blend),
        ));
    }
    let step = |(
        key,
        mut stepper,
        time,
        source,
        (transform, color, look, grades),
        (name, blend),
    ): (Key, Stepper, Time, (u32, u32), Appearance, (String, bool))| {
        let r = stepper
            .frame_or_blend(time, blend)
            .map_err(|e| render_error(&name, &e));
        (
            key,
            stepper,
            r.map(|frame| LayerFrame {
                frame,
                source,
                transform,
                color,
                look,
                grades,
            }),
        )
    };
    let results: Vec<_> = if jobs.len() <= 1 {
        jobs.into_iter().map(step).collect()
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = jobs
                .into_iter()
                .map(|job| scope.spawn(move || step(job)))
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().ok())
                .collect::<Vec<_>>()
        })
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| ExportError::Render("解码线程出错".into()))?
    };
    let mut frames = Vec::with_capacity(results.len());
    for (key, stepper, r) in results {
        steppers.insert(key, stepper);
        frames.push(r?);
    }
    Ok((frames, (background, background_grades), effects))
}

fn render_error(name: &str, e: &MediaError) -> ExportError {
    ExportError::Render(format!("解码 {name} 失败：{e}"))
}

/// 写线程：把读回的 NV12 帧直接从映射内存写进 ffmpeg（不先拷进 Vec），写完的缓冲还回去复用。
/// 写失败（ffmpeg 已退出）就停下。
fn write_frames(
    rx: &Receiver<MappedNv12>,
    done: &SyncSender<MappedNv12>,
    sink: impl Write,
) -> bool {
    let mut sink = std::io::BufWriter::with_capacity(1 << 20, sink);
    for frame in rx {
        if frame.write_to(&mut sink).is_err() {
            return false;
        }
        // 合成线程已经结束时还不回去也没关系，缓冲随之释放
        drop(done.try_send(frame));
    }
    sink.flush().is_ok()
}

/// 渲染全部画面并写进 `sink`（ffmpeg 的标准输入）。
/// 写失败返回 `Ok(false)`：说明 ffmpeg 提前退出了，原因由调用方从 ffmpeg 的报错里取。
///
/// # Errors
/// 取消、解码失败、显卡合成失败。
pub fn render_video(
    project: &Project,
    settings: &ExportSettings,
    sink: impl Write + Send,
    on_frame: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<bool, ExportError> {
    let size = output_size(settings);
    let rate = settings.frame_rate;
    let total = frame_count(project, rate);
    let mut compositor =
        Compositor::new(&settings.render).map_err(|e| ExportError::Render(e.to_string()))?;
    log::info!("导出合成用显卡：{}", compositor.adapter_name());
    let (layers_tx, layers_rx) = sync_channel(3);
    let (frames_tx, frames_rx) = sync_channel::<MappedNv12>(4);
    // 写完的缓冲回收：容量足够装下所有在途的帧，写线程永远不会卡在这里
    let (done_tx, done_rx) = sync_channel::<MappedNv12>(IN_FLIGHT + 8);
    std::thread::scope(|scope| {
        let max_texture = compositor.max_texture_size();
        scope.spawn(move || {
            decode_frames(project, rate, size, max_texture, total, &layers_tx, cancel);
        });
        let writer = scope.spawn(move || write_frames(&frames_rx, &done_tx, sink));
        let mut outcome = Ok(true);
        let mut written = 0u64;
        for k in 0..total {
            if cancel.load(Ordering::Acquire) {
                outcome = Err(ExportError::Cancelled);
                break;
            }
            let Ok(layers) = layers_rx.recv() else {
                // 解码线程提前退出：取消时是正常的，否则是解码线程出了意外
                outcome = Err(if cancel.load(Ordering::Acquire) {
                    ExportError::Cancelled
                } else {
                    ExportError::Render("解码线程意外退出".into())
                });
                break;
            };
            let (layers, background, effects) = match layers {
                Ok(l) => l,
                Err(e) => {
                    outcome = Err(e);
                    break;
                }
            };
            while let Ok(f) = done_rx.try_recv() {
                compositor.recycle(f);
            }
            let gpu: Vec<Layer<'_>> = layers
                .iter()
                .map(|l| Layer {
                    pixels: FramePixels::Bgra {
                        width: l.frame.width,
                        height: l.frame.height,
                        data: &l.frame.data,
                    },
                    geometry: layer_geometry(size, l.source, &l.transform),
                    opacity: l.transform.opacity,
                    color: l.color,
                    look: l.look,
                    grades: &l.grades,
                })
                .collect();
            compositor.set_background_chain(&background.0, &background.1);
            compositor.set_effects(&effects);
            if let Err(e) = compositor.submit_nv12(size, &gpu) {
                outcome = Err(ExportError::Render(e.to_string()));
                break;
            }
            if compositor.in_flight() < IN_FLIGHT && k + 1 < total {
                continue;
            }
            // 取回最早的帧交给写线程；最后一帧提交后把在途的全部取完
            while compositor.in_flight() > 0 {
                let frame = match compositor.receive_nv12_mapped() {
                    Ok(f) => f,
                    Err(e) => {
                        outcome = Err(ExportError::Render(e.to_string()));
                        break;
                    }
                };
                if frames_tx.send(frame).is_err() {
                    outcome = Ok(false);
                    break;
                }
                written += 1;
                on_frame(written, total);
                if k + 1 < total {
                    break;
                }
            }
            if !matches!(outcome, Ok(true)) {
                break;
            }
        }
        drop(frames_tx);
        drop(layers_rx);
        let wrote_all = writer.join().unwrap_or(false);
        outcome.map(|ok| ok && wrote_all)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::testing::sample_project;

    #[test]
    fn frame_count_covers_partial_last_frame() {
        let (p, _) = sample_project();
        // 时间轴 8 秒
        assert_eq!(frame_count(&p, FrameRate::new(30, 1)), 240);
        assert_eq!(frame_count(&Project::new(), FrameRate::new(30, 1)), 0);
        // 不整除的时长：最后不足一帧也要出一帧
        let mut r = Project::new();
        r.settings = p.settings;
        let len = Time::from_millis(1010);
        let mut tl = p.timeline;
        for t in &mut tl.tracks {
            t.clips.retain(|c| c.start == Time::ZERO);
            for c in &mut t.clips {
                let mut cc = (**c).clone();
                cc.duration = len;
                cc.fade_in = Time::ZERO;
                cc.fade_out = Time::ZERO;
                *c = Arc::new(cc);
            }
        }
        r.timeline = tl;
        assert_eq!(frame_count(&r, FrameRate::new(30, 1)), 31);
    }

    #[test]
    fn decode_target_follows_shown_size() {
        // 缩小的层按显示尺寸解码：交给解码器的缩放（有正经的缩小滤波）而不是显卡一次采样缩 6 倍
        let small = decode_target((1280, 720), 0.5, 16384);
        assert_eq!((small.max_width, small.max_height), (640, 360));
        let tiny = decode_target((1280, 720), 0.001, 16384);
        assert_eq!((tiny.max_width, tiny.max_height), (2, 2));
        let big = decode_target((1280, 720), 2.5, 16384);
        assert_eq!((big.max_width, big.max_height), (3200, 1800));
        // 4K 导出里放大 10 倍：夹到显卡纹理上限
        let huge = decode_target((3840, 2160), 10.0, 16384);
        assert_eq!((huge.max_width, huge.max_height), (16384, 16384));
    }

    #[test]
    fn output_size_is_even() {
        let (p, _) = sample_project();
        let mut s = crate::default_settings(&p, "x.mp4".into());
        s.width = 853;
        s.height = 1;
        assert_eq!(output_size(&s), (852, 2));
    }

    #[test]
    fn undecodable_media_is_a_render_error() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.mp4");
        std::fs::write(&bad, b"not a video").unwrap();
        let (mut p, ids) = sample_project();
        p.assets.get_mut(&ids.video_asset).unwrap().path = bad;
        let mut s = crate::default_settings(&p, dir.path().join("out.mp4"));
        s.render = ve_render::RenderConfig { software: true };
        s.width = 64;
        s.height = 36;
        let err = render_video(
            &p,
            &s,
            std::io::sink(),
            &mut |_, _| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(err, ExportError::Render(_)), "{err:?}");
        assert!(err.to_string().contains("clip.mp4"), "{err}");
    }
}
