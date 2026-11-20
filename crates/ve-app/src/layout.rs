//! 布局模式（剪映顶栏的「布局」）：默认布局、媒体素材优先、属性调节优先、音频创作、竖屏创作。

use crate::chrome;
use crate::theme::palette;
use crate::workspace::{Popup, Workspace};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, h_flex, h_resizable, resizable_panel, v_flex, v_resizable,
};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, deferred, div, px,
};

/// 布局模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LayoutMode {
    /// 上面素材 / 播放器 / 属性三栏，下面整条时间轴。
    #[default]
    Default,
    /// 媒体素材优先：素材在左边占满高度；右边上面播放器和属性、下面时间轴。
    MediaFirst,
    /// 属性调节优先：属性在右边占满高度；左边上面素材和播放器、下面时间轴。
    InspectorFirst,
    /// 音频创作：时间轴更高（看得清波形、好对节拍），素材栏切到「音频」。
    Audio,
    /// 竖屏创作：播放器在右边占满高度；左边上面素材和属性、下面时间轴。
    Portrait,
}

impl LayoutMode {
    /// 菜单里的选项：（编号，名字，模式）。
    pub(crate) const ALL: [(&'static str, &'static str, Self); 5] = [
        ("layout-default", "默认布局", Self::Default),
        ("layout-media", "媒体素材优先", Self::MediaFirst),
        ("layout-inspector", "属性调节优先", Self::InspectorFirst),
        ("layout-audio", "音频创作", Self::Audio),
        ("layout-portrait", "竖屏创作", Self::Portrait),
    ];
}

impl Workspace {
    /// 换布局。音频创作顺手把素材栏切到「音频」（剪映也是）。
    pub(crate) fn set_layout(&mut self, mode: LayoutMode) {
        self.layout = mode;
        self.popup = None;
        if mode == LayoutMode::Audio {
            self.bin_tab = crate::media_bin::BinTab::Audio;
        }
    }

