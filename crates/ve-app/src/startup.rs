//! 启动参数处理：打开工程、导入素材、播放性能测试。

use crate::app::Args;
use crate::workspace::Workspace;
use gpui_kit::{Context, Window};
use std::io::Write as _;
use std::time::{Duration, Instant};

/// 性能测试结果文件（当前目录）。
pub(crate) const BENCH_FILE: &str = "ve-bench.log";

impl Workspace {
    /// 按命令行参数打开工程 / 导入素材；带 `bench_seconds` 时播放并记录帧率后退出。
    pub fn startup(&mut self, args: &Args, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(project) = &args.project {
            self.open_path(project, window, cx);
        }
        if !args.media.is_empty() {
            self.import_with(args.media.clone(), true, window, cx);
        }
        let Some(seconds) = args.bench_seconds else {
            return;
        };
        let layers = args.bench_layers.max(1);
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            // 等素材导入完、时间轴有内容
            for _ in 0..200 {
                let ready = this.update_in(cx, |this, _, _| {
                    this.doc.project.timeline.duration() > ve_core::Time::ZERO
                });
                if ready.unwrap_or(true) {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
            }
            let start = this.update_in(cx, |this, _, cx| {
                if layers > 1 {
                    this.stack_bench_layers(layers);
                }
                this.engine.play();
                cx.notify();
                (this.preview.frames_shown, this.preview.long_gaps)
            });
            let Ok((start, gaps_before)) = start else { return };
            let began = Instant::now();
            cx.background_executor()
                .timer(Duration::from_secs(seconds))
                .await;
            let _ignored = this.update_in(cx, |this, _, cx| {
                let elapsed = began.elapsed().as_secs_f64();
                #[expect(clippy::cast_precision_loss, reason = "帧数很小")]
                let fps = (this.preview.frames_shown - start) as f64 / elapsed;
                let line = format!(
                    "layers={layers} frames={} seconds={elapsed:.2} fps={fps:.2} gaps_over_25ms={} decoded={} project_fps={:.3}\n",
                    this.preview.frames_shown - start,
                    this.preview.long_gaps - gaps_before,
                    this.engine.stats().decoded_frames,
                    this.doc.project.settings.frame_rate.as_f64()
                );
                log::info!("bench: {line}");
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(BENCH_FILE)
                {
                    drop(f.write_all(line.as_bytes()));
                }
                cx.quit();
            });
        }));
    }

    /// 把第一个素材铺成 `layers` 层（性能测试用），长度和原时间轴一样。
    fn stack_bench_layers(&mut self, layers: usize) {
        let Some(asset) = self.doc.project.assets.keys().next().copied() else {
            return;
        };
        let length = self.doc.project.timeline.duration();
        let result = self.doc.edit(|p| {
            let clips: Vec<_> = p
                .timeline
                .tracks
                .iter()
                .filter(|t| t.kind == ve_core::TrackKind::Video)
                .flat_map(|t| t.clips.iter().map(|c| c.id))
                .collect();
            ve_core::edit::delete_clips(p, &clips)?;
            ve_core::bench::stack_layers(p, asset, layers, length)
        });
        if let Err(e) = result {
            log::error!("铺 {layers} 层失败：{e}");
        }
        self.sync_engine();
    }
}
