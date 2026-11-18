//! 主窗口：持有文档和播放引擎，布局四个面板，处理动作。
//!
//! 各面板的渲染分在 `media_bin` / `preview` / `inspector` / `timeline::view` / `export_ui` 里，
//! 都是 `impl Workspace` 的方法；纯逻辑在 `document`、`timeline::{geometry, interaction}`。

use crate::actions as act;
use crate::chrome;
use crate::document::Document;
use crate::inspector::Inspector;
use crate::preview::PreviewState;
use crate::theme::palette;
use crate::timeline::view::TimelineUi;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, ExternalPaths, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, RenderImage, Styled as _, Subscription, Task, Window,
    div, px,
};
use std::collections::HashMap;
use std::sync::Arc;
use ve_core::{AssetId, EditError, Project, Time};
use ve_engine::{Engine, EngineEvent};

/// 素材的缩略图（已转成可直接显示的图片）和波形。
#[derive(Debug, Clone, Default)]
pub struct MediaThumbs {
    /// 分析的是哪个文件（素材编号可能被重复使用）。
    pub path: std::path::PathBuf,
    /// 缩略图。
    pub thumbnails: Vec<Arc<RenderImage>>,
    /// 波形。
    pub peaks: Arc<Vec<(f32, f32)>>,
}

/// J / L 穿梭播放的最高倍数。
const MAX_SHUTTLE: f32 = 8.0;

