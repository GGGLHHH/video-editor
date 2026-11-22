//! 画面特效片段的属性（剪映选中特效后的「特效」标签）：换特效种类、调强度和速度。

use crate::chrome;
use crate::workspace::Workspace;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use ve_core::{ClipId, Effect, EffectKind};

/// 特效标签的控件。
#[derive(Debug)]
pub(crate) struct EffectFields {
    /// 强度 0–100。
    amount: Entity<SliderState>,
    /// 速度 0–100（50 是原速）。
    speed: Entity<SliderState>,
    /// 正在拖强度滑条（整次拖动一步撤销）。
    gesture: bool,
}

impl EffectFields {
    pub(crate) fn new(
        window: &Window,
        cx: &mut Context<'_, Workspace>,
    ) -> (Self, Vec<Subscription>) {
        let amount = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(100.0)
                .step(1.0)
                .default_value(Effect::DEFAULT_AMOUNT * 100.0)
        });
        let speed = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(100.0)
                .step(1.0)
                .default_value(Effect::DEFAULT_SPEED * 100.0)
        });
        let subs = vec![
            cx.subscribe_in(
                &amount,
                window,
                |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                    this.on_effect_slider(e, |fx, v| fx.amount = v, window, cx);
                },
            ),
            cx.subscribe_in(
                &speed,
                window,
                |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                    this.on_effect_slider(e, |fx, v| fx.speed = v, window, cx);
                },
            ),
        ];
        (
            Self {
                amount,
                speed,
                gesture: false,
            },
            subs,
        )
    }
}

impl Workspace {
    /// 选中的特效片段和它在播放头这一刻的特效（强度有关键帧时按时间求值）。
    fn bound_effect(&self) -> Option<(ClipId, Effect)> {
        self.bound_effect_at().map(|(id, e, _)| (id, e))
    }

    /// 同 [`Self::bound_effect`]，再加播放头相对片段起点的时间。
    fn bound_effect_at(&self) -> Option<(ClipId, Effect, ve_core::Time)> {
        let id = self.inspector.bound?;
        let (_, c) = self.doc.project.timeline.find_clip(id)?;
        let offset = self.key_offset(c);
        Some((id, c.effect_at(offset)?, offset))
    }

    fn set_bound_effect(
        &mut self,
        f: impl FnOnce(&mut Effect),
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((clip, mut e, at)) = self.bound_effect_at() else {
            return;
        };
        f(&mut e);
        if self.inspector.effect.gesture {
            let _ = self
                .doc
                .edit(|p| ve_core::edit::apply_effect_at(p, clip, at, e));
            self.after_change(window, cx);
        } else {
            self.apply(window, cx, |p| {
                ve_core::edit::apply_effect_at(p, clip, at, e)
            });
        }
    }

    /// 强度 / 速度滑条（0–100）：`set` 把 0–1 的值写进特效。整次拖动一步撤销。
    fn on_effect_slider(
        &mut self,
        e: &SliderEvent,
        set: fn(&mut Effect, f32),
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match e {
            SliderEvent::Change(v) => {
                if !self.inspector.effect.gesture {
                    self.doc.begin_gesture();
                    self.inspector.effect.gesture = true;
                }
                let value = (v.start() / 100.0).clamp(0.0, 1.0);
                self.set_bound_effect(|fx| set(fx, value), window, cx);
            }
            SliderEvent::Release(_) => {
                if self.inspector.effect.gesture {
                    self.doc.end_gesture();
                    self.inspector.effect.gesture = false;
                }
            }
        }
    }

    /// 强度滑条跟着片段走（换了片段、撤销之后）；正在拖的时候不动。
    pub(crate) fn refresh_effect_slider(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.inspector.effect.gesture {
            return;
        }
        let Some((_, e)) = self.bound_effect() else {
            return;
        };
        for (value, s) in [
            (e.amount, &self.inspector.effect.amount),
            (e.speed, &self.inspector.effect.speed),
        ] {
            let ui = (value * 100.0).round();
            if (s.read(cx).value().start() - ui).abs() > 0.5 {
                s.update(cx, |s, cx| s.set_value(ui, window, cx));
            }
        }
    }

    /// 「特效」标签：特效种类一排、强度滑条。
    pub(crate) fn render_effect_tab(&self, cx: &Context<'_, Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let current = self.bound_effect().map(|(_, e)| e);
        let kinds = h_flex()
            .flex_wrap()
            .gap_1()
            .children(EffectKind::ALL.iter().enumerate().map(|(i, &k)| {
                let on = current.is_some_and(|e| e.kind == k);
                chrome::chip(format!("effect-kind-{i}"), k.label(), on, cx).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.set_bound_effect(|e| e.kind = k, window, cx);
                    },
                ))
            }));
        let amount = current.map_or(0.0, |e| e.amount);
        v_flex()
            .gap_3()
            .child(div().text_xs().text_color(muted).child("特效"))
            .child(kinds)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w_12().text_xs().child("强度"))
                    .child(
                        div()
                            .id("effect-amount")
                            .flex_1()
                            .child(Slider::new(&self.inspector.effect.amount)),
                    )
                    .child(self.value_box(
                        "effect-amount",
                        &self.inspector.effect.amount,
                        format!("{:.0}", amount * 100.0),
                        32.0,
                        cx,
                    ))
                    .child(self.render_group_keys(ve_core::KeyGroup::Effect, cx)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w_12().text_xs().child("速度"))
                    .child(
                        div()
                            .id("effect-speed")
                            .flex_1()
                            .child(Slider::new(&self.inspector.effect.speed)),
                    )
                    .child(self.value_box(
                        "effect-speed",
                        &self.inspector.effect.speed,
                        format!("{:.0}", current.map_or(0.0, |e| e.speed) * 100.0),
                        32.0,
                        cx,
                    )),
            )
            .children(self.render_easing_row(ve_core::KeyGroup::Effect.props(), "fx", cx))
    }
}
