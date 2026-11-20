//! 视频防抖（剪映「画面 › 基础 › 视频防抖」）：关闭 / 裁切最少 / 推荐 / 最稳定。选一档就在后台把片段
//! 用到的那一段做成防抖后的文件，片段换成它（一步撤销）；换档从原片重新做，关闭换回原片。

use crate::chrome;
use crate::inspector_group::Group;
use crate::workspace::{Workspace, notify};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};
use std::path::PathBuf;
use ve_core::ClipId;
use ve_core::stabilize::{LEVELS, smoothing, stabilize_clip, unstabilize_clip};

/// 防抖文件的目录：`%APPDATA%\video-editor\stabilize`。
#[must_use]
pub(crate) fn default_dir() -> PathBuf {
    crate::recent::default_file()
        .parent()
        .map_or_else(|| PathBuf::from("stabilize"), |p| p.join("stabilize"))
}

impl Workspace {
    /// 换一个防抖文件目录（测试用）。
    pub fn use_stabilize_dir(&mut self, dir: PathBuf) {
        self.stabilize_dir = dir;
    }

    /// 是不是正在做防抖。
    #[must_use]
    pub const fn is_stabilizing(&self) -> bool {
        self.stabilizing
    }

    /// 选中的、能防抖的片段（视频轨上的视频素材，不是图片、没倒放）和它现在的档位。
    fn stabilize_target(&self) -> Option<(ClipId, Option<u8>)> {
        let (clip, _, _) = self.bound_video_clip()?;
        let p = &self.doc.project;
        let (_, c) = p.timeline.find_clip(clip)?;
        let asset = p.assets.get(&c.asset()?)?;
        (c.reversed.is_none() && !asset.is_still() && asset.video.is_some())
            .then_some((clip, c.stabilized.map(|s| s.2)))
    }

    /// 选一档（`None` 关闭）。
    pub fn set_stabilize(
        &mut self,
        level: Option<u8>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.stabilizing {
            return;
        }
        let Some((clip, current)) = self.stabilize_target() else {
            return;
        };
        if level == current {
            return;
        }
        let Some(level) = level else {
            self.apply(window, cx, |p| unstabilize_clip(p, clip));
            return;
        };
        let p = &self.doc.project;
        let Some((_, c)) = p.timeline.find_clip(clip) else {
            return;
        };
        // 从原片做（已经防抖过的话换算回原素材里现在用的这一段）
        let (asset, from) = ve_core::edit::original_range(p, c)
            .map_or_else(|| (c.asset(), c.source_in()), |(a, s)| (Some(a), s));
        let Some(source) = asset.and_then(|a| p.assets.get(&a)) else {
            return;
        };
        let Some(key) = self.source_key(clip) else {
            return;
        };
        let (path, name, len) = (source.path.clone(), source.name.clone(), c.source_span());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let out = self.stabilize_dir.join(format!("防抖-{nanos}.mp4"));
        self.stabilizing = true;
        notify(Notification::info("视频防抖处理中…"), window, cx);
        cx.notify();
        let work = cx.background_spawn(async move {
            ve_export::stabilize_segment(&path, from, len, smoothing(level), &out)?;
            let mut a = crate::import::probe_asset(&out)?;
            a.name = format!("防抖 · {name}");
            Ok::<_, String>(a)
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let made = work.await;
            let _ignored = this.update_in(cx, |this, window, cx| {
                this.stabilizing = false;
                match made {
                    // 处理期间换了工程、片段被裁剪或撤销过：结果对不上，不套用
                    Ok(stale) if this.source_key(clip).as_ref() != Some(&key) => {
                        // 生成的文件用不上了
                        drop(std::fs::remove_file(&stale.path));
                        notify(
                            Notification::warning(
                                "片段在处理期间改动了，防抖结果没有套用，请重新选一次",
                            ),
                            window,
                            cx,
                        );
                    }
                    Ok(asset) => {
                        let added = this.apply(window, cx, |p| {
                            if current.is_some() {
                                unstabilize_clip(p, clip)?;
                            }
                            let id = p.add_asset(asset);
                            stabilize_clip(p, clip, id, level).map(|()| id)
                        });
                        if let Some(id) = added {
                            let a = this.doc.project.assets.get(&id).cloned();
                            this.analyze_assets(a.into_iter().collect(), window, cx);
                        }
                    }
                    Err(e) => {
                        log::warn!("视频防抖失败：{e}");
                        notify(
                            Notification::warning(format!("视频防抖失败：{e}")),
                            window,
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        }));
    }

    /// 「视频防抖」组（画面 › 基础的最下面）。
    pub(crate) fn render_stabilize(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let (_, current) = self.stabilize_target()?;
        let muted = cx.theme().muted_foreground;
        let chip = |id: &'static str, label: &'static str, level: Option<u8>| {
            chrome::chip(id, label, current == level, cx).on_click(
                cx.listener(move |this, _, window, cx| this.set_stabilize(level, window, cx)),
            )
        };
        let ids = ["stab-0", "stab-1", "stab-2"];
        let body = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(chip("stab-off", "关闭", None))
                    .children(
                        LEVELS
                            .iter()
                            .zip(ids)
                            .enumerate()
                            .map(|(i, ((label, _), id))| chip(id, label, u8::try_from(i).ok())),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(if self.stabilizing {
                        "处理中…"
                    } else {
                        "越稳裁得越多；处理好后片段换成防抖后的文件，选「关闭」换回原片"
                    }),
            )
            .into_any_element();
        Some(self.render_group(
            Group {
                id: "stab",
                title: "视频防抖",
                check: None,
                reset: None,
                extra: None,
            },
            body,
            cx,
        ))
    }
}
