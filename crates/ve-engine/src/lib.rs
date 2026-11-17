//! 播放引擎：时间轴解析、混音、帧缓存、音视频工作线程、音频输出。

mod audio_worker;
pub mod clock;
pub mod engine;
pub mod frame_cache;
pub mod mix;
pub mod output;
pub mod resolve;
mod video_worker;

pub use clock::PlayClock;
pub use engine::{
    AudioBackend, Engine, EngineConfig, EngineError, EngineEvent, EngineStats, FrameUpdate,
};
pub use frame_cache::FrameCache;
pub use mix::{clamp_mix, gain_at, mix_into};
pub use resolve::{AudioSegment, VideoSource, resolve_audio, resolve_subtitles, resolve_video};
