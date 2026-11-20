//! 录音（剪映工具栏的 🎤）：点一下从播放头开始录、时间轴跟着播放；再点一下停下，
//! 录音放到播放头开始的位置（第一条空着的音频轨，没有就新建一条）。

use crate::actions as act;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use std::path::PathBuf;
use ve_core::{Time, format_timecode};

/// 录音文件的目录：`%APPDATA%/video-editor/recordings`。
#[must_use]
pub(crate) fn default_dir() -> PathBuf {
    crate::recent::default_file()
        .parent()
        .map_or_else(|| PathBuf::from("recordings"), |p| p.join("recordings"))
}

impl Workspace {
    /// 换一个录音目录（测试用）。
    pub fn use_record_dir(&mut self, dir: PathBuf) {
        self.record_dir = dir;
    }

    /// 是不是在录音。
    #[must_use]
    pub const fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    pub(crate) fn toggle_record(
        &mut self,
        _: &act::ToggleRecord,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some((recorder, start)) = self.recording.take() {
            self.engine.pause();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let path = self.record_dir.join(format!("录音-{nanos}.wav"));
            match recorder.finish(&path) {
                Ok(_) => self.place_recording(path, start, window, cx),
                Err(e) => crate::workspace::notify(
                    Notification::warning(format!("录音失败：{e}")),
                    window,
                    cx,
                ),
            }
            cx.notify();
            return;
        }
        match ve_engine::Recorder::start_default() {
            Ok(r) => {
                self.end_preview_axis();
                let start = self.playhead_frame();
                self.recording = Some((r, start));
                self.engine.play();
            }
            Err(e) => crate::workspace::notify(
                Notification::warning(format!("没法录音：{e}")),
                window,
                cx,
            ),
        }
        cx.notify();
    }

    /// 录好的文件导入素材库，放到 `start`（一步撤销）。
    pub fn place_recording(
        &mut self,
        path: PathBuf,
        start: Time,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let probing = cx.background_spawn(async move { crate::import::probe_asset(&path) });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let probed = probing.await;
            let _ignored = this.update_in(cx, |this, window, cx| match probed {
                Ok(mut asset) => {
                    asset.name = format!(
                        "录音 {}",
                        format_timecode(start, this.doc.project.settings.frame_rate)
                    );
                    let id = this.doc.add_asset(asset);
                    let added = this.doc.project.assets.get(&id).cloned();
                    if let Some(c) =
                        this.apply(window, cx, |p| ve_core::edit::place_audio(p, id, start))
                    {
                        this.doc.select(c, false);
                        this.after_change(window, cx);
                    }
                    this.analyze_assets(added.into_iter().collect(), window, cx);
                }
                Err(e) => crate::workspace::notify(Notification::warning(e), window, cx),
            });
        }));
    }

    /// 工具栏的 🎤：录音中变红，显示录了多久和电平。
    pub(crate) fn render_record_button(&self, cx: &Context<'_, Self>) -> AnyElement {
        let Some((r, _)) = &self.recording else {
            return Button::new("tb-record")
                .ghost()
                .compact()
                .icon(Icon::new(Lucide::Mic))
                .tooltip("录音：从播放头开始录，时间轴同时播放；再点一下停止")
                .on_click(cx.listener(|_, _, window, cx| {
                    window.dispatch_action(Box::new(act::ToggleRecord), cx);
                }))
                .into_any_element();
        };
        let level = r.level();
        h_flex()
            .id("tb-recording")
            .test_support()
            .gap_1()
            .px_2()
            .h(px(24.))
            .rounded(px(4.))
            .bg(palette::recording().opacity(0.2))
            .cursor_pointer()
            .on_click(cx.listener(|_, _, window, cx| {
                window.dispatch_action(Box::new(act::ToggleRecord), cx);
            }))
            .child(div().size(px(8.)).rounded_full().bg(palette::recording()))
            .child(div().text_xs().child(SharedString::from(format!(
                "录音中 {}",
                format_timecode(r.elapsed(), self.doc.project.settings.frame_rate)
            ))))
            .child(
                div()
                    .w(px(40.))
                    .h(px(4.))
                    .bg(palette::divider())
                    .child(div().h_full().w(px(40.0 * level)).bg(palette::accent())),
            )
            .into_any_element()
    }
}
