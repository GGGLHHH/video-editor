//! 属性面板：选中媒体片段时调音量、淡入淡出（视频轨片段还有「画面」区）；选中字幕时改文字。

use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, Subscription, Window, div, px,
};
use ve_core::edit::{GAIN_RANGE_DB, set_fades, set_gain, set_subtitle_text};
use ve_core::{ClipContent, ClipId, Time, format_timecode};

/// 属性面板控件状态。
#[derive(Debug)]
pub struct Inspector {
    gain: Entity<SliderState>,
    fade_in: Entity<InputState>,
    fade_out: Entity<InputState>,
    text: Entity<TextareaState>,
    /// 控件当前对应的片段。
    pub(crate) bound: Option<ClipId>,
    gain_gesture: bool,
    /// 「画面」区。
    pub(crate) transform: crate::inspector_transform::TransformFields,
}

fn secs_text(t: Time) -> String {
    format!("{:.2}", t.as_secs_f64())
}

fn parse_secs(s: &str) -> Option<Time> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(Time::from_secs_f64)
}

impl Inspector {
    /// 创建控件并订阅事件。
    pub fn new(window: &mut Window, cx: &mut Context<'_, Workspace>) -> (Self, Vec<Subscription>) {
        let gain = cx.new(|_| {
            SliderState::new()
                .min(GAIN_RANGE_DB.0)
                .max(GAIN_RANGE_DB.1)
                .step(0.5)
                .default_value(0.0)
        });
        let fade_in = cx.new(|cx| InputState::new(window, cx).placeholder("0.00"));
        let fade_out = cx.new(|cx| InputState::new(window, cx).placeholder("0.00"));
        let text = cx.new(|cx| TextareaState::new(window, cx).rows(4));
        let (transform, transform_subs) =
            crate::inspector_transform::TransformFields::new(window, cx);
        let mut subs = vec![
            cx.subscribe_in(
                &gain,
                window,
                |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                    this.on_gain(e, window, cx);
                },
            ),
            cx.subscribe_in(
                &fade_in,
                window,
                |this: &mut Workspace, _, e: &InputEvent, window, cx| {
                    if matches!(e, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.commit_fades(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &fade_out,
                window,
                |this: &mut Workspace, _, e: &InputEvent, window, cx| {
                    if matches!(e, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.commit_fades(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &text,
                window,
                |this: &mut Workspace, _, e: &InputEvent, window, cx| {
                    if matches!(
                        e,
                        InputEvent::Blur
                            | InputEvent::PressEnter {
                                secondary: true,
                                ..
                            }
                    ) {
                        this.commit_subtitle(window, cx);
                    }
                },
            ),
        ];
        subs.extend(transform_subs);
        (
            Self {
                gain,
                fade_in,
                fade_out,
                text,
                bound: None,
                gain_gesture: false,
                transform,
            },
            subs,
        )
    }

    /// 解除绑定：下次同步时按当前选择重新填控件。
    pub const fn unbind(&mut self) {
        self.bound = None;
    }
}

impl Workspace {
    fn on_gain(&mut self, e: &SliderEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(clip) = self.inspector.bound else {
            return;
        };
        match e {
            SliderEvent::Change(v) => {
                if !self.inspector.gain_gesture {
                    self.doc.begin_gesture();
                    self.inspector.gain_gesture = true;
                }
                let db = v.start();
                let _ = self.doc.edit(|p| set_gain(p, clip, db));
                self.after_change(window, cx);
            }
            SliderEvent::Release(_) => {
                if self.inspector.gain_gesture {
                    self.doc.end_gesture();
                    self.inspector.gain_gesture = false;
                }
            }
        }
    }

    fn commit_fades(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(clip) = self.inspector.bound else {
            return;
        };
        let fade_in = parse_secs(&self.inspector.fade_in.read(cx).value()).unwrap_or(Time::ZERO);
        let fade_out = parse_secs(&self.inspector.fade_out.read(cx).value()).unwrap_or(Time::ZERO);
        self.apply(window, cx, |p| set_fades(p, clip, fade_in, fade_out));
        self.inspector.unbind();
        self.sync_inspector(window, cx);
    }

    fn commit_subtitle(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(clip) = self.inspector.bound else {
            return;
        };
        let text = self.inspector.text.read(cx).value().to_string();
        let current = self
            .doc
            .project
            .timeline
            .find_clip(clip)
            .and_then(|(_, c)| c.subtitle_text().map(str::to_owned));
        if current.is_some_and(|c| c != text) {
            self.apply(window, cx, |p| set_subtitle_text(p, clip, text));
        }
    }

    /// 换绑之前：把控件里还没提交的改动（字幕文字、淡入淡出）写回原片段。
    /// 点别的片段或空白处时，失焦提交可能晚于选择变化，不先提交就会丢。
    fn commit_pending(&mut self, cx: &Context<'_, Self>) {
        let Some(clip) = self.inspector.bound else {
            return;
        };
        let Some((_, current)) = self.doc.project.timeline.find_clip(clip) else {
            return;
        };
        let current = (**current).clone();
        let edited = match &current.content {
            ClipContent::Subtitle { text } => {
                let typed = self.inspector.text.read(cx).value().to_string();
                (typed != *text).then(|| self.doc.edit(|p| set_subtitle_text(p, clip, typed)))
            }
            ClipContent::Media { .. } => {
                let fade_in =
                    parse_secs(&self.inspector.fade_in.read(cx).value()).unwrap_or(current.fade_in);
                let fade_out = parse_secs(&self.inspector.fade_out.read(cx).value())
                    .unwrap_or(current.fade_out);
                (fade_in != current.fade_in || fade_out != current.fade_out)
                    .then(|| self.doc.edit(|p| set_fades(p, clip, fade_in, fade_out)))
            }
        };
        let transformed = self.commit_pending_transform(cx);
        if edited.is_some() || transformed {
            self.sync_engine();
        }
    }

    /// 选择变了就把片段的值填进控件。
    pub(crate) fn sync_inspector(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let selected = match self.doc.selection() {
            [one] => Some(*one),
            _ => None,
        };
        if selected == self.inspector.bound {
            return;
        }
        self.commit_pending(cx);
        self.inspector.bound = selected;
        let Some(clip) = selected
            .and_then(|c| self.doc.project.timeline.find_clip(c))
            .map(|(_, c)| (**c).clone())
        else {
            return;
        };
        match &clip.content {
            ClipContent::Media { .. } => {
                self.inspector
                    .gain
                    .update(cx, |s, cx| s.set_value(clip.gain_db, window, cx));
                self.inspector
                    .fade_in
                    .update(cx, |s, cx| s.set_value(secs_text(clip.fade_in), window, cx));
                self.inspector.fade_out.update(cx, |s, cx| {
                    s.set_value(secs_text(clip.fade_out), window, cx);
                });
                self.fill_transform(&clip.transform, window, cx);
            }
            ClipContent::Subtitle { text } => {
                let text = text.clone();
                self.inspector
                    .text
                    .update(cx, |s, cx| s.set_value(text, window, cx));
            }
        }
    }

    pub(crate) fn render_inspector(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let rate = self.doc.project.settings.frame_rate;
        let row = |label: &'static str, value: String| {
            h_flex()
                .justify_between()
                .text_sm()
                .child(div().text_color(muted).child(label))
                .child(SharedString::from(value))
        };
        let clip = self
            .inspector
            .bound
            .and_then(|c| self.doc.project.timeline.find_clip(c))
            .map(|(_, c)| (**c).clone());
        let body = match clip {
            None => {
                let s = self.doc.project.settings;
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("未选中片段（单击时间轴上的片段）"),
                    )
                    .child(row("画面", format!("{}×{}", s.width, s.height)))
                    .child(row("帧率", format!("{:.3}", s.frame_rate.as_f64())))
                    .child(row(
                        "总长",
                        format_timecode(self.doc.project.timeline.duration(), rate),
                    ))
                    .child(row("素材", self.doc.project.assets.len().to_string()))
            }
            Some(c) => {
                let common = v_flex()
                    .gap_2()
                    .child(row("开始", format_timecode(c.start, rate)))
                    .child(row("长度", format_timecode(c.duration, rate)));
                match &c.content {
                    ClipContent::Media { asset, .. } => {
                        let name = self
                            .doc
                            .project
                            .assets
                            .get(asset)
                            .map(|a| a.name.clone())
                            .unwrap_or_default();
                        common
                            .child(div().text_sm().truncate().child(SharedString::from(name)))
                            .child(row("音量", format!("{:+.1} dB", c.gain_db)))
                            .child(Slider::new(&self.inspector.gain))
                            .child(div().text_sm().text_color(muted).child("淡入（秒）"))
                            .child(Input::new(&self.inspector.fade_in).id("fade-in"))
                            .child(div().text_sm().text_color(muted).child("淡出（秒）"))
                            .child(Input::new(&self.inspector.fade_out))
                            .children(self.render_transform_section(cx))
                    }
                    ClipContent::Subtitle { .. } => common
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("字幕文字（失焦或 Ctrl+Enter 生效）"),
                        )
                        .child(
                            div()
                                .id("subtitle-text")
                                .test_support()
                                .child(Textarea::new(&self.inspector.text)),
                        ),
                }
            }
        };
        v_flex()
            .id("inspector")
            .test_support()
            .size_full()
            .p_3()
            .gap_3()
            .border_l_1()
            .border_color(cx.theme().border)
            .child(div().text_sm().child("属性"))
            .child(body)
            .child(div().h(px(1.)))
    }
}
