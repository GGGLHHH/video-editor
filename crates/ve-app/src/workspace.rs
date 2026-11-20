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
use gpui_kit::prelude::FluentBuilder as _;
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

/// 顶栏的弹出框。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    /// 「菜单」。
    Menu,
    /// 「布局」。
    Layout,
    /// 快捷键一览。
    Shortcuts,
    /// 播放器标题右边的 ≡ 菜单。
    Player,
}

impl Workspace {
    /// 打开 / 关上一个弹出框（开着别的就换成它）。
    pub(crate) fn toggle_popup(&mut self, popup: Popup) {
        self.popup = (self.popup != Some(popup)).then_some(popup);
    }

    /// 在弹出框外按下鼠标：关掉它，记下按在哪（见 [`Self::click_popup_button`]）。
    pub(crate) const fn dismiss_popup(&mut self, at: gpui_kit::Point<gpui_kit::Pixels>) {
        if let Some(open) = self.popup.take() {
            self.popup_dismissed = Some((open, at));
        }
    }

    /// 点弹出框的按钮。弹出框开着时点它的按钮，按下那一刻弹出框已经因为「点到外面」关了，
    /// 松开时不能再把它打开——这一下就是要关。
    pub(crate) fn click_popup_button(&mut self, popup: Popup, e: &gpui_kit::ClickEvent) {
        let down = match e {
            gpui_kit::ClickEvent::Mouse(m) => Some(m.down.position),
            _ => None,
        };
        if let Some((closed, at)) = self.popup_dismissed.take()
            && closed == popup
            && Some(at) == down
        {
            return;
        }
        self.toggle_popup(popup);
    }

    /// 这个弹出框开着没有。
    pub(crate) fn popup_open(&self, popup: Popup) -> bool {
        self.popup == Some(popup)
    }
}

