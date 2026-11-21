//! 预览面板：显示引擎交出的帧（按实验结论换帧后回收上上帧的贴图），叠加字幕，播放控制条。

use crate::actions as act;
use crate::chrome;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::StatefulInteractiveElement as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Bounds, Context, InteractiveElement as _, IntoElement, MouseButton, MouseUpEvent,
    ParentElement as _, Pixels, RenderImage, SharedString, Styled as _, Window, canvas, div, img,
    px,
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
    /// 相邻两次换帧间隔超过 25 ms 的次数（性能统计：掉帧）。
    pub long_gaps: u64,
    /// 上一次换帧的时刻。
    last_shown: Option<std::time::Instant>,
    /// 字幕字体的 em / 行高（见 [`ass_em_ratio`]）和量的是哪个字体；换了字体就重新量。
    pub em_ratio: Option<f32>,
    /// `em_ratio` 量的字体。
    pub em_font: String,
    /// 正在预览上拖动的图层。
    pub(crate) drag: Option<crate::preview_edit::PreviewDrag>,
    /// 正在预览上拖动的蒙版（属性面板在「蒙版」时）。
    pub(crate) mask_drag: Option<crate::preview_mask::MaskDrag>,
    /// 鼠标样式（悬停 / 拖动时按位置变）。
    pub(crate) cursor: gpui_kit::CursorStyle,
    /// 底栏「比例」菜单是否展开。
    pub(crate) ratio_menu: bool,
    /// 底栏「画质」菜单是否展开。
    pub(crate) quality_menu: bool,
    /// 预览画质（剪映「原画 / 清晰」）。
    pub(crate) quality: Quality,
    /// 全屏预览（剪映的 ⤢）：整个窗口只显示画面，没有控件。
    pub(crate) fullscreen: bool,
    /// 正在显示的那一帧的像素（取色器用）。
    pub(crate) last_frame: Option<Arc<VideoFrame>>,
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
        let family = self.doc.project.subtitle_style.font.clone();
        if self.preview.em_ratio.is_some() && self.preview.em_font == family {
            return;
        }
        let ts = window.text_system();
        let id = ts.resolve_font(&gpui_kit::font(family.clone()));
        self.preview.em_font = family;
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
    /// 播放器上有没有画面（取色器要从这一帧取）。
    #[must_use]
    pub const fn showing_frame(&self) -> bool {
        self.preview.last_frame.is_some()
    }

    pub(crate) fn pull_frame(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(update) = self.engine.take_frame() else {
            return;
        };
        let next = update.frame.as_deref().and_then(to_render_image);
        self.preview.last_frame.clone_from(&update.frame);
        if let Some(old) = self.preview.retired.take() {
            cx.drop_image(old, Some(window));
        }
        self.preview.retired = std::mem::replace(&mut self.preview.image, next);
        self.preview.frames_shown += 1;
        if self.scopes.on {
            self.refresh_scopes(window, cx);
        }
        let now = std::time::Instant::now();
        if self
            .preview
            .last_shown
            .replace(now)
            .is_some_and(|prev| now - prev > std::time::Duration::from_millis(25))
        {
            self.preview.long_gaps += 1;
        }
    }

    pub(crate) fn render_preview(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(self.render_player_header(cx))
            .child(self.render_preview_frame(cx))
            .child(self.render_transport(cx))
    }

    /// 画面上的字幕（导出烧进去的那些，预览里用文字画）：字号、描边、离底距离按画面实际显示高度换算，
    /// 和导出一致；入场 / 出场动画按这一刻的样子画。
    fn subtitle_layers(&self, video: Bounds<Pixels>) -> Vec<gpui_kit::AnyElement> {
        let (vx, vy) = (
            video.origin.x - self.preview.bounds.origin.x,
            video.origin.y - self.preview.bounds.origin.y,
        );
        let project = Arc::clone(&self.engine_project);
        let subtitles = ve_engine::resolve_subtitle_shows(&project, self.playhead);
        let shown_h = f32::from(video.size.height);
        let shown_w = f32::from(video.size.width);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "显示高度为正且很小"
        )]
        let shown_h_px = shown_h.max(1.0) as u32;
        subtitles
            .into_iter()
            .map(|show| {
                let style = show.style;
                let state = show.state;
                // ASS 字号是整行高：行高就用它，字号换成 GPUI 的 em，和导出的字一样大、离底一样远
                #[expect(clippy::cast_possible_truncation, reason = "字号很小")]
                let line = px(style.font_px(shown_h_px) as f32);
                let font = line * self.preview.em_ratio.unwrap_or(1.0);
                // 描边为 0 时不画；有描边时至少 1 像素，缩小的预览里也看得见
                #[expect(clippy::cast_possible_truncation, reason = "描边很小")]
                let outline = if style.outline > 0.0 {
                    (style.outline_px(shown_h_px) as f32).max(1.0)
                } else {
                    0.0
                };
                #[expect(clippy::cast_possible_truncation, reason = "边距很小")]
                let margin = px(style.margin_px(shown_h_px) as f32);
                let bottom = self.preview.bounds.size.height - (vy + video.size.height) + margin;
                // 缩放以底边中点为中心（和 ASS 的 \an2 + \fscx 一致）、位置偏移、透明度
                #[expect(clippy::cast_possible_truncation, reason = "几十像素")]
                let metrics = TextMetrics {
                    outline: outline * state.scale,
                    spacing: style.spacing_px(shown_h_px) as f32 * state.scale,
                    shadow: if style.shadow > 0.0 {
                        (style.shadow_px(shown_h_px) as f32).max(1.0) * state.scale
                    } else {
                        0.0
                    },
                    padding: style.padding_px(shown_h_px) as f32 * state.scale,
                };
                subtitle_overlay(
                    show.text,
                    font * state.scale,
                    line * state.scale,
                    metrics,
                    style,
                    show.chars.as_deref(),
                )
                .absolute()
                .left(vx + px(state.x * shown_w))
                .w(video.size.width)
                .opacity(state.alpha)
                // 底边对齐、向上长（和 libass 一致），多行时不会掉出画面
                .bottom(bottom - px(state.y * shown_h))
                .into_any_element()
            })
            .collect()
    }

    /// 画面区：画面、字幕、选中框。全屏时只画它（不能拖改画面、没有选中框，和剪映一样）。
    pub(crate) fn render_preview_frame(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let settings = self.doc.project.settings;
        let video = fit_rect(self.preview.bounds, settings.width, settings.height);
        let local = |b: Bounds<Pixels>| {
            (
                b.origin.x - self.preview.bounds.origin.x,
                b.origin.y - self.preview.bounds.origin.y,
            )
        };
        let (vx, vy) = local(video);
        let this = cx.entity();
        div()
            .id("preview")
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| this.preview_hover(*hovered, cx)))
            .test_support()
            .flex_1()
            .relative()
            .overflow_hidden()
            .bg(palette::preview_bg())
            .when(!self.preview.fullscreen, |frame| {
                frame
                    .cursor(self.preview.cursor)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::preview_mouse_down))
                    .on_mouse_move(cx.listener(Self::preview_mouse_move))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, cx| this.preview_mouse_up(cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, cx| this.preview_mouse_up(cx)),
                    )
            })
            .child(
                canvas(
                    move |bounds, window, cx| {
                        // 布局量出的新尺寸这一帧才拿到，画面已经按旧尺寸摆好了：尺寸变了就再画一帧
                        // （改窗口大小、拖分隔条后停着不动时，画面否则会一直偏在一边）
                        let changed = this.update(cx, |w, _| {
                            std::mem::replace(&mut w.preview.bounds, bounds) != bounds
                        });
                        if changed {
                            window.request_animation_frame();
                        }
                    },
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
            .children(self.subtitle_layers(video))
            .when(!self.preview.fullscreen, |frame| {
                frame.children(self.render_selection_box(cx))
            })
            .children(self.render_scopes())
    }

    /// 播放器底栏（剪映式）：左边当前时间（青）/ 总长，中间播放键，右边比例和全屏。
    fn render_transport(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let rate = self.doc.project.settings.frame_rate;
        let playing = self.engine.is_playing();
        h_flex()
            .id("transport")
            .test_support()
            .relative()
            .h(px(40.))
            .flex_none()
            .px_3()
            .gap_2()
            .child(
                h_flex()
                    .id("timecode")
                    .gap_2()
                    .font_family("Consolas")
                    .text_xs()
                    .child(
                        div()
                            .text_color(palette::accent())
                            .child(SharedString::from(format_timecode(self.playhead, rate))),
                    )
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(format_timecode(
                                self.doc.project.timeline.duration(),
                                rate,
                            ))),
                    )
                    .children(shuttle_label(playing, self.engine.rate()).map(|label| {
                        div()
                            .id("tr-rate")
                            .test_support()
                            .text_color(palette::accent())
                            .child(label)
                    })),
            )
            .child(
                // 播放键在整条底栏里居中
                h_flex()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .justify_center()
                    .child(
                        Button::new("tr-play")
                            .ghost()
                            .icon(if playing {
                                IconName::Pause
                            } else {
                                IconName::Play
                            })
                            .tooltip("播放 / 暂停（空格）")
                            .on_click(cx.listener(|_, _, w, cx| {
                                w.dispatch_action(Box::new(act::PlayPause), cx);
                            })),
                    ),
            )
            .child(div().flex_1())
            .child(self.render_quality_button(cx))
            .child(self.render_ratio_button(cx))
            .child(
                Button::new("tr-fullscreen")
                    .ghost()
                    .compact()
                    .icon(Icon::new(Lucide::Maximize))
                    .tooltip("全屏预览（Ctrl+Shift+F，Esc 退出）")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_fullscreen(true, window, cx);
                    })),
            )
    }
}

