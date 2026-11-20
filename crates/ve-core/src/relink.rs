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

/// 时间轴上用到素材 `asset` 的最远位置（素材内时间）；没用到时为零。倒放片段的原素材、
/// 复合片段里面的片段也算。
#[must_use]
pub fn used_until(project: &Project, asset: AssetId) -> Time {
    let mut clips = Vec::new();
    crate::compound::for_each_clip(&project.timeline.tracks, &mut |c| clips.push(c));
    clips
        .into_iter()
        .filter_map(|c| {
            let span = c.source_span();
            let direct = match c.content {
                ClipContent::Media {
                    asset: a,
                    source_in,
                } if a == asset => Some(source_in + span),
                _ => None,
            };
            // 倒放 / 防抖过的：关掉后要用回原素材的这一段
            let original = crate::edit::original_range(project, c)
                .filter(|&(a, _)| a == asset)
                .map(|(_, source_in)| source_in + span);
            direct.max(original)
        })
        .max()
        .unwrap_or(Time::ZERO)
}

/// 文件 `found` 能不能顶替素材 `asset`：原来有的画面、声音它都要有，长度要够时间轴上用到的。
fn check_fits(project: &Project, asset: AssetId, found: &Asset) -> Result<(), RelinkError> {
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
    Ok(())
}

/// 素材 `asset` 换成文件 `found`（路径、时长、流信息用新的，id 和名字不变）。
fn take(project: &mut Project, asset: AssetId, found: &Asset) {
    if let Some(slot) = project.assets.get_mut(&asset) {
        slot.path.clone_from(&found.path);
        slot.duration = found.duration;
        slot.video = found.video;
        slot.audio = found.audio;
    }
}

/// 把素材 `asset` 换成探测好的文件 `found`，再到 `found` 所在目录里按原文件名找回别的丢失素材：
/// 同名文件用 `probe` 探测，和选的文件一样要有原来的画面、声音、长度够才换（重名但内容不对的不换）。
/// 返回所有重新链接上的素材，`asset` 排第一。
///
/// # Errors
/// 素材不存在；新文件少了原来的画面或声音；新文件比时间轴上用到的部分短。
pub fn relink(
    project: &mut Project,
    asset: AssetId,
    found: &Asset,
    exists: impl Fn(&Path) -> bool,
    probe: impl Fn(&Path) -> Option<Asset>,
) -> Result<Vec<AssetId>, RelinkError> {
    check_fits(project, asset, found)?;
    take(project, asset, found);
    let mut done = vec![asset];
    let Some(dir) = found.path.parent().map(Path::to_path_buf) else {
        return Ok(done);
    };
    let others: Vec<AssetId> = project
        .assets
        .values()
        .filter(|a| a.id != asset && !exists(&a.path))
        .map(|a| a.id)
        .collect();
    for other in others {
        let Some(name) = project.assets[&other].path.file_name() else {
            continue;
        };
        let candidate = dir.join(name);
        if !exists(&candidate) {
            continue;
        }
        if let Some(probed) = probe(&candidate)
            && check_fits(project, other, &probed).is_ok()
        {
            take(project, other, &probed);
            done.push(other);
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
        let done = relink(&mut p, ids.video_asset, &found, exists, |path| {
            Some(probed(path.to_str()?, 20, false, true))
        })
        .unwrap();
        assert_eq!(done, vec![ids.video_asset, ids.audio_asset]);
        assert_eq!(p.assets[&ids.audio_asset].path, PathBuf::from("新/bgm.mp3"));
        assert_eq!(p.assets[&ids.video_asset].name, "clip.mp4", "名字不变");
        assert_eq!(missing_assets(&p, exists), Vec::<AssetId>::new());
    }

    #[test]
    fn a_same_name_file_that_does_not_fit_is_left_missing() {
        let (mut p, ids) = sample_project();
        let exists = |path: &Path| path.starts_with("新");
        let found = probed("新/clip.mp4", 10, true, true);
        // 同名的 bgm.mp3 只有 1 秒，时间轴用到 8 秒：不换
        let done = relink(&mut p, ids.video_asset, &found, exists, |path| {
            Some(probed(path.to_str()?, 1, false, true))
        })
        .unwrap();
        assert_eq!(done, vec![ids.video_asset]);
        assert_eq!(p.assets[&ids.audio_asset].path, PathBuf::from("bgm.mp3"));
        // 探测不了的也不换
        let (mut q, fresh) = sample_project();
        let unprobed = relink(&mut q, fresh.video_asset, &found, exists, |_| None).unwrap();
        assert_eq!(unprobed, vec![fresh.video_asset]);
    }

    #[test]
    fn clips_inside_compounds_count_toward_the_use() {
        let (mut p, ids) = sample_project();
        crate::compound::make_compound(&mut p, &[ids.v1, ids.v2]).unwrap();
        assert_eq!(used_until(&p, ids.video_asset), Time::from_secs(8));
        let short = probed("x.mp4", 7, true, true);
        assert_eq!(
            relink(&mut p, ids.video_asset, &short, |_| false, |_| None),
            Err(RelinkError::TooShort)
        );
    }

    #[test]
    fn relinking_takes_the_new_file_info_and_skips_files_still_missing() {
        let (mut p, ids) = sample_project();
        let found = probed("别处/改名了.mp4", 12, true, true);
        let done = relink(&mut p, ids.video_asset, &found, |_| false, |_| None).unwrap();
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
            relink(&mut p, ids.video_asset, &no_video, |_| false, |_| None),
            Err(RelinkError::KindMismatch("画面"))
        );
        let no_audio = probed("x.mp4", 10, true, false);
        assert_eq!(
            relink(&mut p, ids.video_asset, &no_audio, |_| false, |_| None),
            Err(RelinkError::KindMismatch("声音"))
        );
        // 第二段用到素材 5–8 秒
        assert_eq!(used_until(&p, ids.video_asset), Time::from_secs(8));
        let short = probed("x.mp4", 7, true, true);
        assert_eq!(
            relink(&mut p, ids.video_asset, &short, |_| false, |_| None),
            Err(RelinkError::TooShort)
        );
        assert_eq!(
            relink(&mut p, AssetId(999), &short, |_| false, |_| None),
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
