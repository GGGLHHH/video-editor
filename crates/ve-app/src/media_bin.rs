//! 素材面板（剪映式「资源区」）：一级标签 + 二级栏 + 网格卡片；导入（对话框或拖文件进窗口），
//! 双击 / 悬停「+」放进时间轴、拖到时间轴。

use crate::actions as act;
use crate::chrome;
use crate::import::{Analysis, analyze, probe_asset};
use crate::preview::to_render_image;
use crate::theme::palette;
use crate::workspace::{MediaThumbs, Workspace};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, ClickEvent, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, img, px,
};
use std::path::PathBuf;
use std::sync::Arc;
use ve_core::{Asset, AssetId, format_timecode};

/// 从素材库拖到时间轴的内容。
#[derive(Debug, Clone)]
pub struct AssetDrag {
    /// 素材。
    pub asset: AssetId,
    /// 显示名。
    pub name: SharedString,
}

impl Render for AssetDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(cx.theme().primary)
            .text_color(cx.theme().primary_foreground)
            .text_sm()
            .child(self.name.clone())
    }
}

fn thumbs_of(path: PathBuf, analysis: Analysis) -> MediaThumbs {
    MediaThumbs {
        path,
        thumbnails: analysis
            .thumbnails
            .iter()
            .filter_map(|f| to_render_image(f))
            .collect(),
        peaks: Arc::new(analysis.peaks),
    }
}

