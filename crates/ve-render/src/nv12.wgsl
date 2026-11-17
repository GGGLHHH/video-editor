// 合成结果（RGB）→ BT.709 有限范围 NV12。
// Y 一遍：每个像素一个亮度；UV 一遍：输出半尺寸，每个点取源 2×2 块的平均色度。

@group(0) @binding(0) var src: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // 覆盖整个目标的大三角形
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4(x, y, 0.0, 1.0);
}

fn luma(c: vec3<f32>) -> f32 {
    return (16.0 + 219.0 * dot(c, vec3(0.2126, 0.7152, 0.0722))) / 255.0;
}

fn chroma(c: vec3<f32>) -> vec2<f32> {
    let cb = 128.0 + 224.0 * dot(c, vec3(-0.114572, -0.385428, 0.5));
    let cr = 128.0 + 224.0 * dot(c, vec3(0.5, -0.454153, -0.045847));
    return vec2(cb, cr) / 255.0;
}

@fragment
fn fs_y(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(src, vec2<i32>(pos.xy), 0).rgb;
    return vec4(luma(c), 0.0, 0.0, 1.0);
}

@fragment
fn fs_uv(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let base = vec2<i32>(pos.xy) * 2;
    let c = (textureLoad(src, base, 0).rgb
        + textureLoad(src, base + vec2(1, 0), 0).rgb
        + textureLoad(src, base + vec2(0, 1), 0).rgb
        + textureLoad(src, base + vec2(1, 1), 0).rgb) * 0.25;
    return vec4(chroma(c), 0.0, 1.0);
}
