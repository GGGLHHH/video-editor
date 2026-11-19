//! 属性面板的「画面」区：视频轨片段的位置、缩放、旋转、裁剪、不透明度。

use crate::preview_handles::{Align, align};
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Input, InputEvent, InputState, NumberInput, NumberInputEvent, NumberStep, StepAction,
};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, Subscription, Window, div, px,
};
use ve_core::edit::{apply_transform_at, reset_transform};
use ve_core::{ClipId, Crop, KeyGroup, Time, TrackKind, Transform, wrap_degrees};

/// 位置偏移的输入范围（±500%）。
const OFFSET_RANGE: (f32, f32) = (-5.0, 5.0);
/// 单边裁剪的输入上限。
const CROP_MAX: f32 = 0.99;

/// 「画面」区的控件。
#[derive(Debug)]
pub struct TransformFields {
    x: Entity<InputState>,
    y: Entity<InputState>,
    scale: Entity<InputState>,
    /// 旋转（度）。
    rotation: Entity<InputState>,
    /// 左、上、右、下。
    crop: [Entity<InputState>; 4],
    opacity: Entity<SliderState>,
    opacity_gesture: bool,
    /// 缩放滑块（1%–400%，剪映的「缩放」那一条）；超出范围的值靠输入框打。
    scale_slider: Entity<SliderState>,
    scale_gesture: bool,
    /// 上次填进输入框的文字：没被改过的字段提交时沿用片段的精确值，
    /// 不会把四舍五入后的显示值写回去（预览上拖出来的值精度比显示的高）。
    shown: [String; 8],
}

/// 缩放滑块的范围（%）。
const SCALE_SLIDER: (f32, f32) = (1.0, 400.0);

/// 带「−」「+」的数值框。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepField {
    X,
    Y,
    Scale,
    Rotation,
}

/// 按一格之后的文字：每格 1（%，或 1°）；文字不是数时从 0 起。
fn stepped(text: &str, action: StepAction) -> String {
    let v = text
        .trim()
        .trim_end_matches(['%', '°'])
        .parse::<f64>()
        .unwrap_or(0.0);
    let next = match action {
        StepAction::Increment => v + 1.0,
        StepAction::Decrement => v - 1.0,
    };
    one_decimal(next)
}

/// 比例 → 百分比文字（去掉多余的小数）。
fn pct(v: f32) -> String {
    one_decimal(f64::from(v) * 100.0)
}

