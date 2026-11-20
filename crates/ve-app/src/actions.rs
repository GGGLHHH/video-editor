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
            PlayBackward,
            StepForward,
            StepBack,
            GoStart,
            GoEnd,
            Split,
            SplitAll,
            TrimLeft,
            TrimRight,
            SelectLeft,
            SelectRight,
            Delete,
            RippleDelete,
            ZoomIn,
            ZoomOut,
            AddSubtitle,
            Cancel,
            SelectAll,
            DeselectAll,
            ToggleSnapping,
            ToggleMagnet,
            ToggleLinkage,
            Copy,
            Cut,
            Paste,
            CopyAttributes,
            PasteAttributes,
            ToggleDisabled,
            ToggleMarker,
            NextMarker,
            PrevMarker,
            ToggleFullscreen,
            Mirror,
            Rotate90,
            ZoomToFit,
            Freeze,
            SetInPoint,
            SetOutPoint,
            RangeFromClips,
            ClearRange,
            PlayRange,
            SelectSameColor,
            ReplaceClip,
            SelectTool,
            BladeTool,
            TogglePreviewAxis,
            Reverse,
            ToggleRecord,
            MakeCompound,
            BreakCompound,
            SlipTool,
            SlideTool,
            RollTool,
            ExportOtio,
            ExportEdl,
            ImportOtio,
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
        KeyBinding::new("ctrl-shift-f", ToggleFullscreen, Some(CONTEXT)),
        KeyBinding::new("ctrl-z", Undo, Some(NO_INPUT)),
        KeyBinding::new("ctrl-y", Redo, Some(NO_INPUT)),
        KeyBinding::new("ctrl-shift-z", Redo, Some(NO_INPUT)),
        KeyBinding::new("space", PlayPause, Some(NO_INPUT)),
        KeyBinding::new("k", Pause, Some(NO_INPUT)),
        KeyBinding::new("l", PlayForward, Some(NO_INPUT)),
        KeyBinding::new("j", PlayBackward, Some(NO_INPUT)),
        KeyBinding::new("right", StepForward, Some(NO_INPUT)),
        KeyBinding::new("left", StepBack, Some(NO_INPUT)),
        KeyBinding::new("home", GoStart, Some(NO_INPUT)),
        KeyBinding::new("end", GoEnd, Some(NO_INPUT)),
        // 剪映（FCP X 键位）：Ctrl+B 分割、Ctrl+Shift+B 批量分割、Q / W 向左 / 右裁剪、[ / ] 向左 / 右全选；
        // A / B 鼠标选择 / 分割模式，S 预览轴
        KeyBinding::new("ctrl-b", Split, Some(NO_INPUT)),
        KeyBinding::new("a", SelectTool, Some(NO_INPUT)),
        KeyBinding::new("b", BladeTool, Some(NO_INPUT)),
        KeyBinding::new("s", TogglePreviewAxis, Some(NO_INPUT)),
        // Premiere 的修剪工具：Y 滑移、U 滑动；滚动编辑 R（Premiere 是 N，这里 N 已是吸附）
        KeyBinding::new("y", SlipTool, Some(NO_INPUT)),
        KeyBinding::new("u", SlideTool, Some(NO_INPUT)),
        KeyBinding::new("r", RollTool, Some(NO_INPUT)),
        KeyBinding::new("ctrl-shift-b", SplitAll, Some(NO_INPUT)),
        KeyBinding::new("q", TrimLeft, Some(NO_INPUT)),
        KeyBinding::new("w", TrimRight, Some(NO_INPUT)),
        KeyBinding::new("[", SelectLeft, Some(NO_INPUT)),
        KeyBinding::new("]", SelectRight, Some(NO_INPUT)),
        KeyBinding::new("delete", Delete, Some(NO_INPUT)),
        KeyBinding::new("backspace", Delete, Some(NO_INPUT)),
        KeyBinding::new("shift-delete", RippleDelete, Some(NO_INPUT)),
        KeyBinding::new("ctrl-=", ZoomIn, Some(NO_INPUT)),
        KeyBinding::new("ctrl--", ZoomOut, Some(NO_INPUT)),
        KeyBinding::new("shift-z", ZoomToFit, Some(NO_INPUT)),
        KeyBinding::new("t", AddSubtitle, Some(NO_INPUT)),
        // Esc：拖到一半先取消拖动（回到原位），否则取消选择
        KeyBinding::new("escape", Cancel, Some(NO_INPUT)),
        KeyBinding::new("ctrl-a", SelectAll, Some(NO_INPUT)),
        KeyBinding::new("ctrl-shift-a", DeselectAll, Some(NO_INPUT)),
        // 吸附开关（S 已经是分割；Resolve 也用 N）
        KeyBinding::new("n", ToggleSnapping, Some(NO_INPUT)),
        // 剪映：P 主轨磁吸、~ 联动（~ 和 ` 是同一个键）
        KeyBinding::new("p", ToggleMagnet, Some(NO_INPUT)),
        // 片段的复制粘贴只在没聚焦输入框时生效（输入框里 Ctrl+C / V 照常复制文字）
        KeyBinding::new("ctrl-c", Copy, Some(NO_INPUT)),
        KeyBinding::new("ctrl-x", Cut, Some(NO_INPUT)),
        KeyBinding::new("ctrl-v", Paste, Some(NO_INPUT)),
        KeyBinding::new("ctrl-shift-c", CopyAttributes, Some(NO_INPUT)),
        KeyBinding::new("ctrl-shift-v", PasteAttributes, Some(NO_INPUT)),
        KeyBinding::new("v", ToggleDisabled, Some(NO_INPUT)),
        // 剪映：Alt+G 新建复合片段、Alt+Shift+G 解除复合片段
        KeyBinding::new("alt-g", MakeCompound, Some(NO_INPUT)),
        KeyBinding::new("alt-shift-g", BreakCompound, Some(NO_INPUT)),
        // 剪映：M 添加标记，Shift+M / Alt+Shift+M 下一个 / 上一个标记
        KeyBinding::new("m", ToggleMarker, Some(NO_INPUT)),
        KeyBinding::new("shift-m", NextMarker, Some(NO_INPUT)),
        KeyBinding::new("alt-shift-m", PrevMarker, Some(NO_INPUT)),
        KeyBinding::new("`", ToggleLinkage, Some(NO_INPUT)),
        // 剪映：I / O 区域入点 / 出点，Shift+X 以片段选定区域，Alt+X 取消选定区域，Shift+Enter 播放出入点区域
        KeyBinding::new("i", SetInPoint, Some(NO_INPUT)),
        KeyBinding::new("o", SetOutPoint, Some(NO_INPUT)),
        KeyBinding::new("shift-x", RangeFromClips, Some(NO_INPUT)),
        KeyBinding::new("alt-x", ClearRange, Some(NO_INPUT)),
        KeyBinding::new("shift-enter", PlayRange, Some(NO_INPUT)),
    ]);
}
