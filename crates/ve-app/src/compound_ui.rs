//! 复合片段（剪映右键「新建复合片段」Alt+G、「解除复合片段」Alt+Shift+G）的界面部分：
//! 动作、属性面板。打包和摊开的规则在 [`ve_core::compound`]。

use crate::actions as act;
use crate::workspace::Workspace;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window,
    div,
};
use ve_core::{ClipId, format_timecode};

impl Workspace {
    /// 选中的片段能不能打包（至少一段素材或复合片段）。
    pub(crate) fn can_make_compound(&self) -> bool {
        self.doc.selection().iter().any(|&id| {
            self.doc
                .project
                .timeline
                .find_clip(id)
                .is_some_and(|(_, c)| c.asset().is_some() || c.compound().is_some())
        })
    }

    /// 选中的唯一一段复合片段。
    pub(crate) fn selected_compound(&self) -> Option<ClipId> {
        let [id] = self.doc.selection() else {
            return None;
        };
        let (_, c) = self.doc.project.timeline.find_clip(*id)?;
        c.compound().map(|_| *id)
    }

    /// 新建复合片段：选中的打包成一段，选中它。
    pub(crate) fn make_compound(
        &mut self,
        _: &act::MakeCompound,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.can_make_compound() {
            return;
        }
        let clips = self.doc.selection().to_vec();
        if let Some(id) = self.apply(window, cx, |p| ve_core::compound::make_compound(p, &clips)) {
            self.doc.select(id, false);
            self.after_change(window, cx);
        }
    }

    /// 解除复合片段：里面的片段放回时间轴，选中它们。
    pub(crate) fn break_compound(
        &mut self,
        _: &act::BreakCompound,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(clip) = self.selected_compound() else {
            return;
        };
        if let Some(ids) = self.apply(window, cx, |p| ve_core::compound::break_compound(p, clip)) {
            self.doc.set_selection(ids);
            self.after_change(window, cx);
        }
    }

    /// 复合片段的属性面板：名字、时间、里面有几条轨道几段，「解除复合片段」。
    pub(crate) fn render_compound_panel(
        &self,
        clip: &ve_core::Clip,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        let Some((inner, _)) = clip.compound() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let rate = self.doc.project.settings.frame_rate;
        let clips: usize = inner.tracks.iter().map(|t| t.clips.len()).sum();
        let row = |label: &'static str, value: String| {
            h_flex()
                .justify_between()
                .text_sm()
                .child(div().text_color(muted).child(label))
                .child(SharedString::from(value))
        };
        v_flex()
            .p_3()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .child(SharedString::from(inner.name.clone())),
            )
            .child(row("开始", format_timecode(clip.start, rate)))
            .child(row("长度", format_timecode(clip.duration, rate)))
            .child(row(
                "里面",
                format!("{} 条轨道、{clips} 段", inner.tracks.len()),
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("复合片段可以整段移动、裁剪、分割；要改里面的片段先解除"),
            )
            .child(
                Button::new("insp-break-compound")
                    .label("解除复合片段")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.break_compound(&act::BreakCompound, window, cx);
                    })),
            )
            .into_any_element()
    }
}
