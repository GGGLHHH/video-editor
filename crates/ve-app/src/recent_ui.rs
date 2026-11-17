//! 工具栏「最近 ▾」下拉菜单。

use crate::recent::RecentList;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, deferred, div, px,
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
        self.recent_menu_open = false;
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

    /// 「最近 ▾」按钮和展开的列表。
    pub(crate) fn render_recent_button(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        div()
            .relative()
            .child(
                Button::new("tb-recent")
                    .ghost()
                    .compact()
                    .label("最近 ▾")
                    .tooltip("最近打开的工程")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.recent_menu_open = true;
                        cx.notify();
                    })),
            )
            .children(
                self.recent_menu_open
                    .then(|| deferred(self.render_recent_list(cx)).with_priority(1)),
            )
    }

    fn render_recent_list(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let items = self.recent.paths().iter().enumerate().map(|(i, path)| {
            let exists = path.exists();
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let target = path.clone();
            h_flex()
                .id(SharedString::from(format!("recent-{i}")))
                .test_support()
                .px_2()
                .py_1()
                .rounded_md()
                .hover(|s| s.bg(cx.theme().accent))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_recent(target.clone(), window, cx);
                }))
                .child(
                    v_flex()
                        .min_w_0()
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
                        ),
                )
        });
        let empty = self.recent.paths().is_empty();
        v_flex()
            .id("recent-menu")
            .test_support()
            .absolute()
            .top(px(34.))
            .left_0()
            .w(px(440.))
            .p_1()
            .gap_0p5()
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.recent_menu_open = false;
                cx.notify();
            }))
            .children(items)
            .children(empty.then(|| {
                div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(muted)
                    .child("还没有最近打开的工程")
            }))
            .children((!empty).then(|| {
                h_flex()
                    .justify_end()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .pt_1()
                    .child(
                        Button::new("recent-clear")
                            .ghost()
                            .compact()
                            .label("清空列表")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.recent.clear();
                                this.save_recent();
                                this.recent_menu_open = false;
                                cx.notify();
                            })),
                    )
            }))
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
