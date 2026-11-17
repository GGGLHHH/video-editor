//! 导出界面：选输出文件 → 选项对话框（分辨率 / 码率 / 编码器）→ 后台导出，工具栏显示进度和取消。

use crate::actions as act;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Window, div,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use ve_export::{Encoder, ExportSettings};

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

/// 输出分辨率选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// 跟工程一样。
    Project,
    /// 高 1080。
    P1080,
    /// 高 720。
    P720,
}

/// 按目标高度等比缩放，宽取偶数。
#[must_use]
pub fn scaled_size(width: u32, height: u32, res: Resolution) -> (u32, u32) {
    let target = match res {
        Resolution::Project => return (width, height),
        Resolution::P1080 => 1080,
        Resolution::P720 => 720,
    };
    let w = u32::try_from(u64::from(width) * u64::from(target) / u64::from(height.max(1)))
        .unwrap_or(u32::MAX);
    ((w & !1).max(2), target)
}

/// 导出选项表单。
#[derive(Debug)]
pub struct ExportForm {
    path: PathBuf,
    resolution: Resolution,
    encoder: Encoder,
    bitrate: Entity<InputState>,
}

impl ExportForm {
    fn new(path: PathBuf, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let bitrate = cx.new(|cx| InputState::new(window, cx).default_value("12000"));
        Self {
            path,
            resolution: Resolution::Project,
            encoder: Encoder::Nvenc,
            bitrate,
        }
    }

    fn settings(&self, project: &ve_core::Project, cx: &gpui_kit::App) -> ExportSettings {
        let s = project.settings;
        let (width, height) = scaled_size(s.width, s.height, self.resolution);
        let kbps = self
            .bitrate
            .read(cx)
            .value()
            .trim()
            .parse()
            .unwrap_or(12_000);
        ExportSettings {
            path: self.path.clone(),
            width,
            height,
            frame_rate: s.frame_rate,
            video_bitrate_kbps: kbps,
            encoder: self.encoder,
        }
    }
}

impl Render for ExportForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let choice = |id: &'static str, label: &'static str, on: bool| {
            let b = Button::new(id).compact().label(label);
            if on { b.primary() } else { b.ghost() }
        };
        let res = self.resolution;
        let enc = self.encoder;
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(self.path.display().to_string())),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(div().w_20().text_sm().child("分辨率"))
                    .child(
                        choice("res-project", "同工程", res == Resolution::Project).on_click(
                            cx.listener(|f, _, _, cx| {
                                f.resolution = Resolution::Project;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        choice("res-1080", "1080p", res == Resolution::P1080).on_click(
                            cx.listener(|f, _, _, cx| {
                                f.resolution = Resolution::P1080;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(choice("res-720", "720p", res == Resolution::P720).on_click(
                        cx.listener(|f, _, _, cx| {
                            f.resolution = Resolution::P720;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(div().w_20().text_sm().child("编码"))
                    .child(
                        choice("enc-nvenc", "NVENC（显卡）", enc == Encoder::Nvenc).on_click(
                            cx.listener(|f, _, _, cx| {
                                f.encoder = Encoder::Nvenc;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        choice("enc-x264", "x264（CPU）", enc == Encoder::X264).on_click(
                            cx.listener(|f, _, _, cx| {
                                f.encoder = Encoder::X264;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(div().w_20().text_sm().child("码率 kbps"))
                    .child(Input::new(&self.bitrate)),
            )
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
        if self.doc.project.timeline.duration() <= ve_core::Time::ZERO {
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
                let _ignored = this.update_in(cx, |_, window, cx| {
                    Self::open_export_dialog(path, window, cx);
                });
            }
        }));
    }

    fn open_export_dialog(path: PathBuf, window: &mut Window, cx: &mut Context<'_, Self>) {
        let form = cx.new(|cx| ExportForm::new(path, window, cx));
        let ws = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let form = form.clone();
            let ws = ws.clone();
            dialog
                .title("导出视频")
                .width(gpui_kit::px(460.))
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
                    let settings = ws.read(cx).doc.project.clone();
                    let settings = form.read(cx).settings(&settings, cx);
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
                let result = ve_export::export(
                    &project,
                    &settings,
                    |p| drop(tx.try_send(Msg::Progress(p))),
                    &cancel,
                )
                .map_err(|e| e.to_string());
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

    pub(crate) fn render_export_status(
        &self,
        cx: &Context<'_, Self>,
    ) -> Option<gpui_kit::AnyElement> {
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
        assert_eq!(scaled_size(1080, 1920, Resolution::P720), (404, 720));
        assert_eq!(scaled_size(3840, 2160, Resolution::P1080), (1920, 1080));
    }
}
