//! 属性面板「画面」区的关键帧按钮：打 / 删关键帧（◆）、跳到上一个 / 下一个、改缓动。
//!
//! 照剪映的习惯：◆ 一次记下所有画面属性；片段有了动画以后，在播放头处改属性会自动新建或更新关键帧
//! （见 `ve_core::edit::apply_transform_at`）。

use crate::workspace::Workspace;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable as _, h_flex, v_flex};
use gpui_kit::{
    Context, IntoElement, ParentElement as _, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use ve_core::edit::{add_keyframe, remove_keyframe, set_keyframe_easing};
use ve_core::{ClipId, Easing, Time};

/// 缓动按钮：（id，文字，缓动）。
const EASINGS: [(&str, &str, Easing); 4] = [
    ("tf-ease-linear", "匀速", Easing::Linear),
    ("tf-ease-in", "缓入", Easing::EaseIn),
    ("tf-ease-out", "缓出", Easing::EaseOut),
    ("tf-ease-inout", "缓入缓出", Easing::EaseInOut),
];

/// 绑定片段的关键帧情况。
struct KeyState {
    clip: ClipId,
    /// 片段在时间轴上的起点。
    start: Time,
    /// 播放头相对片段起点的时间（可能在片段外）。
    offset: Time,
    /// 播放头在片段上（才能打关键帧）。
    on_clip: bool,
    /// 关键帧时间（相对片段起点，排好）。
    times: Vec<Time>,
    /// 播放头正好在关键帧上时，它的缓动。
    easing_here: Option<Easing>,
}

impl Workspace {
    fn key_state(&self) -> Option<KeyState> {
        let (clip, ..) = self.bound_video_clip()?;
        let (_, c) = self.doc.project.timeline.find_clip(clip)?;
        let offset = self.playhead - c.start;
        Some(KeyState {
            clip,
            start: c.start,
            offset,
            on_clip: offset >= Time::ZERO && offset < c.duration,
            times: c.animation.times(),
            easing_here: c.animation.easing_at(offset),
        })
    }

    /// ◆：播放头不在关键帧上时打一个，在关键帧上时删掉它。
    fn toggle_keyframe(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(k) = self.key_state().filter(|k| k.on_clip) else {
            return;
        };
        if k.times.contains(&k.offset) {
            self.apply(window, cx, |p| remove_keyframe(p, k.clip, k.offset));
        } else {
            self.apply(window, cx, |p| add_keyframe(p, k.clip, k.offset));
        }
        if let Some((_, t, _)) = self.bound_video_clip() {
            self.fill_transform(&t, window, cx);
        }
    }

    /// 跳到上一个（`forward = false`）/ 下一个关键帧。
    fn jump_keyframe(&mut self, forward: bool, cx: &mut Context<'_, Self>) {
        let Some(k) = self.key_state() else { return };
        let target = if forward {
            k.times.iter().find(|t| **t > k.offset)
        } else {
            k.times.iter().rev().find(|t| **t < k.offset)
        };
        if let Some(t) = target {
            self.seek(k.start + *t, cx);
        }
    }

    fn set_easing_here(&mut self, easing: Easing, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(k) = self.key_state() else { return };
        self.apply(window, cx, |p| {
            set_keyframe_easing(p, k.clip, k.offset, easing)
        });
    }

    /// 关键帧按钮一行；播放头在关键帧上时下面再加一行缓动按钮。
    pub(crate) fn render_keyframe_bar(&self, cx: &Context<'_, Self>) -> Option<impl IntoElement> {
        let k = self.key_state()?;
        let on_key = k.times.contains(&k.offset);
        let has_prev = k.times.iter().any(|t| *t < k.offset);
        let has_next = k.times.iter().any(|t| *t > k.offset);
        let small =
            |id: &'static str, label: &'static str| Button::new(id).ghost().compact().label(label);
        let keys = h_flex()
            .gap_1()
            .child(
                small("tf-key-prev", "◀")
                    .tooltip("上一个关键帧")
                    .disabled(!has_prev)
                    .on_click(cx.listener(|this, _, _, cx| this.jump_keyframe(false, cx))),
            )
            .child(
                Button::new("tf-key")
                    .compact()
                    .when(on_key, ButtonVariants::primary)
                    .when(!on_key, Button::outline)
                    .label(if on_key {
                        "◆ 删除关键帧"
                    } else {
                        "◇ 添加关键帧"
                    })
                    .tooltip(if k.on_clip {
                        "在播放头处记下全部画面属性；之后在别处改属性会自动打关键帧"
                    } else {
                        "播放头要在这个片段上"
                    })
                    .disabled(!k.on_clip)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_keyframe(window, cx))),
            )
            .child(
                small("tf-key-next", "▶")
                    .tooltip("下一个关键帧")
                    .disabled(!has_next)
                    .on_click(cx.listener(|this, _, _, cx| this.jump_keyframe(true, cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{} 个关键帧", k.times.len())),
            );
        let easing = k.easing_here.map(|current| {
            EASINGS
                .into_iter()
                .fold(h_flex().gap_1(), |row, (id, label, e)| {
                    row.child(
                        small(id, label)
                            // 当前这一段用的缓动：实心按钮，一眼看得出
                            .when(current == e, ButtonVariants::primary)
                            .tooltip("从这个关键帧到下一个的快慢")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_easing_here(e, window, cx);
                            })),
                    )
                })
        });
        Some(v_flex().gap_1().child(keys).children(easing))
    }
}
