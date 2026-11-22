//! 属性面板「画面」的外观（剪映）：二级分段「基础 / 抠像 / 蒙版」；基础里的圆角、混合模式、描边、阴影，
//! 抠像里的色度抠图（取色器在播放器上点一下取色），蒙版的形状、位置、大小、旋转、羽化、圆角、反转。
//! 每个数值一个滑条，拖一次一步撤销。

use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, prelude::FluentBuilder as _, px, rgb,
};
use ve_core::edit::{apply_look_at, reset_look_at};
use ve_core::{
    BlendMode, ChromaKey, ClipId, KeyGroup, Look, Mask, MaskShape, Prop, Shadow, Stroke, Time,
};

/// 「画面」标签的二级分段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PictureSeg {
    /// 基础：位置大小、混合、描边、阴影。
    #[default]
    Basic,
    /// 抠像：色度抠图。
    Key,
    /// 蒙版。
    Mask,
}

/// 外观里的一个数值（一个滑条）。界面上的单位见 [`LookField::range`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LookField {
    Radius,
    StrokeWidth,
    ShadowOpacity,
    ShadowBlur,
    ShadowDistance,
    ShadowAngle,
    KeyIntensity,
    KeyShadow,
    MaskX,
    MaskY,
    MaskWidth,
    MaskHeight,
    MaskRotation,
    MaskFeather,
    MaskRound,
}

impl LookField {
    const ALL: [Self; 15] = [
        Self::Radius,
        Self::StrokeWidth,
        Self::ShadowOpacity,
        Self::ShadowBlur,
        Self::ShadowDistance,
        Self::ShadowAngle,
        Self::KeyIntensity,
        Self::KeyShadow,
        Self::MaskX,
        Self::MaskY,
        Self::MaskWidth,
        Self::MaskHeight,
        Self::MaskRotation,
        Self::MaskFeather,
        Self::MaskRound,
    ];

    /// 滑条的范围（界面单位）。
    const fn range(self) -> (f32, f32) {
        match self {
            Self::ShadowAngle | Self::MaskRotation => (-180.0, 180.0),
            Self::MaskX | Self::MaskY => (-100.0, 100.0),
            Self::MaskWidth | Self::MaskHeight => (1.0, 200.0),
            _ => (0.0, 100.0),
        }
    }

    /// 界面值 = 存的值 × 这个倍数。
    const fn scale(self) -> f32 {
        match self {
            Self::StrokeWidth | Self::ShadowBlur | Self::ShadowDistance => 1000.0,
            Self::ShadowAngle | Self::MaskRotation => 1.0,
            _ => 100.0,
        }
    }

    /// 名字。
    const fn label(self) -> &'static str {
        match self {
            Self::Radius | Self::MaskRound => "圆角",
            Self::StrokeWidth => "大小",
            Self::ShadowOpacity => "不透明度",
            Self::ShadowBlur => "模糊",
            Self::ShadowDistance => "距离",
            Self::ShadowAngle | Self::MaskRotation => "角度",
            Self::KeyIntensity => "强度",
            Self::KeyShadow => "阴影",
            Self::MaskX => "位置 X",
            Self::MaskY => "位置 Y",
            Self::MaskWidth => "宽",
            Self::MaskHeight => "高",
            Self::MaskFeather => "羽化",
        }
    }

    /// 元素编号。
    const fn id(self) -> &'static str {
        match self {
            Self::Radius => "look-radius",
            Self::StrokeWidth => "look-stroke-width",
            Self::ShadowOpacity => "look-shadow-opacity",
            Self::ShadowBlur => "look-shadow-blur",
            Self::ShadowDistance => "look-shadow-distance",
            Self::ShadowAngle => "look-shadow-angle",
            Self::KeyIntensity => "look-key-intensity",
            Self::KeyShadow => "look-key-shadow",
            Self::MaskX => "look-mask-x",
            Self::MaskY => "look-mask-y",
            Self::MaskWidth => "look-mask-w",
            Self::MaskHeight => "look-mask-h",
            Self::MaskRotation => "look-mask-rot",
            Self::MaskFeather => "look-mask-feather",
            Self::MaskRound => "look-mask-round",
        }
    }

    /// 对应的可打关键帧属性。
    const fn prop(self) -> Prop {
        match self {
            Self::Radius => Prop::Radius,
            Self::StrokeWidth => Prop::StrokeWidth,
            Self::ShadowOpacity => Prop::ShadowOpacity,
            Self::ShadowBlur => Prop::ShadowBlur,
            Self::ShadowDistance => Prop::ShadowDistance,
            Self::ShadowAngle => Prop::ShadowAngle,
            Self::KeyIntensity => Prop::KeyIntensity,
            Self::KeyShadow => Prop::KeyShadow,
            Self::MaskX => Prop::MaskX,
            Self::MaskY => Prop::MaskY,
            Self::MaskWidth => Prop::MaskWidth,
            Self::MaskHeight => Prop::MaskHeight,
            Self::MaskRotation => Prop::MaskRotation,
            Self::MaskFeather => Prop::MaskFeather,
            Self::MaskRound => Prop::MaskRound,
        }
    }

    /// 存的值（这一项没开时 `None`）。
    fn get(self, l: &Look) -> Option<f32> {
        self.prop().look_get(l)
    }

    /// 写存的值（这一项没开时什么都不做）。
    fn set(self, l: &mut Look, v: f32) {
        self.prop().look_set(l, v);
    }

    /// 界面值 → 存的值。
    fn ui_to_value(self, v: f32) -> f32 {
        v / self.scale()
    }

    /// 存的值 → 界面值（整数）。
    fn value_to_ui(self, v: f32) -> f32 {
        (v * self.scale()).round()
    }
}

