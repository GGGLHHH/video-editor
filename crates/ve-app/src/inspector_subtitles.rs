//! 属性面板「字幕」标签：字幕列表编辑器（剪映）。全部字幕按顺序一行一条；点一行选中它、
//! 播放头跳过去，选中的那行展开成文字框直接改；每行「＋」在后面插一条、「🗑」删掉；
//! 播放头所在的那条用主色青标出来。

use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use ve_core::{ClipId, Time, TrackKind};

/// 「＋」插入的新字幕长度。
const NEW_SUBTITLE: Time = Time::from_secs(2);

impl Workspace {
    /// 选中一条字幕并把播放头放到它开头。
    fn pick_subtitle(
        &mut self,
        clip: ClipId,
        start: Time,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.doc.select(clip, false);
        self.seek(start, cx);
        self.after_change(window, cx);
    }

    /// 在这条字幕后面插一条：紧接着它的结尾（和下一条撞上时提示）。
    fn insert_subtitle_after(
        &mut self,
        end: Time,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(clip) = self.apply(window, cx, |p| {
            ve_core::edit::add_caption(p, end, NEW_SUBTITLE, "新字幕".into())
        }) {
            self.pick_subtitle(clip, end, window, cx);
        }
    }

    /// 字幕列表列的片段：字幕轨（最下面那条文字轨）上的，上面叠放的标题不算。
    fn caption_clips(&self) -> Vec<std::sync::Arc<ve_core::Clip>> {
        self.doc
            .project
            .timeline
            .tracks
            .iter()
            .rfind(|t| t.kind == TrackKind::Subtitle)
            .map(|t| t.clips.clone())
            .unwrap_or_default()
    }

    pub(crate) fn render_subtitle_list(&self, cx: &Context<'_, Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let current = self.inspector.bound;
        let playhead = self.playhead;
        let clips = self.caption_clips();
        let rows = clips.iter().enumerate().map(|(i, c)| {
            let (id, start, end) = (c.id, c.start, c.end());
            let text = c.subtitle_text().unwrap_or_default().replace('\n', " ");
            let selected = current == Some(id);
            let playing = start <= playhead && playhead < end;
            let line = h_flex()
                .id(SharedString::from(format!("sub-row-{i}")))
                .test_support()
                .gap_2()
                .px_1()
                .py_1()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(cx.theme().accent))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.pick_subtitle(id, start, window, cx);
                }))
                .child(
                    div()
                        .w(px(22.))
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(SharedString::from((i + 1).to_string())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(if playing {
                            palette::accent()
                        } else {
                            cx.theme().foreground
                        })
                        .child(SharedString::from(text)),
                )
                .child(
                    Button::new(SharedString::from(format!("sub-add-{i}")))
                        .ghost()
                        .xsmall()
                        .label("＋")
                        .tooltip("在这条后面插一条")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            // 按钮在行里：别让点击再冒到行上（那会选回这一行、播放头跳回去）
                            cx.stop_propagation();
                            this.insert_subtitle_after(end, window, cx);
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("sub-del-{i}")))
                        .ghost()
                        .xsmall()
                        .label("🗑")
                        .tooltip("删除这条")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.apply(window, cx, |p| ve_core::edit::delete_clips(p, &[id]));
                        })),
                );
            let editor = selected.then(|| {
                v_flex()
                    .pl(px(30.))
                    .gap_1()
                    .child(
                        div()
                            .id("subtitle-text")
                            .test_support()
                            .child(Textarea::new(&self.inspector.text)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("失焦或 Ctrl+Enter 生效"),
                    )
            });
            v_flex()
                .when_selected_row(selected, cx)
                .child(line)
                .children(editor)
                .into_any_element()
        });
        v_flex()
            .id("subtitle-list")
            .test_support()
            .gap_0p5()
            .children(rows)
            .into_any_element()
    }
}

/// 选中的那行有浅底色。
trait RowStyle: gpui_kit::Styled + Sized {
    fn when_selected_row(self, selected: bool, cx: &gpui_kit::App) -> Self {
        if selected {
            self.rounded(px(4.)).bg(cx.theme().muted).pb_1()
        } else {
            self
        }
    }
}

impl<T: gpui_kit::Styled> RowStyle for T {}
