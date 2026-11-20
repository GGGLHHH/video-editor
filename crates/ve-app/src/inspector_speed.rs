//! 属性面板「变速」标签（剪映）：二级分段「常规变速 / 曲线变速」。
//!
//! - 常规变速：倍数点选，素材用的那一段不变、时间轴上的长度跟着变；「声音变调」默认关着（音调不变）。
//! - 曲线变速：预设（自定义、蒙太奇、英雄时刻、子弹时间、跳接、闪进、闪出），卡片上画着速度曲线；
//!   选「自定义」后五个点的倍数各一个滑条（0.1x–10x，按对数均匀）。

use crate::chrome;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, canvas, div, fill, point, px, size,
};
use ve_core::{CurvePreset, SpeedCurve, SpeedPoint, format_timecode};

/// 倍数选项：（编号，名字，倍数）。
const SPEEDS: [(&str, &str, f32); 8] = [
    ("0-25", "0.25x", 0.25),
    ("0-5", "0.5x", 0.5),
    ("0-75", "0.75x", 0.75),
    ("1", "1x", 1.0),
    ("1-5", "1.5x", 1.5),
    ("2", "2x", 2.0),
    ("3", "3x", 3.0),
    ("5", "5x", 5.0),
];

/// 自定义曲线五个点的位置。
const CUSTOM_AT: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// 倍数 → 滑条值（-100–100：0.1x–10x 按对数均匀）。
fn to_slider(speed: f32) -> f32 {
    (speed.clamp(0.1, 10.0).log10() * 100.0).round()
}

/// 滑条值 → 倍数。
fn from_slider(v: f32) -> f32 {
    10f32.powf(v / 100.0).clamp(0.1, 10.0)
}

/// 「变速」标签的控件。
#[derive(Debug)]
pub(crate) struct SpeedFields {
    /// 自定义曲线五个点的滑条。
    points: [Entity<SliderState>; 5],
    gesture: bool,
    /// 在看「曲线变速」（否则「常规变速」）。
    pub(crate) curve_seg: bool,
}

impl SpeedFields {
    pub(crate) fn new(
        window: &Window,
        cx: &mut Context<'_, Workspace>,
    ) -> (Self, Vec<Subscription>) {
        let points = CUSTOM_AT.map(|_| {
            cx.new(|_| {
                SliderState::new()
                    .min(-100.0)
                    .max(100.0)
                    .step(1.0)
                    .default_value(0.0)
            })
        });
        let subs = points
            .iter()
            .enumerate()
            .map(|(i, s)| {
                cx.subscribe_in(
                    s,
                    window,
                    move |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                        this.on_curve_point(i, e, window, cx);
                    },
                )
            })
            .collect();
        (
            Self {
                points,
                gesture: false,
                curve_seg: false,
            },
            subs,
        )
    }
}

/// 一条曲线是不是某个预设（点都一样）。
fn preset_of(curve: &SpeedCurve) -> Option<CurvePreset> {
    CurvePreset::ALL
        .into_iter()
        .filter(|p| *p != CurvePreset::Custom)
        .find(|p| p.curve() == *curve)
}

/// 小图：速度曲线（纵向按对数，中线是 1x）。
#[expect(clippy::many_single_char_names, reason = "画图坐标")]
fn curve_chart(curve: Option<SpeedCurve>, w: f32, h: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            window.paint_quad(fill(bounds, palette::timeline_bg()));
            window.paint_quad(fill(
                gpui_kit::Bounds::new(point(px(ox), px(oy + h / 2.0)), size(px(w), px(1.0))),
                palette::divider(),
            ));
            let Some(curve) = &curve else { return };
            let n = 40;
            for i in 0..n {
                #[expect(clippy::cast_precision_loss, reason = "画图")]
                let u = (i as f32 + 0.5) / n as f32;
                let v = curve.speed_at(u).log10(); // -1–1
                let y = oy + h / 2.0 - v * (h / 2.0 - 2.0);
                #[expect(clippy::cast_precision_loss, reason = "画图")]
                let x = ox + w * i as f32 / n as f32;
                window.paint_quad(fill(
                    gpui_kit::Bounds::new(
                        point(px(x), px(y - 1.0)),
                        size(px(w / 40.0 + 0.5), px(2.0)),
                    ),
                    palette::accent(),
                ));
            }
        },
    )
    .w(px(w))
    .h(px(h))
}

