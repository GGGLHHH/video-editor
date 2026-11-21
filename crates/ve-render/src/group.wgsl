// 半透明复合片段（ve_core::compose::GroupPass）：组里的几层照常画完后，和画这组之前拍下的
// 快照按整组不透明度混合。对普通叠加的图层，这正好等于「整组合成好再按不透明度叠上去」。

@group(0) @binding(0) var before: texture_2d<f32>;
@group(1) @binding(0) var after: texture_2d<f32>;

struct Amount {
    // x：整组的不透明度
    v: vec4<f32>,
}
@group(2) @binding(0) var<uniform> amount: Amount;

// 一个盖住整个画面的三角形
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4(x, y, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    return mix(textureLoad(before, p, 0), textureLoad(after, p, 0), amount.v.x);
}
