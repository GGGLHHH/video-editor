//! 旋转光标：系统和 GPUI 都没有，这里自己画图案，再在 Windows 上设成真正的系统光标
//! （跟手、不拖影）。别的系统退回手指光标。
//!
//! 图案是一段弧线、两头带箭头，黑芯白边；弧线朝向 `angle`（鼠标相对画面中心的方向）鼓出，
//! 看上去像「绕着画面转」。弧线中点就是热点（图片正中间），光标指哪儿就点哪儿。

/// 光标图案的基准边长（100% 缩放时，像素）。
pub const BASE_SIZE: u32 = 32;
/// 朝向按这么多度一档缓存（24 档）。
pub const STEP_DEG: f32 = 15.0;

/// 弧线两头各离中点的角度（度）。
const SPAN_DEG: f32 = 40.0;

/// 画旋转光标：边长 `size` 像素，BGRA、非预乘透明、从上到下逐行。热点在正中间（弧线中点）。
#[must_use]
pub fn rotate_cursor_image(size: u32, angle_deg: f32) -> Vec<u8> {
    #[expect(clippy::cast_precision_loss, reason = "光标边长很小")]
    let (extent, k) = (size as f32, size as f32 / 32.0);
    let (dir, radius) = (angle_deg.to_radians(), arc_radius(size));
    let shape = Shape::new(dir, radius, k);
    // 弧线所在圆的圆心放在热点的反方向，弧线中点正好落在热点上
    let (sin, cos) = dir.sin_cos();
    let shift = (radius * cos - extent / 2.0, radius * sin - extent / 2.0);
    let outline = 2.0 * k;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            #[expect(clippy::cast_precision_loss, reason = "光标边长很小")]
            let p = (x as f32 + 0.5 + shift.0, y as f32 + 0.5 + shift.1);
            let d = shape.distance(p);
            let core = (0.5 - d).clamp(0.0, 1.0);
            let all = (outline + 0.5 - d).clamp(0.0, 1.0);
            // 白边上叠黑芯：颜色按黑芯占白边的比例从白到黑
            let white = if all > 0.0 { 1.0 - core / all } else { 0.0 };
            let c = to_byte(white);
            out.extend_from_slice(&[c, c, c, to_byte(all)]);
        }
    }
    out
}

/// 弧线半径（像素）。
#[must_use]
pub fn arc_radius(size: u32) -> f32 {
    #[expect(clippy::cast_precision_loss, reason = "光标边长很小")]
    let radius = size as f32 * 9.0 / 32.0;
    radius
}

