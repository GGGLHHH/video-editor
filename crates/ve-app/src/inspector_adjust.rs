//! 属性面板「调整」标签：亮度、对比度、饱和度、色温、色调五个滑条（-100–100，能打关键帧），
//! 再加高光、阴影、光感、锐化、颗粒、暗角（剪映「基础」里的，也能打关键帧）；
//! 拖一次滑条算一步撤销；「重置」回到不调色。

use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use ve_core::edit::{apply_adjust_at, set_adjust};
use ve_core::{AdjustExtra, AdjustItem, ColorAdjust, KeyGroup, Prop, Time};

/// 「调整」标签的控件。
#[derive(Debug)]
pub(crate) struct AdjustFields {
    sliders: [Entity<SliderState>; 5],
    /// 高光、阴影、光感、锐化、颗粒、暗角。
    extras: [Entity<SliderState>; 6],
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
        let mut subs: Vec<Subscription> = AdjustItem::ALL
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
        let extras = AdjustExtra::ALL.map(|x| {
            let (lo, hi) = x.range();
            cx.new(|_| {
                SliderState::new()
                    .min(lo * 100.0)
                    .max(hi * 100.0)
                    .step(1.0)
                    .default_value(0.0)
            })
        });
        subs.extend(AdjustExtra::ALL.iter().zip(&extras).map(|(&x, slider)| {
            cx.subscribe_in(
                slider,
                window,
                move |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                    this.on_adjust_extra(x, e, window, cx);
                },
            )
        }));
        (
            Self {
                sliders,
                extras,
                gesture: false,
            },
            subs,
        )
    }
}

/// 色温（冷 → 暖）、色调（绿 → 品红）滑条下面的渐变条（剪映的滑轨是渐变色）。
fn gradient_strip(item: AdjustItem) -> Option<gpui_kit::AnyElement> {
    let (id, from, to) = match item {
        AdjustItem::Temperature => ("adj-temperature-gradient", 0x3E_8EF7, 0xF7_A23E),
        AdjustItem::Tint => ("adj-tint-gradient", 0x46_C76A, 0xD9_4FC8),
        _ => return None,
    };
    Some(
        div()
            .id(id)
            .test_support()
            .mx(px(8.))
            .h(px(3.))
            .rounded(px(2.))
            .bg(gpui_kit::linear_gradient(
                90.,
                gpui_kit::linear_color_stop(gpui_kit::rgb(from), 0.),
                gpui_kit::linear_color_stop(gpui_kit::rgb(to), 1.),
            ))
            .into_any_element(),
    )
}

/// 滑条上的值（-100–100）。
fn slider_value(v: f32) -> f32 {
    (v * 100.0).round()
}

impl Workspace {
    /// 选中的视频片段（或调节片段）在播放头这一刻的调色，和播放头相对片段起点的时间。
    fn bound_adjust(&self) -> Option<(ve_core::ClipId, ColorAdjust, Time)> {
        let clip = self.inspector.bound?;
        let (track, c) = self.doc.project.timeline.find_clip(clip)?;
        let video = self.doc.project.timeline.track(track)?.kind == ve_core::TrackKind::Video;
        let offset = self.key_offset(c);
        video.then(|| (clip, c.adjust_at(offset), offset))
    }

    /// 有调色 / 音量关键帧时，滑条跟着播放头换成这一刻的值（`force` 时不管有没有关键帧都填）。
    /// 正在拖的滑条不动。
    pub(crate) fn refresh_animated_sliders(
        &self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(clip) = self.inspector.bound else {
            return;
        };
        let Some((_, c)) = self.doc.project.timeline.find_clip(clip) else {
            return;
        };
        let c = std::sync::Arc::clone(c);
        let offset = self.key_offset(&c);
        let colour_keys = Prop::ADJUST_ALL
            .iter()
            .any(|p| !c.animation.keys(*p).is_empty());
        if (force || colour_keys) && !self.inspector.adjust.gesture {
            let a = c.adjust_at(offset);
            let rows = AdjustItem::ALL
                .iter()
                .map(|item| item.get(&a))
                .zip(&self.inspector.adjust.sliders)
                .chain(
                    AdjustExtra::ALL
                        .iter()
                        .map(|x| x.get(&a))
                        .zip(&self.inspector.adjust.extras),
                );
            for (value, slider) in rows {
                let v = slider_value(value);
                if (slider.read(cx).value().start() - v).abs() > f32::EPSILON {
                    slider.update(cx, |s, cx| s.set_value(v, window, cx));
                }
            }
        }
        let volume_keys = !c.animation.keys(Prop::Volume).is_empty();
        if (force || volume_keys) && !self.inspector.gain_gesture {
            let db = c.gain_db_at(offset);
            let gain = &self.inspector.gain;
            if (gain.read(cx).value().start() - db).abs() > f32::EPSILON {
                gain.update(cx, |s, cx| s.set_value(db, window, cx));
            }
        }
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
        for (x, slider) in AdjustExtra::ALL.iter().zip(&self.inspector.adjust.extras) {
            let v = slider_value(x.get(a));
            slider.update(cx, |s, cx| s.set_value(v, window, cx));
        }
    }

