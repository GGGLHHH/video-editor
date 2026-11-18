//! 属性面板「调整」标签：亮度、对比度、饱和度、色温、色调五个滑条（-100–100），
//! 拖一次滑条算一步撤销；「重置」回到不调色。

use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, Subscription, Window, div, px,
};
use ve_core::edit::set_adjust;
use ve_core::{AdjustItem, ColorAdjust};

/// 「调整」标签的控件。
#[derive(Debug)]
pub(crate) struct AdjustFields {
    sliders: [Entity<SliderState>; 5],
    gesture: bool,
}

impl AdjustFields {
    /// 创建五个滑条并订阅。
    pub(crate) fn new(
        window: &Window,
        cx: &mut Context<'_, Workspace>,
    ) -> (Self, Vec<Subscription>) {
        let sliders = AdjustItem::ALL.map(|_| {
            cx.new(|_| {
                SliderState::new()
                    .min(-100.0)
                    .max(100.0)
                    .step(1.0)
                    .default_value(0.0)
            })
        });
        let subs = AdjustItem::ALL
            .iter()
            .zip(&sliders)
            .map(|(&item, slider)| {
                cx.subscribe_in(
                    slider,
                    window,
                    move |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                        this.on_adjust(item, e, window, cx);
                    },
                )
            })
            .collect();
        (
            Self {
                sliders,
                gesture: false,
            },
            subs,
        )
    }
}

/// 滑条上的值（-100–100）。
fn slider_value(v: f32) -> f32 {
    (v * 100.0).round()
}

impl Workspace {
    /// 选中的视频片段现在的调色。
    fn bound_adjust(&self) -> Option<(ve_core::ClipId, ColorAdjust)> {
        let (clip, ..) = self.bound_video_clip()?;
        let (_, c) = self.doc.project.timeline.find_clip(clip)?;
        Some((clip, c.adjust))
    }

    /// 把片段的调色填进滑条。
    pub(crate) fn fill_adjust(
        &self,
        a: &ColorAdjust,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        for (item, slider) in AdjustItem::ALL.iter().zip(&self.inspector.adjust.sliders) {
            let v = slider_value(item.get(a));
            slider.update(cx, |s, cx| s.set_value(v, window, cx));
        }
    }

    fn on_adjust(
        &mut self,
        item: AdjustItem,
        e: &SliderEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((clip, mut a)) = self.bound_adjust() else {
            return;
        };
        match e {
            SliderEvent::Change(v) => {
                if !self.inspector.adjust.gesture {
                    self.doc.begin_gesture();
                    self.inspector.adjust.gesture = true;
                }
                item.set(&mut a, v.start() / 100.0);
                let _ = self.doc.edit(|p| set_adjust(p, clip, a));
                self.after_change(window, cx);
            }
            SliderEvent::Release(_) => {
                if self.inspector.adjust.gesture {
                    self.doc.end_gesture();
                    self.inspector.adjust.gesture = false;
                }
            }
        }
    }

    fn reset_adjust(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((clip, _)) = self.bound_adjust() else {
            return;
        };
        self.apply(window, cx, |p| set_adjust(p, clip, ColorAdjust::NONE));
        self.fill_adjust(&ColorAdjust::NONE, window, cx);
    }

    pub(crate) fn render_adjust_tab(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let current = self.bound_adjust().map_or(ColorAdjust::NONE, |(_, a)| a);
        let rows = AdjustItem::ALL
            .iter()
            .zip(&self.inspector.adjust.sliders)
            .map(|(&item, slider)| {
                let id = match item {
                    AdjustItem::Brightness => "adj-brightness",
                    AdjustItem::Contrast => "adj-contrast",
                    AdjustItem::Saturation => "adj-saturation",
                    AdjustItem::Temperature => "adj-temperature",
                    AdjustItem::Tint => "adj-tint",
                };
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .w(px(52.))
                            .flex_none()
                            .text_sm()
                            .text_color(muted)
                            .child(item.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .id(id)
                            .test_support()
                            .child(Slider::new(slider)),
                    )
                    .child(div().w(px(32.)).text_sm().child(SharedString::from(format!(
                        "{}",
                        slider_value(item.get(&current))
                    ))))
            });
        v_flex()
            .id("insp-adjust")
            .test_support()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .child(div().text_sm().child("基础调节"))
                    .child(
                        Button::new("adj-reset")
                            .ghost()
                            .compact()
                            .label("重置")
                            .tooltip("回到不调色")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.reset_adjust(window, cx)),
                            ),
                    ),
            )
            .children(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_values_are_whole_percent() {
        assert_eq!(slider_value(0.254), 25.0);
        assert_eq!(slider_value(-1.0), -100.0);
        assert_eq!(slider_value(0.0), 0.0);
    }
}
