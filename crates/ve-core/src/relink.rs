//! 素材丢失后重新链接（剪映「链接媒体」）：给找不到的素材指一个新文件；
//! 同一批丢失的素材常常是一起搬走的，所以顺便到新文件所在的目录里按原文件名把别的也找回来。
//!
//! 文件存不存在由调用方判断（传 `exists`），这里不碰文件系统，方便测试。

use crate::{Asset, AssetId, ClipContent, Project, Time};
use std::path::Path;
use thiserror::Error;

/// 换不上的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RelinkError {
    /// 素材不存在。
    #[error("素材不存在")]
    AssetNotFound(AssetId),
    /// 新文件少了原素材有的画面或声音。
    #[error("这个文件和原素材不一样：原素材有{0}，它没有")]
    KindMismatch(&'static str),
    /// 新文件比时间轴上用到的部分短。
    #[error("这个文件太短，时间轴上用到的部分放不下")]
    TooShort,
}

/// 现在找不到文件的素材（按 id 顺序）。
#[must_use]
pub fn missing_assets(project: &Project, exists: impl Fn(&Path) -> bool) -> Vec<AssetId> {
    project
        .assets
        .values()
        .filter(|a| !exists(&a.path))
        .map(|a| a.id)
        .collect()
}

/// 时间轴上用到素材 `asset` 的最远位置（素材内时间）；没用到时为零。倒放片段的原素材也算。
#[must_use]
pub fn used_until(project: &Project, asset: AssetId) -> Time {
    project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter_map(|c| {
            let span = c.source_span();
            let direct = match c.content {
                ClipContent::Media {
                    asset: a,
                    source_in,
                } if a == asset => Some(source_in + span),
                _ => None,
            };
            let reversed = c
                .reversed
                .filter(|&(a, _)| a == asset)
                .map(|(_, source_in)| source_in + span);
            let stabilized = c
                .stabilized
                .filter(|&(a, ..)| a == asset)
                .map(|(_, source_in, _)| source_in + span);
            direct.max(reversed).max(stabilized)
        })
        .max()
        .unwrap_or(Time::ZERO)
}

