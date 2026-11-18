//! 属性面板（剪映式）：顶部一级标签，视频片段「画面 / 音频 / 变速 / 动画 / 调整」，
//! 音频片段「音频 / 变速」，字幕「文本 / 动画」，什么都没选时显示「草稿参数」。
//! 变速、动画、调整先占位，后续补功能。

use crate::chrome;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use ve_core::edit::{GAIN_RANGE_DB, set_fades, set_gain, set_subtitle_text};
use ve_core::{ClipContent, ClipId, Time, format_timecode};

/// 属性面板控件状态。
#[derive(Debug)]
pub struct Inspector {
    gain: Entity<SliderState>,
    fade_in: Entity<InputState>,
    fade_out: Entity<InputState>,
    pub(crate) text: Entity<TextareaState>,
    /// 控件当前对应的片段。
    pub(crate) bound: Option<ClipId>,
    gain_gesture: bool,
    /// 「画面」区。
    pub(crate) transform: crate::inspector_transform::TransformFields,
    /// 「调整」标签。
    pub(crate) adjust: crate::inspector_adjust::AdjustFields,
    /// 用户点选的一级标签（当前片段没有这个标签时显示第一个）。
    pub(crate) tab: InspectorTab,
    /// 「动画」标签：在看出场（否则入场）。
    pub(crate) anim_out: bool,
    /// 「动画」标签：预设时长（秒）。
    pub(crate) anim_secs: f64,
}

/// 属性面板的一级标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InspectorTab {
    /// 画面：位置大小、旋转、裁剪、不透明度、关键帧。
    #[default]
    Picture,
    /// 音频：音量、淡入淡出。
    Audio,
    /// 变速（后续）。
    Speed,
    /// 动画（后续）。
    Animation,
    /// 调整：调色（后续）。
    Adjust,
    /// 文本：字幕时间和样式。
    Text,
    /// 字幕：字幕列表编辑器（一览、改字、插入、删除）。
    Subtitles,
}

