//! 属性面板的属性组（剪映）：组标题一行是「☑ 名字 ▴」+ 右边「↻」重置；点标题折叠 / 展开，
//! 可勾选的组不勾就不生效（取消勾选 = 这一组回到默认值）。

use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

/// 组标题上的动作。
pub(crate) type GroupAction = fn(&mut Workspace, &mut Window, &mut Context<'_, Workspace>);

/// 勾选框：勾上（`true`）/ 取消。
pub(crate) type GroupToggle = fn(&mut Workspace, bool, &mut Window, &mut Context<'_, Workspace>);

/// 一个属性组。
pub(crate) struct Group {
    /// 编号（如 `tf`），折叠状态按它记；标题、勾选框、重置按钮的编号是 `编号-title` 等。
    pub id: &'static str,
    /// 名字。
    pub title: &'static str,
    /// 可勾选的组：现在勾没勾、点勾选框时做什么（勾上 / 取消）。
    pub check: Option<(bool, GroupToggle)>,
    /// 「↻」重置这一组。
    pub reset: Option<GroupAction>,
    /// 组标题右边额外的东西（关键帧按钮）。
    pub extra: Option<AnyElement>,
}

impl Workspace {
    /// 画一个属性组：标题行 + （展开时）内容。
    pub(crate) fn render_group(
        &self,
        group: Group,
        body: AnyElement,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        let id = group.id;
        let collapsed = self.inspector.collapsed.contains(id);
        let muted = cx.theme().muted_foreground;
        let title = h_flex()
            .id(SharedString::from(format!("{id}-title")))
            .test_support()
            .gap_1()
            .cursor_pointer()
            .text_sm()
            .child(group.title)
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(if collapsed { "▾" } else { "▴" }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.inspector.collapsed.remove(id) {
                    this.inspector.collapsed.insert(id);
                }
                cx.notify();
            }));
        let check = group.check.map(|(on, toggle)| {
            div()
                .id(SharedString::from(format!("{id}-check")))
                .test_support()
                .size(px(14.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .border_1()
                .border_color(if on {
                    palette::accent()
                } else {
                    cx.theme().border
                })
                .when(on, |s| s.bg(palette::accent()))
                .text_xs()
                .cursor_pointer()
                .child(if on { "✓" } else { "" })
                .on_click(cx.listener(move |this, _, window, cx| {
                    toggle(this, !on, window, cx);
                    cx.notify();
                }))
        });
        let reset = group.reset.map(|reset| {
            Button::new(SharedString::from(format!("{id}-reset")))
                .ghost()
                .compact()
                .label("↻")
                .tooltip("重置这一组")
                .on_click(cx.listener(move |this, _, window, cx| reset(this, window, cx)))
        });
        let header = h_flex()
            .gap_2()
            .children(check)
            .child(title)
            .child(div().flex_1())
            .children(reset)
            .children(group.extra);
        let off = group.check.is_some_and(|(on, _)| !on);
        v_flex()
            .id(SharedString::from(id))
            .test_support()
            .gap_2()
            .child(header)
            .children((!collapsed).then(|| {
                // 没勾上的组：内容变淡（剪映），改了值就自动勾上
                div().when(off, |s| s.opacity(0.5)).child(body)
            }))
            .into_any_element()
    }
}
