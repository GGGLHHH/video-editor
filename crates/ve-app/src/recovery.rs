//! 崩溃恢复：运行时在会话目录里记一笔「进程号 → 开着哪个工程」，正常退出时删掉。
//! 下次启动发现有记录、那个进程却已经不在了，说明上次没有正常退出（崩溃、断电、被强行结束），
//! 就重新打开它当时的工程。工程本身由自动保存随时存好（停手 2 秒），最多丢最后两秒的改动。

use crate::workspace::{Workspace, notify};
use gpui_kit::component::notification::Notification;
use gpui_kit::{Context, Window};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 一条会话记录。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// 当时开着的工程（还没保存过的新工程为空）。
    pub project: Option<PathBuf>,
    /// 那个进程的启动时刻（系统给的数，只用来比相等）：进程号会被别的进程重新用上，
    /// 号对上、启动时刻也对上才算「还在运行」。
    #[serde(default)]
    pub started: Option<u64>,
}

/// 超过这么久的记录不再恢复（直接删掉）。
const STALE: std::time::Duration = std::time::Duration::from_hours(7 * 24);

/// 没有正常结束的一次会话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crashed {
    /// 当时的进程号。
    pub pid: u32,
    /// 当时的记录。
    pub session: Session,
    /// 记录最后更新的时刻（多个时取最近的）。
    pub modified: std::time::SystemTime,
}

/// 会话目录（每个运行中的进程一个 `<进程号>.json`）。
#[derive(Debug, Clone)]
pub struct Sessions {
    dir: PathBuf,
}

impl Sessions {
    /// 用目录 `dir`。
    #[must_use]
    pub const fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// 默认位置：`%APPDATA%\video-editor\sessions`。
    #[must_use]
    pub fn default_dir() -> PathBuf {
        crate::recent::default_file()
            .parent()
            .map_or_else(|| PathBuf::from("sessions"), |p| p.join("sessions"))
    }

    fn file(&self, pid: u32) -> PathBuf {
        self.dir.join(format!("{pid}.json"))
    }

    /// 记下进程 `pid` 现在开着的工程。
    ///
    /// # Errors
    /// 写不进去。
    pub fn record(&self, pid: u32, project: Option<&Path>) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let s = Session {
            project: project.map(Path::to_path_buf),
            started: process_started(pid),
        };
        let json = serde_json::to_vec(&s).map_err(std::io::Error::other)?;
        std::fs::write(self.file(pid), json)
    }

    /// 删掉进程 `pid` 的记录（正常退出、恢复完）。
    pub fn clear(&self, pid: u32) {
        drop(std::fs::remove_file(self.file(pid)));
    }

    /// 没有正常结束的会话（记录在、进程不在），最近的排前面。读不懂的记录当作没有工程。
    /// `alive(进程号, 记下的启动时刻)` 判断那个进程还在不在。
    #[must_use]
    pub fn crashed(&self, alive: impl Fn(u32, Option<u64>) -> bool) -> Vec<Crashed> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<Crashed> = entries
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let pid: u32 = path
                    .extension()
                    .is_some_and(|x| x == "json")
                    .then(|| path.file_stem()?.to_str()?.parse().ok())??;
                let session: Session = std::fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                if alive(pid, session.started) {
                    return None;
                }
                let modified = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                Some(Crashed {
                    pid,
                    session,
                    modified,
                })
            })
            .collect();
        out.sort_by_key(|c| std::cmp::Reverse(c.modified));
        out
    }
}

/// 进程 `pid` 还在不在；给了 `started` 时还要启动时刻对得上（不是后来用上这个号的别的进程）。
#[must_use]
pub fn process_alive(pid: u32, started: Option<u64>) -> bool {
    imp::process_alive(pid) && started.is_none_or(|s| imp::process_started(pid) == Some(s))
}

/// 进程 `pid` 的启动时刻（查不到为 `None`）。
#[must_use]
pub fn process_started(pid: u32) -> Option<u64> {
    imp::process_started(pid)
}

#[cfg(windows)]
#[expect(unsafe_code, reason = "查进程是否还在：标准库没有这个接口")]
mod imp {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub(super) fn process_started(pid: u32) -> Option<u64> {
        // SAFETY: OpenProcess 只读参数；拿到的句柄下面一定关掉。
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: handle 有效；四个出参都是可写的局部变量。
        let ok = unsafe {
            GetProcessTimes(
                handle,
                &raw mut created,
                &raw mut exited,
                &raw mut kernel,
                &raw mut user,
            )
        } != 0;
        // SAFETY: handle 有效且之后不再使用。
        unsafe { CloseHandle(handle) };
        ok.then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }

    pub(super) fn process_alive(pid: u32) -> bool {
        // SAFETY: OpenProcess 只读参数；拿到的句柄下面一定关掉。
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        // SAFETY: handle 是刚打开的有效进程句柄，code 是可写的局部变量。
        let ok = unsafe { GetExitCodeProcess(handle, &raw mut code) } != 0;
        // SAFETY: handle 有效且之后不再使用。
        unsafe { CloseHandle(handle) };
        ok && u32::try_from(STILL_ACTIVE).is_ok_and(|still| code == still)
    }
}

#[cfg(not(windows))]
mod imp {
    /// 别的系统上不判断：都当作还在（不做恢复）。
    pub(super) const fn process_alive(_pid: u32) -> bool {
        true
    }

