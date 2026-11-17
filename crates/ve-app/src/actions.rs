//! 动作与快捷键。

use gpui_kit::{App, KeyBinding};

#[expect(missing_docs, reason = "动作结构体由宏生成，名字即说明")]
#[expect(clippy::derive_partial_eq_without_eq, reason = "宏生成的派生")]
mod generated {
    gpui_kit::actions!(
        editor,
        [
            NewProject,
            Open,
            Save,
            SaveAs,
            Import,
            Export,
            ImportSrt,
            ExportSrt,
            Undo,
            Redo,
            PlayPause,
            Pause,
            PlayForward,
            JumpBack,
            StepForward,
            StepBack,
            GoStart,
            GoEnd,
            Split,
            Delete,
            RippleDelete,
            ZoomIn,
            ZoomOut,
            AddSubtitle,
        ]
    );
}

pub use generated::*;

/// 编辑器根视图的按键上下文。
pub const CONTEXT: &str = "Editor";

/// 单键快捷键只在没有聚焦输入框时生效（否则打字会被吃掉）。
const NO_INPUT: &str = "Editor && !Input";

/// 注册快捷键。
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-n", NewProject, Some(CONTEXT)),
        KeyBinding::new("ctrl-o", Open, Some(CONTEXT)),
        KeyBinding::new("ctrl-s", Save, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-s", SaveAs, Some(CONTEXT)),
        KeyBinding::new("ctrl-i", Import, Some(CONTEXT)),
        KeyBinding::new("ctrl-e", Export, Some(CONTEXT)),
        KeyBinding::new("ctrl-z", Undo, Some(NO_INPUT)),
        KeyBinding::new("ctrl-y", Redo, Some(NO_INPUT)),
        KeyBinding::new("ctrl-shift-z", Redo, Some(NO_INPUT)),
        KeyBinding::new("space", PlayPause, Some(NO_INPUT)),
        KeyBinding::new("k", Pause, Some(NO_INPUT)),
        KeyBinding::new("l", PlayForward, Some(NO_INPUT)),
        KeyBinding::new("j", JumpBack, Some(NO_INPUT)),
        KeyBinding::new("right", StepForward, Some(NO_INPUT)),
        KeyBinding::new("left", StepBack, Some(NO_INPUT)),
        KeyBinding::new("home", GoStart, Some(NO_INPUT)),
        KeyBinding::new("end", GoEnd, Some(NO_INPUT)),
        KeyBinding::new("s", Split, Some(NO_INPUT)),
        KeyBinding::new("delete", Delete, Some(NO_INPUT)),
        KeyBinding::new("backspace", Delete, Some(NO_INPUT)),
        KeyBinding::new("shift-delete", RippleDelete, Some(NO_INPUT)),
        KeyBinding::new("ctrl-=", ZoomIn, Some(NO_INPUT)),
        KeyBinding::new("ctrl--", ZoomOut, Some(NO_INPUT)),
        KeyBinding::new("t", AddSubtitle, Some(NO_INPUT)),
    ]);
}
