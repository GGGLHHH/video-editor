// 每层画一个四边形：四个角的位置（NDC，已按旋转算好），纹理坐标取裁剪后的子区域；
// 像素是预乘透明，整体乘不透明度。
// 每层的参数放在实例顶点数据里（第 i 层用第 i 个实例）。不用每层一个 uniform 缓冲：
// WARP 软件显卡在那种写法下所有层都会读到最后一层的参数。
//
// 外观（ve_core::Look）都在图层自己的坐标系里算（local：显示部分左上角为原点、像素、y 向下）：
// 圆角、描边（外沿一圈）、阴影（偏移 + 模糊）、蒙版、色度抠图。四边形按描边 / 阴影往外扩 expand 像素。
// 混合模式不是「正常」的图层用 fs_blend：读已经合成好的底图（backdrop），按 W3C 公式混合后直接写回。

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
// 文字蒙版的距离场（crate::text_mask）：128 = 正好在字的边上，越大越在字里面，每 TEXT_SPREAD
// 个纹素差 127；没有文字蒙版的图层绑 1×1 占位
@group(0) @binding(2) var text_field: texture_2d<f32>;
const TEXT_SPREAD: f32 = 12.0;

// 文字形状的有向距离（外面为正，像素）：字按蒙版框（半宽高 hb）等比放进去、居中
fn text_sd(p: vec2<f32>, hb: vec2<f32>) -> f32 {
    let dims = vec2<f32>(textureDimensions(text_field));
    let k = min(2.0 * hb.x / dims.x, 2.0 * hb.y / dims.y);  // 一个纹素多少像素
    let uv = p / (k * dims) + vec2(0.5);
    let s = textureSampleLevel(text_field, samp, clamp(uv, vec2(0.0), vec2(1.0)), 0.0).r;
    var d = -((s * 255.0 - 128.0) / 127.0) * TEXT_SPREAD * k;
    // 字的框外面：至少按离框的距离算（框边上本来就是留白）
    let q = abs(p) - 0.5 * k * dims;
    if q.x > 0.0 || q.y > 0.0 {
        d = max(d, length(max(q, vec2(0.0))) + TEXT_SPREAD * k);
    }
    return d;
}
@group(1) @binding(0) var backdrop: texture_2d<f32>;
@group(2) @binding(0) var lut: texture_3d<f32>;
@group(2) @binding(1) var lut_samp: sampler;
// 另外几个蒙版（实例属性的 16 个槽已经用满）：每个实例一条，v[0].x 是个数，
// 之后每个蒙版三个 vec4：（中心、半宽高）、（旋转、羽化、圆角、反转）、（形状）
struct ExtraMasks {
    v: array<vec4<f32>, 10>,
};
@group(3) @binding(0) var<storage, read> extras: array<ExtraMasks>;

struct Instance {
    @location(0) top: vec4<f32>,       // NDC：左上 xy、右上 xy
    @location(1) bottom: vec4<f32>,    // NDC：左下 xy、右下 xy
    @location(2) uv: vec4<f32>,        // 纹理坐标：u0, v0, u1, v1
    @location(3) p0: vec4<f32>,        // 不透明度、要不要调色、混合模式、圆角半径（像素）
    @location(4) red: vec4<f32>,       // 调色矩阵的三行：输出 = 行 · (r, g, b, 1)
    @location(5) green: vec4<f32>,
    @location(6) blue: vec4<f32>,
    @location(7) sizes: vec4<f32>,     // 显示部分宽、高（像素），往外扩的像素，描边粗细（像素，0 = 没有）
    @location(8) stroke: vec4<f32>,    // 描边颜色 rgb
    @location(9) shadow: vec4<f32>,    // 阴影颜色 rgb、不透明度（0 = 没有）
    @location(10) shadow2: vec4<f32>,  // 阴影偏移 xy（图层坐标、像素）、模糊（像素）、蒙版形状（-1 = 没有）
    @location(11) mask: vec4<f32>,     // 蒙版中心 xy（图层坐标、像素）、半宽、半高
    @location(12) mask2: vec4<f32>,    // 蒙版旋转（弧度）、羽化（像素）、圆角（像素）、反转
    @location(13) key: vec4<f32>,      // 色度抠图颜色 rgb、低阈值
    @location(14) key2: vec4<f32>,     // 高阈值、开没开、用不用查找表（进阶调色）
    @location(15) detail: vec4<f32>,   // 锐化（0–1）、颗粒（0–1）、暗角（-1–1）、颗粒种子
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) uv: vec4<f32>,
    @location(2) @interpolate(flat) p0: vec4<f32>,
    @location(3) @interpolate(flat) red: vec4<f32>,
    @location(4) @interpolate(flat) green: vec4<f32>,
    @location(5) @interpolate(flat) blue: vec4<f32>,
    @location(6) @interpolate(flat) sizes: vec4<f32>,
    @location(7) @interpolate(flat) stroke: vec4<f32>,
    @location(8) @interpolate(flat) shadow: vec4<f32>,
    @location(9) @interpolate(flat) shadow2: vec4<f32>,
    @location(10) @interpolate(flat) mask: vec4<f32>,
    @location(11) @interpolate(flat) mask2: vec4<f32>,
    @location(12) @interpolate(flat) key: vec4<f32>,
    @location(13) @interpolate(flat) key2: vec4<f32>,
    @location(14) @interpolate(flat) detail: vec4<f32>,
    @location(15) @interpolate(flat) inst: u32,
};

