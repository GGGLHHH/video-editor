//! 裁剪窗口（工具栏「裁剪」，剪映）：播放头这一刻的原图上拖框——拖四角、四边改大小，拖中间挪位置，
//! 选比例后按比例拉；「确定」才写回片段（一步撤销），「取消」不动。

use crate::crop_box::{Handle, Rect, drag, hit};
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Bounds, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Point, Render, RenderImage, StatefulInteractiveElement as _, Styled as _, Task, Window,
    div, px,
};
use std::sync::Arc;
use ve_core::{ClipId, Crop, Transform};

/// 画面区域最大的宽、高（素材按比例放进去）。
const STAGE_MAX: (f32, f32) = (440.0, 260.0);
/// 角、边能点中的宽度（屏幕像素）。
const SLOP_PX: f32 = 8.0;

/// 一个比例选项：（按钮 id，名字，宽高比；自由时 `None`）。
type RatioChip = (&'static str, &'static str, Option<(u32, u32)>);

/// 比例选项。
const RATIOS: [RatioChip; 6] = [
    ("crop-free", "自由", None),
    ("crop-16-9", "16:9", Some((16, 9))),
    ("crop-9-16", "9:16", Some((9, 16))),
    ("crop-1-1", "1:1", Some((1, 1))),
    ("crop-4-3", "4:3", Some((4, 3))),
    ("crop-3-4", "3:4", Some((3, 4))),
];

/// 裁剪窗口里的内容（框、比例、原图）。
pub(crate) struct CropPanel {
    /// 素材大小（像素）。
    source: (u32, u32),
    /// 框（素材像素）。
    pub(crate) rect: Rect,
    /// 锁定的比例（自由时 `None`）。
    ratio: Option<(u32, u32)>,
    /// 播放头这一刻的原图（后台取到之前是空的）。
    image: Option<Arc<RenderImage>>,
    /// 画面区域在窗口里的位置。
    stage: Bounds<Pixels>,
    /// 正在拖：按住的部分、按下的位置、按下时的框。
    dragging: Option<(Handle, Point<Pixels>, Rect)>,
    _load: Task<()>,
}

/// 素材放进画面区域后的大小。
fn stage_size((w, h): (u32, u32)) -> (f32, f32) {
    #[expect(clippy::cast_precision_loss, reason = "画面尺寸很小")]
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let k = (STAGE_MAX.0 / w).min(STAGE_MAX.1 / h);
    (w * k, h * k)
}

impl CropPanel {
    const fn size(&self) -> (f32, f32) {
        #[expect(clippy::cast_precision_loss, reason = "画面尺寸很小")]
        let s = (self.source.0 as f32, self.source.1 as f32);
        s
    }

    /// 屏幕上一个像素是素材的几个像素。
    fn px_scale(&self) -> f32 {
        self.size().0 / stage_size(self.source).0
    }

    /// 现在的框换成裁剪值。
    pub(crate) fn crop(&self) -> Crop {
        self.rect.to_crop(self.size())
    }

    fn pick_ratio(&mut self, ratio: Option<(u32, u32)>, cx: &mut Context<'_, Self>) {
        self.ratio = ratio;
        if let Some(r) = ratio {
            self.rect = Rect::from_crop(ve_core::centered_crop(self.source, r), self.size());
        }
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<'_, Self>) {
        self.ratio = None;
        self.rect = Rect::from_crop(Crop::default(), self.size());
        cx.notify();
    }

    /// 窗口坐标 → 素材像素。
    fn to_source(&self, p: Point<Pixels>) -> (f32, f32) {
        let k = self.px_scale();
        (
            f32::from(p.x - self.stage.origin.x) * k,
            f32::from(p.y - self.stage.origin.y) * k,
        )
    }

    fn press(&mut self, p: Point<Pixels>, cx: &mut Context<'_, Self>) {
        let slop = SLOP_PX * self.px_scale();
        self.dragging = hit(self.rect, self.to_source(p), slop).map(|h| (h, p, self.rect));
        cx.notify();
    }