    pub(super) const fn process_started(_pid: u32) -> Option<u64> {
        None
    }
}

/// 上次写进记录的工程。
#[derive(Debug, PartialEq, Eq)]
enum Recorded {
    /// 还没写过。
    Nothing,
    /// 写过这个（`None` = 还没保存过的新工程）。
    Project(Option<PathBuf>),
}

/// 运行中的会话记录。
#[derive(Debug)]
pub(crate) struct Live {
    sessions: Sessions,
    pid: u32,
    /// 记过没有、上次记下的工程（变了才重写）。
    recorded: Recorded,
}

impl Workspace {
    /// 打开崩溃恢复：上次没有正常退出、这次也没指定要打开的工程时，重新打开上次的工程；
    /// 之后一直记着本进程开着哪个工程，窗口正常关闭时删掉记录。
    pub fn enable_recovery(
        &mut self,
        sessions: Sessions,
        pid: u32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let now = std::time::SystemTime::now();
        let mut crashed = sessions.crashed(process_alive);
        // 恢复不了的（太旧、没存过、文件没了）直接删掉
        crashed.retain(|c| {
            let fresh = now.duration_since(c.modified).is_ok_and(|age| age < STALE);
            let usable = fresh && c.session.project.as_deref().is_some_and(Path::exists);
            if !usable {
                sessions.clear(c.pid);
            }
            usable
        });
        // 这次没指定工程：重新打开最近那个，只删它的记录；别的留着下次再恢复
        // （两个窗口一起崩了，一次开一个）。这次指定了工程就都留着。
        let reopen = crashed
            .first()
            .filter(|_| self.doc.path().is_none() && !self.doc.is_dirty());
        if let Some(c) = reopen
            && let Some(path) = c.session.project.clone()
        {
            sessions.clear(c.pid);
            self.open_path(&path, window, cx);
            let name = path
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
            notify(
                Notification::info(format!(
                    "上次没有正常退出，已重新打开「{name}」（自动保存最多丢最后两秒的改动）"
                )),
                window,
                cx,
            );
        }
        self.recovery = Some(Live {
            sessions,
            pid,
            recorded: Recorded::Nothing,
        });
        self.sync_session();
        cx.on_release(|this: &mut Self, _| {
            if let Some(live) = this.recovery.take() {
                live.sessions.clear(live.pid);
            }
        })
        .detach();
    }

    /// 工程换了（打开、另存为、新草稿第一次保存）就更新记录。
    pub(crate) fn sync_session(&mut self) {
        let path = self.doc.path().map(Path::to_path_buf);
        let Some(live) = self.recovery.as_mut() else {
            return;
        };
        if live.recorded == Recorded::Project(path.clone()) {
            return;
        }
        if let Err(e) = live.sessions.record(live.pid, path.as_deref()) {
            log::warn!("崩溃恢复记录没写上：{e}");
        }
        live.recorded = Recorded::Project(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crashed_sessions_are_the_recorded_ones_whose_process_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let s = Sessions::new(dir.path().join("sessions"));
        assert_eq!(s.crashed(|_, _| false), [], "没有目录也没事");
        s.record(11, Some(Path::new("甲.vep"))).unwrap();
        s.record(22, None).unwrap();
        std::fs::write(dir.path().join("sessions").join("不是进程号.json"), "{}").unwrap();
        std::fs::write(dir.path().join("sessions").join("33.json"), "坏了").unwrap();
        // 22 还在运行
        let gone = s.crashed(|pid, _| pid == 22);
        let mut pids: Vec<u32> = gone.iter().map(|c| c.pid).collect();
        pids.sort_unstable();
        assert_eq!(pids, [11, 33]);
        let eleven = gone.iter().find(|c| c.pid == 11).unwrap();
        assert_eq!(eleven.session.project.as_deref(), Some(Path::new("甲.vep")));
        let broken = gone.iter().find(|c| c.pid == 33).unwrap();
        assert_eq!(broken.session, Session::default(), "读不懂当作没有工程");
        s.clear(11);
        assert_eq!(s.crashed(|pid, _| pid == 22).len(), 1);
    }

    #[test]
    fn this_process_is_alive_and_a_made_up_one_is_not() {
        let me = std::process::id();
        assert!(process_alive(me, None));
        assert!(process_alive(me, process_started(me)));
        if cfg!(windows) {
            assert!(!process_alive(u32::MAX - 1, None));
            // 号对上、启动时刻对不上：是后来用上这个号的别的进程
            let other = process_started(me).map(|t| t + 1);
            assert!(!process_alive(me, other));
        }
    }

    #[test]
    fn a_reused_process_number_still_counts_as_crashed() {
        let dir = tempfile::tempdir().unwrap();
        let s = Sessions::new(dir.path().to_path_buf());
        let me = std::process::id();
        s.record(me, Some(Path::new("甲.vep"))).unwrap();
        assert_eq!(s.crashed(process_alive), [], "记的就是本进程");
        if cfg!(windows) {
            // 记录里的启动时刻换成别的：当年那个进程已经没了，号被本进程用上了
            let file = dir.path().join(format!("{me}.json"));
            let mut session: Session =
                serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
            session.started = session.started.map(|t| t ^ 1);
            std::fs::write(&file, serde_json::to_vec(&session).unwrap()).unwrap();
            assert_eq!(s.crashed(process_alive).len(), 1);
        }
    }
}
