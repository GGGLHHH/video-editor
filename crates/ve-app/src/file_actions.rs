//! 文件类动作：新建、打开、保存、另存、导入 / 导出 SRT。

use crate::actions as act;
use crate::document::Document;
use crate::project_io;
use crate::workspace::Workspace;
use gpui_kit::component::notification::Notification;
use gpui_kit::{Context, PathPromptOptions, PromptLevel, Window};
use std::path::PathBuf;
use ve_core::srt::{export_srt, import_srt, parse_ass, parse_lrc, parse_srt, write_srt};

/// 对话框的起始目录：工程所在目录，否则用户的「视频」文件夹。
pub(crate) fn start_dir(doc: &Document) -> PathBuf {
    doc.path()
        .and_then(|p| p.parent().map(PathBuf::from))
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Videos")))
        .unwrap_or_else(|| PathBuf::from("."))
}

impl Workspace {
    /// 有未保存修改时先问；用户选「不保存」或已保存后执行 `then`。
    pub(crate) fn confirm_discard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<'_, Self>) + 'static,
    ) {
        self.leave_all_compounds(window, cx);
        if !self.doc.is_dirty() {
            then(self, window, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "当前工程有未保存的修改",
            Some("要先保存吗？"),
            &["保存", "不保存", "取消"],
            cx,
        );
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await;
            let _ignored = this.update_in(cx, |this, window, cx| match choice {
                Ok(0) => this.save_then(window, cx, then),
                Ok(1) => then(this, window, cx),
                _ => {}
            });
        }));
    }

    /// 保存后执行 `then`：有路径直接存；没有路径先弹「另存为」，选好并存成功后再执行。
    /// 取消另存为或保存失败时不执行 `then`。
    fn save_then(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<'_, Self>) + 'static,
    ) {
        if let Some(path) = self.doc.path().map(PathBuf::from) {
            if self.save_to(&path, window, cx) {
                then(self, window, cx);
            }
            return;
        }
        let rx = cx.prompt_for_new_path(&start_dir(&self.doc), Some("未命名.vep"));
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    if this.save_to(&path, window, cx) {
                        then(this, window, cx);
                    }
                });
            }
        }));
    }

    /// 保存到 `path`，返回是否成功（失败时弹提示）。
    fn save_to(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        // 在复合片段里面：先回到最外层再存（存的是整个工程）
        self.leave_all_compounds(window, cx);
        match project_io::save(&mut self.doc, path) {
            Ok(saved) => {
                self.remember_recent(saved);
                // 新草稿第一次保存、另存为：崩溃恢复记录换成新路径
                self.sync_session();
                cx.notify();
                true
            }
            Err(e) => {
                crate::workspace::notify(Notification::error(e), window, cx);
                false
            }
        }
    }

    pub(crate) fn new_project(
        &mut self,
        _: &act::NewProject,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.confirm_discard(window, cx, |this, window, cx| {
            this.replace_document(Document::new(), window, cx);
        });
    }

    pub(crate) fn open(&mut self, _: &act::Open, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.confirm_discard(window, cx, |this, window, cx| {
            let rx = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("打开工程".into()),
            });
            this.tasks.push(cx.spawn_in(window, async move |this, cx| {
                if let Ok(Ok(Some(paths))) = rx.await
                    && let Some(path) = paths.into_iter().next()
                {
                    let _ignored =
                        this.update_in(cx, |this, window, cx| this.open_path(&path, window, cx));
                }
            }));
        });
    }

    /// 打开指定工程文件。
    pub fn open_path(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match project_io::open(path) {
            Ok(opened) => {
                self.replace_document(opened.document, window, cx);
                self.remember_recent(path.to_path_buf());
                if !opened.missing.is_empty() {
                    let names = opened.missing.join("、");
                    crate::workspace::notify(
                        Notification::warning(format!(
                            "找不到这些素材，相关片段会显示为黑场 / 静音：{names}。在素材面板点「链接媒体」重新指定文件"
                        ))
                        .autohide(false),
                        window,
                        cx,
                    );
                }
            }
            Err(e) => crate::workspace::notify(Notification::error(e), window, cx),
        }
    }

    pub(crate) fn save(&mut self, _: &act::Save, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.save_then(window, cx, |_, window, cx| {
            crate::workspace::notify(Notification::success("已保存"), window, cx);
        });
    }

    pub(crate) fn save_as(
        &mut self,
        _: &act::SaveAs,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let rx = cx.prompt_for_new_path(&start_dir(&self.doc), Some("未命名.vep"));
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    if this.save_to(&path, window, cx) {
                        crate::workspace::notify(Notification::success("已保存"), window, cx);
                    }
                });
            }
        }));
    }

    pub(crate) fn import_srt(
        &mut self,
        _: &act::ImportSrt,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("导入 SRT 字幕".into()),
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    this.import_srt_path(&path, window, cx);
                });
            }
        }));
    }

    /// 导入指定 SRT 文件。
    pub fn import_srt_path(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let text = match std::fs::read(path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) => {
                crate::workspace::notify(
                    Notification::error(format!("读不了 {}：{e}", path.display())),
                    window,
                    cx,
                );
                return;
            }
        };
        // 按扩展名：.lrc 歌词、.ass / .ssa（剪映「导入字幕」也认），其余当 SRT
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let parsed = match ext.as_str() {
            "lrc" => parse_lrc(&text),
            "ass" | "ssa" => parse_ass(&text),
            _ => parse_srt(&text),
        };
        match parsed {
            Ok(cues) => {
                if let Some(report) = self.apply(window, cx, |p| import_srt(p, &cues)) {
                    let skipped = if report.skipped > 0 {
                        format!("，跳过 {} 条（和已有字幕重叠或长度为零）", report.skipped)
                    } else {
                        String::new()
                    };
                    let msg = format!("导入了 {} 条字幕{skipped}", report.added.len());
                    crate::workspace::notify(Notification::success(msg), window, cx);
                }
            }
            Err(e) => crate::workspace::notify(Notification::error(e.to_string()), window, cx),
        }
    }

    /// 右键「导出所选片段」：区域设成选中片段从最早起点到最晚终点，再弹导出框（默认只导出区域）。
    pub(crate) fn export_selection(
        &mut self,
        _: &act::ExportSelection,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let clips = self.doc.selection().to_vec();
        if clips.is_empty() {
            return;
        }
        if self
            .apply(window, cx, |p| ve_core::edit::range_from_clips(p, &clips))
            .is_some()
        {
            self.export(&act::Export, window, cx);
        }
    }

    /// 选中的第一个媒体片段的素材文件。
    pub(crate) fn selected_media_path(&self) -> Option<PathBuf> {
        let project = &self.doc.project;
        self.doc.selection().iter().find_map(|&id| {
            let (_, c) = project.timeline.find_clip(id)?;
            project.assets.get(&c.asset()?).map(|a| a.path.clone())
        })
    }

    /// 右键「打开文件所在位置」：在资源管理器里打开素材所在的文件夹并选中它。
    pub(crate) fn reveal_in_folder(
        &mut self,
        _: &act::RevealInFolder,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(path) = self.selected_media_path() else {
            return;
        };
        if let Err(e) = std::process::Command::new("explorer.exe")
            .args(reveal_args(&path))
            .spawn()
        {
            crate::workspace::notify(
                Notification::warning(format!("打不开资源管理器：{e}")),
                window,
                cx,
            );
        }
    }

    /// 「导出静帧」：播放头这一帧按工程大小存成 PNG。
    pub(crate) fn export_still(
        &mut self,
        _: &act::ExportStill,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let rx = cx.prompt_for_new_path(&start_dir(&self.doc), Some("静帧.png"));
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let _ignored = this.update(cx, |this, cx| {
                    this.export_still_to(project_io::with_png(&path), cx);
                });
            }
        }));
    }

    /// 正在导出静帧（测试用）。
    #[must_use]
    pub const fn is_exporting_still(&self) -> bool {
        self.still_task.is_some()
    }

    /// 把播放头这一帧存成 PNG：先把这一帧（整个工程合成好的，工程大小）导出成一帧的视频，
    /// 再取出那一帧——和导出视频走同一条路，画面和导出的一样。
    pub fn export_still_to(&mut self, out: PathBuf, cx: &mut Context<'_, Self>) {
        use gpui_kit::AppContext as _;
        let project = self.whole_project();
        let at = self.playhead_frame();
        let frame = project.settings.frame_rate.frame_duration();
        let work = cx.background_spawn(async move {
            // 临时的一帧视频：系统临时目录里起个不重名的，用完删掉
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let clip =
                std::env::temp_dir().join(format!("ve-still-{}-{nanos}.mp4", std::process::id()));
            let mut settings = crate::export_ui::ExportChoices {
                bitrate: crate::export_ui::Bitrate::Higher,
                ..crate::export_ui::ExportChoices::default()
            }
            .settings(clip.clone(), &project.settings);
            settings.range = Some((at, at + frame));
            ve_export::export(
                &project,
                &settings,
                |_| {},
                &std::sync::atomic::AtomicBool::new(false),
            )
            .map_err(|e| {
                let _ignored = std::fs::remove_file(&clip);
                e.to_string()
            })?;
            let grabbed = ve_export::grab_frame(&clip, ve_core::Time::ZERO, &out);
            let _ignored = std::fs::remove_file(&clip);
            grabbed.map(|()| out)
        });
        self.still_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ignored = this.update(cx, |this, cx| {
                this.still_task = None;
                match result {
                    Ok(path) => log::info!("静帧存到 {}", path.display()),
                    Err(e) => log::warn!("导出静帧失败：{e}"),
                }
                cx.notify();
            });
        }));
    }

    /// 「粘贴文稿生成字幕」：弹一个框贴文稿，按句拆成字幕从播放头起一句接一句放。
    #[expect(clippy::unused_self, reason = "动作处理函数要挂在工作区上")]
    pub(crate) fn import_script(
        &mut self,
        _: &act::ImportScript,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        use gpui_kit::base::TestSupportExt as _;
        use gpui_kit::component::WindowExt as _;
        use gpui_kit::component::button::{Button, ButtonVariants as _};
        use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
        use gpui_kit::component::input::{Textarea, TextareaState};
        use gpui_kit::{AppContext as _, InteractiveElement as _, ParentElement as _, div, px};
        let ws = cx.entity();
        let text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(10)
                .placeholder("把文稿贴在这里：按句号、问号、感叹号、分号和换行断句")
        });
        window.open_dialog(cx, move |dialog, _, _| {
            let ws = ws.clone();
            let text = text.clone();
            dialog
                .title("粘贴文稿生成字幕")
                .width(px(520.))
                .child(
                    div()
                        .id("script-text")
                        .test_support()
                        .child(Textarea::new(&text)),
                )
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|b| b.ghost().label("取消")))
                        .child(
                            DialogAction::new()
                                .child(Button::new("script-ok").primary().label("生成字幕")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let t = text.read(cx).value().to_string();
                    ws.update(cx, |this, cx| this.import_script_text(&t, window, cx));
                    true
                })
        });
    }

    /// 把文稿 `text` 按句拆成字幕，从播放头起放（和已有字幕重叠的跳过）。
    pub fn import_script_text(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let cues = ve_core::srt::script_cues(text, self.playhead);
        if cues.is_empty() {
            crate::workspace::notify(Notification::warning("文稿里没有句子"), window, cx);
            return;
        }
        if let Some(report) = self.apply(window, cx, |p| import_srt(p, &cues)) {
            let msg = format!("生成了 {} 条字幕", report.added.len());
            crate::workspace::notify(Notification::success(msg), window, cx);
        }
    }

    pub(crate) fn export_srt(
        &mut self,
        _: &act::ExportSrt,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let rx = cx.prompt_for_new_path(&start_dir(&self.doc), Some("字幕.srt"));
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    let text = write_srt(&export_srt(&this.whole_project()));
                    match std::fs::write(&path, text) {
                        Ok(()) => crate::workspace::notify(
                            Notification::success("字幕已导出"),
                            window,
                            cx,
                        ),
                        Err(e) => crate::workspace::notify(
                            Notification::error(format!("导出失败：{e}")),
                            window,
                            cx,
                        ),
                    }
                });
            }
        }));
    }
}

/// 资源管理器「选中这个文件」的参数（`explorer /select,路径`）。
fn reveal_args(path: &std::path::Path) -> [std::ffi::OsString; 1] {
    let mut arg = std::ffi::OsString::from("/select,");
    arg.push(path.as_os_str());
    [arg]
}

#[cfg(test)]
mod reveal_tests {
    use super::*;

    #[test]
    fn explorer_selects_the_file() {
        let [arg] = reveal_args(std::path::Path::new(r"C:\素材\a.mp4"));
        assert_eq!(arg, std::ffi::OsString::from(r"/select,C:\素材\a.mp4"));
    }
}
