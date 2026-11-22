//! 属性面板「动画」标签：视频片段的入场 / 出场 / 组合动画（剪映）——卡片点一下套用（选中的描青边），
//! 选「无」去掉，选中后能调时长；动画叠加在关键帧上，不会打关键帧。字幕的入场 / 出场动画（渐显、
//! 滑入、放大……）在下面另一套。

use crate::chrome;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Subscription, div, px,
};
use gpui_kit::{SharedString, Window};
use ve_core::motion::{ClipMotion, Combo, Motion, MotionClip, Phase};
use ve_core::{ClipId, TextIn, TextMotion, TextOut, Time, TrackKind};

/// 新套用的动画默认多长（秒）。
const DEFAULT_SECS: f32 = 0.5;

/// 「动画」标签在看哪一组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AnimSeg {
    /// 入场。
    #[default]
    In,
    /// 出场。
    Out,
    /// 组合。
    Combo,
}

/// 「动画」标签的控件。
#[derive(Debug)]
pub(crate) struct MotionFields {
    /// 在看哪一组。
    pub(crate) seg: AnimSeg,
    /// 时长滑条（秒）。
    dur: Entity<SliderState>,
    gesture: bool,
}

impl MotionFields {
    pub(crate) fn new(
        window: &Window,
        cx: &mut Context<'_, Workspace>,
    ) -> (Self, Vec<Subscription>) {
        let dur = cx.new(|_| {
            SliderState::new()
                .min(0.1)
                .max(5.0)
                .step(0.1)
                .default_value(DEFAULT_SECS)
        });
        let sub = cx.subscribe_in(
            &dur,
            window,
            |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                this.on_motion_duration(e, window, cx);
            },
        );
        (
            Self {
                seg: AnimSeg::default(),
                dur,
                gesture: false,
            },
            vec![sub],
        )
    }
}

/// 预设的名字和元素编号后缀。
const fn naming(phase: Phase, m: Motion) -> (&'static str, &'static str) {
    match (phase, m) {
        (Phase::In, Motion::Fade) => ("渐显", "fade"),
        (Phase::Out, Motion::Fade) => ("渐隐", "fade"),
        (_, Motion::ZoomIn) => ("放大", "zoom-in"),
        (_, Motion::ZoomOut) => ("缩小", "zoom-out"),
        (Phase::In, Motion::SlideLeft) => ("左侧滑入", "left"),
        (Phase::Out, Motion::SlideLeft) => ("向左滑出", "left"),
        (Phase::In, Motion::SlideRight) => ("右侧滑入", "right"),
        (Phase::Out, Motion::SlideRight) => ("向右滑出", "right"),
        (Phase::In, Motion::SlideUp) => ("上方滑入", "up"),
        (Phase::Out, Motion::SlideUp) => ("向上滑出", "up"),
        (Phase::In, Motion::SlideDown) => ("下方滑入", "down"),
        (Phase::Out, Motion::SlideDown) => ("向下滑出", "down"),
    }
}

/// 卡片上画的小图标：动的方向。
const fn glyph(phase: Phase, m: Motion) -> &'static str {
    match (phase, m) {
        (_, Motion::Fade) => "◐",
        (_, Motion::ZoomIn) => "⤢",
        (_, Motion::ZoomOut) => "⤡",
        (Phase::In, Motion::SlideLeft) | (Phase::Out, Motion::SlideRight) => "→",
        (Phase::In, Motion::SlideRight) | (Phase::Out, Motion::SlideLeft) => "←",
        (Phase::In, Motion::SlideUp) | (Phase::Out, Motion::SlideDown) => "↓",
        (Phase::In, Motion::SlideDown) | (Phase::Out, Motion::SlideUp) => "↑",
    }
}

const fn combo_glyph(c: Combo) -> &'static str {
    match c {
        Combo::Spin => "↻",
        Combo::Swing => "⇋",
        Combo::Pulse => "◎",
    }
}

const fn combo_slug(c: Combo) -> &'static str {
    match c {
        Combo::Spin => "spin",
        Combo::Swing => "swing",
        Combo::Pulse => "pulse",
    }
}

/// 动画卡片：上面一块深色缩略图（图标），下面名字；选中的描青边。
fn card(
    id: String,
    icon: &'static str,
    label: &'static str,
    on: bool,
    cx: &gpui_kit::App,
) -> gpui_kit::base::ObservedElement<gpui_kit::Stateful<gpui_kit::Div>> {
    let fg = if on {
        palette::accent()
    } else {
        cx.theme().foreground
    };
    v_flex()
        .id(SharedString::from(id))
        .test_support()
        .w(px(62.))
        .p(px(2.))
        .gap(px(2.))
        .items_center()
        .rounded(px(4.))
        .border_1()
        .border_color(if on {
            palette::accent()
        } else {
            cx.theme().border
        })
        .cursor_pointer()
        .hover(|s| s.bg(cx.theme().accent))
        .child(
            div()
                .w_full()
                .h(px(34.))
                .rounded(px(3.))
                .bg(palette::timeline_bg())
                .flex()
                .items_center()
                .justify_center()
                .text_lg()
                .text_color(fg)
                .child(icon),
        )
        .child(div().text_xs().text_color(fg).child(label))
}

