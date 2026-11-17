//! 导入：探测文件生成素材；后台预计算缩略图和波形。不依赖界面框架。

use std::path::Path;
use std::sync::Arc;
use ve_core::{Asset, AssetId};
use ve_media::VideoFrame;

/// 每个素材的缩略图张数。
pub const THUMBNAIL_COUNT: usize = 8;
/// 缩略图高度（像素）。
pub const THUMBNAIL_HEIGHT: u32 = 48;
/// 波形每秒桶数。
pub const PEAKS_PER_SECOND: u32 = 50;

/// 素材的预计算结果。
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    /// 均匀分布的缩略图（视频才有）。
    pub thumbnails: Vec<Arc<VideoFrame>>,
    /// 波形（音频才有），每秒 [`PEAKS_PER_SECOND`] 桶。
    pub peaks: Vec<(f32, f32)>,
}

/// 探测文件，生成素材（`id` 由加入工程时分配）。
///
/// # Errors
/// 不是可识别的媒体文件时返回给用户看的说明。
pub fn probe_asset(path: &Path) -> Result<Asset, String> {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let r = ve_media::probe(path).map_err(|e| format!("无法导入 {name}：{e}"))?;
    // 存绝对路径：导出时 ffmpeg 在临时目录运行，工程文件也按绝对 + 相对两份记
    let path = std::path::absolute(path).map_err(|e| format!("无法导入 {name}：{e}"))?;
    Ok(Asset {
        id: AssetId(0),
        path,
        name,
        duration: r.duration,
        video: r.video,
        audio: r.audio,
    })
}

/// 计算缩略图和波形（耗时，放后台线程）。失败的部分留空并记日志。
#[must_use]
pub fn analyze(asset: &Asset) -> Analysis {
    let mut out = Analysis::default();
    if asset.video.is_some() {
        match ve_media::thumbnails(&asset.path, THUMBNAIL_COUNT, THUMBNAIL_HEIGHT) {
            Ok(t) => out.thumbnails = t.into_iter().map(Arc::new).collect(),
            Err(e) => log::warn!("缩略图失败 {}：{e}", asset.path.display()),
        }
    }
    if asset.audio.is_some() && asset.video.is_none() {
        match ve_media::waveform_peaks(&asset.path, PEAKS_PER_SECOND) {
            Ok(p) => out.peaks = p,
            Err(e) => log::warn!("波形失败 {}：{e}", asset.path.display()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_media::testing::{Fixture, fixture};

    #[test]
    fn probe_asset_fills_streams_and_name() {
        let a = probe_asset(&fixture(Fixture::VideoWithAudio)).unwrap();
        assert_eq!(a.name, "video_with_audio.mp4");
        assert!(a.video.is_some() && a.audio.is_some());
        assert!(a.duration > ve_core::Time::from_millis(1900));
    }

    #[test]
    fn probe_asset_stores_absolute_path() {
        let cwd = std::env::current_dir().unwrap();
        let rel = pathdiff::diff_paths(fixture(Fixture::FrameIndexed30), &cwd).unwrap();
        assert!(rel.is_relative());
        assert!(probe_asset(&rel).unwrap().path.is_absolute());
    }

    #[test]
    fn unsupported_file_gives_readable_reason() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("说明.txt");
        std::fs::write(&path, "不是视频").unwrap();
        let err = probe_asset(&path).unwrap_err();
        assert!(err.contains("说明.txt"), "{err}");
    }

    #[test]
    fn analyze_video_and_audio() {
        let video = probe_asset(&fixture(Fixture::FrameIndexed30)).unwrap();
        let a = analyze(&video);
        assert_eq!(a.thumbnails.len(), THUMBNAIL_COUNT);
        // 不放大：源只有 36 像素高
        assert_eq!(a.thumbnails[0].height, 36.min(THUMBNAIL_HEIGHT));
        assert_eq!(a.peaks.len(), 0);

        let tone = probe_asset(&fixture(Fixture::ToneStereo48k)).unwrap();
        let b = analyze(&tone);
        assert_eq!(b.thumbnails.len(), 0);
        assert_eq!(b.peaks.len(), 2 * PEAKS_PER_SECOND as usize);
        assert!(b.peaks.iter().all(|&(lo, hi)| lo < -0.4 && hi > 0.4));
    }

    #[test]
    fn analyze_missing_file_is_empty_not_panic() {
        let mut a = probe_asset(&fixture(Fixture::VideoWithAudio)).unwrap();
        a.path = "D:/不存在/x.mp4".into();
        let r = analyze(&a);
        assert!(r.thumbnails.is_empty() && r.peaks.is_empty());
    }
}
