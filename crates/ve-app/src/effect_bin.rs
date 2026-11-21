//! 素材面板的「特效」标签（剪映「特效 › 画面特效」的本地版）：点一个特效，在播放头处加一段
//! 特效片段，作用于它下面所有轨道的画面；选中后在属性面板「特效」里换种类、调强度。

use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use ve_core::{Effect, EffectKind};

/// 新加的特效片段多长（和调节一样）。
const EFFECT_LENGTH: ve_core::Time = ve_core::edit::ADJUSTMENT_LENGTH;

impl Workspace {
    /// 在播放头处加一段特效 `kind`，选中它、属性面板翻到「特效」。
    pub fn add_effect_at_playhead(
        &mut self,
        kind: EffectKind,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let at = self.playhead_frame();
        if let Some(clip) = self.apply(window, cx, |p| {
            ve_core::edit::add_effect(p, at, EFFECT_LENGTH, Effect::new(kind))
        }) {
            self.doc.select(clip, false);
            self.inspector.tab = crate::inspector::InspectorTab::Effect;
            self.after_change(window, cx);
        }
    }

    /// 「特效」标签：每种特效一张卡片，下面是插件。
    pub(crate) fn render_effect_bin_tab(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let tiles = EffectKind::ALL.iter().enumerate().map(|(i, &k)| {
            let card = v_flex()
                .id(SharedString::from(format!("bin-effect-{i}")))
                .test_support();
            crate::timeline::drop::draggable(
                card,
                crate::timeline::drop::BinItem::Effect(Effect::new(k)),
                k.label(),
            )
            .group("effect-tile")
            .w(px(88.))
            .gap_1()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.add_effect_at_playhead(k, window, cx);
            }))
            .child(
                h_flex()
                    .w(px(88.))
                    .h(px(50.))
                    .justify_center()
                    .rounded(px(4.))
                    .bg(cx.theme().muted)
                    .border_2()
                    .border_color(gpui_kit::transparent_black())
                    .group_hover("effect-tile", |s| s.border_color(palette::accent()))
                    .child(
                        Icon::new(Lucide::Sparkles)
                            .size(px(20.))
                            .text_color(palette::effect_clip()),
                    ),
            )
            .child(div().text_xs().text_color(muted).child(k.label()))
        });
        v_flex()
            .id("bin-effects")
            .test_support()
            .p_2()
            .gap_2()
            .child(div().text_xs().text_color(muted).child(
                "点一个特效，在播放头处加一段：作用于它下面所有轨道的画面，在属性面板「特效」里换种类、调强度",
            ))
            .child(h_flex().flex_wrap().gap_2().children(tiles))
            .child(self.render_plugin_tiles(cx))
    }

    /// 插件：每个一张卡片；「重新载入」重读插件目录。
    fn render_plugin_tiles(&self, cx: &Context<'_, Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let tiles = self.plugins.iter().enumerate().map(|(i, name)| {
            let name = name.clone();
            let label = SharedString::from(name.clone());
            let card = v_flex()
                .id(SharedString::from(format!("bin-plugin-{i}")))
                .test_support();
            let card = match Effect::plugin(&name) {
                Some(effect) => crate::timeline::drop::draggable(
                    card,
                    crate::timeline::drop::BinItem::Effect(effect),
                    label.clone(),
                ),
                None => card,
            };
            card.group("plugin-tile")
                .w(px(88.))
                .gap_1()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.add_plugin_effect_at_playhead(&name, window, cx);
                }))
                .child(
                    h_flex()
                        .w(px(88.))
                        .h(px(50.))
                        .justify_center()
                        .rounded(px(4.))
                        .bg(cx.theme().muted)
                        .border_2()
                        .border_color(gpui_kit::transparent_black())
                        .group_hover("plugin-tile", |s| s.border_color(palette::accent()))
                        .child(Icon::new(Lucide::Puzzle).size(px(20.)).text_color(muted)),
                )
                .child(div().text_xs().text_color(muted).truncate().child(label))
        });
        v_flex()
            .gap_2()
            .pt_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().text_xs().text_color(muted).child("插件"))
                    .child(
                        Button::new("bin-plugin-reload")
                            .ghost()
                            .xsmall()
                            .label("重新载入")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.reload_plugins(window, cx);
                            })),
                    ),
            )
            .child(h_flex().flex_wrap().gap_2().children(tiles))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(format!(
                        "把 .wgsl 特效放进 {}，点「重新载入」",
                        self.plugin_dir.display()
                    ))),
            )
    }
}
