//! 全局设置（剪映顶栏菜单「全局设置」）：草稿放哪、缓存（代理文件）放哪 / 占多大 / 清理、
//! 导入图片的默认时长、新草稿的默认帧率。存在 `%APPDATA%\video-editor\settings.json`，启动时读。
//!
//! 倒放、防抖、定格、录音生成的文件是工程里的素材（工程还在用），只显示占多大，不在这里删。

use crate::actions as act;
use crate::workspace::{Workspace, notify};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 导入图片的默认时长可选的几档（秒）。
pub const STILL_CHOICES: [u32; 5] = [3, 5, 8, 10, 15];
/// 新草稿默认帧率可选的几档。
pub const RATE_CHOICES: [u32; 5] = [24, 25, 30, 50, 60];

/// 全局设置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// 草稿（自动保存的新工程）放哪；`None` 用默认位置。
    pub draft_dir: Option<PathBuf>,
    /// 缓存（代理文件）放哪；`None` 用默认位置。
    pub cache_dir: Option<PathBuf>,
    /// 导入图片的默认时长（秒）。
    pub still_secs: u32,
    /// 新草稿的帧率（导入第一段视频后按视频的来）。
    pub frame_rate: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            draft_dir: None,
            cache_dir: None,
            still_secs: 5,
            frame_rate: 30,
        }
    }
}

impl AppSettings {
    /// 设置文件：`%APPDATA%\video-editor\settings.json`。
    #[must_use]
    pub fn default_file() -> PathBuf {
        crate::recent::default_file().parent().map_or_else(
            || PathBuf::from("settings.json"),
            |p| p.join("settings.json"),
        )
    }

    /// 读设置；没有或读不懂时用默认值，范围外的值换回默认。
    #[must_use]
    pub fn load(file: &Path) -> Self {
        let read: Self = std::fs::read(file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let d = Self::default();
        Self {
            still_secs: if (1..=600).contains(&read.still_secs) {
                read.still_secs
            } else {
                d.still_secs
            },
            frame_rate: if (1..=240).contains(&read.frame_rate) {
                read.frame_rate
            } else {
                d.frame_rate
            },
            ..read
        }
    }

    /// 写设置。
    ///
    /// # Errors
    /// 写不进去。
    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(file, json)
    }

    /// 草稿放哪（没设时用默认位置）。
    #[must_use]
    pub fn drafts(&self) -> PathBuf {
        self.draft_dir
            .clone()
            .unwrap_or_else(crate::autosave::default_dir)
    }

    /// 缓存放哪（没设时用默认位置）。
    #[must_use]
    pub fn cache(&self) -> PathBuf {
        self.cache_dir
            .clone()
            .unwrap_or_else(crate::proxy::default_dir)
    }
}

/// 文件夹里所有文件加起来多大（字节，含子文件夹；读不了的跳过）。
#[must_use]
pub fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// 给人看的大小：`0 B`、`12.3 KB`、`4.5 MB`、`1.2 GB`。
#[must_use]
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    #[expect(clippy::cast_precision_loss, reason = "显示用")]
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// 是不是我们生成的代理文件：`16 位十六进制.mp4`，或没做完的 `.part.mp4`。
fn is_proxy(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let stem = name
        .strip_suffix(".part.mp4")
        .or_else(|| name.strip_suffix(".mp4"))
        .unwrap_or("");
    stem.len() == 16 && stem.chars().all(|c| c.is_ascii_hexdigit())
}

/// 缓存文件夹里代理文件一共多大（「清理」能腾出来的就是这些；用户自己放的文件不算）。
#[must_use]
pub fn proxy_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| is_proxy(&e.path()))
        .map(|e| e.metadata().map_or(0, |m| m.len()))
        .sum()
}

/// 删掉缓存文件夹里的代理文件（16 位十六进制名的 `.mp4` 和没做完的 `.part.mp4`），返回删了多少字节。别的文件不碰。
#[must_use = "删了多少字节，给人看"]
pub fn clear_proxies(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut freed = 0;
    for e in entries.flatten() {
        let path = e.path();
        if is_proxy(&path) {
            let len = e.metadata().map_or(0, |m| m.len());
            if std::fs::remove_file(&path).is_ok() {
                freed += len;
            }
        }
    }
    freed
}

impl Workspace {
    /// 现在的全局设置。
    #[must_use]
    pub const fn settings(&self) -> &AppSettings {
        &self.settings
    }

