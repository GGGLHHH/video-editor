// 每层画一个四边形：四个角的位置（NDC，已按旋转算好），纹理坐标取裁剪后的子区域；
// 像素是预乘透明，整体乘不透明度。
// 每层的参数放在实例顶点数据里（第 i 层用第 i 个实例）。不用每层一个 uniform 缓冲：
// WARP 软件显卡在那种写法下所有层都会读到最后一层的参数。

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct Instance {
    @location(0) top: vec4<f32>,     // NDC：左上 xy、右上 xy
    @location(1) bottom: vec4<f32>,  // NDC：左下 xy、右下 xy
    @location(2) uv: vec4<f32>,      // 纹理坐标：u0, v0, u1, v1
    @location(3) opacity: f32,
    @location(4) red: vec4<f32>,     // 调色矩阵的三行：输出 = 行 · (r, g, b, 1)
    @location(5) green: vec4<f32>,
    @location(6) blue: vec4<f32>,
    @location(7) colored: f32,       // 1 = 调色，0 = 原样
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) opacity: f32,
    @location(2) @interpolate(flat) red: vec4<f32>,
    @location(3) @interpolate(flat) green: vec4<f32>,
    @location(4) @interpolate(flat) blue: vec4<f32>,
    @location(5) @interpolate(flat) colored: f32,
};

@vertex
fn vs(@builtin(vertex_index) i: u32, layer: Instance) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[i];
    var o: VOut;
    // 先在上边、下边各按 c.x 插值，再按 c.y 在两者之间插值（四个角正好落在顶点上）
    let upper = mix(layer.top.xy, layer.top.zw, c.x);
    let lower = mix(layer.bottom.xy, layer.bottom.zw, c.x);
    o.pos = vec4(mix(upper, lower, c.y), 0.0, 1.0);
    o.uv = vec2(mix(layer.uv.x, layer.uv.z, c.x), mix(layer.uv.y, layer.uv.w, c.y));
    o.opacity = layer.opacity;
    o.red = layer.red;
    o.green = layer.green;
    o.blue = layer.blue;
    o.colored = layer.colored;
    return o;
}

// 调色（sRGB 0–1、不预乘）：颜色矩阵，和 ve_core::ColorMatrix::apply 一样（片段自己的调色和
// 调节片段都已经乘进这个矩阵里）
fn adjust_color(c: vec3<f32>, v: VOut) -> vec3<f32> {
    let x = vec4(c, 1.0);
    return clamp(vec3(dot(v.red, x), dot(v.green, x), dot(v.blue, x)), vec3(0.0), vec3(1.0));
}

@fragment
fn fs(v: VOut) -> @location(0) vec4<f32> {
    let s = textureSample(tex, samp, v.uv);
    var c = s;
    // 没调色的图层不动（反预乘再预乘会让半透明边缘的颜色有误差）
    if v.colored > 0.5 && s.a > 0.0 {
        c = vec4(adjust_color(s.rgb / s.a, v) * s.a, s.a);
    }
    return c * v.opacity;
}
