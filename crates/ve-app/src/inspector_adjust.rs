//! 属性面板「调整」标签：亮度、对比度、饱和度、色温、色调五个滑条（-100–100），
//! 拖一次滑条算一步撤销；「重置」回到不调色。

use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, Subscription, Window, div, px,
};
use ve_core::edit::{apply_adjust_at, set_adjust};
use ve_core::{AdjustItem, ColorAdjust, KeyGroup, Prop, Time};

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
    /// 选中的视频片段（或调节片段）在播放头这一刻的调色，和播放头相对片段起点的时间。
    fn bound_adjust(&self) -> Option<(ve_core::ClipId, ColorAdjust, Time)> {
        let clip = self.inspector.bound?;
        let (track, c) = self.doc.project.timeline.find_clip(clip)?;
        let video = self.doc.project.timeline.track(track)?.kind == ve_core::TrackKind::Video;
        let offset = (self.playhead - c.start).clamp(Time::ZERO, c.duration);
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
        let offset = (self.playhead - c.start).clamp(Time::ZERO, c.duration);
        let colour_keys = Prop::ADJUST
            .iter()
            .any(|p| !c.animation.keys(*p).is_empty());
        if (force || colour_keys) && !self.inspector.adjust.gesture {
            let a = c.adjust_at(offset);
            for (item, slider) in AdjustItem::ALL.iter().zip(&self.inspector.adjust.sliders) {
                let v = slider_value(item.get(&a));
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
                    .child(self.render_group_keys(KeyGroup::Adjust(item), cx))
            });
        let rows: Vec<_> = rows.map(IntoElement::into_any_element).collect();
        let body = match self.inspector.grade.seg {
            crate::inspector_grade::AdjustSeg::Basic => v_flex()
                .gap_3()
                .child(self.render_group(
                    crate::inspector_group::Group {
                        id: "adj",
                        title: "基础调节",
                        check: None,
                        reset: Some(Self::reset_adjust),
                        extra: None,
                    },
                    v_flex().gap_3().children(rows).into_any_element(),
                    cx,
                ))
                .children(self.render_lut_group(cx))
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
