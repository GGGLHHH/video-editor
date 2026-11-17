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
pub mod document;
pub mod export_ui;
mod file_actions;
pub mod import;
pub mod inspector;
mod inspector_keyframes;
pub mod inspector_transform;
pub mod media_bin;
mod platform_cursor;
pub mod preview;
mod preview_edit;
pub mod preview_handles;
pub mod project_io;
pub mod recent;
mod recent_ui;
pub mod rotate_cursor;
mod startup;
pub mod theme;
pub mod timeline;
pub mod workspace;

pub use app::run;
