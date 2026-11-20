//! 属性面板「调整」的进阶调色（剪映）：二级分段「基础 / HSL / 曲线 / 色轮」。
//!
//! - 基础：原来的五个滑条，下面是 LUT（导入 .cube、强度）。
//! - HSL：八个色环选色相，色相 / 饱和度 / 亮度三个滑条。
//! - 曲线：亮度、红、绿、蓝四条；点一下加点、拖点改值，右键点删掉。
//! - 色轮：暗部、中灰、亮部、偏移四个轮，拖轮子里的点选颜色倾向，下面的滑条调明暗。

use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, PathPromptOptions, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, canvas, div,
    fill, point, prelude::FluentBuilder as _, px, rgb, size,
};
use ve_core::{ClipId, Grade, HslBand, LutRef};

/// 「调整」标签的二级分段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AdjustSeg {
    #[default]
    Basic,
    Hsl,
    Curves,
    Wheels,
}

/// 曲线的通道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum CurveChannel {
    #[default]
    Luma,
    Red,
    Green,
    Blue,
}

impl CurveChannel {
    const ALL: [(Self, &'static str, &'static str); 4] = [
        (Self::Luma, "curve-ch-luma", "亮度"),
        (Self::Red, "curve-ch-red", "红"),
        (Self::Green, "curve-ch-green", "绿"),
        (Self::Blue, "curve-ch-blue", "蓝"),
    ];

    const fn points(self, g: &Grade) -> &Vec<(f32, f32)> {
        match self {
            Self::Luma => &g.curves.luma,
            Self::Red => &g.curves.red,
            Self::Green => &g.curves.green,
            Self::Blue => &g.curves.blue,
        }
    }

    const fn points_mut(self, g: &mut Grade) -> &mut Vec<(f32, f32)> {
        match self {
            Self::Luma => &mut g.curves.luma,
            Self::Red => &mut g.curves.red,
            Self::Green => &mut g.curves.green,
            Self::Blue => &mut g.curves.blue,
        }
    }

    const fn color(self) -> u32 {
        match self {
            Self::Luma => 0xE0_E0E0,
            Self::Red => 0xE5_484D,
            Self::Green => 0x46_A758,
            Self::Blue => 0x3E_63DD,
        }
    }
}

/// 色轮的四个轮子。
const WHEELS: [(&str, &str); 4] = [
    ("wheel-shadows", "暗部"),
    ("wheel-midtones", "中灰"),
    ("wheel-highlights", "亮部"),
    ("wheel-offset", "偏移"),
];

/// 曲线编辑区的边长、色轮的直径（像素）。
const CURVE_SIZE: f32 = 220.0;
const WHEEL_SIZE: f32 = 96.0;

/// 进阶调色的控件。
#[derive(Debug)]
pub(crate) struct GradeFields {
    pub(crate) seg: AdjustSeg,
    band: HslBand,
    /// HSL：色相、饱和度、亮度。
    hsl: [Entity<SliderState>; 3],
    lut_intensity: Entity<SliderState>,
    /// 色轮下面的明暗滑条。
    wheel_luma: [Entity<SliderState>; 4],
    channel: CurveChannel,
    /// 正在拖的曲线点（下标）。
    dragging_point: Option<usize>,
    /// 正在拖的色轮（下标）。
    dragging_wheel: Option<usize>,
    gesture: bool,
    curve_bounds: Bounds<Pixels>,
    wheel_bounds: [Bounds<Pixels>; 4],
}

/// 滑条订阅回调用来区分是哪个滑条。
#[derive(Debug, Clone, Copy)]
enum GradeSlider {
    Hsl(usize),
    Lut,
    WheelLuma(usize),
}

fn slider(cx: &mut Context<'_, Workspace>, min: f32, max: f32, value: f32) -> Entity<SliderState> {
    cx.new(|_| {
        SliderState::new()
            .min(min)
            .max(max)
            .step(1.0)
            .default_value(value)
    })
}

