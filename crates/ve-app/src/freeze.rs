//! 定格（剪映工具栏的「定格」）：取播放头那一帧存成图片，在播放头处切开片段、插进一段 3 秒的静止画面。

use crate::actions as act;
use crate::workspace::Workspace;
use gpui_kit::component::notification::Notification;
use gpui_kit::{AppContext as _, Context, Window};
use std::path::{Path, PathBuf};
use ve_core::edit::{FREEZE_LENGTH, insert_freeze};
use ve_core::{ClipId, Time};

/// 默认的定格图片目录：`%APPDATA%/video-editor/freeze`（和草稿目录放在一起）。
#[must_use]
pub(crate) fn default_dir() -> PathBuf {
    crate::recent::default_file()
        .parent()
        .map_or_else(|| PathBuf::from("freeze"), |p| p.join("freeze"))
}

/// 定格图片的文件名：取帧时刻的纳秒时间戳，不会重名。
fn file_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("定格-{nanos}.png")
}

/// 取帧、探测，在后台线程里跑。素材名叫「定格 · 原素材名」（文件名只为不重名）。
fn grab(source: &Path, at: Time, out: &Path, name: &str) -> Result<ve_core::Asset, String> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    ve_export::grab_frame(source, at, out)?;
    let mut asset = crate::import::probe_asset(out)?;
    asset.name = format!("定格 · {name}");
    Ok(asset)
}

impl Workspace {
    /// 换一个定格图片目录（测试用，避免写进用户真实的目录）。
    pub fn use_freeze_dir(&mut self, dir: PathBuf) {
        self.freeze_dir = dir;
    }

    /// 要定格的片段：选中的视频片段里盖住播放头的那个。返回（片段，素材文件，素材名，素材时间）。
    fn freeze_target(&self, at: Time) -> Option<(ClipId, PathBuf, String, Time)> {
        let p = &self.doc.project;
        self.selected_video_clips().into_iter().find_map(|id| {
            let (_, clip) = p.timeline.find_clip(id)?;
            if at < clip.start || at >= clip.end() {
                return None;
            }
            let asset = p.assets.get(&clip.asset()?)?;
            // 图片本身没有时间：取它唯一的一帧
            let source = if asset.is_still() {
                Time::ZERO
            } else {
                clip.source_at(at - clip.start)
            };
            Some((id, asset.path.clone(), asset.name.clone(), source))
        })
    }

    pub(crate) fn freeze(
        &mut self,
        _: &act::Freeze,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let at = self.playhead_frame();
        let Some((clip, source, name, source_at)) = self.freeze_target(at) else {
            crate::workspace::notify(
                Notification::warning("先选中播放头所在的视频片段，再定格"),
                window,
                cx,
            );
            return;
        };
        let out = self.freeze_dir.join(file_name());
        let work = cx.background_spawn(async move { grab(&source, source_at, &out, &name) });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ignored = this.update_in(cx, |this, window, cx| match result {
                Ok(asset) => {
                    let still = this.apply(window, cx, |p| {
                        let id = p.add_asset(asset);
                        insert_freeze(p, clip, at, id, FREEZE_LENGTH).map(|c| (id, c))
                    });
                    if let Some((asset, frozen)) = still {
                        this.doc.select(frozen, false);
                        this.after_change(window, cx);
                        let added = this.doc.project.assets.get(&asset).cloned();
                        this.analyze_assets(added.into_iter().collect(), window, cx);
                    }
                }
                Err(e) => crate::workspace::notify(
                    Notification::warning(format!("定格失败：{e}")),
                    window,
                    cx,
                ),
            });
        }));
    }
}
