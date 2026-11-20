//! 电平表（剪映时间轴右边那两条竖的）：播放时显示左右声道的峰值，-60 dB 到 0 dB；
//! 过了 -12 dB 变黄、过了 -3 dB 变红。

use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _, div, px,
    relative, rgb,
};
use ve_engine::level::meter_fraction;

/// 电平表的宽度。
pub(crate) const METER_W: f32 = 22.0;

/// 峰值对应的颜色。
fn level_color(fraction: f32) -> Hsla {
    // -12 dB = 0.8、-3 dB = 0.95（60 dB 刻度）
    if fraction >= 0.95 {
        rgb(0xE5_484D).into()
    } else if fraction >= 0.8 {
        rgb(0xF5_C451).into()
    } else {
        rgb(0x3F_C27A).into()
    }
}

impl Workspace {
    /// 时间轴右边的电平表。
    pub(crate) fn render_level_meter(&self, _cx: &Context<'_, Self>) -> impl IntoElement + use<> {
        let levels = self.engine.levels();
        let bar = |peak: f32| {
            let f = meter_fraction(peak);
            div()
                .relative()
                .w(px(6.))
                .h_full()
                .rounded(px(2.))
                .bg(gpui_kit::rgba(0xffff_ff14))
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .w_full()
                        .h(relative(f))
                        .rounded(px(2.))
                        .bg(level_color(f)),
                )
        };
        v_flex()
            .id("level-meter")
            .test_support()
            .flex_none()
            .w(px(METER_W))
            .h_full()
            .py_2()
            .items_center()
            .child(
                h_flex()
                    .flex_1()
                    .gap(px(2.))
                    .child(bar(levels[0]))
                    .child(bar(levels[1])),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_change_near_the_top() {
        assert_eq!(level_color(0.5), rgb(0x3F_C27A).into());
        assert_eq!(level_color(0.85), rgb(0xF5_C451).into());
        assert_eq!(level_color(1.0), rgb(0xE5_484D).into());
    }
}
