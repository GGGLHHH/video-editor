//! 时间轴工具栏（剪映式）：左边撤销、恢复、分割、裁剪、删除、镜像、旋转、标记、加字幕，
//! 右边主轨磁吸、吸附、联动三个开关和缩放（适应窗口、缩小、滑块、放大）。
//! 全是图标按钮，悬停出提示（带快捷键）。

use crate::actions as act;
use crate::chrome;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::Slider;
use gpui_kit::component::{Disableable as _, Icon, h_flex};
use gpui_kit::{
    Action, Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _, div, px,
};

/// 一个图标按钮：点了就发出对应的动作。
fn tool(
    id: &'static str,
    icon: Lucide,
    tip: &'static str,
    action: fn() -> Box<dyn Action>,
    cx: &Context<'_, Workspace>,
) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .icon(Icon::new(icon))
        .tooltip(tip)
        .on_click(cx.listener(move |_, _, window, cx| {
            window.dispatch_action(action(), cx);
        }))
}

/// 开关类图标按钮：开着时图标是主色青（剪映的开关都这样表示）。
fn toggle(
    id: &'static str,
    icon: Lucide,
    tip: &'static str,
    on: bool,
    action: fn() -> Box<dyn Action>,
    cx: &Context<'_, Workspace>,
) -> Button {
    let icon = Icon::new(icon);
    let icon = if on {
        icon.text_color(palette::accent())
    } else {
        icon
    };
    Button::new(id)
        .ghost()
        .compact()
        .icon(icon)
        .tooltip(tip)
        .on_click(cx.listener(move |_, _, window, cx| {
            window.dispatch_action(action(), cx);
        }))
}

fn gap() -> impl IntoElement {
    div().w(px(1.)).h(px(16.)).mx_1().bg(palette::divider())
}

impl Workspace {
    pub(crate) fn render_timeline_toolbar(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        h_flex()
            .id("timeline-toolbar")
            .test_support()
            .h(px(chrome::HEADER_HEIGHT + 4.0))
            .flex_none()
            .px_2()
            .gap_0p5()
            .border_b_1()
            .border_color(palette::divider())
            .child(self.render_tool_button(cx))
            .children(self.render_compound_breadcrumb(cx))
            .child(gap())
            .child(tool(
                "tb-undo",
                Lucide::Undo2,
                "撤销（Ctrl+Z）",
                || Box::new(act::Undo),
                cx,
            ))
            .child(tool(
                "tb-redo",
                Lucide::Redo2,
                "恢复（Ctrl+Y）",
                || Box::new(act::Redo),
                cx,
            ))
            .child(gap())
            .child(tool(
                "tb-split",
                Lucide::SquareSplitHorizontal,
                "分割（Ctrl+B）",
                || Box::new(act::Split),
                cx,
            ))
            .child(tool(
                "tb-trim-left",
                Lucide::ArrowLeftToLine,
                "向左裁剪（Q）：剪掉播放头左边的部分",
                || Box::new(act::TrimLeft),
                cx,
            ))
            .child(tool(
                "tb-trim-right",
                Lucide::ArrowRightToLine,
                "向右裁剪（W）：剪掉播放头右边的部分",
                || Box::new(act::TrimRight),
                cx,
            ))
            .child(tool(
                "tb-delete",
                Lucide::Trash,
                "删除（Delete）",
                || Box::new(act::Delete),
                cx,
            ))
            .child(gap())
            .child(self.render_clip_tools(cx))
            .child(gap())
            .child(tool(
                "tb-marker",
                Lucide::BookmarkPlus,
                "在播放头处加标记（M），再按一次去掉；Shift+M / Alt+Shift+M 跳到下一个 / 上一个",
                || Box::new(act::ToggleMarker),
                cx,
            ))
            .child(tool(
                "tb-subtitle",
                Lucide::Type,
                "在播放头处加一条字幕（T）",
                || Box::new(act::AddSubtitle),
                cx,
            ))
            .child(div().flex_1())
            .child(self.render_record_button(cx))
            .child(gap())
            .child(self.render_toolbar_switches(cx))
    }

    /// 改选中视频片段的一组：定格、镜像、旋转（没选中视频片段时变灰）。
    fn render_clip_tools(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        let video_selected = !self.selected_video_clips().is_empty();
        h_flex()
            .gap_0p5()
            .child(
                tool(
                    "tb-freeze",
                    Lucide::Snowflake,
                    "定格：在播放头处插入 3 秒静止画面（选中的视频片段）",
                    || Box::new(act::Freeze),
                    cx,
                )
                .disabled(!video_selected),
            )
            .child(
                tool(
                    "tb-reverse",
                    Lucide::Rewind,
                    "倒放：选中的片段倒着放（再点一次恢复）",
                    || Box::new(act::Reverse),
                    cx,
                )
                .disabled(self.reverse_target().is_none() || self.is_reversing()),
            )
            .child(
                tool(
                    "tb-mirror",
                    Lucide::FlipHorizontal2,
                    "镜像：选中的视频片段左右翻转",
                    || Box::new(act::Mirror),
                    cx,
                )
                .disabled(!video_selected),
            )
            .child(
                tool(
                    "tb-rotate",
                    Lucide::RotateCw,
                    "旋转：选中的视频片段顺时针转 90°",
                    || Box::new(act::Rotate90),
                    cx,
                )
                .disabled(!video_selected),
            )
    }

    /// 工具栏右边：主轨磁吸、吸附、联动三个开关和缩放。
    fn render_toolbar_switches(&self, cx: &Context<'_, Self>) -> impl IntoElement {
        h_flex()
            .gap_0p5()
            .child(toggle(
                "tb-magnet",
                Lucide::AlignHorizontalJustifyStart,
                "主轨磁吸（P）：主轨片段从 0 开始首尾相接，删掉、剪短后自动补位",
                self.doc.project.settings.main_magnet,
                || Box::new(act::ToggleMagnet),
                cx,
            ))
            .child(toggle(
                "tb-snap",
                Lucide::Magnet,
                "吸附（N）：拖动时贴到播放头、片段和画面边缘；按住 Alt 临时取反",
                self.snapping,
                || Box::new(act::ToggleSnapping),
                cx,
            ))
            .child(toggle(
                "tb-linkage",
                Lucide::Link,
                "联动（~）：主轨片段挪动时，压在上面的字幕跟着走",
                self.doc.project.settings.linkage,
                || Box::new(act::ToggleLinkage),
                cx,
            ))
            .child(toggle(
                "tb-preview-axis",
                Lucide::ScanEye,
                "预览轴（S）：鼠标在时间轴上悬停时，播放器显示那一刻的画面，播放头不动",
                self.timeline.preview_axis,
                || Box::new(act::TogglePreviewAxis),
                cx,
            ))
            .child(gap())
            .child(tool(
                "tb-zoom-fit",
                Lucide::Scan,
                "全局预览缩放（Shift+Z）：整条时间线刚好放进视图",
                || Box::new(act::ZoomToFit),
                cx,
            ))
            .child(tool(
                "tb-zoom-out",
                Lucide::ZoomOut,
                "缩小时间线（Ctrl+-）",
                || Box::new(act::ZoomOut),
                cx,
            ))
            .child(
                div()
                    .id("tb-zoom-slider")
                    .test_support()
                    .w(px(96.))
                    .px_1()
                    .child(Slider::new(&self.zoom_slider)),
            )
            .child(tool(
                "tb-zoom-in",
                Lucide::ZoomIn,
                "放大时间线（Ctrl+=）",
                || Box::new(act::ZoomIn),
                cx,
            ))
    }
}
