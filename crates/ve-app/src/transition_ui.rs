//! 素材面板「转场」标签：选好时长，点一个转场，挂到选中片段的开头（和前一段之间）；「无」去掉。

use crate::chrome;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use ve_core::{Time, Transition, TransitionKind};

/// 时长选项（秒）。
const DURATIONS: [(&str, f64); 3] = [("0.5s", 0.5), ("1s", 1.0), ("1.5s", 1.5)];

/// 元素编号的后缀。
const fn slug(kind: TransitionKind) -> &'static str {
    match kind {
        TransitionKind::Dissolve => "dissolve",
        TransitionKind::FadeBlack => "fade-black",
        TransitionKind::Slide => "slide",
        TransitionKind::Push => "push",
        TransitionKind::ZoomIn => "zoom-in",
    }
}

impl Workspace {
    /// 把转场挂到选中的片段上（`None` 去掉）；一步撤销。
    fn apply_transition(
        &mut self,
        kind: Option<TransitionKind>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let selected = self.doc.selection().to_vec();
        if selected.is_empty() {
            crate::workspace::notify(
                Notification::warning("先在时间轴上选中后一段片段"),
                window,
                cx,
            );
            return;
        }
        let duration = Time::from_secs_f64(self.transition_secs);
        let t = kind.map(|kind| Transition { kind, duration });
        self.apply(window, cx, |p| {
            for &clip in &selected {
                ve_core::edit::set_transition(p, clip, t)?;
            }
            Ok(())
        });
    }

    pub(crate) fn render_transition_tab(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let durations = h_flex().gap_1().children(DURATIONS.map(|(label, s)| {
            #[expect(clippy::float_cmp, reason = "比较的是同一组常量")]
            let on = self.transition_secs == s;
            chrome::chip(format!("tr-dur-{label}"), label, on, cx).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.transition_secs = s;
                    cx.notify();
                },
            ))
        }));
        let tile = |id: SharedString, label: &'static str| {
            v_flex()
                .id(id)
                .test_support()
                .w(px(88.))
                .gap_1()
                .cursor_pointer()
                .group("transition-tile")
                .child(
                    div()
                        .w(px(88.))
                        .h(px(50.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .bg(cx.theme().muted)
                        .border_1()
                        .border_color(gpui_kit::transparent_black())
                        .group_hover("transition-tile", |s| s.border_color(palette::accent()))
                        .text_sm()
                        .child(label),
                )
        };
        let tiles = h_flex()
            .flex_wrap()
            .gap_2()
            .child(tile("tr-none".into(), "无").on_click(
                cx.listener(|this, _, window, cx| this.apply_transition(None, window, cx)),
            ))
            .children(TransitionKind::ALL.map(|kind| {
                crate::timeline::drop::draggable(
                    tile(format!("tr-{}", slug(kind)).into(), kind.label()),
                    crate::timeline::drop::BinItem::Transition(kind),
                    kind.label(),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.apply_transition(Some(kind), window, cx);
                }))
            }));
        v_flex()
            .id("bin-transitions")
            .test_support()
            .p_2()
            .gap_2()
            .child(
                div().text_xs().text_color(muted).child(
                    "在时间轴上选中后一段片段，点一个转场：挂在它和前一段之间（两段要紧挨着）",
                ),
            )
            .child(div().text_sm().text_color(muted).child("时长"))
            .child(durations)
            .child(tiles)
    }
}