/// 把素材 `asset` 换成探测好的文件 `found`（路径、时长、流信息用新的，id 和名字不变），
/// 再到 `found` 所在目录里按原文件名找回别的丢失素材（只改路径）。
/// 返回所有重新链接上的素材，`asset` 排第一。
///
/// # Errors
/// 素材不存在；新文件少了原来的画面或声音；新文件比时间轴上用到的部分短。
pub fn relink(
    project: &mut Project,
    asset: AssetId,
    found: &Asset,
    exists: impl Fn(&Path) -> bool,
) -> Result<Vec<AssetId>, RelinkError> {
    let old = project
        .assets
        .get(&asset)
        .ok_or(RelinkError::AssetNotFound(asset))?;
    if old.video.is_some() && found.video.is_none() {
        return Err(RelinkError::KindMismatch("画面"));
    }
    if old.audio.is_some() && found.audio.is_none() {
        return Err(RelinkError::KindMismatch("声音"));
    }
    if used_until(project, asset) > found.source_limit() {
        return Err(RelinkError::TooShort);
    }
    let dir = found.path.parent().map(Path::to_path_buf);
    let mut done = vec![asset];
    if let Some(slot) = project.assets.get_mut(&asset) {
        slot.path.clone_from(&found.path);
        slot.duration = found.duration;
        slot.video = found.video;
        slot.audio = found.audio;
    }
    let Some(dir) = dir else {
        return Ok(done);
    };
    for other in project.assets.values_mut() {
        if other.id == asset || exists(&other.path) {
            continue;
        }
        let Some(name) = other.path.file_name() else {
            continue;
        };
        let candidate = dir.join(name);
        if exists(&candidate) {
            other.path = candidate;
            done.push(other.id);
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::sample_project;
    use std::path::PathBuf;

    fn probed(path: &str, secs: i64, video: bool, audio: bool) -> Asset {
        Asset {
            id: AssetId(0),
            path: PathBuf::from(path),
            name: String::new(),
            duration: Time::from_secs(secs),
            video: video.then(crate::testing::video_info),
            audio: audio.then(crate::testing::audio_info),
        }
    }

    #[test]
    fn missing_assets_follow_the_exists_check() {
        let (p, ids) = sample_project();
        let all = missing_assets(&p, |_| false);
        assert_eq!(all, vec![ids.video_asset, ids.audio_asset]);
        let some = missing_assets(&p, |path| path == Path::new("bgm.mp3"));
        assert_eq!(some, vec![ids.video_asset]);
    }

    #[test]
    fn relinking_one_finds_the_others_in_the_same_folder() {
        let (mut p, ids) = sample_project();
        let on_disk = [PathBuf::from("新/clip.mp4"), PathBuf::from("新/bgm.mp3")];
        let exists = |path: &Path| on_disk.iter().any(|d| d == path);
        let found = probed("新/clip.mp4", 10, true, true);
        let done = relink(&mut p, ids.video_asset, &found, exists).unwrap();
        assert_eq!(done, vec![ids.video_asset, ids.audio_asset]);
        assert_eq!(p.assets[&ids.audio_asset].path, PathBuf::from("新/bgm.mp3"));
        assert_eq!(p.assets[&ids.video_asset].name, "clip.mp4", "名字不变");
        assert_eq!(missing_assets(&p, exists), Vec::<AssetId>::new());
    }

    #[test]
    fn relinking_takes_the_new_file_info_and_skips_files_still_missing() {
        let (mut p, ids) = sample_project();
        let found = probed("别处/改名了.mp4", 12, true, true);
        let done = relink(&mut p, ids.video_asset, &found, |_| false).unwrap();
        assert_eq!(done, vec![ids.video_asset]);
        assert_eq!(p.assets[&ids.video_asset].duration, Time::from_secs(12));
        assert_eq!(p.assets[&ids.audio_asset].path, PathBuf::from("bgm.mp3"));
    }

    #[test]
    fn a_file_without_picture_or_sound_or_too_short_is_refused() {
        let (mut p, ids) = sample_project();
        let before = p.clone();
        let no_video = probed("x.mp3", 10, false, true);
        assert_eq!(
            relink(&mut p, ids.video_asset, &no_video, |_| false),
            Err(RelinkError::KindMismatch("画面"))
        );
        let no_audio = probed("x.mp4", 10, true, false);
        assert_eq!(
            relink(&mut p, ids.video_asset, &no_audio, |_| false),
            Err(RelinkError::KindMismatch("声音"))
        );
        // 第二段用到素材 5–8 秒
        assert_eq!(used_until(&p, ids.video_asset), Time::from_secs(8));
        let short = probed("x.mp4", 7, true, true);
        assert_eq!(
            relink(&mut p, ids.video_asset, &short, |_| false),
            Err(RelinkError::TooShort)
        );
        assert_eq!(
            relink(&mut p, AssetId(999), &short, |_| false),
            Err(RelinkError::AssetNotFound(AssetId(999)))
        );
        assert_eq!(p, before, "失败时工程不变");
    }

    #[test]
    fn reversed_clips_count_their_original_asset() {
        let (mut p, ids) = sample_project();
        let reversed_asset = p.add_asset(probed("倒放.mp4", 3, true, true));
        let t = p
            .timeline
            .tracks
            .iter_mut()
            .find(|t| t.clips.iter().any(|c| c.id == ids.v2))
            .unwrap();
        let i = t.index_of(ids.v2).unwrap();
        let c = std::sync::Arc::make_mut(&mut t.clips[i]);
        c.content = ClipContent::Media {
            asset: reversed_asset,
            source_in: Time::ZERO,
        };
        c.reversed = Some((ids.video_asset, Time::from_secs(6)));
        assert_eq!(used_until(&p, ids.video_asset), Time::from_secs(9));
        assert_eq!(used_until(&p, reversed_asset), Time::from_secs(3));
    }
}
