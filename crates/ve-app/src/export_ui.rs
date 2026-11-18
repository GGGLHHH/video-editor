//! 导出界面（剪映式）：选输出文件 → 导出对话框（分辨率 / 帧率 / 码率 / 编码，高级里选编码器，
//! 底部实时显示时长和估计大小）→ 后台导出，顶栏显示进度和取消。

use crate::actions as act;
use crate::chrome;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, px,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use ve_core::{FrameRate, ProjectSettings, Time, format_timecode};
use ve_export::{Codec, Encoder, ExportSettings, estimated_bytes, recommended_kbps};

/// 进行中的导出。
#[derive(Debug)]
pub struct ExportProgress {
    /// 0–1。
    pub progress: f32,
    /// 取消标志。
    pub cancel: Arc<AtomicBool>,
    /// 输出文件。
    pub path: PathBuf,
}

/// 输出分辨率选项（按短边，和剪映一样：竖屏的 720P 是 720×1280）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// 跟工程一样。
    Project,
    /// 短边 720。
    P720,
    /// 短边 1080。
    P1080,
    /// 短边 1440（2K）。
    P1440,
    /// 短边 2160（4K）。
    P2160,
}

/// 按目标短边等比缩放，宽高取偶数。
#[must_use]
pub fn scaled_size(width: u32, height: u32, res: Resolution) -> (u32, u32) {
    let target: u32 = match res {
        Resolution::Project => return (width, height),
        Resolution::P720 => 720,
        Resolution::P1080 => 1080,
        Resolution::P1440 => 1440,
        Resolution::P2160 => 2160,
    };
    let short = u64::from(width.min(height).max(1));
    let scale = |v: u32| {
        let s = u32::try_from(u64::from(v) * u64::from(target) / short).unwrap_or(u32::MAX);
        (s & !1).max(2)
    };
    (scale(width), scale(height))
}

/// 码率档位（剪映：更低 / 推荐 / 更高 / 自定义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bitrate {
    /// 推荐的一半。
    Lower,
    /// 按分辨率和帧率推荐。
    Recommended,
    /// 推荐的两倍。
    Higher,
    /// 自己填。
    Custom,
}

/// 导出对话框里选的东西（纯数据，方便测试）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportChoices {
    /// 分辨率。
    pub resolution: Resolution,
    /// 帧率；`None` 跟工程一样。
    pub frame_rate: Option<FrameRate>,
    /// 码率档位。
    pub bitrate: Bitrate,
    /// 自定义码率（kbps）。
    pub custom_kbps: u32,
    /// 编码格式。
    pub codec: Codec,
    /// 编码器。
    pub encoder: Encoder,
}

impl Default for ExportChoices {
    fn default() -> Self {
        Self {
            resolution: Resolution::Project,
            frame_rate: None,
            bitrate: Bitrate::Recommended,
            custom_kbps: 12_000,
            codec: Codec::H264,
            encoder: Encoder::Nvenc,
        }
    }
}

impl ExportChoices {
    /// 按工程设置换算成导出设置。
    #[must_use]
    pub fn settings(&self, path: PathBuf, project: &ProjectSettings) -> ExportSettings {
        let (width, height) = scaled_size(project.width, project.height, self.resolution);
        let frame_rate = self.frame_rate.unwrap_or(project.frame_rate);
        let recommended = recommended_kbps(width, height, frame_rate, self.codec);
        let video_bitrate_kbps = match self.bitrate {
            Bitrate::Lower => (recommended / 2).max(500),
            Bitrate::Recommended => recommended,
            Bitrate::Higher => recommended.saturating_mul(2),
            Bitrate::Custom => self.custom_kbps.clamp(100, 500_000),
        };
        ExportSettings {
            path,
            width,
            height,
            frame_rate,
            video_bitrate_kbps,
            encoder: self.encoder,
            codec: self.codec,
            render: ve_export::RenderConfig::default(),
        }
    }
}

/// 估计大小的写法：「约 12.3 MB」。
#[must_use]
pub fn size_text(bytes: u64) -> String {
    #[expect(clippy::cast_precision_loss, reason = "只是给人看的估计值")]
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("约 {:.2} GB", mb / 1000.0)
    } else {
        format!("约 {mb:.1} MB")
    }
}

/// 导出选项表单。
#[derive(Debug)]
pub struct ExportForm {
    path: PathBuf,
    project: ProjectSettings,
    duration: Time,
    choices: ExportChoices,
    custom: Entity<InputState>,
    advanced: bool,
}