impl GradeFields {
    pub(crate) fn new(
        window: &Window,
        cx: &mut Context<'_, Workspace>,
    ) -> (Self, Vec<Subscription>) {
        let hsl = [0, 1, 2].map(|_| slider(cx, -100.0, 100.0, 0.0));
        let lut_intensity = slider(cx, 0.0, 100.0, 100.0);
        let wheel_luma = [0, 1, 2, 3].map(|_| slider(cx, -100.0, 100.0, 0.0));
        let mut subs = Vec::new();
        let mut sub =
            |s: &Entity<SliderState>, which: GradeSlider, cx: &mut Context<'_, Workspace>| {
                subs.push(cx.subscribe_in(
                    s,
                    window,
                    move |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                        this.on_grade_slider(which, e, window, cx);
                    },
                ));
            };
        for (i, s) in hsl.iter().enumerate() {
            sub(s, GradeSlider::Hsl(i), cx);
        }
        sub(&lut_intensity, GradeSlider::Lut, cx);
        for (i, s) in wheel_luma.iter().enumerate() {
            sub(s, GradeSlider::WheelLuma(i), cx);
        }
        (
            Self {
                seg: AdjustSeg::default(),
                band: HslBand::Red,
                hsl,
                lut_intensity,
                wheel_luma,
                channel: CurveChannel::default(),
                dragging_point: None,
                dragging_wheel: None,
                gesture: false,
                curve_bounds: Bounds::default(),
                wheel_bounds: [Bounds::default(); 4],
            },
            subs,
        )
    }
}

/// 色轮一个轮子的值（RGB 偏移）。
const fn wheel_value(g: &Grade, i: usize) -> [f32; 3] {
    let w = &g.wheels;
    [w.shadows, w.midtones, w.highlights, w.offset][i]
}

const fn wheel_value_mut(g: &mut Grade, i: usize) -> &mut [f32; 3] {
    let w = &mut g.wheels;
    match i {
        0 => &mut w.shadows,
        1 => &mut w.midtones,
        2 => &mut w.highlights,
        _ => &mut w.offset,
    }
}

/// RGB 偏移拆成（轮上的位置 x、y：-1–1，明暗）。三个通道的平均是明暗，剩下的是颜色倾向。
fn wheel_split(v: [f32; 3]) -> (f32, f32, f32) {
    let luma = (v[0] + v[1] + v[2]) / 3.0;
    let c = v.map(|x| x - luma);
    // 红在 0°、绿在 120°、蓝在 240°（屏幕上 y 向下，所以 y 取反）
    let x = c[0] - f32::midpoint(c[1], c[2]);
    let y = -(c[1] - c[2]) * 3f32.sqrt() / 2.0;
    (x, y, luma)
}

/// 反过来：轮上的位置和明暗 → RGB 偏移。
fn wheel_join(x: f32, y: f32, luma: f32) -> [f32; 3] {
    let s3 = 3f32.sqrt();
    let c = [2.0 * x / 3.0, -x / 3.0 - y / s3, -x / 3.0 + y / s3];
    c.map(|v| (v + luma).clamp(-1.0, 1.0))
}

impl Workspace {
    /// 选中的视频片段（或调节片段）和它的进阶调色（没调时是默认值）。
    fn bound_grade(&self) -> Option<(ClipId, Grade)> {
        let clip = self.inspector.bound?;
        let (track, c) = self.doc.project.timeline.find_clip(clip)?;
        let video = self.doc.project.timeline.track(track)?.kind == ve_core::TrackKind::Video;
        video.then(|| (clip, c.grade.as_deref().cloned().unwrap_or_default()))
    }

    /// 改进阶调色：在拖动手势里时并进同一步撤销，否则单独一步。
    fn edit_grade(
        &mut self,
        f: impl FnOnce(&mut Grade),
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((clip, mut g)) = self.bound_grade() else {
            return;
        };
        f(&mut g);
        if self.inspector.grade.gesture {
            let _ = self
                .doc
                .edit(|p| ve_core::edit::set_grade(p, clip, Some(g)));
            self.after_change(window, cx);
        } else {
            self.apply(window, cx, |p| ve_core::edit::set_grade(p, clip, Some(g)));
        }
    }

    fn begin_grade_gesture(&mut self) {
        if !self.inspector.grade.gesture {
            self.doc.begin_gesture();
            self.inspector.grade.gesture = true;
        }
    }

