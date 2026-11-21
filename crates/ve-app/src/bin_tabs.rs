//! 素材面板的「音频」「滤镜」标签（剪映的本地版：不接在线素材库）。
//!
//! - 音频：列出所有带声音的素材，「+」把声音放到音频轨末尾——视频文件就是剪映的「音频提取」。
//! - 滤镜：一组调色预设，点一下套到选中的视频片段上（之后还能在「调整」里接着改）。
//! - 调节：「自定义调节」在播放头处加一段调节片段（调节层），给它下面所有轨道的画面调色。

use crate::actions as act;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    ClickEvent, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};
use ve_core::{AssetId, ColorAdjust, FILTERS, Filter, format_timecode};

/// 滤镜卡片上的参照色（肤色、天空、草地、红、黄、灰），套上滤镜后画成色条，一眼看出效果。
const SWATCH: [[f32; 3]; 6] = [
    [0.93, 0.76, 0.65],
    [0.45, 0.70, 0.93],
    [0.38, 0.68, 0.30],
    [0.86, 0.24, 0.22],
    [0.97, 0.84, 0.30],
    [0.55, 0.55, 0.55],
];

/// 0–1 的 RGB → 界面颜色。
fn color(c: [f32; 3]) -> Hsla {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "夹在 0–255"
    )]
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    rgb((byte(c[0]) << 16) | (byte(c[1]) << 8) | byte(c[2])).into()
}

impl Workspace {
    /// 把素材的声音放到音频轨末尾，选中放上去的片段。
    fn append_audio_to_timeline(
        &mut self,
        asset: AssetId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(clip) = self.apply(window, cx, |p| ve_core::edit::append_audio(p, asset)) {
            self.doc.select(clip, false);
            self.after_change(window, cx);
        }
    }

    /// 把滤镜套到选中的视频片段上（一步撤销）。
    fn apply_filter(&mut self, filter: Filter, window: &mut Window, cx: &mut Context<'_, Self>) {
        let clips = self.selected_picture_clips();
        if clips.is_empty() {
            crate::workspace::notify(
                Notification::warning("先在时间轴上选中视频片段，再点滤镜"),
                window,
                cx,
            );
            return;
        }
        self.apply(window, cx, |p| {
            for &c in &clips {
                ve_core::edit::set_adjust(p, c, filter.adjust)?;
            }
            Ok(())
        });
        self.inspector.unbind();
        self.after_change(window, cx);
    }

    /// 选中的视频片段都用着这组调色。
    fn all_selected_use(&self, adjust: &ColorAdjust) -> bool {
        let clips = self.selected_picture_clips();
        !clips.is_empty()
            && clips.iter().all(|&c| {
                self.doc
                    .project
                    .timeline
                    .find_clip(c)
                    .is_some_and(|(_, clip)| clip.adjust == *adjust)
            })
    }

