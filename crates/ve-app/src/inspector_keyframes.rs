//! 属性面板「画面」区的关键帧按钮：打 / 删关键帧（◆）、跳到上一个 / 下一个、改缓动。
//!
//! 照剪映电脑版：组标题那一行的 ◆ 一次记下所有画面属性；每个属性行右边还有一组 `‹ ◇ ›`，
//! 只给这一行的属性打帧（位置的 X、Y 一起）。有关键帧的属性，在播放头处改值会自动新建或更新关键帧
//! （见 `ve_core::edit::apply_transform_at`）。

use crate::workspace::Workspace;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable as _, h_flex, v_flex};
use gpui_kit::{
    Context, IntoElement, ParentElement as _, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use ve_core::edit::{
    add_keyframe, add_keyframe_props, remove_keyframe, remove_keyframe_props, set_keyframe_easing,
};
use ve_core::{ClipId, Easing, KeyGroup, Prop, Time};

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
        let (clip, ..) = self.bound_visual_clip()?;
        let (_, c) = self.doc.project.timeline.find_clip(clip)?;
        let offset = self.playhead - c.start;
        Some(KeyState {
            clip,
            start: c.start,
            offset,
            on_clip: offset >= Time::ZERO && offset < c.duration,
            times: c.animation.times_of(&Prop::TRANSFORM),
            // 缓动管这一刻所有属性的关键帧（画面、调色、外观、音量……）
            easing_here: c
                .animation
                .easing_of(&c.animation.props().collect::<Vec<_>>(), offset),
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
        if let Some((_, t, _)) = self.bound_visual_clip() {
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
            let all: Vec<Prop> = p
                .timeline
                .find_clip(k.clip)
                .map(|(_, c)| c.animation.props().collect())
                .unwrap_or_default();
            set_keyframe_easing(p, k.clip, k.offset, &all, easing)
        });
    }

    /// 播放头正好在 `props` 的关键帧上时，一排缓动按钮（编号 `{prefix}-ease-…`），只改这几个属性
    /// 在这一刻的关键帧；不在关键帧上时 `None`。画面以外的属性（比如特效强度）用。
    pub(crate) fn render_easing_row(
        &self,
        props: &'static [Prop],
        prefix: &'static str,
        cx: &Context<'_, Self>,
    ) -> Option<impl IntoElement> {
        let (clip, _, offset, _) = self.bound_offset()?;
        let (_, c) = self.doc.project.timeline.find_clip(clip)?;
        let current = c.animation.easing_of(props, offset)?;
        Some(
            EASINGS
                .into_iter()
                .fold(h_flex().gap_1(), move |row, (id, label, e)| {
                    let id = id.replacen("tf", prefix, 1);
                    row.child(
                        Button::new(gpui_kit::SharedString::from(id))
                            .ghost()
                            .compact()
                            .label(label)
                            .when(current == e, ButtonVariants::primary)
                            .tooltip("从这个关键帧到下一个的快慢")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.apply(window, cx, |p| {
                                    set_keyframe_easing(p, clip, offset, props, e)
                                });
                            })),
                    )
                }),
        )
    }

    /// 播放头落在片段 `c` 里的位置（读值、打关键帧用，见 [`ve_core::Clip::key_offset`]）。
    pub(crate) fn key_offset(&self, c: &ve_core::Clip) -> Time {
        c.key_offset(
            self.playhead,
            self.doc.project.settings.frame_rate.frame_duration(),
        )
    }

    /// 选中的那一个片段（任何轨道）和播放头的关系：（片段，起点，播放头相对起点，播放头在片段上）。
    fn bound_offset(&self) -> Option<(ClipId, Time, Time, bool)> {
        let clip = self.inspector.bound?;
        let (_, c) = self.doc.project.timeline.find_clip(clip)?;
        let offset = self.playhead - c.start;
        Some((
            clip,
            c.start,
            offset,
            offset >= Time::ZERO && offset < c.duration,
        ))
    }

    /// 某一行（属性组）的 ◇：播放头在这组的关键帧上时删掉，否则只给这组打一个。
    fn toggle_group_key(
        &mut self,
        group: KeyGroup,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(k) = self
            .bound_offset()
            .map(|(clip, start, offset, on_clip)| KeyState {
                clip,
                start,
                offset,
                on_clip,
                times: Vec::new(),
                easing_here: None,
            })
            .filter(|k| k.on_clip)
        else {
            return;
        };
        let props = group.props();
        let here = self
            .doc
            .project
            .timeline
            .find_clip(k.clip)
            .is_some_and(|(_, c)| c.animation.times_of(props).contains(&k.offset));
        if here {
            self.apply(window, cx, |p| {
                remove_keyframe_props(p, k.clip, k.offset, props)
            });
        } else {
            self.apply(window, cx, |p| {
                add_keyframe_props(p, k.clip, k.offset, props)
            });
        }
        if let Some((_, t, _)) = self.bound_visual_clip() {
            self.fill_transform(&t, window, cx);
        }
        self.refresh_animated_sliders(true, window, cx);
    }

    /// 跳到这一组的上一个 / 下一个关键帧。
    fn jump_group_key(&mut self, group: KeyGroup, forward: bool, cx: &mut Context<'_, Self>) {
        let Some((clip, start, offset, _)) = self.bound_offset() else {
            return;
        };
        let times = self.group_times(clip, group);
        let target = if forward {
            times.iter().find(|t| **t > offset)
        } else {
            times.iter().rev().find(|t| **t < offset)
        };
        if let Some(t) = target {
            self.seek(start + *t, cx);
        }
    }

    fn group_times(&self, clip: ClipId, group: KeyGroup) -> Vec<Time> {
        self.doc
            .project
            .timeline
            .find_clip(clip)
            .map(|(_, c)| c.animation.times_of(group.props()))
            .unwrap_or_default()
    }

    /// 属性行右边的 `‹ ◇ ›`：◆ 实心 = 播放头正好在这组的关键帧上。
    pub(crate) fn render_group_keys(
        &self,
        group: KeyGroup,
        cx: &Context<'_, Self>,
    ) -> impl IntoElement {
        let name = match group {
            KeyGroup::Position => "pos",
            KeyGroup::Scale => "scale",
            KeyGroup::Rotation => "rot",
            KeyGroup::Opacity => "opacity",
            KeyGroup::Crop => "crop",
            KeyGroup::Adjust(ve_core::AdjustItem::Brightness) => "brightness",
            KeyGroup::Adjust(ve_core::AdjustItem::Contrast) => "contrast",
            KeyGroup::Adjust(ve_core::AdjustItem::Saturation) => "saturation",
            KeyGroup::Adjust(ve_core::AdjustItem::Temperature) => "temperature",
            KeyGroup::Adjust(ve_core::AdjustItem::Tint) => "tint",
            KeyGroup::Volume => "volume",
            KeyGroup::Effect => "effect",
            KeyGroup::Look(p) => look_key_name(p),
        };
        let id = |suffix: &str| -> gpui_kit::SharedString {
            if suffix.is_empty() {
                format!("tf-key-{name}").into()
            } else {
                format!("tf-key-{name}-{suffix}").into()
            }
        };
        let (times, offset, on_clip) = self
            .bound_offset()
            .map_or((Vec::new(), Time::ZERO, false), |(clip, _, offset, on)| {
                (self.group_times(clip, group), offset, on)
            });
        let on_key = times.contains(&offset);
        let has_prev = times.iter().any(|t| *t < offset);
        let has_next = times.iter().any(|t| *t > offset);
        let arrow = |id: gpui_kit::SharedString, label: &'static str| {
            Button::new(id).ghost().xsmall().label(label)
        };
        h_flex()
            .flex_none()
            .child(
                arrow(id("prev"), "‹")
                    .tooltip("这一项的上一个关键帧")
                    .disabled(!has_prev)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.jump_group_key(group, false, cx)),
                    ),
            )
            .child(
                Button::new(id(""))
                    .ghost()
                    .xsmall()
                    .label(if on_key { "◆" } else { "◇" })
                    .when(on_key, ButtonVariants::primary)
                    .tooltip(if on_key {
                        "删除这一项在播放头处的关键帧"
                    } else {
                        "只给这一项在播放头处打关键帧"
                    })
                    .disabled(!on_clip)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_group_key(group, window, cx);
                    })),
            )
            .child(
                arrow(id("next"), "›")
                    .tooltip("这一项的下一个关键帧")
                    .disabled(!has_next)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.jump_group_key(group, true, cx)),
                    ),
            )
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

/// 外观属性那一行关键帧按钮的编号后缀（`tf-key-<后缀>`）。
const fn look_key_name(p: Prop) -> &'static str {
    match p {
        Prop::Radius => "radius",
        Prop::StrokeWidth => "stroke-width",
        Prop::ShadowOpacity => "shadow-opacity",
        Prop::ShadowBlur => "shadow-blur",
        Prop::ShadowDistance => "shadow-distance",
        Prop::ShadowAngle => "shadow-angle",
        Prop::KeyIntensity => "key-intensity",
        Prop::KeyShadow => "key-shadow",
        Prop::MaskX => "mask-x",
        Prop::MaskY => "mask-y",
        Prop::MaskWidth => "mask-w",
        Prop::MaskHeight => "mask-h",
        Prop::MaskRotation => "mask-rot",
        Prop::MaskFeather => "mask-feather",
        Prop::MaskRound => "mask-round",
        _ => "other",
    }
}