/// 外观的滑条。
#[derive(Debug)]
pub(crate) struct LookFields {
    sliders: Vec<(LookField, Entity<SliderState>)>,
    gesture: bool,
    /// 二级分段。
    pub(crate) seg: PictureSeg,
    /// 取色器开着：下一次在播放器上点击取那里的颜色当抠图颜色。
    pub(crate) picking: bool,
    /// 「蒙版」里正在改第几个蒙版（0 = 第一个）。
    pub(crate) mask_index: usize,
    /// 文字蒙版的字（回车或失焦时写回）。
    pub(crate) mask_text: Entity<InputState>,
}

/// 正在改第 `i` 个蒙版时面板看到的外观：`mask` 换成那一个（其余不变）；没有那一个时就是原样。
pub(crate) fn view_mask(look: Look, i: usize) -> Look {
    i.checked_sub(1)
        .and_then(|k| look.extra_masks.get(k).copied().flatten())
        .map_or(look, |m| Look {
            mask: Some(m),
            ..look
        })
}

/// 面板改完的外观 `view` 放回去：`mask` 写回第 `i` 个蒙版，第一个蒙版用 `real`（这一刻的值，
/// 免得把动画到一半的值当成改了、多打关键帧）；选了「无」就是删掉这个蒙版。
pub(crate) fn unview_mask(view: Look, real: Look, i: usize) -> Look {
    let editing_extra = i
        .checked_sub(1)
        .is_some_and(|k| real.extra_masks.get(k).copied().flatten().is_some());
    let mut out = Look {
        mask: real.mask,
        extra_masks: real.extra_masks,
        ..view
    };
    if editing_extra {
        match view.mask {
            Some(m) => out.extra_masks[i - 1] = Some(m),
            None => out.remove_mask(i),
        }
    } else {
        match view.mask {
            Some(m) => out.mask = Some(m),
            // 第一个选了「无」：后面的往前挪
            None => out.remove_mask(0),
        }
    }
    out
}

/// 描边 / 阴影可选的颜色（剪映的色板第一行）。
const SWATCHES: [u32; 8] = [
    0xFF_FFFF, 0x00_0000, 0xE5_484D, 0xFF_C53D, 0x46_A758, 0x12_A594, 0x3E_63DD, 0x8E_4EC6,
];

