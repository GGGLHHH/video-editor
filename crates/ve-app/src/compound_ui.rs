//! 复合片段（剪映右键「新建复合片段」Alt+G、「解除复合片段」Alt+Shift+G）的界面部分：
//! 动作、属性面板，进入复合片段编辑（剪映的子草稿）/ 返回上一层。打包和摊开的规则在 [`ve_core::compound`]。

use crate::actions as act;
use crate::workspace::{Workspace, notify};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window,
    div,
};
use ve_core::{ClipId, TrackKind, format_timecode};

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
        let id = clip.id;
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
                    .child("复合片段可以整段移动、裁剪、分割，下面的位置大小、不透明度对里面所有画面一起生效；要改里面的片段点「进入」或先解除"),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("insp-enter-compound")
                            .primary()
                            .label("进入复合片段编辑")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.enter_compound(id, window, cx);
                            })),
                    )
                    .child(
                        Button::new("insp-break-compound")
                            .label("解除复合片段")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.break_compound(&act::BreakCompound, window, cx);
                            })),
                    ),
            )
            .children(self.render_transform_section(cx))
            .into_any_element()
    }
}

/// 把里面那份工程 `inside` 写回外层 `outer` 的复合片段 `clip`：轨道写进复合片段，里面导入的素材、
/// 用掉的编号、改过的字幕样式和草稿参数（比例、分辨率、帧率）一起带回来；主轨磁吸、联动是进去时
/// 临时关的，保持外层自己的。
fn carry_back(
    outer: &mut ve_core::Project,
    inside: ve_core::Project,
    clip: ClipId,
) -> Result<(), ve_core::EditError> {
    let (magnet, linkage) = (outer.settings.main_magnet, outer.settings.linkage);
    outer.assets = inside.assets;
    outer.ids = inside.ids;
    outer.subtitle_style = inside.subtitle_style;
    outer.settings = ve_core::ProjectSettings {
        main_magnet: magnet,
        linkage,
        ..inside.settings
    };
    ve_core::compound::set_inner(outer, clip, inside.timeline.tracks)
}

/// 进入复合片段编辑时记下的外面一层。
#[derive(Debug)]
pub(crate) struct OpenCompound {
    /// 外面那层的文档（返回时写回去）。
    outer: crate::document::Document,
    /// 在编辑外面的哪段复合片段。
    clip: ClipId,
    /// 复合片段的名字（面包屑上显示）。
    name: String,
}

impl Workspace {
    /// 正在编辑复合片段里面时，从外到里的名字（不在里面时为空）。
    #[must_use]
    pub fn compound_path(&self) -> Vec<String> {
        self.compound_stack.iter().map(|o| o.name.clone()).collect()
    }

    /// 进入复合片段（剪映：复合片段是子草稿，双击进去编辑）：时间轴换成它里面的轨道，
    /// 改完点「返回上一层」写回去。里面不开主轨磁吸、联动（不然一进去就会把里面排紧）。
    pub fn enter_compound(
        &mut self,
        clip: ClipId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((track, c)) = self.doc.project.timeline.find_clip(clip) else {
            return;
        };
        let Some((inner, _)) = c.compound() else {
            return;
        };
        // 轨道锁着：进去改了也写不回去，干脆不让进
        if self
            .doc
            .project
            .timeline
            .track(track)
            .is_some_and(|t| t.locked)
        {
            notify(
                Notification::warning("复合片段所在的轨道锁着，先解锁再进入编辑"),
                window,
                cx,
            );
            return;
        }
        let name = inner.name.clone();
        let mut p = self.doc.project.clone();
        p.settings.main_magnet = false;
        p.settings.linkage = false;
        p.timeline = ve_core::Timeline {
            tracks: inner.tracks.clone(),
            markers: Vec::new(),
            marker_colors: Vec::new(),
            range: None,
            cover: None,
        };
        // 每类至少一条轨道（界面和编辑都按有这三类来）
        for kind in [TrackKind::Subtitle, TrackKind::Video, TrackKind::Audio] {
            if p.timeline.track_of_kind(kind).is_none() {
                let _ = ve_core::edit::add_track(&mut p, kind);
            }
        }
        let path = self.doc.path().map(std::path::PathBuf::from);
        let outer = std::mem::replace(
            &mut self.doc,
            crate::document::Document::from_project(p, path),
        );
        self.compound_stack.push(OpenCompound { outer, clip, name });
        self.after_switching_document(window, cx);
    }