/// 一位小数、去掉 `.0`；负零和四舍五入后是 0 的小负数写成 `0`。
fn one_decimal(v: f64) -> String {
    let s = format!("{v:.1}");
    let s = s.strip_suffix(".0").unwrap_or(&s);
    if s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

/// 百分比文字 → 比例；不是有限数时返回 `None`。
fn parse_pct(s: &str) -> Option<f32> {
    s.trim()
        .trim_end_matches('%')
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .map(|v| v / 100.0)
}

/// 面板各字段显示的文字，和 `texts` 同一个顺序：X、Y、缩放、裁剪左上右下、旋转。
fn shown_texts(t: &Transform) -> [String; 8] {
    [
        pct(t.x),
        pct(t.y),
        pct(t.scale),
        pct(t.crop.left),
        pct(t.crop.top),
        pct(t.crop.right),
        pct(t.crop.bottom),
        deg(t.rotation),
    ]
}

/// 角度 → 文字（一位小数，去掉 `.0`）。
fn deg(v: f32) -> String {
    one_decimal(f64::from(v))
}

/// 角度文字 → 度；不是有限数时返回 `None`。
fn parse_deg(s: &str) -> Option<f32> {
    s.trim()
        .trim_end_matches('°')
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
}

/// 由各字段文字算出新的画面属性：解析不了的字段保持原值，越界的夹到范围内；
/// 左右或上下裁剪加起来到 100% 以上时整次修改无效，返回 `None`。
#[must_use]
pub fn transform_from_text(current: &Transform, fields: &[String; 8]) -> Option<Transform> {
    let get = |i: usize, old: f32| parse_pct(&fields[i]).unwrap_or(old);
    let crop = |i: usize, old: f32| get(i, old).clamp(0.0, CROP_MAX);
    let t = Transform {
        x: get(0, current.x).clamp(OFFSET_RANGE.0, OFFSET_RANGE.1),
        y: get(1, current.y).clamp(OFFSET_RANGE.0, OFFSET_RANGE.1),
        scale: get(2, current.scale).clamp(Transform::SCALE_RANGE.0, Transform::SCALE_RANGE.1),
        crop: Crop {
            left: crop(3, current.crop.left),
            top: crop(4, current.crop.top),
            right: crop(5, current.crop.right),
            bottom: crop(6, current.crop.bottom),
        },
        opacity: current.opacity,
        rotation: wrap_degrees(parse_deg(&fields[7]).unwrap_or(current.rotation)),
        mirror: current.mirror,
    };
    t.is_valid().then_some(t)
}

impl TransformFields {
    /// 创建控件并订阅事件。
    pub fn new(window: &mut Window, cx: &mut Context<'_, Workspace>) -> (Self, Vec<Subscription>) {
        // 「−」「+」不让输入框自己悄悄改文字（那样不发事件），改成发 Step 事件，由这里改值并提交
        let mut input = || {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder("0");
                state.set_step(None::<NumberStep>, window, cx);
                state
            })
        };
        let (x, y, scale, rotation) = (input(), input(), input(), input());
        let crop = [input(), input(), input(), input()];
        let opacity = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(100.0)
                .step(1.0)
                .default_value(100.0)
        });
        let scale_slider = cx.new(|_| {
            SliderState::new()
                .min(SCALE_SLIDER.0)
                .max(SCALE_SLIDER.1)
                .step(1.0)
                .default_value(100.0)
        });
        let mut subs: Vec<Subscription> = [&x, &y, &scale, &rotation]
            .into_iter()
            .chain(&crop)
            .map(|field| {
                cx.subscribe_in(
                    field,
                    window,
                    |this: &mut Workspace, _, e: &InputEvent, window, cx| {
                        if matches!(e, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                            this.commit_transform(window, cx);
                        }
                    },
                )
            })
            .collect();
        subs.push(cx.subscribe_in(
            &opacity,
            window,
            |this: &mut Workspace, _, e: &SliderEvent, window, cx| this.on_opacity(e, window, cx),
        ));
        subs.push(cx.subscribe_in(
            &scale_slider,
            window,
            |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                this.on_scale_slider(e, window, cx);
            },
        ));
        // 数值框两边的「−」「+」（和方向键 ↑ ↓）：按一格改值并立即生效
        for (field, state) in [
            (StepField::X, &x),
            (StepField::Y, &y),
            (StepField::Scale, &scale),
            (StepField::Rotation, &rotation),
        ] {
            subs.push(cx.subscribe_in(
                state,
                window,
                move |this: &mut Workspace, _, e: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step(action) = e;
                    this.step_transform(field, *action, window, cx);
                },
            ));
        }
        (
            Self {
                x,
                y,
                scale,
                rotation,
                crop,
                opacity,
                opacity_gesture: false,
                scale_slider,
                scale_gesture: false,
                shown: Default::default(),
            },
            subs,
        )
    }

    /// 各字段的文字；没被改过的字段返回空串（解析失败 → 沿用原值）。
    fn edited_texts(&self, cx: &gpui_kit::App) -> [String; 8] {
        let mut texts = self.texts(cx);
        for (t, shown) in texts.iter_mut().zip(&self.shown) {
            if t == shown {
                t.clear();
            }
        }
        texts
    }

    fn texts(&self, cx: &gpui_kit::App) -> [String; 8] {
        let read = |e: &Entity<InputState>| e.read(cx).value().to_string();
        [
            read(&self.x),
            read(&self.y),
            read(&self.scale),
            read(&self.crop[0]),
            read(&self.crop[1]),
            read(&self.crop[2]),
            read(&self.crop[3]),
            read(&self.rotation),
        ]
    }
}