impl LookFields {
    /// 创建滑条并订阅。
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<'_, Workspace>,
    ) -> (Self, Vec<Subscription>) {
        let mut subs = Vec::new();
        let mask_text = cx.new(|cx| InputState::new(window, cx).placeholder("输入文字"));
        subs.push(cx.subscribe_in(
            &mask_text,
            window,
            |this: &mut Workspace, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit_mask_text(window, cx);
                }
            },
        ));
        let sliders = LookField::ALL
            .iter()
            .map(|&f| {
                let (min, max) = f.range();
                let s = cx.new(|_| {
                    SliderState::new()
                        .min(min)
                        .max(max)
                        .step(1.0)
                        .default_value(min.max(0.0))
                });
                subs.push(cx.subscribe_in(
                    &s,
                    window,
                    move |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                        this.on_look_slider(f, e, window, cx);
                    },
                ));
                (f, s)
            })
            .collect();
        (
            Self {
                sliders,
                gesture: false,
                seg: PictureSeg::default(),
                picking: false,
                mask_index: 0,
                mask_text,
            },
            subs,
        )
    }

    fn slider(&self, f: LookField) -> Option<&Entity<SliderState>> {
        self.sliders.iter().find(|(g, _)| *g == f).map(|(_, s)| s)
    }
}

impl Workspace {
    /// 选中的视频轨媒体片段、它在播放头这一刻的外观（有关键帧的项按时间求值）、播放头相对片段起点的时间。
    fn bound_look_at(&self) -> Option<(ClipId, Look, Time)> {
        self.bound_look_real()
            .map(|(clip, look, at)| (clip, view_mask(look, self.inspector.look.mask_index), at))
    }

    /// 选中的视频轨媒体片段，或视频轨上的复合片段（整组的蒙版、圆角）。
    pub(crate) fn bound_look_clip(&self) -> Option<ClipId> {
        self.bound_video_clip().map(|(c, ..)| c).or_else(|| {
            let (c, ..) = self.bound_visual_clip()?;
            let (_, clip) = self.doc.project.timeline.find_clip(c)?;
            clip.compound().map(|_| c)
        })
    }

    /// 选中片段这一刻真正的外观（不换蒙版）。
    fn bound_look_real(&self) -> Option<(ClipId, Look, Time)> {
        let clip = self.bound_look_clip()?;
        let (_, c) = self.doc.project.timeline.find_clip(clip)?;
        let offset = self.key_offset(c);
        Some((clip, c.look_at(offset), offset))
    }

    /// 正在改的是第几个蒙版（那一个已经没了时当第一个）。
    fn mask_index_now(&self, real: &Look) -> usize {
        let i = self.inspector.look.mask_index;
        if i > 0 && real.extra_masks.get(i - 1).copied().flatten().is_some() {
            i
        } else {
            0
        }
    }

    /// 选中的视频轨媒体片段和它在播放头这一刻的外观。
    /// 把选中片段的文字蒙版的字填进输入框（没有时空着）。
    pub(crate) fn fill_mask_text(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let text = self
            .bound_look_clip()
            .and_then(|id| self.doc.project.timeline.find_clip(id))
            .and_then(|(_, c)| c.mask_text.as_ref().map(|t| t.text.clone()))
            .unwrap_or_default();
        self.inspector
            .look
            .mask_text
            .update(cx, |s, cx| s.set_value(text, window, cx));
    }

    /// 输入框里的字写回片段（一步撤销）；空的不收，输入框换回原来的字。
    fn commit_mask_text(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(clip) = self.bound_look_clip() else {
            return;
        };
        let text = self.inspector.look.mask_text.read(cx).value().to_string();
        let current = self
            .doc
            .project
            .timeline
            .find_clip(clip)
            .and_then(|(_, c)| c.mask_text.as_ref().map(|t| t.text.clone()));
        if current.is_none() || current.as_deref() == Some(text.as_str()) {
            return;
        }
        let ok = ve_core::MaskText {
            text: text.clone(),
            font: String::new(),
        }
        .is_valid();
        if ok {
            self.apply(window, cx, |p| ve_core::edit::set_mask_text(p, clip, &text));
        } else {
            self.fill_mask_text(window, cx);
        }
    }

    fn bound_look(&self) -> Option<(ClipId, Look)> {
        self.bound_look_at().map(|(clip, look, _)| (clip, look))
    }

