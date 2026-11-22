//! 自动保存（剪映：草稿随时自动存到本地，顶栏显示「HH:MM:SS 自动保存本地」）。
//!
//! 改动停下 [`QUIET`] 以后保存；还没保存过的新工程按日期命名（「11月18日」，重名加「(2)」）
//! 存进草稿目录。只有正式启动的程序打开自动保存，测试里默认关着。

use crate::workspace::Workspace;
use gpui_kit::{Context, Window};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 改动停下多久后保存。
pub const QUIET: Duration = Duration::from_secs(2);
/// 多久检查一次。
const TICK: Duration = Duration::from_millis(500);

/// 自动保存的状态。
#[derive(Debug, Clone)]
pub struct Autosave {
    /// 新工程存到这里。
    pub dir: PathBuf,
    /// 上次看到的修改计数和看到它的时刻（改动停下多久了）。
    seen: Option<(u64, Instant)>,
    /// 上次自动保存的时刻（本地时间 HH:MM:SS），给顶栏显示。
    pub last_saved: Option<String>,
    /// 上次自动保存失败的原因。
    pub error: Option<String>,
}

/// 本地时间：（月，日，时，分，秒）。Windows 上读系统本地时间，其他系统用 UTC。
#[must_use]
pub fn local_now() -> (u32, u32, u32, u32, u32) {
    #[cfg(windows)]
    #[expect(unsafe_code, reason = "读系统本地时间：标准库没有时区")]
    {
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        // SAFETY: GetLocalTime 只往传入的结构体里写
        let t = unsafe {
            let mut t = std::mem::zeroed();
            GetLocalTime(&raw mut t);
            t
        };
        (
            u32::from(t.wMonth),
            u32::from(t.wDay),
            u32::from(t.wHour),
            u32::from(t.wMinute),
            u32::from(t.wSecond),
        )
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        utc_from_unix(secs)
    }
}

/// UNIX 秒数 → UTC 的（月，日，时，分，秒）。
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    reason = "日期分量都很小"
)]
pub const fn utc_from_unix(secs: u64) -> (u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant 的 civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        month as u32,
        day as u32,
        (rem / 3600) as u32,
        (rem / 60 % 60) as u32,
        (rem % 60) as u32,
    )
}

/// 新草稿的名字：「11月18日」。
#[must_use]
pub fn draft_name(month: u32, day: u32) -> String {
    format!("{month}月{day}日")
}

/// `dir` 里还没被占用的 `<stem>.vep`；重名时加「 (2)」「 (3)」……
#[must_use]
pub fn unique_path(dir: &Path, stem: &str) -> PathBuf {
    let ext = crate::project_io::PROJECT_EXTENSION;
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..10_000)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

impl Workspace {
    /// 打开自动保存，新工程存到 `dir`。
    pub fn enable_autosave(&mut self, dir: PathBuf, window: &Window, cx: &Context<'_, Self>) {
        self.autosave = Some(Autosave {
            dir,
            seen: None,
            last_saved: None,
            error: None,
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let alive = this
                    .update_in(cx, |this, _, cx| this.autosave_tick(cx))
                    .is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    /// 检查一次：有未保存的改动、改动停下够久、没在拖动，就保存。
    fn autosave_tick(&mut self, cx: &mut Context<'_, Self>) {
        let now = cx.background_executor().now();
        let revision = self.doc.revision();
        let Some(a) = self.autosave.as_mut() else {
            return;
        };
        match a.seen {
            Some((seen, _)) if seen == revision => {}
            _ => {
                a.seen = Some((revision, now));
                return;
            }
        }
        let quiet = a
            .seen
            .is_some_and(|(_, at)| now.duration_since(at) >= QUIET);
        if !self.doc.is_dirty() || !quiet || self.doc.in_gesture() {
            return;
        }
        self.autosave_now(cx);
    }

    /// 马上自动保存一次（有改动时；退出前也调它）：新草稿按日期命名存进草稿目录。
    /// 没开自动保存、没有改动时什么都不做。
    pub(crate) fn autosave_now(&mut self, cx: &mut Context<'_, Self>) {
        let Some(a) = self.autosave.as_ref() else {
            return;
        };
        if !self.doc.is_dirty() {
            return;
        }
        // 在复合片段里面时存的是整个工程（里面的改动写回去之后的样子），路径用最外层的
        let dir = a.dir.clone();
        let path = self.outermost_path().map_or_else(
            || {
                let (month, day, ..) = local_now();
                unique_path(&dir, &draft_name(month, day))
            },
            Path::to_path_buf,
        );
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            log::warn!("建不了草稿目录 {}：{e}", dir.display());
        }
        let result = if self.compound_stack.is_empty() {
            crate::project_io::save(&mut self.doc, &path)
        } else {
            let whole = self.whole_project();
            let path = crate::project_io::with_extension(&path);
            ve_core::save_project(&whole, &path)
                .map_err(|e| format!("保存失败：{e}"))
                .map(|()| {
                    self.mark_whole_saved(&path);
                    path
                })
        };
        match result {
            Ok(saved) => {
                let (.., h, m, s) = local_now();
                if let Some(a) = self.autosave.as_mut() {
                    a.last_saved = Some(format!("{h:02}:{m:02}:{s:02}"));
                    a.error = None;
                }
                self.remember_recent(saved);
                self.sync_session();
            }
            Err(e) => {
                log::warn!("自动保存失败：{e}");
                if let Some(a) = self.autosave.as_mut() {
                    a.error = Some(e);
                }
            }
        }
        cx.notify();
    }
}

/// 默认草稿目录：`%APPDATA%/video-editor/drafts`。
#[must_use]
pub fn default_dir() -> PathBuf {
    crate::recent::default_file()
        .parent()
        .map_or_else(|| PathBuf::from("drafts"), |p| p.join("drafts"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_unique_paths() {
        assert_eq!(draft_name(11, 18), "11月18日");
        let dir = tempfile::tempdir().unwrap();
        let first = unique_path(dir.path(), "11月18日");
        assert_eq!(first, dir.path().join("11月18日.vep"));
        std::fs::write(&first, "x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "11月18日"),
            dir.path().join("11月18日 (2).vep")
        );
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc_from_unix(0), (1, 1, 0, 0, 0));
        // 2026-11-18 01:02:03 UTC
        assert_eq!(utc_from_unix(1_794_963_723), (11, 18, 1, 2, 3));
        // 闰年 2 月 29 日
        assert_eq!(utc_from_unix(1_709_164_800), (2, 29, 0, 0, 0));
        let (month, day, h, m, s) = local_now();
        assert!((1..=12).contains(&month) && (1..=31).contains(&day));
        assert!(h < 24 && m < 60 && s < 61);
    }
}
