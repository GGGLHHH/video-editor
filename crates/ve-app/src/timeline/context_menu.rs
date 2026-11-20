//! 时间轴片段的右键菜单（剪映）：复制 / 剪切 / 粘贴、复制 / 粘贴属性、删除；选中同色片段、
//! 8 个颜色标签色点 + 还原；分割、向左 / 右裁剪；停用片段、替换片段。右侧画快捷键键帽，用不了的项变灰。

use super::Hit;
use crate::actions as act;
use crate::chrome;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    Action, AnyElement, Context, InteractiveElement as _, IntoElement as _, MouseButton,
    MouseDownEvent, ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window,
    deferred, div, px,
};

/// 菜单里的一项：编号、文字、快捷键、动作、能不能用。
struct Item {
    id: &'static str,
    label: &'static str,
    keys: &'static str,
    action: fn() -> Box<dyn Action>,
    enabled: bool,
}

impl Workspace {
    /// 右键按下：点在片段上就（必要时）选中它并弹出菜单；点在别处收起菜单。
    pub(crate) fn timeline_right_down(
        &mut self,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.focus.focus(window, cx);
        let o = self.timeline.bounds.origin;
        let (x, y) = (f32::from(e.position.x - o.x), f32::from(e.position.y - o.y));
        let geo = self.timeline.geometry;
        self.timeline.context_menu = match geo.hit_test(x, y, &self.doc.project.timeline) {
            // 锁定轨道上的片段：不弹菜单（选不中，菜单会作用到别的选择上）
            Hit::Clip { clip, .. } if self.doc.selectable(clip) => {
                if !self.doc.is_selected(clip) {
                    self.doc.select(clip, false);
                }
                // 记窗口坐标：菜单画在整个窗口上，不被时间轴面板裁掉
                Some((f32::from(e.position.x), f32::from(e.position.y)))
            }
            _ => None,
        };
        self.after_change(window, cx);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "菜单项一项一项列着，和剪映的菜单对照着看"
    )]
    fn menu_items(&self) -> Vec<Vec<Item>> {
        let selected = self.doc.selection();
        let any = !selected.is_empty();
        let video = selected
            .first()
            .is_some_and(|&c| ve_core::edit::copy_attributes(&self.doc.project, c).is_ok());
        let all_off = any
            && selected.iter().all(|&c| {
                self.doc
                    .project
                    .timeline
                    .find_clip(c)
                    .is_some_and(|(_, clip)| clip.disabled)
            });
        let single_media = matches!(selected, [one] if self
            .doc
            .project
            .timeline
            .find_clip(*one)
            .is_some_and(|(_, c)| c.asset().is_some()));
        let item = |id, label, keys, action: fn() -> Box<dyn Action>, enabled| Item {
            id,
            label,
            keys,
            action,
            enabled,
        };
        vec![
            vec![
                item("cm-copy", "复制", "Ctrl+C", || Box::new(act::Copy), any),
                item("cm-cut", "剪切", "Ctrl+X", || Box::new(act::Cut), any),
                item(
                    "cm-paste",
                    "粘贴",
                    "Ctrl+V",
                    || Box::new(act::Paste),
                    self.clipboard.clips.is_some(),
                ),
                item(
                    "cm-copy-attrs",
                    "复制属性",
                    "Ctrl+Shift+C",
                    || Box::new(act::CopyAttributes),
                    video,
                ),
                item(
                    "cm-paste-attrs",
                    "粘贴属性",
                    "Ctrl+Shift+V",
                    || Box::new(act::PasteAttributes),
                    self.clipboard.attributes.is_some(),
                ),
                item("cm-delete", "删除", "Delete", || Box::new(act::Delete), any),
            ],
            vec![item(
                "cm-same-color",
                "选中同色片段",
                "",
                || Box::new(act::SelectSameColor),
                self.selection_label().is_some(),
            )],
            vec![
                item("cm-split", "分割", "Ctrl+B", || Box::new(act::Split), any),
                item(
                    "cm-trim-left",
                    "向左裁剪",
                    "Q",
                    || Box::new(act::TrimLeft),
                    any,
                ),
                item(
                    "cm-trim-right",
                    "向右裁剪",
                    "W",
                    || Box::new(act::TrimRight),
                    any,
                ),
            ],
            vec![
                item(
                    "cm-disable",
                    if all_off {
                        "启用片段"
                    } else {
                        "停用片段"
                    },
                    "V",
                    || Box::new(act::ToggleDisabled),
                    any,
                ),
                item(
                    "cm-replace",
                    "替换片段",
                    "",
                    || Box::new(act::ReplaceClip),
                    single_media,
                ),
                item(
                    "cm-reverse",
                    "倒放",
                    "",
                    || Box::new(act::Reverse),
                    self.reverse_target().is_some() && !self.is_reversing(),
                ),
            ],
        ]
    }

    /// 颜色标签一行：8 个色点 + ⟲ 还原（剪映）。
    fn render_label_dots(&self, cx: &Context<'_, Self>) -> AnyElement {
        let current = self.selection_label();
        let dot = |i: u8| {
            let on = current == Some(i);
            div()
                .id(gpui_kit::SharedString::from(format!("cm-label-{i}")))
                .test_support()
                .size(px(16.))
                .rounded_full()
                .bg(crate::clip_label::label_color(i))
                .border_2()
                .border_color(if on {
                    palette::text()
                } else {
                    gpui_kit::transparent_black()
                })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.timeline.context_menu = None;
                    this.label_selection(Some(i), window, cx);
                }))
        };
        h_flex()
            .h(px(26.))
            .px_2()
            .gap_1p5()
            .children((0..ve_core::timeline::LABEL_COLORS).map(dot))
            .child(
                div()
                    .id("cm-label-clear")
                    .test_support()
                    .ml_1()
                    .text_sm()
                    .cursor_pointer()
                    .child("⟲")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.timeline.context_menu = None;
                        this.label_selection(None, window, cx);
                    })),
            )
            .into_any_element()
    }

    /// 右键菜单（弹在按下的位置，窗口坐标）；靠近窗口下边时往上弹。
    pub(crate) fn render_context_menu(
        &self,
        window: &Window,
        cx: &Context<'_, Self>,
    ) -> Option<AnyElement> {
        let (x, y) = self.timeline.context_menu?;
        let groups = self.menu_items();
        // 色点一行跟在「选中同色片段」后面（第 2 组）
        #[expect(clippy::cast_precision_loss, reason = "菜单只有十来项")]
        let height = (groups.iter().map(Vec::len).sum::<usize>() + 1) as f32 * 26.0
            + (groups.len().saturating_sub(1)) as f32 * 9.0
            + 10.0;
        let viewport = window.viewport_size();
        let y = y.min(f32::from(viewport.height) - height).max(0.0);
        let x = x.min(f32::from(viewport.width) - 240.0).max(0.0);
        let muted = cx.theme().muted_foreground;
        let mut rows: Vec<AnyElement> = Vec::new();
        for (gi, group) in groups.into_iter().enumerate() {
            if gi > 0 {
                rows.push(
                    div()
                        .my_1()
                        .h(px(1.))
                        .bg(cx.theme().border)
                        .into_any_element(),
                );
            }
            for it in group {
                let action = it.action;
                let row = h_flex()
                    .id(it.id)
                    .test_support()
                    .h(px(26.))
                    .px_2()
                    .gap_4()
                    .justify_between()
                    .rounded(px(4.))
                    .text_sm()
                    .child(it.label)
                    .child(chrome::keycaps(it.keys, cx));
                let row = if it.enabled {
                    row.cursor_pointer()
                        .hover(|s| s.bg(palette::menu_hover()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.timeline.context_menu = None;
                            cx.notify();
                            window.dispatch_action(action(), cx);
                        }))
                } else {
                    row.text_color(muted)
                };
                rows.push(row.into_any_element());
            }
            if gi == 1 {
                rows.push(self.render_label_dots(cx));
            }
        }
        Some(
            deferred(
                v_flex()
                    .id("clip-menu")
                    .test_support()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(240.))
                    .p_1()
                    .bg(palette::menu_bg())
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(chrome::CARD_RADIUS))
                    .shadow_lg()
                    // 菜单叠在时间轴上：拦下按下事件，别让时间轴当成在片段上点了一下（会开始拖动）
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.timeline.context_menu = None;
                        cx.notify();
                    }))
                    .children(rows),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}