impl ExportForm {
    fn new(
        path: PathBuf,
        project: ProjectSettings,
        duration: Time,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let custom = cx.new(|cx| InputState::new(window, cx).default_value("12000"));
        Self {
            path,
            project,
            duration,
            choices: ExportChoices::default(),
            custom,
            advanced: false,
        }
    }

    fn settings(&self, cx: &gpui_kit::App) -> ExportSettings {
        let mut c = self.choices;
        if let Ok(v) = self.custom.read(cx).value().trim().parse() {
            c.custom_kbps = v;
        }
        c.settings(self.path.clone(), &self.project)
    }

    /// 一个选项小块：点了就改 `choices`。
    fn pick(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        on: bool,
        change: impl Fn(&mut ExportChoices) + 'static,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        let id: SharedString = id.into();
        chrome::chip(id, label.into(), on, cx)
            .on_click(cx.listener(move |f, _, _, cx| {
                change(&mut f.choices);
                cx.notify();
            }))
            .into_any_element()
    }
}

/// 一行：左边名字，右边一排选项。
fn row(label: &'static str, items: Vec<AnyElement>, cx: &gpui_kit::App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_start()
        .child(
            div()
                .w(px(72.))
                .flex_none()
                .pt_1()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(h_flex().flex_1().flex_wrap().gap_1().children(items))
}

impl ExportForm {
    /// 各行的选项：（分辨率，帧率，码率，编码，编码器）。
    #[expect(clippy::type_complexity, reason = "五组选项一起算，拆开反而难读")]
    fn option_items(
        &self,
        cx: &Context<'_, Self>,
    ) -> (
        Vec<AnyElement>,
        Vec<AnyElement>,
        Vec<AnyElement>,
        Vec<AnyElement>,
        Vec<AnyElement>,
    ) {
        let c = self.choices;
        let res_items = [
            ("res-project", "原始", Resolution::Project),
            ("res-720", "720P", Resolution::P720),
            ("res-1080", "1080P", Resolution::P1080),
            ("res-1440", "2K", Resolution::P1440),
            ("res-2160", "4K", Resolution::P2160),
        ]
        .map(|(id, label, r)| {
            Self::pick(
                id,
                label,
                c.resolution == r,
                move |ch| ch.resolution = r,
                cx,
            )
        })
        .into();
        let fps_items = std::iter::once(Self::pick(
            "fps-project",
            "同工程",
            c.frame_rate.is_none(),
            |ch| ch.frame_rate = None,
            cx,
        ))
        .chain([24, 25, 30, 50, 60].map(|n| {
            let r = FrameRate::new(n, 1);
            Self::pick(
                format!("fps-{n}"),
                n.to_string(),
                c.frame_rate == Some(r),
                move |ch| {
                    ch.frame_rate = Some(r);
                },
                cx,
            )
        }))
        .collect();
        let br_items = [
            ("br-low", "更低", Bitrate::Lower),
            ("br-rec", "推荐", Bitrate::Recommended),
            ("br-high", "更高", Bitrate::Higher),
            ("br-custom", "自定义", Bitrate::Custom),
        ]
        .map(|(id, label, b)| Self::pick(id, label, c.bitrate == b, move |ch| ch.bitrate = b, cx))
        .into();
        let codec_items = [
            ("codec-h264", "H.264（最通用）", Codec::H264),
            ("codec-hevc", "HEVC（更省空间）", Codec::Hevc),
        ]
        .map(|(id, label, k)| Self::pick(id, label, c.codec == k, move |ch| ch.codec = k, cx))
        .into();
        let enc_items = [
            ("enc-nvenc", "显卡（NVENC）", Encoder::Nvenc),
            ("enc-x264", "CPU", Encoder::X264),
        ]
        .map(|(id, label, e)| Self::pick(id, label, c.encoder == e, move |ch| ch.encoder = e, cx))
        .into();
        (res_items, fps_items, br_items, codec_items, enc_items)
    }

    /// 底部：时长和估计大小。
    fn render_estimate(
        &self,
        preview: &ExportSettings,
        cx: &Context<'_, Self>,
    ) -> impl IntoElement {
        h_flex()
            .id("export-estimate")
            .test_support()
            .pt_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .gap_2()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(format!(
                "时长 {}",
                format_timecode(self.duration, preview.frame_rate)
            )))
            .child("｜")
            .child(SharedString::from(format!(
                "大小 {}（估计，码率 {} kbps）",
                size_text(estimated_bytes(preview.video_bitrate_kbps, self.duration)),
                preview.video_bitrate_kbps
            )))
    }
}

