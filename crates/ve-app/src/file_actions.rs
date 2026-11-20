//! 文件类动作：新建、打开、保存、另存、导入 / 导出 SRT。

use crate::actions as act;
use crate::document::Document;
use crate::project_io;
use crate::workspace::Workspace;
use gpui_kit::component::notification::Notification;
use gpui_kit::{Context, PathPromptOptions, PromptLevel, Window};
use std::path::PathBuf;
use ve_core::srt::{export_srt, import_srt, parse_srt, write_srt};

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
        match parse_srt(&text) {
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
                    let text = write_srt(&export_srt(&this.doc.project));
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
