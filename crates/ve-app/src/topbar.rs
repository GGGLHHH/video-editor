//! 剪映式顶栏：左边「菜单 ▾」和保存状态，中间工程名，右边导出主按钮。
//!
//! 文件、编辑类操作都收进「菜单」（原来的一排工具按钮里的剪辑操作挪到了时间轴工具栏）。

use crate::actions as act;
use crate::chrome;
use crate::theme::palette;
use crate::workspace::{Popup, Workspace};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::{Selectable as _, TestSupportExt as _};
use gpui_kit::component::Sizable as _;
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

/// 菜单分组（和剪映一样：文件、编辑、帮助）。
fn menu_groups() -> [(&'static str, Vec<MenuItem>); 3] {
    [
        (
            "文件",
            vec![
                item("menu-new", "新建草稿", "Ctrl+N", || {
                    Box::new(act::NewProject)
                }),
                item("menu-open", "打开…", "Ctrl+O", || Box::new(act::Open)),
                item("menu-save", "保存", "Ctrl+S", || Box::new(act::Save)),
                item("menu-save-as", "另存为…", "Ctrl+Alt+S", || {
                    Box::new(act::SaveAs)
                }),
                item("menu-import", "导入素材…", "Ctrl+I", || {
                    Box::new(act::Import)
                }),
                item(
                    "menu-srt-in",
                    "导入字幕（SRT / ASS / LRC）…",
                    "",
                    || Box::new(act::ImportSrt),
                ),
                item("menu-script", "粘贴文稿生成字幕…", "", || {
                    Box::new(act::ImportScript)
                }),
                item("menu-srt-out", "导出字幕（SRT）…", "", || {
                    Box::new(act::ExportSrt)
                }),
                item(
                    "menu-otio-in",
                    "导入 OTIO（Premiere / 达芬奇）…",
                    "",
                    || Box::new(act::ImportOtio),
                ),
                item(
                    "menu-otio-out",
                    "导出 OTIO（Premiere / 达芬奇）…",
                    "",
                    || Box::new(act::ExportOtio),
                ),
                item("menu-edl-out", "导出 EDL…", "", || {
                    Box::new(act::ExportEdl)
                }),
                item("menu-settings", "全局设置…", "", || {
                    Box::new(act::OpenSettings)
                }),
                item("menu-home", "回到首页", "", || Box::new(act::ShowHome)),
                item("menu-export", "导出视频…", "Ctrl+E", || {
                    Box::new(act::Export)
                }),
                item("menu-still", "导出静帧…", "", || {
                    Box::new(act::ExportStill)
                }),
                item("menu-quit", "退出", "Ctrl+Q", || Box::new(act::Quit)),
            ],
        ),
        (
            "编辑",
            vec![
                item("menu-undo", "撤销", "Ctrl+Z", || Box::new(act::Undo)),
                item("menu-redo", "恢复", "Ctrl+Y", || Box::new(act::Redo)),
                item("menu-copy", "复制", "Ctrl+C", || Box::new(act::Copy)),
                item("menu-cut", "剪切", "Ctrl+X", || Box::new(act::Cut)),
                item("menu-paste", "粘贴", "Ctrl+V", || Box::new(act::Paste)),
                item("menu-copy-attrs", "复制属性", "Ctrl+Shift+C", || {
                    Box::new(act::CopyAttributes)
                }),
                item("menu-paste-attrs", "粘贴属性", "Ctrl+Shift+V", || {
                    Box::new(act::PasteAttributes)
                }),
                item("menu-delete", "删除", "Delete", || Box::new(act::Delete)),
                item("menu-select-all", "全选", "Ctrl+A", || {
                    Box::new(act::SelectAll)
                }),
            ],
        ),
        (
            "帮助",
            vec![
                item("menu-shortcuts", "快捷键", "", || {
                    Box::new(act::ShowShortcuts)
                }),
                item("menu-about", "关于", "", || Box::new(act::About)),
            ],
        ),
    ]
}

impl Workspace {
    /// 顶栏放在窗口自己的标题栏里（没有系统标题栏）：右边是最小化 / 最大化 / 关闭，空白处拖动窗口。
    pub(crate) fn render_topbar(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        gpui_kit::component::TitleBar::new()
            .h(px(TOPBAR_HEIGHT))
            .pl(px(0.))
            .bg(palette::window_bg())
            .border_color(palette::window_bg())
            .child(self.render_topbar_items(cx))
    }

    fn render_topbar_items(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let (dot, status) = self.save_status(muted);
        h_flex()
            .id("toolbar")
            .test_support()
            .relative()
            .h_full()
            .flex_1()
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
                    .child(self.render_project_name(cx)),
            )
            .child(div().flex_1())
            .children(self.render_export_status(cx))
            .child(self.render_shortcuts_button(cx))
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

    /// 顶栏正中的草稿名：点一下变成输入框改名（剪映）。
    fn render_project_name(&self, cx: &Context<'_, Self>) -> AnyElement {
        if let Some(r) = &self.rename {
            return div()
                .id("project-rename")
                .test_support()
                .w(px(220.))
                .child(gpui_kit::component::input::Input::new(&r.input).small())
                .into_any_element();
        }
        div()
            .id("project-name")
            .test_support()
            .px_2()
            .rounded(px(4.))
            .cursor_text()
            .hover(|s| s.bg(cx.theme().muted))
            .child(SharedString::from(self.doc.name()))
            .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx)))
            .into_any_element()
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
                    .selected(self.popup_open(Popup::Menu))
                    .on_click(cx.listener(|this, e: &gpui_kit::ClickEvent, _, cx| {
                        this.click_popup_button(Popup::Menu, e);
                        cx.notify();
                    })),
            )
            .children(
                self.popup_open(Popup::Menu)
                    .then(|| deferred(self.render_menu(cx)).with_priority(1)),
            )
    }

    fn render_menu(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        // 两栏：左边「文件」，右边「编辑」「帮助」「最近打开」（一栏太长，屏幕矮时放不下）
        let mut left: Vec<AnyElement> = Vec::new();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (gi, (group, items)) in menu_groups().into_iter().enumerate() {
            if gi == 1 {
                left = std::mem::take(&mut rows);
            } else if gi > 1 {
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
                            this.popup = None;
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
            // 盖住下面的东西：点菜单不会顺带点到下面的画面 / 时间轴
            .occlude()
            .test_support()
            .absolute()
            .top(px(30.))
            .left_0()
            .w(px(640.))
            .max_h(px(640.))
            .overflow_y_scroll()
            .p_1()
            .bg(palette::menu_bg())
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(chrome::CARD_RADIUS))
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, e: &gpui_kit::MouseDownEvent, _, cx| {
                this.dismiss_popup(e.position);
                cx.notify();
            }))
            .child(
                h_flex()
                    .items_start()
                    .gap_1()
                    .child(v_flex().flex_1().min_w_0().children(left))
                    .child(v_flex().flex_1().min_w_0().children(rows)),
            )
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
