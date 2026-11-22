//! 插件特效：插件就是一段 WGSL，里面写
//!
//! ```wgsl
//! fn plugin_effect(c: vec3<f32>, p: vec2<f32>, dims: vec2<f32>, amount: f32, t: f32) -> vec3<f32>
//! ```
//!
//! `c` 是这个像素已经合成好的颜色（0–1），`p` 是像素坐标，`dims` 是画面大小，`amount` 是强度（0–1），
//! `t` 是特效片段里过了几秒；要看周围的像素用 `bd(vec2<i32>(...))`。返回新颜色。
//! 合成器把它和内置着色器拼在一起编译成一道特效，预览和导出一样。
//!
//! 插件源码先在这里登记（进程里共用），合成器第一次用到时编译；写错了在登记时就报出来。

use std::collections::BTreeMap;
use std::sync::Mutex;

/// 插件特效的入口：读底图，调插件函数，写回。
const ENTRY: &str = "
@fragment
fn fs_plugin(v: VOut) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(backdrop));
    let p = floor(v.pos.xy);
    let c = bd(vec2<i32>(p));
    let out = plugin_effect(c, p, dims, clamp(v.p0.y, 0.0, 1.0), v.p0.z);
    return vec4(clamp(out, vec3(0.0), vec3(1.0)), 1.0);
}
";

/// 插件特效的入口函数名。
pub(crate) const ENTRY_POINT: &str = "fs_plugin";

static SOURCES: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// 插件拼上内置着色器以后的完整源码。
#[must_use]
pub fn full_source(plugin: &str) -> String {
    format!("{}\n{plugin}\n{ENTRY}", crate::LAYER_SHADER)
}

/// 检查插件能不能编译（语法、类型、有没有 `plugin_effect`）。
///
/// # Errors
/// 编译不过：返回编译器的说明。
pub fn check(plugin: &str) -> Result<(), String> {
    use wgpu::naga;
    let module =
        naga::front::wgsl::parse_str(&full_source(plugin)).map_err(|e| e.emit_to_string(plugin))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| e.emit_to_string(plugin))?;
    Ok(())
}

/// 登记插件 `name`（同名的换成新的）。
///
/// # Errors
/// 编译不过（[`check`]）。
pub fn register(name: &str, source: &str) -> Result<(), String> {
    check(source)?;
    SOURCES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(name.to_owned(), source.to_owned());
    Ok(())
}

/// 登记了哪些插件（按名字排）。
#[must_use]
pub fn registered() -> Vec<String> {
    SOURCES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .keys()
        .cloned()
        .collect()
}

/// 插件 `name` 的源码。
#[must_use]
pub fn source(name: &str) -> Option<String> {
    SOURCES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(name)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn good_plugins_register_and_bad_ones_explain_why() {
        let invert = "fn plugin_effect(c: vec3<f32>, p: vec2<f32>, dims: vec2<f32>, amount: f32, t: f32) -> vec3<f32> { return mix(c, vec3(1.0) - c, amount); }";
        register("测试反色", invert).unwrap();
        assert!(registered().contains(&"测试反色".to_owned()));
        assert_eq!(source("测试反色").as_deref(), Some(invert));
        let syntax = register("坏的", "fn plugin_effect( { 不是 wgsl").unwrap_err();
        assert_ne!(syntax, "");
        let missing = check("fn something_else() {}").unwrap_err();
        assert!(missing.contains("plugin_effect"), "{missing}");
        assert!(source("坏的").is_none());
    }
}