/// 预览画质（剪映播放器底栏的「原画」）：只影响预览，导出总是原画。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// 原画质：保持原片清晰度（按原片最多 4K 解码）。
    #[default]
    Original,
    /// 清晰画质：播放流畅，不影响导出质量（按 720P 解码）。
    Smooth,
}

impl Quality {
    /// 两档：（元素编号，名字，说明，画质）。
    pub const ALL: [(&'static str, &'static str, &'static str, Self); 2] = [
        (
            "quality-original",
            "原画质",
            "保持原片清晰度",
            Self::Original,
        ),
        (
            "quality-smooth",
            "清晰画质",
            "播放流畅，不影响导出质量",
            Self::Smooth,
        ),
    ];

    /// 预览解码尺寸上限。
    #[must_use]
    pub const fn target(self) -> ve_media::ScaleTarget {
        match self {
            Self::Original => ve_media::ScaleTarget {
                max_width: 3840,
                max_height: 3840,
            },
            Self::Smooth => ve_media::ScaleTarget {
                max_width: 1280,
                max_height: 1280,
            },
        }
    }

    /// 底栏按钮上的字。
    #[must_use]
    pub const fn short(self) -> &'static str {
        match self {
            Self::Original => "原画",
            Self::Smooth => "清晰",
        }
    }
}

