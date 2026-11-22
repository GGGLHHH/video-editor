//! 「音频」标签里的声音处理：音频降噪（剪映）、均衡（低音 / 中音 / 高音）、压缩。
//! 降噪和压缩是可勾选的组：勾上按默认强度开，取消就关；均衡的 ↻ 三段归零。

use crate::inspector_group::Group;
use crate::workspace::Workspace;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use ve_core::audio_fx::{EQ_LABELS, EQ_RANGE_DB};
use ve_core::{AudioFx, ClipId};

/// 勾上降噪 / 压缩时的默认强度。
const DEFAULT_STRENGTH: f32 = 0.5;

/// 哪个滑条。
#[derive(Debug, Clone, Copy)]
enum FxSlider {
    Denoise,
    Eq(usize),
    Compress,
}

/// 声音处理的滑条。
#[derive(Debug)]
pub(crate) struct AudioFxFields {
    denoise: Entity<SliderState>,
    eq: [Entity<SliderState>; 3],
    compress: Entity<SliderState>,
    /// 正在拖滑条（整次拖动一步撤销）。
    gesture: bool,
}

fn slider(cx: &mut Context<'_, Workspace>, min: f32, max: f32, value: f32) -> Entity<SliderState> {
    cx.new(|_| {
        SliderState::new()
            .min(min)
            .max(max)
            .step(1.0)
            .default_value(value)
    })
}

impl AudioFxFields {
    pub(crate) fn new(
        window: &Window,
        cx: &mut Context<'_, Workspace>,
    ) -> (Self, Vec<Subscription>) {
        let denoise = slider(cx, 0.0, 100.0, DEFAULT_STRENGTH * 100.0);
        let eq = [0, 1, 2].map(|_| slider(cx, EQ_RANGE_DB.0, EQ_RANGE_DB.1, 0.0));
        let compress = slider(cx, 0.0, 100.0, DEFAULT_STRENGTH * 100.0);
        let mut subs = Vec::new();
        let mut sub =
            |s: &Entity<SliderState>, which: FxSlider, cx: &mut Context<'_, Workspace>| {
                subs.push(cx.subscribe_in(
                    s,
                    window,
                    move |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                        this.on_fx_slider(which, e, window, cx);
                    },
                ));
            };
        sub(&denoise, FxSlider::Denoise, cx);
        for (i, s) in eq.iter().enumerate() {
            sub(s, FxSlider::Eq(i), cx);
        }
        sub(&compress, FxSlider::Compress, cx);
        (
            Self {
                denoise,
                eq,
                compress,
                gesture: false,
            },
            subs,
        )
    }
}

impl Workspace {
    /// 选中的素材片段和它的声音处理。
    fn bound_audio_fx(&self) -> Option<(ClipId, AudioFx)> {
        let id = self.inspector.bound?;
        let (_, c) = self.doc.project.timeline.find_clip(id)?;
        c.asset()?;
        Some((id, c.audio_fx))
    }

    fn edit_audio_fx(
        &mut self,
        f: impl FnOnce(&mut AudioFx),
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((clip, mut fx)) = self.bound_audio_fx() else {
            return;
        };
        f(&mut fx);
        if self.inspector.audio_fx.gesture {
            let _ = self.doc.edit(|p| ve_core::edit::set_audio_fx(p, clip, fx));
            self.after_change(window, cx);
        } else {
            self.apply(window, cx, |p| ve_core::edit::set_audio_fx(p, clip, fx));
        }
    }

    fn on_fx_slider(
        &mut self,
        which: FxSlider,
        e: &SliderEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match e {
            SliderEvent::Change(v) => {
                if !self.inspector.audio_fx.gesture {
                    self.doc.begin_gesture();
                    self.inspector.audio_fx.gesture = true;
                }
                let v = v.start();
                self.edit_audio_fx(
                    |fx| match which {
                        FxSlider::Denoise => fx.denoise = (v / 100.0).clamp(0.0, 1.0),
                        FxSlider::Eq(i) => fx.eq[i] = v.clamp(EQ_RANGE_DB.0, EQ_RANGE_DB.1),
                        FxSlider::Compress => fx.compress = (v / 100.0).clamp(0.0, 1.0),
                    },
                    window,
                    cx,
                );
            }
            SliderEvent::Release(_) => {
                if self.inspector.audio_fx.gesture {
                    self.doc.end_gesture();
                    self.inspector.audio_fx.gesture = false;
                }
            }
        }
    }

