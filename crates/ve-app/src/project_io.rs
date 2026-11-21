//! 工程存取：保存 / 打开，打开时列出找不到的素材。不依赖界面框架。

use crate::document::Document;
use std::path::{Path, PathBuf};

/// 工程文件扩展名。
pub const PROJECT_EXTENSION: &str = "vep";

/// 打开的结果。
#[derive(Debug)]
pub struct Opened {
    /// 文档。
    pub document: Document,
    /// 找不到的素材文件名（给用户看）。
    pub missing: Vec<String>,
}

/// 保存到 `path`（没有扩展名时补 `.vep`），成功后文档标为已保存。返回实际路径。
///
/// # Errors
/// 写文件失败时返回给用户看的说明。
pub fn save(doc: &mut Document, path: &Path) -> Result<PathBuf, String> {
    let path = with_extension(path);
    ve_core::save_project(&doc.project, &path).map_err(|e| format!("保存失败：{e}"))?;
    doc.mark_saved(path.clone());
    Ok(path)
}

/// 没有扩展名时补 `.vep`。
#[must_use]
pub fn with_extension(path: &Path) -> PathBuf {
    if path.extension().is_some() {
        path.to_path_buf()
    } else {
        path.with_extension(PROJECT_EXTENSION)
    }
}

/// 打开工程文件。
///
/// # Errors
/// 读不到或格式不对时返回给用户看的说明。
pub fn open(path: &Path) -> Result<Opened, String> {
    let loaded =
        ve_core::load_project(path).map_err(|e| format!("打不开 {}：{e}", path.display()))?;
    let missing = loaded
        .missing_assets
        .iter()
        .filter_map(|id| loaded.project.assets.get(id))
        .map(|a| a.name.clone())
        .collect();
    Ok(Opened {
        document: Document::from_project(loaded.project, Some(path.to_path_buf())),
        missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::edit::append_asset;
    use ve_media::testing::{Fixture, fixture};

    fn doc_with_media(dir: &Path) -> Document {
        let mut d = Document::new();
        let src = fixture(Fixture::VideoWithAudio);
        let copy = dir.join("素材.mp4");
        std::fs::copy(&src, &copy).unwrap();
        let asset = crate::import::probe_asset(&copy).unwrap();
        let id = d.add_asset(asset);
        d.edit(|p| append_asset(p, id)).unwrap();
        d
    }

    #[test]
    fn save_then_open_restores_timeline() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = doc_with_media(dir.path());
        let file = dir.path().join("工程.vep");
        save(&mut d, &file).unwrap();
        assert!(!d.is_dirty());
        assert_eq!(d.title(), "工程");
        let opened = open(&file).unwrap();
        assert_eq!(opened.document.project, d.project);
        assert_eq!(opened.missing, Vec::<String>::new());
        assert_eq!(opened.document.path(), Some(file.as_path()));
        assert!(!opened.document.can_undo());
    }

    #[test]
    fn missing_asset_notice_lists_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = doc_with_media(dir.path());
        let file = dir.path().join("a.vep");
        save(&mut d, &file).unwrap();
        std::fs::remove_file(dir.path().join("素材.mp4")).unwrap();
        let opened = open(&file).unwrap();
        assert_eq!(opened.missing, vec!["素材.mp4".to_owned()]);
    }

    #[test]
    fn save_adds_extension_and_reports_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Document::new();
        let saved = save(&mut d, &dir.path().join("无后缀")).unwrap();
        assert_eq!(
            saved.extension().and_then(|e| e.to_str()),
            Some(PROJECT_EXTENSION)
        );
        assert!(save(&mut d, &dir.path().join("没有这个目录").join("a.vep")).is_err());
        assert!(
            open(&dir.path().join("nope.vep"))
                .unwrap_err()
                .contains("nope.vep")
        );
    }
}
