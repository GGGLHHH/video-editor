//! 素材面板「特效」卡片上的示意图（剪映的特效卡片有预览图）。
//!
//! 不跑真的特效：用几层 div 拼一个小风景（天空渐变、地面、太阳），
//! 再按特效种类叠一层能一眼看出效果的东西（暗角、颗粒、马赛克、色差……）。

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{
    AnyElement, Hsla, InteractiveElement as _, IntoElement as _, ParentElement as _, Rgba,
    SharedString, Styled as _, div, linear_color_stop, linear_gradient, px, rgb,
};
use ve_core::EffectKind;

/// 示意图的宽、高（像素，和卡片里的框一样大，减掉边框）。
const W: f32 = 84.0;
const H: f32 = 46.0;
/// 地平线（离顶部的像素）。
const HORIZON: f32 = 30.0;
/// 太阳中心和半径。
const SUN: (f32, f32, f32) = (56.0, 15.0, 7.0);

const SKY_TOP: u32 = 0x3E_6FC4;
const SKY_LOW: u32 = 0xF0_B46E;
const GROUND: u32 = 0x37_6B45;
const SUN_COLOR: u32 = 0xFF_E38A;

/// 第 `i` 张特效卡片的示意图（元素 id `bin-effect-{i}-preview`）。
pub(crate) fn effect_preview(i: usize, kind: EffectKind) -> AnyElement {
    let mut scene = div()
        .relative()
        .w(px(W))
        .h(px(H))
        .overflow_hidden()
        .child(sky())
        .child(ground());
    scene = match kind {
        EffectKind::Mosaic => scene.children(mosaic()),
        EffectKind::ChromaShift => scene
            .child(sun_at(-3.0, 0.0, SUN.2, tint(0xFF_3B3B, 0.75)))
            .child(sun_at(3.0, 0.0, SUN.2, tint(0x3B_E6FF, 0.75)))
            .child(sun_at(0.0, 0.0, SUN.2, tint(SUN_COLOR, 0.9))),
        EffectKind::Blur => scene
            .child(sun_at(0.0, 0.0, SUN.2 + 4.0, tint(SUN_COLOR, 0.25)))
            .child(sun_at(0.0, 0.0, SUN.2 + 2.0, tint(SUN_COLOR, 0.45)))
            .child(sun_at(0.0, 0.0, SUN.2, tint(SUN_COLOR, 0.7)))
            // 地平线糊开
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(HORIZON - 4.0))
                    .w(px(W))
                    .h(px(8.0))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(tint(SKY_LOW, 0.0), 0.),
                        linear_color_stop(tint(GROUND, 0.9), 1.),
                    )),
            )
            .child(overlay(0xFF_FFFF, 0.12)),
        EffectKind::Glow => scene
            .child(sun_at(0.0, 0.0, SUN.2 + 9.0, tint(0xFF_F2B0, 0.25)))
            .child(sun_at(0.0, 0.0, SUN.2 + 5.0, tint(0xFF_F2B0, 0.45)))
            .child(sun_at(0.0, 0.0, SUN.2, tint(0xFF_FFFF, 1.0))),
        _ => scene.child(sun_at(0.0, 0.0, SUN.2, tint(SUN_COLOR, 1.0))),
    };
    scene = match kind {
        EffectKind::Sharpen => scene
            .child(
                // 太阳描一圈深边、地平线一条亮线：边缘更「硬」
                sun_at(0.0, 0.0, SUN.2, tint(SUN_COLOR, 0.0))
                    .border_1()
                    .border_color(tint(0x20_1A10, 0.8)),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(HORIZON))
                    .w(px(W))
                    .h(px(1.0))
                    .bg(tint(0xFF_FFFF, 0.85)),
            ),
        EffectKind::Vignette => scene.children(vignette()),
        EffectKind::Grain => scene.children(grain()),
        EffectKind::Flash => scene.child(overlay(0xFF_FFFF, 0.6)),
        _ => scene,
    };
    div()
        .id(SharedString::from(format!("bin-effect-{i}-preview")))
        .test_support()
        .child(scene)
        .into_any_element()
}

fn tint(hex: u32, alpha: f32) -> Hsla {
    let mut c: Rgba = rgb(hex);
    c.a = alpha;
    c.into()
}

fn sky() -> gpui_kit::Div {
    div()
        .absolute()
        .left_0()
        .top_0()
        .w(px(W))
        .h(px(HORIZON))
        .bg(linear_gradient(
            180.,
            linear_color_stop(rgb(SKY_TOP), 0.),
            linear_color_stop(rgb(SKY_LOW), 1.),
        ))
}