/// 比例选项：（元素编号，名字，比例；`None` 是「原始」）。
pub type RatioOption = (&'static str, &'static str, Option<(u32, u32)>);

/// 比例菜单里的选项。
pub const RATIOS: [RatioOption; 8] = [
    ("ratio-original", "原始", None),
    ("ratio-16-9", "16:9", Some((16, 9))),
    ("ratio-9-16", "9:16", Some((9, 16))),
    ("ratio-4-3", "4:3", Some((4, 3))),
    ("ratio-3-4", "3:4", Some((3, 4))),
    ("ratio-1-1", "1:1", Some((1, 1))),
    ("ratio-21-9", "21:9", Some((21, 9))),
    ("ratio-235-100", "2.35:1", Some((235, 100))),
];

impl Workspace {
    /// 切换预览画质。
    pub(crate) fn set_quality(&mut self, q: Quality, cx: &mut Context<'_, Self>) {
        self.preview.quality = q;
        self.preview.quality_menu = false;
        self.engine.set_preview_max(q.target());
        cx.notify();
    }

    /// 底栏的「原画 ▾」按钮和向上弹出的两档画质（剪映）。
    fn render_quality_button(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let current = self.preview.quality;
        div()
            .relative()
            .child(
                Button::new("tr-quality")
                    .ghost()
                    .compact()
                    .label(format!("{} ▾", current.short()))
                    .tooltip("预览画质（只影响预览，不影响导出）")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.preview.quality_menu = !this.preview.quality_menu;
                        cx.notify();
                    })),
            )
            .children(self.preview.quality_menu.then(|| {
                let muted = cx.theme().muted_foreground;
                let items = Quality::ALL.map(|(id, label, note, q)| {
                    h_flex()
                        .id(id)
                        .test_support()
                        .py_1()
                        .px_2()
                        .gap_2()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|st| st.bg(palette::menu_hover()))
                        .on_click(cx.listener(move |this, _, _, cx| this.set_quality(q, cx)))
                        .child(
                            div()
                                .w(px(12.))
                                .text_color(palette::accent())
                                .child(if q == current { "✓" } else { "" }),
                        )
                        .child(
                            v_flex()
                                .child(div().text_sm().child(label))
                                .child(div().text_xs().text_color(muted).child(note)),
                        )
                });
                gpui_kit::deferred(
                    v_flex()
                        .id("quality-menu")
            // 盖住下面的东西：点菜单不会顺带点到下面的画面 / 时间轴
            .occlude()
                        .test_support()
                        .absolute()
                        .bottom(px(30.))
                        .right_0()
                        .w(px(200.))
                        .p_1()
                        .bg(palette::menu_bg())
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(px(chrome::CARD_RADIUS))
                        .shadow_lg()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.preview.quality_menu = false;
                            cx.notify();
                        }))
                        .children(items),
                )
                .with_priority(1)
            }))
    }

    /// 底栏的「比例」按钮和向上弹出的菜单（剪映：横竖屏一键切换，画面自动适配）。
    fn render_ratio_button(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let s = self.doc.project.settings;
        let current = aspect_label(s.width, s.height);
        div()
            .relative()
            .child(
                Button::new("tr-ratio")
                    .ghost()
                    .compact()
                    .label(SharedString::from(current.clone()))
                    .tooltip("画面比例：换比例时保持短边，画面自动适配")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.preview.ratio_menu = !this.preview.ratio_menu;
                        cx.notify();
                    })),
            )
            .children(self.preview.ratio_menu.then(|| {
                let items = RATIOS.iter().map(|&(id, label, ratio)| {
                    let on = ratio.is_some() && label == current;
                    h_flex()
                        .id(id)
                        .test_support()
                        .h(px(26.))
                        .px_2()
                        .gap_2()
                        .rounded(px(4.))
                        .text_sm()
                        .cursor_pointer()
                        .hover(|st| st.bg(palette::menu_hover()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.preview.ratio_menu = false;
                            this.apply(window, cx, |p| ve_core::edit::set_aspect(p, ratio));
                        }))
                        .child(div().w(px(12.)).text_color(palette::accent()).child(if on {
                            "✓"
                        } else {
                            ""
                        }))
                        .child(label)
                });
                gpui_kit::deferred(
                    v_flex()
                        .id("ratio-menu")
            // 盖住下面的东西：点菜单不会顺带点到下面的画面 / 时间轴
            .occlude()
                        .test_support()
                        .absolute()
                        .bottom(px(30.))
                        .right_0()
                        .w(px(120.))
                        .p_1()
                        .bg(palette::menu_bg())
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(px(chrome::CARD_RADIUS))
                        .shadow_lg()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.preview.ratio_menu = false;
                            cx.notify();
                        }))
                        .children(items),
                )
                .with_priority(1)
            }))
    }
}