@vertex
fn vs(@builtin(vertex_index) i: u32, @builtin(instance_index) inst: u32, layer: Instance) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let size = max(layer.sizes.xy, vec2(1.0));
    // 往外扩：参数从 0–1 扩到 -e–1+e（四个角是仿射的平行四边形，外推仍落在同一平面上）
    let e = layer.sizes.z / size;
    let c = corners[i] * (1.0 + 2.0 * e) - e;
    var o: VOut;
    let upper = mix(layer.top.xy, layer.top.zw, c.x);
    let lower = mix(layer.bottom.xy, layer.bottom.zw, c.x);
    o.pos = vec4(mix(upper, lower, c.y), 0.0, 1.0);
    o.local = c * size;
    o.uv = layer.uv;
    o.p0 = layer.p0;
    o.red = layer.red;
    o.green = layer.green;
    o.blue = layer.blue;
    o.sizes = layer.sizes;
    o.stroke = layer.stroke;
    o.shadow = layer.shadow;
    o.shadow2 = layer.shadow2;
    o.mask = layer.mask;
    o.mask2 = layer.mask2;
    o.key = layer.key;
    o.key2 = layer.key2;
    o.detail = layer.detail;
    o.inst = inst;
    return o;
}

// 调色（sRGB 0–1、不预乘）：颜色矩阵，和 ve_core::ColorMatrix::apply 一样（片段自己的调色和
// 调节片段都已经乘进这个矩阵里）
fn adjust_color(c: vec3<f32>, v: VOut) -> vec3<f32> {
    let x = vec4(c, 1.0);
    return clamp(vec3(dot(v.red, x), dot(v.green, x), dot(v.blue, x)), vec3(0.0), vec3(1.0));
}

// 蒙版形状的有向距离（像素，外面为正；反转时里外对调）；没有蒙版时是很大的负数（处处都在里面）
fn mask_sd(local: vec2<f32>, v: VOut) -> f32 {
    return shape_sd(local, v.shadow2.w, v.mask, v.mask2);
}

// 所有蒙版合起来（并集）的有向距离：取最小的（描边、阴影贴着它）
fn masks_sd(local: vec2<f32>, v: VOut) -> f32 {
    var d = mask_sd(local, v);
    let e = extras[v.inst];
    let n = i32(e.v[0].x + 0.5);
    for (var k = 0; k < n; k = k + 1) {
        d = min(d, shape_sd(local, e.v[3 + k * 3].x, e.v[1 + k * 3], e.v[2 + k * 3]));
    }
    return d;
}

// 所有蒙版合起来留下的比例：每个按自己的羽化算，取最大的
fn masks_alpha(local: vec2<f32>, v: VOut) -> f32 {
    var a = mask_alpha(mask_sd(local, v), v.mask2.y);
    let e = extras[v.inst];
    let n = i32(e.v[0].x + 0.5);
    for (var k = 0; k < n; k = k + 1) {
        let m2 = e.v[2 + k * 3];
        a = max(a, mask_alpha(shape_sd(local, e.v[3 + k * 3].x, e.v[1 + k * 3], m2), m2.y));
    }
    return a;
}