impl Workspace {
    pub(crate) fn import(
        &mut self,
        _: &act::Import,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("导入素材".into()),
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ignored =
                    this.update_in(cx, |this, window, cx| this.import_paths(paths, window, cx));
            }
        }));
    }

    /// 导入一批文件：后台探测，成功的加入素材库，失败的弹提示；随后后台算缩略图和波形。
    pub fn import_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.import_with(paths, false, window, cx);
    }

    /// 同 [`Self::import_paths`]；`append` 为真时导入后依次放到时间轴末尾。
    pub fn import_with(
        &mut self,
        paths: Vec<PathBuf>,
        append: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        self.importing += paths.len();
        cx.notify();
        let probing = cx.background_spawn(async move {
            paths
                .iter()
                .map(|p| probe_asset(p))
                .collect::<Vec<Result<Asset, String>>>()
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let results = probing.await;
            let _ignored = this.update_in(cx, |this, window, cx| {
                this.importing = this.importing.saturating_sub(results.len());
                let mut added = Vec::new();
                for r in results {
                    match r {
                        Ok(asset) => {
                            let id = this.doc.add_asset(asset);
                            if append {
                                let _ = this.doc.edit(|p| ve_core::edit::append_asset(p, id));
                            }
                            if let Some(a) = this.doc.project.assets.get(&id) {
                                added.push(a.clone());
                            }
                        }
                        Err(message) => {
                            crate::workspace::notify(Notification::warning(message), window, cx);
                        }
                    }
                }
                this.after_change(window, cx);
                this.analyze_assets(added, window, cx);
            });
        }));
    }

    /// 后台算缩略图和波形。
    pub(crate) fn analyze_assets(
        &mut self,
        assets: Vec<Asset>,
        window: &Window,
        cx: &Context<'_, Self>,
    ) {
        for asset in assets {
            let (id, path) = (asset.id, asset.path.clone());
            let work = cx.background_spawn(async move { analyze(&asset) });
            self.tasks.push(cx.spawn_in(window, async move |this, cx| {
                let analysis = work.await;
                let _ignored = this.update_in(cx, |this, window, cx| {
                    this.accept_analysis(id, &path, analysis, window, cx);
                });
            }));
        }
        self.tasks.retain(|t| !t.is_ready());
    }

    /// 存入分析结果；该编号的素材已不是这个文件（撤销后再导入、换了工程）时丢弃，返回是否存入。
    pub fn accept_analysis(
        &mut self,
        id: AssetId,
        path: &std::path::Path,
        analysis: Analysis,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self
            .doc
            .project
            .assets
            .get(&id)
            .is_none_or(|a| a.path != path)
        {
            return false;
        }
        if let Some(old) = self
            .media
            .insert(id, thumbs_of(path.to_path_buf(), analysis))
        {
            for image in old.thumbnails {
                cx.drop_image(image, Some(window));
            }
        }
        cx.notify();
        true
    }

    /// 某素材已分析的文件（测试与调试用）。
    #[must_use]
    pub fn media_path(&self, id: AssetId) -> Option<&std::path::Path> {
        self.media.get(&id).map(|m| m.path.as_path())
    }

    fn append_to_timeline(
        &mut self,
        asset: AssetId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(clip) = self.apply(window, cx, |p| ve_core::edit::append_asset(p, asset)) {
            self.doc.select(clip, false);
            self.after_change(window, cx);
        }
    }

    fn render_asset_card(
        &self,
        a: &Asset,
        used: bool,
        rate: ve_core::FrameRate,
        cx: &Context<'_, Self>,
    ) -> impl IntoElement {
        let id = a.id;
        let thumb = self
            .media
            .get(&id)
            .and_then(|m| m.thumbnails.first().cloned());
        let audio_only = a.video.is_none();
        let drag = AssetDrag {
            asset: id,
            name: a.name.clone().into(),
        };
        let chip = |text: SharedString| {
            div()
                .px_1()
                .rounded(px(3.))
                .bg(gpui_kit::rgba(0x0000_00a0))
                .text_color(gpui_kit::white())
                .text_size(px(10.))
                .child(text)
        };
        v_flex()
            .id(SharedString::from(format!("asset-{}", id.0)))
            .test_support()
            .group("asset-card")
            .w(px(CARD_W))
            .gap_1()
            .cursor_pointer()
            .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                if e.click_count() >= 2 {
                    this.append_to_timeline(id, window, cx);
                }
            }))
            .child(
                div()
                    .relative()
                    .w(px(CARD_W))
                    .h(px(CARD_W * 9.0 / 16.0))
                    .rounded(px(4.))
                    .overflow_hidden()
                    .bg(cx.theme().muted)
                    .border_1()
                    .border_color(gpui_kit::transparent_black())
                    .group_hover("asset-card", |s| s.border_color(palette::accent()))
                    .children(thumb.map(|t| img(t).size_full()))
                    .children(audio_only.then(|| {
                        h_flex().size_full().justify_center().child(
                            Icon::new(Lucide::Music)
                                .size(px(22.))
                                .text_color(palette::accent()),
                        )
                    }))
                    .children(used.then(|| {
                        div()
                            .absolute()
                            .top(px(3.))
                            .left(px(3.))
                            .child(chip("已添加".into()))
                    }))
                    .child(
                        div()
                            .absolute()
                            .bottom(px(3.))
                            .right(px(3.))
                            .child(chip(format_timecode(a.duration, rate).into())),
                    )
                    .child(
                        // 悬停时右上角出现「+」：放到时间轴末尾
                        div()
                            .absolute()
                            .top(px(2.))
                            .right(px(2.))
                            .opacity(0.)
                            .group_hover("asset-card", |s| s.opacity(1.))
                            .child(
                                Button::new(SharedString::from(format!("add-{}", id.0)))
                                    .primary()
                                    .xsmall()
                                    .icon(IconName::Plus)
                                    .tooltip("放到时间轴末尾")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.append_to_timeline(id, window, cx);
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .truncate()
                    .child(SharedString::from(a.name.clone())),
            )
    }

    /// 素材标签的内容：顶部「导入」按钮，下面缩略图网格；空工程时一个大的虚线框。
    fn render_media_tab(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let rate = self.doc.project.settings.frame_rate;
        let used: std::collections::HashSet<AssetId> = self
            .doc
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().filter_map(|c| c.asset()))
            .collect();
        let mut assets: Vec<&Asset> = self.doc.project.assets.values().collect();
        assets.sort_by_key(|a| a.id);
        let cards = assets
            .into_iter()
            .map(|a| self.render_asset_card(a, used.contains(&a.id), rate, cx));
        let empty = self.doc.project.assets.is_empty();
        let muted = cx.theme().muted_foreground;
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(px(36.))
                    .flex_none()
                    .px_2()
                    .gap_2()
                    .child(
                        Button::new("bin-import")
                            .compact()
                            .icon(IconName::Plus)
                            .label("导入")
                            .on_click(
                                cx.listener(|this, _, w, cx| this.import(&act::Import, w, cx)),
                            ),
                    )
                    .children(
                        (self.importing > 0)
                            .then(|| div().text_xs().text_color(muted).child("导入中…")),
                    ),
            )
            .child(
                div()
                    .id("media-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .child(h_flex().flex_wrap().items_start().gap_3().children(cards))
                    .children(empty.then(|| {
                        v_flex()
                            .id("bin-drop-hint")
                            .h(px(150.))
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .rounded(px(6.))
                            .border_1()
                            .border_dashed()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().muted)
                            .cursor_pointer()
                            .on_click(
                                cx.listener(|this, _, w, cx| this.import(&act::Import, w, cx)),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .text_sm()
                                    .child(
                                        Icon::new(Lucide::CirclePlus).text_color(palette::accent()),
                                    )
                                    .child("导入"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("视频、音频、图片，也可以直接拖进窗口"),
                            )
                    })),
            )
    }

    /// 文本标签：「默认文本」（在播放头处加一条字幕）和「导入本地字幕」两个大方块。
    fn render_text_tab(cx: &Context<'_, Self>) -> impl IntoElement {
        let tile = |id: &'static str, icon: Lucide, label: &'static str| {
            v_flex()
                .id(id)
                .test_support()
                .w(px(CARD_W))
                .gap_1()
                .cursor_pointer()
                .group("text-tile")
                .child(
                    h_flex()
                        .w(px(CARD_W))
                        .h(px(CARD_W * 9.0 / 16.0))
                        .justify_center()
                        .rounded(px(4.))
                        .bg(cx.theme().muted)
                        .border_1()
                        .border_color(gpui_kit::transparent_black())
                        .group_hover("text-tile", |s| s.border_color(palette::accent()))
                        .child(Icon::new(icon).size(px(22.))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
        };
        h_flex()
            .p_2()
            .gap_3()
            .items_start()
            .child(tile("bin-text-default", Lucide::Type, "默认文本").on_click(
                cx.listener(|_, _, w, cx| w.dispatch_action(Box::new(act::AddSubtitle), cx)),
            ))
            .child(
                tile("bin-text-srt", Lucide::Captions, "导入本地字幕").on_click(
                    cx.listener(|_, _, w, cx| w.dispatch_action(Box::new(act::ImportSrt), cx)),
                ),
            )
    }

    pub(crate) fn render_media_bin(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let tabs = BinTab::ALL.iter().map(|&tab| {
            chrome::icon_tab(tab.id(), tab.icon(), tab.label(), self.bin_tab == tab, cx).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.bin_tab = tab;
                    cx.notify();
                }),
            )
        });
        let sub = |label: &'static str, selected: bool| {
            div()
                .px_2()
                .py_1()
                .rounded(px(4.))
                .text_xs()
                .when_sub_selected(selected, cx)
                .child(label)
        };
        let content = match self.bin_tab {
            BinTab::Media => self.render_media_tab(cx).into_any_element(),
            BinTab::Text => Self::render_text_tab(cx).into_any_element(),
            BinTab::Transition => self.render_transition_tab(cx).into_any_element(),
            BinTab::Audio => self.render_audio_tab(cx).into_any_element(),
            BinTab::Filter => self.render_filter_tab(cx).into_any_element(),
            BinTab::Adjust => Self::render_adjust_bin_tab(cx).into_any_element(),
        };
        v_flex()
            .id("media-bin")
            .test_support()
            .size_full()
            .child(
                h_flex()
                    .id("bin-tabs")
                    .flex_none()
                    .px_1()
                    .gap_0p5()
                    .border_b_1()
                    .border_color(palette::divider())
                    .children(tabs),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(
                        // 二级栏：先只有各标签的第一项，结构和剪映一致，后续往里加分类
                        v_flex()
                            .w(px(84.))
                            .h_full()
                            .flex_none()
                            .p_1()
                            .gap_1()
                            .border_r_1()
                            .border_color(palette::divider())
                            .children(
                                self.bin_tab
                                    .sections()
                                    .iter()
                                    .enumerate()
                                    .map(|(i, s)| sub(s, i == 0)),
                            ),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(content)),
            )
    }
}

