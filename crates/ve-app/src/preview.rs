//! 预览面板：显示引擎交出的帧（按实验结论换帧后回收上上帧的贴图），叠加字幕，播放控制条。

use crate::actions as act;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{IconName, h_flex, v_flex};
use gpui_kit::{
    Bounds, Context, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, RenderImage,
    SharedString, Styled as _, Window, canvas, div, img, px,
};
use std::sync::Arc;
use ve_core::format_timecode;
use ve_export::SubtitleStyle;
use ve_media::VideoFrame;

/// 预览状态。
#[derive(Debug, Default)]
pub struct PreviewState {
    /// 正在显示的帧（`None` = 黑场）。
    pub image: Option<Arc<RenderImage>>,
    /// 上一帧：下一次换帧时才从贴图图集里删（这一帧可能还在屏幕上）。
    retired: Option<Arc<RenderImage>>,
    /// 预览区域（上一帧布局的结果）。
    pub bounds: Bounds<Pixels>,
    /// 显示过的帧数（性能统计）。
    pub frames_shown: u64,
    /// 字幕字体的 em / 行高（见 [`ass_em_ratio`]），第一次渲染时按实际字体度量算出。
    pub em_ratio: Option<f32>,
}

/// BGRA 帧 → 可直接上传的图片（GPUI 的 `RenderImage` 内部就是 BGRA）。
#[must_use]
pub fn to_render_image(frame: &VideoFrame) -> Option<Arc<RenderImage>> {
    let buf = image::RgbaImage::from_raw(frame.width, frame.height, frame.data.clone())?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

/// libass 把字号当成字体整行高（ascent + descent，和 `VSFilter` 一致），GPUI 的字号是 em。
/// 返回 em / 行高（`ascent`、`descent` 是字号 `size` 时的度量，descent 正负都行），夹在 0.5–1。
#[must_use]
pub fn ass_em_ratio(ascent: f32, descent: f32, size: f32) -> f32 {
    let line = ascent + descent.abs();
    if line <= 0.0 {
        1.0
    } else {
        (size / line).clamp(0.5, 1.0)
    }
}

impl Workspace {
    /// 按字幕字体的实际度量算一次 em / 行高。
    pub(crate) fn measure_subtitle_font(&mut self, window: &Window) {
        if self.preview.em_ratio.is_some() {
            return;
        }
        let ts = window.text_system();
        let id = ts.resolve_font(&gpui_kit::font(SubtitleStyle::default().font));
        let size = px(100.0);
        self.preview.em_ratio = Some(ass_em_ratio(
            f32::from(ts.ascent(id, size)),
            f32::from(ts.descent(id, size)),
            100.0,
        ));
    }
}

/// 画面在区域里等比居中的位置。
#[must_use]
pub fn fit_rect(area: Bounds<Pixels>, width: u32, height: u32) -> Bounds<Pixels> {
    let (aw, ah) = (f32::from(area.size.width), f32::from(area.size.height));
    if width == 0 || height == 0 || aw <= 0.0 || ah <= 0.0 {
        return area;
    }
    #[expect(clippy::cast_precision_loss, reason = "画面尺寸远小于 2^23")]
    let (w, h) = (width as f32, height as f32);
    let scale = (aw / w).min(ah / h);
    let (fw, fh) = (w * scale, h * scale);
    Bounds::new(
        gpui_kit::point(
            area.origin.x + px((aw - fw) / 2.0),
            area.origin.y + px((ah - fh) / 2.0),
        ),
        gpui_kit::size(px(fw), px(fh)),
    )
}

impl Workspace {
    /// 从引擎取新帧（每次渲染调用）。
    pub(crate) fn pull_frame(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(update) = self.engine.take_frame() else {
            return;
        };
        let next = update.frame.as_deref().and_then(to_render_image);
        if let Some(old) = self.preview.retired.take() {
            cx.drop_image(old, Some(window));
        }
        self.preview.retired = std::mem::replace(&mut self.preview.image, next);
        self.preview.frames_shown += 1;
    }

    pub(crate) fn render_preview(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let settings = self.doc.project.settings;
        let video = fit_rect(self.preview.bounds, settings.width, settings.height);
        let local = |b: Bounds<Pixels>| {
            (
                b.origin.x - self.preview.bounds.origin.x,
                b.origin.y - self.preview.bounds.origin.y,
            )
        };
        let (vx, vy) = local(video);
        let project = Arc::clone(&self.project);
        let subtitles = ve_engine::resolve_subtitles(&project, self.playhead);
        let style = SubtitleStyle::default();
        // 预览里的字号、描边按画面实际显示高度换算，和导出一致
        let shown_h = f32::from(video.size.height);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "显示高度为正且很小"
        )]
        let shown_h_px = shown_h.max(1.0) as u32;
        // ASS 字号是整行高：行高就用它，字号换成 GPUI 的 em，和导出的字一样大、离底一样远
        #[expect(clippy::cast_possible_truncation, reason = "字号很小")]
        let line = px(style.font_px(shown_h_px) as f32);
        let font = line * self.preview.em_ratio.unwrap_or(1.0);
        #[expect(clippy::cast_possible_truncation, reason = "描边很小")]
        let outline = (style.outline_px(shown_h_px) as f32).max(1.0);
        #[expect(clippy::cast_possible_truncation, reason = "边距很小")]
        let margin = px(style.margin_px(shown_h_px) as f32);

        let this = cx.entity();
        let frame = div()
            .id("preview")
            .test_support()
            .flex_1()
            .relative()
            .overflow_hidden()
            .bg(palette::preview_bg())
            .child(
                canvas(
                    move |bounds, _, cx| this.update(cx, |w, _| w.preview.bounds = bounds),
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(vx)
                    .top(vy)
                    .w(video.size.width)
                    .h(video.size.height)
                    .bg(gpui_kit::black())
                    .children(
                        self.preview
                            .image
                            .clone()
                            .map(|image| img(image).size_full()),
                    ),
            )
            .children(subtitles.into_iter().map(|text| {
                subtitle_overlay(text, font, line, outline, &style.font)
                    .absolute()
                    .left(vx)
                    .w(video.size.width)
                    // 底边对齐、向上长（和 libass 一致），多行时不会掉出画面
                    .bottom(self.preview.bounds.size.height - (vy + video.size.height) + margin)
            }));

        v_flex()
            .size_full()
            .child(frame)
            .child(self.render_transport(cx))
    }

    fn render_transport(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let rate = self.doc.project.settings.frame_rate;
        let timecode = format!(
            "{} / {}",
            format_timecode(self.playhead, rate),
            format_timecode(self.doc.project.timeline.duration(), rate)
        );
        let playing = self.engine.is_playing();
        h_flex()
            .id("transport")
            .test_support()
            .h(px(40.))
            .px_2()
            .gap_1()
            .justify_center()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("tr-start")
                    .ghost()
                    .compact()
                    .label("|◀")
                    .on_click(cx.listener(|this, _, _, cx| this.seek(ve_core::Time::ZERO, cx))),
            )
            .child(
                Button::new("tr-back")
                    .ghost()
                    .compact()
                    .label("◀|")
                    .on_click(cx.listener(|_, _, w, cx| {
                        w.dispatch_action(Box::new(act::StepBack), cx);
                    })),
            )
            .child(
                Button::new("tr-play")
                    .primary()
                    .compact()
                    .icon(if playing {
                        IconName::Pause
                    } else {
                        IconName::Play
                    })
                    .on_click(
                        cx.listener(|_, _, w, cx| w.dispatch_action(Box::new(act::PlayPause), cx)),
                    ),
            )
            .child(
                Button::new("tr-fwd")
                    .ghost()
                    .compact()
                    .label("|▶")
                    .on_click(cx.listener(|_, _, w, cx| {
                        w.dispatch_action(Box::new(act::StepForward), cx);
                    })),
            )
            .child(
                div()
                    .id("timecode")
                    .ml_3()
                    .font_family("Consolas")
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(timecode)),
            )
    }
}

