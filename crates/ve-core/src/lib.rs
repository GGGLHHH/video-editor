//! 纯模型：时间、工程、时间轴、编辑操作、撤销重做、SRT、工程文件。
//!
//! 除工程文件读写外不做 IO，不依赖 UI 或 `FFmpeg`。

pub mod adjust;
pub mod asset;
pub mod audio_fx;
pub mod bench;
pub mod color_space;
pub mod compose;
pub mod compound;
pub mod edit;
pub mod effect;
pub mod grade;
pub mod history;
pub mod ids;
pub mod interchange;
pub mod keyframe;
pub mod look;
pub mod magnet;
pub mod motion;
pub mod project;
pub mod project_file;
pub mod relink;
pub mod snap;
pub mod speed_curve;
pub mod srt;
pub mod stabilize;
pub mod subtitle_style;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod text_motion;
pub mod time;
pub mod timeline;
pub mod transform;
pub mod transition;
pub mod trim_tools;

pub use adjust::{
    AdjustExtra, AdjustItem, ColorAdjust, ColorMatrix, Detail, FILTERS, Filter, Tone,
};
pub use asset::{Asset, AudioInfo, VideoInfo};
pub use audio_fx::{AudioFx, Voice};
pub use edit::EditError;
pub use effect::{Effect, EffectKind, EffectPass};
pub use grade::{
    Curves, Grade, GradeChain, GradeStep, HslAdjust, HslBand, HslChannel, LutRef, Wheels,
};
pub use history::History;
pub use ids::{AssetId, ClipId, IdGen, TrackId};
pub use keyframe::{Animation, Easing, Key, KeyGroup, Prop};
pub use look::{BlendMode, ChromaKey, Look, Mask, MaskShape, Shadow, Stroke};
pub use project::{Project, ProjectSettings};
pub use project_file::{LoadedProject, ProjectFileError, load_project, save_project};
pub use speed_curve::{CurvePreset, SpeedCurve, SpeedPoint};
pub use subtitle_style::SubtitleStyle;
pub use text_motion::{
    TextIn, TextLoop, TextMotion, TextOut, TextPose, TextState, text_pose, text_state,
};
pub use time::{FLICKS_PER_SECOND, FrameRate, Time, format_timecode};
pub use timeline::{Clip, ClipContent, Cover, InvariantError, Timeline, Track, TrackKind};
pub use transform::{
    Crop, LayerGeometry, Rect, Transform, centered_crop, layer_geometry, wrap_degrees,
};
pub use transition::{Transition, TransitionKind};