/// 朝向换算成缓存档位（0–23）。
#[must_use]
pub fn bucket(angle_deg: f32) -> u32 {
    #[expect(clippy::cast_possible_truncation, reason = "取整后的档位很小")]
    let b = ((angle_deg / STEP_DEG).round() as i64).rem_euclid(24) as u32;
    b
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "已夹到 0–255"
)]
fn to_byte(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

type V = (f32, f32);

fn sub(a: V, b: V) -> V {
    (a.0 - b.0, a.1 - b.1)
}

fn dot(a: V, b: V) -> f32 {
    a.0.mul_add(b.0, a.1 * b.1)
}

fn cross(a: V, b: V) -> f32 {
    a.0.mul_add(b.1, -(a.1 * b.0))
}

/// 弧线加两头的箭头。
struct Shape {
    dir: f32,
    span: f32,
    radius: f32,
    half_width: f32,
    heads: [[V; 3]; 2],
}

impl Shape {
    fn new(dir: f32, radius: f32, k: f32) -> Self {
        let span = SPAN_DEG.to_radians();
        let (len, half) = (4.5 * k, 3.5 * k);
        let head = |end: f32, outward: f32| {
            let (sin, cos) = end.sin_cos();
            let p = (radius * cos, radius * sin);
            // 沿弧继续往外的切线方向、径向
            let t = (-sin * outward, cos * outward);
            let n = (cos, sin);
            let back = (p.0 - t.0 * k, p.1 - t.1 * k);
            [
                (p.0 + t.0 * len, p.1 + t.1 * len),
                (back.0 + n.0 * half, back.1 + n.1 * half),
                (back.0 - n.0 * half, back.1 - n.1 * half),
            ]
        };
        Self {
            dir,
            span,
            radius,
            half_width: 1.25 * k,
            heads: [head(dir + span, 1.0), head(dir - span, -1.0)],
        }
    }

    /// 到形状的距离（里面为负）。
    fn distance(&self, p: V) -> f32 {
        let a = p.1.atan2(p.0);
        let da = (a - self.dir + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        let arc = if da.abs() <= self.span {
            (p.0.hypot(p.1) - self.radius).abs() - self.half_width
        } else {
            let end = self.dir + self.span.copysign(da);
            let e = (self.radius * end.cos(), self.radius * end.sin());
            let q = sub(p, e);
            q.0.hypot(q.1) - self.half_width
        };
        self.heads
            .iter()
            .map(|t| triangle_distance(p, t))
            .fold(arc, f32::min)
    }
}

/// 点到三角形的有符号距离（里面为负）。
fn triangle_distance(point: V, tri: &[V; 3]) -> f32 {
    let edges = [
        sub(tri[1], tri[0]),
        sub(tri[2], tri[1]),
        sub(tri[0], tri[2]),
    ];
    let orient = cross(edges[0], edges[2]).signum();
    let mut best_sq = f32::INFINITY;
    let mut inside = true;
    for (edge, corner) in edges.into_iter().zip(tri) {
        let rel = sub(point, *corner);
        let along = (dot(rel, edge) / dot(edge, edge)).clamp(0.0, 1.0);
        let off = (edge.0.mul_add(-along, rel.0), edge.1.mul_add(-along, rel.1));
        best_sq = best_sq.min(dot(off, off));
        // 点要在三条边的同一侧（和三角形的绕向一致）才算在里面
        if orient * cross(rel, edge) < 0.0 {
            inside = false;
        }
    }
    let dist = best_sq.sqrt();
    if inside { -dist } else { dist }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(img: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn arc_faces_the_given_direction() {
        let s = BASE_SIZE;
        let right = rotate_cursor_image(s, 0.0);
        assert_eq!(right.len(), (s * s * 4) as usize);
        let c = s / 2;
        // 弧线半径：32 像素时 9 像素
        let r = s * 9 / 32;
        // 弧线中点就在热点（正中间）上，黑色不透明；弧线所在圆的圆心在热点反方向，透明
        assert_eq!(pixel(&right, s, c, c), [0, 0, 0, 255]);
        assert_eq!(pixel(&right, s, c - r, c)[3], 0);
        // 朝下（90°）：圆心移到热点上方，热点右边的弧外侧透明
        let down = rotate_cursor_image(s, 90.0);
        assert_eq!(pixel(&down, s, c, c), [0, 0, 0, 255]);
        assert_eq!(pixel(&down, s, c, c - r)[3], 0);
        assert_eq!(pixel(&down, s, c + 1, c + 5)[3], 0);
    }

    #[test]
    fn black_core_has_white_outline() {
        let s = BASE_SIZE;
        let img = rotate_cursor_image(s, 0.0);
        let c = s / 2;
        // 弧外侧挨着黑芯的一圈是白边（白、不透明）
        let edge = pixel(&img, s, c + 2, c);
        assert_eq!(edge, [255, 255, 255, 255], "{edge:?}");
        // 再往外是透明的
        assert_eq!(pixel(&img, s, c + 5, c)[3], 0);
    }

    #[test]
    fn arrow_heads_are_solid() {
        // 朝右时弧的下端在 40°；沿切线往外 1.5 像素是箭头中间，应是实心黑（光靠弧线的圆头够不着）
        let img = rotate_cursor_image(BASE_SIZE, 0.0);
        assert_eq!(pixel(&img, BASE_SIZE, 12, 22), [0, 0, 0, 255]);
    }

    #[test]
    fn scales_with_size() {
        let big = rotate_cursor_image(64, 0.0);
        assert_eq!(big.len(), 64 * 64 * 4);
        assert_eq!(pixel(&big, 64, 32, 32), [0, 0, 0, 255]);
        assert_eq!(pixel(&big, 64, 32 + 4, 32), [255, 255, 255, 255]);
    }

    #[test]
    fn buckets_wrap_every_full_turn() {
        assert_eq!(bucket(0.0), 0);
        assert_eq!(bucket(14.0), 1);
        assert_eq!(bucket(-15.0), 23);
        assert_eq!(bucket(360.0), 0);
        assert_eq!(bucket(353.0), 0);
        assert_eq!(bucket(352.0), 23);
    }
}
