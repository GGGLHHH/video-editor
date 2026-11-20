//! 倒放（剪映工具栏 / 右键菜单的「倒放」）：把选中片段用到的那一段素材倒着生成一个新文件，
//! 片段换成它（后台做，做完一步撤销）；已经倒放过的片段再点一次换回原来的素材。

use crate::actions as act;
use crate::workspace::Workspace;
use gpui_kit::component::notification::Notification;
use gpui_kit::{AppContext as _, Context, Window};
use std::path::{Path, PathBuf};
use ve_core::edit::{reverse_clip, unreverse_clip};
use ve_core::{ClipId, Time};

/// 倒放文件的目录：`%APPDATA%/video-editor/reverse`（和定格图片放在一起）。
#[must_use]
pub(crate) fn default_dir() -> PathBuf {
    crate::recent::default_file()
        .parent()
        .map_or_else(|| PathBuf::from("reverse"), |p| p.join("reverse"))
}

/// 一次倒放要做的事。
struct Job {
    clip: ClipId,
    source: PathBuf,
    name: String,
    from: Time,
    len: Time,
    video: bool,
    audio: bool,
}

/// 生成倒放文件并探测（后台线程）。
fn make(job: &Job, out: &Path) -> Result<ve_core::Asset, String> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    ve_export::reverse_segment(&job.source, job.from, job.len, job.video, job.audio, out)?;
    let mut asset = crate::import::probe_asset(out)?;
    asset.name = format!("倒放 · {}", job.name);
    Ok(asset)
}

impl Workspace {
    /// 换一个倒放文件目录（测试用）。
    pub fn use_reverse_dir(&mut self, dir: PathBuf) {
        self.reverse_dir = dir;
    }

    /// 是不是正在生成倒放文件。
    #[must_use]
    pub const fn is_reversing(&self) -> bool {
        self.reversing
    }

    /// 选中的、能倒放的片段（视频或声音素材，不是图片）。
    pub(crate) fn reverse_target(&self) -> Option<ClipId> {
        let [clip] = self.doc.selection() else {
            return None;
        };
        let p = &self.doc.project;
        let (_, c) = p.timeline.find_clip(*clip)?;
        let asset = p.assets.get(&c.asset()?)?;
        (c.reversed.is_some() || !asset.is_still()).then_some(*clip)
    }

    pub(crate) fn reverse(
        &mut self,
        _: &act::Reverse,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.reversing {
            return;
        }
        let Some(clip) = self.reverse_target() else {
            crate::workspace::notify(
                Notification::warning("先选中一个视频或音频片段，再倒放"),
                window,
                cx,
            );
            return;
        };
        let p = &self.doc.project;
        let Some((track, c)) = p.timeline.find_clip(clip) else {
            return;
        };
        // 已经倒放过：换回原来的素材
        if c.reversed.is_some() {
            self.apply(window, cx, |p| unreverse_clip(p, clip));
            return;
        }
        let Some(asset) = c.asset().and_then(|a| p.assets.get(&a)) else {
            return;
        };
        let on_video = p.timeline.track(track).map(|t| t.kind) == Some(ve_core::TrackKind::Video);
        let job = Job {
            clip,
            source: asset.path.clone(),
            name: asset.name.clone(),
            from: c.source_in(),
            len: c.source_span(),
            video: on_video && asset.video.is_some(),
            audio: asset.audio.is_some(),
        };
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let ext = if job.video { "mp4" } else { "m4a" };
        let out = self.reverse_dir.join(format!("倒放-{nanos}.{ext}"));
        self.reversing = true;
        crate::workspace::notify(Notification::info("倒放中…"), window, cx);
        cx.notify();
        let key = self.source_key(clip);
        let work = cx.background_spawn(async move { (job.clip, make(&job, &out)) });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let (clip, result) = work.await;
            let _ignored = this.update_in(cx, |this, window, cx| {
                this.reversing = false;
                match result {
                    // 处理期间换了工程、片段被裁剪或撤销过：结果对不上，不套用
                    Ok(stale) if this.source_key(clip) != key => {
                        // 生成的文件用不上了
                        drop(std::fs::remove_file(&stale.path));
                        crate::workspace::notify(
                            Notification::warning(
                                "片段在处理期间改动了，倒放结果没有套用，请重新倒放",
                            ),
                            window,
                            cx,
                        );
                    }
                    Ok(asset) => {
                        let added = this.apply(window, cx, |p| {
                            let id = p.add_asset(asset);
                            reverse_clip(p, clip, id).map(|()| id)
                        });
                        if let Some(id) = added {
                            let a = this.doc.project.assets.get(&id).cloned();
                            this.analyze_assets(a.into_iter().collect(), window, cx);
                        }
                    }
                    Err(e) => {
                        log::warn!("倒放失败：{e}");
                        crate::workspace::notify(
                            Notification::warning(format!("倒放失败：{e}")),
                            window,
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        }));
    }
}
