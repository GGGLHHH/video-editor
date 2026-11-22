//! 属性面板「变速」标签（剪映）：二级分段「常规变速 / 曲线变速」。
//!
//! - 常规变速：倍数点选，素材用的那一段不变、时间轴上的长度跟着变；「声音变调」默认关着（音调不变）。
//! - 曲线变速：预设（自定义、蒙太奇、英雄时刻、子弹时间、跳接、闪进、闪出），卡片上画着速度曲线；
//!   选「自定义」后五个点能在曲线图上上下拖，也各有一个滑条（0.1x–10x，按对数均匀）。
//! - 常规变速的倍数也是滑条（同样按对数），数值框能直接输入倍数；时长能直接输入，改时长就是改倍速。

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
    to_slider_exact(speed).round()
}

/// 倍数 → 滑条值，不取整（输入的倍数、时长要准）。
pub(crate) fn to_slider_exact(speed: f32) -> f32 {
    speed.clamp(0.1, 10.0).log10() * 100.0
}

/// 滑条值 → 倍数。
pub(crate) fn from_slider(v: f32) -> f32 {
    10f32.powf(v / 100.0).clamp(0.1, 10.0)
}

/// 「变速」标签的控件。
#[derive(Debug)]
pub(crate) struct SpeedFields {
    /// 自定义曲线五个点的滑条。
    points: [Entity<SliderState>; 5],
    /// 常规变速的倍数滑条（对数刻度，同 `points`）。
    rate: Entity<SliderState>,
    gesture: bool,
    /// 自定义曲线图在窗口里的位置（上一帧布局的结果），拖点用。
    chart: gpui_kit::Bounds<gpui_kit::Pixels>,
    /// 正在曲线图上拖的点。
    curve_drag: Option<usize>,
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
        let mut subs: Vec<Subscription> = points
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
        let rate = cx.new(|_| {
            SliderState::new()
                .min(-100.0)
                .max(100.0)
                .step(0.5)
                .default_value(0.0)
        });
        subs.push(cx.subscribe_in(
            &rate,
            window,
            |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                this.on_speed_rate(e, window, cx);
            },
        ));
        (
            Self {
                points,
                rate,
                gesture: false,
                chart: gpui_kit::Bounds::default(),
                curve_drag: None,
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

/// 曲线图里倍数 `speed` 的高度（纵向按对数，中线是 1x，上下各留 2 像素）。
fn speed_y(oy: f32, h: f32, speed: f32) -> f32 {
    oy + h / 2.0 - speed.clamp(0.1, 10.0).log10() * (h / 2.0 - 2.0)
}

/// 曲线图里高度 `y` 是多少倍。
fn y_speed(oy: f32, h: f32, y: f32) -> f32 {
    let v = (oy + h / 2.0 - y) / (h / 2.0 - 2.0);
    10f32.powf(v.clamp(-1.0, 1.0))
}

/// 画速度曲线；`dots` 时把自定义的五个点画成小方块（能拖）。
#[expect(clippy::many_single_char_names, reason = "画图坐标")]
fn paint_curve(
    window: &mut Window,
    bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
    curve: Option<&SpeedCurve>,
    dots: bool,
) {
    let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    window.paint_quad(fill(bounds, palette::timeline_bg()));
    window.paint_quad(fill(
        gpui_kit::Bounds::new(point(px(ox), px(oy + h / 2.0)), size(px(w), px(1.0))),
        palette::divider(),
    ));
    let Some(curve) = curve else { return };
    let n = 40;
    for i in 0..n {
        #[expect(clippy::cast_precision_loss, reason = "画图")]
        let u = (i as f32 + 0.5) / n as f32;
        let y = speed_y(oy, h, curve.speed_at(u));
        #[expect(clippy::cast_precision_loss, reason = "画图")]
        let x = ox + w * i as f32 / n as f32;
        window.paint_quad(fill(
            gpui_kit::Bounds::new(point(px(x), px(y - 1.0)), size(px(w / 40.0 + 0.5), px(2.0))),
            palette::accent(),
        ));
    }
    if dots {
        for at in CUSTOM_AT {
            let (x, y) = (ox + w * at, speed_y(oy, h, curve.speed_at(at)));
            window.paint_quad(
                fill(
                    gpui_kit::Bounds::new(point(px(x - 4.0), px(y - 4.0)), size(px(8.0), px(8.0))),
                    gpui_kit::white(),
                )
                .corner_radii(px(4.0)),
            );
        }
    }
}

/// 小图：速度曲线（预设卡片上的）。
fn curve_chart(curve: Option<SpeedCurve>, w: f32, h: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| paint_curve(window, bounds, curve.as_ref(), false),
    )
    .w(px(w))
    .h(px(h))
}

/// 曲线图上离 `x` 最近、横向 14 像素以内的点。
fn point_near(chart: gpui_kit::Bounds<gpui_kit::Pixels>, x: f32) -> Option<usize> {
    let (ox, w) = (f32::from(chart.origin.x), f32::from(chart.size.width));
    CUSTOM_AT
        .iter()
        .enumerate()
        .map(|(i, at)| (i, (ox + w * at - x).abs()))
        .filter(|&(_, d)| d <= 14.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

impl Workspace {
    /// 常规变速的倍数滑条：拖动时一步撤销。
    fn on_speed_rate(&mut self, e: &SliderEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(clip) = self.inspector.bound else {
            return;
        };
        match e {
            SliderEvent::Change(v) => {
                let speed = from_slider(v.start());
                if !self.inspector.speed.gesture {
                    self.doc.begin_gesture();
                    self.inspector.speed.gesture = true;
                }
                let _ = self.doc.edit(|p| ve_core::edit::set_speed(p, clip, speed));
                self.after_change(window, cx);
            }
            SliderEvent::Release(_) => self.end_speed_gesture(),
        }
    }

    fn end_speed_gesture(&mut self) {
        if self.inspector.speed.gesture {
            self.doc.end_gesture();
            self.inspector.speed.gesture = false;
        }
    }

    /// 自定义曲线第 `i` 个点换成 `speed`，别的点照旧。
    fn set_curve_point(
        &mut self,
        i: usize,
        speed: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(clip) = self.inspector.bound else {
            return;
        };
        let Some(now) = self
            .doc
            .project
            .timeline
            .find_clip(clip)
            .and_then(|(_, c)| c.curve.clone())
        else {
            return;
        };
        let Some(curve) = SpeedCurve::new(
            CUSTOM_AT
                .iter()
                .enumerate()
                .map(|(k, &at)| SpeedPoint {
                    at,
                    speed: if k == i { speed } else { now.speed_at(at) },
                })
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

    fn speed_curve_mouse_down(
        &mut self,
        e: &gpui_kit::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let chart = self.inspector.speed.chart;
        let Some(i) = point_near(chart, f32::from(e.position.x)) else {
            return;
        };
        self.inspector.speed.curve_drag = Some(i);
        self.speed_curve_drag_to(e.position, window, cx);
    }

    fn speed_curve_drag_to(
        &mut self,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(i) = self.inspector.speed.curve_drag else {
            return;
        };
        let chart = self.inspector.speed.chart;
        let speed = y_speed(
            f32::from(chart.origin.y),
            f32::from(chart.size.height),
            f32::from(at.y),
        );
        self.set_curve_point(i, speed, window, cx);
    }

    fn speed_curve_mouse_up(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.inspector.speed.curve_drag.take().is_some() {
            self.end_speed_gesture();
            self.refresh_curve_sliders(window, cx);
        }
    }

    /// 自定义曲线的大图：五个点能上下拖。
    fn render_curve_editor(clip: &ve_core::Clip, cx: &Context<'_, Self>) -> AnyElement {
        let this = cx.entity();
        let curve = clip.curve.clone();
        div()
            .id("curve-editor")
            .test_support()
            .w(px(260.))
            .h(px(96.))
            .cursor_ns_resize()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        this.update(cx, |w, _| w.inspector.speed.chart = bounds);
                    },
                    move |bounds, (), window, _| {
                        paint_curve(window, bounds, curve.as_ref(), true);
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(Self::speed_curve_mouse_down),
            )
            .on_mouse_move(
                cx.listener(|this, e: &gpui_kit::MouseMoveEvent, window, cx| {
                    this.speed_curve_drag_to(e.position, window, cx);
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.speed_curve_mouse_up(window, cx)),
            )
            .on_mouse_up_out(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.speed_curve_mouse_up(window, cx)),
            )
            .into_any_element()
    }

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

    /// 倍数滑条、自定义曲线的滑条跟着片段走（换了片段、撤销之后）。
    pub(crate) fn refresh_curve_sliders(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.inspector.speed.gesture {
            return;
        }
        let Some((_, clip)) = self
            .inspector
            .bound
            .and_then(|c| self.doc.project.timeline.find_clip(c))
        else {
            return;
        };
        let rate = to_slider_exact(clip.speed);
        let s = &self.inspector.speed.rate;
        if (s.read(cx).value().start() - rate).abs() > 0.25 {
            s.update(cx, |s, cx| s.set_value(rate, window, cx));
        }
        let Some(curve) = clip.curve.clone() else {
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
        // 复合片段也能曲线变速（里面已经有曲线变速的片段时设不上，见 compound::curve_fits）
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
        let slider = &self.inspector.speed.rate;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "素材时长的秒数远小于 f32 精度上限"
        )]
        let span = clip.source_span().as_secs_f64() as f32;
        v_flex()
            .gap_3()
            .child(div().text_sm().text_color(muted).child("倍数"))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .id("speed-rate")
                            .test_support()
                            .flex_1()
                            .child(Slider::new(slider)),
                    )
                    .child(self.value_box_mapped(
                        "speed-rate",
                        slider,
                        format!("{:.2}x", clip.speed),
                        52.0,
                        crate::value_edit::ValueMap::Speed,
                        cx,
                    )),
            )
            .child(h_flex().flex_wrap().gap_1().children(chips))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .text_sm()
                    .child(div().text_color(muted).child("时长"))
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(self.value_box_mapped(
                                "speed-duration",
                                slider,
                                format!("{:.2}s", clip.duration.as_secs_f64()),
                                64.0,
                                crate::value_edit::ValueMap::Duration(span),
                                cx,
                            ))
                            .child(div().text_xs().text_color(muted).child(SharedString::from(
                                format!("素材 {}", format_timecode(clip.source_span(), rate)),
                            ))),
                    ),
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
            .child(Self::render_curve_editor(clip, cx))
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