    /// 用这份设置（启动时、改了设置后）；`file` 给了就存进去。
    pub fn use_settings(
        &mut self,
        settings: AppSettings,
        file: Option<PathBuf>,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(a) = self.autosave.as_mut() {
            a.dir = settings.drafts();
        }
        self.use_proxy_cache(settings.cache());
        // 只改还没存过、没动过的新工程；打开的工程用它自己的帧率
        if self.doc.path().is_none() && self.doc.project.assets.is_empty() && !self.doc.is_dirty() {
            self.doc.project.settings.frame_rate = ve_core::FrameRate::new(settings.frame_rate, 1);
        }
        if let Some(file) = file.as_ref().or(self.settings_file.as_ref())
            && let Err(e) = settings.save(file)
        {
            log::warn!("设置没存上：{e}");
        }
        if file.is_some() {
            self.settings_file = file;
        }
        self.settings = settings;
        cx.notify();
    }

    /// 导入的图片片段多长。
    pub(crate) fn still_length(&self) -> ve_core::Time {
        ve_core::Time::from_secs(i64::from(self.settings.still_secs))
    }

    pub(crate) fn open_settings(
        &mut self,
        _: &act::OpenSettings,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let ws = cx.entity();
        let form = cx.new(|_| SettingsForm {
            settings: self.settings.clone(),
            cache_size: proxy_size(&self.settings.cache()),
            generated_size: [
                crate::reverse::default_dir(),
                crate::stabilize_ui::default_dir(),
                crate::freeze::default_dir(),
                crate::record::default_dir(),
            ]
            .iter()
            .map(|d| dir_size(d))
            .sum(),
            ws: ws.clone(),
        });
        window.open_dialog(cx, move |dialog, _, _| {
            let form = form.clone();
            let ws = ws.clone();
            dialog
                .title("全局设置")
                .width(px(560.))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            div()
                                .id("settings-cancel")
                                .test_support()
                                .child(DialogClose::new().trigger(|b| b.ghost().label("取消"))),
                        )
                        .child(
                            DialogAction::new()
                                .child(Button::new("settings-save").primary().label("保存")),
                        ),
                )
                .on_ok(move |_, _, cx| {
                    let s = form.read(cx).settings.clone();
                    ws.update(cx, |this, cx| {
                        let file = this.settings_file.clone();
                        this.use_settings(s, file, cx);
                    });
                    true
                })
        });
    }

    /// 清掉代理缓存（预览时用的低清副本，原片不动；代理模式开着时会重新做）。
    pub(crate) fn clear_proxy_cache(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> u64 {
        let freed = clear_proxies(&self.settings.cache());
        self.forget_proxies();
        self.ensure_proxies(window, cx);
        notify(
            Notification::success(format!("已清理缓存 {}", format_size(freed))),
            window,
            cx,
        );
        freed
    }
}

/// 设置对话框的内容（点「保存」才生效）。
#[derive(Debug)]
pub struct SettingsForm {
    settings: AppSettings,
    cache_size: u64,
    generated_size: u64,
    ws: Entity<Workspace>,
}

