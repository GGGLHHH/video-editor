//! 时间轴：几何换算、拖拽交互（纯逻辑）与视图。

pub mod geometry;
pub mod interaction;
pub mod view;

pub use geometry::{Hit, TimelineGeometry, Zone};
pub use interaction::{Drag, DragOutcome};
