// 图层着色器（shader.wgsl）和复合片段分组（group.wgsl）共用的形状函数：圆角矩形、蒙版形状的
// 有向距离和按羽化留下的比例。编译时拼在它们前面。

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

// 一个蒙版的有向距离：形状编号（-1 = 没有）、（中心、半宽高）、（旋转、羽化、圆角、反转）
fn shape_sd(local: vec2<f32>, shape: f32, mask: vec4<f32>, mask2: vec4<f32>) -> f32 {
    if shape < -0.5 {
        return -1e6;
    }
    let hb = max(mask.zw, vec2(0.5));
    let ang = mask2.x;
    let d0 = local - mask.xy;
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
        d = sd_round_box(p, hb, mask2.z);           // 矩形
    } else if shape < 4.5 {
        // 爱心：高 2*hb.y、宽 2*hb.x；iq 的爱心底尖在 0、顶在约 1.07、宽约 ±0.6
        let q = vec2(p.x / hb.x * 0.6, (-p.y / hb.y) * 0.54 + 0.5);
        d = sd_heart(q) * s * 1.6;
    } else {
        let q = vec2(p.x / hb.x, -p.y / hb.y);
        d = sd_star5(q, 1.0, 0.45) * s;
    }
    if mask2.w > 0.5 {
        d = -d;
    }
    return d;
}

// 蒙版留下的比例（0–1），按羽化渐变
fn mask_alpha(d: f32, feather: f32) -> f32 {
    let f = max(feather, 0.75);
    return 1.0 - smoothstep(-f, f, d);
}