/// 主窗口视图。
pub struct Workspace {
    pub(crate) doc: Document,
    pub(crate) engine: Engine,
    /// 推给引擎的工程（也给时间轴绘制用）。
    pub(crate) project: Arc<Project>,
    engine_revision: Option<u64>,
    pub(crate) focus: FocusHandle,
    pub(crate) media: HashMap<AssetId, MediaThumbs>,
    pub(crate) preview: PreviewState,
    pub(crate) playhead: Time,
    pub(crate) timeline: TimelineUi,
    /// 吸附开关：时间轴拖片段、预览上移动和旋转都看它；按住 Alt 临时取反。
    pub(crate) snapping: bool,
    pub(crate) inspector: Inspector,
    pub(crate) export: Option<crate::export_ui::ExportProgress>,
    /// 上一次成功导出的文件。
    pub(crate) last_export: Option<std::path::PathBuf>,
    pub(crate) importing: usize,
    /// 最近打开的工程，及其存放文件。
    pub(crate) recent: crate::recent::RecentList,
    pub(crate) recent_file: std::path::PathBuf,
    /// 「转场」标签里选的时长（秒）。
    pub(crate) transition_secs: f64,
    /// 布局模式。
    pub(crate) layout: crate::layout::LayoutMode,
    /// 顶栏「布局」菜单是否展开。
    pub(crate) layout_menu: bool,
    /// 片段 / 属性剪贴板。
    pub(crate) clipboard: crate::clip_ops::ClipBoard,
    /// 自动保存（正式启动的程序才打开）。
    pub(crate) autosave: Option<crate::autosave::Autosave>,
    /// 素材面板当前的一级标签。
    pub(crate) bin_tab: crate::media_bin::BinTab,
    /// 顶栏「菜单」是否展开。
    pub(crate) menu_open: bool,
    title: String,
    pub(crate) tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl Workspace {
    /// 新建主窗口视图（空工程）。
    pub fn new(engine: Engine, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let events = engine.events();
        // 引擎事件用定时轮询（不 await 跨线程的通道）：引擎线程不直接唤醒界面任务，
        // 测试调度器才能保持确定性；空闲开销是每秒几十次 try_recv。
        let listen = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(15))
                    .await;
                let mut any = None;
                while let Ok(event) = events.try_recv() {
                    any = Some(event);
                }
                let alive = match any {
                    Some(event) => this
                        .update(cx, |_, cx| Self::on_engine_event(event, cx))
                        .is_ok(),
                    None => this.update(cx, |_, _| ()).is_ok(),
                };
                if !alive {
                    break;
                }
            }
        });
        let (inspector, subscriptions) = Inspector::new(window, cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let doc = Document::new();
        Self {
            project: Arc::new(doc.project.clone()),
            doc,
            engine,
            engine_revision: None,
            focus,
            media: HashMap::new(),
            preview: PreviewState::default(),
            playhead: Time::ZERO,
            snapping: true,
            timeline: TimelineUi::default(),
            inspector,
            export: None,
            last_export: None,
            importing: 0,
            recent: crate::recent::RecentList::load(&crate::recent::default_file()),
            recent_file: crate::recent::default_file(),
            menu_open: false,
            bin_tab: crate::media_bin::BinTab::default(),
            autosave: None,
            clipboard: crate::clip_ops::ClipBoard::default(),
            layout: crate::layout::LayoutMode::default(),
            transition_secs: 1.0,
            layout_menu: false,
            title: String::new(),
            tasks: vec![listen],
            _subscriptions: subscriptions,
        }
    }

    /// 当前文档（测试和外部查询用）。
    #[must_use]
    pub const fn document(&self) -> &Document {
        &self.doc
    }

    /// 播放引擎。
    #[must_use]
    pub const fn engine(&self) -> &Engine {
        &self.engine
    }

    fn on_engine_event(event: EngineEvent, cx: &mut Context<'_, Self>) {
        match event {
            EngineEvent::FrameReady | EngineEvent::Ended => cx.notify(),
        }
    }

    /// 工程变了就推给引擎。
    pub(crate) fn sync_engine(&mut self) {
        if self.engine_revision != Some(self.doc.revision()) {
            self.project = Arc::new(self.doc.project.clone());
            self.engine.set_project(Arc::clone(&self.project));
            self.engine_revision = Some(self.doc.revision());
        }
    }

    /// 换掉整个文档（新建 / 打开）。
    pub(crate) fn replace_document(
        &mut self,
        doc: Document,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.engine.pause();
        self.doc = doc;
        self.engine_revision = None;
        for thumbs in std::mem::take(&mut self.media).into_values() {
            for image in thumbs.thumbnails {
                cx.drop_image(image, Some(window));
            }
        }
        self.sync_engine();
        self.engine.seek(Time::ZERO);
        self.inspector.unbind();
        self.after_change(window, cx);
        let assets: Vec<_> = self.doc.project.assets.values().cloned().collect();
        self.analyze_assets(assets, window, cx);
    }

    /// 执行一次编辑；失败时弹提示。
    pub(crate) fn apply<T>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
        f: impl FnOnce(&mut Project) -> Result<T, EditError>,
    ) -> Option<T> {
        match self.doc.edit(f) {
            Ok(v) => {
                self.after_change(window, cx);
                Some(v)
            }
            Err(e) => {
                notify(Notification::warning(e.to_string()), window, cx);
                None
            }
        }
    }

    /// 文档变化后：同步引擎、属性面板，重绘。
    pub(crate) fn after_change(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.clamp_timeline_scroll();
        self.sync_engine();
        self.sync_inspector(window, cx);
        cx.notify();
    }

    pub(crate) fn seek(&mut self, t: Time, cx: &mut Context<'_, Self>) {
        self.engine.seek(t);
        self.playhead = self.engine.position();
        cx.notify();
    }

    // ---- 播放控制 ----

    fn play_pause(&mut self, _: &act::PlayPause, _: &mut Window, cx: &mut Context<'_, Self>) {
        if self.engine.is_playing() {
            self.engine.pause();
        } else {
            self.engine.play();
        }
        cx.notify();
    }

    fn pause(&mut self, _: &act::Pause, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.engine.pause();
        cx.notify();
    }

    fn play_forward(&mut self, _: &act::PlayForward, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.shuttle(1.0, cx);
    }

    fn play_backward(&mut self, _: &act::PlayBackward, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.shuttle(-1.0, cx);
    }

    /// J / L（剪映、Premiere 的穿梭播放）：往同一个方向再按一次倍数翻倍（最多 8 倍），
    /// 换方向从 1 倍开始；K 暂停，空格回到原速。
    fn shuttle(&self, direction: f32, cx: &mut Context<'_, Self>) {
        let rate = self.engine.rate();
        let next = if self.engine.is_playing() && rate * direction > 0.0 {
            (rate * 2.0).clamp(-MAX_SHUTTLE, MAX_SHUTTLE)
        } else {
            direction
        };
        self.engine.play_at_rate(next);
        cx.notify();
    }

    fn step(&mut self, frames: i64, cx: &mut Context<'_, Self>) {
        self.engine.pause();
        let rate = self.doc.project.settings.frame_rate;
        let current = rate.frame_of(rate.snap(self.engine.position()));
        self.seek(rate.time_of_frame(current + frames), cx);
    }

    fn step_forward(&mut self, _: &act::StepForward, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.step(1, cx);
    }

    fn step_back(&mut self, _: &act::StepBack, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.step(-1, cx);
    }

    fn go_start(&mut self, _: &act::GoStart, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.seek(Time::ZERO, cx);
    }

    fn go_end(&mut self, _: &act::GoEnd, _: &mut Window, cx: &mut Context<'_, Self>) {
        let end = self.doc.project.timeline.duration();
        self.seek(end, cx);
    }

    // ---- 编辑 ----

    fn undo(&mut self, _: &act::Undo, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.doc.undo() {
            self.inspector.unbind();
            self.after_change(window, cx);
        }
    }

    fn redo(&mut self, _: &act::Redo, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.doc.redo() {
            self.inspector.unbind();
            self.after_change(window, cx);
        }
    }

    /// 播放头所在的帧（对齐到帧）。
    fn playhead_frame(&self) -> Time {
        self.doc
            .project
            .settings
            .frame_rate
            .snap(self.engine.position())
    }

    /// 分割（Ctrl+B）：切开选中的片段，右半段自动选中（剪映）；没选中时切开播放头下的所有片段。
    fn split(&mut self, _: &act::Split, window: &mut Window, cx: &mut Context<'_, Self>) {
        let at = self.playhead_frame();
        let selected = self.doc.selection().to_vec();
        if selected.is_empty() {
            self.apply(window, cx, |p| Ok(ve_core::edit::split_all_at(p, at)));
            return;
        }
        let rights = self.apply(window, cx, |p| {
            // 选中的片段里有些可能不跨过播放头，忽略即可
            Ok(selected
                .iter()
                .filter_map(|&clip| ve_core::edit::split_at(p, clip, at).ok())
                .collect::<Vec<_>>())
        });
        if let Some(rights) = rights.filter(|r| !r.is_empty()) {
            self.doc.set_selection(rights);
            self.after_change(window, cx);
        }
    }

    /// 批量分割（Ctrl+Shift+B）：不管选中什么，切开播放头下所有轨道的片段。
    fn split_all(&mut self, _: &act::SplitAll, window: &mut Window, cx: &mut Context<'_, Self>) {
        let at = self.playhead_frame();
        self.apply(window, cx, |p| Ok(ve_core::edit::split_all_at(p, at)));
    }

    /// Q / W 的目标：选中的片段；没选中时是播放头下的所有片段。
    fn trim_targets(&self) -> Vec<ve_core::ClipId> {
        let selected = self.doc.selection().to_vec();
        if selected.is_empty() {
            let at = self.playhead_frame();
            self.doc
                .project
                .timeline
                .tracks
                .iter()
                .filter_map(|t| t.clip_at(at).map(|c| c.id))
                .collect()
        } else {
            selected
        }
    }

    /// 向左裁剪（Q）：剪掉播放头左边的部分。
    fn trim_left(&mut self, _: &act::TrimLeft, window: &mut Window, cx: &mut Context<'_, Self>) {
        let (at, targets) = (self.playhead_frame(), self.trim_targets());
        self.apply(window, cx, |p| ve_core::edit::trim_left_at(p, &targets, at));
    }

    /// 向右裁剪（W）：剪掉播放头右边的部分。
    fn trim_right(&mut self, _: &act::TrimRight, window: &mut Window, cx: &mut Context<'_, Self>) {
        let (at, targets) = (self.playhead_frame(), self.trim_targets());
        self.apply(window, cx, |p| {
            ve_core::edit::trim_right_at(p, &targets, at)
        });
    }

    /// 向左全选（[）：起点在播放头左边的所有片段。
    fn select_left(
        &mut self,
        _: &act::SelectLeft,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let clips = self
            .doc
            .project
            .timeline
            .clips_before(self.engine.position());
        self.doc.set_selection(clips);
        self.after_change(window, cx);
    }

    /// 向右全选（]）：终点在播放头右边的所有片段。
    fn select_right(
        &mut self,
        _: &act::SelectRight,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let clips = self
            .doc
            .project
            .timeline
            .clips_after(self.engine.position());
        self.doc.set_selection(clips);
        self.after_change(window, cx);
    }

    /// Esc：正在拖动（时间轴或预览）时取消这次拖动、回到按下前，不记撤销；
    /// 正在框选时回到框之前的选择；否则取消选择。
    fn cancel(&mut self, _: &act::Cancel, window: &mut Window, cx: &mut Context<'_, Self>) {
        let preview = self.preview.drag.take().is_some();
        if preview {
            crate::platform_cursor::clear();
        }
        let (timeline_busy, timeline_drag) = self.cancel_timeline_gesture();
        if preview || timeline_drag {
            self.doc.cancel_gesture();
            // 预览拖动时属性面板的数字跟着变过，还原后也要填回原值
            if let Some((_, t, _)) = self.bound_video_clip() {
                self.fill_transform(&t, window, cx);
            }
        } else if !timeline_busy {
            self.doc.clear_selection();
        }
        self.after_change(window, cx);
    }

    /// 当前的鼠标样式：（时间轴，预览）。
    #[must_use]
    pub const fn pointer_cursors(&self) -> (gpui_kit::CursorStyle, gpui_kit::CursorStyle) {
        (self.timeline.cursor, self.preview.cursor)
    }

    fn toggle_snapping(
        &mut self,
        _: &act::ToggleSnapping,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.snapping = !self.snapping;
        cx.notify();
    }

    /// 主轨磁吸开关（P）：存在工程设置里，算一步撤销；打开时主轨立刻排紧。
    fn toggle_magnet(
        &mut self,
        _: &act::ToggleMagnet,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.apply(window, cx, |p| {
            p.settings.main_magnet = !p.settings.main_magnet;
            Ok(())
        });
    }

    /// 联动开关（~）：字幕跟着主轨片段走。
    fn toggle_linkage(
        &mut self,
        _: &act::ToggleLinkage,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.apply(window, cx, |p| {
            p.settings.linkage = !p.settings.linkage;
            Ok(())
        });
    }

    fn select_all(&mut self, _: &act::SelectAll, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.doc.select_all();
        self.after_change(window, cx);
    }

    fn deselect_all(
        &mut self,
        _: &act::DeselectAll,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.doc.clear_selection();
        self.after_change(window, cx);
    }

    fn delete(&mut self, _: &act::Delete, window: &mut Window, cx: &mut Context<'_, Self>) {
        let selected = self.doc.selection().to_vec();
        if !selected.is_empty() {
            self.apply(window, cx, |p| ve_core::edit::delete_clips(p, &selected));
        }
    }

    fn ripple_delete(
        &mut self,
        _: &act::RippleDelete,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let selected = self.doc.selection().to_vec();
        if !selected.is_empty() {
            self.apply(window, cx, |p| ve_core::edit::ripple_delete(p, &selected));
        }
    }

    fn add_subtitle(
        &mut self,
        _: &act::AddSubtitle,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let at = self
            .doc
            .project
            .settings
            .frame_rate
            .snap(self.engine.position());
        if let Some(clip) = self.apply(window, cx, |p| {
            ve_core::edit::add_subtitle(p, at, Time::from_secs(2), "新字幕".into())
        }) {
            self.doc.select(clip, false);
            self.after_change(window, cx);
        }
    }

    /// 工具栏 / Ctrl+= 缩放：以播放头为中心（剪映）；播放头不在视图里时以视图中间为中心。
    fn zoom(&mut self, factor: f32, cx: &mut Context<'_, Self>) {
        let width = f32::from(self.timeline.bounds.size.width);
        let geo = &self.timeline.geometry;
        let playhead = geo.time_to_x(self.engine.position());
        let anchor = if (geo.header_width..=width).contains(&playhead) {
            playhead
        } else {
            width / 2.0
        };
        self.timeline.geometry.zoom_around(anchor, factor);
        self.clamp_timeline_scroll();
        cx.notify();
    }

    fn zoom_in(&mut self, _: &act::ZoomIn, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.zoom(1.25, cx);
    }

    fn zoom_out(&mut self, _: &act::ZoomOut, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.zoom(0.8, cx);
    }

    /// 根元素上挂所有动作（快捷键和菜单都发到这里）。
    fn bind_actions<E: gpui_kit::InteractiveElement>(el: E, cx: &Context<'_, Self>) -> E {
        el.on_action(cx.listener(Self::new_project))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::import))
            .on_action(cx.listener(Self::export))
            .on_action(cx.listener(Self::import_srt))
            .on_action(cx.listener(Self::export_srt))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::toggle_snapping))
            .on_action(cx.listener(Self::toggle_magnet))
            .on_action(cx.listener(Self::toggle_linkage))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy_attrs))
            .on_action(cx.listener(Self::paste_attrs))
            .on_action(cx.listener(Self::toggle_disabled))
            .on_action(cx.listener(Self::toggle_marker))
            .on_action(cx.listener(Self::next_marker))
            .on_action(cx.listener(Self::prev_marker))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::deselect_all))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::play_pause))
            .on_action(cx.listener(Self::pause))
            .on_action(cx.listener(Self::play_forward))
            .on_action(cx.listener(Self::play_backward))
            .on_action(cx.listener(Self::step_forward))
            .on_action(cx.listener(Self::step_back))
            .on_action(cx.listener(Self::go_start))
            .on_action(cx.listener(Self::go_end))
            .on_action(cx.listener(Self::split))
            .on_action(cx.listener(Self::split_all))
            .on_action(cx.listener(Self::trim_left))
            .on_action(cx.listener(Self::trim_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::ripple_delete))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::add_subtitle))
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.sync_engine();
        self.pull_frame(window, cx);
        self.measure_subtitle_font(window);
        self.playhead = self.engine.position();
        self.follow_playhead_in_inspector(window, cx);
        if self.engine.is_playing() {
            window.request_animation_frame();
        }
        let title = format!("{} — 视频剪辑器", self.doc.title());
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }

        Self::bind_actions(v_flex(), cx)
            .id("workspace")
            .test_support()
            .size_full()
            .key_context(act::CONTEXT)
            .track_focus(&self.focus)
            .bg(palette::window_bg())
            .text_color(cx.theme().foreground)
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.import_paths(paths.0.to_vec(), window, cx);
            }))
            .relative()
            .children(self.render_context_menu(window, cx))
            .child(self.render_topbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px(px(chrome::CARD_GAP))
                    .child(self.render_panels(window, cx)),
            )
    }
}

/// 弹出提示。
pub(crate) fn notify(note: Notification, window: &mut Window, cx: &mut App) {
    window.push_notification(note, cx);
}

/// 打开主窗口时用：在 `cx.new` 里构造。
pub fn build(engine: Engine, window: &mut Window, cx: &mut App) -> Entity<Workspace> {
    // 旋转光标要直接调系统（GPUI 没有自定义光标）；装不上（无界面测试、非 Windows）就退回手指光标
    crate::platform_cursor::install(window);
    cx.new(|cx| Workspace::new(engine, window, cx))
}
