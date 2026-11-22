// 复合片段分组（ve_core::compose::GroupPass）：组开始时拍下快照（before）、把画布清成透明，
// 组里的几层（和组里的特效）就只画在这块透明画布上（after）——特效、混合模式只作用在组里。
// 这一道把组叠回去：整组的抠图 → 形状（圆角、蒙版）→ 描边、阴影 → 不透明度 → 混合模式。
// 没有整组外观时只按不透明度叠。和图层着色器同一套公式（shapes.wgsl，编译时拼在前面）。

@group(0) @binding(0) var before: texture_2d<f32>;
@group(1) @binding(0) var after: texture_2d<f32>;

// 复合片段整组的蒙版不支持文字形状（面板上也不给选）：当作不裁
fn text_sd(p: vec2<f32>, hb: vec2<f32>) -> f32 {
    return -1e6;
}

struct Amount {
    // v[0]：整组不透明度、有没有整组外观（1 / 0）、旋转角（弧度，顺时针）、圆角（像素）
    // v[1]：旋转中心 xy、显示部分转之前的左上角 xy（画布像素）
    // v[2]：显示部分宽高（像素）、第一个蒙版的形状编号（-1 = 没有）、混合模式编号
    // v[3]、v[4]：第一个蒙版（中心、半宽高）、（旋转、羽化、圆角、反转）
    // v[5]：另外几个蒙版的个数；v[6..14] 每个三个 vec4，同 shader.wgsl 的 ExtraMasks
    // v[15]：描边颜色 rgb、粗细（像素，0 = 没有）
    // v[16]：阴影颜色 rgb、不透明度（0 = 没有）
    // v[17]：阴影偏移 xy（显示部分的坐标）、模糊（像素）、0
    // v[18]：抠图颜色 rgb、低阈值
    // v[19]：高阈值、开没开、0、0
    v: array<vec4<f32>, 20>,
}
@group(2) @binding(0) var<uniform> amount: Amount;

// 一个盖住整个画面的三角形
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4(x, y, 0.0, 1.0);
}

// 画布上的点换成整组显示部分自己的坐标（转回转之前，和 ve_core::LayerGeometry::to_local 一样）
fn group_local(p: vec2<f32>) -> vec2<f32> {
    let center = amount.v[1].xy;
    let ang = -amount.v[0].z;
    let d = p - center;
    let back = vec2(d.x * cos(ang) - d.y * sin(ang), d.x * sin(ang) + d.y * cos(ang)) + center;
    return back - amount.v[1].zw;
}

// 所有蒙版合起来（并集）的有向距离，取最小的。另外几个蒙版（最多 3 个）展开写、下标都是常量——
// 在循环里按变量下标读 uniform 数组，测试用的软件显卡上会让设备丢失
fn group_masks_sd(local: vec2<f32>) -> f32 {
    var d = shape_sd(local, amount.v[2].z, amount.v[3], amount.v[4]);
    let n = amount.v[5].x;
    if n > 0.5 {
        d = min(d, shape_sd(local, amount.v[8].x, amount.v[6], amount.v[7]));
    }
    if n > 1.5 {
        d = min(d, shape_sd(local, amount.v[11].x, amount.v[9], amount.v[10]));
    }
    if n > 2.5 {
        d = min(d, shape_sd(local, amount.v[14].x, amount.v[12], amount.v[13]));
    }
    return d;
}

// 所有蒙版留下的比例：每个按自己的羽化算，取最大的
fn group_masks_alpha(local: vec2<f32>) -> f32 {
    var a = mask_alpha(shape_sd(local, amount.v[2].z, amount.v[3], amount.v[4]), amount.v[4].y);
    let n = amount.v[5].x;
    if n > 0.5 {
        a = max(a, mask_alpha(shape_sd(local, amount.v[8].x, amount.v[6], amount.v[7]), amount.v[7].y));
    }
    if n > 1.5 {
        a = max(a, mask_alpha(shape_sd(local, amount.v[11].x, amount.v[9], amount.v[10]), amount.v[10].y));
    }
    if n > 2.5 {
        a = max(a, mask_alpha(shape_sd(local, amount.v[14].x, amount.v[12], amount.v[13]), amount.v[13].y));
    }
    return a;
}

// 整组（已经画好的 raw，预乘透明）按整组外观处理后的颜色，还没乘不透明度
fn group_color(raw: vec4<f32>, pos: vec2<f32>) -> vec4<f32> {
    let local = group_local(pos);
    let half_size = amount.v[2].xy * 0.5;
    let radius = amount.v[0].w;
    let d_frame = sd_round_box(local - half_size, half_size, radius);
    let d = max(d_frame, group_masks_sd(local));
    var c = raw;
    // 抠图：按组里画好的颜色算
    if amount.v[19].y > 0.5 && c.a > 0.0 {
        let dist = length(chroma(c.rgb / c.a) - chroma(amount.v[18].rgb));
        let low = amount.v[18].w;
        c = c * clamp((dist - low) / max(amount.v[19].x - low, 1e-4), 0.0, 1.0);
    }
    c = c * (group_masks_alpha(local) * clamp(0.5 - d_frame, 0.0, 1.0));
    // 描边：框（和蒙版）外一圈
    let w = amount.v[15].w;
    if w > 0.0 {
        let ring = clamp(w + 0.5 - d, 0.0, 1.0) * clamp(d + 0.5, 0.0, 1.0);
        c = premul_over(c, vec4(amount.v[15].rgb * ring, ring));
    }
    // 阴影：偏移过的框，按模糊半径渐隐
    if amount.v[16].a > 0.0 {
        let q = local - amount.v[17].xy;
        let ds = max(sd_round_box(q - half_size, half_size, radius), group_masks_sd(q));
        let b = max(amount.v[17].z, 0.5);
        let sa = (1.0 - smoothstep(-b, b, ds)) * amount.v[16].a;
        c = premul_over(c, vec4(amount.v[16].rgb * sa, sa));
    }
    return c;
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let b = textureLoad(before, p, 0);
    var c = textureLoad(after, p, 0);
    var mode = 0.0;
    if amount.v[0].y > 0.5 {
        c = group_color(c, pos.xy);
        mode = amount.v[2].w;
    }
    return blend_over(b, c * amount.v[0].x, mode);
}
