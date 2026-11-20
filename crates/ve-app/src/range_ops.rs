//! 入出点区域（剪映 I / O）：I 设入点、O 设出点、Shift+X 以选中的片段选定区域、Alt+X 取消区域、
//! Shift+Enter 播放区域（播到出点停）。区域画在标尺上，导出时可以只导出这一段。

use crate::actions as act;
use crate::workspace::Workspace;
use gpui_kit::{Context, Window};
use ve_core::edit::{range_from_clips, set_in_point, set_out_point};

impl Workspace {
    pub(crate) fn set_in(
        &mut self,
        _: &act::SetInPoint,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let at = self.playhead_frame();
        self.apply(window, cx, |p| set_in_point(p, at));
    }

    pub(crate) fn set_out(
        &mut self,
        _: &act::SetOutPoint,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let at = self.playhead_frame();
        self.apply(window, cx, |p| set_out_point(p, at));
    }

    pub(crate) fn range_from_selection(
        &mut self,
        _: &act::RangeFromClips,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let selected = self.doc.selection().to_vec();
        if selected.is_empty() {
            return;
        }
        self.apply(window, cx, |p| range_from_clips(p, &selected));
    }

    pub(crate) fn clear_range(
        &mut self,
        _: &act::ClearRange,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.doc.project.timeline.range.is_none() {
            return;
        }
        self.apply(window, cx, |p| {
            p.timeline.range = None;
            Ok(())
        });
    }

    /// Shift+Enter：从入点播到出点停下。
    pub(crate) fn play_range(
        &mut self,
        _: &act::PlayRange,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((from, to)) = self.doc.project.timeline.range else {
            return;
        };
        self.seek(from, cx);
        self.engine.play();
        self.play_until = Some(to);
        cx.notify();
    }

    /// 每帧调用：播放区域时到了出点就停在出点；用户自己停了就不再管。
    pub(crate) fn stop_at_range_end(&mut self, cx: &mut Context<'_, Self>) {
        let Some(end) = self.play_until else {
            return;
        };
        if !self.engine.is_playing() {
            self.play_until = None;
        } else if self.engine.position() >= end {
            self.engine.pause();
            self.seek(end, cx);
            self.play_until = None;
        }
    }
}
