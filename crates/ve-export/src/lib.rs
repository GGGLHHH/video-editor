//! 导出：生成 ASS 字幕和 ffmpeg 滤镜图，调用 ffmpeg 可执行文件编码，汇报进度。

pub mod ass;
pub mod graph;
pub mod render;
pub mod run;

pub use ass::{SubtitleStyle, has_subtitles, write_ass};
pub use graph::{
    AudioCodec, AudioFormat, Codec, Encoder, ExportError, ExportSettings, FfmpegJob, Output,
    VideoAudio, build_job, estimated_bytes, recommended_kbps,
};
pub use render::{frame_count, output_size};
pub use run::{
    PROXY_SHORT_SIDE, default_settings, export, grab_frame, make_proxy, reverse_segment,
    stabilize_segment,
};
pub use ve_render::RenderConfig;
