//! 错误类型。

use std::path::PathBuf;
use thiserror::Error;

/// 媒体处理错误。
#[derive(Debug, Error)]
pub enum MediaError {
    /// `FFmpeg` 初始化失败。
    #[error("FFmpeg 初始化失败：{0}")]
    Init(String),
    /// 打不开文件或不是可识别的媒体。
    #[error("打不开 {path}：{source}")]
    Open {
        /// 文件。
        path: PathBuf,
        /// 原因。
        source: ffmpeg_next::Error,
    },
    /// 文件里没有需要的流。
    #[error("{path} 里没有{kind}")]
    NoStream {
        /// 文件。
        path: PathBuf,
        /// 「视频流」或「音频流」。
        kind: &'static str,
    },
    /// 解码过程出错。
    #[error("解码失败：{0}")]
    Decode(#[from] ffmpeg_next::Error),
}
