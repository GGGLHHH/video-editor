//! 测性能用的标准多层工程：同一个素材铺 N 层（主画面 + 画中画 + 台标 …）。
//!
//! 播放测速、拖动延迟、导出测速都用同一种铺法，数字才能互相比较。

use crate::edit::{
    EditError, add_track, insert_asset_at, move_clip, set_transform, trim_end, trim_start,
};
use crate::{AssetId, Crop, Project, Time, TrackKind, Transform};

/// 第 i 层（i ≥ 1）的位置和大小：右下大画中画、左上小台标，其余错开摆放。
const LAYOUT: [(f32, f32, f32); 3] = [(0.3, 0.3, 0.33), (-0.4, -0.38, 0.15), (-0.3, 0.3, 0.25)];

/// 第 i 层的画面属性（第 0 层铺满）。
#[must_use]
pub fn layer_transform(i: usize) -> Transform {
    if i == 0 {
        return Transform::default();
    }
    let (x, y, scale) = LAYOUT[(i - 1) % LAYOUT.len()];
    Transform {
        x,
        y,
        scale,
        crop: Crop::default(),
        opacity: 1.0,
        rotation: 0.0,
    }
}

/// 用素材 `asset` 铺 `layers` 层，每层长 `length`（素材不够长时截短）；工程尺寸和帧率跟随素材。
/// 第 i 层从素材第 i 秒开始取，各层同一时刻是不同的画面（最坏情况：每层都要单独解码）。
///
/// # Errors
/// 素材不存在或不是视频、`layers` 为 0、长度超出素材。
pub fn stack_layers(
    project: &mut Project,
    asset: AssetId,
    layers: usize,
    length: Time,
) -> Result<(), EditError> {
    let info = project
        .assets
        .get(&asset)
        .ok_or(EditError::AssetNotFound(asset))?
        .video
        .ok_or(EditError::WrongTrackKind)?;
    if layers == 0 {
        return Err(EditError::InvalidValue);
    }
    project.settings.width = info.width;
    project.settings.height = info.height;
    project.settings.frame_rate = info.frame_rate;
    for i in 0..layers {
        let track = if i == 0 {
            project
                .timeline
                .track_of_kind(TrackKind::Video)
                .ok_or(EditError::WrongTrackKind)?
                .id
        } else {
            add_track(project, TrackKind::Video)?
        };
        let clip = insert_asset_at(project, asset, track, Time::ZERO)?;
        let offset = Time::from_secs(i64::try_from(i).unwrap_or(i64::MAX));
        if offset > Time::ZERO {
            trim_start(project, clip, offset)?;
            move_clip(project, clip, track, Time::ZERO)?;
        }
        let available = project
            .timeline
            .find_clip(clip)
            .map_or(length, |(_, c)| c.duration);
        trim_end(project, clip, length.min(available))?;
        if i > 0 {
            set_transform(project, clip, layer_transform(i))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::sample_project;

    #[test]
    fn stacks_layers_bottom_up() {
        let (mut p, ids) = sample_project();
        crate::edit::delete_clips(&mut p, &[ids.v1, ids.v2]).unwrap();
        stack_layers(&mut p, ids.video_asset, 3, Time::from_secs(4)).unwrap();
        let tracks: Vec<_> = p.timeline.video_tracks_bottom_up().collect();
        assert_eq!(tracks.len(), 3);
        for (i, t) in tracks.iter().enumerate() {
            assert_eq!(t.clips.len(), 1);
            assert_eq!(t.clips[0].start, Time::ZERO);
            assert_eq!(t.clips[0].duration, Time::from_secs(4));
            assert_eq!(t.clips[0].transform, layer_transform(i));
            // 第 i 层从素材第 i 秒开始
            assert_eq!(
                t.clips[0].source_in(),
                Time::from_secs(i64::try_from(i).unwrap())
            );
        }
        assert_eq!((p.settings.width, p.settings.height), (1920, 1080));
        p.timeline.check_invariants(&p.assets).unwrap();
        assert_eq!(
            stack_layers(&mut p, ids.audio_asset, 1, Time::from_secs(1)),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(
            stack_layers(&mut p, ids.video_asset, 0, Time::from_secs(1)),
            Err(EditError::InvalidValue)
        );
    }
}
