//! 代理模式（剪映「代理模式」）：给大分辨率的视频素材生成低清副本，预览时用副本、导出还用原片。
//!
//! 副本放在 `%APPDATA%\video-editor\proxies`，按原文件的路径、大小、修改时间起名，下次打开直接用。
//! 只换交给播放引擎的那份工程里的路径（素材的宽高等信息不变，画面布局照旧）；时间轴、导出都是原片。

use crate::workspace::{Workspace, notify};
use gpui_kit::component::notification::Notification;
use gpui_kit::{AppContext as _, Context, Window};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use ve_core::{Asset, AssetId, Project};

/// 短边超过多少像素才做代理（1080p 起，720p 以下解码本来就快）。
pub(crate) const DEFAULT_MIN_SHORT_SIDE: u32 = 721;

/// 代理模式的状态。
#[derive(Debug)]
pub(crate) struct Proxies {
    /// 开着没有。
    pub(crate) on: bool,
    /// 副本放在哪。
    dir: PathBuf,
    /// 短边超过多少才做。
    min_short: u32,
    /// 做好的：原文件 → 副本。
    ready: HashMap<PathBuf, PathBuf>,
    /// 排着队和正在做的原文件。
    pending: HashSet<PathBuf>,
    /// 排队等着做的（一次只做一个：几十个 4K 素材同时编码会把机器和后台线程池都占满）。
    queue: VecDeque<PathBuf>,
    /// 有一个正在做。
    running: bool,
    /// 做失败过的（这次开着代理模式时不再重试、不再反复提示）。
    failed: HashSet<PathBuf>,
}

impl Proxies {
    pub(crate) fn new() -> Self {
        Self {
            on: false,
            dir: default_dir(),
            min_short: DEFAULT_MIN_SHORT_SIDE,
            ready: HashMap::new(),
            pending: HashSet::new(),
            queue: VecDeque::new(),
            running: false,
            failed: HashSet::new(),
        }
    }
}

/// 默认目录：`%APPDATA%\video-editor\proxies`。
#[must_use]
pub(crate) fn default_dir() -> PathBuf {
    crate::recent::default_file()
        .parent()
        .map_or_else(|| PathBuf::from("proxies"), |p| p.join("proxies"))
}

/// 原文件对应的副本路径：路径 + 大小 + 修改时间的散列（原文件换了内容就重新做）。
#[must_use]
pub fn proxy_path(dir: &Path, source: &Path) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut h);
    if let Ok(m) = std::fs::metadata(source) {
        m.len().hash(&mut h);
        if let Ok(t) = m.modified() {
            t.hash(&mut h);
        }
    }
    dir.join(format!("{:016x}.mp4", h.finish()))
}

/// 这个素材要不要做代理：会动的视频，短边超过 `min_short`。
fn wants_proxy(a: &Asset, min_short: u32) -> bool {
    a.video
        .is_some_and(|v| !v.still && v.width.min(v.height) >= min_short)
}

impl Workspace {
    /// 换副本目录、门槛（测试用）。
    pub fn use_proxy_settings(&mut self, dir: PathBuf, min_short: u32) {
        self.proxies.dir = dir;
        self.proxies.min_short = min_short;
    }

    /// 代理模式开着没有。
    #[must_use]
    pub const fn proxy_mode(&self) -> bool {
        self.proxies.on
    }

    /// 正在做的副本个数。
    #[must_use]
    pub fn proxies_pending(&self) -> usize {
        self.proxies.pending.len()
    }

    /// 素材 `id` 的副本做好了没有。
    #[must_use]
    pub fn has_proxy(&self, id: AssetId) -> bool {
        self.doc
            .project
            .assets
            .get(&id)
            .is_some_and(|a| self.proxies.ready.contains_key(&a.path))
    }

    /// 开 / 关代理模式；开的时候把还没有副本的素材都排上。
    pub fn set_proxy_mode(&mut self, on: bool, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.proxies.on = on;
        if on {
            // 重新打开代理模式：之前失败的再试一次
            self.proxies.failed.clear();
            self.ensure_proxies(window, cx);
        }
        self.engine_revision = None;
        self.sync_engine();
        cx.notify();
    }

