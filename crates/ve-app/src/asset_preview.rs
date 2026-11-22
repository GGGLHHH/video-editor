//! 素材面板：单击素材卡片先在播放器里单独看这个素材（剪映「正在预览 — 视频」），右键删除素材。
//!
//! 预览时播放器换成只有这个素材的临时工程，时间轴的播放头停在原地；按 Esc、点时间轴或者做任何
//! 编辑都回到时间轴。

use crate::chrome;
use crate::theme::palette;
use crate::workspace::{Workspace, notify};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, MouseButton,
    ParentElement as _, Pixels, Point, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, deferred, div, px,
};
use std::sync::Arc;
use ve_core::{AssetId, Project, Time};

/// 正在单独预览的素材。
#[derive(Debug, Clone, Copy)]
pub(crate) struct AssetPreview {
    asset: AssetId,
    /// 时间轴的播放头（预览完回到这里）。
    back_to: Time,
    /// 开始预览时文档的版本：之后改了文档就结束预览。
    revision: u64,
}

impl AssetPreview {
    /// 预览时时间轴的播放头停在这里。
    pub(crate) const fn back_to(&self) -> Time {
        self.back_to
    }

    /// 文档还是开始预览时的那个版本。
    pub(crate) const fn revision_is(&self, revision: u64) -> bool {
        self.revision == revision
    }
}

impl Workspace {
    /// 正在单独预览的素材（没有时 `None`）。
    #[must_use]
    pub fn previewing_asset(&self) -> Option<AssetId> {
        self.asset_preview.map(|p| p.asset)
    }

    /// 送给播放引擎的工程（摊开复合片段、换上代理；预览素材时是那个临时工程）。
    #[must_use]
    pub fn engine_project(&self) -> &Project {
        &self.engine_project
    }

    /// 播放器上显示的时间：预览素材时是素材里的时间，否则是时间轴的播放头。
    pub(crate) fn player_time(&self) -> Time {
        if self.asset_preview.is_some() {
            self.engine.position()
        } else {
            self.playhead
        }
    }

    /// 播放器上显示的总长。
    pub(crate) fn player_duration(&self) -> Time {
        if self.asset_preview.is_some() {
            self.engine_project.timeline.duration()
        } else {
            self.doc.project.timeline.duration()
        }
    }

    /// 播放器标题：预览素材时「正在预览 — 素材名」。
    pub(crate) fn player_title(&self) -> Option<SharedString> {
        let p = self.asset_preview?;
        let name = self.doc.project.assets.get(&p.asset)?.name.clone();
        Some(format!("正在预览 — {name}").into())
    }

    /// 单击素材卡片：播放器单独放这个素材，从头开始。
    pub(crate) fn start_asset_preview(&mut self, asset: AssetId, cx: &mut Context<'_, Self>) {
        let Some(mut project) = ve_core::edit::preview_project(&self.doc.project, asset) else {
            return;
        };
        self.with_proxies(&mut project);
        let back_to = self.asset_preview.map_or(self.playhead, |p| p.back_to);
        self.engine.pause();
        self.engine_project = Arc::new(project);
        self.engine.set_project(Arc::clone(&self.engine_project));
        self.engine.seek(Time::ZERO);
        self.asset_preview = Some(AssetPreview {
            asset,
            back_to,
            revision: self.doc.revision(),
        });
        cx.notify();
    }

    /// 结束预览，播放器回到时间轴、播放头回到原来的位置。没在预览时返回 `false`。
    pub(crate) fn end_asset_preview(&mut self, cx: &mut Context<'_, Self>) -> bool {
        let Some(p) = self.asset_preview.take() else {
            return false;
        };
        self.engine.pause();
        self.engine_revision = None;
        self.sync_engine();
        self.engine.seek(p.back_to);
        self.playhead = p.back_to;
        cx.notify();
        true
    }

    /// 预览中文档改了（编辑、撤销……）：结束预览。返回还在不在预览。
    pub(crate) fn keep_asset_preview(&mut self, cx: &mut Context<'_, Self>) -> bool {
        match self.asset_preview {
            Some(p) if p.revision == self.doc.revision() => true,
            Some(_) => {
                self.end_asset_preview(cx);
                false
            }
            None => false,
        }
    }

    /// 右键素材卡片：在鼠标处弹出素材菜单。
    pub(crate) const fn open_asset_menu(&mut self, asset: AssetId, at: Point<Pixels>) {
        self.asset_menu = Some((asset, at));
    }

    /// 删除素材：时间轴上用到它的片段一起删（一步撤销）。
    fn delete_asset(&mut self, asset: AssetId, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.asset_menu = None;
        if self.previewing_asset() == Some(asset) {
            self.end_asset_preview(cx);
        }
        if let Some(n) = self.apply(window, cx, |p| ve_core::edit::remove_asset(p, asset))
            && n > 0
        {
            notify(
                Notification::info(format!("素材删掉了，时间轴上用到它的 {n} 个片段也一起删了")),
                window,
                cx,
            );
        }
    }

    /// 右键素材卡片弹出的菜单。
    pub(crate) fn render_asset_menu(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let (asset, at) = self.asset_menu?;
        let row = div()
            .id("asset-menu-delete")
            .test_support()
            .h(px(26.))
            .px_2()
            .flex()
            .items_center()
            .rounded(px(4.))
            .text_sm()
            .cursor_pointer()
            .hover(|s| s.bg(palette::menu_hover()))
            .child("删除")
            .on_click(cx.listener(move |this, _, window, cx| {
                this.delete_asset(asset, window, cx);
            }));
        Some(
            deferred(
                v_flex()
                    .id("asset-menu")
                    .occlude()
                    .test_support()
                    .absolute()
                    .left(at.x)
                    .top(at.y)
                    .w(px(160.))
                    .p_1()
                    .bg(palette::menu_bg())
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(chrome::CARD_RADIUS))
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.asset_menu = None;
                        cx.notify();
                    }))
                    .child(row),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}
