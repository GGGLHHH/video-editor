// 每层画一个矩形：位置用 NDC，纹理坐标取裁剪后的子区域；像素是预乘透明，整体乘不透明度。
// 每层的参数放在实例顶点数据里（第 i 层用第 i 个实例）。不用每层一个 uniform 缓冲：
// WARP 软件显卡在那种写法下所有层都会读到最后一层的参数。

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct Instance {
    @location(0) rect: vec4<f32>,    // NDC：x0, y0（上）, x1, y1（下）
    @location(1) uv: vec4<f32>,      // 纹理坐标：u0, v0, u1, v1
    @location(2) opacity: f32,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) opacity: f32,
};

@vertex
fn vs(@builtin(vertex_index) i: u32, layer: Instance) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[i];
    var o: VOut;
    o.pos = vec4(mix(layer.rect.x, layer.rect.z, c.x), mix(layer.rect.y, layer.rect.w, c.y), 0.0, 1.0);
    o.uv = vec2(mix(layer.uv.x, layer.uv.z, c.x), mix(layer.uv.y, layer.uv.w, c.y));
    o.opacity = layer.opacity;
    return o;
}

@fragment
fn fs(v: VOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, v.uv) * v.opacity;
}
