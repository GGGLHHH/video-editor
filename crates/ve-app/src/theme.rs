//! 深色定制主题（不用组件库的默认配色）与时间轴配色。

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, rgb};

/// 时间轴与预览用到的颜色。
pub mod palette {
    use gpui_kit::{Hsla, rgb, rgba};

    /// 预览区背景。
    #[must_use]
    pub fn preview_bg() -> Hsla {
        rgb(0x25_2525).into()
    }
    /// 窗口底色（面板卡片之间的缝）。
    #[must_use]
    pub fn window_bg() -> Hsla {
        rgb(0x14_1414).into()
    }
    /// 面板卡片底色。
    #[must_use]
    pub fn panel_bg() -> Hsla {
        rgb(0x25_2525).into()
    }
    /// 主色（剪映的青）：选中的标签、开关、主按钮、当前时间。
    #[must_use]
    pub fn accent() -> Hsla {
        rgb(0x00_c8d2).into()
    }
    /// 面板标题栏 / 标签栏的分隔线。
    #[must_use]
    pub fn divider() -> Hsla {
        rgb(0x1c_1c1c).into()
    }
    /// 下拉菜单底色。
    #[must_use]
    pub fn menu_bg() -> Hsla {
        rgb(0x33_3333).into()
    }
    /// 下拉菜单里鼠标悬停的那一项。
    #[must_use]
    pub fn menu_hover() -> Hsla {
        rgb(0x44_4444).into()
    }
    /// 时间轴背景（和面板卡片同色）。
    #[must_use]
    pub fn timeline_bg() -> Hsla {
        rgb(0x25_2525).into()
    }
    /// 轨道底色（交替）。
    #[must_use]
    pub fn track_bg(even: bool) -> Hsla {
        if even {
            rgb(0x29_2929).into()
        } else {
            rgb(0x26_2626).into()
        }
    }
    /// 标尺和轨道头背景。
    #[must_use]
    pub fn ruler_bg() -> Hsla {
        rgb(0x22_2222).into()
    }
    /// 刻度与次要文字。
    #[must_use]
    pub fn tick() -> Hsla {
        rgb(0x6a_6a6a).into()
    }
    /// 主要文字。
    #[must_use]
    pub fn text() -> Hsla {
        rgb(0xe6_e6e6).into()
    }
    /// 视频片段（剪映的深青）。
    #[must_use]
    pub fn video_clip() -> Hsla {
        rgb(0x12_575d).into()
    }
    /// 音频片段。
    #[must_use]
    pub fn audio_clip() -> Hsla {
        rgb(0x2a_5478).into()
    }
    /// 字幕片段（剪映的砖红）。
    #[must_use]
    pub fn subtitle_clip() -> Hsla {
        rgb(0xa5_483a).into()
    }
    /// 调节片段（剪映特效、调节一类的紫色）。
    #[must_use]
    pub fn adjustment_clip() -> Hsla {
        rgb(0x7b_4c8c).into()
    }
    /// 波形。
    #[must_use]
    pub fn waveform() -> Hsla {
        rgba(0xb8f5_d0cc).into()
    }
    /// 选中描边（剪映是白色细描边）。
    #[must_use]
    pub fn selection() -> Hsla {
        rgb(0xff_ffff).into()
    }
    /// 时间轴上关键帧的小菱形。
    #[must_use]
    pub fn keyframe() -> Hsla {
        rgb(0xff_ffff).into()
    }
    /// 关键帧小菱形的描边（压在花哨的缩略图上也看得清）。
    #[must_use]
    pub fn keyframe_edge() -> Hsla {
        rgb(0x11_1111).into()
    }
    /// 鼠标悬停的片段描边（比选中弱得多）。
    #[must_use]
    pub fn hover() -> Hsla {
        rgba(0xffff_ff66).into()
    }
    /// 拖动中片段原位置的影子。
    #[must_use]
    pub fn ghost() -> Hsla {
        rgba(0xffff_ff1f).into()
    }
    /// 框选矩形的填充。
    #[must_use]
    pub fn marquee() -> Hsla {
        rgba(0xffff_ff1a).into()
    }
    /// 选中片段上的整体蒙色。
    #[must_use]
    pub fn selection_tint() -> Hsla {
        rgba(0xffff_ff1a).into()
    }
    /// 时间轴上转场小方块的底色。
    #[must_use]
    pub fn transition() -> Hsla {
        rgba(0x00c8_d266).into()
    }
    /// 停用片段上压的深色。
    #[must_use]
    pub fn disabled_veil() -> Hsla {
        rgba(0x1414_14c0).into()
    }
    /// 锁定轨道上的片段：比停用浅的一层灰。
    #[must_use]
    pub fn locked_veil() -> Hsla {
        rgba(0x1414_1470).into()
    }
    /// 播放头（剪映是白色）。
    #[must_use]
    pub fn playhead() -> Hsla {
        rgb(0xf2_f2f2).into()
    }
    /// 吸附线（主色青）。
    #[must_use]
    pub fn snap() -> Hsla {
        rgb(0x00_c8d2).into()
    }
}

/// 切到深色并覆盖几处颜色。
pub fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    // 切换模式会重置颜色，所以覆盖要放在第二次 update 里
    Theme::update(cx, |t| {
        let c = |v: u32| -> Hsla { rgb(v).into() };
        // 剪映的取色：面板 #252525、正文 #E6E6E6、次要字 #8A8A8A、主色青 #00C8D2
        t.background = c(0x25_2525);
        t.foreground = c(0xe6_e6e6);
        t.border = c(0x38_3838);
        t.muted = c(0x1e_1e1e);
        t.muted_foreground = c(0x8a_8a8a);
        t.primary = c(0x00_c8d2);
        t.primary_foreground = c(0xff_ffff);
        t.primary_hover = c(0x1f_d6df);
        t.primary_active = c(0x00_aab3);
        // 主按钮（导出等）单独有一套颜色，不跟着 primary 变
        t.button_primary = c(0x00_c8d2);
        t.button_primary_hover = c(0x1f_d6df);
        t.button_primary_active = c(0x00_aab3);
        t.button_primary_foreground = c(0xff_ffff);
        t.accent = c(0x33_3333);
        t.secondary = c(0x33_3333);
    });
}
