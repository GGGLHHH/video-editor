//! 属性面板「文本」标签：字幕样式（剪映的「文本、排列、气泡、花字应用到全部字幕」默认勾选，
//! 这里所有字幕共用一套样式）。预设、字体、字号、粗体、描边、离底高度都是点选。

use crate::chrome;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div,
};
use ve_core::SubtitleStyle;
use ve_core::edit::set_subtitle_style;

/// 预设：（编号，名字，文字色，描边色，描边宽）。
const PRESETS: [(&str, &str, u32, u32, f32); 6] = [
    ("white-black", "白字黑边", 0xFF_FFFF, 0x00_0000, 2.5),
    ("black-white", "黑字白边", 0x00_0000, 0xFF_FFFF, 2.5),
    ("yellow-black", "黄字黑边", 0xFF_D700, 0x00_0000, 2.5),
    ("red-white", "红字白边", 0xE5_3935, 0xFF_FFFF, 2.5),
    ("cyan-black", "青字黑边", 0x00_C8D2, 0x00_0000, 2.5),
    ("white-plain", "白字无边", 0xFF_FFFF, 0x00_0000, 0.0),
];
/// 字体：（编号，名字，字体名）。
const FONTS: [(&str, &str, &str); 4] = [
    ("yahei", "微软雅黑", "Microsoft YaHei"),
    ("hei", "黑体", "SimHei"),
    ("song", "宋体", "SimSun"),
    ("kai", "楷体", "KaiTi"),
];
/// 字号（画面高的比例）。
const SIZES: [(&str, &str, f32); 4] = [
    ("s", "小", 0.04),
    ("m", "中", 0.055),
    ("l", "大", 0.07),
    ("xl", "特大", 0.09),
];
/// 描边宽度。
const OUTLINES: [(&str, &str, f32); 4] = [
    ("none", "无", 0.0),
    ("thin", "细", 1.5),
    ("mid", "中", 2.5),
    ("thick", "粗", 4.0),
];
/// 离底高度（画面高的比例）。
const MARGINS: [(&str, &str, f32); 3] = [
    ("low", "靠下", 0.05),
    ("mid", "中下", 0.15),
    ("high", "居中", 0.45),
];

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

impl Workspace {
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
                let mut s = this.doc.project.subtitle_style.clone();
                change(&mut s);
                this.apply(window, cx, |p| set_subtitle_style(p, s));
            }))
            .into_any_element()
    }

    pub(crate) fn render_text_style_tab(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let s = &self.doc.project.subtitle_style;
        let group = |label: &'static str, chips: Vec<AnyElement>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().text_color(muted).child(label))
                .child(h_flex().flex_wrap().gap_1().children(chips))
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
        let fonts = FONTS
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
            .chain(std::iter::once(Self::style_chip(
                "ts-bold".into(),
                "粗体",
                s.bold,
                |st| st.bold = !st.bold,
                cx,
            )))
            .collect();
        let outlines = OUTLINES
            .iter()
            .map(|&(id, label, w)| {
                Self::style_chip(
                    format!("ts-outline-{id}"),
                    label,
                    close(s.outline, w),
                    move |st| st.outline = w,
                    cx,
                )
            })
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
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("样式应用到全部字幕"),
            )
            .child(group("预设", presets))
            .child(group("字体", fonts))
            .child(group("字号", sizes))
            .child(group("描边", outlines))
            .child(group("位置", margins))
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
        for &(.., w) in &OUTLINES {
            assert!(
                SubtitleStyle {
                    outline: w,
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
}
