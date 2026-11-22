// 复合片段分组（ve_core::compose::GroupPass）：组里的几层照常画完后，和画这组之前拍下的
// 快照混合。混合比例 = 整组不透明度 × 这个像素在整组形状里的程度（复合片段的蒙版、圆角；
// 没有形状时处处是 1）。对普通叠加的图层，这正好等于「整组合成好、裁好再叠上去」。
// 形状函数（sd_round_box、shape_sd、mask_alpha）在 shapes.wgsl，编译时拼在前面。

@group(0) @binding(0) var before: texture_2d<f32>;

// 复合片段整组的蒙版不支持文字形状（面板上也不给选）：当作不裁
fn text_sd(p: vec2<f32>, hb: vec2<f32>) -> f32 {
    return -1e6;
}
@group(1) @binding(0) var after: texture_2d<f32>;

struct Amount {
    // v[0]：整组不透明度、有没有形状（1 / 0）、旋转角（弧度，顺时针）、圆角（像素）
    // v[1]：旋转中心 xy、显示部分转之前的左上角 xy（画布像素）
    // v[2]：显示部分宽高（像素）、第一个蒙版的形状编号（-1 = 没有）、0
    // v[3]、v[4]：第一个蒙版（中心、半宽高）、（旋转、羽化、圆角、反转）
    // v[5]：另外几个蒙版的个数；之后每个三个 vec4，同 shader.wgsl 的 ExtraMasks
    v: array<vec4<f32>, 15>,
}
@group(2) @binding(0) var<uniform> amount: Amount;

// 一个盖住整个画面的三角形
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4(x, y, 0.0, 1.0);
}

// 一个另外加的蒙版留下的比例
fn extra_alpha(local: vec2<f32>, center: vec4<f32>, more: vec4<f32>, shape: f32) -> f32 {
    return mask_alpha(shape_sd(local, shape, center, more), more.y);
}

// 画布上的点在整组形状里的程度（0–1）
fn group_cover(p: vec2<f32>) -> f32 {
    let center = amount.v[1].xy;
    let ang = -amount.v[0].z;
    let d = p - center;
    // 转回转之前（和 ve_core::LayerGeometry::to_local 一样），再换成显示部分自己的坐标
    let back = vec2(d.x * cos(ang) - d.y * sin(ang), d.x * sin(ang) + d.y * cos(ang)) + center;
    let local = back - amount.v[1].zw;
    let half = amount.v[2].xy * 0.5;
    var a = clamp(0.5 - sd_round_box(local - half, half, amount.v[0].w), 0.0, 1.0);
    var m = mask_alpha(shape_sd(local, amount.v[2].z, amount.v[3], amount.v[4]), amount.v[4].y);
    // 另外几个蒙版（最多 3 个）：展开写、下标都是常量——在循环里按变量下标读 uniform 数组，
    // 测试用的软件显卡上会让设备丢失
    let n = amount.v[5].x;
    if n > 0.5 {
        m = max(m, extra_alpha(local, amount.v[6], amount.v[7], amount.v[8].x));
    }
    if n > 1.5 {
        m = max(m, extra_alpha(local, amount.v[9], amount.v[10], amount.v[11].x));
    }
    if n > 2.5 {
        m = max(m, extra_alpha(local, amount.v[12], amount.v[13], amount.v[14].x));
    }
    return a * m;
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    var k = amount.v[0].x;
    if amount.v[0].y > 0.5 {
        k = k * group_cover(pos.xy);
    }
    return mix(textureLoad(before, p, 0), textureLoad(after, p, 0), k);
}
