//! 时间轴的鼠标模式和预览轴（剪映工具栏的「↖˅」和「预览轴」）。
//!
//! - 鼠标选择模式（A）/ 鼠标分割模式（B）：分割模式下点片段就在点的地方切开（播放中不能切）。
//! - 预览轴（S）：开着时鼠标在时间轴上悬停，播放器就显示鼠标那一刻的画面，播放头不动；
//!   鼠标离开时间轴、点一下或开始播放时回到播放头的画面。

use super::Hit;
use crate::actions as act;
use crate::chrome;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, deferred, div, px,
};
use ve_core::Time;

/// 鼠标模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseTool {
    /// 选择（拖动、修剪、框选）。
    #[default]
    Select,
    /// 分割：点片段就切开。
    Blade,
    /// 滑移：拖片段换用素材里的另一段，位置不动。
    Slip,
    /// 滑动：拖片段整段挪，前后紧挨着的片段跟着伸缩。
    Slide,
    /// 滚动：拖两段之间的剪辑点。
    Roll,
}

/// 鼠标模式菜单：编号、图标、名字、快捷键、模式。
const TOOL_MENU: [(&str, Lucide, &str, &str, MouseTool); 5] = [
    (
        "tool-select",
        Lucide::MousePointer2,
        "选择",
        "A",
        MouseTool::Select,
    ),
    (
        "tool-blade",
        Lucide::Scissors,
        "分割",
        "B",
        MouseTool::Blade,
    ),
    (
        "tool-slip",
        Lucide::MoveHorizontal,
        "滑移",
        "Y",
        MouseTool::Slip,
    ),
    (
        "tool-slide",
        Lucide::ArrowLeftRight,
        "滑动",
        "U",
        MouseTool::Slide,
    ),
    ("tool-roll", Lucide::Columns2, "滚动", "R", MouseTool::Roll),
];

impl Workspace {
    /// 播放头位置（预览轴在预览别处时也是原来的位置）。
    #[must_use]
    pub const fn playhead(&self) -> Time {
        self.playhead
    }

    pub(crate) fn select_tool(
        &mut self,
        _: &act::SelectTool,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.set_tool(MouseTool::Select, cx);
    }

    pub(crate) fn blade_tool(
        &mut self,
        _: &act::BladeTool,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.set_tool(MouseTool::Blade, cx);
    }

    pub(crate) fn slip_tool(
        &mut self,
        _: &act::SlipTool,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.set_tool(MouseTool::Slip, cx);
    }

    pub(crate) fn slide_tool(
        &mut self,
        _: &act::SlideTool,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.set_tool(MouseTool::Slide, cx);
    }

    pub(crate) fn roll_tool(
        &mut self,
        _: &act::RollTool,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.set_tool(MouseTool::Roll, cx);
    }

    /// 修剪工具下按在片段上：选择模式的拖法换成滑移 / 滑动 / 滚动（滚动按在哪条边就是哪个剪辑点）。
    pub(crate) fn tool_drag(
        &self,
        drag: super::interaction::Drag,
        origin: Time,
    ) -> Option<super::interaction::Drag> {
        use super::interaction::{Drag, TrimKind};
        let Some(clip) = drag.clip() else {
            return Some(drag);
        };
        let tool = |kind, clip| Drag::Tool {
            kind,
            clip,
            origin,
            applied: Time::ZERO,
        };
        Some(match (self.timeline.tool, drag) {
            (MouseTool::Select | MouseTool::Blade, d) => d,
            (MouseTool::Slip, _) => tool(TrimKind::Slip, clip),
            (MouseTool::Slide, _) => tool(TrimKind::Slide, clip),
            (MouseTool::Roll, Drag::TrimEnd { .. }) => tool(TrimKind::Roll, clip),
            (MouseTool::Roll, Drag::TrimStart { .. }) => {
                // 左边缘：剪辑点是前一段和它之间
                let (track, c) = self.doc.project.timeline.find_clip(clip)?;
                let t = self.doc.project.timeline.track(track)?;
                let prev = t.clips.iter().rev().find(|p| p.end() == c.start)?;
                tool(TrimKind::Roll, prev.id)
            }
            (MouseTool::Roll, _) => return None,
        })
    }

    fn set_tool(&mut self, tool: MouseTool, cx: &mut Context<'_, Self>) {
        self.timeline.tool = tool;
        self.timeline.tool_menu = false;
        cx.notify();
    }

    pub(crate) fn toggle_preview_axis(
        &mut self,
        _: &act::TogglePreviewAxis,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.timeline.preview_axis = !self.timeline.preview_axis;
        if !self.timeline.preview_axis {
            self.end_preview_axis();
        }
        cx.notify();
    }

    /// 预览轴：播放器先回到播放头那一帧（鼠标离开、点击、开始播放时）。
    pub(crate) fn end_preview_axis(&mut self) {
        if let Some(anchor) = self.timeline.axis_anchor.take() {
            self.engine.seek(anchor);
        }
    }

