//! 属性面板「文本」标签：字幕样式（剪映的文本面板）。默认「应用到全部字幕」：所有字幕共用一套样式；
//! 关掉后这一条字幕有自己的样式。
//!
//! 预设；字体（常用几种 + 系统里的全部字体）；字号（快捷几档 + 滑条）；粗体 / 斜体 / 下划线；
//! 颜色（色块 + 取色器）、不透明度、字间距、位置；描边、阴影、背景框各一组（颜色 + 滑条）。

use crate::chrome;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, px,
};
use ve_core::SubtitleStyle;
use ve_core::edit::{set_own_subtitle_style, set_subtitle_style};

/// 预设：（编号，名字，文字色，描边色，描边宽）。
const PRESETS: [(&str, &str, u32, u32, f32); 6] = [
    ("white-black", "白字黑边", 0xFF_FFFF, 0x00_0000, 2.5),
    ("black-white", "黑字白边", 0x00_0000, 0xFF_FFFF, 2.5),
    ("yellow-black", "黄字黑边", 0xFF_D700, 0x00_0000, 2.5),
    ("red-white", "红字白边", 0xE5_3935, 0xFF_FFFF, 2.5),
    ("cyan-black", "青字黑边", 0x00_C8D2, 0x00_0000, 2.5),
    ("white-plain", "白字无边", 0xFF_FFFF, 0x00_0000, 0.0),
];
/// 常用字体：（编号，名字，字体名）；别的从「全部字体」里选。
const FONTS: [(&str, &str, &str); 4] = [
    ("yahei", "微软雅黑", "Microsoft YaHei"),
    ("hei", "黑体", "SimHei"),
    ("song", "宋体", "SimSun"),
    ("kai", "楷体", "KaiTi"),
];
/// 字号快捷档（画面高的比例）。
const SIZES: [(&str, &str, f32); 4] = [
    ("s", "小", 0.04),
    ("m", "中", 0.055),
    ("l", "大", 0.07),
    ("xl", "特大", 0.09),
];
/// 离底高度（画面高的比例）。
const MARGINS: [(&str, &str, f32); 3] = [
    ("low", "靠下", 0.05),
    ("mid", "中下", 0.15),
    ("high", "居中", 0.45),
];
/// 色块（剪映色板第一行）。
const SWATCHES: [u32; 8] = [
    0xFF_FFFF, 0x00_0000, 0xE5_484D, 0xFF_C53D, 0x46_A758, 0x12_A594, 0x3E_63DD, 0x8E_4EC6,
];

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// 文字样式里用滑条调的几项。界面上的数：字号、字间距、描边、阴影按 1080 高的像素，不透明度按百分比。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextField {
    Size,
    Opacity,
    Spacing,
    Outline,
    Shadow,
    ShadowOpacity,
    BackgroundOpacity,
}

impl TextField {
    const ALL: [Self; 7] = [
        Self::Size,
        Self::Opacity,
        Self::Spacing,
        Self::Outline,
        Self::Shadow,
        Self::ShadowOpacity,
        Self::BackgroundOpacity,
    ];

    const fn id(self) -> &'static str {
        match self {
            Self::Size => "ts-size-slider",
            Self::Opacity => "ts-opacity",
            Self::Spacing => "ts-spacing",
            Self::Outline => "ts-outline-width",
            Self::Shadow => "ts-shadow",
            Self::ShadowOpacity => "ts-shadow-opacity",
            Self::BackgroundOpacity => "ts-bg-opacity",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Size => "字号",
            Self::Opacity | Self::ShadowOpacity | Self::BackgroundOpacity => "不透明度",
            Self::Spacing => "字间距",
            Self::Outline => "粗细",
            Self::Shadow => "距离",
        }
    }

    /// 界面上的范围。
    fn range(self) -> (f32, f32) {
        let px1080 = |(a, b): (f32, f32)| (a * 1080.0, b * 1080.0);
        match self {
            Self::Size => px1080(SubtitleStyle::SIZE_RANGE),
            Self::Opacity | Self::ShadowOpacity | Self::BackgroundOpacity => (0.0, 100.0),
            Self::Spacing => SubtitleStyle::SPACING_RANGE,
            Self::Outline => SubtitleStyle::OUTLINE_RANGE,
            Self::Shadow => SubtitleStyle::SHADOW_RANGE,
        }
    }

    /// 样式里的值 → 界面上的数（整数）。
    fn get(self, s: &SubtitleStyle) -> f32 {
        let v = match self {
            Self::Size => s.size_ratio * 1080.0,
            Self::Opacity => s.opacity * 100.0,
            Self::Spacing => s.spacing,
            Self::Outline => s.outline,
            Self::Shadow => s.shadow,
            Self::ShadowOpacity => s.shadow_opacity * 100.0,
            Self::BackgroundOpacity => s.background_opacity * 100.0,
        };
        v.round()
    }

    /// 界面上的数 → 写回样式（夹在范围里）。
    fn set(self, s: &mut SubtitleStyle, ui: f32) {
        let (lo, hi) = self.range();
        let v = ui.clamp(lo, hi);
        match self {
            Self::Size => s.size_ratio = v / 1080.0,
            Self::Opacity => s.opacity = v / 100.0,
            Self::Spacing => s.spacing = v,
            Self::Outline => s.outline = v,
            Self::Shadow => s.shadow = v,
            Self::ShadowOpacity => s.shadow_opacity = v / 100.0,
            Self::BackgroundOpacity => s.background_opacity = v / 100.0,
        }
    }
}

