//! 剪映式界面外壳的公用小件：圆角面板卡片、面板标题栏、图标标签、文字标签、
//! 分段控件、快捷键键帽。

use crate::theme::palette;
use gpui_kit::base::{ObservedElement, TestSupportExt as _};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::{
    App, Div, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Stateful, Styled as _, div, px,
};

/// 面板卡片的圆角。
pub(crate) const CARD_RADIUS: f32 = 6.0;
/// 卡片之间缝隙的一半（每张卡片四周各留这么多）。
pub(crate) const CARD_GAP: f32 = 2.0;
/// 面板标题栏 / 标签栏的高度。
pub(crate) const HEADER_HEIGHT: f32 = 32.0;

/// 圆角面板卡片：填满所在的分栏，四周留出缝隙，里面竖排放 `body`。
pub(crate) fn card(body: impl IntoElement) -> Div {
    div().size_full().p(px(CARD_GAP)).child(
        v_flex()
            .size_full()
            .rounded(px(CARD_RADIUS))
            .bg(palette::panel_bg())
            .overflow_hidden()
            .child(body),
    )
}

/// 面板顶部的标题栏（如「播放器」），底部一条分隔线。
#[must_use]
pub(crate) fn panel_header(title: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .h(px(HEADER_HEIGHT))
        .flex_none()
        .px_3()
        .gap_2()
        .border_b_1()
        .border_color(palette::divider())
        .text_xs()
        .text_color(cx.theme().foreground)
        .child(title.into())
}

/// 图标在上、两个字在下的一级标签（素材面板顶部那一排）；选中的变成主色青。
pub(crate) fn icon_tab(
    id: impl Into<ElementId>,
    icon: impl Into<Icon>,
    label: &'static str,
    selected: bool,
    cx: &App,
) -> ObservedElement<Stateful<Div>> {
    let color = if selected {
        palette::accent()
    } else {
        cx.theme().foreground
    };
    v_flex()
        .id(id)
        .test_support()
        .items_center()
        .justify_center()
        .gap_0p5()
        .w(px(48.))
        .h(px(46.))
        .rounded(px(4.))
        .text_color(color)
        .cursor_pointer()
        .hover(|s| s.bg(cx.theme().accent))
        .child(icon.into().size(px(18.)).text_color(color))
        .child(div().text_xs().child(label))
}

/// 一行文字标签（属性面板顶部「画面 / 音频 / 变速…」）；选中的变成主色青。
pub(crate) fn text_tab(
    id: impl Into<ElementId>,
    label: &'static str,
    selected: bool,
    cx: &App,
) -> ObservedElement<Stateful<Div>> {
    div()
        .id(id)
        .test_support()
        .px_2()
        .h(px(HEADER_HEIGHT))
        .flex()
        .items_center()
        .text_sm()
        .cursor_pointer()
        .text_color(if selected {
            palette::accent()
        } else {
            cx.theme().foreground
        })
        .hover(|s| s.text_color(palette::accent()))
        .child(label)
}

/// 可选项小块（草稿参数里的比例、分辨率、帧率）：选中的是主色青描边 + 青字。
pub(crate) fn chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    cx: &App,
) -> ObservedElement<Stateful<Div>> {
    let (fg, border) = if selected {
        (palette::accent(), palette::accent())
    } else {
        (cx.theme().foreground, cx.theme().border)
    };
    div()
        .id(id)
        .test_support()
        .px_2()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .border_1()
        .border_color(border)
        .bg(cx.theme().muted)
        .text_xs()
        .text_color(fg)
        .cursor_pointer()
        .hover(|s| s.bg(cx.theme().accent))
        .child(label.into())
}

/// 快捷键键帽：深色小圆角块 + 白字（菜单项右侧）。
pub(crate) fn keycaps(keys: &str, cx: &App) -> Div {
    h_flex()
        .gap_0p5()
        .children(keys.split('+').filter(|k| !k.is_empty()).map(|k| {
            div()
                .px_1()
                .min_w(px(16.))
                .h(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .bg(palette::window_bg())
                .text_color(cx.theme().muted_foreground)
                .text_size(px(10.))
                .child(SharedString::from(k.to_owned()))
        }))
}