impl Workspace {
    /// 选中的视频片段的（id，动画，长度）。
    fn bound_motion(&self) -> Option<(ClipId, ClipMotion, Time)> {
        let (id, _, _) = self.bound_video_clip()?;
        let (_, c) = self.doc.project.timeline.find_clip(id)?;
        Some((id, c.motion, c.duration))
    }

    /// 现在这一组选了的那个动画的时长。
    fn current_motion_secs(&self, m: ClipMotion) -> Option<f32> {
        let pick = match self.inspector.motion.seg {
            AnimSeg::In => m.intro,
            AnimSeg::Out => m.outro,
            AnimSeg::Combo => None,
        };
        #[expect(clippy::cast_possible_truncation, reason = "动画时长几秒")]
        pick.map(|p| p.duration.as_secs_f64() as f32)
    }

    fn set_motion(&mut self, m: ClipMotion, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((clip, _, _)) = self.bound_motion() else {
            return;
        };
        self.apply(window, cx, |p| ve_core::edit::set_clip_motion(p, clip, m));
    }

    /// 时长滑条：改现在这一组选了的动画的时长（拖动一步撤销）。
    fn on_motion_duration(
        &mut self,
        e: &SliderEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((clip, mut m, _)) = self.bound_motion() else {
            return;
        };
        match e {
            SliderEvent::Change(v) => {
                let d = Time::from_secs_f64(f64::from(v.start().max(0.1)));
                let slot = match self.inspector.motion.seg {
                    AnimSeg::In => &mut m.intro,
                    AnimSeg::Out => &mut m.outro,
                    AnimSeg::Combo => return,
                };
                let Some(one) = slot.as_mut() else {
                    return;
                };
                one.duration = d;
                if !self.inspector.motion.gesture {
                    self.doc.begin_gesture();
                    self.inspector.motion.gesture = true;
                }
                let _ = self
                    .doc
                    .edit(|p| ve_core::edit::set_clip_motion(p, clip, m));
                self.after_change(window, cx);
            }
            SliderEvent::Release(_) => {
                if self.inspector.motion.gesture {
                    self.doc.end_gesture();
                    self.inspector.motion.gesture = false;
                }
            }
        }
    }

    /// 时长滑条跟着选中的动画走（换了片段、换了组、撤销之后）。
    pub(crate) fn refresh_motion_slider(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.inspector.motion.gesture {
            return;
        }
        let Some(secs) = self
            .bound_motion()
            .and_then(|(_, m, _)| self.current_motion_secs(m))
        else {
            return;
        };
        let s = &self.inspector.motion.dur;
        if (s.read(cx).value().start() - secs).abs() > 0.01 {
            s.update(cx, |s, cx| s.set_value(secs, window, cx));
        }
    }