/// 主窗口视图。
pub struct Workspace {
    pub(crate) doc: Document,
    pub(crate) engine: Engine,
    /// 推给引擎的工程（也给时间轴绘制用）。
    pub(crate) project: Arc<Project>,
    /// 给播放引擎的工程：复合片段摊开（[`ve_core::compound::flatten`]）；没有复合片段时就是 `project`。
    pub(crate) engine_project: Arc<Project>,
    pub(crate) engine_revision: Option<u64>,
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
    /// 找不到文件的素材（[`Self::refresh_missing`]）。
    pub(crate) missing: std::collections::BTreeSet<AssetId>,
    /// 快捷键一览里正在改的键位（等用户按新键）。
    pub(crate) recording_key: Option<String>,
    /// 改键后的提示（比如新键原来用于别的操作，已经从那里去掉）。
    pub(crate) shortcut_note: Option<String>,
    /// 片段 / 属性剪贴板。
    pub(crate) clipboard: crate::clip_ops::ClipBoard,
    /// 自动保存（正式启动的程序才打开）。
    pub(crate) autosave: Option<crate::autosave::Autosave>,
    /// 代理模式（[`crate::proxy`]）。
    pub(crate) proxies: crate::proxy::Proxies,
    /// 示波器（[`crate::scopes`]）。
    pub(crate) scopes: crate::scopes::ScopeState,
    /// 插件目录（[`crate::plugins`]）。
    pub(crate) plugin_dir: std::path::PathBuf,
    /// 载入了的插件名。
    pub(crate) plugins: Vec<String>,
    /// 崩溃恢复的会话记录（[`Self::enable_recovery`]）。
    pub(crate) recovery: Option<crate::recovery::Live>,
    /// 素材面板当前的一级标签。
    pub(crate) bin_tab: crate::media_bin::BinTab,
    /// 顶栏展开着的弹出框（菜单、布局、快捷键），同时只开一个。
    pub(crate) popup: Option<Popup>,
    /// 刚被「点到外面」关掉的弹出框和按下的位置。
    pub(crate) popup_dismissed: Option<(Popup, gpui_kit::Point<gpui_kit::Pixels>)>,
    title: String,
    pub(crate) tasks: Vec<Task<()>>,
    /// 时间轴工具栏的缩放滑块（0–100，对数均匀）。
    pub(crate) zoom_slider: Entity<gpui_kit::component::slider::SliderState>,
    /// 定格图片存放的目录。
    pub(crate) freeze_dir: std::path::PathBuf,
    /// 播放入出点区域时，播到这里停（Shift+Enter）。
    pub(crate) play_until: Option<Time>,
    /// 倒放文件的目录。
    pub(crate) reverse_dir: std::path::PathBuf,
    /// 正在后台生成倒放文件。
    pub(crate) reversing: bool,
    /// 防抖文件放哪（[`crate::stabilize_ui`]）。
    pub(crate) stabilize_dir: std::path::PathBuf,
    /// 正在做视频防抖。
    pub(crate) stabilizing: bool,
    /// 换过几次文档（新建、打开、导入）：后台任务做完时据此判断工程是不是已经换了。
    pub(crate) doc_epoch: u64,
    /// 正在录音：录音器和从时间轴哪里开始录的。
    pub(crate) recording: Option<(ve_engine::Recorder, Time)>,
    /// 录音文件的目录。
    pub(crate) record_dir: std::path::PathBuf,
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
    #[expect(
        clippy::too_many_lines,
        reason = "按字段逐个列出初始值，拆开反而找不到某个字段的默认值"
    )]
    pub fn new(engine: Engine, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        // 默认原画质（剪映播放器底栏的「原画」）
        engine.set_preview_max(crate::preview::Quality::default().target());
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
        let (inspector, mut subscriptions) = Inspector::new(window, cx);
        let zoom_slider = cx.new(|_| {
            gpui_kit::component::slider::SliderState::new()
                .min(0.0)
                .max(100.0)
                .step(0.5)
                .default_value(crate::timeline::geometry::zoom_level(
                    crate::timeline::geometry::TimelineGeometry::default().px_per_second,
                ))
        });
        subscriptions.push(cx.subscribe_in(
            &zoom_slider,
            window,
            |this: &mut Self, _, e: &gpui_kit::component::slider::SliderEvent, _, cx| {
                if let gpui_kit::component::slider::SliderEvent::Change(v) = e {
                    let want = crate::timeline::geometry::px_for_zoom_level(v.start());
                    let now = this.timeline.geometry.px_per_second;
                    this.zoom(want / now, cx);
                }
            },
        ));
        // 改快捷键：在快捷键匹配之前截住按下的键
        let weak = cx.entity().downgrade();
        subscriptions.push(cx.intercept_keystrokes(move |e, _, cx| {
            let Some(ws) = weak.upgrade() else {
                return;
            };
            if ws.read(cx).recording_key.is_none() || e.keystroke.is_modifier_key() {
                return;
            }
            cx.stop_propagation();
            let key = e.keystroke.unparse(cx.platform_info().os);
            ws.update(cx, |w, cx| w.finish_recording(&key, cx));
        }));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let doc = Document::new();
        let project = Arc::new(doc.project.clone());
        Self {
            engine_project: Arc::clone(&project),
            project,
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
            popup: None,
            popup_dismissed: None,
            bin_tab: crate::media_bin::BinTab::default(),
            autosave: None,
            proxies: crate::proxy::Proxies::new(),
            scopes: crate::scopes::ScopeState::default(),
            plugin_dir: crate::plugins::default_dir(),
            plugins: Vec::new(),
            recovery: None,
            clipboard: crate::clip_ops::ClipBoard::default(),
            layout: crate::layout::LayoutMode::default(),
            missing: std::collections::BTreeSet::new(),
            recording_key: None,
            shortcut_note: None,
            transition_secs: 1.0,
            title: String::new(),
            tasks: vec![listen],
            zoom_slider,
            freeze_dir: crate::freeze::default_dir(),
            play_until: None,
            reverse_dir: crate::reverse::default_dir(),
            reversing: false,
            stabilize_dir: crate::stabilize_ui::default_dir(),
            stabilizing: false,
            doc_epoch: 0,
            recording: None,
            record_dir: crate::record::default_dir(),
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
            // 复合片段摊开再给播放引擎（时间轴上还是一段）
            let flat = ve_core::compound::flatten(&self.project);
            self.engine_project = if self.proxies.on {
                // 代理模式：有副本的素材换成副本
                let mut p = flat.unwrap_or_else(|| (*self.project).clone());
                self.with_proxies(&mut p);
                Arc::new(p)
            } else {
                flat.map_or_else(|| Arc::clone(&self.project), Arc::new)
            };
            self.engine.set_project(Arc::clone(&self.engine_project));
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
        self.doc_epoch += 1;
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

    /// 片段 `clip` 现在用的素材（工程编号 + 内容 + 长度 + 变速 + 倒放 / 防抖记录）。后台按片段生成
    /// 文件（倒放、防抖）前记一份，做完再比：变了（换了工程、片段被裁剪、撤销……）就不套用。
    pub(crate) fn source_key(&self, clip: ve_core::ClipId) -> Option<SourceKey> {
        let (_, c) = self.doc.project.timeline.find_clip(clip)?;
        Some(SourceKey {
            epoch: self.doc_epoch,
            content: c.content.clone(),
            duration: c.duration,
            speed: c.speed.to_bits(),
            reversed: c.reversed,
            stabilized: c.stabilized,
        })
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
        self.refresh_missing();
        self.sync_session();
        self.sync_engine();
        self.sync_inspector(window, cx);
        cx.notify();
    }

    pub(crate) fn seek(&mut self, t: Time, cx: &mut Context<'_, Self>) {
        self.timeline.axis_anchor = None;
        self.engine.seek(t);
        self.playhead = self.engine.position();
        cx.notify();
    }

    // ---- 播放控制 ----

    fn play_pause(&mut self, _: &act::PlayPause, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.end_preview_axis();
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
    pub(crate) fn playhead_frame(&self) -> Time {
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

    /// Esc：开着弹出框（菜单、布局、快捷键）时先关掉它；正在拖动（时间轴或预览）时取消这次拖动、
    /// 回到按下前，不记撤销；正在框选时回到框之前的选择；否则取消选择。
    fn cancel(&mut self, _: &act::Cancel, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.popup.take().is_some() {
            cx.notify();
            return;
        }
        // 全屏时 Esc 先退出全屏
        if self.preview.fullscreen {
            self.set_fullscreen(false, window, cx);
            return;
        }
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

    fn toggle_fullscreen(
        &mut self,
        _: &act::ToggleFullscreen,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.set_fullscreen(!self.preview.fullscreen, window, cx);
    }

    /// 进入 / 退出全屏预览：窗口跟着进入 / 退出系统全屏。
    pub(crate) fn set_fullscreen(&mut self, on: bool, window: &Window, cx: &mut Context<'_, Self>) {
        if self.preview.fullscreen == on {
            return;
        }
        self.preview.fullscreen = on;
        // 拖到一半进全屏：这次拖动作废
        self.preview.drag = None;
        if window.is_fullscreen() != on {
            window.toggle_fullscreen();
        }
        cx.notify();
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

    /// Shift+Z：整条时间线刚好放进视图，滚回开头。
    fn zoom_to_fit(&mut self, _: &act::ZoomToFit, _: &mut Window, cx: &mut Context<'_, Self>) {
        let geo = &mut self.timeline.geometry;
        let lane = f32::from(self.timeline.bounds.size.width) - geo.header_width;
        if let Some(px) =
            crate::timeline::geometry::fit_px_per_second(self.doc.project.timeline.duration(), lane)
        {
            geo.px_per_second = px;
            geo.scroll_x = 0.0;
            self.clamp_timeline_scroll();
            cx.notify();
        }
    }

    /// 缩放滑块跟着别处的缩放（Ctrl+滚轮、按钮、Shift+Z）走。
    fn sync_zoom_slider(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let level = crate::timeline::geometry::zoom_level(self.timeline.geometry.px_per_second);
        let shown = self.zoom_slider.read(cx).value().start();
        if (shown - level).abs() > 0.25 {
            self.zoom_slider
                .update(cx, |s, cx| s.set_value(level, window, cx));
        }
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
            .on_action(cx.listener(Self::toggle_fullscreen))
            .on_action(cx.listener(Self::mirror))
            .on_action(cx.listener(Self::rotate_90))
            .on_action(cx.listener(Self::zoom_to_fit))
            .on_action(cx.listener(Self::freeze))
            .on_action(cx.listener(Self::set_in))
            .on_action(cx.listener(Self::set_out))
            .on_action(cx.listener(Self::range_from_selection))
            .on_action(cx.listener(Self::clear_range))
            .on_action(cx.listener(Self::play_range))
            .on_action(cx.listener(Self::select_same_color))
            .on_action(cx.listener(Self::replace_clip))
            .on_action(cx.listener(Self::make_compound))
            .on_action(cx.listener(Self::export_otio))
            .on_action(cx.listener(Self::export_edl))
            .on_action(cx.listener(Self::import_otio))
            .on_action(cx.listener(Self::slip_tool))
            .on_action(cx.listener(Self::slide_tool))
            .on_action(cx.listener(Self::roll_tool))
            .on_action(cx.listener(Self::break_compound))
            .on_action(cx.listener(Self::select_tool))
            .on_action(cx.listener(Self::blade_tool))
            .on_action(cx.listener(Self::toggle_preview_axis))
            .on_action(cx.listener(Self::reverse))
            .on_action(cx.listener(Self::toggle_record))
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.sync_engine();
        self.pull_frame(window, cx);
        self.measure_subtitle_font(window);
        self.stop_at_range_end(cx);
        // 预览轴在预览别处时，播放头还是原来的位置
        self.playhead = self
            .timeline
            .axis_anchor
            .unwrap_or_else(|| self.engine.position());
        self.follow_playhead_in_inspector(window, cx);
        self.refresh_animated_sliders(false, window, cx);
        self.refresh_look_sliders(window, cx);
        self.refresh_curve_sliders(window, cx);
        self.refresh_grade_sliders(window, cx);
        self.refresh_effect_slider(window, cx);
        self.refresh_audio_fx_sliders(window, cx);
        self.sync_zoom_slider(window, cx);
        // 播放中、录音中一直刷新
        if self.engine.is_playing() || self.recording.is_some() {
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
            .when(self.preview.fullscreen, |root| {
                root.bg(palette::preview_bg())
                    .child(self.render_preview_frame(cx))
            })
            .when(!self.preview.fullscreen, |root| {
                root.children(self.render_context_menu(window, cx))
                    .child(self.render_topbar(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .px(px(chrome::CARD_GAP))
                            .child(self.render_panels(window, cx)),
                    )
            })
    }
}

/// 弹出提示。
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "调用的地方都是拿着 &mut Window 的界面代码；改成 & 会让一串函数跟着改签名"
)]
pub(crate) fn notify(note: Notification, window: &mut Window, cx: &mut App) {
    // 延到这一轮更新结束再弹：启动时（命令行带工程、崩溃恢复）主窗口还在搭建，组件库的根视图
    // 还没有，直接弹会 panic
    window.defer(cx, move |window, cx| window.push_notification(note, cx));
}

/// 打开主窗口时用：在 `cx.new` 里构造。
pub fn build(engine: Engine, window: &mut Window, cx: &mut App) -> Entity<Workspace> {
    // 旋转光标要直接调系统（GPUI 没有自定义光标）；装不上（无界面测试、非 Windows）就退回手指光标
    crate::platform_cursor::install(window);
    cx.new(|cx| Workspace::new(engine, window, cx))
}

/// 见 [`Workspace::source_key`]。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SourceKey {
    epoch: u64,
    content: ve_core::ClipContent,
    duration: Time,
    speed: u32,
    reversed: Option<(AssetId, Time)>,
    stabilized: Option<(AssetId, Time, u8)>,
}