/// 文字样式里的几种颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorTarget {
    Text,
    Outline,
    Shadow,
    Background,
}

impl ColorTarget {
    const ALL: [Self; 4] = [Self::Text, Self::Outline, Self::Shadow, Self::Background];

    const fn id(self) -> &'static str {
        match self {
            Self::Text => "ts-color",
            Self::Outline => "ts-outline-color",
            Self::Shadow => "ts-shadow-color",
            Self::Background => "ts-bg-color",
        }
    }

    const fn get(self, s: &SubtitleStyle) -> u32 {
        match self {
            Self::Text => s.color,
            Self::Outline => s.outline_color,
            Self::Shadow => s.shadow_color,
            Self::Background => s.background_color,
        }
    }

    fn set(self, s: &mut SubtitleStyle, rgb: u32) {
        match self {
            Self::Text => s.color = rgb,
            Self::Outline => s.outline_color = rgb,
            Self::Shadow => s.shadow_color = rgb,
            Self::Background => {
                s.background_color = rgb;
                // 选了背景色就让背景显出来（剪映：勾上背景）
                if s.background_opacity <= 0.0 {
                    s.background_opacity = 1.0;
                }
            }
        }
    }
}

/// 0xRRGGBB → 颜色。
fn hsla(rgb: u32) -> Hsla {
    gpui_kit::rgb(rgb).into()
}

/// 颜色 → 0xRRGGBB。
fn rgb_of(c: Hsla) -> u32 {
    let c = gpui_kit::Rgba::from(c);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "0–255"
    )]
    let byte = |v: f32| u32::from((v.clamp(0.0, 1.0) * 255.0).round() as u8);
    (byte(c.r) << 16) | (byte(c.g) << 8) | byte(c.b)
}

/// 「文本」标签的滑条、取色器和字体下拉框。
pub(crate) struct TextStyleFields {
    sliders: Vec<(TextField, Entity<SliderState>)>,
    pickers: Vec<(ColorTarget, Entity<ColorPickerState>)>,
    font: Entity<SelectState<Vec<SharedString>>>,
    /// 正在拖滑条（一次拖动一步撤销）。
    gesture: bool,
}

impl std::fmt::Debug for TextStyleFields {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextStyleFields")
            .field("gesture", &self.gesture)
            .finish_non_exhaustive()
    }
}

impl TextStyleFields {
    /// 建好控件并订阅。
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<'_, Workspace>,
    ) -> (Self, Vec<Subscription>) {
        let mut subs = Vec::new();
        let sliders = TextField::ALL
            .iter()
            .map(|&f| {
                let (min, max) = f.range();
                let s = cx.new(|_| {
                    SliderState::new()
                        .min(min)
                        .max(max)
                        .step(1.0)
                        .default_value(f.get(&SubtitleStyle::default()))
                });
                subs.push(cx.subscribe_in(
                    &s,
                    window,
                    move |this: &mut Workspace, _, e: &SliderEvent, window, cx| {
                        this.on_text_slider(f, e, window, cx);
                    },
                ));
                (f, s)
            })
            .collect();
        let pickers = ColorTarget::ALL
            .iter()
            .map(|&t| {
                let state = cx.new(|cx| {
                    ColorPickerState::new(window, cx)
                        .default_value(hsla(t.get(&SubtitleStyle::default())))
                });
                subs.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this: &mut Workspace, _, e: &ColorPickerEvent, window, cx| {
                        if let ColorPickerEvent::Change(Some(c)) = e {
                            this.edit_text_style(|s| t.set(s, rgb_of(*c)), window, cx);
                        }
                    },
                ));
                (t, state)
            })
            .collect();
        let fonts: Vec<SharedString> = cx
            .text_system()
            .all_font_names()
            .into_iter()
            .filter(|n| !n.starts_with('.') && !n.starts_with('@'))
            .map(SharedString::from)
            .collect();
        let font = cx.new(|cx| SelectState::new(fonts, None, window, cx).searchable(true));
        subs.push(cx.subscribe_in(
            &font,
            window,
            |this: &mut Workspace, _, e: &SelectEvent<Vec<SharedString>>, window, cx| {
                if let SelectEvent::Confirm(Some(name)) = e {
                    let name = name.to_string();
                    this.edit_text_style(move |s| s.font = name, window, cx);
                }
            },
        ));
        (
            Self {
                sliders,
                pickers,
                font,
                gesture: false,
            },
            subs,
        )
    }

    fn slider(&self, f: TextField) -> Option<&Entity<SliderState>> {
        self.sliders.iter().find(|(g, _)| *g == f).map(|(_, s)| s)
    }

    fn picker(&self, t: ColorTarget) -> Option<&Entity<ColorPickerState>> {
        self.pickers.iter().find(|(g, _)| *g == t).map(|(_, s)| s)
    }
}

