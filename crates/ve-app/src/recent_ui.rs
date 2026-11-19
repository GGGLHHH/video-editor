//! 菜单里的「最近打开」：列表、打开、清空。

use crate::recent::RecentList;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use std::path::PathBuf;

impl Workspace {
    /// 换一个最近列表文件（测试用，避免碰到用户真实的列表）。
    pub fn use_recent_file(&mut self, file: PathBuf) {
        self.recent = RecentList::load(&file);
        self.recent_file = file;
    }

    /// 最近打开的工程。
    #[must_use]
    pub const fn recent(&self) -> &RecentList {
        &self.recent
    }

    /// 记一次打开 / 保存，并写盘（写失败只记日志）。
    pub(crate) fn remember_recent(&mut self, path: PathBuf) {
        self.recent.push(path);
        self.save_recent();
    }

    fn save_recent(&self) {
        if let Err(e) = self.recent.save(&self.recent_file) {
            log::warn!("保存最近列表失败 {}：{e}", self.recent_file.display());
        }
    }

    fn open_recent(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.popup = None;
        if !path.exists() {
            self.recent.remove(&path);
            self.save_recent();
            crate::workspace::notify(
                Notification::warning(format!("{} 已不存在，已从最近列表移除", path.display())),
                window,
                cx,
            );
            cx.notify();
            return;
        }
        self.confirm_discard(window, cx, move |this, window, cx| {
            this.open_path(&path, window, cx);
        });
    }

    /// 菜单里「最近打开」一节：每个工程一行，末尾「清空列表」。
    pub(crate) fn recent_items(&self, cx: &Context<'_, Self>) -> Vec<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let mut rows: Vec<AnyElement> =
            self.recent
                .paths()
                .iter()
                .enumerate()
                .map(|(i, path)| {
                    let exists = path.exists();
                    let name = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let target = path.clone();
                    v_flex()
                        .id(SharedString::from(format!("recent-{i}")))
                        .test_support()
                        .px_2()
                        .py_1()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(crate::theme::palette::menu_hover()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_recent(target.clone(), window, cx);
                        }))
                        .child(div().text_sm().when_missing(!exists, muted).child(
                            SharedString::from(if exists {
                                name
                            } else {
                                format!("{name}（文件不存在）")
                            }),
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .truncate()
                                .child(SharedString::from(path.display().to_string())),
                        )
                        .into_any_element()
                })
                .collect();
        if rows.is_empty() {
            rows.push(
                div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(muted)
                    .child("还没有最近打开的工程")
                    .into_any_element(),
            );
        } else {
            rows.push(
                h_flex()
                    .justify_end()
                    .child(
                        Button::new("recent-clear")
                            .ghost()
                            .compact()
                            .label("清空列表")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.recent.clear();
                                this.save_recent();
                                this.popup = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        rows
    }
}

/// 文件不存在的条目显示为灰色。
trait MissingStyle: gpui_kit::Styled + Sized {
    fn when_missing(self, missing: bool, color: gpui_kit::Hsla) -> Self {
        if missing {
            self.text_color(color)
        } else {
            self
        }
    }
}

impl<T: gpui_kit::Styled> MissingStyle for T {}