/// 画面比例的叫法：常见的写成「16:9」这样，其他的约分后写出来。
#[must_use]
pub fn aspect_label(width: u32, height: u32) -> String {
    const fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    if width == 0 || height == 0 {
        return "—".to_owned();
    }
    let g = gcd(width, height);
    let (w, h) = (width / g, height / g);
    // 1366×768 这类近似 16:9 的也按 16:9 叫
    let r = f64::from(width) / f64::from(height);
    for (name, v) in [
        ("16:9", 16.0 / 9.0),
        ("9:16", 9.0 / 16.0),
        ("4:3", 4.0 / 3.0),
        ("3:4", 3.0 / 4.0),
        ("1:1", 1.0),
        ("21:9", 21.0 / 9.0),
        ("2.35:1", 2.35),
    ] {
        if (r - v).abs() < 0.01 {
            return name.to_owned();
        }
    }
    format!("{w}:{h}")
}

/// 预览里画字幕要的几个像素量（已经按显示大小和动画缩放算好）。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TextMetrics {
    /// 描边宽度。
    pub outline: f32,
    /// 字间距。
    pub spacing: f32,
    /// 阴影往右下偏多远。
    pub shadow: f32,
    /// 背景框比字多出的边。
    pub padding: f32,
}

/// 字幕：描边色的字在 8 个方向偏移垫底作描边，阴影色的字再整体往右下偏作阴影；
/// 有背景框时不描边，字底下一块背景（阴影变成框的影子），和 libass 的 `BorderStyle=3` 一样。
#[expect(
    clippy::too_many_lines,
    reason = "阴影、描边、背景框照着 libass 一层层叠，拆开反而难对照导出"
)]
fn subtitle_overlay(
    text: &str,
    font: Pixels,
    line: Pixels,
    m: TextMetrics,
    style: &SubtitleStyle,
    chars: Option<&[(f32, f32)]>,
) -> gpui_kit::Div {
    let family: SharedString = style.font.clone().into();
    let shared: SharedString = text.to_owned().into();
    let weight = if style.bold {
        gpui_kit::FontWeight::BOLD
    } else {
        gpui_kit::FontWeight::NORMAL
    };
    let boxed = style.has_background();
    // 有字间距时也按一个字一个格子排（GPUI 的文字没有字间距）
    let all_shown: Vec<(f32, f32)> = if m.spacing > 0.0 && chars.is_none() {
        vec![(1.0, 1.0); text.chars().count()]
    } else {
        Vec::new()
    };
    let reveal = chars.or_else(|| (!all_shown.is_empty()).then_some(all_shown.as_slice()));
    // 逐字入场：每个字一个格子，各自的不透明度和缩放（底边对齐，和 ASS 的逐字 \alpha、\fscx 一样）
    let per_char = |color: gpui_kit::Hsla, reveal: &[(f32, f32)]| {
        let mut rows = v_flex().items_center();
        if !boxed {
            rows = rows.w_full();
        }
        let mut k = 0;
        for row_text in text.split('\n') {
            let mut row = h_flex().items_end().justify_center().gap(px(m.spacing));
            for ch in row_text.chars() {
                let (a, s) = reveal.get(k).copied().unwrap_or((1.0, 1.0));
                k += 1;
                row = row.child(
                    div()
                        .text_size(font * s)
                        .line_height(line)
                        .font_family(family.clone())
                        .font_weight(weight)
                        .text_color(color.opacity(a))
                        .child(SharedString::from(ch.to_string())),
                );
            }
            rows = rows.child(row);
        }
        rows
    };
    let styled = |d: gpui_kit::Div, color: gpui_kit::Hsla| {
        let mut d = if boxed { d } else { d.w_full() };
        if style.italic {
            d = d.italic();
        }
        if style.underline {
            d = d.underline();
        }
        match reveal {
            Some(reveal) => d.child(per_char(color, reveal)),
            None => d
                .text_center()
                .text_size(font)
                .line_height(line)
                .font_family(family.clone())
                .font_weight(weight)
                .text_color(color)
                .child(shared.clone()),
        }
    };
    let color = |rgb: u32, opacity: f32| -> gpui_kit::Hsla {
        let c: gpui_kit::Hsla = gpui_kit::rgb(rgb).into();
        c.opacity(opacity)
    };
    let fill = color(style.color, style.opacity);
    if boxed {
        let shade = color(style.shadow_color, style.shadow_opacity);
        let back = color(style.background_color, style.background_opacity);
        let shadow = (m.shadow > 0.0).then(|| {
            div()
                .absolute()
                .left(px(m.shadow))
                .top(px(m.shadow))
                .size_full()
                .bg(shade)
        });
        let body = styled(div(), fill)
            .id("subtitle-main")
            .test_support()
            .px(px(m.padding))
            .py(px(m.padding))
            .bg(back);
        return h_flex()
            .w_full()
            .justify_center()
            .child(div().relative().children(shadow).child(body));
    }
    let outline_copies: u8 = if m.outline > 0.0 { 8 } else { 0 };
    let edge = color(
        style.outline_color,
        stacked_alpha(style.opacity, outline_copies),
    );
    let shade = color(
        style.shadow_color,
        stacked_alpha(style.shadow_opacity, outline_copies + 1),
    );
    // 正文在文档流里，决定高度；描边色、阴影色的字叠在下面（为 0 时不画）
    let mut root = div().relative();
    let ring: &[(f32, f32)] = &[
        (-1.0, -1.0),
        (0.0, -1.0),
        (1.0, -1.0),
        (-1.0, 0.0),
        (1.0, 0.0),
        (-1.0, 1.0),
        (0.0, 1.0),
        (1.0, 1.0),
    ];
    let outline: &[(f32, f32)] = if m.outline > 0.0 { ring } else { &[] };
    if m.shadow > 0.0 {
        // 阴影是带描边的字整体的影子
        for &(dx, dy) in std::iter::once(&(0.0, 0.0)).chain(outline) {
            root = root.child(styled(
                div()
                    .absolute()
                    .left(px(dx.mul_add(m.outline, m.shadow)))
                    .top(px(dy.mul_add(m.outline, m.shadow))),
                shade,
            ));
        }
    }
    for &(dx, dy) in outline {
        root = root.child(styled(
            div()
                .absolute()
                .left(px(dx * m.outline))
                .top(px(dy * m.outline)),
            edge,
        ));
    }
    root.child(styled(div(), fill).id("subtitle-main").test_support())
}

