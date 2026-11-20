//! 播放器面板的标题「播放器-时间线01」和右边的 ≡ 菜单（剪映）：画质、预览轴、全屏预览。

use crate::chrome;
use crate::preview::Quality;
use crate::theme::palette;
use crate::workspace::{Popup, Workspace};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, deferred, div, px,
};

/// 播放器标题（只有一条时间线，剪映在这里写它的名字）。
pub(crate) const PLAYER_TITLE: &str = "播放器-时间线01";

/// 菜单里的一项。
#[derive(Clone, Copy)]
enum Item {
    Quality(Quality),
    PreviewAxis,
    Fullscreen,
}

impl Workspace {
    /// 预览轴开着没有（测试也看它）。
    #[must_use]
    pub const fn preview_axis_on(&self) -> bool {
        self.timeline.preview_axis
    }

    /// 播放器面板的标题行：标题 + ≡ 菜单。
    pub(crate) fn render_player_header(&self, cx: &Context<'_, Self>) -> AnyElement {
        chrome::panel_header(PLAYER_TITLE, cx)
            .child(div().flex_1())
            .child(
                div()
                    .relative()
                    .child(
                        Button::new("player-menu-btn")
                            .ghost()
                            .compact()
                            .icon(Icon::new(Lucide::Menu))
                            .tooltip("播放器设置")
                            .on_click(cx.listener(|this, e: &gpui_kit::ClickEvent, _, cx| {
                                this.click_popup_button(Popup::Player, e);
                                cx.notify();
                            })),
                    )
                    .children(
                        self.popup_open(Popup::Player)
                            .then(|| deferred(self.render_player_menu(cx)).with_priority(1)),
                    ),
            )
            .into_any_element()
    }

    /// 菜单项右边的快捷键（按改过的键显示）。
    fn hint(cx: &Context<'_, Self>, default: &str) -> String {
        crate::keymap::slot_for_default(cx, default).map_or_else(
            || crate::keymap::to_shown(default),
            |slot| crate::keymap::to_shown(&crate::keymap::current(cx, &slot)),
        )
    }

    fn render_player_menu(&self, cx: &Context<'_, Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let row =
            |id: SharedString, label: &'static str, checked: bool, hint: String, item: Item| {
                h_flex()
                    .id(id)
                    .test_support()
                    .h(px(26.))
                    .px_2()
                    .gap_2()
                    .rounded(px(4.))
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(palette::menu_hover()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_player_item(item, window, cx);
                    }))
                    .child(
                        div()
                            .w(px(12.))
                            .text_color(palette::accent())
                            .child(if checked { "✓" } else { "" }),
                    )
                    .child(div().flex_1().child(label))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(SharedString::from(hint)),
                    )
            };
        let quality = Quality::ALL.map(|(id, label, _, q)| {
            row(
                SharedString::from(format!("player-{id}")),
                label,
                self.preview.quality == q,
                String::new(),
                Item::Quality(q),
            )
        });
        v_flex()
            .id("player-menu")
            .test_support()
            .absolute()
            .top(px(26.))
            .right_0()
            .w(px(200.))
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
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(muted)
                    .child("预览画质"),
            )
            .children(quality)
            .child(div().my_1().h(px(1.)).bg(palette::divider()))
            .child(row(
                "player-axis".into(),
                "预览轴",
                self.timeline.preview_axis,
                Self::hint(cx, "s"),
                Item::PreviewAxis,
            ))
            .child(row(
                "player-fullscreen".into(),
                "全屏预览",
                false,
                Self::hint(cx, "ctrl-shift-f"),
                Item::Fullscreen,
            ))
            .into_any_element()
    }

    fn pick_player_item(&mut self, item: Item, window: &Window, cx: &mut Context<'_, Self>) {
        self.popup = None;
        match item {
            Item::Quality(q) => self.set_quality(q, cx),
            Item::PreviewAxis => {
                self.timeline.preview_axis = !self.timeline.preview_axis;
                if !self.timeline.preview_axis {
                    self.end_preview_axis();
                }
            }
            Item::Fullscreen => self.set_fullscreen(true, window, cx),
        }
        cx.notify();
    }
}
