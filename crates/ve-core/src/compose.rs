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
    /// 画面属性。
    pub transform: Transform,
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
                transform: clip.transform,
            })
        })
        .collect()
}