/// 描边、阴影是同一个字偏移着叠很多份画出来的；GPUI 的透明度逐份相乘，重叠处会越叠越实。
/// 每份用这个透明度，`copies` 份叠满的地方合起来正好是 `alpha`（和 libass 整块一个透明度一致，
/// 只有边缘叠得少的地方略淡）。
fn stacked_alpha(alpha: f32, copies: u8) -> f32 {
    if copies <= 1 || alpha >= 1.0 {
        return alpha;
    }
    1.0 - (1.0 - alpha.max(0.0)).powf(1.0 / f32::from(copies))
}

/// 倍速 / 倒放时时间码旁边的提示（原速、暂停时不显示）。
fn shuttle_label(playing: bool, rate: f32) -> Option<SharedString> {
    if !playing || (rate - 1.0).abs() < f32::EPSILON {
        return None;
    }
    Some(if rate < 0.0 {
        format!("◀ {}x", -rate).into()
    } else {
        format!("{rate}x").into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacked_copies_add_up_to_the_set_opacity() {
        for (a, k) in [(0.6_f32, 9_u8), (0.5, 8), (1.0, 8), (0.0, 9), (0.3, 1)] {
            let each = stacked_alpha(a, k);
            // k 份叠在一起：1 - (1 - each)^k
            let total = 1.0 - (1.0 - each).powi(i32::from(k));
            assert!((total - a).abs() < 1e-4, "{a} × {k}: {total}");
        }
    }

    #[test]
    fn aspect_labels_use_common_names() {
        assert_eq!(aspect_label(1920, 1080), "16:9");
        assert_eq!(aspect_label(1080, 1920), "9:16");
        assert_eq!(aspect_label(1366, 768), "16:9", "近似 16:9 也叫 16:9");
        assert_eq!(aspect_label(1000, 1000), "1:1");
        assert_eq!(aspect_label(1440, 1080), "4:3");
        assert_eq!(aspect_label(1000, 700), "10:7");
        assert_eq!(aspect_label(0, 1080), "—");
    }

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