impl Workspace {
    /// 选中的字幕有自己的样式时：（片段，样式）。
    fn bound_own_style(&self) -> Option<(ve_core::ClipId, SubtitleStyle)> {
        let clip = self.inspector.bound?;
        let (_, c) = self.doc.project.timeline.find_clip(clip)?;
        Some((clip, c.own_subtitle_style()?.clone()))
    }

    /// 现在面板改的那份样式：这一条自己的，或者共用的。
    fn text_style_in_use(&self) -> SubtitleStyle {
        self.bound_own_style()
            .map_or_else(|| self.doc.project.subtitle_style.clone(), |(_, s)| s)
    }

    /// 把改好的样式写回（「应用到全部字幕」关着时只写这一条）。`gesture` 时不另记撤销、不弹错误。
    fn write_text_style(
        &mut self,
        s: SubtitleStyle,
        gesture: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let own = self.bound_own_style().map(|(clip, _)| clip);
        let edit = move |p: &mut ve_core::Project| match own {
            Some(clip) => set_own_subtitle_style(p, &[clip], Some(&s)),
            None => set_subtitle_style(p, s),
        };
        if gesture {
            let _ = self.doc.edit(edit);
            self.after_change(window, cx);
        } else {
            self.apply(window, cx, edit);
        }
    }

    /// 改一下样式（一步撤销）。
    fn edit_text_style(
        &mut self,
        change: impl FnOnce(&mut SubtitleStyle),
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let mut s = self.text_style_in_use();
        change(&mut s);
        self.write_text_style(s, false, window, cx);
    }

    fn on_text_slider(
        &mut self,
        f: TextField,
        e: &SliderEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match e {
            SliderEvent::Change(v) => {
                if !self.inspector.text_style.gesture {
                    self.doc.begin_gesture();
                    self.inspector.text_style.gesture = true;
                }
                let mut s = self.text_style_in_use();
                f.set(&mut s, v.start());
                self.write_text_style(s, true, window, cx);
            }
            SliderEvent::Release(_) => {
                if self.inspector.text_style.gesture {
                    self.doc.end_gesture();
                    self.inspector.text_style.gesture = false;
                }
            }
        }
    }

    /// 滑条、取色器跟着现在的样式走（换了字幕、撤销、点了预设之后）；正在拖的时候不动。
    pub(crate) fn refresh_text_style_controls(
        &self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.inspector.text_style.gesture
            || self.inspector.tab != crate::inspector::InspectorTab::Text
        {
            return;
        }
        let s = self.text_style_in_use();
        for (f, slider) in &self.inspector.text_style.sliders {
            let v = f.get(&s);
            if (slider.read(cx).value().start() - v).abs() > 0.5 {
                slider.update(cx, |sl, cx| sl.set_value(v, window, cx));
            }
        }
        for (t, picker) in &self.inspector.text_style.pickers {
            let want = t.get(&s);
            let shown = picker.read(cx).value().map(rgb_of);
            if shown != Some(want) && !picker.read(cx).is_open() {
                picker.update(cx, |p, cx| p.set_value(hsla(want), window, cx));
            }
        }
    }

