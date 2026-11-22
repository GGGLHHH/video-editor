//! 文字蒙版：把字排好、画成灰度，再算成有向距离场（每个像素离字的边多远），交给着色器当蒙版形状。
//! 距离场能和别的形状一样按距离做羽化、描边、阴影，放大缩小也不糊。

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache};
use std::sync::{Mutex, OnceLock};

/// 排字用的字号（像素）。
const GLYPH_PX: f32 = 96.0;
/// 距离场记到离边多远（像素）；超过的都按这么远算。
pub const SPREAD: f32 = 12.0;

/// 算好的距离场：`data` 一个像素一字节，128 = 正好在边上，越大越在字里面（每 [`SPREAD`]
/// 像素差 127）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextField {
    /// 宽。
    pub width: u32,
    /// 高。
    pub height: u32,
    /// 一行行排好的像素。
    pub data: Vec<u8>,
}

/// 整个进程共用一份系统字体（第一次用时扫一遍字体目录，要一点时间）。
fn fonts() -> &'static Mutex<(FontSystem, SwashCache)> {
    static FONTS: OnceLock<Mutex<(FontSystem, SwashCache)>> = OnceLock::new();
    FONTS.get_or_init(|| Mutex::new((FontSystem::new(), SwashCache::new())))
}

/// 把字排好画成灰度（0–255 覆盖度），四周留 [`SPREAD`] 的边；没有能画的字时 `None`。
fn coverage(text: &str, font: &str) -> Option<(usize, usize, Vec<u8>)> {
    let mut guard = fonts().lock().ok()?;
    let (system, cache) = &mut *guard;
    let mut buffer = Buffer::new(system, Metrics::new(GLYPH_PX, GLYPH_PX * 1.25));
    let family = if font.is_empty() {
        Family::SansSerif
    } else {
        Family::Name(font)
    };
    let mut laid = buffer.borrow_with(system);
    laid.set_size(None, None);
    laid.set_text(text, &Attrs::new().family(family), Shaping::Advanced, None);
    laid.shape_until_scroll(false);
    let (mut text_w, mut text_h) = (0.0f32, 0.0f32);
    for run in laid.layout_runs() {
        text_w = text_w.max(run.line_w);
        text_h = text_h.max(run.line_top + run.line_height);
    }
    if text_w < 1.0 || text_h < 1.0 {
        return None;
    }
    let pad = SPREAD.ceil();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "排好的字只有几千像素宽"
    )]
    let (width, height) = (
        (2.0f32.mul_add(pad, text_w)).ceil() as usize,
        (2.0f32.mul_add(pad, text_h)).ceil() as usize,
    );
    let mut out = vec![0u8; width * height];
    #[expect(clippy::cast_possible_truncation, reason = "边距是小整数")]
    let offset = pad as i32;
    laid.draw(
        cache,
        Color::rgb(255, 255, 255),
        |left, top, cols, rows, color| {
            let alpha = color.a();
            for dy in 0..i32::try_from(rows).unwrap_or(0) {
                for dx in 0..i32::try_from(cols).unwrap_or(0) {
                    let (Ok(col), Ok(row)) = (
                        usize::try_from(left + dx + offset),
                        usize::try_from(top + dy + offset),
                    ) else {
                        continue;
                    };
                    if col < width && row < height {
                        let cell = &mut out[row * width + col];
                        *cell = (*cell).max(alpha);
                    }
                }
            }
        },
    );
    drop(guard);
    out.iter()
        .any(|&a| a >= 128)
        .then_some((width, height, out))
}

/// 一维平方距离变换（Felzenszwalb）：`cost` 里是每个位置自己的代价（种子 0，别处很大），
/// 结果写进 `out`；`hull`、`bounds` 是工作区（长度至少 n、n + 1）。
fn transform_line(cost: &[f32], out: &mut [f32], hull: &mut [usize], bounds: &mut [f32]) {
    let count = cost.len();
    #[expect(clippy::cast_precision_loss, reason = "下标很小")]
    let pos = |i: usize| i as f32;
    let mut top = 0usize;
    hull[0] = 0;
    bounds[0] = f32::NEG_INFINITY;
    bounds[1] = f32::INFINITY;
    for q in 1..count {
        loop {
            let p = hull[top];
            let cross = (pos(q).mul_add(pos(q), cost[q]) - pos(p).mul_add(pos(p), cost[p]))
                / (2.0 * pos(q) - 2.0 * pos(p));
            if cross <= bounds[top] && top > 0 {
                top -= 1;
            } else {
                top += 1;
                hull[top] = q;
                bounds[top] = cross;
                bounds[top + 1] = f32::INFINITY;
                break;
            }
        }
    }
    top = 0;
    for (q, slot) in out.iter_mut().enumerate() {
        loop {
            if bounds[top + 1] < pos(q) {
                top += 1;
            } else {
                break;
            }
        }
        let gap = pos(q) - pos(hull[top]);
        *slot = gap.mul_add(gap, cost[hull[top]]);
    }
}