    /// 滑条跟着片段走（换了片段、撤销之后）；正在拖的时候不动。关着的降噪 / 压缩显示默认强度。
    pub(crate) fn refresh_audio_fx_sliders(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.inspector.audio_fx.gesture {
            return;
        }
        let Some((_, fx)) = self.bound_audio_fx() else {
            return;
        };
        let f = &self.inspector.audio_fx;
        let mut set = |s: &Entity<SliderState>, ui: f32| {
            if (s.read(cx).value().start() - ui).abs() > 0.5 {
                s.update(cx, |s, cx| s.set_value(ui, window, cx));
            }
        };
        let strength = |v: f32| if v > 0.0 { v } else { DEFAULT_STRENGTH } * 100.0;
        set(&f.denoise, strength(fx.denoise).round());
        set(&f.compress, strength(fx.compress).round());
        for i in 0..3 {
            set(&f.eq[i], fx.eq[i].round());
        }
    }

    fn toggle_denoise(&mut self, on: bool, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.edit_audio_fx(
            |fx| fx.denoise = if on { DEFAULT_STRENGTH } else { 0.0 },
            window,
            cx,
        );
        self.refresh_audio_fx_sliders(window, cx);
    }

    fn toggle_compress(&mut self, on: bool, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.edit_audio_fx(
            |fx| fx.compress = if on { DEFAULT_STRENGTH } else { 0.0 },
            window,
            cx,
        );
        self.refresh_audio_fx_sliders(window, cx);
    }

    fn reset_eq(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.edit_audio_fx(|fx| fx.eq = [0.0; 3], window, cx);
        self.refresh_audio_fx_sliders(window, cx);
    }

    fn pick_voice(
        &mut self,
        voice: ve_core::Voice,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.edit_audio_fx(|fx| fx.voice = voice, window, cx);
    }

    /// 「变声」：一排预设，点一下换（都不改声音长度）。
    fn render_voice(&self, fx: AudioFx, cx: &Context<'_, Self>) -> AnyElement {
        let chips = ve_core::Voice::ALL.map(|v| {
            crate::chrome::chip(format!("fx-voice-{v:?}"), v.label(), fx.voice == v, cx)
                .on_click(cx.listener(move |w, _, window, cx| w.pick_voice(v, window, cx)))
                .into_any_element()
        });
        let body = h_flex()
            .gap_1()
            .flex_wrap()
            .children(chips)
            .into_any_element();
        self.render_group(
            Group {
                id: "fx-voice",
                title: "变声",
                check: None,
                reset: None,
                extra: None,
            },
            body,
            cx,
        )
    }

    /// 「音频降噪」「均衡」「压缩」「变声」四个组。
    pub(crate) fn render_audio_fx(&self, cx: &Context<'_, Self>) -> Vec<AnyElement> {
        let Some((_, fx)) = self.bound_audio_fx() else {
            return Vec::new();
        };
        let f = &self.inspector.audio_fx;
        let row =
            |id: &'static str, label: &'static str, s: &Entity<SliderState>, value: String| {
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w_10().text_xs().child(label))
                    .child(div().id(id).flex_1().child(Slider::new(s)))
                    .child(self.value_box(id, s, value, 48.0, cx))
            };
        let denoise = v_flex()
            .child(row(
                "fx-denoise",
                "强度",
                &f.denoise,
                format!("{:.0}", fx.denoise * 100.0),
            ))
            .into_any_element();
        let eq = v_flex()
            .gap_1()
            .children(
                ["fx-eq-0", "fx-eq-1", "fx-eq-2"]
                    .into_iter()
                    .enumerate()
                    .map(|(i, id)| row(id, EQ_LABELS[i], &f.eq[i], format!("{:+.0} dB", fx.eq[i]))),
            )
            .into_any_element();
        let compress = v_flex()
            .child(row(
                "fx-compress",
                "强度",
                &f.compress,
                format!("{:.0}", fx.compress * 100.0),
            ))
            .into_any_element();
        vec![
            self.render_group(
                Group {
                    id: "fx-dn",
                    title: "音频降噪",
                    check: Some((fx.denoise > 0.0, Self::toggle_denoise)),
                    reset: None,
                    extra: None,
                },
                denoise,
                cx,
            ),
            self.render_group(
                Group {
                    id: "fx-eq",
                    title: "均衡",
                    check: None,
                    reset: Some(Self::reset_eq),
                    extra: None,
                },
                eq,
                cx,
            ),
            self.render_group(
                Group {
                    id: "fx-cp",
                    title: "压缩",
                    check: Some((fx.compress > 0.0, Self::toggle_compress)),
                    reset: None,
                    extra: None,
                },
                compress,
                cx,
            ),
            self.render_voice(fx, cx),
        ]
    }
}
