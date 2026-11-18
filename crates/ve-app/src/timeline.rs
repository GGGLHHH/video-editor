//! 时间轴：几何换算、拖拽交互（纯逻辑）与视图。

mod context_menu;
pub mod geometry;
pub mod interaction;
mod toolbar;
pub mod tracks;
pub mod view;

pub use geometry::{Hit, TimelineGeometry, Zone};
pub use interaction::{Drag, DragOutcome};