impl Render for ExportForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let c = self.choices;
        let preview = self.settings(cx);
        let (res_items, fps_items, br_items, codec_items, enc_items) = self.option_items(cx);
        let muted = cx.theme().muted_foreground;
        let file_name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(div().text_sm().child(SharedString::from(file_name)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .truncate()
                            .child(SharedString::from(self.path.display().to_string())),
                    ),
            )
            .child(row("分辨率", res_items, cx))
            .child(
                div()
                    .pl(px(80.))
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(format!(
                        "输出 {}×{}（按工程比例换算）",
                        preview.width, preview.height
                    ))),
            )
            .child(row("帧率", fps_items, cx))
            .child(row("码率", br_items, cx))
            .children((c.bitrate == Bitrate::Custom).then(|| {
                h_flex()
                    .pl(px(80.))
                    .gap_2()
                    .child(div().w(px(120.)).child(Input::new(&self.custom)))
                    .child(div().text_sm().text_color(muted).child("kbps"))
            }))
            .child(row("视频编码", codec_items, cx))
            .child(
                div()
                    .id("export-advanced")
                    .test_support()
                    .text_xs()
                    .text_color(muted)
                    .cursor_pointer()
                    .child(if self.advanced {
                        "高级 ▴"
                    } else {
                        "高级 ▾"
                    })
                    .on_click(cx.listener(|f, _, _, cx| {
                        f.advanced = !f.advanced;
                        cx.notify();
                    })),
            )
            .children(self.advanced.then(|| {
                v_flex()
                    .gap_2()
                    .child(row("编码器", enc_items, cx))
                    .child(row(
                        "音频",
                        vec![
                            div()
                                .pt_1()
                                .text_sm()
                                .child("AAC 192 kbps")
                                .into_any_element(),
                        ],
                        cx,
                    ))
            }))
            .child(self.render_estimate(&preview, cx))
    }
}

enum Msg {
    Progress(f32),
    Done(Result<(), String>),
}

