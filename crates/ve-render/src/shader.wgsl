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
@group(1) @binding(0) var backdrop: texture_2d<f32>;
@group(2) @binding(0) var lut: texture_3d<f32>;
@group(2) @binding(1) var lut_samp: sampler;

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
};

@vertex
fn vs(@builtin(vertex_index) i: u32, layer: Instance) -> VOut {
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
    return o;
}

// 调色（sRGB 0–1、不预乘）：颜色矩阵，和 ve_core::ColorMatrix::apply 一样（片段自己的调色和
// 调节片段都已经乘进这个矩阵里）
fn adjust_color(c: vec3<f32>, v: VOut) -> vec3<f32> {
    let x = vec4(c, 1.0);
    return clamp(vec3(dot(v.red, x), dot(v.green, x), dot(v.blue, x)), vec3(0.0), vec3(1.0));
}

// 圆角矩形的有向距离（中心在原点、半宽高 b、圆角 r；外面为正）
fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let rr = min(r, min(b.x, b.y));
    let q = abs(p) - b + vec2(rr);
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - rr;
}

fn dot2(v: vec2<f32>) -> f32 {
    return dot(v, v);
}

// 爱心（iq 的 sdHeart：底尖在原点、高约 1，y 向上）
fn sd_heart(p_in: vec2<f32>) -> f32 {
    let p = vec2(abs(p_in.x), p_in.y);
    if p.y + p.x > 1.0 {
        return sqrt(dot2(p - vec2(0.25, 0.75))) - sqrt(2.0) / 4.0;
    }
    return sqrt(min(dot2(p - vec2(0.0, 1.0)), dot2(p - 0.5 * max(p.x + p.y, 0.0))))
        * sign(p.x - p.y);
}

// 五角星（iq 的 sdStar5：外接圆半径 r、内凹 rf，尖朝上，y 向上）
fn sd_star5(p_in: vec2<f32>, r: f32, rf: f32) -> f32 {
    let k1 = vec2(0.809016994375, -0.587785252292);
    let k2 = vec2(-k1.x, k1.y);
    var p = vec2(abs(p_in.x), p_in.y);
    p = p - 2.0 * max(dot(k1, p), 0.0) * k1;
    p = p - 2.0 * max(dot(k2, p), 0.0) * k2;
    p = vec2(abs(p.x), p.y - r);
    let ba = rf * vec2(-k1.y, k1.x) - vec2(0.0, 1.0);
    let h = clamp(dot(p, ba) / dot(ba, ba), 0.0, r);
    return length(p - ba * h) * sign(p.y * ba.x - p.x * ba.y);
}

// 蒙版形状的有向距离（像素，外面为正；反转时里外对调）；没有蒙版时是很大的负数（处处都在里面）
fn mask_sd(local: vec2<f32>, v: VOut) -> f32 {
    let shape = v.shadow2.w;
    if shape < -0.5 {
        return -1e6;
    }
    let hb = max(v.mask.zw, vec2(0.5));
    let ang = v.mask2.x;
    let d0 = local - v.mask.xy;
    // 转回蒙版自己的方向
    let p = vec2(d0.x * cos(ang) + d0.y * sin(ang), -d0.x * sin(ang) + d0.y * cos(ang));
    let s = min(hb.x, hb.y);
    var d = 0.0;
    if shape < 0.5 {
        d = p.y;                                    // 线性：留上面
    } else if shape < 1.5 {
        d = abs(p.y) - hb.y;                        // 镜面：留中间一条
    } else if shape < 2.5 {
        d = (length(p / hb) - 1.0) * s;             // 圆形（椭圆）
    } else if shape < 3.5 {
        d = sd_round_box(p, hb, v.mask2.z);         // 矩形
    } else if shape < 4.5 {
        // 爱心：高 2*hb.y、宽 2*hb.x；iq 的爱心底尖在 0、顶在约 1.07、宽约 ±0.6
        let q = vec2(p.x / hb.x * 0.6, (-p.y / hb.y) * 0.54 + 0.5);
        d = sd_heart(q) * s * 1.6;
    } else {
        let q = vec2(p.x / hb.x, -p.y / hb.y);
        d = sd_star5(q, 1.0, 0.45) * s;
    }
    if v.mask2.w > 0.5 {
        d = -d;
    }
    return d;
}

// 蒙版留下的比例（0–1），按羽化渐变
fn mask_alpha(d: f32, v: VOut) -> f32 {
    let f = max(v.mask2.y, 0.75);
    return 1.0 - smoothstep(-f, f, d);
}

// BT.709 色度（Cb、Cr），和 ve_core::ChromaKey 一样
fn chroma(c: vec3<f32>) -> vec2<f32> {
    let y = dot(c, vec3(0.2126, 0.7152, 0.0722));
    return vec2((c.b - y) / 1.8556, (c.r - y) / 1.5748);
}

