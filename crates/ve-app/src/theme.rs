//! 深色定制主题（不用组件库的默认配色）与时间轴配色。

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, rgb};

/// 时间轴与预览用到的颜色。
pub mod palette {
    use gpui_kit::{Hsla, rgb, rgba};

    /// 预览区背景。
    #[must_use]
    pub fn preview_bg() -> Hsla {
        rgb(0x0b_0c0f).into()
    }
    /// 时间轴背景。
    #[must_use]
    pub fn timeline_bg() -> Hsla {
        rgb(0x15_171c).into()
    }
    /// 轨道底色（交替）。
    #[must_use]
    pub fn track_bg(even: bool) -> Hsla {
        if even {
            rgb(0x1b_1e24).into()
        } else {
            rgb(0x18_1a20).into()
        }
    }
    /// 标尺背景。
    #[must_use]
    pub fn ruler_bg() -> Hsla {
        rgb(0x11_1318).into()
    }
    /// 刻度与次要文字。
    #[must_use]
    pub fn tick() -> Hsla {
        rgb(0x5d_6472).into()
    }
    /// 主要文字。
    #[must_use]
    pub fn text() -> Hsla {
        rgb(0xd7_dbe3).into()
    }
    /// 视频片段。
    #[must_use]
    pub fn video_clip() -> Hsla {
        rgb(0x2b_5b8c).into()
    }
    /// 音频片段。
    #[must_use]
    pub fn audio_clip() -> Hsla {
        rgb(0x23_704d).into()
    }
    /// 字幕片段。
    #[must_use]
    pub fn subtitle_clip() -> Hsla {
        rgb(0x6a_4c9c).into()
    }
    /// 波形。
    #[must_use]
    pub fn waveform() -> Hsla {
        rgba(0xb8f5_d0cc).into()
    }
    /// 选中描边。
    #[must_use]
    pub fn selection() -> Hsla {
        rgb(0xff_c94a).into()
    }
    /// 鼠标悬停的片段描边（比选中弱得多）。
    #[must_use]
    pub fn hover() -> Hsla {
        rgba(0xffff_ff80).into()
    }
    /// 拖动中片段原位置的影子。
    #[must_use]
    pub fn ghost() -> Hsla {
        rgba(0xffff_ff1f).into()
    }
    /// 框选矩形的填充。
    #[must_use]
    pub fn marquee() -> Hsla {
        rgba(0xffc9_4a26).into()
    }
    /// 选中片段上的整体蒙色。
    #[must_use]
    pub fn selection_tint() -> Hsla {
        rgba(0xffc9_4a40).into()
    }
    /// 选中片段标题栏（亮底）上的文字。
    #[must_use]
    pub fn selection_text() -> Hsla {
        rgb(0x1a_1608).into()
    }
    /// 播放头。
    #[must_use]
    pub fn playhead() -> Hsla {
        rgb(0xff_4d5e).into()
    }
    /// 吸附线。
    #[must_use]
    pub fn snap() -> Hsla {
        rgb(0x5a_d1ff).into()
    }
}

/// 切到深色并覆盖几处颜色。
pub fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    // 切换模式会重置颜色，所以覆盖要放在第二次 update 里
    Theme::update(cx, |t| {
        let c = |v: u32| -> Hsla { rgb(v).into() };
        t.background = c(0x1e_2127);
        t.foreground = c(0xd7_dbe3);
        t.border = c(0x2c_313a);
        t.muted = c(0x26_2a31);
        t.muted_foreground = c(0x8b_93a1);
        t.primary = c(0x3d_8bfd);
        t.primary_foreground = c(0xff_ffff);
        t.accent = c(0x2a_2f38);
        t.secondary = c(0x2a_2f38);
    });
}
