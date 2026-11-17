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
                this.engine.play();
                cx.notify();
                this.preview.frames_shown
            });
            let Ok(start) = start else { return };
            let began = Instant::now();
            cx.background_executor()
                .timer(Duration::from_secs(seconds))
                .await;
            let _ignored = this.update_in(cx, |this, _, cx| {
                let elapsed = began.elapsed().as_secs_f64();
                #[expect(clippy::cast_precision_loss, reason = "帧数很小")]
                let fps = (this.preview.frames_shown - start) as f64 / elapsed;
                let line = format!(
                    "frames={} seconds={elapsed:.2} fps={fps:.2} decoded={} project_fps={:.3}\n",
                    this.preview.frames_shown - start,
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
}
