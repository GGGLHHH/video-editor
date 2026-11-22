//! 剪映（FCP X 键位）里另外几个常用快捷键：↑ / ↓ 上一个 / 下一个分割点、Shift+← / → 大步移动、
//! Shift+Alt+K 添加关键帧、Ctrl+R 唤起变速面板、Ctrl+. / Ctrl+, 音量 ±0.1 dB、Tab 切换素材面板、
//! Ctrl+Q 退出（先保存）。

use crate::actions as act;
use crate::inspector::InspectorTab;
use crate::media_bin::BinTab;
use crate::workspace::Workspace;
use gpui_kit::InteractiveElement as _;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{Context, IntoElement as _, ParentElement as _, Styled as _, Window, div, px};
use ve_core::Time;

/// Shift+← / → 一次走多少帧（FCP X 是 10 帧）。
const BIG_STEP_FRAMES: i64 = 10;

/// Ctrl+Alt+= / - 一次把轨道调高 / 调低多少像素，和轨道高度的上下限。
const TRACK_HEIGHT_STEP: f32 = 8.0;
const TRACK_HEIGHT_RANGE: (f32, f32) = (32.0, 112.0);

/// Ctrl+. / Ctrl+, 一次调多少音量（dB）。
const VOLUME_STEP_DB: f32 = 0.1;

impl Workspace {
    /// 时间轴每条轨道多高（像素）。
    #[must_use]
    pub const fn track_height(&self) -> f32 {
        self.timeline.geometry.track_height
    }

    /// 时间轴第 `i` 行的（顶边纵坐标，高）：文字轨、特效轨比普通轨矮。
    #[must_use]
    pub fn timeline_row(&self, i: usize) -> (f32, f32) {
        let g = &self.timeline.geometry;
        (g.track_top(i), g.row_height(i))
    }

    fn change_track_height(&mut self, by: f32, cx: &mut Context<'_, Self>) {
        let g = &mut self.timeline.geometry;
        g.track_height = (g.track_height + by).clamp(TRACK_HEIGHT_RANGE.0, TRACK_HEIGHT_RANGE.1);
        cx.notify();
    }

    pub(crate) fn track_taller(
        &mut self,
        _: &act::TrackTaller,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.change_track_height(TRACK_HEIGHT_STEP, cx);
    }

    pub(crate) fn track_shorter(
        &mut self,
        _: &act::TrackShorter,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.change_track_height(-TRACK_HEIGHT_STEP, cx);
    }

    /// 选中的视频片段里能分离 / 还原音频的：（全都已经分离过，片段）。
    pub(crate) fn audio_split_targets(&self) -> (bool, Vec<ve_core::ClipId>) {
        let tl = &self.doc.project.timeline;
        let clips: Vec<(ve_core::ClipId, bool)> = self
            .doc
            .selection()
            .iter()
            .filter_map(|&id| {
                let (track, c) = tl.find_clip(id)?;
                let video = tl.track(track)?.kind == ve_core::TrackKind::Video;
                let sounds = self
                    .doc
                    .project
                    .assets
                    .get(&c.asset()?)
                    .is_some_and(|a| a.audio.is_some());
                (video && sounds).then_some((id, c.audio_split))
            })
            .collect();
        let all_split = !clips.is_empty() && clips.iter().all(|(_, s)| *s);
        (all_split, clips.into_iter().map(|(id, _)| id).collect())
    }

    /// Ctrl+Shift+S：选中的视频片段分离音频；都已经分离过就还原（剪映）。
    pub(crate) fn separate_audio(
        &mut self,
        _: &act::SeparateAudio,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let (all_split, clips) = self.audio_split_targets();
        if clips.is_empty() {
            return;
        }
        if all_split {
            self.apply(window, cx, |p| ve_core::edit::restore_audio(p, &clips));
        } else {
            self.apply(window, cx, |p| {
                ve_core::edit::separate_audio(p, &clips).map(|_| ())
            });
        }
    }

    /// 素材面板现在是哪个分类。
    #[must_use]
    pub const fn bin_tab(&self) -> BinTab {
        self.bin_tab
    }

    /// 属性面板现在是哪个标签。
    #[must_use]
    pub const fn inspector_tab(&self) -> InspectorTab {
        self.inspector.tab
    }