impl Workspace {
    fn on_curve_point(
        &mut self,
        i: usize,
        e: &SliderEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(clip) = self.inspector.bound else {
            return;
        };
        match e {
            SliderEvent::Change(v) => {
                let speeds: Vec<f32> = self
                    .inspector
                    .speed
                    .points
                    .iter()
                    .enumerate()
                    .map(|(k, s)| {
                        if k == i {
                            from_slider(v.start())
                        } else {
                            from_slider(s.read(cx).value().start())
                        }
                    })
                    .collect();
                let Some(curve) = SpeedCurve::new(
                    CUSTOM_AT
                        .iter()
                        .zip(speeds)
                        .map(|(&at, speed)| SpeedPoint { at, speed })
                        .collect(),
                ) else {
                    return;
                };
                if !self.inspector.speed.gesture {
                    self.doc.begin_gesture();
                    self.inspector.speed.gesture = true;
                }
                let _ = self
                    .doc
                    .edit(|p| ve_core::edit::set_speed_curve(p, clip, Some(curve)));
                self.after_change(window, cx);
            }
            SliderEvent::Release(_) => {
                if self.inspector.speed.gesture {
                    self.doc.end_gesture();
                    self.inspector.speed.gesture = false;
                }
            }
        }
    }

    /// 自定义曲线的滑条跟着片段的曲线走（换了片段、撤销之后）。
    pub(crate) fn refresh_curve_sliders(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.inspector.speed.gesture {
            return;
        }
        let Some(curve) = self
            .inspector
            .bound
            .and_then(|c| self.doc.project.timeline.find_clip(c))
            .and_then(|(_, c)| c.curve.clone())
        else {
            return;
        };
        for (at, s) in CUSTOM_AT.iter().zip(&self.inspector.speed.points) {
            let v = to_slider(curve.speed_at(*at));
            if (s.read(cx).value().start() - v).abs() > 0.5 {
                s.update(cx, |s, cx| s.set_value(v, window, cx));
            }
        }
    }

