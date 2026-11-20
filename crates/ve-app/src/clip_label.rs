//! 片段颜色标签和替换片段（剪映右键菜单）：8 个色点 + 还原、「选中同色片段」、「替换片段」。

use crate::actions as act;
use crate::workspace::Workspace;
use gpui_kit::component::notification::Notification;
use gpui_kit::{AppContext as _, Context, Hsla, PathPromptOptions, Window, rgb};
use std::path::PathBuf;
use ve_core::ClipId;
use ve_core::edit::{replace_asset, set_label};

/// 8 种标签颜色（红、橙、黄、绿、青、蓝、紫、粉），和右键菜单里色点的顺序一样。
pub(crate) const LABEL_RGB: [u32; ve_core::timeline::LABEL_COLORS as usize] = [
    0xE5_484D, 0xF7_6B15, 0xFF_C53D, 0x46_A758, 0x12_A594, 0x3E_63DD, 0x8E_4EC6, 0xD6_409F,
];

/// 标签颜色。
pub(crate) fn label_color(label: u8) -> Hsla {
    rgb(LABEL_RGB[usize::from(label) % LABEL_RGB.len()]).into()
}

impl Workspace {
    /// 给选中的片段标颜色（`None` 还原）。
    pub(crate) fn label_selection(
        &mut self,
        label: Option<u8>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let selected = self.doc.selection().to_vec();
        if selected.is_empty() {
            return;
        }
        self.apply(window, cx, |p| set_label(p, &selected, label));
    }

    /// 选中的第一个片段的颜色标签。
    pub(crate) fn selection_label(&self) -> Option<u8> {
        let first = *self.doc.selection().first()?;
        self.doc.project.timeline.find_clip(first)?.1.label
    }

    /// 选中同色片段：和选中的（第一个）片段颜色一样的都选上（锁定轨道上的除外）。
    pub(crate) fn select_same_color(
        &mut self,
        _: &act::SelectSameColor,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(label) = self.selection_label() else {
            return;
        };
        let same: Vec<ClipId> = self
            .doc
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.label == Some(label))
            .map(|c| c.id)
            .filter(|&c| self.doc.selectable(c))
            .collect();
        self.doc.set_selection(same);
        self.after_change(window, cx);
    }

    /// 替换片段：选一个文件，换掉选中片段的素材（位置、长度、属性都不变）。
    pub(crate) fn replace_clip(
        &mut self,
        _: &act::ReplaceClip,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let [clip] = self.doc.selection() else {
            return;
        };
        let clip = *clip;
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("替换片段".into()),
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    this.replace_clip_with(clip, path, window, cx);
                });
            }
        }));
    }

    /// 用文件 `path` 替换片段 `clip` 的素材（文件先导入素材库）。
    pub fn replace_clip_with(
        &mut self,
        clip: ClipId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let probing = cx.background_spawn(async move { crate::import::probe_asset(&path) });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let probed = probing.await;
            let _ignored = this.update_in(cx, |this, window, cx| match probed {
                Ok(asset) => {
                    let id = this.doc.add_asset(asset);
                    let added = this.doc.project.assets.get(&id).cloned();
                    this.apply(window, cx, |p| replace_asset(p, clip, id));
                    this.analyze_assets(added.into_iter().collect(), window, cx);
                }
                Err(message) => {
                    crate::workspace::notify(Notification::warning(message), window, cx);
                }
            });
        }));
    }
}