impl InspectorTab {
    /// 标签文字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Picture => "画面",
            Self::Audio => "音频",
            Self::Speed => "变速",
            Self::Animation => "动画",
            Self::Adjust => "调整",
            Self::Text => "文本",
            Self::Subtitles => "字幕",
        }
    }

    const fn id(self) -> &'static str {
        match self {
            Self::Picture => "insp-tab-picture",
            Self::Audio => "insp-tab-audio",
            Self::Speed => "insp-tab-speed",
            Self::Animation => "insp-tab-animation",
            Self::Adjust => "insp-tab-adjust",
            Self::Text => "insp-tab-text",
            Self::Subtitles => "insp-tab-subtitles",
        }
    }

    /// 某类片段有哪些标签（和剪映一致）。
    #[must_use]
    pub const fn for_clip(video: bool, subtitle: bool) -> &'static [Self] {
        if subtitle {
            &[Self::Subtitles, Self::Text, Self::Animation]
        } else if video {
            &[
                Self::Picture,
                Self::Audio,
                Self::Speed,
                Self::Animation,
                Self::Adjust,
            ]
        } else {
            &[Self::Audio, Self::Speed]
        }
    }

    /// 在 `available` 里实际显示哪个：点选过的那个还在就用它，否则第一个。
    #[must_use]
    pub fn effective(self, available: &[Self]) -> Option<Self> {
        if available.contains(&self) {
            Some(self)
        } else {
            available.first().copied()
        }
    }
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
        let (adjust, adjust_subs) = crate::inspector_adjust::AdjustFields::new(window, cx);
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
        subs.extend(adjust_subs);
        (
            Self {
                gain,
                fade_in,
                fade_out,
                text,
                bound: None,
                gain_gesture: false,
                tab: InspectorTab::default(),
                anim_out: false,
                anim_secs: 0.5,
                transform,
                adjust,
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
                if let Some((_, t, _)) = self.bound_video_clip() {
                    self.fill_transform(&t, window, cx);
                    self.fill_adjust(&clip.adjust, window, cx);
                }
            }
            ClipContent::Subtitle { text } => {
                let text = text.clone();
                self.inspector
                    .text
                    .update(cx, |s, cx| s.set_value(text, window, cx));
            }
        }
    }

    /// 什么都没选时：草稿参数（剪映）。比例、分辨率、帧率可以直接点选修改，其余只读。
    fn render_draft_params(&self, cx: &Context<'_, Self>) -> gpui_kit::AnyElement {
        use ve_core::FrameRate;
        let muted = cx.theme().muted_foreground;
        let s = self.doc.project.settings;
        let rate = s.frame_rate;
        let row = |label: &'static str, value: String| {
            h_flex()
                .justify_between()
                .text_sm()
                .child(div().text_color(muted).child(label))
                .child(SharedString::from(value))
        };
        let group = |label: &'static str, chips: Vec<gpui_kit::AnyElement>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().text_color(muted).child(label))
                .child(h_flex().flex_wrap().gap_1().children(chips))
        };
        let current_ratio = crate::preview::aspect_label(s.width, s.height);
        let ratios = crate::preview::RATIOS
            .iter()
            .filter(|(_, _, r)| r.is_some())
            .map(|&(id, label, ratio)| {
                chrome::chip(format!("draft-{id}"), label, label == current_ratio, cx)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply(window, cx, |p| ve_core::edit::set_aspect(p, ratio));
                    }))
                    .into_any_element()
            })
            .collect();
        let short = s.width.min(s.height);
        let resolutions = [("720P", 720), ("1080P", 1080), ("2K", 1440), ("4K", 2160)]
            .into_iter()
            .map(|(label, px)| {
                chrome::chip(format!("draft-res-{px}"), label, short == px, cx)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply(window, cx, |p| ve_core::edit::set_resolution(p, px));
                    }))
                    .into_any_element()
            })
            .collect();
        let rates = [24, 25, 30, 50, 60]
            .into_iter()
            .map(|fps| {
                let r = FrameRate::new(fps, 1);
                chrome::chip(format!("draft-fps-{fps}"), fps.to_string(), rate == r, cx)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply(window, cx, |p| ve_core::edit::set_frame_rate(p, r));
                    }))
                    .into_any_element()
            })
            .collect();
        let body = v_flex()
            .gap_3()
            .child(row("名称", self.doc.name()))
            .child(row("分辨率", format!("{}×{}", s.width, s.height)))
            .child(row("帧率", format!("{:.2} 帧/秒", rate.as_f64())))
            .child(group("比例", ratios))
            .child(group("分辨率（短边）", resolutions))
            .child(group("帧率", rates))
            .child(row(
                "总长",
                format_timecode(self.doc.project.timeline.duration(), rate),
            ))
            .child(row("素材", self.doc.project.assets.len().to_string()))
            .child(
                div()
                    .pt_2()
                    .text_xs()
                    .text_color(muted)
                    .child("单击时间轴上的片段，查看和修改它的属性"),
            );
        inspector_frame(
            h_flex().child(chrome::text_tab("insp-tab-draft", "草稿参数", true, cx)),
            body,
        )
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
        let Some(c) = clip else {
            return self.render_draft_params(cx);
        };
        let video = self.bound_video_clip().is_some();
        let subtitle = matches!(c.content, ClipContent::Subtitle { .. });
        let available = InspectorTab::for_clip(video, subtitle);
        let current = self.inspector.tab.effective(available);
        let tabs = h_flex().children(available.iter().map(|&tab| {
            chrome::text_tab(tab.id(), tab.label(), Some(tab) == current, cx).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.inspector.tab = tab;
                    cx.notify();
                },
            ))
        }));
        let timing = v_flex()
            .gap_2()
            .child(row("开始", format_timecode(c.start, rate)))
            .child(row("长度", format_timecode(c.duration, rate)));
        let body = match (current, &c.content) {
            (Some(InspectorTab::Picture), ClipContent::Media { asset, .. }) => {
                let name = self
                    .doc
                    .project
                    .assets
                    .get(asset)
                    .map(|a| a.name.clone())
                    .unwrap_or_default();
                v_flex()
                    .gap_3()
                    .child(chrome::segments(&[("基础", true)], cx))
                    .child(div().text_sm().truncate().child(SharedString::from(name)))
                    .child(timing)
                    .children(self.render_transform_section(cx))
                    .into_any_element()
            }
            (Some(InspectorTab::Audio), ClipContent::Media { .. }) => v_flex()
                .id("insp-audio")
                .test_support()
                .gap_2()
                .child(row("音量", format!("{:+.1} dB", c.gain_db)))
                .child(Slider::new(&self.inspector.gain))
                .child(div().text_sm().text_color(muted).child("淡入（秒）"))
                .child(Input::new(&self.inspector.fade_in).id("fade-in"))
                .child(div().text_sm().text_color(muted).child("淡出（秒）"))
                .child(Input::new(&self.inspector.fade_out))
                .into_any_element(),
            (Some(InspectorTab::Subtitles), ClipContent::Subtitle { .. }) => {
                self.render_subtitle_list(cx)
            }
            (Some(InspectorTab::Text), ClipContent::Subtitle { .. }) => v_flex()
                .gap_3()
                .child(timing)
                .child(self.render_text_style_tab(cx))
                .into_any_element(),
            (Some(InspectorTab::Speed), ClipContent::Media { .. }) => {
                self.render_speed_tab(&c, cx).into_any_element()
            }
            (Some(InspectorTab::Animation), ClipContent::Media { .. }) if video => {
                self.render_motion_tab(cx).into_any_element()
            }
            (Some(InspectorTab::Animation), _) => {
                chrome::coming_soon("文字动画：入场、出场、循环", cx).into_any_element()
            }
            (Some(InspectorTab::Adjust), _) => self.render_adjust_tab(cx).into_any_element(),
            _ => div().into_any_element(),
        };
        inspector_frame(tabs, body)
    }
}

/// 属性面板外框：顶部标签栏（带分隔线），下面可滚动的内容。
fn inspector_frame(tabs: impl IntoElement, body: impl IntoElement) -> gpui_kit::AnyElement {
    v_flex()
        .id("inspector")
        .test_support()
        .size_full()
        .child(
            h_flex()
                .flex_none()
                .px_2()
                .border_b_1()
                .border_color(palette::divider())
                .child(tabs),
        )
        .child(
            v_flex()
                .id("inspector-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_3()
                .child(body),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_depend_on_clip_kind_and_fall_back_to_first() {
        let video = InspectorTab::for_clip(true, false);
        assert_eq!(video[0], InspectorTab::Picture);
        assert_eq!(
            InspectorTab::for_clip(false, false),
            [InspectorTab::Audio, InspectorTab::Speed]
        );
        let sub = InspectorTab::for_clip(false, true);
        assert_eq!(
            sub,
            [
                InspectorTab::Subtitles,
                InspectorTab::Text,
                InspectorTab::Animation
            ]
        );
        // 点过「音频」：换到字幕时没有这个标签，退回第一个；回到视频又是「音频」
        assert_eq!(
            InspectorTab::Audio.effective(sub),
            Some(InspectorTab::Subtitles)
        );
        assert_eq!(
            InspectorTab::Audio.effective(video),
            Some(InspectorTab::Audio)
        );
        assert_eq!(InspectorTab::Audio.effective(&[]), None);
        assert_eq!(InspectorTab::Adjust.label(), "调整");
    }
}