fn premul_over(top: vec4<f32>, under: vec4<f32>) -> vec4<f32> {
    return top + under * (1.0 - top.a);
}

// 这一层在这个像素上的颜色（预乘透明，已乘不透明度）
fn layer_color(v: VOut) -> vec4<f32> {
    let size = max(v.sizes.xy, vec2(1.0));
    let half_size = size * 0.5;
    let radius = v.p0.w;
    let d_frame = sd_round_box(v.local - half_size, half_size, radius);
    let d_mask = mask_sd(v.local, v);
    // 画面的形状 = 框 ∩ 蒙版：描边、阴影都贴着它
    let d = max(d_frame, d_mask);
    // 画面本身：只在框里面取样（框外的纹理坐标落在裁剪区外面）
    let t = clamp(v.local / size, vec2(0.0), vec2(1.0));
    let uv = vec2(mix(v.uv.x, v.uv.z, t.x), mix(v.uv.y, v.uv.w, t.y));
    let s = textureSampleLevel(tex, samp, uv, 0.0);
    // 框内（圆角）和蒙版留下的比例
    let cover = mask_alpha(d_mask, v) * clamp(0.5 - d_frame, 0.0, 1.0);
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
    // 描边：框外一圈
    let w = v.sizes.w;
    if w > 0.0 {
        let ring = clamp(w + 0.5 - d, 0.0, 1.0) * clamp(d + 0.5, 0.0, 1.0);
        c = premul_over(c, vec4(v.stroke.rgb * ring, ring));
    }
    // 阴影：偏移过的框，按模糊半径渐隐
    if v.shadow.a > 0.0 {
        let q = v.local - v.shadow2.xy;
        let ds = max(sd_round_box(q - half_size, half_size, radius), mask_sd(q, v));
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

fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        return b * 2.0 * s;
    }
    let s2 = 2.0 * s - 1.0;
    return b + s2 - b * s2;
}

// 一个通道的混合 B(底色, 图层色)，和 ve_core::BlendMode::channel 一样
fn blend_channel(mode: f32, cb: f32, cs: f32) -> f32 {
    if mode < 1.5 {
        return min(cb, cs);                      // 1 变暗
    } else if mode < 2.5 {
        return cb + cs - cb * cs;                // 2 滤色
    } else if mode < 3.5 {
        return hard_light(cs, cb);               // 3 叠加
    } else if mode < 4.5 {
        return cb * cs;                          // 4 正片叠底
    } else if mode < 5.5 {
        return max(cb, cs);                      // 5 变亮
    } else if mode < 6.5 {
        return hard_light(cb, cs);               // 6 强光
    } else if mode < 7.5 {                       // 7 柔光
        if cs <= 0.5 {
            return cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb);
        }
        var dd = sqrt(cb);
        if cb <= 0.25 {
            dd = ((16.0 * cb - 12.0) * cb + 4.0) * cb;
        }
        return cb + (2.0 * cs - 1.0) * (dd - cb);
    } else if mode < 8.5 {
        return max(cb + cs - 1.0, 0.0);          // 8 线性加深
    } else if mode < 9.5 {                       // 9 颜色加深
        if cb >= 1.0 {
            return 1.0;
        }
        if cs <= 0.0 {
            return 0.0;
        }
        return 1.0 - min((1.0 - cb) / cs, 1.0);
    }
    if cb <= 0.0 {                               // 10 颜色减淡
        return 0.0;
    }
    if cs >= 1.0 {
        return 1.0;
    }
    return min(cb / (1.0 - cs), 1.0);
}

@fragment
fn fs_blend(v: VOut) -> @location(0) vec4<f32> {
    let c = layer_color(v);
    let cb = textureLoad(backdrop, vec2<i32>(v.pos.xy), 0).rgb;
    if c.a <= 0.0 {
        return vec4(cb, 1.0);
    }
    let cs = clamp(c.rgb / c.a, vec3(0.0), vec3(1.0));
    let m = v.p0.z;
    let mixed = vec3(blend_channel(m, cb.r, cs.r), blend_channel(m, cb.g, cs.g), blend_channel(m, cb.b, cs.b));
    return vec4(cb * (1.0 - c.a) + mixed * c.a, 1.0);
}

// ---- 画面特效（ve_core::EffectKind）：读底图（已经合成好的画面），整个画面重画 ----
// p0 = （种类编号、强度 0–1、特效片段里过了几秒、0）

fn bd(p: vec2<i32>) -> vec3<f32> {
    let d = vec2<i32>(textureDimensions(backdrop));
    return textureLoad(backdrop, clamp(p, vec2(0), d - vec2(1)), 0).rgb;
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
    return vec4(clamp(out, vec3(0.0), vec3(1.0)), 1.0);
}
