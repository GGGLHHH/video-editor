//! 示波器（达芬奇 / Premiere 的 Lumetri 示波器；剪映没有）：按播放器当前这一帧画亮度波形图、
//! 矢量示波器（色度分布）和 RGB 直方图。在 CPU 上隔点采样算，画成小图贴在画面右上角。

use crate::preview::to_render_image;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    RenderImage, Styled as _, Window, div, img, px,
};
use std::sync::Arc;
use ve_core::Time;
use ve_media::VideoFrame;

/// 波形图大小。
pub const WAVEFORM: (u32, u32) = (256, 128);
/// 矢量示波器大小。
pub const VECTORSCOPE: u32 = 128;
/// 直方图大小。
pub const HISTOGRAM: (u32, u32) = (256, 96);

/// 三张示波器图（BGRA）。
#[derive(Debug)]
pub struct ScopeImages {
    /// 亮度波形：横轴是画面的横向位置，纵轴是亮度（上面亮）。
    pub waveform: VideoFrame,
    /// 矢量示波器：圆心是灰色，往外饱和度越高，角度是色相。
    pub vectorscope: VideoFrame,
    /// RGB 直方图：横轴 0–255，纵轴是像素多少。
    pub histogram: VideoFrame,
}

fn blank(w: u32, h: u32) -> VideoFrame {
    let mut data = vec![0u8; (w * h * 4) as usize];
    for px in data.as_chunks_mut::<4>().0 {
        *px = [12, 12, 12, 255];
    }
    VideoFrame {
        pts: Time::ZERO,
        width: w,
        height: h,
        data,
    }
}

/// 计数转亮度（开平方，暗处也看得见），`max` 是最多的那一格。
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "0–255"
)]
fn level(count: u32, max: u32) -> u8 {
    if count == 0 || max == 0 {
        return 0;
    }
    ((count as f32 / max as f32).sqrt() * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// 按帧 `f`（BGRA）算三张示波器图：隔点采样，最多取约 256×144 个点。
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::many_single_char_names,
    reason = "像素坐标、宽高 w / h、颜色 r / g / b 和 0–255 的颜色"
)]
pub fn compute(f: &VideoFrame) -> ScopeImages {
    let (w, h) = (f.width.max(1), f.height.max(1));
    let sx = (w / 256).max(1);
    let sy = (h / 144).max(1);
    let (ww, wh) = WAVEFORM;
    let vs = VECTORSCOPE;
    let mut wave = vec![0u32; (ww * wh) as usize];
    let mut vec_counts = vec![0u32; (vs * vs) as usize];
    let mut hist = [[0u32; 256]; 3];
    for y in (0..h).step_by(sy as usize) {
        for x in (0..w).step_by(sx as usize) {
            let i = ((y * w + x) * 4) as usize;
            let Some(&[b, g, r, _]) = f.data.get(i..i + 4).and_then(|s| s.as_array::<4>()) else {
                continue;
            };
            hist[0][usize::from(r)] += 1;
            hist[1][usize::from(g)] += 1;
            hist[2][usize::from(b)] += 1;
            let (rf, gf, bf) = (
                f32::from(r) / 255.0,
                f32::from(g) / 255.0,
                f32::from(b) / 255.0,
            );
            // BT.709
            let luma = 0.2126 * rf + 0.7152 * gf + 0.0722 * bf;
            let col = (x * ww / w).min(ww - 1);
            let row = (((1.0 - luma) * (wh - 1) as f32).round() as u32).min(wh - 1);
            wave[(row * ww + col) as usize] += 1;
            let cb = (bf - luma) / 1.8556;
            let cr = (rf - luma) / 1.5748;
            let vx = (((cb + 0.5) * (vs - 1) as f32).round() as u32).min(vs - 1);
            let vy = (((0.5 - cr) * (vs - 1) as f32).round() as u32).min(vs - 1);
            vec_counts[(vy * vs + vx) as usize] += 1;
        }
    }
    // 波形：绿色
    let mut waveform = blank(ww, wh);
    let wmax = wave.iter().copied().max().unwrap_or(0);
    for (px, &c) in waveform.data.as_chunks_mut::<4>().0.iter_mut().zip(&wave) {
        let v = level(c, wmax);
        if v > 0 {
            *px = [v / 3, v, v / 3, 255];
        }
    }
    // 矢量：白点，十字线
    let mut vectorscope = blank(vs, vs);
    let vmax = vec_counts.iter().copied().max().unwrap_or(0);
    for (i, px) in vectorscope
        .data
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .enumerate()
    {
        let (x, y) = (i as u32 % vs, i as u32 / vs);
        if x == vs / 2 || y == vs / 2 {
            *px = [48, 48, 48, 255];
        }
        let v = level(vec_counts[i], vmax);
        if v > 0 {
            *px = [v, v, v, 255];
        }
    }
    // 直方图：三色叠加
    let (hw, hh) = HISTOGRAM;
    let mut histogram = blank(hw, hh);
    let hmax = hist.iter().flatten().copied().max().unwrap_or(0).max(1);
    for x in 0..hw {
        let bin = (x * 256 / hw) as usize;
        let heights = hist.map(|ch| (ch[bin] as f32 / hmax as f32 * hh as f32).round() as u32);
        for y in 0..hh {
            let up = hh - 1 - y;
            let i = ((y * hw + x) * 4) as usize;
            let on = heights.map(|t| up < t);
            if on.iter().any(|&o| o) {
                let c = |o: bool| if o { 200 } else { 30 };
                histogram.data[i..i + 4].copy_from_slice(&[c(on[2]), c(on[1]), c(on[0]), 255]);
            }
        }
    }
    ScopeImages {
        waveform,
        vectorscope,
        histogram,
    }
}

