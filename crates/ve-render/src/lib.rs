//! 显卡合成：多层画面按位置 / 缩放 / 裁剪 / 不透明度叠加。
//!
//! 预览和导出共用同一个合成器，保证两边画面一致。图层从下往上排，像素是预乘透明的 BGRA；
//! 输出给预览的是 BGRA，给导出的是 BT.709 有限范围的 NV12。
//!
//! 一个 [`Compositor`] 只在一个线程里用，各自持有自己的显卡设备。

mod compositor;
mod error;

pub use compositor::{Compositor, FramePixels, Layer, MappedNv12, Nv12, RenderConfig};
pub use error::RenderError;
