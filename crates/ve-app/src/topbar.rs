//! 剪映式顶栏：左边「菜单 ▾」和保存状态，中间工程名，右边导出主按钮。
//!
//! 文件、编辑类操作都收进「菜单」（原来的一排工具按钮里的剪辑操作挪到了时间轴工具栏）。

use crate::actions as act;
use crate::chrome;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::{Selectable as _, TestSupportExt as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, h_flex, v_flex};
use gpui_kit::{
    Action, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, deferred, div, px,
};

/// 顶栏高度。
pub(crate) const TOPBAR_HEIGHT: f32 = 36.0;

/// 菜单里的一项：编号、文字、快捷键、动作。
struct MenuItem {
    id: &'static str,
    label: &'static str,
    keys: &'static str,
    action: fn() -> Box<dyn Action>,
}

const fn item(
    id: &'static str,
    label: &'static str,
    keys: &'static str,
    action: fn() -> Box<dyn Action>,
) -> MenuItem {
    MenuItem {
        id,
        label,
        keys,
        action,
    }
}

/// 菜单分组（和剪映一样：文件、编辑）。
fn menu_groups() -> [(&'static str, Vec<MenuItem>); 2] {
    [
        (
            "文件",
            vec![
                item("menu-new", "新建草稿", "Ctrl+N", || {
                    Box::new(act::NewProject)
                }),
                item("menu-open", "打开…", "Ctrl+O", || Box::new(act::Open)),
                item("menu-save", "保存", "Ctrl+S", || Box::new(act::Save)),
                item("menu-save-as", "另存为…", "Ctrl+Shift+S", || {
                    Box::new(act::SaveAs)
                }),
                item("menu-import", "导入素材…", "Ctrl+I", || {
                    Box::new(act::Import)
                }),
                item("menu-srt-in", "导入字幕（SRT）…", "", || {
                    Box::new(act::ImportSrt)
                }),
                item("menu-srt-out", "导出字幕（SRT）…", "", || {
                    Box::new(act::ExportSrt)
                }),
                item("menu-export", "导出视频…", "Ctrl+E", || {
                    Box::new(act::Export)
                }),
            ],
        ),
        (
            "编辑",
            vec![
                item("menu-undo", "撤销", "Ctrl+Z", || Box::new(act::Undo)),
                item("menu-redo", "恢复", "Ctrl+Y", || Box::new(act::Redo)),
                item("menu-delete", "删除", "Delete", || Box::new(act::Delete)),
                item("menu-select-all", "全选", "Ctrl+A", || {
                    Box::new(act::SelectAll)
                }),
            ],
        ),
    ]
}

impl Workspace {
    pub(crate) fn render_topbar(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let (dot, status) = self.save_status(muted);
        h_flex()
            .id("toolbar")
            .test_support()
            .relative()
            .h(px(TOPBAR_HEIGHT))
            .flex_none()
            .px_2()
            .gap_2()
            .bg(palette::window_bg())
            .child(
                div()
                    .px_1()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child("剪辑"),
            )
            .child(self.render_menu_button(cx))
            .child(
                h_flex()
                    .id("save-status")
                    .test_support()
                    .gap_1()
                    .text_xs()
                    .text_color(muted)
                    .child(div().size(px(7.)).rounded_full().bg(dot))
                    .child(status),
            )
            .child(
                // 工程名在整条顶栏里居中（不随左右内容挪动）
                h_flex()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .justify_center()
                    .text_sm()
                    .child(
                        div()
                            .id("project-name")
                            .test_support()
                            .child(SharedString::from(self.doc.name())),
                    ),
            )
            .child(div().flex_1())
            .children(self.render_export_status(cx))
            .child(self.render_layout_button(cx))
            .child(
                Button::new("tb-export")
                    .primary()
                    .compact()
                    .icon(Icon::new(Lucide::Upload))
                    .label("导出")
                    .disabled(self.export.is_some())
                    .on_click(cx.listener(|this, _, w, cx| this.export(&act::Export, w, cx))),
            )
    }

    /// 顶栏的保存状态：（圆点颜色，文字）。
    fn save_status(&self, muted: gpui_kit::Hsla) -> (gpui_kit::Hsla, SharedString) {
        let auto = self.autosave.as_ref();
        let error = auto.and_then(|a| a.error.clone());
        let saved_at = auto.and_then(|a| a.last_saved.clone());
        match (error, self.doc.is_dirty(), saved_at) {
            (Some(e), ..) => (gpui_kit::red(), format!("自动保存失败：{e}").into()),
            (None, true, _) => (muted, "有未保存的修改".into()),
            // 剪映：「● 00:53:02 自动保存本地」
            (None, false, Some(at)) => (palette::accent(), format!("{at} 自动保存本地").into()),
            (None, false, None) if self.doc.path().is_some() => {
                (palette::accent(), "已保存到本地".into())
            }
            (None, false, None) => (muted, "未保存".into()),
        }
    }

    fn render_menu_button(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        div()
            .relative()
            .child(
                Button::new("tb-menu")
                    .compact()
                    .label("菜单 ▾")
                    .selected(self.menu_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = !this.menu_open;
                        cx.notify();
                    })),
            )
            .children(
                self.menu_open
                    .then(|| deferred(self.render_menu(cx)).with_priority(1)),
            )
    }

    fn render_menu(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let mut rows: Vec<AnyElement> = Vec::new();
        for (gi, (group, items)) in menu_groups().into_iter().enumerate() {
            if gi > 0 {
                rows.push(separator(cx));
            }
            rows.push(group_label(group, muted));
            for it in items {
                let action = it.action;
                rows.push(
                    h_flex()
                        .id(it.id)
                        .test_support()
                        .h(px(28.))
                        .px_2()
                        .gap_4()
                        .justify_between()
                        .rounded(px(4.))
                        .text_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(palette::menu_hover()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.menu_open = false;
                            cx.notify();
                            window.dispatch_action(action(), cx);
                        }))
                        .child(it.label)
                        .child(chrome::keycaps(it.keys, cx))
                        .into_any_element(),
                );
            }
        }
        rows.push(separator(cx));
        rows.push(group_label("最近打开", muted));
        rows.extend(self.recent_items(cx));
        v_flex()
            .id("app-menu")
            .test_support()
            .absolute()
            .top(px(30.))
            .left_0()
            .w(px(360.))
            .max_h(px(640.))
            .overflow_y_scroll()
            .p_1()
            .bg(palette::menu_bg())
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(chrome::CARD_RADIUS))
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu_open = false;
                cx.notify();
            }))
            .children(rows)
    }
}

fn group_label(text: &'static str, color: gpui_kit::Hsla) -> AnyElement {
    div()
        .px_2()
        .pt_1()
        .pb_0p5()
        .text_xs()
        .text_color(color)
        .child(text)
        .into_any_element()
}

fn separator(cx: &gpui_kit::App) -> AnyElement {
    div()
        .my_1()
        .h(px(1.))
        .bg(cx.theme().border)
        .into_any_element()
}
