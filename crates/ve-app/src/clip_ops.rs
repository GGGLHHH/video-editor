//! 片段的复制 / 剪切 / 粘贴、复制 / 粘贴属性、停用（剪映右键菜单和对应快捷键）。

use crate::actions as act;
use crate::workspace::Workspace;
use gpui_kit::component::notification::Notification;
use gpui_kit::{Context, Window};
use ve_core::edit::{
    Attributes, Copied, copy_attributes, copy_clips, delete_clips, paste_attributes, paste_clips,
    set_disabled,
};

/// 剪贴板（只在这个窗口里用，不进系统剪贴板）。
#[derive(Debug, Clone, Default)]
pub(crate) struct ClipBoard {
    /// 复制的片段。
    pub(crate) clips: Option<Copied>,
    /// 复制的属性。
    pub(crate) attributes: Option<Attributes>,
}

impl Workspace {
    pub(crate) fn copy(&mut self, _: &act::Copy, _: &mut Window, _: &mut Context<'_, Self>) {
        let selected = self.doc.selection().to_vec();
        if let Ok(c) = copy_clips(&self.doc.project, &selected)
            && !c.is_empty()
        {
            self.clipboard.clips = Some(c);
        }
    }

    pub(crate) fn cut(&mut self, _: &act::Cut, window: &mut Window, cx: &mut Context<'_, Self>) {
        let selected = self.doc.selection().to_vec();
        if selected.is_empty() {
            return;
        }
        self.copy(&act::Copy, window, cx);
        self.apply(window, cx, |p| delete_clips(p, &selected));
    }

    /// 粘贴到播放头处，粘出来的片段被选中。
    pub(crate) fn paste(
        &mut self,
        _: &act::Paste,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(copied) = self.clipboard.clips.clone() else {
            return;
        };
        let at = self
            .doc
            .project
            .settings
            .frame_rate
            .snap(self.engine.position());
        if let Some(ids) = self.apply(window, cx, |p| paste_clips(p, &copied, at)) {
            self.doc.set_selection(ids);
            self.after_change(window, cx);
        }
    }

    pub(crate) fn copy_attrs(
        &mut self,
        _: &act::CopyAttributes,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(&first) = self.doc.selection().first() else {
            return;
        };
        match copy_attributes(&self.doc.project, first) {
            Ok(a) => self.clipboard.attributes = Some(a),
            Err(_) => crate::workspace::notify(
                Notification::warning("只能复制视频片段的属性"),
                window,
                cx,
            ),
        }
    }

    pub(crate) fn paste_attrs(
        &mut self,
        _: &act::PasteAttributes,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(attrs) = self.clipboard.attributes.clone() else {
            return;
        };
        let selected = self.doc.selection().to_vec();
        self.apply(window, cx, |p| paste_attributes(p, &selected, &attrs));
        self.inspector.unbind();
        self.after_change(window, cx);
    }

    /// 选中的视频轨片段（含调节片段）：能调色、套滤镜的。
    pub(crate) fn selected_picture_clips(&self) -> Vec<ve_core::ClipId> {
        let tl = &self.doc.project.timeline;
        self.doc
            .selection()
            .iter()
            .copied()
            .filter(|&c| {
                tl.find_clip(c)
                    .and_then(|(t, _)| tl.track(t))
                    .is_some_and(|t| t.kind == ve_core::TrackKind::Video)
            })
            .collect()
    }

    /// 选中的视频素材片段（不含调节片段）：能定格、镜像、旋转的。
    pub(crate) fn selected_video_clips(&self) -> Vec<ve_core::ClipId> {
        let tl = &self.doc.project.timeline;
        self.selected_picture_clips()
            .into_iter()
            .filter(|&c| {
                tl.find_clip(c)
                    .is_some_and(|(_, clip)| clip.asset().is_some())
            })
            .collect()
    }

    /// 镜像（工具栏）：选中的视频片段都镜像了就全部取消，否则全部镜像。
    pub(crate) fn mirror(
        &mut self,
        _: &act::Mirror,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let clips = self.selected_video_clips();
        if clips.is_empty() {
            return;
        }
        let all_on = clips.iter().all(|&c| {
            self.doc
                .project
                .timeline
                .find_clip(c)
                .is_some_and(|(_, clip)| clip.transform.mirror)
        });
        self.apply(window, cx, |p| {
            ve_core::edit::set_mirror(p, &clips, !all_on)
        });
        self.inspector.unbind();
        self.after_change(window, cx);
    }

    /// 旋转（工具栏）：选中的视频片段各自顺时针转 90°（一步撤销）；有旋转关键帧的在播放头处打帧。
    pub(crate) fn rotate_90(
        &mut self,
        _: &act::Rotate90,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let clips = self.selected_video_clips();
        if clips.is_empty() {
            return;
        }
        let at = self.engine.position();
        self.apply(window, cx, |p| {
            for &c in &clips {
                let (start, len) = p
                    .timeline
                    .find_clip(c)
                    .map_or((at, ve_core::Time::ZERO), |(_, clip)| {
                        (clip.start, clip.duration)
                    });
                // 播放头不在片段上时按离它近的一头算
                let offset = (at - start).clamp(ve_core::Time::ZERO, len);
                ve_core::edit::rotate_quarter(p, c, offset)?;
            }
            Ok(())
        });
        self.inspector.unbind();
        self.after_change(window, cx);
    }

    /// V：选中的都停用了就全部启用，否则全部停用。
    pub(crate) fn toggle_disabled(
        &mut self,
        _: &act::ToggleDisabled,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let selected = self.doc.selection().to_vec();
        if selected.is_empty() {
            return;
        }
        let all_off = selected.iter().all(|&c| {
            self.doc
                .project
                .timeline
                .find_clip(c)
                .is_some_and(|(_, clip)| clip.disabled)
        });
        self.apply(window, cx, |p| set_disabled(p, &selected, !all_off));
    }

    /// M：在播放头处打标记，已经有就去掉（一步撤销）。
    pub(crate) fn toggle_marker(
        &mut self,
        _: &act::ToggleMarker,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let at = self
            .doc
            .project
            .settings
            .frame_rate
            .snap(self.engine.position());
        self.apply(window, cx, |p| {
            p.timeline.toggle_marker(at);
            Ok(())
        });
    }

    pub(crate) fn next_marker(
        &mut self,
        _: &act::NextMarker,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(t) = self
            .doc
            .project
            .timeline
            .next_marker(self.engine.position())
        {
            self.seek(t, cx);
        }
    }

    pub(crate) fn prev_marker(
        &mut self,
        _: &act::PrevMarker,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(t) = self
            .doc
            .project
            .timeline
            .prev_marker(self.engine.position())
        {
            self.seek(t, cx);
        }
    }
}
