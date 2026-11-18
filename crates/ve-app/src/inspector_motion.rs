//! 属性面板「动画」标签：入场 / 出场预设（剪映的动画），用关键帧实现。

use crate::chrome;
use crate::workspace::Workspace;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div,
};
use ve_core::Time;
use ve_core::motion::{Motion, Phase, apply_motion};

/// 时长选项（秒）。
const DURATIONS: [(&str, f64); 4] = [("0.3s", 0.3), ("0.5s", 0.5), ("1s", 1.0), ("2s", 2.0)];

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

impl Workspace {
    pub(crate) fn render_motion_tab(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let phase = if self.inspector.anim_out {
            Phase::Out
        } else {
            Phase::In
        };
        let secs = self.inspector.anim_secs;
        let phases = h_flex()
            .gap_1()
            .child(
                chrome::chip("anim-phase-in", "入场", phase == Phase::In, cx).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.inspector.anim_out = false;
                        cx.notify();
                    }),
                ),
            )
            .child(
                chrome::chip("anim-phase-out", "出场", phase == Phase::Out, cx).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.inspector.anim_out = true;
                        cx.notify();
                    }),
                ),
            );
        let durations = h_flex().gap_1().children(DURATIONS.map(|(label, s)| {
            #[expect(clippy::float_cmp, reason = "比较的是同一组常量")]
            let on = secs == s;
            chrome::chip(format!("anim-dur-{label}"), label, on, cx).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.inspector.anim_secs = s;
                    cx.notify();
                },
            ))
        }));
        let tag = if phase == Phase::In { "in" } else { "out" };
        let presets = h_flex().flex_wrap().gap_1().children(Motion::ALL.map(|m| {
            let (label, id) = naming(phase, m);
            chrome::chip(format!("anim-{tag}-{id}"), label, false, cx).on_click(cx.listener(
                move |this, _, window, cx| {
                    let Some(clip) = this.inspector.bound else {
                        return;
                    };
                    let d = Time::from_secs_f64(this.inspector.anim_secs);
                    this.apply(window, cx, |p| apply_motion(p, clip, phase, m, d));
                    if let Some((_, t, _)) = this.bound_video_clip() {
                        this.fill_transform(&t, window, cx);
                    }
                },
            ))
        }));
        v_flex()
            .id("insp-motion")
            .gap_3()
            .child(phases)
            .child(div().text_sm().text_color(muted).child("时长"))
            .child(durations)
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("预设（点一下套用，再点别的就替换）"),
            )
            .child(presets)
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("动画是用关键帧做的：套用后可以在「画面」里接着改关键帧"),
            )
    }
}
