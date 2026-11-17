//! 画面合成要用的解析：某个时间点上，各视频轨各露出哪个片段的哪一刻。
//!
//! 预览和导出都从这里取图层，保证两边取到的是同一组画面。

use crate::{AssetId, ClipId, Project, Time, TrackId, Transform};

/// 时间点上的一个画面图层。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerSource {
    /// 所在轨道。
    pub track: TrackId,
    /// 素材。
    pub asset: AssetId,
    /// 片段。
    pub clip: ClipId,
    /// 素材内时间。
    pub source_time: Time,
    /// 这一刻的画面属性（有关键帧动画时已经求过值）。
    pub transform: Transform,
    /// 整个片段里最大的缩放（导出按它决定解码尺寸，放大时画面不糊）。
    pub max_scale: f32,
}

/// 时间点 `t` 上所有视频轨的画面，从最下层到最上层（合成顺序）。
#[must_use]
pub fn resolve_layers(project: &Project, t: Time) -> Vec<LayerSource> {
    project
        .timeline
        .video_tracks_bottom_up()
        .filter_map(|tr| {
            let clip = tr.clip_at(t)?;
            Some(LayerSource {
                track: tr.id,
                asset: clip.asset()?,
                clip: clip.id,
                source_time: clip.source_in() + (t - clip.start),
                transform: clip.transform_at(t - clip.start),
                max_scale: clip
                    .animation
                    .max_value(crate::Prop::Scale)
                    .map_or(clip.transform.scale, |m| m.max(clip.transform.scale)),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{add_keyframe, apply_transform_at};
    use crate::testing::sample_project;

    #[test]
    fn layers_carry_the_animated_picture_and_its_largest_scale() {
        let (mut p, ids) = sample_project();
        let at = |scale: f32| Transform {
            scale,
            ..Transform::default()
        };
        // v1（0–5 秒）：片段内 0 秒缩放 1，2 秒缩放 3
        add_keyframe(&mut p, ids.v1, Time::ZERO).unwrap();
        apply_transform_at(&mut p, ids.v1, Time::from_secs(2), at(3.0)).unwrap();
        let layer = resolve_layers(&p, Time::from_secs(1))[0];
        assert_eq!(layer.clip, ids.v1);
        assert!((layer.transform.scale - 2.0).abs() < 1e-6, "{layer:?}");
        assert!((layer.max_scale - 3.0).abs() < 1e-6, "整个片段里最大的缩放");
        // 没有缩放动画：最大缩放就是固定值
        let still = resolve_layers(&p, Time::from_secs(6))[0];
        assert_eq!(still.clip, ids.v2);
        assert_eq!(still.max_scale, still.transform.scale);
    }
}
