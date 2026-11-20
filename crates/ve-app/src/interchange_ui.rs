//! 工程交换的菜单（文件 › 导出 OTIO / EDL、导入 OTIO）：和 Premiere、达芬奇、Final Cut 互通剪辑结构。
//! 格式本身在 [`ve_core::interchange`]。

use crate::actions as act;
use crate::document::Document;
use crate::workspace::{Workspace, notify};
use gpui_kit::component::notification::Notification;
use gpui_kit::{AppContext as _, Context, PathPromptOptions, Window};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use ve_core::interchange::{build_project, from_otio, to_edl, to_otio};

/// 导出哪种。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interchange {
    /// `OpenTimelineIO`。
    Otio,
    /// CMX3600 EDL。
    Edl,
}

impl Workspace {
    /// 把时间轴写成 `kind` 格式存到 `path`（复合片段先摊开）。
    ///
    /// # Errors
    /// 写不进去。
    pub fn export_interchange(&self, kind: Interchange, path: &Path) -> std::io::Result<()> {
        let project = ve_core::compound::flatten(&self.doc.project)
            .unwrap_or_else(|| self.doc.project.clone());
        let title = self.doc.title();
        let text = match kind {
            Interchange::Otio => to_otio(&project, &title),
            Interchange::Edl => to_edl(&project, &title),
        };
        std::fs::write(path, text)
    }

    fn prompt_export_interchange(
        &self,
        kind: Interchange,
        window: &Window,
        cx: &Context<'_, Self>,
    ) {
        let name = match kind {
            Interchange::Otio => "时间线.otio",
            Interchange::Edl => "时间线.edl",
        };
        let rx = cx.prompt_for_new_path(&crate::file_actions::start_dir(&self.doc), Some(name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    match this.export_interchange(kind, &path) {
                        Ok(()) => notify(Notification::success("已导出"), window, cx),
                        Err(e) => notify(Notification::error(format!("导出失败：{e}")), window, cx),
                    }
                });
            }
        })
        .detach();
    }

    pub(crate) fn export_otio(
        &mut self,
        _: &act::ExportOtio,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.prompt_export_interchange(Interchange::Otio, window, cx);
    }

    pub(crate) fn export_edl(
        &mut self,
        _: &act::ExportEdl,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.prompt_export_interchange(Interchange::Edl, window, cx);
    }

    pub(crate) fn import_otio(
        &mut self,
        _: &act::ImportOtio,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("导入 OpenTimelineIO".into()),
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    this.import_otio_path(path, window, cx);
                });
            }
        }));
    }

    /// 读 OTIO 文件 `path`，素材在后台探测，做成一个新草稿打开。
    pub fn import_otio_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let rate = self.doc.project.settings.frame_rate;
        let work = cx.background_spawn(async move {
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let tracks = from_otio(&text).map_err(|e| e.to_string())?;
            let mut assets = BTreeMap::new();
            for c in tracks.iter().flat_map(|t| &t.clips) {
                if !assets.contains_key(&c.path)
                    && let Ok(a) = crate::import::probe_asset(&c.path)
                {
                    assets.insert(c.path.clone(), a);
                }
            }
            Ok::<_, String>(build_project(&tracks, &assets, rate))
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let built = work.await;
            let _ignored = this.update_in(cx, |this, window, cx| match built {
                Ok((project, skipped)) => {
                    this.replace_document(Document::from_project(project, None), window, cx);
                    let note = if skipped == 0 {
                        Notification::success("已导入")
                    } else {
                        Notification::warning(format!(
                            "已导入，{skipped} 段找不到素材或放不下，跳过了"
                        ))
                    };
                    notify(note, window, cx);
                }
                Err(e) => notify(Notification::error(format!("导入失败：{e}")), window, cx),
            });
        }));
    }
}
