//! 最近打开的工程：最多 [`MAX_RECENT`] 个，最近的在前，存在用户配置目录的 JSON 文件里。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 最多记几个。
pub const MAX_RECENT: usize = 10;

/// 最近打开的工程列表。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentList {
    paths: Vec<PathBuf>,
}

/// Windows 路径不区分大小写，比较时统一小写。
fn key(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").to_lowercase()
}

impl RecentList {
    /// 列表（最近的在前）。
    #[must_use]
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// 记一次打开 / 保存：放到最前，去重，超出上限丢最旧的。
    pub fn push(&mut self, path: PathBuf) {
        let k = key(&path);
        self.paths.retain(|p| key(p) != k);
        self.paths.insert(0, path);
        self.paths.truncate(MAX_RECENT);
    }

    /// 移除一项。
    pub fn remove(&mut self, path: &Path) {
        let k = key(path);
        self.paths.retain(|p| key(p) != k);
    }

    /// 清空。
    pub fn clear(&mut self) {
        self.paths.clear();
    }

    /// 从文件读；文件不存在或内容坏了时返回空列表。
    #[must_use]
    pub fn load(file: &Path) -> Self {
        std::fs::read(file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// 写入文件（自动建目录）。
    ///
    /// # Errors
    /// 建目录或写文件失败。
    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(file, json)
    }
}

/// 默认存放位置：`%APPDATA%\video-editor\recent.json`（取不到 APPDATA 时放当前目录）。
#[must_use]
pub fn default_file() -> PathBuf {
    std::env::var_os("APPDATA")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join("video-editor")
        .join("recent.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_moves_to_front_dedupes_and_caps() {
        let mut r = RecentList::default();
        for i in 0..12 {
            r.push(PathBuf::from(format!("D:/p/{i}.vep")));
        }
        assert_eq!(r.paths().len(), MAX_RECENT);
        assert_eq!(r.paths()[0], PathBuf::from("D:/p/11.vep"));
        // 已有的再打开一次：移到最前，不重复（Windows 路径不分大小写）
        r.push(PathBuf::from("d:/P/5.VEP"));
        assert_eq!(r.paths()[0], PathBuf::from("d:/P/5.VEP"));
        assert_eq!(
            r.paths()
                .iter()
                .filter(|p| p.to_string_lossy().to_lowercase() == "d:/p/5.vep")
                .count(),
            1
        );
        assert_eq!(r.paths().len(), MAX_RECENT);
    }

    #[test]
    fn remove_and_clear() {
        let mut r = RecentList::default();
        r.push("a.vep".into());
        r.push("b.vep".into());
        r.remove(Path::new("A.vep"));
        assert_eq!(r.paths(), &[PathBuf::from("b.vep")]);
        r.clear();
        assert_eq!(r.paths().len(), 0);
    }

    #[test]
    fn save_and_load_round_trip_and_tolerate_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("子目录").join("recent.json");
        let mut r = RecentList::default();
        r.push("D:/工程/一.vep".into());
        r.push("D:/工程/二.vep".into());
        r.save(&file).unwrap();
        assert_eq!(RecentList::load(&file), r);
        // 文件不存在或内容坏了：当作空列表，不报错
        assert_eq!(
            RecentList::load(&dir.path().join("没有.json")),
            RecentList::default()
        );
        std::fs::write(&file, "not json").unwrap();
        assert_eq!(RecentList::load(&file), RecentList::default());
    }

    #[test]
    fn default_location_is_under_appdata() {
        let p = default_file();
        assert!(
            p.ends_with(Path::new("video-editor").join("recent.json")),
            "{}",
            p.display()
        );
    }
}
