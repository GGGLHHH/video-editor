//! 素材：导入的媒体文件及其流信息。

use crate::{AssetId, FrameRate, Time};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 视频流信息。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoInfo {
    /// 宽（像素）。
    pub width: u32,
    /// 高（像素）。
    pub height: u32,
    /// 帧率。
    pub frame_rate: FrameRate,
    /// 静态图片：可以无限延长，没有素材长度上限。
    #[serde(default)]
    pub still: bool,
}

/// 音频流信息。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioInfo {
    /// 采样率。
    pub sample_rate: u32,
    /// 声道数。
    pub channels: u16,
}

/// 一个导入的媒体文件。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    /// id；`Project::add_asset` 时分配。
    pub id: AssetId,
    /// 文件路径（绝对路径）。
    pub path: PathBuf,
    /// 显示名（通常是文件名）。
    pub name: String,
    /// 时长；静态图片为默认放入时间轴的长度。
    pub duration: Time,
    /// 视频流（没有则为纯音频）。
    pub video: Option<VideoInfo>,
    /// 音频流。
    pub audio: Option<AudioInfo>,
}

impl Asset {
    /// 是否静态图片。
    #[must_use]
    pub fn is_still(&self) -> bool {
        self.video.is_some_and(|v| v.still)
    }

    /// 素材可用的最大长度：静态图片不限，其余为文件时长。
    #[must_use]
    pub fn source_limit(&self) -> Time {
        if self.is_still() {
            Time(i64::MAX / 4)
        } else {
            self.duration
        }
    }
}