/// 示波器面板的状态。
#[derive(Debug, Default)]
pub(crate) struct ScopeState {
    /// 开着没有。
    pub(crate) on: bool,
    /// 画好的三张图。
    images: Option<[Arc<RenderImage>; 3]>,
    /// 上一组：这一帧可能还在屏幕上，等下一次换图再从图集里删（和预览画面一样）。
    retired: Option<[Arc<RenderImage>; 3]>,
}

impl Workspace {
    /// 开 / 关示波器。
    pub(crate) fn toggle_scopes(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.scopes.on = !self.scopes.on;
        self.refresh_scopes(window, cx);
    }

    /// 示波器开着没有。
    #[must_use]
    pub const fn scopes_on(&self) -> bool {
        self.scopes.on
    }

    /// 按播放器当前这一帧重画示波器（开着时每换一帧调用）。
    pub(crate) fn refresh_scopes(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let next = if self.scopes.on {
            self.preview.last_frame.as_deref().and_then(|f| {
                let s = compute(f);
                Some([
                    to_render_image(&s.waveform)?,
                    to_render_image(&s.vectorscope)?,
                    to_render_image(&s.histogram)?,
                ])
            })
        } else {
            None
        };
        if let Some(old) = self.scopes.retired.take() {
            for image in old {
                cx.drop_image(image, Some(window));
            }
        }
        self.scopes.retired = std::mem::replace(&mut self.scopes.images, next);
        cx.notify();
    }

    /// 画面右上角的示波器面板。
    #[expect(clippy::cast_precision_loss, reason = "几百像素的图大小")]
    pub(crate) fn render_scopes(&self) -> Option<AnyElement> {
        let [wave, vector, hist] = self.scopes.images.clone()?;
        let label = |t: &'static str| div().text_xs().text_color(gpui_kit::white()).child(t);
        Some(
            v_flex()
                .id("scopes")
                .test_support()
                .absolute()
                .top(px(8.))
                .right(px(8.))
                .p_1()
                .gap_1()
                .rounded(px(4.))
                .bg(gpui_kit::rgba(0x0000_00c0))
                .child(label("亮度波形"))
                .child(img(wave).w(px(WAVEFORM.0 as f32)).h(px(WAVEFORM.1 as f32)))
                .child(
                    h_flex()
                        .gap_1()
                        .items_start()
                        .child(
                            v_flex().child(label("矢量")).child(
                                img(vector)
                                    .w(px(VECTORSCOPE as f32))
                                    .h(px(VECTORSCOPE as f32)),
                            ),
                        )
                        .child(
                            v_flex()
                                .child(label("直方图"))
                                .child(img(hist).w(px(120.)).h(px(HISTOGRAM.1 as f32))),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
#[expect(clippy::cast_possible_truncation, reason = "测试里的像素下标很小")]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> VideoFrame {
        VideoFrame {
            pts: Time::ZERO,
            width: w,
            height: h,
            data: bgra.repeat((w * h) as usize),
        }
    }

    fn bright(f: &VideoFrame) -> Vec<(u32, u32)> {
        f.data
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, p)| p[1] > 100)
            .map(|(i, _)| (i as u32 % f.width, i as u32 / f.width))
            .collect()
    }

    #[test]
    fn a_mid_gray_frame_lands_in_the_middle_of_every_scope() {
        let s = compute(&solid(64, 36, [128, 128, 128, 255]));
        // 波形：只有中间那一行亮
        let rows: std::collections::BTreeSet<u32> =
            bright(&s.waveform).iter().map(|p| p.1).collect();
        assert_eq!(rows.len(), 1);
        let row = *rows.first().unwrap();
        assert!(row.abs_diff(WAVEFORM.1 / 2) <= 2, "{row}");
        // 矢量：灰色在圆心
        let dots = bright(&s.vectorscope);
        assert!(dots.iter().all(|&(x, y)| x.abs_diff(VECTORSCOPE / 2) <= 1 && y.abs_diff(VECTORSCOPE / 2) <= 1), "{dots:?}");
        // 直方图：只有 128 那一列有柱子
        let cols: std::collections::BTreeSet<u32> =
            bright(&s.histogram).iter().map(|p| p.0).collect();
        assert!(
            cols.iter()
                .all(|&x| (x * 256 / HISTOGRAM.0).abs_diff(128) <= 1),
            "{cols:?}"
        );
    }

    #[test]
    fn saturated_red_moves_off_center_and_up_the_red_axis() {
        let s = compute(&solid(32, 18, [0, 0, 255, 255]));
        let dots = bright(&s.vectorscope);
        assert_ne!(dots, []);
        // 红色：Cr 大（往上），Cb 小（往左）
        assert!(
            dots.iter()
                .all(|&(x, y)| x < VECTORSCOPE / 2 && y < VECTORSCOPE / 2),
            "{dots:?}"
        );
    }
}