// 这一层在这个像素上的颜色（预乘透明，已乘不透明度）
fn layer_color(v: VOut) -> vec4<f32> {
    let size = max(v.sizes.xy, vec2(1.0));
    let half_size = size * 0.5;
    let radius = v.p0.w;
    let d_frame = sd_round_box(v.local - half_size, half_size, radius);
    let d_mask = masks_sd(v.local, v);
    // 画面的形状 = 框 ∩ 蒙版：描边、阴影都贴着它
    let d = max(d_frame, d_mask);
    // 画面本身：只在框里面取样（框外的纹理坐标落在裁剪区外面）
    let t = clamp(v.local / size, vec2(0.0), vec2(1.0));
    let uv = vec2(mix(v.uv.x, v.uv.z, t.x), mix(v.uv.y, v.uv.w, t.y));
    var s = textureSampleLevel(tex, samp, uv, 0.0);
    // 锐化：减去上下左右四个邻点的平均（反锐化掩模），在预乘的值上做
    if v.detail.x > 0.0 {
        let texel = 1.0 / vec2<f32>(textureDimensions(tex));
        let around = textureSampleLevel(tex, samp, uv + vec2(0.0, -texel.y), 0.0)
            + textureSampleLevel(tex, samp, uv + vec2(0.0, texel.y), 0.0)
            + textureSampleLevel(tex, samp, uv + vec2(-texel.x, 0.0), 0.0)
            + textureSampleLevel(tex, samp, uv + vec2(texel.x, 0.0), 0.0);
        let sharp = s.rgb + (s.rgb - around.rgb * 0.25) * (v.detail.x * 2.0);
        s = vec4(clamp(sharp, vec3(0.0), vec3(s.a)), s.a);
    }
    // 框内（圆角）和蒙版留下的比例
    let cover = masks_alpha(v.local, v) * clamp(0.5 - d_frame, 0.0, 1.0);
    // 没调色、没抠像的图层直接用预乘的取样值（反预乘再预乘会让半透明边缘的颜色有误差）
    var c = s * cover;
    if (v.p0.y > 0.5 || v.key2.y > 0.5 || v.key2.z > 0.5) && s.a > 0.0 {
        var rgb = s.rgb / s.a;
        if v.key2.z > 0.5 {
            // 进阶调色：整条链烘焙好的 3D 查找表（坐标对准格子中心）
            let n = f32(textureDimensions(lut).x);
            let coord = clamp(rgb, vec3(0.0), vec3(1.0)) * ((n - 1.0) / n) + vec3(0.5 / n);
            rgb = textureSampleLevel(lut, lut_samp, coord, 0.0).rgb;
        } else if v.p0.y > 0.5 {
            rgb = adjust_color(rgb, v);
        }
        var a = s.a * cover;
        if v.key2.y > 0.5 {
            // 色度抠图按原片的颜色算（调色之前）
            let dist = length(chroma(s.rgb / s.a) - chroma(v.key.rgb));
            a = a * clamp((dist - v.key.w) / max(v.key2.x - v.key.w, 1e-4), 0.0, 1.0);
        }
        c = vec4(rgb * a, a);
    }
    // 暗角：离中心越远越暗（负数是变亮的白边），按画面自己的范围算
    if v.detail.z != 0.0 {
        let dv = length(t - vec2(0.5)) * 1.41421356;
        let w = smoothstep(0.25, 1.0, dv) * v.detail.z;
        if w > 0.0 {
            c = vec4(c.rgb * (1.0 - 0.85 * w), c.a);
        } else {
            c = vec4(mix(c.rgb, vec3(c.a), -0.85 * w), c.a);
        }
    }
    // 颗粒：每个像素一个随机明暗，种子随素材时间变
    if v.detail.y > 0.0 {
        let p = floor(v.local) + vec2(v.detail.w * 13.0, v.detail.w * 7.0);
        let n = fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453) - 0.5;
        c = vec4(clamp(c.rgb + vec3(n * v.detail.y * 0.3 * c.a), vec3(0.0), vec3(c.a)), c.a);
    }
    // 描边：框外一圈
    let w = v.sizes.w;
    if w > 0.0 {
        let ring = clamp(w + 0.5 - d, 0.0, 1.0) * clamp(d + 0.5, 0.0, 1.0);
        c = premul_over(c, vec4(v.stroke.rgb * ring, ring));
    }
    // 阴影：偏移过的框，按模糊半径渐隐
    if v.shadow.a > 0.0 {
        let q = v.local - v.shadow2.xy;
        let ds = max(sd_round_box(q - half_size, half_size, radius), masks_sd(q, v));
        let b = max(v.shadow2.z, 0.5);
        let sa = (1.0 - smoothstep(-b, b, ds)) * v.shadow.a;
        c = premul_over(c, vec4(v.shadow.rgb * sa, sa));
    }
    return c * v.p0.x;
}

@fragment
fn fs(v: VOut) -> @location(0) vec4<f32> {
    return layer_color(v);
}