    /// 音频标签里的一行：图标、名字、「音频 / 视频的声音 · 时长」、「+」；双击也放到音频轨。
    fn audio_row(
        a: &ve_core::Asset,
        rate: ve_core::FrameRate,
        cx: &Context<'_, Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let id = a.id;
        let kind = if a.video.is_some() {
            "视频的声音"
        } else {
            "音频"
        };
        h_flex()
            .id(SharedString::from(format!("audio-{}", id.0)))
            .test_support()
            .h(px(44.))
            .px_2()
            .gap_2()
            .rounded(px(4.))
            .bg(cx.theme().muted)
            .border_1()
            .border_color(gpui_kit::transparent_black())
            .hover(|s| s.border_color(palette::accent()))
            .cursor_pointer()
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                if e.click_count() >= 2 {
                    this.append_audio_to_timeline(id, window, cx);
                }
            }))
            .child(
                Icon::new(Lucide::Music)
                    .size(px(18.))
                    .text_color(palette::accent()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .truncate()
                            .child(SharedString::from(a.name.clone())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(SharedString::from(format!(
                                "{kind} · {}",
                                format_timecode(a.duration, rate)
                            ))),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!("add-audio-{}", id.0)))
                    .primary()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip("只把声音放到音频轨末尾")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.append_audio_to_timeline(id, window, cx);
                    })),
            )
    }

    /// 「音频」标签：带声音的素材一行一个。
    pub(crate) fn render_audio_tab(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let rate = self.doc.project.settings.frame_rate;
        let muted = cx.theme().muted_foreground;
        let mut assets: Vec<_> = self
            .doc
            .project
            .assets
            .values()
            .filter(|a| a.audio.is_some())
            .collect();
        assets.sort_by_key(|a| a.id);
        let empty = assets.is_empty();
        let rows = assets.into_iter().map(|a| Self::audio_row(a, rate, cx));
        v_flex()
            .id("bin-audio")
            .test_support()
            .size_full()
            .p_2()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("bin-audio-import")
                            .compact()
                            .icon(IconName::Plus)
                            .label("导入")
                            .on_click(
                                cx.listener(|this, _, w, cx| this.import(&act::Import, w, cx)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("视频文件也在这里：点「+」只放它的声音（音频提取）"),
                    ),
            )
            .child(
                v_flex()
                    .id("audio-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_1()
                    .children(rows)
                    .children(empty.then(|| {
                        div()
                            .pt_4()
                            .text_sm()
                            .text_color(muted)
                            .child("还没有带声音的素材：导入音乐、音效或视频")
                    })),
            )
    }

    /// 「滤镜」标签：预设卡片网格，色条预览效果；选中片段正用着的那个描青边。
    pub(crate) fn render_filter_tab(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let tiles = FILTERS.iter().enumerate().map(|(i, &f)| {
            let on = self.all_selected_use(&f.adjust);
            let card = v_flex()
                .id(SharedString::from(format!("filter-{i}")))
                .test_support();
            crate::timeline::drop::draggable(
                card,
                crate::timeline::drop::BinItem::Filter(i),
                f.name,
            )
            .group("filter-tile")
            .w(px(88.))
            .gap_1()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.apply_filter(f, window, cx)))
            .child(
                h_flex()
                    .w(px(88.))
                    .h(px(50.))
                    .rounded(px(4.))
                    .overflow_hidden()
                    .border_2()
                    .border_color(if on {
                        palette::accent()
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .group_hover("filter-tile", |s| s.border_color(palette::accent()))
                    .children(SWATCH.map(|c| div().flex_1().h_full().bg(color(f.adjust.apply(c))))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(if on { palette::accent() } else { muted })
                    .child(f.name),
            )
        });
        v_flex()
            .id("bin-filters")
            .test_support()
            .p_2()
            .gap_2()
            .child(div().text_xs().text_color(muted).child(
                "在时间轴上选中视频片段，点一个滤镜套上；「原图」去掉。套上后可在属性面板「调整」里接着改",
            ))
            .child(h_flex().flex_wrap().gap_2().children(tiles))
    }

    /// 在播放头处加一段调节片段，选中它、属性面板翻到「调整」。
    fn add_adjustment_at_playhead(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let at = self.playhead_frame();
        if let Some(clip) = self.apply(window, cx, |p| {
            ve_core::edit::add_adjustment(p, at, ve_core::edit::ADJUSTMENT_LENGTH)
        }) {
            self.doc.select(clip, false);
            self.inspector.tab = crate::inspector::InspectorTab::Adjust;
            self.after_change(window, cx);
        }
    }

    /// 「调节」标签：一张「自定义调节」卡片。
    pub(crate) fn render_adjust_bin_tab(cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .id("bin-adjust")
            .test_support()
            .p_2()
            .gap_2()
            .child(div().text_xs().text_color(muted).child(
                "在播放头处加一段调节：给它下面所有轨道的画面一起调色（调节层），在属性面板「调整」里改；也能套滤镜",
            ))
            .child(
                crate::timeline::drop::draggable(
                    div().id("bin-adjust-custom"),
                    crate::timeline::drop::BinItem::Adjustment,
                    "自定义调节",
                )
                    .test_support()
                    .w(px(88.))
                    .h(px(88.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .bg(cx.theme().muted)
                    .border_1()
                    .border_color(gpui_kit::transparent_black())
                    .hover(|s| s.border_color(palette::accent()))
                    .cursor_pointer()
                    .text_sm()
                    .child("自定义调节")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_adjustment_at_playhead(window, cx);
                    })),
            )
    }
}
