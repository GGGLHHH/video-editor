//! 顶栏的草稿名点一下就能改（剪映）：存过的工程把文件改名（同一个文件夹，重名时换个不重的），
//! 没存过的就按这个名字存成草稿。

use crate::workspace::Workspace;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::{AppContext as _, Context, Entity, Subscription, Window};
use std::path::{Path, PathBuf};

/// 正在改名的输入框。
pub(crate) struct Rename {
    pub(crate) input: Entity<InputState>,
    _sub: Subscription,
}

impl std::fmt::Debug for Rename {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rename").finish_non_exhaustive()
    }
}

/// `old` 改名成 `name` 后的路径：同一个文件夹、`.vep`；已经有这个文件时加「(2)」「(3)」……
fn renamed_path(old: &Path, name: &str) -> PathBuf {
    let dir = old.parent().unwrap_or_else(|| Path::new("."));
    crate::autosave::unique_path(dir, name)
}

impl Workspace {
    /// 点顶栏的草稿名：变成输入框，全选好。
    pub(crate) fn start_rename(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.compound_stack.is_empty() {
            return;
        }
        let now = self.doc.name();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(now));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit_rename(window, cx);
                }
            },
        );
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.rename = Some(Rename { input, _sub: sub });
        cx.notify();
    }

    /// 输完了：改名（名字没变或空着就不动）。
    fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(r) = self.rename.take() else {
            return;
        };
        let name = crate::export_ui::file_title(r.input.read(cx).value().trim());
        window.focus(&self.focus, cx);
        cx.notify();
        if name.is_empty() || name == self.doc.name() {
            return;
        }
        let result = if let Some(old) = self.doc.path().map(Path::to_path_buf) {
            let new = renamed_path(&old, &name);
            std::fs::rename(&old, &new).map(|()| {
                self.doc.set_path(new.clone());
                self.recent.remove(&old);
                new
            })
        } else {
            // 没存过：按这个名字存成草稿
            let dir = self.settings.drafts();
            let path = crate::autosave::unique_path(&dir, &name);
            std::fs::create_dir_all(&dir).and_then(|()| {
                crate::project_io::save(&mut self.doc, &path).map_err(std::io::Error::other)
            })
        };
        match result {
            Ok(path) => self.remember_recent(path),
            Err(e) => {
                crate::workspace::notify(Notification::error(format!("改名失败：{e}")), window, cx);
            }
        }
    }
}
