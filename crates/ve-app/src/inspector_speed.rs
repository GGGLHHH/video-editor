//! 属性面板「变速」标签（剪映「常规变速」）：倍数点选，素材用的那一段不变、时间轴上的长度跟着变；
//! 「声音变调」默认关着（音调不变），打开后音调跟着快慢变。

use crate::chrome;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div,
};
use ve_core::format_timecode;

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

impl Workspace {
    pub(crate) fn render_speed_tab(
        &self,
        clip: &ve_core::Clip,
        cx: &Context<'_, Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let rate = self.doc.project.settings.frame_rate;
        let id = clip.id;
        let chips = SPEEDS.map(|(slug, label, s)| {
            let on = (clip.speed - s).abs() < 1e-4;
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
            .id("insp-speed")
            .test_support()
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
    }
}
