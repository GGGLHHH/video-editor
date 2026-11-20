//! 视频防抖（剪映「画面 › 基础 › 视频防抖」：裁切最少 / 推荐 / 最稳定）：把片段用到的那一段生成
//! 一个防抖后的新文件（ffmpeg `vidstab`，见 `ve_export::stabilize_segment`），片段换成它；记下原来的
//! 素材，关掉防抖时换回去。和倒放一样，位置、长度、变速、属性都不变。

use crate::edit::{EditError, update_clip};
use crate::{AssetId, ClipContent, ClipId, Project, Time};

/// 三档：名字和平滑窗口（前后各多少帧一起平均，越大越稳、裁得越多）。
pub const LEVELS: [(&str, u32); 3] = [("裁切最少", 6), ("推荐", 12), ("最稳定", 24)];

/// 第 `level` 档的平滑窗口（越界按最后一档）。
#[must_use]
pub fn smoothing(level: u8) -> u32 {
    LEVELS
        .get(usize::from(level))
        .or(LEVELS.last())
        .map_or(12, |(_, s)| *s)
}

/// 片段换成防抖后的素材 `stable`（片段原来用的那一段生成的文件），从它开头取；记下原来的素材和档位。
///
/// # Errors
/// 片段不存在、不是视频素材片段、已经倒放或防抖过、档位越界、素材不存在或不够长。
pub fn stabilize_clip(
    project: &mut Project,
    clip: ClipId,
    stable: AssetId,
    level: u8,
) -> Result<(), EditError> {
    let (_, c) = project
        .timeline
        .find_clip(clip)
        .ok_or(EditError::ClipNotFound(clip))?;
    let ClipContent::Media { asset, source_in } = c.content else {
        return Err(EditError::WrongTrackKind);
    };
    if c.reversed.is_some() || c.stabilized.is_some() || usize::from(level) >= LEVELS.len() {
        return Err(EditError::InvalidValue);
    }
    let span = c.source_span();
    let limit = project
        .assets
        .get(&stable)
        .ok_or(EditError::AssetNotFound(stable))?
        .source_limit();
    // 生成的文件可能比要的短一两帧（编码器取整）：差一帧以内也接受
    let frame = project.settings.frame_rate.frame_duration().max(Time(1));
    if limit + frame < span {
        return Err(EditError::SourceTooShort);
    }
    update_clip(project, clip, |slot| {
        slot.content = ClipContent::Media {
            asset: stable,
            source_in: Time::ZERO,
        };
        slot.stabilized = Some((asset, source_in, level));
        if limit < span {
            slot.duration = slot.duration.min(limit.scaled(1.0 / f64::from(slot.speed)));
            slot.fade_in = slot.fade_in.min(slot.duration);
            slot.fade_out = slot.fade_out.min(slot.duration - slot.fade_in);
        }
        Ok(())
    })
}

/// 关掉防抖：换回原来的素材和素材内起点。
///
/// # Errors
/// 片段不存在、没有防抖过。
pub fn unstabilize_clip(project: &mut Project, clip: ClipId) -> Result<(), EditError> {
    update_clip(project, clip, |c| {
        let (asset, source_in, _) = c.stabilized.take().ok_or(EditError::InvalidValue)?;
        c.content = ClipContent::Media { asset, source_in };
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::sample_project;

    #[test]
    fn stabilizing_swaps_the_asset_and_turning_it_off_swaps_back() {
        let (mut p, ids) = sample_project();
        // v2 用素材 5–8 秒（3 秒）
        let stable = p.add_asset(crate::Asset {
            id: AssetId(0),
            path: "稳.mp4".into(),
            name: "稳".into(),
            duration: Time::from_secs(3),
            video: Some(crate::testing::video_info()),
            audio: Some(crate::testing::audio_info()),
        });
        let before = p.clone();
        stabilize_clip(&mut p, ids.v2, stable, 1).unwrap();
        let (_, c) = p.timeline.find_clip(ids.v2).unwrap();
        assert_eq!(c.asset(), Some(stable));
        assert_eq!(c.source_in(), Time::ZERO);
        assert_eq!(c.stabilized.map(|s| s.2), Some(1));
        assert_eq!(
            stabilize_clip(&mut p, ids.v2, stable, 1),
            Err(EditError::InvalidValue),
            "防抖过的不能再防抖"
        );
        unstabilize_clip(&mut p, ids.v2).unwrap();
        assert_eq!(p, before);
        assert_eq!(
            unstabilize_clip(&mut p, ids.v2),
            Err(EditError::InvalidValue)
        );
        assert_eq!(
            stabilize_clip(&mut p, ids.v2, stable, 3),
            Err(EditError::InvalidValue),
            "档位越界"
        );
        assert_eq!(
            stabilize_clip(&mut p, ids.s1, stable, 0),
            Err(EditError::WrongTrackKind)
        );
    }

    #[test]
    fn a_short_file_is_refused_and_levels_map_to_smoothing() {
        let (mut p, ids) = sample_project();
        let short = p.add_asset(crate::Asset {
            id: AssetId(0),
            path: "短.mp4".into(),
            name: "短".into(),
            duration: Time::from_secs(1),
            video: Some(crate::testing::video_info()),
            audio: None,
        });
        assert_eq!(
            stabilize_clip(&mut p, ids.v2, short, 0),
            Err(EditError::SourceTooShort)
        );
        assert_eq!(smoothing(0), 6);
        assert_eq!(smoothing(2), 24);
        assert_eq!(smoothing(9), 24);
    }
}