impl SettingsForm {
    /// 选一个文件夹，`set` 写进设置。
    fn pick_dir(
        window: &Window,
        cx: &Context<'_, Self>,
        prompt: &'static str,
        set: fn(&mut AppSettings, PathBuf),
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(prompt.into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(dir) = paths.into_iter().next()
            {
                let _ignored = this.update(cx, |this, cx| {
                    set(&mut this.settings, dir);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn dir_row(
        id: &'static str,
        label: &'static str,
        dir: &Path,
        cx: &Context<'_, Self>,
        set: fn(&mut AppSettings, PathBuf),
    ) -> AnyElement {
        h_flex()
            .gap_2()
            .child(row_label(label, cx))
            .child(
                div()
                    .id(SharedString::from(format!("{id}-path")))
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .truncate()
                    .child(SharedString::from(dir.display().to_string())),
            )
            .child(
                Button::new(id)
                    .ghost()
                    .small()
                    .label("更改")
                    .on_click(cx.listener(move |_, _, window, cx| {
                        Self::pick_dir(window, cx, label, set);
                    })),
            )
            .into_any_element()
    }

    fn choices(
        prefix: &'static str,
        options: &[u32],
        current: u32,
        unit: &'static str,
        set: fn(&mut AppSettings, u32),
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        h_flex()
            .gap_1()
            .flex_wrap()
            .children(options.iter().map(|&v| {
                crate::chrome::chip(
                    format!("{prefix}-{v}"),
                    format!("{v}{unit}"),
                    v == current,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    set(&mut this.settings, v);
                    cx.notify();
                }))
            }))
            .into_any_element()
    }
}

fn row_label(text: &'static str, cx: &gpui_kit::App) -> AnyElement {
    div()
        .w(px(96.))
        .flex_none()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

impl Render for SettingsForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let s = &self.settings;
        let muted = cx.theme().muted_foreground;
        v_flex()
            .id("settings-form")
            .test_support()
            .gap_3()
            .child(div().text_sm().child("草稿"))
            .child(Self::dir_row(
                "settings-drafts",
                "草稿位置",
                &s.drafts(),
                cx,
                |st, d| {
                    st.draft_dir = Some(d);
                },
            ))
            .child(div().text_sm().child("缓存"))
            .child(Self::dir_row(
                "settings-cache",
                "缓存位置",
                &s.cache(),
                cx,
                |st, d| {
                    st.cache_dir = Some(d);
                },
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(row_label("代理文件", cx))
                    .child(
                        div()
                            .id("settings-cache-size")
                            .test_support()
                            .flex_1()
                            .text_sm()
                            .child(SharedString::from(format_size(self.cache_size))),
                    )
                    .child(
                        Button::new("settings-clear-cache")
                            .ghost()
                            .small()
                            .label("清理")
                            .on_click(cx.listener(|this, _, window, cx| {
                                // 清理的是现在在用（已保存）的缓存目录，清完也按它重新算
                                let dir = this.ws.update(cx, |ws, cx| {
                                    ws.clear_proxy_cache(window, cx);
                                    ws.settings().cache()
                                });
                                this.cache_size = proxy_size(&dir);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(row_label("生成的素材", cx))
                    .child(
                        div()
                            .text_sm()
                            .child(SharedString::from(format_size(self.generated_size))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("倒放、防抖、定格、录音生成的文件，工程在用，不在这里删"),
                    ),
            )
            .child(div().text_sm().child("剪辑"))
            .child(
                h_flex()
                    .gap_2()
                    .child(row_label("图片默认时长", cx))
                    .child(Self::choices(
                        "settings-still",
                        &STILL_CHOICES,
                        s.still_secs,
                        "秒",
                        |st, v| st.still_secs = v,
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(row_label("新草稿帧率", cx))
                    .child(Self::choices(
                        "settings-rate",
                        &RATE_CHOICES,
                        s.frame_rate,
                        "",
                        |st, v| st.frame_rate = v,
                        cx,
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_fall_back_on_bad_values() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        assert_eq!(AppSettings::load(&file), AppSettings::default());
        let s = AppSettings {
            draft_dir: Some(dir.path().join("草稿")),
            cache_dir: None,
            still_secs: 8,
            frame_rate: 25,
        };
        s.save(&file).unwrap();
        assert_eq!(AppSettings::load(&file), s);
        std::fs::write(&file, r#"{"still_secs": 0, "frame_rate": 9999}"#).unwrap();
        let bad = AppSettings::load(&file);
        assert_eq!((bad.still_secs, bad.frame_rate), (5, 30));
        std::fs::write(&file, "不是 json").unwrap();
        assert_eq!(AppSettings::load(&file), AppSettings::default());
    }

    #[test]
    fn sizes_are_counted_and_only_proxy_files_are_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(p.join("0123456789abcdef.mp4"), [0u8; 1000]).unwrap();
        std::fs::write(p.join("fedcba9876543210.part.mp4"), [0u8; 24]).unwrap();
        std::fs::write(p.join("我的视频.mp4"), [0u8; 10]).unwrap();
        std::fs::create_dir(p.join("子")).unwrap();
        std::fs::write(p.join("子").join("a.bin"), [0u8; 5]).unwrap();
        assert_eq!(dir_size(p), 1039);
        // 缓存那一栏只算代理文件（和「清理」删的是同一批）
        assert_eq!(proxy_size(p), 1024);
        assert_eq!(clear_proxies(p), 1024);
        assert!(p.join("我的视频.mp4").exists(), "别的文件不碰");
        assert_eq!(dir_size(p), 15);
        assert_eq!(proxy_size(p), 0);
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    }
}