    /// 鼠标在时间轴上悬停（没按键）：记下鼠标那一刻；预览轴开着时让播放器显示那一刻。
    pub(crate) fn timeline_hover_at(&mut self, x: f32, y: f32, cx: &mut Context<'_, Self>) {
        let geo = self.timeline.geometry;
        let inside = x >= geo.header_width && y >= 0.0;
        let at = inside.then(|| {
            self.doc
                .project
                .settings
                .frame_rate
                .snap(geo.x_to_time(x).max(Time::ZERO))
        });
        if at == self.timeline.hover_time {
            return;
        }
        self.timeline.hover_time = at;
        if self.timeline.preview_axis && !self.engine.is_playing() {
            match at {
                Some(t) => {
                    if self.timeline.axis_anchor.is_none() {
                        self.timeline.axis_anchor = Some(self.playhead);
                    }
                    self.engine
                        .seek(t.min(self.doc.project.timeline.duration()));
                }
                None => self.end_preview_axis(),
            }
        }
        cx.notify();
    }

    /// 鼠标离开时间轴。
    pub(crate) fn timeline_hover_end(&mut self, cx: &mut Context<'_, Self>) {
        self.timeline.hover_time = None;
        self.end_preview_axis();
        cx.notify();
    }

    /// 分割模式下按在片段上：在按下的地方（吸到帧）切开它，右半段选中。返回这一下是不是分割。
    pub(crate) fn blade_click(
        &mut self,
        hit: Hit,
        x: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self.timeline.tool != MouseTool::Blade {
            return false;
        }
        let Hit::Clip { clip, .. } = hit else {
            return false;
        };
        if self.engine.is_playing() {
            crate::workspace::notify(
                Notification::warning("播放中，不支持鼠标点击分割"),
                window,
                cx,
            );
            return true;
        }
        if !self.doc.selectable(clip) {
            return true;
        }
        let at = self
            .doc
            .project
            .settings
            .frame_rate
            .snap(self.timeline.geometry.x_to_time(x));
        if let Some(right) = self.apply(window, cx, |p| ve_core::edit::split_at(p, clip, at)) {
            self.doc.select(right, false);
            self.after_change(window, cx);
        }
        true
    }

    /// 工具栏最左边的「↖˅」：现在的鼠标模式，点开选模式。
    pub(crate) fn render_tool_button(&self, cx: &Context<'_, Self>) -> AnyElement {
        let current = self.timeline.tool;
        let icon = match current {
            MouseTool::Select => Lucide::MousePointer2,
            MouseTool::Blade => Lucide::Scissors,
            MouseTool::Slip => Lucide::MoveHorizontal,
            MouseTool::Slide => Lucide::ArrowLeftRight,
            MouseTool::Roll => Lucide::Columns2,
        };
        div()
            .relative()
            .child(
                Button::new("tb-tool")
                    .ghost()
                    .compact()
                    .icon(Icon::new(icon).text_color(if current == MouseTool::Select {
                        cx.theme().foreground
                    } else {
                        palette::accent()
                    }))
                    .label("˅")
                    .tooltip("鼠标模式：选择（A）/ 分割（B）/ 滑移（Y）/ 滑动（U）/ 滚动（R）")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.timeline.tool_menu = !this.timeline.tool_menu;
                        cx.notify();
                    })),
            )
            .children(self.timeline.tool_menu.then(|| {
                let item = |id: &'static str,
                            icon: Lucide,
                            label: &'static str,
                            keys: &'static str,
                            tool: MouseTool| {
                    h_flex()
                        .id(id)
                        .test_support()
                        .h(px(26.))
                        .px_2()
                        .gap_2()
                        .rounded(px(4.))
                        .text_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(palette::menu_hover()))
                        .on_click(cx.listener(move |this, _, _, cx| this.set_tool(tool, cx)))
                        .child(
                            div()
                                .w(px(12.))
                                .text_color(palette::accent())
                                .child(if current == tool { "✓" } else { "" }),
                        )
                        .child(Icon::new(icon))
                        .child(div().flex_1().child(label))
                        .child(chrome::keycaps(keys, cx))
                };
                deferred(
                    v_flex()
                        .id("tool-menu")
            // 盖住下面的东西：点菜单不会顺带点到下面的画面 / 时间轴
            .occlude()
                        .test_support()
                        .absolute()
                        .top(px(30.))
                        .left_0()
                        .w(px(180.))
                        .p_1()
                        .bg(palette::menu_bg())
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(px(chrome::CARD_RADIUS))
                        .shadow_lg()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.timeline.tool_menu = false;
                            cx.notify();
                        }))
                        .children(TOOL_MENU.map(|(id, icon, label, keys, tool)| {
                            item(id, icon, label, keys, tool)
                        })),
                )
                .with_priority(1)
            }))
            .into_any_element()
    }
}