    /// 给要做代理、还没有副本的素材排队做副本（后台，一个一个来）；已经在目录里的直接用。
    pub(crate) fn ensure_proxies(&mut self, window: &Window, cx: &Context<'_, Self>) {
        if !self.proxies.on {
            return;
        }
        let px = &self.proxies;
        let todo: Vec<PathBuf> = self
            .doc
            .project
            .assets
            .values()
            .filter(|a| wants_proxy(a, px.min_short))
            .map(|a| a.path.clone())
            .filter(|p| {
                !px.ready.contains_key(p) && !px.pending.contains(p) && !px.failed.contains(p)
            })
            .collect();
        for source in todo {
            let out = proxy_path(&self.proxies.dir, &source);
            if out.is_file() {
                self.proxies.ready.insert(source, out);
            } else {
                self.proxies.pending.insert(source.clone());
                self.proxies.queue.push_back(source);
            }
        }
        self.next_proxy(window, cx);
        self.engine_revision = None;
        self.sync_engine();
    }

    /// 没有在做的就开始做队列里的下一个。先写到临时文件，做完整了才改成正式名字
    /// （做到一半失败、程序退出时，不会留下一个会被当成「做好了」的残缺文件）。
    fn next_proxy(&mut self, window: &Window, cx: &Context<'_, Self>) {
        if self.proxies.running {
            return;
        }
        let Some(source) = self.proxies.queue.pop_front() else {
            return;
        };
        self.proxies.running = true;
        let out = proxy_path(&self.proxies.dir, &source);
        let job = cx.background_spawn({
            let (source, out) = (source.clone(), out.clone());
            async move {
                let part = out.with_extension("part.mp4");
                let made = ve_export::make_proxy(&source, &part)
                    .and_then(|()| std::fs::rename(&part, &out).map_err(|e| e.to_string()));
                if made.is_err() {
                    drop(std::fs::remove_file(&part));
                }
                made
            }
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let made = job.await;
            let _ignored = this.update_in(cx, |this, window, cx| {
                this.proxies.running = false;
                this.proxies.pending.remove(&source);
                match made {
                    Ok(()) => {
                        this.proxies.ready.insert(source, out);
                        this.engine_revision = None;
                        this.sync_engine();
                        if this.proxies.pending.is_empty() {
                            notify(Notification::success("代理文件已生成"), window, cx);
                        }
                    }
                    Err(e) => {
                        let name = source
                            .file_name()
                            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                        log::warn!("{name} 的代理没做成：{e}");
                        this.proxies.failed.insert(source);
                        notify(
                            Notification::warning(format!("{name} 的代理没做成：{e}")),
                            window,
                            cx,
                        );
                    }
                }
                this.next_proxy(window, cx);
                cx.notify();
            });
        }));
    }

    /// 播放引擎现在读的素材文件（代理模式下可能是副本）。
    #[must_use]
    pub fn playing_path(&self, id: AssetId) -> Option<PathBuf> {
        self.engine_project.assets.get(&id).map(|a| a.path.clone())
    }

    /// 代理模式开着时，把工程里有副本的素材路径换成副本（给播放引擎）。
    pub(crate) fn with_proxies(&self, project: &mut Project) {
        if !self.proxies.on {
            return;
        }
        for a in project.assets.values_mut() {
            if let Some(p) = self.proxies.ready.get(&a.path) {
                a.path.clone_from(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_names_follow_the_file_and_its_content() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.mp4");
        std::fs::write(&src, "一").unwrap();
        let first = proxy_path(Path::new("p"), &src);
        assert_eq!(
            first,
            proxy_path(Path::new("p"), &src),
            "同一个文件同一个名字"
        );
        std::fs::write(&src, "长一点的内容").unwrap();
        assert_ne!(first, proxy_path(Path::new("p"), &src), "内容变了重新做");
        assert_eq!(first.parent(), Some(Path::new("p")));
    }

    #[test]
    fn only_big_moving_video_wants_a_proxy() {
        let mut a = Asset {
            id: AssetId(1),
            path: "a.mp4".into(),
            name: "a".into(),
            duration: ve_core::Time::from_secs(1),
            video: Some(ve_core::VideoInfo {
                width: 1920,
                height: 1080,
                frame_rate: ve_core::FrameRate::new(30, 1),
                still: false,
            }),
            audio: None,
        };
        assert!(wants_proxy(&a, DEFAULT_MIN_SHORT_SIDE));
        a.video = a.video.map(|v| ve_core::VideoInfo { height: 720, ..v });
        assert!(!wants_proxy(&a, DEFAULT_MIN_SHORT_SIDE), "720p 不做");
        a.video = a.video.map(|v| ve_core::VideoInfo {
            height: 2160,
            still: true,
            ..v
        });
        assert!(!wants_proxy(&a, DEFAULT_MIN_SHORT_SIDE), "图片不做");
        a.video = None;
        assert!(!wants_proxy(&a, 0), "纯音频不做");
    }
}
