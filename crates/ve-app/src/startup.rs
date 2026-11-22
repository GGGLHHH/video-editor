//! 启动参数处理：打开工程、导入素材、播放性能测试。

use crate::app::Args;
use crate::workspace::Workspace;
use gpui_kit::{Context, Window};
use std::io::Write as _;
use std::time::{Duration, Instant};

/// 性能测试结果文件（当前目录）。
pub(crate) const BENCH_FILE: &str = "ve-bench.log";

impl Workspace {
    /// 截图核对用（`--show`）：选中第一段视频，属性面板切到 `name`：picture、mask、key、adjust、
    /// hsl、curves、wheels、animation、speed、audio；text 选中第一条字幕看「文本」；export 弹导出框。
    /// 认不出的名字什么都不做。
    pub fn show_panel(&mut self, name: &str, window: &mut Window, cx: &mut Context<'_, Self>) {
        use crate::inspector::InspectorTab as Tab;
        use crate::inspector_grade::AdjustSeg;
        use crate::inspector_look::PictureSeg;
        // 逗号隔开能一次开几个（例如 `adjust,bin-effect,guides`）
        if name.contains(',') {
            for part in name.split(',') {
                self.show_panel(part.trim(), window, cx);
            }
            return;
        }
        let bin = match name {
            "bin-effect" => Some(crate::media_bin::BinTab::Effect),
            "bin-sticker" => Some(crate::media_bin::BinTab::Sticker),
            "bin-adjust" => Some(crate::media_bin::BinTab::Adjust),
            "bin-filter" => Some(crate::media_bin::BinTab::Filter),
            _ => None,
        };
        if let Some(tab) = bin {
            self.bin_tab = tab;
            cx.notify();
            return;
        }
        if name == "guides" {
            self.toggle_guides(&crate::actions::ToggleGuides, window, cx);
            return;
        }
        if name == "crop" {
            self.show_panel("picture", window, cx);
            cx.defer_in(window, |this, window, cx| {
                this.open_crop(&crate::actions::OpenCrop, window, cx);
            });
            return;
        }
        if name == "export" {
            // 启动时主窗口的组件库根视图还没搭好，弹框要等这一轮更新完（同决策 100）
            cx.defer_in(window, |this, window, cx| {
                this.export(&crate::actions::Export, window, cx);
            });
            return;
        }
        if name == "text" {
            let first = self
                .doc
                .project
                .timeline
                .tracks
                .iter()
                .filter(|t| t.kind == ve_core::TrackKind::Subtitle)
                .flat_map(|t| t.clips.iter())
                .map(|c| c.id)
                .next();
            if let Some(id) = first {
                self.doc.select(id, false);
            }
            self.inspector.tab = Tab::Text;
            self.after_change(window, cx);
            return;
        }
        let (tab, picture, adjust) = match name {
            "picture" => (Tab::Picture, Some(PictureSeg::Basic), None),
            "mask" => (Tab::Picture, Some(PictureSeg::Mask), None),
            "key" => (Tab::Picture, Some(PictureSeg::Key), None),
            "adjust" => (Tab::Adjust, None, Some(AdjustSeg::Basic)),
            "hsl" => (Tab::Adjust, None, Some(AdjustSeg::Hsl)),
            "curves" => (Tab::Adjust, None, Some(AdjustSeg::Curves)),
            "wheels" => (Tab::Adjust, None, Some(AdjustSeg::Wheels)),
            "animation" => (Tab::Animation, None, None),
            "speed" => (Tab::Speed, None, None),
            "audio" => (Tab::Audio, None, None),
            _ => return,
        };
        let first = self
            .doc
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == ve_core::TrackKind::Video)
            .flat_map(|t| t.clips.iter())
            .find(|c| c.asset().is_some() || c.compound().is_some())
            .map(|c| c.id);
        if let Some(id) = first {
            self.doc.select(id, false);
        }
        self.inspector.tab = tab;
        if let Some(seg) = picture {
            self.inspector.look.seg = seg;
        }
        if let Some(seg) = adjust {
            self.inspector.grade.seg = seg;
        }
        self.after_change(window, cx);
    }

    /// 按命令行参数打开工程 / 导入素材；带 `bench_seconds` 时播放并记录帧率后退出。
    pub fn startup(&mut self, args: &Args, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(project) = &args.project {
            self.open_path(project, window, cx);
        }
        if !args.media.is_empty() {
            self.import_with(args.media.clone(), true, window, cx);
        }
        if let Some(name) = &args.show {
            self.show_panel(name, window, cx);
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
