//! 素材丢失后重新链接（剪映「链接媒体」）：找不到文件的素材在素材面板和时间轴上标红，
//! 点「链接媒体」选一个新文件；同目录里同名的其它丢失素材一起找回（[`ve_core::relink`]）。

use crate::workspace::{Workspace, notify};
use gpui_kit::component::notification::Notification;
use gpui_kit::{AppContext as _, Context, PathPromptOptions, Window};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use ve_core::AssetId;
use ve_core::relink::relink;

impl Workspace {
    /// 重新检查哪些素材找不到文件（打开工程、编辑、撤销后）。
    pub(crate) fn refresh_missing(&mut self) {
        self.missing = ve_core::relink::missing_assets(&self.doc.project, Path::exists)
            .into_iter()
            .collect();
    }

    /// 素材 `id` 找不到文件。
    #[must_use]
    pub fn is_missing(&self, id: AssetId) -> bool {
        self.missing.contains(&id)
    }

    /// 「链接媒体」：选一个文件换给素材 `asset`。
    pub(crate) fn relink_prompt(
        &mut self,
        asset: AssetId,
        window: &Window,
        cx: &Context<'_, Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("链接媒体".into()),
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    this.relink_with(asset, path, window, cx);
                });
            }
        }));
    }

    /// 把素材 `asset` 链接到文件 `path`（后台探测文件，一步撤销）。
    pub fn relink_with(
        &mut self,
        asset: AssetId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        // 同一文件夹里和别的丢失素材同名的文件：一起在后台探测（换不换由 relink 判断）
        let siblings: Vec<PathBuf> = path.parent().map_or_else(Vec::new, |dir| {
            self.doc
                .project
                .assets
                .values()
                .filter(|a| a.id != asset && !a.path.exists())
                .filter_map(|a| Some(dir.join(a.path.file_name()?)))
                .collect()
        });
        let probing = cx.background_spawn(async move {
            let found = crate::import::probe_asset(&path)?;
            let others: HashMap<PathBuf, ve_core::Asset> = siblings
                .into_iter()
                .filter(|p| p.exists())
                .filter_map(|p| Some((p.clone(), crate::import::probe_asset(&p).ok()?)))
                .collect();
            Ok::<_, String>((found, others))
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let probed = probing.await;
            let _ignored = this.update_in(cx, |this, window, cx| match probed {
                Ok((found, others)) => this.finish_relink(asset, &found, &others, window, cx),
                Err(message) => notify(Notification::warning(message), window, cx),
            });
        }));
    }

    fn finish_relink(
        &mut self,
        asset: AssetId,
        found: &ve_core::Asset,
        others: &HashMap<PathBuf, ve_core::Asset>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let probe = |p: &Path| others.get(p).cloned();
        // 先在副本上试，换不上时说清原因
        let mut trial = self.doc.project.clone();
        if let Err(e) = relink(&mut trial, asset, found, Path::exists, probe) {
            notify(Notification::warning(e.to_string()), window, cx);
            return;
        }
        let Some(done) = self.apply(window, cx, |p| {
            relink(p, asset, found, Path::exists, probe)
                .map_err(|_| ve_core::EditError::InvalidValue)
        }) else {
            return;
        };
        // 缩略图、波形按新文件重新分析
        for id in &done {
            if let Some(thumbs) = self.media.remove(id) {
                for image in thumbs.thumbnails {
                    cx.drop_image(image, Some(window));
                }
            }
        }
        let assets: Vec<_> = done
            .iter()
            .filter_map(|id| self.doc.project.assets.get(id).cloned())
            .collect();
        self.analyze_assets(assets, window, cx);
        let message = if done.len() > 1 {
            format!("已链接 {} 个素材（同一文件夹里的一起找回了）", done.len())
        } else {
            "已链接媒体".to_owned()
        };
        notify(Notification::success(message), window, cx);
    }
}