impl Workspace {
    pub(crate) fn export(
        &mut self,
        _: &act::Export,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.export.is_some() {
            return;
        }
        if self.doc.project.timeline.duration() <= Time::ZERO {
            crate::workspace::notify(Notification::warning("时间轴是空的"), window, cx);
            return;
        }
        let dir = self
            .doc
            .path()
            .and_then(|p| p.parent().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let rx = cx.prompt_for_new_path(&dir, Some("导出.mp4"));
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    let project = this.doc.project.settings;
                    let duration = this.doc.project.timeline.duration();
                    Self::open_export_dialog(path, project, duration, window, cx);
                });
            }
        }));
    }

    fn open_export_dialog(
        path: PathBuf,
        project: ProjectSettings,
        duration: Time,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let form = cx.new(|cx| ExportForm::new(path, project, duration, window, cx));
        let ws = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let form = form.clone();
            let ws = ws.clone();
            dialog
                .title("导出")
                .width(px(560.))
                .child(form.clone())
                // 普通 Dialog 不会自己画按钮：on_ok 只登记回调，按钮要放进 footer
                .footer(
                    DialogFooter::new()
                        .child(
                            div()
                                .id("export-cancel-dialog")
                                .test_support()
                                .child(DialogClose::new().trigger(|b| b.ghost().label("取消"))),
                        )
                        .child(DialogAction::new().child(
                            Button::new("export-confirm").primary().label("开始导出"),
                        )),
                )
                .on_ok(move |_, window, cx| {
                    let settings = form.read(cx).settings(cx);
                    ws.update(cx, |this, cx| this.start_export(settings, window, cx));
                    true
                })
        });
    }

    /// 后台导出；进度显示在工具栏。
    pub fn start_export(
        &mut self,
        settings: ExportSettings,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.export = Some(ExportProgress {
            progress: 0.0,
            cancel: Arc::clone(&cancel),
            path: settings.path.clone(),
        });
        let project = self.doc.project.clone();
        let (tx, rx) = smol::channel::unbounded::<Msg>();
        let spawned = std::thread::Builder::new()
            .name("ve-export".into())
            .spawn(move || {
                // 导出里任何意外的 panic 也要变成失败消息，否则界面会一直停在某个进度
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    ve_export::export(
                        &project,
                        &settings,
                        |p| drop(tx.try_send(Msg::Progress(p))),
                        &cancel,
                    )
                    .map_err(|e| e.to_string())
                }))
                .unwrap_or_else(|_| Err("导出时出现内部错误".to_owned()));
                drop(tx.try_send(Msg::Done(result)));
            });
        if let Err(e) = spawned {
            self.export = None;
            crate::workspace::notify(
                Notification::error(format!("无法开始导出：{e}")),
                window,
                cx,
            );
            return;
        }
        // 定时取进度（不 await 跨线程通道，原因同引擎事件）
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let mut done = false;
                while let Ok(msg) = rx.try_recv() {
                    done |= matches!(msg, Msg::Done(_));
                    let alive = this
                        .update_in(cx, |this, window, cx| this.on_export_msg(msg, window, cx))
                        .is_ok();
                    done |= !alive;
                }
                if done {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn on_export_msg(&mut self, msg: Msg, window: &mut Window, cx: &mut Context<'_, Self>) {
        match msg {
            Msg::Progress(p) => {
                if let Some(e) = self.export.as_mut() {
                    e.progress = p;
                }
            }
            Msg::Done(result) => {
                let path = self.export.take().map(|e| e.path);
                match result {
                    Ok(()) => {
                        let shown = path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        crate::workspace::notify(
                            Notification::success(format!("导出完成：{shown}")),
                            window,
                            cx,
                        );
                        self.last_export = path;
                    }
                    Err(e) => {
                        crate::workspace::notify(
                            Notification::error(e).autohide(false),
                            window,
                            cx,
                        );
                    }
                }
            }
        }
        cx.notify();
    }

    /// 是否在导出。
    #[must_use]
    pub const fn is_exporting(&self) -> bool {
        self.export.is_some()
    }

    pub(crate) fn render_export_status(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let Some(e) = self.export.as_ref() else {
            let done = self.last_export.clone()?;
            return Some(
                Button::new("export-reveal")
                    .ghost()
                    .compact()
                    .label("打开所在文件夹")
                    .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&done)))
                    .into_any_element(),
            );
        };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "0–100"
        )]
        let pct = (e.progress * 100.0).round() as u32;
        Some(
            h_flex()
                .gap_2()
                .text_sm()
                .child(SharedString::from(format!("导出中 {pct}%")))
                .child(
                    Button::new("export-cancel")
                        .ghost()
                        .compact()
                        .label("取消")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(e) = &this.export {
                                e.cancel.store(true, Ordering::Release);
                            }
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_size_keeps_aspect_and_even_width() {
        assert_eq!(scaled_size(1920, 1080, Resolution::Project), (1920, 1080));
        assert_eq!(scaled_size(1920, 1080, Resolution::P720), (1280, 720));
        // 按短边：竖屏的 720P 是 720×1280（剪映的叫法）
        assert_eq!(scaled_size(1080, 1920, Resolution::P720), (720, 1280));
        assert_eq!(scaled_size(3840, 2160, Resolution::P1080), (1920, 1080));
        assert_eq!(scaled_size(1920, 1080, Resolution::P2160), (3840, 2160));
        assert_eq!(scaled_size(1920, 1080, Resolution::P1440), (2560, 1440));
    }

    #[test]
    fn choices_turn_into_export_settings() {
        let project = ProjectSettings::default();
        let path = PathBuf::from("out.mp4");
        let rec = ExportChoices::default().settings(path.clone(), &project);
        assert_eq!((rec.width, rec.height), (1920, 1080));
        assert_eq!(rec.frame_rate, project.frame_rate);
        assert_eq!(rec.video_bitrate_kbps, 9300);
        assert_eq!(rec.codec, Codec::H264);
        let small = ExportChoices {
            resolution: Resolution::P720,
            frame_rate: Some(FrameRate::new(60, 1)),
            bitrate: Bitrate::Lower,
            codec: Codec::Hevc,
            ..ExportChoices::default()
        }
        .settings(path.clone(), &project);
        assert_eq!((small.width, small.height), (1280, 720));
        assert_eq!(small.frame_rate, FrameRate::new(60, 1));
        assert_eq!(
            small.video_bitrate_kbps, 2900,
            "720p60 HEVC 推荐 5800 的一半"
        );
        let custom = ExportChoices {
            bitrate: Bitrate::Custom,
            custom_kbps: 3,
            ..ExportChoices::default()
        }
        .settings(path, &project);
        assert_eq!(custom.video_bitrate_kbps, 100, "夹到最小值");
        assert_eq!(size_text(12_345_678), "约 12.3 MB");
        assert_eq!(size_text(2_500_000_000), "约 2.50 GB");
    }
}