impl Workspace {
    /// 属性面板绑定的片段在视频轨上时，返回它、播放头那一刻的画面属性（有动画时已求值），
    /// 以及那一刻相对片段起点的时间（播放头不在片段上时夹到片段两端）。
    pub(crate) fn bound_video_clip(&self) -> Option<(ClipId, Transform, Time)> {
        let clip = self.inspector.bound?;
        let (track, c) = self.doc.project.timeline.find_clip(clip)?;
        let kind = self.doc.project.timeline.track(track)?.kind;
        if kind != TrackKind::Video || c.asset().is_none() {
            return None;
        }
        let offset = (self.playhead - c.start).clamp(Time::ZERO, c.duration);
        Some((clip, c.transform_at(offset), offset))
    }

    /// 播放头动了：绑定的片段有动画时，面板里的数字跟着换成这一刻的值。
    /// 只在算出来的文字和上次填的不一样时才填，用户正在输入的内容不会被每帧覆盖。
    pub(crate) fn follow_playhead_in_inspector(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((clip, t, _)) = self.bound_video_clip() else {
            return;
        };
        let animated = self
            .doc
            .project
            .timeline
            .find_clip(clip)
            .is_some_and(|(_, c)| !c.animation.is_empty());
        if animated && shown_texts(&t) != self.inspector.transform.shown {
            self.fill_transform(&t, window, cx);
        }
    }

    /// 把片段的画面属性填进控件。
    pub(crate) fn fill_transform(
        &mut self,
        t: &Transform,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let shown = shown_texts(t);
        self.inspector.transform.shown.clone_from(&shown);
        let f = &self.inspector.transform;
        let fields = [
            &f.x,
            &f.y,
            &f.scale,
            &f.crop[0],
            &f.crop[1],
            &f.crop[2],
            &f.crop[3],
            &f.rotation,
        ];
        for (field, text) in fields.into_iter().zip(shown) {
            field.update(cx, |s, cx| s.set_value(text, window, cx));
        }
        let opacity = t.opacity * 100.0;
        f.opacity
            .update(cx, |s, cx| s.set_value(opacity, window, cx));
        let scale = (t.scale * 100.0).clamp(SCALE_SLIDER.0, SCALE_SLIDER.1);
        f.scale_slider
            .update(cx, |s, cx| s.set_value(scale, window, cx));
    }

    /// 数值框的「−」「+」：改这一格、立即提交（一步撤销）。
    fn step_transform(
        &mut self,
        field: StepField,
        action: StepAction,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let f = &self.inspector.transform;
        let state = match field {
            StepField::X => &f.x,
            StepField::Y => &f.y,
            StepField::Scale => &f.scale,
            StepField::Rotation => &f.rotation,
        }
        .clone();
        let text = stepped(&state.read(cx).value(), action);
        state.update(cx, |s, cx| s.set_value(text, window, cx));
        self.commit_transform(window, cx);
    }

    /// 缩放滑块：拖动中实时改，松手合成一步撤销（和不透明度一样）。
    fn on_scale_slider(
        &mut self,
        e: &SliderEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((clip, current, at)) = self.bound_video_clip() else {
            return;
        };
        match e {
            SliderEvent::Change(v) => {
                if !self.inspector.transform.scale_gesture {
                    self.doc.begin_gesture();
                    self.inspector.transform.scale_gesture = true;
                }
                let t = Transform {
                    scale: (v.start() / 100.0)
                        .clamp(Transform::SCALE_RANGE.0, Transform::SCALE_RANGE.1),
                    ..current
                };
                let _ = self.doc.edit(|p| apply_transform_at(p, clip, at, t));
                self.after_change(window, cx);
                // 输入框跟着滑块走
                if let Some((_, now, _)) = self.bound_video_clip() {
                    let text = shown_texts(&now)[2].clone();
                    self.inspector.transform.shown[2].clone_from(&text);
                    self.inspector
                        .transform
                        .scale
                        .update(cx, |s, cx| s.set_value(text, window, cx));
                }
            }
            SliderEvent::Release(_) => {
                if self.inspector.transform.scale_gesture {
                    self.doc.end_gesture();
                    self.inspector.transform.scale_gesture = false;
                }
            }
        }
    }

