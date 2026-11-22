//! 属性面板滑条右边的数值：点一下变成输入框，输入数值回车（或点别处）就套上（剪映每个数值都能
//! 直接输入）。写回滑条后照样发出「拖动 / 松开」事件，各面板原来的处理（一步撤销、有关键帧时
//! 打帧）都不用改。

use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, px,
};

/// 输入框里的数和滑条的值怎么换算。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ValueMap {
    /// 一样。
    Plain,
    /// 滑条是对数刻度的倍速，输入框里是倍数（2 = 2x）。
    Speed,
    /// 滑条是对数刻度的倍速，输入框里是时长（秒）；素材那一段长这么多秒。
    Duration(f32),
}

impl ValueMap {
    /// 滑条的值 → 输入框里的文字。
    fn to_text(self, v: f32) -> String {
        match self {
            Self::Plain => format_value(v),
            Self::Speed => format_value(crate::inspector_speed::from_slider(v)),
            Self::Duration(span) => format_value(span / crate::inspector_speed::from_slider(v)),
        }
    }

    /// 输入的数 → 滑条的值；没意义（时长 ≤ 0）时 `None`。
    fn to_slider(self, x: f32) -> Option<f32> {
        match self {
            Self::Plain => Some(x),
            Self::Speed => (x > 0.0).then(|| crate::inspector_speed::to_slider_exact(x)),
            Self::Duration(span) => {
                (x > 0.0).then(|| crate::inspector_speed::to_slider_exact(span / x))
            }
        }
    }
}

/// 正在输入的那个数值。
pub(crate) struct ValueEdit {
    /// 哪个数值框（同一个滑条可以有两个框：倍数和时长）。
    name: SharedString,
    /// 怎么换算。
    map: ValueMap,
    /// 输进去的数值写回这个滑条。
    slider: Entity<SliderState>,
    input: Entity<InputState>,
    _sub: Subscription,
}

impl std::fmt::Debug for ValueEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValueEdit").finish_non_exhaustive()
    }
}

/// 输入框里的文字 → 数值（允许带 `%`、`°`、`dB` 这类单位和空格）。
fn parse_value(text: &str) -> Option<f32> {
    let t = text
        .trim()
        .trim_end_matches(['%', '°', 'x', 'X', 's', '秒'])
        .trim_end_matches("dB")
        .trim_end_matches("db")
        .trim();
    t.parse::<f32>().ok().filter(|v| v.is_finite())
}

/// 滑条的值写成输入框里的文字：整数不带小数点，其余最多两位小数。
fn format_value(v: f32) -> String {
    let r = (v * 100.0).round() / 100.0;
    if (r - r.round()).abs() < 1e-6 {
        format!("{}", r.round())
    } else {
        format!("{r}")
    }
}

impl Workspace {
    /// 滑条右边的数值 `shown`（编号 `{id}-value`）：点一下就能输入。
    pub(crate) fn value_box(
        &self,
        id: &str,
        slider: &Entity<SliderState>,
        shown: impl Into<SharedString>,
        width: f32,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        self.value_box_mapped(id, slider, shown, width, ValueMap::Plain, cx)
    }

    /// 同 [`Self::value_box`]，输入的数按 `map` 换算成滑条的值。
    pub(crate) fn value_box_mapped(
        &self,
        id: &str,
        slider: &Entity<SliderState>,
        shown: impl Into<SharedString>,
        width: f32,
        map: ValueMap,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        let name = SharedString::from(format!("{id}-value"));
        if let Some(edit) = self
            .value_edit
            .as_ref()
            .filter(|e| e.slider.entity_id() == slider.entity_id() && e.name == name)
        {
            return div()
                .id(name)
                .test_support()
                .w(px(width + 16.0))
                .flex_none()
                .child(Input::new(&edit.input).xsmall())
                .into_any_element();
        }
        let slider = slider.clone();
        let key = name.clone();
        div()
            .id(name)
            .test_support()
            .w(px(width))
            .flex_none()
            .text_sm()
            .cursor_text()
            .rounded(px(3.))
            .hover(|s| s.bg(cx.theme().muted))
            .child(shown.into())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.start_value_edit(key.clone(), map, &slider, window, cx);
            }))
            .into_any_element()
    }

    /// 开始输入：输入框里是滑条现在的值，全选好。
    fn start_value_edit(
        &mut self,
        name: SharedString,
        map: ValueMap,
        slider: &Entity<SliderState>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let now = map.to_text(slider.read(cx).value().start());
        let input = cx.new(|cx| InputState::new(window, cx).default_value(now));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit_value_edit(window, cx);
                }
            },
        );
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.value_edit = Some(ValueEdit {
            name,
            map,
            slider: slider.clone(),
            input,
            _sub: sub,
        });
        cx.notify();
    }

    /// 输完了：数值夹到滑条的范围里写回滑条，发出「拖动 + 松开」（一步撤销）；读不懂就不改。
    fn commit_value_edit(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(edit) = self.value_edit.take() else {
            return;
        };
        let text = edit.input.read(cx).value();
        if let Some(v) = parse_value(&text).and_then(|x| edit.map.to_slider(x)) {
            edit.slider.update(cx, |s, cx| {
                let v = v.clamp(s.min_value(), s.max_value());
                s.set_value(v, window, cx);
                cx.emit(SliderEvent::Change(SliderValue::from(v)));
                cx.emit(SliderEvent::Release(SliderValue::from(v)));
            });
        }
        // 输入框没了：焦点还给工作区，快捷键、工具栏按钮接着能用
        window.focus(&self.focus, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_values_accept_units_and_show_tidily() {
        assert_eq!(parse_value(" -50 "), Some(-50.0));
        assert_eq!(parse_value("70%"), Some(70.0));
        assert_eq!(parse_value("-3.5 dB"), Some(-3.5));
        assert_eq!(parse_value("45°"), Some(45.0));
        assert_eq!(parse_value("2x"), Some(2.0));
        assert_eq!(parse_value("3.5 s"), Some(3.5));
        // 倍数、时长和对数滑条之间换算
        let v = ValueMap::Speed.to_slider(2.0).unwrap();
        assert_eq!(ValueMap::Speed.to_text(v), "2");
        assert_eq!(ValueMap::Duration(8.0).to_text(v), "4");
        assert_eq!(ValueMap::Duration(8.0).to_slider(0.0), None);
        assert_eq!(parse_value("abc"), None);
        assert_eq!(parse_value("inf"), None);
        assert_eq!(format_value(100.0), "100");
        assert_eq!(format_value(-0.333), "-0.33");
    }
}