    fn move_to(&mut self, p: Point<Pixels>, cx: &mut Context<'_, Self>) {
        let Some((handle, from, start)) = self.dragging else {
            return;
        };
        let k = self.px_scale();
        let delta = (f32::from(p.x - from.x) * k, f32::from(p.y - from.y) * k);
        #[expect(clippy::cast_precision_loss, reason = "比例是小整数")]
        let ratio = self.ratio.map(|(a, b)| a as f32 / b as f32);
        self.rect = drag(start, handle, delta, self.size(), ratio);
        cx.notify();
    }
}

impl CropPanel {
    /// 框外压暗、三分线、框和八个点（画面区域里的像素坐标）。
    fn overlay(&self, (sw, sh): (f32, f32)) -> Vec<gpui_kit::AnyElement> {
        let k = 1.0 / self.px_scale();
        let r = self.rect;
        let (x0, y0, x1, y1) = (r.x0 * k, r.y0 * k, r.x1 * k, r.y1 * k);
        let at = |l: f32, t: f32, w: f32, h: f32| {
            div()
                .absolute()
                .left(px(l))
                .top(px(t))
                .w(px(w.max(0.0)))
                .h(px(h.max(0.0)))
        };
        let dim = gpui_kit::hsla(0.0, 0.0, 0.0, 0.55);
        let line = gpui_kit::hsla(0.0, 0.0, 1.0, 0.35);
        let (bw, bh) = (x1 - x0, y1 - y0);
        let mut out: Vec<gpui_kit::AnyElement> = [
            at(0.0, 0.0, sw, y0),
            at(0.0, y1, sw, sh - y1),
            at(0.0, y0, x0, bh),
            at(x1, y0, sw - x1, bh),
        ]
        .into_iter()
        .map(|d| d.bg(dim).into_any_element())
        .collect();
        for i in [1.0_f32, 2.0] {
            out.push(
                at(x0 + bw * i / 3.0, y0, 1.0, bh)
                    .bg(line)
                    .into_any_element(),
            );
            out.push(
                at(x0, y0 + bh * i / 3.0, bw, 1.0)
                    .bg(line)
                    .into_any_element(),
            );
        }
        out.push(
            at(x0, y0, bw, bh)
                .id("crop-box")
                .test_support()
                .border_1()
                .border_color(gpui_kit::white())
                .into_any_element(),
        );
        let (mx, my) = (f32::midpoint(x0, x1), f32::midpoint(y0, y1));
        for (x, y) in [
            (x0, y0),
            (x1, y0),
            (x0, y1),
            (x1, y1),
            (mx, y0),
            (mx, y1),
            (x0, my),
            (x1, my),
        ] {
            out.push(
                at(x - 4.0, y - 4.0, 8.0, 8.0)
                    .rounded_full()
                    .bg(gpui_kit::white())
                    .into_any_element(),
            );
        }
        out
    }