    /// 时间轴上所有分割点（片段的头尾、0、结尾），排好、去重。
    fn edit_points(&self) -> Vec<Time> {
        let tl = &self.doc.project.timeline;
        let mut points: Vec<Time> = tl
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().flat_map(|c| [c.start, c.end()]))
            .chain([Time::ZERO, tl.duration()])
            .collect();
        points.sort_unstable();
        points.dedup();
        points
    }

    pub(crate) fn next_edit_point(
        &mut self,
        _: &act::NextEditPoint,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let now = self.playhead;
        if let Some(&t) = self.edit_points().iter().find(|&&t| t > now) {
            self.engine.pause();
            self.seek(t, cx);
        }
    }

    pub(crate) fn prev_edit_point(
        &mut self,
        _: &act::PrevEditPoint,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let now = self.playhead;
        if let Some(&t) = self.edit_points().iter().rev().find(|&&t| t < now) {
            self.engine.pause();
            self.seek(t, cx);
        }
    }

    pub(crate) fn big_step_forward(
        &mut self,
        _: &act::BigStepForward,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.step(BIG_STEP_FRAMES, cx);
    }

    pub(crate) fn big_step_back(
        &mut self,
        _: &act::BigStepBack,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.step(-BIG_STEP_FRAMES, cx);
    }

    pub(crate) fn add_keyframe_here(
        &mut self,
        _: &act::AddKeyframe,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.toggle_keyframe(window, cx);
    }

    /// Ctrl+R：选中的是有画面或声音的素材片段时，属性面板换到「变速」。
    pub(crate) fn show_speed(
        &mut self,
        _: &act::ShowSpeed,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let media = self
            .inspector
            .bound
            .and_then(|id| self.doc.project.timeline.find_clip(id))
            .is_some_and(|(_, c)| c.asset().is_some());
        if media {
            self.inspector.tab = InspectorTab::Speed;
            self.sync_inspector(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn volume_up(
        &mut self,
        _: &act::VolumeUp,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.nudge_volume(VOLUME_STEP_DB, window, cx);
    }

    pub(crate) fn volume_down(
        &mut self,
        _: &act::VolumeDown,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.nudge_volume(-VOLUME_STEP_DB, window, cx);
    }

    /// 选中的有声音的片段音量都加 `db`（有音量关键帧的在播放头处打帧），一步撤销。
    fn nudge_volume(&mut self, db: f32, window: &mut Window, cx: &mut Context<'_, Self>) {
        let frame = self.doc.project.settings.frame_rate.frame_duration();
        let targets: Vec<(ve_core::ClipId, Time, f32)> = self
            .doc
            .selection()
            .iter()
            .filter_map(|&id| {
                let (_, c) = self.doc.project.timeline.find_clip(id)?;
                c.asset()?;
                let at = c.key_offset(self.playhead, frame);
                Some((id, at, c.gain_db_at(at)))
            })
            .collect();
        if targets.is_empty() {
            return;
        }
        self.apply(window, cx, |p| {
            for &(id, at, now) in &targets {
                ve_core::edit::apply_gain_at(p, id, at, now + db)?;
            }
            Ok(())
        });
        self.refresh_animated_sliders(true, window, cx);
    }

    /// Tab：素材面板换到下一个分类（最后一个之后回到第一个）。
    pub(crate) fn next_bin_tab(
        &mut self,
        _: &act::NextBinTab,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let i = BinTab::ALL
            .iter()
            .position(|&t| t == self.bin_tab)
            .unwrap_or(0);
        self.bin_tab = BinTab::ALL[(i + 1) % BinTab::ALL.len()];
        cx.notify();
    }

    /// 退出前：在复合片段里面先回到最外层，有没保存的改动就马上自动保存一次。
    pub fn save_before_quit(&mut self, cx: &mut Context<'_, Self>) {
        self.autosave_now(cx);
    }

    pub(crate) fn quit(&mut self, _: &act::Quit, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.save_before_quit(cx);
        cx.quit();
    }

    /// 帮助 › 快捷键：打开快捷键一览。
    pub(crate) fn show_shortcuts(
        &mut self,
        _: &act::ShowShortcuts,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.popup = Some(crate::workspace::Popup::Shortcuts);
        cx.notify();
    }

    /// 第一次打开时的新手引导：几条最常用的操作，点「知道了」后记进全局设置，不再弹。
    pub fn show_guide_if_new(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        use gpui_kit::component::button::ButtonVariants as _;
        if self.settings.seen_guide {
            return;
        }
        let ws = cx.entity();
        let muted = cx.theme().muted_foreground;
        let tips = [
            (
                "导入素材",
                "把视频、音频、图片拖进窗口，或在素材面板点「导入」；拖到下面的时间轴开始剪",
            ),
            (
                "剪",
                "空格播放 / 暂停，Ctrl+B 在播放头处分割，Delete 删除，拖片段两头修剪",
            ),
            (
                "调",
                "点时间轴上的片段，右边属性面板改画面、音频、变速、动画、调整",
            ),
            (
                "导出",
                "右上角「导出」（Ctrl+E）：选分辨率、帧率、格式，草稿会自动保存",
            ),
            ("快捷键", "顶栏 ⌨ 看全部快捷键，点一个键就能改"),
        ];
        window.open_dialog(cx, move |dialog, _, _| {
            let ws = ws.clone();
            dialog
                .title("欢迎使用")
                .width(px(480.))
                .child(
                    v_flex()
                        .id("guide")
                        .test_support()
                        .gap_3()
                        .children(tips.map(|(title, text)| {
                            v_flex()
                                .gap_0p5()
                                .child(div().text_sm().child(title))
                                .child(div().text_xs().text_color(muted).child(text))
                        })),
                )
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new().child(
                        gpui_kit::component::dialog::DialogAction::new().child(
                            gpui_kit::component::button::Button::new("guide-ok")
                                .primary()
                                .label("知道了"),
                        ),
                    ),
                )
                .on_ok(move |_, _, cx| {
                    ws.update(cx, |this, cx| {
                        let mut s = this.settings.clone();
                        s.seen_guide = true;
                        this.use_settings(s, None, cx);
                    });
                    true
                })
        });
    }

    /// 帮助 › 关于：名字和版本号。
    #[expect(clippy::unused_self, reason = "动作处理函数要挂在工作区上")]
    pub(crate) fn about(
        &mut self,
        _: &act::About,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let muted = cx.theme().muted_foreground;
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.title("关于").width(px(360.)).child(
                v_flex()
                    .gap_2()
                    .child(div().text_lg().child("视频剪辑器"))
                    .child(
                        div()
                            .id("about-version")
                            .test_support()
                            .text_sm()
                            .child(format!("版本 {}", env!("CARGO_PKG_VERSION"))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Rust + GPUI；视频编解码用 FFmpeg（LGPL）")
                            .into_any_element(),
                    ),
            )
        });
    }
}
