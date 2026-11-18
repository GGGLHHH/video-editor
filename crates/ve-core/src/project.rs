//! 工程：设置、素材表、时间轴、发号器。

use crate::{Asset, AssetId, FrameRate, IdGen, Timeline};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 工程（序列）设置：输出画面尺寸、帧率、采样率。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSettings {
    /// 画面宽。
    pub width: u32,
    /// 画面高。
    pub height: u32,
    /// 帧率。
    pub frame_rate: FrameRate,
    /// 音频采样率。
    pub sample_rate: u32,
    /// 主轨磁吸（见 [`crate::magnet`]）。新工程默认开（和剪映一样）；旧工程文件里没有这一项，按关读，
    /// 免得打开后第一次编辑就把原来留的空隙全合上。
    #[serde(default)]
    pub main_magnet: bool,
    /// 联动：字幕跟着主轨片段走。新工程默认开，旧文件按关读。
    #[serde(default)]
    pub linkage: bool,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            frame_rate: FrameRate::new(30, 1),
            sample_rate: 48_000,
            main_magnet: true,
            linkage: true,
        }
    }
}

/// 一个工程。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// 设置。
    pub settings: ProjectSettings,
    /// 素材表。
    pub assets: BTreeMap<AssetId, Asset>,
    /// 时间轴。
    pub timeline: Timeline,
    /// 发号器。
    pub ids: IdGen,
    /// 字幕样式（所有字幕共用）。
    #[serde(default)]
    pub subtitle_style: crate::SubtitleStyle,
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

impl Project {
    /// 空工程：默认设置 + MVP 的三条轨道。
    #[must_use]
    pub fn new() -> Self {
        let mut ids = IdGen::new();
        let timeline = Timeline::mvp(&mut ids);
        Self {
            settings: ProjectSettings::default(),
            assets: BTreeMap::new(),
            timeline,
            ids,
            subtitle_style: crate::SubtitleStyle::default(),
        }
    }

    /// 加入素材，分配并返回新 id（忽略传入的 `asset.id`）。
    pub fn add_asset(&mut self, mut asset: Asset) -> AssetId {
        let id = self.ids.asset();
        asset.id = id;
        self.assets.insert(id, asset);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Time;

    #[test]
    fn add_asset_assigns_fresh_id() {
        let mut p = Project::default();
        let a = Asset {
            id: AssetId(0),
            path: "a.mp4".into(),
            name: "a.mp4".into(),
            duration: Time::from_secs(1),
            video: None,
            audio: None,
        };
        let id1 = p.add_asset(a.clone());
        let id2 = p.add_asset(a);
        assert_ne!(id1, id2);
        assert_eq!(p.assets[&id1].id, id1);
        assert_eq!(p.settings.sample_rate, 48_000);
    }
}