/// 卡片宽度（缩略图 16:9）。
const CARD_W: f32 = 104.0;

/// 素材面板顶部的一级标签（剪映的 13 个里和剪辑有关的 6 个）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BinTab {
    /// 导入的素材。
    #[default]
    Media,
    /// 音频库（后续）。
    Audio,
    /// 文本：默认文本、导入字幕。
    Text,
    /// 转场（后续）。
    Transition,
    /// 滤镜（后续）。
    Filter,
    /// 调节（后续）。
    Adjust,
}

impl BinTab {
    /// 从左到右的顺序。
    pub const ALL: [Self; 6] = [
        Self::Media,
        Self::Audio,
        Self::Text,
        Self::Transition,
        Self::Filter,
        Self::Adjust,
    ];

    const fn id(self) -> &'static str {
        match self {
            Self::Media => "bin-tab-media",
            Self::Audio => "bin-tab-audio",
            Self::Text => "bin-tab-text",
            Self::Transition => "bin-tab-transition",
            Self::Filter => "bin-tab-filter",
            Self::Adjust => "bin-tab-adjust",
        }
    }

    /// 标签上的两个字。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Media => "素材",
            Self::Audio => "音频",
            Self::Text => "文本",
            Self::Transition => "转场",
            Self::Filter => "滤镜",
            Self::Adjust => "调节",
        }
    }

    const fn icon(self) -> Lucide {
        match self {
            Self::Media => Lucide::Film,
            Self::Audio => Lucide::Music,
            Self::Text => Lucide::Type,
            Self::Transition => Lucide::ArrowRightLeft,
            Self::Filter => Lucide::Aperture,
            Self::Adjust => Lucide::SlidersHorizontal,
        }
    }

    /// 二级栏（左侧那一列）。
    const fn sections(self) -> &'static [&'static str] {
        match self {
            Self::Media => &["导入"],
            Self::Audio => &["音频提取"],
            Self::Text => &["新建文本"],
            Self::Transition => &["转场效果"],
            Self::Filter => &["滤镜库"],
            Self::Adjust => &["新建调节"],
        }
    }
}

/// 二级栏选中项：深灰圆角底 + 正文色；其余是次要字色。
trait SubStyle: gpui_kit::Styled + Sized {
    fn when_sub_selected(self, selected: bool, cx: &gpui_kit::App) -> Self {
        if selected {
            self.bg(cx.theme().accent).text_color(cx.theme().foreground)
        } else {
            self.text_color(cx.theme().muted_foreground)
        }
    }
}

impl<T: gpui_kit::Styled> SubStyle for T {}