    /// 时间轴卡片（各种布局共用）。
    fn timeline_card(&self, cx: &Context<'_, Self>) -> gpui_kit::Div {
        chrome::card(
            v_flex()
                .size_full()
                .child(self.render_timeline_toolbar(cx))
                .child(
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .child(self.render_timeline(cx)),
                        )
                        .child(self.render_level_meter(cx)),
                ),
        )
    }

    /// 按布局模式排四块面板。初始比例按剪映；之后可以拖分隔条。
    #[expect(
        clippy::too_many_lines,
        reason = "五种布局并排写，一眼能对照各自的排法"
    )]
    pub(crate) fn render_panels(&self, window: &Window, cx: &Context<'_, Self>) -> AnyElement {
        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        match self.layout {
            LayoutMode::Default | LayoutMode::Audio => {
                // 素材约 30%、属性约 22%，时间轴约占 42% 高；音频创作时时间轴约占 62% 高
                let timeline_h = if self.layout == LayoutMode::Audio {
                    0.62
                } else {
                    0.42
                };
                let top = h_resizable("main-split")
                    .child(
                        resizable_panel()
                            .size(px((vw * 0.30).max(240.)))
                            .child(chrome::card(self.render_media_bin(cx))),
                    )
                    .child(resizable_panel().child(chrome::card(self.render_preview(cx))))
                    .child(
                        resizable_panel()
                            .size(px((vw * 0.22).max(280.)))
                            .child(chrome::card(self.render_inspector(cx))),
                    );
                // 两种高度用不同的编号：切换布局时分隔条按新比例重新排，不沿用拖过的位置
                let id = if self.layout == LayoutMode::Audio {
                    "audio-split"
                } else {
                    "vertical-split"
                };
                v_resizable(id)
                    .child(resizable_panel().child(top))
                    .child(
                        resizable_panel()
                            .size(px((vh * timeline_h).max(220.)))
                            .child(self.timeline_card(cx)),
                    )
                    .into_any_element()
            }
            LayoutMode::MediaFirst => {
                // 素材在左边一整列（约 26% 宽）；右边上面播放器 + 属性，下面时间轴
                let upper = h_resizable("media-upper")
                    .child(resizable_panel().child(chrome::card(self.render_preview(cx))))
                    .child(
                        resizable_panel()
                            .size(px((vw * 0.22).max(280.)))
                            .child(chrome::card(self.render_inspector(cx))),
                    );
                let right = v_resizable("media-right")
                    .child(resizable_panel().child(upper))
                    .child(
                        resizable_panel()
                            .size(px((vh * 0.42).max(220.)))
                            .child(self.timeline_card(cx)),
                    );
                h_resizable("media-split")
                    .child(
                        resizable_panel()
                            .size(px((vw * 0.26).max(240.)))
                            .child(chrome::card(self.render_media_bin(cx))),
                    )
                    .child(resizable_panel().child(right))
                    .into_any_element()
            }
            LayoutMode::InspectorFirst => {
                // 属性在右边一整列（约 24% 宽）；左边上面素材 + 播放器，下面时间轴
                let upper = h_resizable("inspector-upper")
                    .child(
                        resizable_panel()
                            .size(px((vw * 0.30).max(240.)))
                            .child(chrome::card(self.render_media_bin(cx))),
                    )
                    .child(resizable_panel().child(chrome::card(self.render_preview(cx))));
                let left = v_resizable("inspector-left")
                    .child(resizable_panel().child(upper))
                    .child(
                        resizable_panel()
                            .size(px((vh * 0.42).max(220.)))
                            .child(self.timeline_card(cx)),
                    );
                h_resizable("inspector-split")
                    .child(resizable_panel().child(left))
                    .child(
                        resizable_panel()
                            .size(px((vw * 0.24).max(300.)))
                            .child(chrome::card(self.render_inspector(cx))),
                    )
                    .into_any_element()
            }
            LayoutMode::Portrait => {
                // 播放器在右边一整列（约 30% 宽）；左边上面素材 + 属性，下面时间轴
                let upper = h_resizable("portrait-upper")
                    .child(resizable_panel().child(chrome::card(self.render_media_bin(cx))))
                    .child(
                        resizable_panel()
                            .size(px((vw * 0.25).max(280.)))
                            .child(chrome::card(self.render_inspector(cx))),
                    );
                let left = v_resizable("portrait-left")
                    .child(resizable_panel().child(upper))
                    .child(
                        resizable_panel()
                            .size(px((vh * 0.45).max(220.)))
                            .child(self.timeline_card(cx)),
                    );
                h_resizable("portrait-split")
                    .child(resizable_panel().child(left))
                    .child(
                        resizable_panel()
                            .size(px((vw * 0.30).max(320.)))
                            .child(chrome::card(self.render_preview(cx))),
                    )
                    .into_any_element()
            }
        }
    }

    /// 顶栏的「布局 ▾」按钮和下拉菜单。
    pub(crate) fn render_layout_button(&self, cx: &Context<'_, Self>) -> AnyElement {
        div()
            .relative()
            .child(
                Button::new("tb-layout")
                    .ghost()
                    .compact()
                    .label("布局 ▾")
                    .tooltip("切换布局")
                    .on_click(cx.listener(|this, e: &gpui_kit::ClickEvent, _, cx| {
                        this.click_popup_button(Popup::Layout, e);
                        cx.notify();
                    })),
            )
            .children(self.popup_open(Popup::Layout).then(|| {
                let items = LayoutMode::ALL.map(|(id, label, mode)| {
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_layout(mode);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .w(px(12.))
                                .text_color(palette::accent())
                                .child(if self.layout == mode { "✓" } else { "" }),
                        )
                        .child(label)
                });
                deferred(
                    v_flex()
                        .id("layout-menu")
            // 盖住下面的东西：点菜单不会顺带点到下面的画面 / 时间轴
            .occlude()
                        .test_support()
                        .absolute()
                        .top(px(30.))
                        .right_0()
                        .w(px(150.))
                        .p_1()
                        .bg(palette::menu_bg())
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(px(chrome::CARD_RADIUS))
                        .shadow_lg()
                        .on_mouse_down_out(cx.listener(
                            |this, e: &gpui_kit::MouseDownEvent, _, cx| {
                                this.dismiss_popup(e.position);
                                cx.notify();
                            },
                        ))
                        .children(items),
                )
                .with_priority(1)
            }))
            .into_any_element()
    }
}