    /// 返回上一层：里面改过的轨道写回那段复合片段（外面一步撤销）。不在复合片段里时返回 `false`。
    pub fn leave_compound(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> bool {
        let Some(open) = self.compound_stack.pop() else {
            return false;
        };
        let inside = std::mem::replace(&mut self.doc, open.outer);
        if inside.revision() > 0 {
            let p = inside.project;
            let clip = open.clip;
            let wrote = self.doc.edit(move |outer| carry_back(outer, p, clip));
            if let Err(e) = wrote {
                notify(
                    Notification::warning(format!("复合片段没写回去：{e}")),
                    window,
                    cx,
                );
            }
        }
        if self.doc.project.timeline.find_clip(open.clip).is_some() {
            self.doc.select(open.clip, false);
        }
        self.after_switching_document(window, cx);
        true
    }

    /// 整个工程：在复合片段里面时，把里面现在的样子一层层写回去（不改任何文档）；不在里面时就是
    /// 当前文档。自动保存、导出 EDL / OTIO / SRT 用它，在里面也是整个工程。
    #[must_use]
    pub fn whole_project(&self) -> ve_core::Project {
        let mut cur = self.doc.project.clone();
        for open in self.compound_stack.iter().rev() {
            let mut outer = open.outer.project.clone();
            // 写不回去（轨道锁着等）时这一层保持原样
            let mut next = outer.clone();
            if carry_back(&mut next, cur, open.clip).is_ok() {
                outer = next;
            }
            cur = outer;
        }
        cur
    }

    /// 在复合片段里面时，最外层文档的路径（自动保存往哪存）。
    pub(crate) fn outermost_path(&self) -> Option<&std::path::Path> {
        self.compound_stack
            .first()
            .map_or_else(|| self.doc.path(), |o| o.outer.path())
    }

    /// 自动保存在复合片段里面时：存整个工程，里外各层都记下这个路径。
    pub(crate) fn mark_whole_saved(&mut self, path: &std::path::Path) {
        for open in &mut self.compound_stack {
            open.outer.mark_saved(path.to_path_buf());
        }
        self.doc.mark_saved(path.to_path_buf());
    }

    /// 一层层都退出来（保存、导出、打开别的工程之前）。
    pub(crate) fn leave_all_compounds(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        while self.leave_compound(window, cx) {}
    }

    /// 换了正在编辑的那层之后：播放引擎、属性面板、缩略图跟着换。
    fn after_switching_document(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.engine.pause();
        self.engine_revision = None;
        self.sync_engine();
        self.engine.seek(ve_core::Time::ZERO);
        self.inspector.unbind();
        self.after_change(window, cx);
    }

    /// 时间轴工具栏上的面包屑：在复合片段里面时显示「‹ 返回上一层  主时间线 › 复合片段1」。
    pub(crate) fn render_compound_breadcrumb(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        if self.compound_stack.is_empty() {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        let path = std::iter::once("主时间线".to_owned())
            .chain(self.compound_path())
            .collect::<Vec<_>>()
            .join(" › ");
        Some(
            h_flex()
                .gap_2()
                .child(
                    Button::new("compound-back")
                        .small()
                        .label("‹ 返回上一层")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.leave_compound(window, cx);
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(SharedString::from(path)),
                )
                .into_any_element(),
        )
    }
}