    fn end_grade_gesture(&mut self) {
        if self.inspector.grade.gesture {
            self.doc.end_gesture();
            self.inspector.grade.gesture = false;
        }
        self.inspector.grade.dragging_point = None;
        self.inspector.grade.dragging_wheel = None;
    }

    #[expect(
        clippy::many_single_char_names,
        reason = "事件 e、坐标 x/y、色轮 g 等短名在这里一看就懂"
    )]
    fn on_grade_slider(
        &mut self,
        which: GradeSlider,
        e: &SliderEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match e {
            SliderEvent::Change(v) => {
                let v = v.start() / 100.0;
                self.begin_grade_gesture();
                let band = self.inspector.grade.band.index();
                self.edit_grade(
                    |g| match which {
                        GradeSlider::Hsl(0) => g.hsl[band].hue = v,
                        GradeSlider::Hsl(1) => g.hsl[band].saturation = v,
                        GradeSlider::Hsl(_) => g.hsl[band].lightness = v,
                        GradeSlider::Lut => {
                            if let Some(l) = &mut g.lut {
                                l.intensity = v.clamp(0.0, 1.0);
                            }
                        }
                        GradeSlider::WheelLuma(i) => {
                            let (x, y, _) = wheel_split(wheel_value(g, i));
                            *wheel_value_mut(g, i) = wheel_join(x, y, v);
                        }
                    },
                    window,
                    cx,
                );
            }
            SliderEvent::Release(_) => self.end_grade_gesture(),
        }
    }

    /// 滑条跟着片段的进阶调色走（换了片段、换了色相、撤销之后）；正在拖的时候不动。
    pub(crate) fn refresh_grade_sliders(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.inspector.grade.gesture {
            return;
        }
        let Some((_, g)) = self.bound_grade() else {
            return;
        };
        let f = &self.inspector.grade;
        let h = g.hsl[f.band.index()];
        let mut set = |s: &Entity<SliderState>, v: f32| {
            let ui = (v * 100.0).round();
            if (s.read(cx).value().start() - ui).abs() > 0.5 {
                s.update(cx, |s, cx| s.set_value(ui, window, cx));
            }
        };
        set(&f.hsl[0], h.hue);
        set(&f.hsl[1], h.saturation);
        set(&f.hsl[2], h.lightness);
        set(
            &f.lut_intensity,
            g.lut.as_ref().map_or(1.0, |l| l.intensity),
        );
        for i in 0..4 {
            set(&f.wheel_luma[i], wheel_split(wheel_value(&g, i)).2);
        }
    }

    fn import_lut(&mut self, window: &Window, cx: &Context<'_, Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择 LUT 文件（.cube）".into()),
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    this.use_lut(path, window, cx);
                });
            }
        }));
    }

    /// 套用 LUT 文件（读不出来时提示）。
    pub fn use_lut(
        &mut self,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if ve_core::grade::load_lut(&path).is_none() {
            crate::workspace::notify(
                gpui_kit::component::notification::Notification::warning(
                    "读不出这个 LUT（只支持 .cube 的 3D LUT）",
                ),
                window,
                cx,
            );
            return;
        }
        self.edit_grade(
            |g| {
                g.lut = Some(LutRef {
                    path,
                    intensity: 1.0,
                });
            },
            window,
            cx,
        );
    }

    /// 二级分段。
    pub(crate) fn render_adjust_segments(&self, cx: &Context<'_, Self>) -> AnyElement {
        let current = self.inspector.grade.seg;
        h_flex()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(4.))
            .bg(cx.theme().muted)
            .children(
                [
                    ("adj-seg-basic", "基础", AdjustSeg::Basic),
                    ("adj-seg-hsl", "HSL", AdjustSeg::Hsl),
                    ("adj-seg-curves", "曲线", AdjustSeg::Curves),
                    ("adj-seg-wheels", "色轮", AdjustSeg::Wheels),
                ]
                .map(|(id, label, seg)| {
                    let on = seg == current;
                    div()
                        .id(id)
                        .test_support()
                        .flex_1()
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(3.))
                        .text_xs()
                        .cursor_pointer()
                        .when(on, |s| {
                            s.bg(cx.theme().accent).text_color(cx.theme().foreground)
                        })
                        .when(!on, |s| s.text_color(cx.theme().muted_foreground))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.inspector.grade.seg = seg;
                            cx.notify();
                        }))
                        .child(label)
                }),
            )
            .into_any_element()
    }

    fn slider_row(
        label: &'static str,
        id: &'static str,
        s: &Entity<SliderState>,
        value: f32,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        h_flex()
            .gap_2()
            .child(
                div()
                    .w(px(64.))
                    .flex_none()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(div().flex_1().id(id).test_support().child(Slider::new(s)))
            .child(
                div()
                    .w(px(36.))
                    .text_sm()
                    .child(SharedString::from(format!("{}", (value * 100.0).round()))),
            )
            .into_any_element()
    }

    /// 基础里的 LUT 一组。
    pub(crate) fn render_lut_group(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let (_, g) = self.bound_grade()?;
        let name = g
            .lut
            .as_ref()
            .and_then(|l| l.path.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let body = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(SharedString::from(name.unwrap_or_else(|| "无".to_owned()))),
                    )
                    .child(
                        Button::new("lut-import")
                            .outline()
                            .compact()
                            .label("导入 LUT…")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.import_lut(window, cx)),
                            ),
                    ),
            )
            .children(g.lut.as_ref().map(|l| {
                Self::slider_row(
                    "强度",
                    "lut-intensity",
                    &self.inspector.grade.lut_intensity,
                    l.intensity,
                    cx,
                )
            }))
            .into_any_element();
        Some(self.render_group(
            crate::inspector_group::Group {
                id: "lut",
                title: "LUT",
                check: Some((g.lut.is_some(), |this, on, window, cx| {
                    if on {
                        this.import_lut(window, cx);
                    } else {
                        this.edit_grade(|g| g.lut = None, window, cx);
                    }
                })),
                reset: Some(|this, window, cx| this.edit_grade(|g| g.lut = None, window, cx)),
                extra: None,
            },
            body,
            cx,
        ))
    }

    /// HSL 分段。
    pub(crate) fn render_hsl_seg(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let (_, g) = self.bound_grade()?;
        let current = self.inspector.grade.band;
        let rings = HslBand::ALL.map(|b| {
            let [r, gr, bl] = ve_core::grade::hsl_preview(b);
            let on = b == current;
            div()
                .id(SharedString::from(format!("hsl-{}", b.index())))
                .test_support()
                .size(px(22.))
                .rounded_full()
                .border_2()
                .border_color(if on {
                    palette::text()
                } else {
                    gpui_kit::transparent_black()
                })
                .bg(rgb((u32::from(r) << 16)
                    | (u32::from(gr) << 8)
                    | u32::from(bl)))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.inspector.grade.band = b;
                    cx.notify();
                }))
        });
        let h = g.hsl[current.index()];
        let f = &self.inspector.grade;
        Some(
            v_flex()
                .id("insp-hsl")
                .test_support()
                .gap_3()
                .child(h_flex().gap_2().children(rings))
                .child(div().text_sm().child(current.label()))
                .child(Self::slider_row("色相", "hsl-hue", &f.hsl[0], h.hue, cx))
                .child(Self::slider_row(
                    "饱和度",
                    "hsl-sat",
                    &f.hsl[1],
                    h.saturation,
                    cx,
                ))
                .child(Self::slider_row(
                    "亮度",
                    "hsl-light",
                    &f.hsl[2],
                    h.lightness,
                    cx,
                ))
                .child(
                    Button::new("hsl-reset")
                        .ghost()
                        .compact()
                        .label("重置 HSL")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.edit_grade(|g| g.hsl = Default::default(), window, cx);
                        })),
                )
                .into_any_element(),
        )
    }

    // ---- 曲线 ----

    /// 曲线编辑区里的点 → 屏幕坐标。
    fn curve_to_screen(b: Bounds<Pixels>, (x, y): (f32, f32)) -> (f32, f32) {
        (
            f32::from(b.origin.x) + x * f32::from(b.size.width),
            f32::from(b.origin.y) + (1.0 - y) * f32::from(b.size.height),
        )
    }

    fn screen_to_curve(b: Bounds<Pixels>, p: gpui_kit::Point<Pixels>) -> (f32, f32) {
        let w = f32::from(b.size.width).max(1.0);
        let h = f32::from(b.size.height).max(1.0);
        (
            ((f32::from(p.x) - f32::from(b.origin.x)) / w).clamp(0.0, 1.0),
            (1.0 - (f32::from(p.y) - f32::from(b.origin.y)) / h).clamp(0.0, 1.0),
        )
    }

    #[expect(
        clippy::many_single_char_names,
        reason = "事件 e、坐标 x/y、色轮 g 等短名在这里一看就懂"
    )]
    fn curve_mouse_down(
        &mut self,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((_, g)) = self.bound_grade() else {
            return;
        };
        let ch = self.inspector.grade.channel;
        let b = self.inspector.grade.curve_bounds;
        let pts = ch.points(&g).clone();
        let near = pts.iter().position(|&pt| {
            let (sx, sy) = Self::curve_to_screen(b, pt);
            (sx - f32::from(e.position.x)).hypot(sy - f32::from(e.position.y)) <= 8.0
        });
        if e.button == MouseButton::Right {
            if let Some(i) = near {
                self.edit_grade(
                    |g| {
                        ch.points_mut(g).remove(i);
                    },
                    window,
                    cx,
                );
            }
            return;
        }
        self.begin_grade_gesture();
        let (x, y) = Self::screen_to_curve(b, e.position);
        let index = near.unwrap_or_else(|| pts.partition_point(|p| p.0 < x));
        if near.is_none() {
            // 和已有的点横坐标重了就不加
            if pts.iter().any(|p| (p.0 - x).abs() < 0.01) {
                return;
            }
            self.edit_grade(|g| ch.points_mut(g).insert(index, (x, y)), window, cx);
        }
        self.inspector.grade.dragging_point = Some(index);
    }

    /// 整个属性面板上的鼠标移动：正在拖曲线点或色轮时接着改（鼠标拖出那一小块也跟手）。
    pub(crate) fn grade_drag_move(
        &mut self,
        e: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let f = &self.inspector.grade;
        if f.dragging_point.is_none() && f.dragging_wheel.is_none() {
            return;
        }
        if e.pressed_button != Some(MouseButton::Left) {
            self.end_grade_gesture();
            return;
        }
        if let Some(i) = self.inspector.grade.dragging_wheel {
            self.wheel_at(i, e.position, window, cx);
        } else {
            self.curve_mouse_move(e, window, cx);
        }
    }

    /// 松开：结束拖曲线点 / 色轮。
    pub(crate) fn grade_drag_up(&mut self) {
        if self.inspector.grade.dragging_point.is_some()
            || self.inspector.grade.dragging_wheel.is_some()
        {
            self.end_grade_gesture();
        }
    }

    fn curve_mouse_move(
        &mut self,
        e: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(i) = self.inspector.grade.dragging_point else {
            return;
        };
        let ch = self.inspector.grade.channel;
        let (x, y) = Self::screen_to_curve(self.inspector.grade.curve_bounds, e.position);
        self.edit_grade(
            |g| {
                let pts = ch.points_mut(g);
                if i >= pts.len() {
                    return;
                }
                // 横坐标夹在左右两个点之间，保持排好序
                let lo = if i == 0 { 0.0 } else { pts[i - 1].0 + 0.01 };
                let hi = pts.get(i + 1).map_or(1.0, |p| p.0 - 0.01);
                pts[i] = (x.clamp(lo, hi.max(lo)), y);
            },
            window,
            cx,
        );
    }

    /// 曲线分段。
    pub(crate) fn render_curves_seg(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let (_, g) = self.bound_grade()?;
        let ch = self.inspector.grade.channel;
        let chips = CurveChannel::ALL.map(|(c, id, label)| {
            crate::chrome::chip(id, label, c == ch, cx).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.inspector.grade.channel = c;
                    cx.notify();
                },
            ))
        });
        let points = ch.points(&g).clone();
        let color = ch.color();
        let this = cx.entity();
        let editor = canvas(
            move |bounds, _, cx| {
                this.update(cx, |w, _| w.inspector.grade.curve_bounds = bounds);
            },
            move |bounds, (), window, _| {
                window.paint_quad(fill(bounds, palette::timeline_bg()));
                // 四等分的网格
                for k in 1..4 {
                    #[expect(clippy::cast_precision_loss, reason = "画图")]
                    let t = k as f32 / 4.0;
                    let (x, y) = Self::curve_to_screen(bounds, (t, t));
                    window.paint_quad(fill(
                        Bounds::new(
                            point(px(x), bounds.origin.y),
                            size(px(1.0), bounds.size.height),
                        ),
                        palette::divider(),
                    ));
                    window.paint_quad(fill(
                        Bounds::new(
                            point(bounds.origin.x, px(y)),
                            size(bounds.size.width, px(1.0)),
                        ),
                        palette::divider(),
                    ));
                }
                let n = 120;
                for k in 0..=n {
                    #[expect(clippy::cast_precision_loss, reason = "画图")]
                    let x = k as f32 / n as f32;
                    let y = ve_core::grade::curve_value(&points, x);
                    let (sx, sy) = Self::curve_to_screen(bounds, (x, y));
                    window.paint_quad(fill(
                        Bounds::new(point(px(sx - 1.0), px(sy - 1.0)), size(px(2.0), px(2.0))),
                        rgb(color),
                    ));
                }
                for &pt in &points {
                    let (sx, sy) = Self::curve_to_screen(bounds, pt);
                    window.paint_quad(
                        fill(
                            Bounds::new(point(px(sx - 4.0), px(sy - 4.0)), size(px(8.0), px(8.0))),
                            rgb(color),
                        )
                        .corner_radii(px(4.0)),
                    );
                }
            },
        )
        .size_full();
        Some(
            v_flex()
                .id("insp-curves")
                .test_support()
                .gap_3()
                .child(h_flex().gap_1().children(chips))
                .child(
                    div()
                        .id("curve-editor")
                        .test_support()
                        .w(px(CURVE_SIZE))
                        .h(px(CURVE_SIZE))
                        .cursor(gpui_kit::CursorStyle::Crosshair)
                        .on_mouse_down(MouseButton::Left, cx.listener(Self::curve_mouse_down))
                        .on_mouse_down(MouseButton::Right, cx.listener(Self::curve_mouse_down))
                        .child(editor),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("点一下加点，拖动改值，右键点删掉"),
                )
                .child(
                    Button::new("curve-reset")
                        .ghost()
                        .compact()
                        .label("重置这条曲线")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit_grade(|g| ch.points_mut(g).clear(), window, cx);
                        })),
                )
                .into_any_element(),
        )
    }

    // ---- 色轮 ----

    #[expect(
        clippy::many_single_char_names,
        reason = "事件 e、坐标 x/y、色轮 g 等短名在这里一看就懂"
    )]
    fn wheel_at(
        &mut self,
        i: usize,
        p: gpui_kit::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let b = self.inspector.grade.wheel_bounds[i];
        let r = f32::from(b.size.width) / 2.0;
        if r <= 0.0 {
            return;
        }
        let cx0 = f32::from(b.origin.x) + r;
        let cy0 = f32::from(b.origin.y) + r;
        let (mut x, mut y) = ((f32::from(p.x) - cx0) / r, (f32::from(p.y) - cy0) / r);
        let len = x.hypot(y);
        if len > 1.0 {
            x /= len;
            y /= len;
        }
        self.edit_grade(
            |g| {
                let (_, _, luma) = wheel_split(wheel_value(g, i));
                *wheel_value_mut(g, i) = wheel_join(x, y, luma);
            },
            window,
            cx,
        );
    }

    /// 色轮分段。
    #[expect(
        clippy::many_single_char_names,
        reason = "事件 e、坐标 x/y、色轮 g 等短名在这里一看就懂"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "一整块面板的布局，拆开反而难对照界面"
    )]
    pub(crate) fn render_wheels_seg(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let (_, g) = self.bound_grade()?;
        let wheels = WHEELS.iter().enumerate().map(|(i, &(id, label))| {
            let (x, y, luma) = wheel_split(wheel_value(&g, i));
            let this = cx.entity();
            let pad = canvas(
                move |bounds, _, cx| {
                    this.update(cx, |w, _| w.inspector.grade.wheel_bounds[i] = bounds);
                },
                move |bounds, (), window, _| {
                    let r = f32::from(bounds.size.width) / 2.0;
                    let (cx0, cy0) = (
                        f32::from(bounds.origin.x) + r,
                        f32::from(bounds.origin.y) + r,
                    );
                    // 一圈色相（24 个小点）
                    for k in 0..24 {
                        #[expect(clippy::cast_precision_loss, reason = "画图")]
                        let a = k as f32 / 24.0 * std::f32::consts::TAU;
                        let rgbv = wheel_join(a.cos(), a.sin(), 0.0)
                            .map(|v| ((v + 0.67) / 1.34).clamp(0.0, 1.0));
                        #[expect(
                            clippy::cast_possible_truncation,
                            clippy::cast_sign_loss,
                            reason = "0–255"
                        )]
                        let c = rgbv.map(|v| (v * 255.0) as u32);
                        let (px0, py0) = (cx0 + a.cos() * (r - 4.0), cy0 + a.sin() * (r - 4.0));
                        window.paint_quad(
                            fill(
                                Bounds::new(
                                    point(px(px0 - 3.0), px(py0 - 3.0)),
                                    size(px(6.0), px(6.0)),
                                ),
                                rgb((c[0] << 16) | (c[1] << 8) | c[2]),
                            )
                            .corner_radii(px(3.0)),
                        );
                    }
                    window.paint_quad(
                        fill(
                            Bounds::new(
                                point(px(cx0 - r + 8.0), px(cy0 - r + 8.0)),
                                size(px(2.0 * r - 16.0), px(2.0 * r - 16.0)),
                            ),
                            palette::timeline_bg(),
                        )
                        .corner_radii(px(r - 8.0)),
                    );
                    let (dx, dy) = (cx0 + x * (r - 8.0), cy0 + y * (r - 8.0));
                    window.paint_quad(
                        fill(
                            Bounds::new(point(px(dx - 4.0), px(dy - 4.0)), size(px(8.0), px(8.0))),
                            palette::text(),
                        )
                        .corner_radii(px(4.0)),
                    );
                },
            )
            .size_full();
            let v = wheel_value(&g, i);
            v_flex()
                .gap_1()
                .items_center()
                .child(div().text_xs().child(label))
                .child(
                    div()
                        .id(id)
                        .test_support()
                        .size(px(WHEEL_SIZE))
                        .cursor(gpui_kit::CursorStyle::Crosshair)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                                this.begin_grade_gesture();
                                this.inspector.grade.dragging_wheel = Some(i);
                                this.wheel_at(i, e.position, window, cx);
                            }),
                        )
                        .child(pad),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!(
                            "R {:+.0} G {:+.0} B {:+.0}",
                            v[0] * 100.0,
                            v[1] * 100.0,
                            v[2] * 100.0
                        ))),
                )
                .child(
                    div()
                        .w(px(WHEEL_SIZE))
                        .id(SharedString::from(format!("{id}-luma")))
                        .test_support()
                        .child(Slider::new(&self.inspector.grade.wheel_luma[i])),
                )
                .child(
                    div()
                        .text_xs()
                        .child(SharedString::from(format!("明暗 {:+.0}", luma * 100.0))),
                )
                .into_any_element()
        });
        Some(
            v_flex()
                .id("insp-wheels")
                .test_support()
                .gap_3()
                .child(h_flex().flex_wrap().gap_3().children(wheels))
                .child(
                    Button::new("wheels-reset")
                        .ghost()
                        .compact()
                        .label("重置色轮")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.edit_grade(|g| g.wheels = ve_core::Wheels::default(), window, cx);
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_position_and_luma_round_trip() {
        for (x, y, l) in [(0.5, 0.0, 0.1), (0.0, -0.6, 0.0), (-0.3, 0.4, -0.2)] {
            let (x2, y2, l2) = wheel_split(wheel_join(x, y, l));
            assert!((x - x2).abs() < 1e-4 && (y - y2).abs() < 1e-4 && (l - l2).abs() < 1e-4);
        }
        // 往右拖是偏红
        let red = wheel_join(1.0, 0.0, 0.0);
        assert!(red[0] > red[1] && red[0] > red[2]);
    }
}