    /// 输入框改完（回车 / 失焦）：按各字段写回；无效时恢复显示原值。
    fn commit_transform(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((clip, current, at)) = self.bound_video_clip() else {
            return;
        };
        if let Some(t) = transform_from_text(&current, &self.inspector.transform.edited_texts(cx))
            && t != current
        {
            self.apply(window, cx, |p| apply_transform_at(p, clip, at, t));
        }
        // 夹过范围或被拒绝时，显示实际生效的值
        if let Some((_, now, _)) = self.bound_video_clip() {
            self.fill_transform(&now, window, cx);
        }
    }

    /// 换绑前把输入框里还没提交的画面属性写回原片段，返回是否改了。
    pub(crate) fn commit_pending_transform(&mut self, cx: &Context<'_, Self>) -> bool {
        let Some((clip, current, at)) = self.bound_video_clip() else {
            return false;
        };
        match transform_from_text(&current, &self.inspector.transform.edited_texts(cx)) {
            Some(t) if t != current => self
                .doc
                .edit(|p| apply_transform_at(p, clip, at, t))
                .is_ok(),
            _ => false,
        }
    }

    fn on_opacity(&mut self, e: &SliderEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((clip, current, at)) = self.bound_video_clip() else {
            return;
        };
        match e {
            SliderEvent::Change(v) => {
                if !self.inspector.transform.opacity_gesture {
                    self.doc.begin_gesture();
                    self.inspector.transform.opacity_gesture = true;
                }
                let t = Transform {
                    opacity: (v.start() / 100.0).clamp(0.0, 1.0),
                    ..current
                };
                let _ = self.doc.edit(|p| apply_transform_at(p, clip, at, t));
                self.after_change(window, cx);
            }
            SliderEvent::Release(_) => {
                if self.inspector.transform.opacity_gesture {
                    self.doc.end_gesture();
                    self.inspector.transform.opacity_gesture = false;
                }
            }
        }
    }

    fn reset_transform(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((clip, ..)) = self.bound_video_clip() else {
            return;
        };
        self.apply(window, cx, |p| reset_transform(p, clip));
        self.fill_transform(&Transform::default(), window, cx);
    }

    /// 「画面」区（只在选中视频轨上的片段时有）。
    /// 对齐：把选中的画面贴到画布的边或中线（有关键帧时在播放头处打帧）。
    fn align_layer(&mut self, how: Align, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((clip, t, offset)) = self.bound_video_clip() else {
            return;
        };
        let project = &self.doc.project;
        let Some(info) = project
            .timeline
            .find_clip(clip)
            .and_then(|(_, c)| c.asset())
            .and_then(|a| project.assets.get(&a))
            .and_then(|a| a.video)
        else {
            return;
        };
        let s = project.settings;
        let geom = ve_core::layer_geometry((s.width, s.height), (info.width, info.height), &t);
        #[expect(clippy::cast_precision_loss, reason = "画布尺寸远小于 2^23")]
        let canvas = (s.width as f32, s.height as f32);
        let next = align(&t, &geom, canvas, how);
        self.apply(window, cx, |p| apply_transform_at(p, clip, offset, next));
        self.fill_transform(&next, window, cx);
    }

