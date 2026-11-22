//! 显卡合成：多层画面按位置 / 缩放 / 裁剪 / 不透明度叠加。
//!
//! 预览和导出共用同一个合成器，保证两边画面一致。图层从下往上排，像素是预乘透明的 BGRA；
//! 输出给预览的是 BGRA，给导出的是 BT.709 有限范围的 NV12。
//!
//! 一个 [`Compositor`] 只在一个线程里用，各自持有自己的显卡设备。

mod compositor;
mod error;
pub mod plugin;

pub use compositor::{Compositor, FramePixels, Layer, MappedNv12, Nv12, RenderConfig};
pub use error::RenderError;

/// 图层着色器的完整源码：共用的形状函数（`shapes.wgsl`）拼上 `shader.wgsl`。
const LAYER_SHADER: &str = concat!(include_str!("shapes.wgsl"), include_str!("shader.wgsl"));

/// 复合片段分组的着色器：共用的形状函数拼上 `group.wgsl`。
const GROUP_SHADER: &str = concat!(include_str!("shapes.wgsl"), include_str!("group.wgsl"));

#[cfg(test)]
mod tests {
    use wgpu::naga;

    #[test]
    fn the_built_in_shaders_compile() {
        for source in [super::LAYER_SHADER, super::GROUP_SHADER] {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
        }
    }
}