    fn render_speed_segments(&self, cx: &Context<'_, Self>) -> AnyElement {
        let curve = self.inspector.speed.curve_seg;
        h_flex()
            .gap_1()
            .child(
                chrome::chip("speed-seg-normal", "常规变速", !curve, cx).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.inspector.speed.curve_seg = false;
                        cx.notify();
                    },
                )),
            )
            .child(
                chrome::chip("speed-seg-curve", "曲线变速", curve, cx).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.inspector.speed.curve_seg = true;
                        cx.notify();
                    },
                )),
            )
            .into_any_element()
    }

    pub(crate) fn render_speed_tab(
        &self,
        clip: &ve_core::Clip,
        cx: &Context<'_, Self>,
    ) -> impl IntoElement {
        let body = if self.inspector.speed.curve_seg {
            self.render_curve_speed(clip, cx)
        } else {
            self.render_normal_speed(clip, cx)
        };
        v_flex()
            .id("insp-speed")
            .test_support()
            .gap_3()
            .child(self.render_speed_segments(cx))
            .child(body)
            .children(self.render_frame_blend(clip, cx))
    }

    /// 智能补帧（剪映）：帧融合开关。只有视频片段有；慢放时才起作用。
    fn render_frame_blend(
        &self,
        clip: &ve_core::Clip,
        cx: &Context<'_, Self>,
    ) -> Option<AnyElement> {
        self.bound_video_clip()?;
        let muted = cx.theme().muted_foreground;
        let id = clip.id;
        let on = clip.frame_blend;
        let toggle = chrome::chip("speed-frame-blend", "帧融合", on, cx).on_click(cx.listener(
            move |this, _, window, cx| {
                this.apply(window, cx, |p| ve_core::edit::set_frame_blend(p, id, !on));
            },
        ));
        Some(
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .justify_between()
                        .text_sm()
                        .child(div().text_color(muted).child("智能补帧"))
                        .child(toggle),
                )
                .child(div().text_xs().text_color(muted).child(
                    "慢放时把前后两帧按时间混合，画面不再一顿一顿的（倍数小于 1 或曲线变速时生效）",
                ))
                .into_any_element(),
        )
    }

    fn render_normal_speed(&self, clip: &ve_core::Clip, cx: &Context<'_, Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let rate = self.doc.project.settings.frame_rate;
        let id = clip.id;
        let chips = SPEEDS.map(|(slug, label, s)| {
            let on = clip.curve.is_none() && (clip.speed - s).abs() < 1e-4;
            chrome::chip(format!("speed-{slug}"), label, on, cx).on_click(cx.listener(
                move |this, _, window, cx| {
                    this.apply(window, cx, |p| ve_core::edit::set_speed(p, id, s));
                },
            ))
        });
        let shift = !clip.keep_pitch;
        let pitch = chrome::chip("speed-pitch", "声音变调", shift, cx).on_click(cx.listener(
            move |this, _, window, cx| {
                this.apply(window, cx, |p| ve_core::edit::set_keep_pitch(p, id, shift));
            },
        ));
        v_flex()
            .gap_3()
            .child(div().text_sm().text_color(muted).child("倍数"))
            .child(h_flex().flex_wrap().gap_1().children(chips))
            .child(
                h_flex()
                    .justify_between()
                    .text_sm()
                    .child(div().text_color(muted).child("时长"))
                    .child(SharedString::from(format!(
                        "{}（素材 {}）",
                        format_timecode(clip.duration, rate),
                        format_timecode(clip.source_span(), rate)
                    ))),
            )
            .child(
                h_flex()
                    .justify_between()
                    .text_sm()
                    .child(div().text_color(muted).child("声音"))
                    .child(pitch),
            )
            .child(div().text_xs().text_color(muted).child(
                "「声音变调」关着时音调不变，打开后音调跟着快慢变；变长时撞到后一段：主轨磁吸开着会把后面整体往后推",
            ))
            .into_any_element()
    }

    /// 自定义曲线：大一点的曲线图和五个点的滑条。
    fn render_custom_points(&self, clip: &ve_core::Clip, cx: &Context<'_, Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let rows = self
            .inspector
            .speed
            .points
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let speed = clip
                    .curve
                    .as_ref()
                    .map_or(1.0, |c| c.speed_at(CUSTOM_AT[i]));
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .w(px(64.))
                            .flex_none()
                            .text_sm()
                            .text_color(muted)
                            .child(SharedString::from(format!("第 {} 点", i + 1))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .id(SharedString::from(format!("curve-point-{i}")))
                            .test_support()
                            .child(Slider::new(s)),
                    )
                    .child(
                        div()
                            .w(px(44.))
                            .text_sm()
                            .child(SharedString::from(format!("{speed:.1}x"))),
                    )
            });
        v_flex()
            .gap_2()
            .child(curve_chart(clip.curve.clone(), 260.0, 48.0))
            .children(rows)
            .into_any_element()
    }

    fn render_curve_speed(&self, clip: &ve_core::Clip, cx: &Context<'_, Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let rate = self.doc.project.settings.frame_rate;
        let id = clip.id;
        let current = clip
            .curve
            .as_ref()
            .map(|c| preset_of(c).unwrap_or(CurvePreset::Custom));
        let card = |preset: Option<CurvePreset>| {
            let on = current == preset;
            let label = preset.map_or("无", CurvePreset::label);
            let slug =
                preset.map_or_else(|| "none".to_owned(), |p| format!("{p:?}").to_lowercase());
            v_flex()
                .id(SharedString::from(format!("curve-{slug}")))
                .test_support()
                .w(px(78.))
                .p_1()
                .gap_1()
                .rounded(px(4.))
                .border_1()
                .border_color(if on {
                    palette::accent()
                } else {
                    cx.theme().border
                })
                .cursor_pointer()
                .child(curve_chart(preset.map(CurvePreset::curve), 68.0, 28.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(if on {
                            palette::accent()
                        } else {
                            cx.theme().foreground
                        })
                        .child(label),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.apply(window, cx, |p| {
                        ve_core::edit::set_speed_curve(p, id, preset.map(CurvePreset::curve))
                    });
                }))
        };
        let cards = std::iter::once(None)
            .chain(CurvePreset::ALL.map(Some))
            .map(card);
        let custom =
            (current == Some(CurvePreset::Custom)).then(|| self.render_custom_points(clip, cx));
        v_flex()
            .gap_3()
            .child(h_flex().flex_wrap().gap_1().children(cards))
            .children(custom)
            .child(
                h_flex()
                    .justify_between()
                    .text_sm()
                    .child(div().text_color(muted).child("时长"))
                    .child(SharedString::from(format!(
                        "{}（素材 {}，平均 {:.2}x）",
                        format_timecode(clip.duration, rate),
                        format_timecode(clip.source_span(), rate),
                        clip.speed
                    ))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_is_logarithmic() {
        assert_eq!(to_slider(1.0), 0.0);
        assert_eq!(to_slider(10.0), 100.0);
        assert_eq!(to_slider(0.1), -100.0);
        assert!((from_slider(to_slider(2.0)) - 2.0).abs() < 0.05);
        assert_eq!(
            preset_of(&CurvePreset::Hero.curve()),
            Some(CurvePreset::Hero)
        );
        assert_eq!(preset_of(&CurvePreset::Custom.curve()), None);
    }
}