/// 二维：每个像素到最近的种子像素的距离。
fn distance(seed: &[bool], width: usize, height: usize) -> Vec<f32> {
    const FAR: f32 = 1e20;
    let mut grid: Vec<f32> = seed.iter().map(|&s| if s { 0.0 } else { FAR }).collect();
    let longest = width.max(height);
    let (mut line, mut out) = (vec![0.0; longest], vec![0.0; longest]);
    let (mut hull, mut bounds) = (vec![0usize; longest], vec![0.0f32; longest + 1]);
    for x in 0..width {
        for y in 0..height {
            line[y] = grid[y * width + x];
        }
        transform_line(&line[..height], &mut out[..height], &mut hull, &mut bounds);
        for y in 0..height {
            grid[y * width + x] = out[y];
        }
    }
    for y in 0..height {
        let row = y * width..(y + 1) * width;
        line[..width].copy_from_slice(&grid[row.clone()]);
        transform_line(&line[..width], &mut out[..width], &mut hull, &mut bounds);
        grid[row].copy_from_slice(&out[..width]);
    }
    grid.into_iter().map(f32::sqrt).collect()
}

/// 把字算成距离场；没有能画的字（空的、全是空格）时 `None`。
#[must_use]
pub fn rasterize(text: &str, font: &str) -> Option<TextField> {
    let (width, height, cover) = coverage(text, font)?;
    let inside: Vec<bool> = cover.iter().map(|&a| a >= 128).collect();
    let outside: Vec<bool> = inside.iter().map(|&i| !i).collect();
    // 外面的点离字多远、里面的点离外面多远；边上按覆盖度补半个像素以内的细节
    let to_inside = distance(&inside, width, height);
    let to_outside = distance(&outside, width, height);
    let data = (0..width * height)
        .map(|i| {
            let edge = (f32::from(cover[i]) / 255.0) - 0.5;
            let signed = if inside[i] {
                to_outside[i] - 0.5
            } else {
                -(to_inside[i] - 0.5)
            } + edge;
            let level = (signed / SPREAD).clamp(-1.0, 1.0).mul_add(127.0, 128.0);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "1–255"
            )]
            let byte = level.round() as u8;
            byte
        })
        .collect();
    Some(TextField {
        width: u32::try_from(width).ok()?,
        height: u32::try_from(height).ok()?,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(f: &TextField, x: u32, y: u32) -> u8 {
        f.data[(y * f.width + x) as usize]
    }

    #[test]
    fn a_letter_is_inside_in_the_middle_and_outside_at_the_corners() {
        let f = rasterize("I", "").expect("系统里有无衬线字体");
        assert!(
            f.height > f.width,
            "「I」是竖长的：{}×{}",
            f.width,
            f.height
        );
        // 四角是边距，离字最远
        assert!(at(&f, 0, 0) < 10, "{}", at(&f, 0, 0));
        assert!(at(&f, f.width - 1, f.height - 1) < 10);
        // 竖笔画中间是字里面
        let inside = (0..f.height).map(|y| at(&f, f.width / 2, y)).max().unwrap();
        assert!(inside > 140, "{inside}");
    }

    #[test]
    fn wider_text_gives_a_wider_field_and_blank_text_gives_none() {
        let one = rasterize("文", "").unwrap();
        let two = rasterize("文字", "").unwrap();
        assert!(
            two.width > one.width + one.width / 2,
            "{} {}",
            one.width,
            two.width
        );
        assert_eq!(rasterize("   ", ""), None);
        assert_eq!(rasterize("", ""), None);
    }

    #[test]
    fn the_distance_transform_measures_straight_lines() {
        // 一个点在 (2, 1)：(0, 1) 离它 2，(2, 4) 离它 3，(5, 5) 离它 5
        let (w, h) = (6, 6);
        let mut seed = vec![false; w * h];
        seed[w + 2] = true;
        let dist = distance(&seed, w, h);
        assert!((dist[w] - 2.0).abs() < 1e-4);
        assert!((dist[4 * w + 2] - 3.0).abs() < 1e-4);
        assert!((dist[5 * w + 5] - 5.0).abs() < 1e-4);
    }
}
