//! FFmpeg 进程内封装：探测、视频 / 音频解码、缩略图、波形。

pub mod audio;
pub mod error;
pub mod probe;
#[cfg(any(test, feature = "test-fixtures"))]
pub mod testing;
pub mod thumbs;
pub mod video;
pub mod waveform;

pub use audio::{AudioDecoder, OUTPUT_CHANNELS, OUTPUT_SAMPLE_RATE};
pub use error::MediaError;
pub use probe::{ProbeResult, STILL_DEFAULT_DURATION, probe};
pub use thumbs::thumbnails;
pub use video::{ScaleTarget, VideoDecoder, VideoFrame};
pub use waveform::waveform_peaks;
