//! 草稿封面（剪映主轨上的「封面」）：用播放头这一帧，或者导入一张本地图片；导出时可以一起导出。

use crate::chrome;
use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, MouseButton,
    ParentElement as _, PathPromptOptions, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, deferred, div, px,
};
use ve_core::{Cover, format_timecode};

impl Workspace {
    /// 设草稿封面（一步撤销；`None` 清掉）。
    pub fn set_cover(
        &mut self,
        cover: Option<Cover>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.timeline.cover_menu = false;
        self.apply(window, cx, |p| {
            p.timeline.cover = cover;
            Ok(())
        });
    }

    fn import_cover(&mut self, window: &Window, cx: &Context<'_, Self>) {
        self.timeline.cover_menu = false;
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择封面图片".into()),
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    this.set_cover(Some(Cover::Image(path)), window, cx);
                });
            }
        }));
    }

    /// 主轨轨道头上的「封面」按钮（只有主轨有）和它的菜单；`top` 是这一行按钮的纵坐标。
    pub(crate) fn render_cover_button(&self, top: f32, cx: &Context<'_, Self>) -> AnyElement {
        let cover = self.doc.project.timeline.cover.clone();
        let rate = self.doc.project.settings.frame_rate;
        let tip: SharedString = match &cover {
            None => "封面：设置草稿封面（导出时可以一起导出）".into(),
            Some(Cover::Frame(t)) => format!("封面：{} 那一帧", format_timecode(*t, rate)).into(),
            Some(Cover::Image(p)) => format!(
                "封面：{}",
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )
            .into(),
        };
        let icon = Icon::new(Lucide::Image)
            .size(px(14.))
            .text_color(if cover.is_some() {
                palette::accent()
            } else {
                palette::tick()
            });
        let item = |id: &'static str, label: &'static str| {
            h_flex()
                .id(id)
                .test_support()
                .h(px(26.))
                .px_2()
                .rounded(px(4.))
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.bg(palette::menu_hover()))
                .child(label)
        };
        div()
            .absolute()
            .top(px(top))
            .left(px(22.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Button::new("track-cover")
                    .ghost()
                    .xsmall()
                    .icon(icon)
                    .tooltip(tip)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.timeline.cover_menu = !this.timeline.cover_menu;
                        cx.notify();
                    })),
            )
            .children(self.timeline.cover_menu.then(|| {
                let playhead = self.playhead_frame();
                deferred(
                    v_flex()
                        .id("cover-menu")
            // 盖住下面的东西：点菜单不会顺带点到下面的画面 / 时间轴
            .occlude()
                        .test_support()
                        .absolute()
                        .top(px(24.))
                        .left_0()
                        .w(px(180.))
                        .p_1()
                        .bg(palette::menu_bg())
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(px(chrome::CARD_RADIUS))
                        .shadow_lg()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.timeline.cover_menu = false;
                            cx.notify();
                        }))
                        .child(
                            item("cover-use-frame", "用播放头这一帧").on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.set_cover(Some(Cover::Frame(playhead)), window, cx);
                                },
                            )),
                        )
                        .child(item("cover-import", "从本地导入图片…").on_click(
                            cx.listener(|this, _, window, cx| this.import_cover(window, cx)),
                        ))
                        .children(cover.is_some().then(|| {
                            item("cover-clear", "清除封面").on_click(
                                cx.listener(|this, _, window, cx| this.set_cover(None, window, cx)),
                            )
                        })),
                )
                .with_priority(1)
            }))
            .into_any_element()
    }
}