    fn on_adjust_extra(
        &mut self,
        x: AdjustExtra,
        e: &SliderEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((clip, mut a, offset)) = self.bound_adjust() else {
            return;
        };
        match e {
            SliderEvent::Change(v) => {
                if !self.inspector.adjust.gesture {
                    self.doc.begin_gesture();
                    self.inspector.adjust.gesture = true;
                }
                x.set(&mut a, v.start() / 100.0);
                let _ = self.doc.edit(|p| apply_adjust_at(p, clip, offset, a));
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

    /// 高光、阴影、光感、锐化、颗粒、暗角六行（每行右边 ‹ ◇ ›）。
    fn render_adjust_extras(
        &self,
        current: &ColorAdjust,
        cx: &Context<'_, Self>,
    ) -> Vec<gpui_kit::AnyElement> {
        let muted = cx.theme().muted_foreground;
        AdjustExtra::ALL
            .iter()
            .zip(&self.inspector.adjust.extras)
            .map(|(&x, slider)| {
                let id = match x {
                    AdjustExtra::Highlights => "adj-highlights",
                    AdjustExtra::Shadows => "adj-shadows",
                    AdjustExtra::Light => "adj-light",
                    AdjustExtra::Sharpen => "adj-sharpen",
                    AdjustExtra::Grain => "adj-grain",
                    AdjustExtra::Vignette => "adj-vignette",
                };
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .w(px(52.))
                            .flex_none()
                            .text_sm()
                            .text_color(muted)
                            .child(x.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .id(id)
                            .test_support()
                            .child(Slider::new(slider)),
                    )
                    .child(self.value_box(
                        id,
                        slider,
                        format!("{}", slider_value(x.get(current))),
                        32.0,
                        cx,
                    ))
                    .child(self.render_group_keys(KeyGroup::Extra(x), cx))
                    .into_any_element()
            })
            .collect()
    }

    fn on_adjust(
        &mut self,
        item: AdjustItem,
        e: &SliderEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((clip, mut a, offset)) = self.bound_adjust() else {
            return;
        };
        match e {
            SliderEvent::Change(v) => {
                if !self.inspector.adjust.gesture {
                    self.doc.begin_gesture();
                    self.inspector.adjust.gesture = true;
                }
                item.set(&mut a, v.start() / 100.0);
                let _ = self.doc.edit(|p| apply_adjust_at(p, clip, offset, a));
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

    /// 「应用到全部」（剪映「应用全部」）：选中片段的调整和进阶调色套到所有视频片段上（一步撤销）。
    fn apply_adjust_to_all(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((clip, ..)) = self.bound_adjust() else {
            return;
        };
        let Some((_, c)) = self.doc.project.timeline.find_clip(clip) else {
            return;
        };
        let (adjust, grade) = (c.adjust, c.grade.as_deref().cloned());
        let targets: Vec<ve_core::ClipId> = self
            .doc
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == ve_core::TrackKind::Video)
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.asset().is_some() && c.id != clip)
            .map(|c| c.id)
            .collect();
        self.apply(window, cx, |p| {
            for &id in &targets {
                set_adjust(p, id, adjust)?;
                ve_core::edit::set_grade(p, id, grade.clone())?;
            }
            Ok(())
        });
    }

    fn reset_adjust(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((clip, ..)) = self.bound_adjust() else {
            return;
        };
        self.apply(window, cx, |p| set_adjust(p, clip, ColorAdjust::NONE));
        self.fill_adjust(&ColorAdjust::NONE, window, cx);
    }

    pub(crate) fn render_adjust_tab(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let current = self.bound_adjust().map_or(ColorAdjust::NONE, |(_, a, _)| a);
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
                        v_flex()
                            .flex_1()
                            .child(div().id(id).test_support().child(Slider::new(slider)))
                            .children(gradient_strip(item)),
                    )
                    .child(self.value_box(
                        id,
                        slider,
                        format!("{}", slider_value(item.get(&current))),
                        32.0,
                        cx,
                    ))
                    .child(self.render_group_keys(KeyGroup::Adjust(item), cx))
            });
        let rows: Vec<_> = rows.map(IntoElement::into_any_element).collect();
        let body = match self.inspector.grade.seg {
            crate::inspector_grade::AdjustSeg::Basic => v_flex()
                .gap_3()
                .child(
                    self.render_group(
                        crate::inspector_group::Group {
                            id: "adj",
                            title: "基础调节",
                            check: None,
                            reset: Some(Self::reset_adjust),
                            extra: Some(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        gpui_kit::component::button::Button::new("adj-apply-all")
                                            .ghost()
                                            .compact()
                                            .label("应用到全部")
                                            .tooltip("把这一段的调整套到所有视频片段上")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.apply_adjust_to_all(window, cx);
                                            })),
                                    )
                                    // 组标题上的 ‹ ◇ ›：「基础」11 项一起打帧（剪映）
                                    .child(self.render_group_keys(KeyGroup::AdjustAll, cx))
                                    .into_any_element(),
                            ),
                        },
                        v_flex()
                            .gap_3()
                            .children(rows)
                            .children(self.render_adjust_extras(&current, cx))
                            .into_any_element(),
                        cx,
                    ),
                )
                .children(self.render_lut_group(cx))
                .children(self.render_input_space(cx))
                .into_any_element(),
            crate::inspector_grade::AdjustSeg::Hsl => self
                .render_hsl_seg(cx)
                .unwrap_or_else(|| div().into_any_element()),
            crate::inspector_grade::AdjustSeg::Curves => self
                .render_curves_seg(cx)
                .unwrap_or_else(|| div().into_any_element()),
            crate::inspector_grade::AdjustSeg::Wheels => self
                .render_wheels_seg(cx)
                .unwrap_or_else(|| div().into_any_element()),
        };
        v_flex()
            .id("insp-adjust")
            .test_support()
            .gap_3()
            .child(self.render_adjust_segments(cx))
            .child(body)
    }
}