    /// 画面区域：原图上盖着框；按住拖框。
    fn render_stage(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let (sw, sh) = stage_size(self.source);
        let entity = cx.entity();
        div()
            .id("crop-stage")
            .test_support()
            .relative()
            .w(px(sw))
            .h(px(sh))
            .flex_none()
            .bg(gpui_kit::black())
            .cursor_pointer()
            .children(
                self.image
                    .clone()
                    .map(|image| gpui_kit::img(image).size_full()),
            )
            .children(self.overlay((sw, sh)))
            .child(
                gpui_kit::canvas(
                    move |bounds, _, cx| {
                        if entity.read(cx).stage != bounds {
                            entity.update(cx, |p, _| p.stage = bounds);
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, e: &gpui_kit::MouseDownEvent, _, cx| {
                    this.press(e.position, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &gpui_kit::MouseMoveEvent, _, cx| {
                this.move_to(e.position, cx);
            }))
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = None),
            )
            .on_mouse_up_out(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = None),
            )
    }
}

impl Render for CropPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let chips = RATIOS.map(|(id, label, ratio)| {
            crate::chrome::chip(id, label, self.ratio == ratio, cx)
                .on_click(cx.listener(move |this, _, _, cx| this.pick_ratio(ratio, cx)))
                .into_any_element()
        });
        let crop = self.crop();
        let pct = |v: f32| (v * 100.0).round();
        v_flex()
            .gap_2()
            .items_center()
            .child(self.render_stage(cx))
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .flex_wrap()
                    .children(chips)
                    .child(div().flex_1())
                    .child(
                        crate::chrome::chip("crop-reset", "重置", false, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.reset(cx))),
                    ),
            )
            .child(
                div()
                    .id("crop-numbers")
                    .test_support()
                    .w_full()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "左 {}%  上 {}%  右 {}%  下 {}%（拖框的角、边改大小，拖中间挪位置）",
                        pct(crop.left),
                        pct(crop.top),
                        pct(crop.right),
                        pct(crop.bottom)
                    )),
            )
    }
}

impl Workspace {
    /// 工具栏「裁剪」：弹裁剪窗口，框从片段现在的裁剪开始。
    pub(crate) fn open_crop(
        &mut self,
        _: &crate::actions::OpenCrop,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        use gpui_kit::component::WindowExt as _;
        use gpui_kit::component::button::{Button, ButtonVariants as _};
        use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
        let Some((clip, current, at)) = self.bound_visual_clip() else {
            return;
        };
        let Some((_, c)) = self.doc.project.timeline.find_clip(clip) else {
            return;
        };
        let Some(asset) = c.asset().and_then(|a| self.doc.project.assets.get(&a)) else {
            return;
        };
        let Some(video) = asset.video else {
            return;
        };
        let source = (video.width, video.height);
        let (path, time) = (asset.path.clone(), c.source_at(at));
        let panel = cx.new(|cx| {
            let load = cx.spawn(async move |this, cx| {
                let frame = cx
                    .background_spawn(async move {
                        let target = ve_media::ScaleTarget {
                            max_width: 880,
                            max_height: 520,
                        };
                        ve_media::VideoDecoder::open(&path, target)
                            .and_then(|mut d| d.seek_to(time))
                            .ok()
                    })
                    .await;
                let image = frame.as_ref().and_then(crate::preview::to_render_image);
                let _ignored = this.update(cx, |p: &mut CropPanel, cx| {
                    p.image = image;
                    cx.notify();
                });
            });
            #[expect(clippy::cast_precision_loss, reason = "画面尺寸很小")]
            let size = (source.0 as f32, source.1 as f32);
            CropPanel {
                source,
                rect: Rect::from_crop(current.crop, size),
                ratio: None,
                image: None,
                stage: Bounds::default(),
                dragging: None,
                _load: load,
            }
        });
        let ws = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let (ws, panel) = (ws.clone(), panel.clone());
            dialog
                .title("裁剪")
                .width(px(480.))
                .child(panel.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            div()
                                .id("crop-cancel")
                                .test_support()
                                .child(DialogClose::new().trigger(|b| b.ghost().label("取消"))),
                        )
                        .child(
                            DialogAction::new()
                                .child(Button::new("crop-ok").primary().label("确定")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let crop = panel.read(cx).crop();
                    ws.update(cx, |this, cx| this.set_crop(clip, crop, window, cx));
                    true
                })
        });
    }

    /// 把片段的裁剪换成 `crop`（一步撤销；有裁剪关键帧时在播放头打帧）。
    fn set_crop(
        &mut self,
        clip: ClipId,
        crop: Crop,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((bound, current, at)) = self.bound_visual_clip() else {
            return;
        };
        if bound != clip {
            return;
        }
        let t = Transform { crop, ..current };
        self.apply(window, cx, |p| {
            ve_core::edit::apply_transform_at(p, clip, at, t)
        });
    }
}