fn ground() -> gpui_kit::Div {
    div()
        .absolute()
        .left_0()
        .top(px(HORIZON))
        .w(px(W))
        .h(px(H - HORIZON))
        .bg(rgb(GROUND))
}

/// 太阳（相对正常位置挪 `(dx, dy)`，半径 `r`）。
fn sun_at(dx: f32, dy: f32, r: f32, color: Hsla) -> gpui_kit::Div {
    div()
        .absolute()
        .left(px(SUN.0 + dx - r))
        .top(px(SUN.1 + dy - r))
        .size(px(r * 2.0))
        .rounded_full()
        .bg(color)
}

fn overlay(hex: u32, alpha: f32) -> gpui_kit::Div {
    div()
        .absolute()
        .left_0()
        .top_0()
        .w(px(W))
        .h(px(H))
        .bg(tint(hex, alpha))
}

/// 四边往里变暗。
fn vignette() -> [gpui_kit::Div; 4] {
    let dark = tint(0x00_0000, 0.75);
    let clear = tint(0x00_0000, 0.0);
    let band = |angle: f32| {
        linear_gradient(
            angle,
            linear_color_stop(dark, 0.),
            linear_color_stop(clear, 1.),
        )
    };
    let side = 18.0;
    let edge = 14.0;
    [
        div()
            .absolute()
            .left_0()
            .top_0()
            .w(px(side))
            .h(px(H))
            .bg(band(90.)),
        div()
            .absolute()
            .right_0()
            .top_0()
            .w(px(side))
            .h(px(H))
            .bg(band(270.)),
        div()
            .absolute()
            .left_0()
            .top_0()
            .w(px(W))
            .h(px(edge))
            .bg(band(180.)),
        div()
            .absolute()
            .left_0()
            .bottom_0()
            .w(px(W))
            .h(px(edge))
            .bg(band(0.)),
    ]
}

/// 撒一把固定位置的亮点、暗点（伪随机，每次画都一样）。
fn grain() -> Vec<gpui_kit::Div> {
    let mut seed: u32 = 0x9E37_79B9;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    (0..70)
        .map(|k| {
            #[expect(clippy::cast_precision_loss, reason = "示意图坐标")]
            let (x, y) = ((next() % 84) as f32, (next() % 46) as f32);
            let color = if k % 2 == 0 {
                tint(0xFF_FFFF, 0.55)
            } else {
                tint(0x00_0000, 0.45)
            };
            div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .size(px(1.5))
                .bg(color)
        })
        .collect()
}

/// 方块：每格取格子中心那点在小风景里的颜色。
fn mosaic() -> Vec<gpui_kit::Div> {
    const CELL: f32 = 7.0;
    let (cols, rows) = (12_u8, 7_u8);
    let mut out = Vec::with_capacity(usize::from(cols) * usize::from(rows));
    for row in 0..rows {
        for col in 0..cols {
            let (x, y) = (f32::from(col) * CELL, f32::from(row) * CELL);
            let (cx, cy) = (x + CELL / 2.0, y + CELL / 2.0);
            let color = scene_color(cx, cy);
            out.push(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .size(px(CELL))
                    .bg(color),
            );
        }
    }
    out
}

/// 小风景在 `(x, y)` 处的颜色（马赛克取色用）。
fn scene_color(px_x: f32, px_y: f32) -> Hsla {
    let (sun_x, sun_y, radius) = SUN;
    if (px_x - sun_x).hypot(px_y - sun_y) <= radius + 1.0 {
        return rgb(SUN_COLOR).into();
    }
    if px_y >= HORIZON {
        return rgb(GROUND).into();
    }
    let along = (px_y / HORIZON).clamp(0.0, 1.0);
    let (top, low): (Rgba, Rgba) = (rgb(SKY_TOP), rgb(SKY_LOW));
    Rgba {
        r: (low.r - top.r).mul_add(along, top.r),
        g: (low.g - top.g).mul_add(along, top.g),
        b: (low.b - top.b).mul_add(along, top.b),
        a: 1.0,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mosaic_takes_its_colours_from_the_scene() {
        let sun: Hsla = rgb(SUN_COLOR).into();
        let ground: Hsla = rgb(GROUND).into();
        assert_eq!(scene_color(SUN.0, SUN.1), sun);
        assert_eq!(scene_color(5.0, H - 2.0), ground);
        // 天空上面偏蓝、靠地平线偏暖
        let (top, low) = (scene_color(2.0, 1.0), scene_color(2.0, HORIZON - 1.0));
        assert!(top.h > 0.5 && low.h < 0.2, "{top:?} {low:?}");
    }

    #[test]
    fn the_grain_dots_stay_inside_the_picture() {
        assert_eq!(grain().len(), 70);
        assert_eq!(mosaic().len(), 12 * 7);
    }
}
