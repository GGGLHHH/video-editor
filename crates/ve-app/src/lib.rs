//! 视频剪辑器桌面应用（GPUI）。
//!
//! 纯逻辑（[`document`]、[`import`]、[`project_io`]、[`timeline`] 的几何与拖拽）不依赖界面框架，
//! 单独测试；视图层是 [`workspace::Workspace`] 和分在各模块里的面板渲染。

// GPUI 惯例：异步闭包参数 this / cx / window 与外层同名
#![expect(
    clippy::shadow_unrelated,
    reason = "GPUI 闭包惯例重名 this / cx / window"
)]

pub mod actions;
pub mod app;
mod asset_preview;
pub mod autosave;
mod bin_tabs;
mod chrome;
mod clip_label;
mod clip_ops;
mod compound_ui;
mod cover;
pub mod document;
mod effect_bin;
mod effect_preview;
pub mod export_ui;
mod file_actions;
mod freeze;
mod home;
pub mod import;
pub mod inspector;
mod inspector_adjust;
mod inspector_audio_fx;
mod inspector_effect;
mod inspector_grade;
mod inspector_group;
mod inspector_keyframes;
mod inspector_look;
mod inspector_motion;
mod inspector_speed;
mod inspector_subtitles;
mod inspector_text_style;
pub mod inspector_transform;
pub mod interchange_ui;
pub mod keymap;
mod layout;
mod level_meter;
pub mod media_bin;
mod nav_keys;
mod platform_cursor;
mod player_menu;
pub mod plugins;
pub mod preview;
mod preview_edit;
pub mod preview_handles;
mod preview_mask;
pub mod project_io;
pub mod proxy;
mod range_ops;
pub mod recent;
mod recent_ui;
mod record;
pub mod recovery;
mod relink_ui;
mod rename_ui;
mod reverse;
pub mod rotate_cursor;
pub mod scopes;
pub mod settings;
mod shortcuts;
mod stabilize_ui;
mod startup;
pub mod theme;
pub mod timeline;
mod topbar;
mod transition_ui;
mod value_edit;
pub mod workspace;

pub use app::run;