/// 白字黑描边：8 个方向偏移的黑字垫底 + 白字。
fn subtitle_overlay(
    text: &str,
    font: Pixels,
    line: Pixels,
    outline: f32,
    family: &str,
) -> gpui_kit::Div {
    let family: SharedString = family.to_owned().into();
    let text: SharedString = text.to_owned().into();
    let styled = |d: gpui_kit::Div, color: gpui_kit::Hsla| {
        d.w_full()
            .text_center()
            .text_size(font)
            .line_height(line)
            .font_family(family.clone())
            .text_color(color)
            .child(text.clone())
    };
    // 白字在文档流里，决定高度；8 个方向偏移的黑字叠在下面作描边
    let mut root = div().relative();
    for (dx, dy) in [
        (-1.0, -1.0),
        (0.0, -1.0),
        (1.0, -1.0),
        (-1.0, 0.0),
        (1.0, 0.0),
        (-1.0, 1.0),
        (0.0, 1.0),
        (1.0, 1.0),
    ] {
        root = root.child(styled(
            div()
                .absolute()
                .left(px(dx * outline))
                .top(px(dy * outline)),
            gpui_kit::black(),
        ));
    }
    root.child(
        styled(div(), gpui_kit::white())
            .id("subtitle-main")
            .test_support(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ass_font_size_is_line_height_not_em() {
        // libass 把 Fontsize 当成整行高（ascent + descent），GPUI 的字号是 em：
        // ascent 0.9em、descent 0.3em 的字体，ASS 的 100 号字只有 em 83.3
        assert!((ass_em_ratio(90.0, -30.0, 100.0) - 0.8333).abs() < 1e-3);
        assert!((ass_em_ratio(90.0, 30.0, 100.0) - 0.8333).abs() < 1e-3);
        // 度量异常时不放大、也不缩得离谱
        assert!((ass_em_ratio(0.0, 0.0, 100.0) - 1.0).abs() < 1e-6);
        assert!((ass_em_ratio(500.0, 500.0, 100.0) - 0.5).abs() < 1e-6);
    }
}
