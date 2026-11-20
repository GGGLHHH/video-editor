//! 快捷键一览（剪映顶栏的 ⌨）：按「时间线 / 播放器 / 基础 / 鼠标」分组列出所有快捷键；
//! 点一下某个键、再按新的键就改掉（[`crate::keymap`]）。

use crate::chrome;
use crate::theme::palette;
use crate::workspace::{Popup, Workspace};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, deferred, div, px,
};

/// 一项：说明、键（几种按法之间是「或」；鼠标操作写成文字）。
type Item = (&'static str, &'static [&'static str]);

/// 分组：组名、各项。键的写法和菜单一样（`Ctrl+B`），测试会核对每个键都真的绑定了。
pub(crate) const GROUPS: [(&str, &[Item]); 4] = [
    (
        "时间线",
        &[
            ("分割", &["Ctrl+B"]),
            ("鼠标选择模式 / 鼠标分割模式", &["A", "B"]),
            ("滑移 / 滑动 / 滚动修剪", &["Y", "U", "R"]),
            ("预览轴开关", &["S"]),
            ("批量分割（所有轨）", &["Ctrl+Shift+B"]),
            ("向左裁剪 / 向右裁剪", &["Q", "W"]),
            ("向左全选 / 向右全选", &["[", "]"]),
            ("删除", &["Delete", "Backspace"]),
            ("波纹删除", &["Shift+Delete"]),
            ("停用片段", &["V"]),
            ("新建 / 解除复合片段", &["Alt+G", "Alt+Shift+G"]),
            ("添加 / 去掉标记", &["M"]),
            ("下一个 / 上一个标记", &["Shift+M", "Alt+Shift+M"]),
            ("区域入点 / 出点", &["I", "O"]),
            ("以片段选定区域 / 取消选定区域", &["Shift+X", "Alt+X"]),
            ("播放出入点区域", &["Shift+Enter"]),
            ("加字幕", &["T"]),
            ("主轨磁吸 / 自动吸附 / 联动", &["P", "N", "`"]),
            ("放大 / 缩小时间线", &["Ctrl+=", "Ctrl+-"]),
            ("全局预览缩放", &["Shift+Z"]),
            ("全选 / 取消全选", &["Ctrl+A", "Ctrl+Shift+A"]),
        ],
    ),
    (
        "播放器",
        &[
            ("播放 / 暂停", &["空格"]),
            ("倒放 / 停止 / 正放（连按加速）", &["J", "K", "L"]),
            ("上一帧 / 下一帧", &["←", "→"]),
            ("跳到开头 / 结尾", &["Home", "End"]),
            ("全屏预览", &["Ctrl+Shift+F"]),
            ("退出全屏 / 取消拖动 / 取消选择", &["Esc"]),
        ],
    ),
    (
        "基础",
        &[
            ("撤销", &["Ctrl+Z"]),
            ("恢复", &["Ctrl+Y", "Ctrl+Shift+Z"]),
            ("复制 / 剪切 / 粘贴", &["Ctrl+C", "Ctrl+X", "Ctrl+V"]),
            ("复制属性 / 粘贴属性", &["Ctrl+Shift+C", "Ctrl+Shift+V"]),
            ("新建 / 打开 / 导入", &["Ctrl+N", "Ctrl+O", "Ctrl+I"]),
            ("保存 / 另存为", &["Ctrl+S", "Ctrl+Shift+S"]),
            ("导出", &["Ctrl+E"]),
        ],
    ),
    (
        "鼠标",
        &[
            ("滚轮：上下滚轨道（轨道放得下时左右滚）", &[]),
            ("Alt / Shift + 滚轮：左右滚", &[]),
            ("Ctrl + 滚轮：以鼠标处为中心缩放", &[]),
            ("拖动时按住 Alt：临时关掉 / 打开吸附", &[]),
            ("Ctrl 点击：加选 / 减选；Shift 点击：选一片", &[]),
        ],
    ),
];

impl Workspace {
    /// 顶栏的 ⌨ 按钮和展开的一览。
    pub(crate) fn render_shortcuts_button(&self, cx: &Context<'_, Self>) -> AnyElement {
        div()
            .relative()
            .child(
                Button::new("tb-shortcuts")
                    .ghost()
                    .compact()
                    .icon(Icon::new(Lucide::Keyboard))
                    .tooltip("快捷键")
                    .on_click(cx.listener(|this, e: &gpui_kit::ClickEvent, _, cx| {
                        this.click_popup_button(Popup::Shortcuts, e);
                        cx.notify();
                    })),
            )
            .children(
                self.popup_open(Popup::Shortcuts)
                    .then(|| deferred(self.render_shortcuts(cx)).with_priority(1)),
            )
            .into_any_element()
    }

    /// 改键后的提示（测试也看它）。
    #[must_use]
    pub fn shortcut_note(&self) -> Option<&str> {
        self.shortcut_note.as_deref()
    }

    /// 按下了新键（`key` 是 GPUI 写法）：Esc 取消，别的键换上去。
    pub(crate) fn finish_recording(&mut self, key: &str, cx: &mut Context<'_, Self>) {
        let Some(slot) = self.recording_key.take() else {
            return;
        };
        self.shortcut_note = if key == "escape" {
            None
        } else {
            crate::keymap::assign(cx, &slot, key).map(|_| {
                format!(
                    "{} 原来用于另一项操作，已经从那里去掉",
                    crate::keymap::to_shown(key)
                )
            })
        };
        cx.notify();
    }

    /// 一览里的一个键：点一下开始改；正在改时显示「请按键…」。
    fn shortcut_key(&self, shown: &'static str, cx: &Context<'_, Self>) -> AnyElement {
        let default = crate::keymap::to_binding(shown);
        let Some(slot) = crate::keymap::slot_for_default(cx, &default) else {
            return chrome::keycaps(shown, cx).into_any_element();
        };
        let current = crate::keymap::current(cx, &slot);
        let recording = self.recording_key.as_deref() == Some(slot.as_str());
        let changed = current != default;
        let body = if recording {
            div()
                .px_1()
                .text_size(px(10.))
                .text_color(palette::accent())
                .child("请按键…")
                .into_any_element()
        } else {
            chrome::keycaps(&crate::keymap::to_shown(&current), cx).into_any_element()
        };
        div()
            .id(SharedString::from(format!("sc-{default}")))
            .test_support()
            .rounded(px(3.))
            .border_1()
            .border_color(if recording || changed {
                palette::accent()
            } else {
                gpui_kit::transparent_black()
            })
            .cursor_pointer()
            .hover(|s| s.bg(palette::menu_hover()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.recording_key = Some(slot.clone());
                this.shortcut_note = None;
                cx.notify();
            }))
            .child(body)
            .into_any_element()
    }

    fn render_shortcuts(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let column =
            |(group, items): (&'static str, &'static [Item])| {
                v_flex()
                    .flex_1()
                    .min_w(px(300.))
                    .gap_1()
                    .child(
                        div()
                            .pb_1()
                            .text_sm()
                            .text_color(palette::accent())
                            .child(group),
                    )
                    .children(items.iter().map(|&(label, keys)| {
                        h_flex()
                            .min_h(px(24.))
                            .gap_3()
                            .justify_between()
                            .text_xs()
                            .child(div().text_color(muted).child(label))
                            .child(h_flex().gap_1().children(keys.iter().enumerate().map(
                                |(i, &k)| {
                                    h_flex()
                                        .gap_1()
                                        .children(
                                            (i > 0).then(|| div().text_color(muted).child("/")),
                                        )
                                        .child(self.shortcut_key(k, cx))
                                },
                            )))
                    }))
            };
        v_flex()
            .id("shortcuts")
            // 盖住下面的东西：点菜单不会顺带点到下面的画面 / 时间轴
            .occlude()
            .test_support()
            .absolute()
            .top(px(30.))
            .right_0()
            .w(px(680.))
            .max_h(px(820.))
            .overflow_y_scroll()
            .p_3()
            .gap_3()
            .bg(palette::menu_bg())
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(chrome::CARD_RADIUS))
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, e: &gpui_kit::MouseDownEvent, _, cx| {
                this.dismiss_popup(e.position);
                cx.notify();
            }))
            .child(
                h_flex()
                    .gap_3()
                    .child(div().text_sm().child("快捷键"))
                    .child(div().flex_1().text_xs().text_color(muted).child(
                        self.shortcut_note.clone().unwrap_or_else(|| {
                            "点一下按键、再按新的键就能改（Esc 取消）".to_owned()
                        }),
                    ))
                    .child(
                        Button::new("shortcuts-reset")
                            .ghost()
                            .compact()
                            .label("恢复默认")
                            .disabled(!crate::keymap::customized(cx))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.recording_key = None;
                                this.shortcut_note = None;
                                crate::keymap::reset(cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .items_start()
                    .gap_4()
                    .child(column(GROUPS[0]))
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_3()
                            .child(column(GROUPS[1]))
                            .child(column(GROUPS[2])),
                    ),
            )
            .child(column(GROUPS[3]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::keymap::to_binding as binding;

    #[test]
    fn every_listed_key_is_really_bound() {
        let source = include_str!("actions.rs");
        for (group, items) in GROUPS {
            for (label, keys) in items {
                for k in *keys {
                    let bound = format!("KeyBinding::new(\"{}\"", binding(k));
                    assert!(source.contains(&bound), "{group}「{label}」的 {k} 没有绑定");
                }
            }
        }
        assert_eq!(binding("Ctrl+Shift+B"), "ctrl-shift-b");
        assert_eq!(binding("Ctrl+-"), "ctrl--");
    }
}