    /// 一个选项：点了就按 `change` 改样式（一步撤销）。
    fn style_chip(
        id: String,
        label: &'static str,
        on: bool,
        change: impl Fn(&mut SubtitleStyle) + 'static,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        chrome::chip(id, label, on, cx)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.edit_text_style(&change, window, cx);
            }))
            .into_any_element()
    }

    /// 「应用到全部字幕」开关：开着时改的是所有字幕共用的样式；关掉后这一条有自己的样式（从共用的复制一份）。
    fn toggle_apply_to_all(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(clip) = self.inspector.bound else {
            return;
        };
        let own = match self.bound_own_style() {
            Some(_) => None,
            None => Some(self.doc.project.subtitle_style.clone()),
        };
        self.apply(window, cx, |p| {
            set_own_subtitle_style(p, &[clip], own.as_ref())
        });
    }

    /// 一行：名字、滑条、数值。
    fn text_slider_row(
        &self,
        f: TextField,
        s: &SubtitleStyle,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        let Some(slider) = self.inspector.text_style.slider(f) else {
            return div().into_any_element();
        };
        h_flex()
            .gap_2()
            .child(
                div()
                    .w(px(56.))
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
                    .child(Slider::new(slider)),
            )
            .child(
                div()
                    .w(px(36.))
                    .text_sm()
                    .child(SharedString::from(format!("{}", f.get(s)))),
            )
            .into_any_element()
    }

    /// 一行颜色：色块 + 取色器（任意颜色）。
    fn text_color_row(
        &self,
        t: ColorTarget,
        s: &SubtitleStyle,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        // 没开背景时，背景那排一个都不标成「当前」
        let current =
            (!matches!(t, ColorTarget::Background) || s.has_background()).then(|| t.get(s));
        let dots = SWATCHES.iter().enumerate().map(|(i, &rgb)| {
            div()
                .id(SharedString::from(format!("{}-{i}", t.id())))
                .test_support()
                .size(px(18.))
                .rounded(px(3.))
                .bg(hsla(rgb))
                .border_2()
                .border_color(if current == Some(rgb) {
                    crate::theme::palette::accent()
                } else {
                    cx.theme().border
                })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.edit_text_style(|st| t.set(st, rgb), window, cx);
                }))
        });
        let picker = self.inspector.text_style.picker(t).map(|state| {
            div()
                .id(SharedString::from(format!("{}-picker", t.id())))
                .test_support()
                .child(ColorPicker::new(state).featured_colors(SWATCHES.map(hsla).to_vec()))
        });
        h_flex()
            .gap_1()
            .flex_wrap()
            .children(dots)
            .children(picker)
            .into_any_element()
    }

    #[expect(
        clippy::too_many_lines,
        reason = "一整块面板的布局，拆开反而难对照界面"
    )]
    pub(crate) fn render_text_style_tab(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let s = self.text_style_in_use();
        let all = self.bound_own_style().is_none();
        let apply_all = chrome::chip(
            "ts-apply-all",
            if all {
                "☑ 应用到全部字幕"
            } else {
                "☐ 应用到全部字幕"
            },
            all,
            cx,
        )
        .on_click(cx.listener(|this, _, window, cx| this.toggle_apply_to_all(window, cx)));
        let group = |label: &'static str, body: Vec<AnyElement>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().text_color(muted).child(label))
                .children(body)
        };
        let chips = |items: Vec<AnyElement>| {
            h_flex()
                .flex_wrap()
                .gap_1()
                .children(items)
                .into_any_element()
        };
        let presets = PRESETS
            .iter()
            .map(|&(id, label, color, edge, width)| {
                let on = s.color == color && s.outline_color == edge && close(s.outline, width);
                Self::style_chip(
                    format!("ts-preset-{id}"),
                    label,
                    on,
                    move |st| {
                        st.color = color;
                        st.outline_color = edge;
                        st.outline = width;
                    },
                    cx,
                )
            })
            .collect();
        let fonts: Vec<AnyElement> = FONTS
            .iter()
            .map(|&(id, label, family)| {
                Self::style_chip(
                    format!("ts-font-{id}"),
                    label,
                    s.font == family,
                    move |st| family.clone_into(&mut st.font),
                    cx,
                )
            })
            .collect();
        let all_fonts = div()
            .id("ts-font-all")
            .test_support()
            .child(
                Select::new(&self.inspector.text_style.font)
                    .placeholder(SharedString::from(format!("全部字体（现在：{}）", s.font)))
                    .search_placeholder("搜字体"),
            )
            .into_any_element();
        let sizes = SIZES
            .iter()
            .map(|&(id, label, r)| {
                Self::style_chip(
                    format!("ts-size-{id}"),
                    label,
                    close(s.size_ratio, r),
                    move |st| st.size_ratio = r,
                    cx,
                )
            })
            .collect();
        let looks = [
            (
                "ts-bold",
                "粗体",
                s.bold,
                (|st: &mut SubtitleStyle| st.bold = !st.bold) as fn(&mut SubtitleStyle),
            ),
            ("ts-italic", "斜体", s.italic, |st| st.italic = !st.italic),
            ("ts-underline", "下划线", s.underline, |st| {
                st.underline = !st.underline;
            }),
        ]
        .into_iter()
        .map(|(id, label, on, toggle)| Self::style_chip(id.into(), label, on, toggle, cx))
        .collect();
        let margins = MARGINS
            .iter()
            .map(|&(id, label, r)| {
                Self::style_chip(
                    format!("ts-pos-{id}"),
                    label,
                    close(s.margin_ratio, r),
                    move |st| st.margin_ratio = r,
                    cx,
                )
            })
            .collect();
        v_flex()
            .id("insp-text-style")
            .test_support()
            .gap_3()
            .child(h_flex().gap_2().child(apply_all).child(
                div().text_xs().text_color(muted).child(if all {
                    "改的是所有字幕共用的样式"
                } else {
                    "只改这一条字幕"
                }),
            ))
            .child(group("预设", vec![chips(presets)]))
            .child(group("字体", vec![chips(fonts), all_fonts]))
            .child(group(
                "字号",
                vec![chips(sizes), self.text_slider_row(TextField::Size, &s, cx)],
            ))
            .child(group("样式", vec![chips(looks)]))
            .child(group(
                "颜色",
                vec![
                    self.text_color_row(ColorTarget::Text, &s, cx),
                    self.text_slider_row(TextField::Opacity, &s, cx),
                    self.text_slider_row(TextField::Spacing, &s, cx),
                ],
            ))
            .child(group("位置", vec![chips(margins)]))
            .child(group(
                "描边",
                vec![
                    self.text_color_row(ColorTarget::Outline, &s, cx),
                    self.text_slider_row(TextField::Outline, &s, cx),
                ],
            ))
            .child(group(
                "阴影",
                vec![
                    self.text_slider_row(TextField::Shadow, &s, cx),
                    self.text_color_row(ColorTarget::Shadow, &s, cx),
                    self.text_slider_row(TextField::ShadowOpacity, &s, cx),
                ],
            ))
            .child(group(
                "背景",
                vec![
                    self.text_color_row(ColorTarget::Background, &s, cx),
                    self.text_slider_row(TextField::BackgroundOpacity, &s, cx),
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("不透明度调到 0 就没有背景；有背景时不画描边")
                        .into_any_element(),
                ],
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_option_is_a_valid_style() {
        for &(_, _, color, edge, width) in &PRESETS {
            let s = SubtitleStyle {
                color,
                outline_color: edge,
                outline: width,
                ..SubtitleStyle::default()
            };
            assert!(s.is_valid());
        }
        for &(.., r) in &SIZES {
            assert!(
                SubtitleStyle {
                    size_ratio: r,
                    ..SubtitleStyle::default()
                }
                .is_valid()
            );
        }
        for &(.., r) in &MARGINS {
            assert!(
                SubtitleStyle {
                    margin_ratio: r,
                    ..SubtitleStyle::default()
                }
                .is_valid()
            );
        }
        // 默认样式正好是「白字黑边 / 微软雅黑 / 中 / 靠下」
        let d = SubtitleStyle::default();
        assert_eq!(d.font, FONTS[0].2);
        assert!(close(d.size_ratio, SIZES[1].2) && close(d.margin_ratio, MARGINS[0].2));
    }

    #[test]
    fn every_slider_end_is_a_valid_style_and_round_trips() {
        for f in TextField::ALL {
            let (lo, hi) = f.range();
            for ui in [lo, hi, f32::midpoint(lo, hi)] {
                let mut s = SubtitleStyle::default();
                f.set(&mut s, ui);
                assert!(s.is_valid(), "{f:?} = {ui}");
                assert!(
                    (f.get(&s) - ui.round()).abs() <= 1.0,
                    "{f:?}: {} vs {ui}",
                    f.get(&s)
                );
            }
        }
        // 选背景色时背景显出来
        let mut s = SubtitleStyle::default();
        ColorTarget::Background.set(&mut s, 0x12_3456);
        assert!(s.has_background() && s.background_color == 0x12_3456);
    }

    #[test]
    fn colors_convert_both_ways() {
        for rgb in [0x00_0000, 0xFF_FFFF, 0xE5_484D, 0x12_A594] {
            assert_eq!(rgb_of(hsla(rgb)), rgb);
        }
    }
}
