//! 素材库：导入（对话框或拖文件进窗口）、列表、双击 / 按钮放进时间轴、拖到时间轴。

use crate::actions as act;
use crate::import::{Analysis, analyze, probe_asset};
use crate::preview::to_render_image;
use crate::workspace::{MediaThumbs, Workspace};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{IconName, h_flex, v_flex};
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

    fn render_asset_item(
        &self,
        a: &Asset,
        rate: ve_core::FrameRate,
        cx: &Context<'_, Self>,
    ) -> impl IntoElement {
        let id = a.id;
        let thumb = self
            .media
            .get(&id)
            .and_then(|m| m.thumbnails.first().cloned());
        let kind = match (a.video, a.audio) {
            (Some(v), _) if v.still => "图片".to_owned(),
            (Some(v), _) => format!("{}×{}", v.width, v.height),
            (None, Some(_)) => "音频".to_owned(),
            (None, None) => String::new(),
        };
        let drag = AssetDrag {
            asset: id,
            name: a.name.clone().into(),
        };
        h_flex()
            .id(SharedString::from(format!("asset-{}", id.0)))
            .test_support()
            .gap_2()
            .p_1()
            .rounded_md()
            .hover(|s| s.bg(cx.theme().accent))
            .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                if e.click_count() >= 2 {
                    this.append_to_timeline(id, window, cx);
                }
            }))
            .child(
                div()
                    .w(px(64.))
                    .h(px(36.))
                    .flex_none()
                    .rounded_sm()
                    .overflow_hidden()
                    .bg(cx.theme().muted)
                    .children(thumb.map(|t| img(t).size_full())),
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
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(format!(
                                "{}  {kind}",
                                format_timecode(a.duration, rate)
                            ))),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!("add-{}", id.0)))
                    .ghost()
                    .compact()
                    .icon(IconName::Plus)
                    .tooltip("放到时间轴末尾")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.append_to_timeline(id, window, cx);
                    })),
            )
    }

    pub(crate) fn render_media_bin(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let rate = self.doc.project.settings.frame_rate;
        let mut assets: Vec<&Asset> = self.doc.project.assets.values().collect();
        assets.sort_by_key(|a| a.id);
        let items = assets
            .into_iter()
            .map(|a| self.render_asset_item(a, rate, cx));
        let empty = self.doc.project.assets.is_empty();
        v_flex()
            .id("media-bin")
            .test_support()
            .size_full()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .h(px(32.))
                    .px_2()
                    .justify_between()
                    .child(div().text_sm().child("素材"))
                    .children((self.importing > 0).then(|| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("导入中…")
                    })),
            )
            .child(
                v_flex()
                    .id("media-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_1()
                    .gap_1()
                    .children(items)
                    .children(empty.then(|| {
                        div()
                            .p_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("点「导入素材」或把文件拖进窗口")
                    })),
            )
    }
}