    fn render_anim_segments(&self, cx: &Context<'_, Self>) -> AnyElement {
        let seg = self.inspector.motion.seg;
        h_flex()
            .gap_1()
            .children(
                [
                    ("anim-phase-in", "入场", AnimSeg::In),
                    ("anim-phase-out", "出场", AnimSeg::Out),
                    ("anim-phase-combo", "组合", AnimSeg::Combo),
                ]
                .map(|(id, label, s)| {
                    chrome::chip(id, label, seg == s, cx).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.inspector.motion.seg = s;
                            cx.notify();
                        },
                    ))
                }),
            )
            .into_any_element()
    }

    /// 入场或出场那一排卡片（第一张是「无」）。
    fn render_phase_cards(
        &self,
        phase: Phase,
        m: ClipMotion,
        cx: &Context<'_, Self>,
    ) -> Vec<AnyElement> {
        let tag = if phase == Phase::In { "in" } else { "out" };
        let now = if phase == Phase::In { m.intro } else { m.outro };
        let secs = self.inspector.motion.dur.read(cx).value().start();
        let none = card(format!("anim-{tag}-none"), "∅", "无", now.is_none(), cx)
            .on_click(cx.listener(move |this, _, window, cx| {
                let next = if phase == Phase::In {
                    ClipMotion { intro: None, ..m }
                } else {
                    ClipMotion { outro: None, ..m }
                };
                this.set_motion(next, window, cx);
            }))
            .into_any_element();
        let cards = Motion::ALL.map(|kind| {
            let (label, slug) = naming(phase, kind);
            let on = now.map(|n| n.motion) == Some(kind);
            card(
                format!("anim-{tag}-{slug}"),
                glyph(phase, kind),
                label,
                on,
                cx,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                // 换一个动作时长不变；第一次选用滑条上的时长
                let duration = now.map_or_else(
                    || Time::from_secs_f64(f64::from(secs.max(0.1))),
                    |n| n.duration,
                );
                let one = Some(MotionClip {
                    motion: kind,
                    duration,
                });
                let next = if phase == Phase::In {
                    ClipMotion {
                        intro: one,
                        combo: None,
                        ..m
                    }
                } else {
                    ClipMotion {
                        outro: one,
                        combo: None,
                        ..m
                    }
                };
                this.set_motion(next, window, cx);
            }))
            .into_any_element()
        });
        std::iter::once(none).chain(cards).collect()
    }

    fn render_combo_cards(m: ClipMotion, cx: &Context<'_, Self>) -> Vec<AnyElement> {
        let none = card(
            "anim-combo-none".to_owned(),
            "∅",
            "无",
            m.combo.is_none(),
            cx,
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            this.set_motion(ClipMotion { combo: None, ..m }, window, cx);
        }))
        .into_any_element();
        let cards = Combo::ALL.map(|c| {
            card(
                format!("anim-combo-{}", combo_slug(c)),
                combo_glyph(c),
                c.label(),
                m.combo == Some(c),
                cx,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                // 组合动画铺满整段，和入场 / 出场二选一
                let next = ClipMotion {
                    combo: Some(c),
                    ..ClipMotion::NONE
                };
                this.set_motion(next, window, cx);
            }))
            .into_any_element()
        });
        std::iter::once(none).chain(cards).collect()
    }

    pub(crate) fn render_motion_tab(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let m = self.bound_motion().map_or(ClipMotion::NONE, |(_, m, _)| m);
        let seg = self.inspector.motion.seg;
        let cards = match seg {
            AnimSeg::In => self.render_phase_cards(Phase::In, m, cx),
            AnimSeg::Out => self.render_phase_cards(Phase::Out, m, cx),
            AnimSeg::Combo => Self::render_combo_cards(m, cx),
        };
        let duration = self.current_motion_secs(m).map(|secs| {
            let s = &self.inspector.motion.dur;
            h_flex()
                .gap_2()
                .items_center()
                .child(div().w_10().text_sm().text_color(muted).child("时长"))
                .child(
                    div()
                        .id("anim-dur")
                        .test_support()
                        .flex_1()
                        .child(Slider::new(s)),
                )
                .child(self.value_box("anim-dur", s, format!("{secs:.1}s"), 44.0, cx))
        });
        v_flex()
            .id("insp-motion")
            .test_support()
            .gap_3()
            .child(self.render_anim_segments(cx))
            .child(h_flex().flex_wrap().gap_1().children(cards))
            .children(duration)
            .child(div().text_xs().text_color(muted).child(
                "动画叠加在关键帧上：能随时改时长、换一个，或选「无」去掉；组合动画铺满整段，和入场 / 出场二选一",
            ))
    }

    /// 字幕的「动画」标签：入场、出场各一排，点一下就换；「应用到全部字幕」。
    pub(crate) fn render_text_motion_tab(
        motion: TextMotion,
        cx: &Context<'_, Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let intro = h_flex()
            .flex_wrap()
            .gap_1()
            .children(TextIn::ALL.iter().enumerate().map(|(i, &v)| {
                chrome::chip(
                    SharedString::from(format!("text-in-{i}")),
                    v.label(),
                    motion.intro == v,
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_bound_text_motion(TextMotion { intro: v, ..motion }, window, cx);
                }))
            }));
        let outro = h_flex()
            .flex_wrap()
            .gap_1()
            .children(TextOut::ALL.iter().enumerate().map(|(i, &v)| {
                chrome::chip(
                    SharedString::from(format!("text-out-{i}")),
                    v.label(),
                    motion.outro == v,
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_bound_text_motion(TextMotion { outro: v, ..motion }, window, cx);
                }))
            }));
        let looping =
            h_flex()
                .flex_wrap()
                .gap_1()
                .children(ve_core::TextLoop::ALL.iter().enumerate().map(|(i, &v)| {
                    chrome::chip(
                        SharedString::from(format!("text-loop-{i}")),
                        v.label(),
                        motion.looping == v,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_bound_text_motion(
                            TextMotion {
                                looping: v,
                                ..motion
                            },
                            window,
                            cx,
                        );
                    }))
                }));
        v_flex()
            .id("insp-text-motion")
            .gap_3()
            .child(div().text_sm().text_color(muted).child("入场"))
            .child(intro)
            .child(div().text_sm().text_color(muted).child("出场"))
            .child(outro)
            .child(div().text_sm().text_color(muted).child("循环"))
            .child(looping)
            .child(
                Button::new("text-motion-all")
                    .compact()
                    .outline()
                    .label("应用到全部字幕")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_all_text_motion(motion, window, cx);
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("入场、出场各 0.5 秒（字幕太短时各占一半）；预览和导出一样"),
            )
    }

    /// 改属性面板绑定的那条字幕的动画。
    fn set_bound_text_motion(
        &mut self,
        motion: TextMotion,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(clip) = self.inspector.bound {
            self.apply(window, cx, |p| {
                ve_core::edit::set_text_motion(p, &[clip], motion)
            });
        }
    }

    /// 所有字幕都用这个动画（一步撤销）。
    fn set_all_text_motion(
        &mut self,
        motion: TextMotion,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let all: Vec<ClipId> = self
            .doc
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Subtitle)
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect();
        self.apply(window, cx, |p| {
            ve_core::edit::set_text_motion(p, &all, motion)
        });
    }
}