@fragment
fn fs_blend(v: VOut) -> @location(0) vec4<f32> {
    let c = layer_color(v);
    let b = textureLoad(backdrop, vec2<i32>(v.pos.xy), 0);
    return blend_over(b, c, v.p0.z);
}

// ---- 画面特效（ve_core::EffectKind）：读底图（已经合成好的画面），整个画面重画 ----
// p0 = （种类编号、强度 0–1、特效片段里过了几秒、0）

// 底图（预乘透明；最外层处处不透明，复合片段的组里可能是透明的）
fn bd(p: vec2<i32>) -> vec3<f32> {
    let d = vec2<i32>(textureDimensions(backdrop));
    return textureLoad(backdrop, clamp(p, vec2(0), d - vec2(1)), 0).rgb;
}

fn bd_alpha(p: vec2<i32>) -> f32 {
    let d = vec2<i32>(textureDimensions(backdrop));
    return textureLoad(backdrop, clamp(p, vec2(0), d - vec2(1)), 0).a;
}

// 黄金角螺旋上 32 个点的平均（半径 r 像素）；bright 为真时只取亮部（发光用）
fn spiral_avg(p: vec2<f32>, r: f32, bright: bool) -> vec3<f32> {
    var sum = vec3(0.0);
    for (var i = 0; i < 32; i = i + 1) {
        let f = f32(i);
        let ang = f * 2.39996323;
        let rad = r * sqrt((f + 0.5) / 32.0);
        let q = p + vec2(cos(ang), sin(ang)) * rad;
        var c = bd(vec2<i32>(round(q)));
        if bright {
            c = max(c - vec3(0.6), vec3(0.0)) / 0.4;
        }
        sum = sum + c;
    }
    return sum / 32.0;
}

fn hash21(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2(123.34, 456.21));
    let r = q + dot(q, q + 45.32);
    return fract(r.x * r.y);
}

@fragment
fn fs_effect(v: VOut) -> @location(0) vec4<f32> {
    let kind = v.p0.x;
    let amount = clamp(v.p0.y, 0.0, 1.0);
    let t = v.p0.z;
    let dims = vec2<f32>(textureDimensions(backdrop));
    let short = min(dims.x, dims.y);
    let p = floor(v.pos.xy);
    let pi = vec2<i32>(p);
    let c = bd(pi);
    var out = c;
    if kind < 0.5 {
        // 模糊：半径最大为短边的 3%
        out = spiral_avg(p, max(amount * short * 0.03, 0.5), false);
    } else if kind < 1.5 {
        // 锐化：减去周围一圈的平均
        let s = max(i32(round(short / 1080.0)), 1);
        let avg = (bd(pi + vec2(s, 0)) + bd(pi - vec2(s, 0)) + bd(pi + vec2(0, s)) + bd(pi - vec2(0, s))) / 4.0;
        out = c + (c - avg) * amount * 2.0;
    } else if kind < 2.5 {
        // 暗角：中心不动，越往角落越暗（强度 1 时角落全黑）
        let r = length(p / dims - vec2(0.5)) * 1.41421356;
        out = c * (1.0 - amount * smoothstep(0.35, 1.0, r));
    } else if kind < 3.5 {
        // 颗粒：每秒 30 次换一组噪点
        let n = hash21(p + floor(t * 30.0) * vec2(17.31, 91.73)) - 0.5;
        out = c + vec3(n * amount * 0.35);
    } else if kind < 4.5 {
        // 马赛克：方块边长最大为短边的 1/24，取方块中心的颜色（不到 2 像素就是原样）
        let b = round(amount * short / 24.0);
        if b >= 2.0 {
            out = bd(vec2<i32>((floor(p / b) + 0.5) * b));
        }
    } else if kind < 5.5 {
        // RGB 色差：红往右、蓝往左
        // 最少错开 1 像素（小画面上也看得出）
        let o = max(i32(round(amount * short * 0.012)), select(0, 1, amount > 0.0));
        out = vec3(bd(pi - vec2(o, 0)).r, c.g, bd(pi + vec2(o, 0)).b);
    } else if kind < 6.5 {
        // 闪白：每秒开头闪一下，很快退掉
        let w = amount * exp(-fract(t) * 6.0);
        out = mix(c, vec3(1.0), w);
    } else {
        // 发光：亮部模糊后加回去
        out = c + spiral_avg(p, short * 0.02, true) * amount * 1.5;
    }
    // 透明度照底图（预乘：颜色不超过透明度）
    let a = bd_alpha(pi);
    return vec4(clamp(out, vec3(0.0), vec3(a)), a);
}