    /// 六个对齐按钮一行（剪映「位置大小」组里的那一排）。
    fn render_align_row(cx: &Context<'_, Self>) -> impl IntoElement {
        use gpui_kit::assets::IconName as Lucide;
        let buttons = Align::ALL.map(|how| {
            let (id, icon, tip) = match how {
                Align::Left => ("tf-align-left", Lucide::AlignStartVertical, "左对齐"),
                Align::CenterX => ("tf-align-cx", Lucide::AlignCenterVertical, "水平居中"),
                Align::Right => ("tf-align-right", Lucide::AlignEndVertical, "右对齐"),
                Align::Top => ("tf-align-top", Lucide::AlignStartHorizontal, "顶对齐"),
                Align::CenterY => ("tf-align-cy", Lucide::AlignCenterHorizontal, "垂直居中"),
                Align::Bottom => ("tf-align-bottom", Lucide::AlignEndHorizontal, "底对齐"),
            };
            Button::new(id)
                .ghost()
                .compact()
                .icon(gpui_kit::component::Icon::new(icon))
                .tooltip(tip)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.align_layer(how, window, cx);
                }))
        });
        h_flex()
            .gap_2()
            .child(
                div()
                    .w(px(64.))
                    .flex_none()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("对齐"),
            )
            .child(h_flex().gap_0p5().children(buttons))
    }

    /// 「位置大小」组标题：名字和整组「重置」。
    fn render_transform_header(cx: &Context<'_, Self>) -> impl IntoElement {
        h_flex()
            .justify_between()
            .child(div().text_sm().child("位置大小"))
            .child(
                Button::new("tf-reset")
                    .ghost()
                    .compact()
                    .label("重置")
                    .tooltip("恢复铺满画面、居中、不旋转、不裁剪，并清掉关键帧")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.reset_transform(window, cx);
                    })),
            )
    }

    pub(crate) fn render_transform_section(
        &self,
        cx: &Context<'_, Self>,
    ) -> Option<impl IntoElement> {
        let (_, t, _) = self.bound_video_clip()?;
        let muted = cx.theme().muted_foreground;
        let f = &self.inspector.transform;
        let field = |id: &'static str, state: &Entity<InputState>| {
            div()
                .flex_1()
                .min_w_0()
                .id(id)
                .test_support()
                .child(Input::new(state))
        };
        // 带「−」「+」的数值框（剪映的 ⇕ 微调）
        let number = |id: &'static str, state: &Entity<InputState>| {
            div()
                .flex_1()
                .min_w_0()
                .id(id)
                .test_support()
                .child(NumberInput::new(state).xsmall())
        };
        let tag = |label: &'static str| div().text_xs().text_color(muted).child(label);
        // 剪映式：一行一个属性，左边名字、中间数值、右边这一行的关键帧 ‹ ◇ ›
        let line = |label: &'static str, body: gpui_kit::AnyElement, group: KeyGroup| {
            h_flex()
                .gap_2()
                .child(
                    div()
                        .w(px(64.))
                        .flex_none()
                        .text_sm()
                        .text_color(muted)
                        .child(label),
                )
                .child(h_flex().flex_1().min_w_0().gap_1().child(body))
                .child(self.render_group_keys(group, cx))
        };
        Some(
            v_flex()
                .gap_2()
                .child(Self::render_transform_header(cx))
                .children(self.render_keyframe_bar(cx))
                .child(line(
                    "缩放 %",
                    h_flex()
                        .flex_1()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .id("tf-scale-slider")
                                .test_support()
                                .child(Slider::new(&f.scale_slider)),
                        )
                        .child(div().w(px(96.)).child(number("tf-scale", &f.scale)))
                        .into_any_element(),
                    KeyGroup::Scale,
                ))
                .child(line(
                    "位置 %",
                    h_flex()
                        .flex_1()
                        .gap_1()
                        .child(tag("X"))
                        .child(number("tf-x", &f.x))
                        .child(tag("Y"))
                        .child(number("tf-y", &f.y))
                        .into_any_element(),
                    KeyGroup::Position,
                ))
                .child(Self::render_align_row(cx))
                .child(line(
                    "旋转 °",
                    number("tf-rotation", &f.rotation).into_any_element(),
                    KeyGroup::Rotation,
                ))
                .child(line(
                    "裁剪 %",
                    h_flex()
                        .flex_1()
                        .gap_1()
                        .child(tag("左"))
                        .child(field("tf-crop-l", &f.crop[0]))
                        .child(tag("上"))
                        .child(field("tf-crop-t", &f.crop[1]))
                        .child(tag("右"))
                        .child(field("tf-crop-r", &f.crop[2]))
                        .child(tag("下"))
                        .child(field("tf-crop-b", &f.crop[3]))
                        .into_any_element(),
                    KeyGroup::Crop,
                ))
                .child(line(
                    "不透明度",
                    self.opacity_row(t.opacity),
                    KeyGroup::Opacity,
                )),
        )
    }

    /// 不透明度：滑块 + 百分比。
    fn opacity_row(&self, opacity: f32) -> gpui_kit::AnyElement {
        h_flex()
            .flex_1()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .id("tf-opacity")
                    .test_support()
                    .child(Slider::new(&self.inspector.transform.opacity)),
            )
            .child(
                div()
                    .w(px(48.))
                    .text_sm()
                    .child(SharedString::from(format!("{}%", pct(opacity)))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(v: [&str; 8]) -> [String; 8] {
        v.map(str::to_owned)
    }

    #[test]
    fn steppers_move_one_unit() {
        assert_eq!(stepped("12", StepAction::Increment), "13");
        assert_eq!(stepped("0.5", StepAction::Decrement), "-0.5");
        assert_eq!(stepped(" 100% ", StepAction::Decrement), "99");
        assert_eq!(stepped("45°", StepAction::Increment), "46");
        assert_eq!(
            stepped("abc", StepAction::Increment),
            "1",
            "不是数时从 0 起"
        );
    }

    #[test]
    fn percent_text_round_trips() {
        assert_eq!(pct(0.25), "25");
        assert_eq!(pct(-0.125), "-12.5");
        // 负零、四舍五入后是 0 的小负数都显示成 0
        assert_eq!(pct(-0.0), "0");
        assert_eq!(pct(-0.000_01), "0");
        assert_eq!(parse_pct(" 50% "), Some(0.5));
        assert_eq!(parse_pct("abc"), None);
        assert_eq!(parse_pct("inf"), None);
    }

    #[test]
    fn fields_parse_clamp_and_reject() {
        let cur = Transform::default();
        let t = transform_from_text(&cur, &fields(["25", "-10", "50", "0", "0", "0", "0", ""]))
            .unwrap();
        assert!(
            (t.x - 0.25).abs() < 1e-6 && (t.y + 0.1).abs() < 1e-6 && (t.scale - 0.5).abs() < 1e-6
        );
        // 解析不了的保持原值；越界的夹到范围
        let t = transform_from_text(&cur, &fields(["abc", "900", "0", "0", "0", "0", "0", ""]))
            .unwrap();
        assert_eq!(t.x, 0.0);
        assert_eq!(t.y, OFFSET_RANGE.1);
        assert_eq!(t.scale, Transform::SCALE_RANGE.0);
        // 左右裁剪加起来 ≥ 100%：整次无效
        assert!(
            transform_from_text(&cur, &fields(["0", "0", "100", "60", "0", "50", "0", ""]))
                .is_none()
        );
        let t = transform_from_text(&cur, &fields(["0", "0", "100", "150", "0", "0", "0", ""]))
            .unwrap();
        assert!((t.crop.left - CROP_MAX).abs() < f32::EPSILON);
    }

    #[test]
    fn rotation_field_in_degrees_wraps_to_half_turn() {
        let cur = Transform::default();
        let rot = |text: &str| {
            transform_from_text(&cur, &fields(["", "", "", "", "", "", "", text]))
                .unwrap()
                .rotation
        };
        assert!((rot("45") - 45.0).abs() < 1e-4);
        assert!((rot("450°") - 90.0).abs() < 1e-4);
        assert!((rot("-190") - 170.0).abs() < 1e-4);
        assert!((rot("180") - 180.0).abs() < 1e-4);
        assert!((rot("-180") - 180.0).abs() < 1e-4);
        assert_eq!(rot("abc"), 0.0);
        assert_eq!(deg(-12.26), "-12.3");
        assert_eq!(deg(90.0), "90");
        assert_eq!(deg(-0.01), "0");
    }
}