impl Workspace {
    /// 「色彩空间」组（HDR / Log 素材换成 Rec.709；只有视频素材片段有）。
    fn render_input_space(&self, cx: &Context<'_, Self>) -> Option<gpui_kit::AnyElement> {
        let (clip, _, _) = self.bound_video_clip()?;
        let (_, c) = self.doc.project.timeline.find_clip(clip)?;
        let current = c.input_space;
        let muted = cx.theme().muted_foreground;
        let option = |id: SharedString, label: &'static str, space| {
            crate::chrome::chip(id, label, current == space, cx).on_click(cx.listener(
                move |this, _, window, cx| {
                    this.apply(window, cx, |p| {
                        ve_core::edit::set_input_space(p, clip, space)
                    });
                },
            ))
        };
        let chips = std::iter::once(option("cs-none".into(), "Rec.709", None)).chain(
            ve_core::color_space::InputSpace::ALL
                .iter()
                .enumerate()
                .map(|(i, &s)| option(format!("cs-{i}").into(), s.label(), Some(s))),
        );
        let body = v_flex()
            .gap_2()
            .child(h_flex().flex_wrap().gap_1().children(chips))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("HDR（HLG / PQ）和相机 Log 素材选对应的一项，先换成 Rec.709 再调色"),
            )
            .into_any_element();
        Some(self.render_group(
            crate::inspector_group::Group {
                id: "cs",
                title: "色彩空间",
                check: None,
                reset: None,
                extra: None,
            },
            body,
            cx,
        ))
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
