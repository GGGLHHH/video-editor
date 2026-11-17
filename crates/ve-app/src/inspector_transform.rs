//! 属性面板的「画面」区：视频轨片段的位置、缩放、旋转、裁剪、不透明度。

use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, Subscription, Window, div,
};
use ve_core::edit::set_transform;
use ve_core::{ClipId, Crop, TrackKind, Transform, wrap_degrees};

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
    /// 上次填进输入框的文字：没被改过的字段提交时沿用片段的精确值，
    /// 不会把四舍五入后的显示值写回去（预览上拖出来的值精度比显示的高）。
    shown: [String; 8],
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
    };
    t.is_valid().then_some(t)
}

impl TransformFields {
    /// 创建控件并订阅事件。
    pub fn new(window: &mut Window, cx: &mut Context<'_, Workspace>) -> (Self, Vec<Subscription>) {
        let mut input = || cx.new(|cx| InputState::new(window, cx).placeholder("0"));
        let (x, y, scale, rotation) = (input(), input(), input(), input());
        let crop = [input(), input(), input(), input()];
        let opacity = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(100.0)
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
        (
            Self {
                x,
                y,
                scale,
                rotation,
                crop,
                opacity,
                opacity_gesture: false,
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
    /// 属性面板绑定的片段在视频轨上时，返回它和它的画面属性。
    fn bound_video_clip(&self) -> Option<(ClipId, Transform)> {
        let clip = self.inspector.bound?;
        let (track, c) = self.doc.project.timeline.find_clip(clip)?;
        let kind = self.doc.project.timeline.track(track)?.kind;
        (kind == TrackKind::Video && c.asset().is_some()).then_some((clip, c.transform))
    }

    /// 把片段的画面属性填进控件。
    pub(crate) fn fill_transform(
        &mut self,
        t: &Transform,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        // 和 `texts` 同一个顺序：X、Y、缩放、裁剪左上右下、旋转
        let shown = [
            pct(t.x),
            pct(t.y),
            pct(t.scale),
            pct(t.crop.left),
            pct(t.crop.top),
            pct(t.crop.right),
            pct(t.crop.bottom),
            deg(t.rotation),
        ];
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
    }

    /// 输入框改完（回车 / 失焦）：按各字段写回；无效时恢复显示原值。
    fn commit_transform(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((clip, current)) = self.bound_video_clip() else {
            return;
        };
        if let Some(t) = transform_from_text(&current, &self.inspector.transform.edited_texts(cx))
            && t != current
        {
            self.apply(window, cx, |p| set_transform(p, clip, t));
        }
        // 夹过范围或被拒绝时，显示实际生效的值
        if let Some((_, now)) = self.bound_video_clip() {
            self.fill_transform(&now, window, cx);
        }
    }

    /// 换绑前把输入框里还没提交的画面属性写回原片段，返回是否改了。
    pub(crate) fn commit_pending_transform(&mut self, cx: &Context<'_, Self>) -> bool {
        let Some((clip, current)) = self.bound_video_clip() else {
            return false;
        };
        match transform_from_text(&current, &self.inspector.transform.edited_texts(cx)) {
            Some(t) if t != current => self.doc.edit(|p| set_transform(p, clip, t)).is_ok(),
            _ => false,
        }
    }

    fn on_opacity(&mut self, e: &SliderEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((clip, current)) = self.bound_video_clip() else {
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
                let _ = self.doc.edit(|p| set_transform(p, clip, t));
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
        let Some((clip, _)) = self.bound_video_clip() else {
            return;
        };
        self.apply(window, cx, |p| set_transform(p, clip, Transform::default()));
        self.fill_transform(&Transform::default(), window, cx);
    }

    /// 「画面」区（只在选中视频轨上的片段时有）。
    pub(crate) fn render_transform_section(
        &self,
        cx: &Context<'_, Self>,
    ) -> Option<impl IntoElement> {
        let (_, t) = self.bound_video_clip()?;
        let muted = cx.theme().muted_foreground;
        let f = &self.inspector.transform;
        let field = |id: &'static str, label: &'static str, state: &Entity<InputState>| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_xs().text_color(muted).child(label))
                .child(div().id(id).test_support().child(Input::new(state)))
        };
        Some(
            v_flex()
                .gap_2()
                .pt_2()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(
                    h_flex()
                        .justify_between()
                        .child(div().text_sm().child("画面"))
                        .child(
                            Button::new("tf-reset")
                                .ghost()
                                .compact()
                                .label("重置")
                                .tooltip("恢复铺满画面、居中、不旋转、不裁剪")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.reset_transform(window, cx);
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(field("tf-x", "X（%）", &f.x))
                        .child(field("tf-y", "Y（%）", &f.y))
                        .child(field("tf-scale", "缩放（%）", &f.scale))
                        .child(field("tf-rotation", "旋转（°）", &f.rotation)),
                )
                .child(div().text_xs().text_color(muted).child("裁剪（%）"))
                .child(
                    h_flex()
                        .gap_2()
                        .child(field("tf-crop-l", "左", &f.crop[0]))
                        .child(field("tf-crop-t", "上", &f.crop[1]))
                        .child(field("tf-crop-r", "右", &f.crop[2]))
                        .child(field("tf-crop-b", "下", &f.crop[3])),
                )
                .child(
                    h_flex()
                        .justify_between()
                        .text_sm()
                        .child(div().text_color(muted).child("不透明度"))
                        .child(SharedString::from(format!("{}%", pct(t.opacity)))),
                )
                .child(
                    div()
                        .id("tf-opacity")
                        .test_support()
                        .child(Slider::new(&f.opacity)),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(v: [&str; 8]) -> [String; 8] {
        v.map(str::to_owned)
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
