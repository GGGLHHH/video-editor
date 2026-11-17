//! 纯模型：时间、工程、时间轴、编辑操作、撤销重做、SRT、工程文件。
//!
//! 除工程文件读写外不做 IO，不依赖 UI 或 `FFmpeg`。

pub mod asset;
pub mod bench;
pub mod compose;
pub mod edit;
pub mod history;
pub mod ids;
pub mod project;
pub mod project_file;
pub mod snap;
pub mod srt;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod time;
pub mod timeline;
pub mod transform;

pub use asset::{Asset, AudioInfo, VideoInfo};
pub use edit::EditError;
pub use history::History;
pub use ids::{AssetId, ClipId, IdGen, TrackId};
pub use project::{Project, ProjectSettings};
pub use project_file::{LoadedProject, ProjectFileError, load_project, save_project};
pub use time::{FLICKS_PER_SECOND, FrameRate, Time, format_timecode};
pub use timeline::{Clip, ClipContent, InvariantError, Timeline, Track, TrackKind};
pub use transform::{Crop, LayerGeometry, Rect, Transform, layer_geometry};