    /// 改外观（一步撤销）。
    fn edit_look(
        &mut self,
        f: impl FnOnce(&mut Look),
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((clip, real, at)) = self.bound_look_real() else {
            return;
        };
        let i = self.mask_index_now(&real);
        let mut view = view_mask(real, i);
        f(&mut view);
        let look = unview_mask(view, real, i);
        self.apply(window, cx, |p| apply_look_at(p, clip, at, look));
    }

    /// 「添加蒙版」：加一个矩形蒙版并切过去改它。
    fn add_mask(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((clip, mut look, at)) = self.bound_look_real() else {
            return;
        };
        let Some(i) = look.add_mask() else {
            return;
        };
        self.inspector.look.mask_index = i;
        self.apply(window, cx, |p| apply_look_at(p, clip, at, look));
    }

    /// 删掉正在改的那个蒙版，回到第一个。
    fn remove_current_mask(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((clip, mut look, at)) = self.bound_look_real() else {
            return;
        };
        let i = self.mask_index_now(&look);
        look.remove_mask(i);
        self.inspector.look.mask_index = 0;
        self.apply(window, cx, |p| apply_look_at(p, clip, at, look));
    }

    /// 蒙版一排：「蒙版 1 / 2 …」切换正在改哪个，「⊕ 添加蒙版」「删除蒙版」。
    fn render_mask_tabs(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let (_, real, _) = self.bound_look_real()?;
        real.mask?;
        let current = self.mask_index_now(&real);
        let count = real.masks().count();
        let tabs = (0..count).map(|i| {
            crate::chrome::chip(
                SharedString::from(format!("mask-tab-{i}")),
                SharedString::from(format!("蒙版 {}", i + 1)),
                i == current,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.inspector.look.mask_index = i;
                cx.notify();
            }))
            .into_any_element()
        });
        let add = (count < ve_core::look::MAX_MASKS).then(|| {
            crate::chrome::chip("mask-add", "⊕ 添加蒙版", false, cx)
                .on_click(cx.listener(|this, _, window, cx| this.add_mask(window, cx)))
                .into_any_element()
        });
        let remove = (count > 1).then(|| {
            crate::chrome::chip("mask-remove", "删除蒙版", false, cx)
                .on_click(cx.listener(|this, _, window, cx| this.remove_current_mask(window, cx)))
                .into_any_element()
        });
        Some(
            h_flex()
                .flex_wrap()
                .gap_1()
                .children(tabs)
                .children(add)
                .children(remove)
                .into_any_element(),
        )
    }

    /// 一组的「↻ 重置」：这一组的关键帧一起清掉，再改成默认值（一步撤销）。
    fn reset_look(
        &mut self,
        f: impl FnOnce(&mut Look),
        props: &'static [Prop],
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((clip, real, at)) = self.bound_look_real() else {
            return;
        };
        let i = self.mask_index_now(&real);
        let mut view = view_mask(real, i);
        f(&mut view);
        let look = unview_mask(view, real, i);
        // 另外的蒙版没有关键帧：重置它时别清掉第一个蒙版的
        let props = if i > 0 && props == Prop::MASK.as_slice() {
            &[][..]
        } else {
            props
        };
        self.apply(window, cx, |p| reset_look_at(p, clip, at, look, props));
    }

    fn on_look_slider(
        &mut self,
        field: LookField,
        e: &SliderEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((clip, real, at)) = self.bound_look_real() else {
            return;
        };
        let i = self.mask_index_now(&real);
        match e {
            SliderEvent::Change(v) => {
                if !self.inspector.look.gesture {
                    self.doc.begin_gesture();
                    self.inspector.look.gesture = true;
                }
                let mut view = view_mask(real, i);
                field.set(&mut view, field.ui_to_value(v.start()));
                let look = unview_mask(view, real, i);
                let _ = self.doc.edit(|p| apply_look_at(p, clip, at, look));
                self.after_change(window, cx);
            }
            SliderEvent::Release(_) => {
                if self.inspector.look.gesture {
                    self.doc.end_gesture();
                    self.inspector.look.gesture = false;
                }
            }
        }
    }

    /// 滑条跟着片段的外观走（换了片段、撤销之后）；正在拖的时候不动。
    pub(crate) fn refresh_look_sliders(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.inspector.look.gesture {
            return;
        }
        let Some((_, look)) = self.bound_look() else {
            return;
        };
        for (f, s) in &self.inspector.look.sliders {
            if let Some(v) = f.get(&look) {
                let ui = f.value_to_ui(v);
                if (s.read(cx).value().start() - ui).abs() > 0.5 {
                    s.update(cx, |s, cx| s.set_value(ui, window, cx));
                }
            }
        }
    }

    /// 取色器：在播放器上点到的地方（画布坐标）取颜色当抠图颜色（剪映：取的是片段原片的颜色，
    /// 不是叠好之后的）。返回这一下是不是取色（是的话播放器不再当成选中 / 拖动）。
    pub(crate) fn pick_key_color(
        &mut self,
        p: (f32, f32),
        canvas: (u32, u32),
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if !self.inspector.look.picking {
            return false;
        }
        self.inspector.look.picking = false;
        cx.notify();
        let Some((clip, t, offset)) = self.bound_video_clip() else {
            return true;
        };
        let project = &self.doc.project;
        let Some((c, asset)) = project
            .timeline
            .find_clip(clip)
            .and_then(|(_, c)| Some((c, project.assets.get(&c.asset()?)?)))
        else {
            return true;
        };
        let Some(info) = asset.video else { return true };
        let geom = ve_core::layer_geometry(canvas, (info.width, info.height), &t);
        let (lx, ly) = geom.to_local(p);
        let s = geom.shown;
        let (fx, fy) = ((lx - s.x) / s.w, (ly - s.y) / s.h);
        if !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy) {
            crate::workspace::notify(
                Notification::warning("在选中的画面上点一下取色"),
                window,
                cx,
            );
            return true;
        }
        let uv = (
            geom.uv.w.mul_add(fx, geom.uv.x),
            geom.uv.h.mul_add(fy, geom.uv.y),
        );
        let at = if asset.is_still() {
            Time::ZERO
        } else {
            c.source_at(offset)
        };
        let path = asset.path.clone();
        let work = cx.background_spawn(async move { sample_source(&path, at, uv) });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let picked = work.await;
            let _ignored = this.update_in(cx, |this, window, cx| match picked {
                Some(color) => this.edit_look(|l| set_key_color(l, color), window, cx),
                None => crate::workspace::notify(
                    Notification::warning("取色失败：读不出这一帧"),
                    window,
                    cx,
                ),
            });
        }));
        true
    }

    /// 一行：名字、滑条、数值。
    fn look_slider_row(
        &self,
        f: LookField,
        look: &Look,
        cx: &Context<'_, Self>,
    ) -> Option<AnyElement> {
        let s = self.inspector.look.slider(f)?;
        let v = f.get(look).map_or(0.0, |v| f.value_to_ui(v));
        Some(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .w(px(64.))
                        .flex_none()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(f.label()),
                )
                .child(
                    div()
                        .flex_1()
                        .id(f.id())
                        .test_support()
                        .child(Slider::new(s)),
                )
                .child(self.value_box(f.id(), s, format!("{v}"), 36.0, cx))
                // 关键帧只作用在第一个蒙版：改另外的蒙版时不显示 ‹ ◇ ›
                .children(
                    (self.inspector.look.mask_index == 0 || !Prop::MASK.contains(&f.prop()))
                        .then(|| self.render_group_keys(KeyGroup::Look(f.prop()), cx)),
                )
                .into_any_element(),
        )
    }

    /// 一排色块（描边 / 阴影 / 抠图颜色）。
    fn swatch_row(
        id: &'static str,
        current: u32,
        pick: fn(&mut Look, u32),
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
                    .child("颜色"),
            )
            .child(
                h_flex()
                    .gap_1()
                    .children(SWATCHES.iter().enumerate().map(|(i, &c)| {
                        div()
                            .id(SharedString::from(format!("{id}-{i}")))
                            .test_support()
                            .size(px(18.))
                            .rounded(px(3.))
                            .bg(rgb(c))
                            .border_2()
                            .border_color(if c == current {
                                palette::accent()
                            } else {
                                cx.theme().border
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_look(|l| pick(l, c), window, cx);
                            }))
                    })),
            )
            .into_any_element()
    }

    /// 二级分段「基础 / 抠像 / 蒙版」（剪映），点了就切换。视频轨上的复合片段也有（整组生效）。
    pub(crate) fn render_picture_segments(&self, cx: &Context<'_, Self>) -> AnyElement {
        let current = self.inspector.look.seg;
        let keyable = self.bound_look_clip().is_some();
        h_flex()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(4.))
            .bg(cx.theme().muted)
            .children(
                [
                    ("seg-basic", "基础", PictureSeg::Basic),
                    ("seg-key", "抠像", PictureSeg::Key),
                    ("seg-mask", "蒙版", PictureSeg::Mask),
                ]
                .into_iter()
                .filter(|&(_, _, seg)| keyable || seg != PictureSeg::Key)
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
                            this.inspector.look.seg = seg;
                            cx.notify();
                        }))
                        .child(label)
                }),
            )
            .into_any_element()
    }

    /// 「基础」里位置大小下面的：圆角一行。
    pub(crate) fn render_radius_row(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let (_, look) = self.bound_look()?;
        self.look_slider_row(LookField::Radius, &look, cx)
    }

    /// 「混合」组里的混合模式（一排可选项，剪映是下拉框）。
    pub(crate) fn render_blend_modes(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let (_, look) = self.bound_look()?;
        Some(
            h_flex()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .w(px(64.))
                        .flex_none()
                        .pt_1()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("混合模式"),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .flex_wrap()
                        .gap_1()
                        .children(BlendMode::ALL.map(|m| {
                            crate::chrome::chip(
                                SharedString::from(format!("blend-{}", m.code())),
                                m.label(),
                                look.blend == m,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.edit_look(|l| l.blend = m, window, cx);
                                },
                            ))
                        })),
                )
                .into_any_element(),
        )
    }

    /// 「描边」「阴影」两组。
    pub(crate) fn render_stroke_shadow(&self, cx: &Context<'_, Self>) -> Vec<AnyElement> {
        let Some((_, look)) = self.bound_look() else {
            return Vec::new();
        };
        let stroke_body = v_flex()
            .gap_2()
            .children(look.stroke.map(|s| {
                Self::swatch_row(
                    "stroke-color",
                    s.color,
                    |l, c| {
                        if let Some(s) = &mut l.stroke {
                            s.color = c;
                        }
                    },
                    cx,
                )
            }))
            .children(
                look.stroke
                    .and_then(|_| self.look_slider_row(LookField::StrokeWidth, &look, cx)),
            )
            .into_any_element();
        let stroke = self.render_group(
            crate::inspector_group::Group {
                id: "look-stroke",
                title: "描边",
                check: Some((look.stroke.is_some(), |this, on, window, cx| {
                    this.edit_look(|l| l.stroke = on.then(Stroke::default), window, cx);
                })),
                reset: Some(|this, window, cx| {
                    this.reset_look(
                        |l| l.stroke = l.stroke.map(|_| Stroke::default()),
                        &Prop::STROKE,
                        window,
                        cx,
                    );
                }),
                extra: None,
            },
            stroke_body,
            cx,
        );
        let shadow_rows = [
            LookField::ShadowOpacity,
            LookField::ShadowBlur,
            LookField::ShadowDistance,
            LookField::ShadowAngle,
        ];
        let shadow_body = v_flex()
            .gap_2()
            .children(look.shadow.map(|s| {
                Self::swatch_row(
                    "shadow-color",
                    s.color,
                    |l, c| {
                        if let Some(s) = &mut l.shadow {
                            s.color = c;
                        }
                    },
                    cx,
                )
            }))
            .children(
                shadow_rows
                    .iter()
                    .filter(|_| look.shadow.is_some())
                    .filter_map(|&f| self.look_slider_row(f, &look, cx)),
            )
            .into_any_element();
        let shadow = self.render_group(
            crate::inspector_group::Group {
                id: "look-shadow",
                title: "阴影",
                check: Some((look.shadow.is_some(), |this, on, window, cx| {
                    this.edit_look(|l| l.shadow = on.then(Shadow::default), window, cx);
                })),
                reset: Some(|this, window, cx| {
                    this.reset_look(
                        |l| l.shadow = l.shadow.map(|_| Shadow::default()),
                        &Prop::SHADOW,
                        window,
                        cx,
                    );
                }),
                extra: None,
            },
            shadow_body,
            cx,
        );
        vec![stroke, shadow]
    }

    /// 「抠像」分段：色度抠图。
    pub(crate) fn render_key_seg(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let (_, look) = self.bound_look()?;
        let picking = self.inspector.look.picking;
        let body = v_flex()
            .gap_2()
            .children(look.chroma.map(|k| {
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .w(px(64.))
                            .flex_none()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("取色器"),
                    )
                    .child(
                        div()
                            .id("key-current")
                            .test_support()
                            .size(px(18.))
                            .rounded(px(3.))
                            .bg(rgb(k.color))
                            .border_1()
                            .border_color(cx.theme().border),
                    )
                    .child(
                        Button::new("key-pick")
                            .compact()
                            .when(picking, ButtonVariants::primary)
                            .when(!picking, Button::outline)
                            .label(if picking {
                                "在播放器上点一下取色"
                            } else {
                                "取色"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.inspector.look.picking = !this.inspector.look.picking;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("key-green")
                            .ghost()
                            .compact()
                            .label("绿幕")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_look(|l| set_key_color(l, 0x00_FF00), window, cx);
                            })),
                    )
                    .child(
                        Button::new("key-blue")
                            .ghost()
                            .compact()
                            .label("蓝幕")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_look(|l| set_key_color(l, 0x00_00FF), window, cx);
                            })),
                    )
            }))
            .children(
                [LookField::KeyIntensity, LookField::KeyShadow]
                    .iter()
                    .filter(|_| look.chroma.is_some())
                    .filter_map(|&f| self.look_slider_row(f, &look, cx)),
            )
            .into_any_element();
        Some(self.render_group(
            crate::inspector_group::Group {
                id: "look-key",
                title: "色度抠图",
                check: Some((look.chroma.is_some(), |this, on, window, cx| {
                    this.edit_look(|l| l.chroma = on.then(ChromaKey::default), window, cx);
                })),
                reset: Some(|this, window, cx| {
                    this.reset_look(
                        |l| l.chroma = l.chroma.map(|_| ChromaKey::default()),
                        &Prop::KEY,
                        window,
                        cx,
                    );
                }),
                extra: None,
            },
            body,
            cx,
        ))
    }

    /// 形状一排：无 + 各种形状（复合片段整组的蒙版不支持文字形状）。
    fn render_mask_shapes(
        &self,
        clip: ClipId,
        current: Option<MaskShape>,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        let compound = self
            .doc
            .project
            .timeline
            .find_clip(clip)
            .is_some_and(|(_, c)| c.compound().is_some());
        let shapes = std::iter::once((None, "无"))
            .chain(MaskShape::ALL.map(|s| (Some(s), s.label())))
            .filter(|(shape, _)| !(compound && *shape == Some(MaskShape::Text)))
            .map(|(shape, label)| {
                let id = SharedString::from(format!(
                    "mask-{}",
                    shape.map_or_else(|| "none".to_owned(), |s| s.code().to_string())
                ));
                crate::chrome::chip(id, label, current == shape, cx)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.edit_look(|l| l.mask = shape.map(Mask::new), window, cx);
                        this.fill_mask_text(window, cx);
                    }))
                    .into_any_element()
            });
        h_flex()
            .flex_wrap()
            .gap_1()
            .children(shapes)
            .into_any_element()
    }

    /// 文字蒙版的字：一行输入框（回车或失焦写回）。
    fn render_mask_text_row(&self, cx: &Context<'_, Self>) -> AnyElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w(px(52.))
                    .flex_none()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("文字"),
            )
            .child(
                div()
                    .id("mask-text-input")
                    .test_support()
                    .flex_1()
                    .child(Input::new(&self.inspector.look.mask_text).small()),
            )
            .into_any_element()
    }

    /// 「蒙版」分段：形状一排（无 + 7 种，复合片段没有文字），选了形状后下面是反转和各项数值。
    pub(crate) fn render_mask_seg(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let (clip, look) = self.bound_look()?;
        let mut rows: Vec<AnyElement> = self.render_mask_tabs(cx).into_iter().collect();
        rows.push(self.render_mask_shapes(clip, look.mask.map(|m| m.shape), cx));
        // 还没有蒙版：「添加蒙版」也能加一个
        if look.mask.is_none() {
            rows.push(
                h_flex()
                    .child(
                        crate::chrome::chip("mask-add", "⊕ 添加蒙版", false, cx)
                            .on_click(cx.listener(|this, _, window, cx| this.add_mask(window, cx))),
                    )
                    .into_any_element(),
            );
        }
        if look.mask.is_some_and(|m| m.shape == MaskShape::Text) {
            rows.push(self.render_mask_text_row(cx));
        }
        if let Some(m) = look.mask {
            rows.push(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("mask-invert")
                            .compact()
                            .when(m.invert, ButtonVariants::primary)
                            .when(!m.invert, Button::outline)
                            .label("反转")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_look(
                                    |l| {
                                        if let Some(m) = &mut l.mask {
                                            m.invert = !m.invert;
                                        }
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new("mask-reset")
                            .ghost()
                            .compact()
                            .label("↻")
                            .tooltip("重置蒙版")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.reset_look(
                                    |l| l.mask = l.mask.map(|m| Mask::new(m.shape)),
                                    &Prop::MASK,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .into_any_element(),
            );
            let mut fields = vec![LookField::MaskX, LookField::MaskY];
            if !matches!(m.shape, MaskShape::Linear) {
                if !matches!(m.shape, MaskShape::Mirror) {
                    fields.push(LookField::MaskWidth);
                }
                fields.push(LookField::MaskHeight);
            }
            fields.extend([LookField::MaskRotation, LookField::MaskFeather]);
            if m.shape == MaskShape::Rectangle {
                fields.push(LookField::MaskRound);
            }
            rows.extend(
                fields
                    .into_iter()
                    .filter_map(|f| self.look_slider_row(f, &look, cx)),
            );
        }
        Some(
            v_flex()
                .id("insp-mask")
                .test_support()
                .gap_2()
                .children(rows)
                .into_any_element(),
        )
    }
}

/// 原片 `path` 在 `at` 那一帧、纹理坐标 `uv` 处的颜色 0xRRGGBB。
#[expect(clippy::many_single_char_names, reason = "坐标和颜色分量")]
fn sample_source(path: &std::path::Path, at: Time, uv: (f32, f32)) -> Option<u32> {
    let target = ve_media::ScaleTarget {
        max_width: 640,
        max_height: 640,
    };
    let mut d = ve_media::VideoDecoder::open(path, target).ok()?;
    let f = d.seek_to(at).ok()?;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "像素坐标"
    )]
    let (x, y) = (
        (uv.0 * f.width as f32).clamp(0.0, f.width as f32 - 1.0) as u32,
        (uv.1 * f.height as f32).clamp(0.0, f.height as f32 - 1.0) as u32,
    );
    let i = ((y * f.width + x) * 4) as usize;
    let [b, g, r, _] = *f.data.get(i..i + 4)?.as_array::<4>()?;
    Some((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b))
}

/// 换抠图颜色（保留强度和阴影）。
fn set_key_color(l: &mut Look, color: u32) {
    l.chroma = Some(ChromaKey {
        color,
        ..l.chroma.unwrap_or_default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip_through_ui_units() {
        let mut l = Look {
            stroke: Some(Stroke::default()),
            shadow: Some(Shadow::default()),
            chroma: Some(ChromaKey::default()),
            mask: Some(Mask::new(MaskShape::Rectangle)),
            ..Look::NONE
        };
        for f in LookField::ALL {
            let (min, max) = f.range();
            let ui = f32::midpoint(min, max).round();
            f.set(&mut l, f.ui_to_value(ui));
            assert_eq!(f.get(&l).map(|v| f.value_to_ui(v)), Some(ui), "{f:?}");
        }
        assert!(l.is_valid(), "{l:?}");
        // 没开的项：读不到、写了也不生效
        let mut off = Look::NONE;
        LookField::StrokeWidth.set(&mut off, 0.05);
        assert_eq!(LookField::StrokeWidth.get(&off), None);
        assert_eq!(off, Look::NONE);
    }
}
