//! 无界面窗口的 UI 集成测试：真实窗口、真实按键 / 鼠标派发、真实播放引擎（静音输出）。

#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "测试辅助函数：失败就该让测试失败"
)]

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Pixels, Point, ScrollDelta, TestAppContext,
    WindowBounds, WindowOptions, point, px, size,
};
use std::path::PathBuf;
use std::time::Duration;
use ve_app::workspace::Workspace;
use ve_core::{Time, TrackKind};
use ve_engine::{AudioBackend, Engine, EngineConfig};
use ve_media::testing::{Fixture, fixture};

struct Ui {
    win: AnyWindowHandle,
    view: Entity<Workspace>,
}

fn open(cx: &mut TestAppContext) -> Ui {
    cx.update(ve_app::app::init);
    let (win, view) = cx.update(|cx| {
        let engine = Engine::new(EngineConfig {
            audio: AudioBackend::Null,
            ..EngineConfig::default()
        })
        .unwrap();
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: size(px(1400.), px(900.)),
            })),
            ..WindowOptions::default()
        };
        gpui_kit::open_window(opts, cx, |w, cx| ve_app::workspace::build(engine, w, cx)).unwrap()
    });
    let ui = Ui { win, view };
    frame(cx, &ui);
    ui
}

fn frame(cx: &mut TestAppContext, ui: &Ui) {
    cx.update_window(ui.win, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

fn press(cx: &mut TestAppContext, ui: &Ui, key: &str) {
    cx.update_window(ui.win, |_, window, cx| window.press(key, cx))
        .unwrap();
    cx.run_until_parked();
}

/// 反复推进前台任务直到条件成立（后台有真实线程：探测、解码、导出）。
fn wait(cx: &mut TestAppContext, ui: &Ui, mut done: impl FnMut(&Workspace) -> bool) {
    for _ in 0..400 {
        // 测试调度器的定时器只在推进虚拟时钟时触发（界面用定时轮询引擎事件 / 导出进度）
        cx.executor().advance_clock(Duration::from_millis(20));
        cx.run_until_parked();
        if ui.view.read_with(cx, |w, _| done(w)) {
            frame(cx, ui);
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("等待超时");
}

fn import(cx: &mut TestAppContext, ui: &Ui, files: Vec<PathBuf>, append: bool) {
    let want = ui
        .view
        .read_with(cx, |w, _| w.document().project.assets.len())
        + files.len();
    cx.update_window(ui.win, |_, window, cx| {
        ui.view
            .update(cx, |w, cx| w.import_with(files, append, window, cx));
    })
    .unwrap();
    wait(cx, ui, |w| w.document().project.assets.len() >= want);
}

fn clips(cx: &TestAppContext, ui: &Ui, kind: TrackKind) -> usize {
    ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .track_of_kind(kind)
            .map_or(0, |t| t.clips.len())
    })
}

/// 时间轴里某轨某时刻的局部坐标（默认几何：轨道头 72、标尺 28、轨高 56、50 像素 / 秒）。
fn timeline_point(secs: f32, track: f32) -> Point<Pixels> {
    point(px(72.0 + secs * 50.0), px(28.0 + 56.0 * track + 28.0))
}

#[gpui_kit::test]
fn window_opens_with_all_panels(cx: &mut TestAppContext) {
    let ui = open(cx);
    cx.update_window(ui.win, |_, window, _| {
        for id in [
            "toolbar",
            "media-bin",
            "preview",
            "transport",
            "inspector",
            "timeline",
        ] {
            assert!(window.try_find(id).is_some(), "缺少面板 {id}");
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn import_paths_adds_assets_and_rejects_unsupported(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let txt = dir.path().join("readme.txt");
    std::fs::write(&txt, "x").unwrap();
    let files = vec![fixture(Fixture::VideoWithAudio), txt];
    cx.update_window(ui.win, |_, window, cx| {
        ui.view
            .update(cx, |w, cx| w.import_paths(files, window, cx));
    })
    .unwrap();
    wait(&mut *cx, &ui, |w| w.document().project.assets.len() == 1);
    // 第一个视频决定工程尺寸
    let size = ui.view.read_with(cx, |w, _| {
        (
            w.document().project.settings.width,
            w.document().project.settings.height,
        )
    });
    assert_eq!(size, (64, 36));
    assert_eq!(clips(cx, &ui, TrackKind::Video), 0);
}

#[gpui_kit::test]
fn double_click_appends_to_timeline(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], false);
    let id = ui.view.read_with(cx, |w, _| {
        *w.document().project.assets.keys().next().unwrap()
    });
    cx.update_window(ui.win, |_, window, cx| {
        window.double_click(format!("asset-{}", id.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(clips(cx, &ui, TrackKind::Video), 1);
    cx.update_window(ui.win, |_, window, cx| {
        window.click(format!("add-{}", id.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(clips(cx, &ui, TrackKind::Video), 2);
}

#[gpui_kit::test]
fn undo_redo_via_keybindings(cx: &mut TestAppContext) {
    let ui = open(cx);
    press(cx, &ui, "t");
    assert_eq!(clips(cx, &ui, TrackKind::Subtitle), 1);
    press(cx, &ui, "ctrl-z");
    assert_eq!(clips(cx, &ui, TrackKind::Subtitle), 0);
    press(cx, &ui, "ctrl-y");
    assert_eq!(clips(cx, &ui, TrackKind::Subtitle), 1);
    cx.update_window(ui.win, |_, window, cx| window.click("tb-undo", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(clips(cx, &ui, TrackKind::Subtitle), 0);
}

#[gpui_kit::test]
fn play_button_and_space_toggle_engine(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    cx.update_window(ui.win, |_, window, cx| window.click("tr-play", cx))
        .unwrap();
    cx.run_until_parked();
    assert!(ui.view.read_with(cx, |w, _| w.engine().is_playing()));
    press(cx, &ui, "space");
    assert!(!ui.view.read_with(cx, |w, _| w.engine().is_playing()));
    // 逐帧：右键前进一帧
    press(cx, &ui, "home");
    press(cx, &ui, "right");
    let t = ui.view.read_with(cx, |w, _| w.engine().position());
    assert_eq!(t, ve_core::FrameRate::new(30, 1).time_of_frame(1));
    press(cx, &ui, "end");
    press(cx, &ui, "left");
    press(cx, &ui, "j");
    let back = ui.view.read_with(cx, |w, _| w.engine().position());
    assert!(back < Time::from_secs(1), "{back:?}");
}

#[gpui_kit::test]
fn split_key_splits_at_playhead_and_delete_removes_selection(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    ui.view
        .read_with(cx, |w, _| w.engine().seek(Time::from_secs(1)));
    frame(cx, &ui);
    press(cx, &ui, "s");
    assert_eq!(clips(cx, &ui, TrackKind::Video), 2);
    // 单击时间轴上第一段（0–1 秒）选中，再按 Delete
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 0.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.document().selection().len()),
        1
    );
    press(cx, &ui, "delete");
    assert_eq!(clips(cx, &ui, TrackKind::Video), 1);
    press(cx, &ui, "ctrl-z");
    assert_eq!(clips(cx, &ui, TrackKind::Video), 2);
}

#[gpui_kit::test]
fn drag_clip_on_timeline_moves_it(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    let from = origin + timeline_point(1.0, 0.0);
    let to = origin + timeline_point(5.0, 0.0);
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let start = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .track_of_kind(TrackKind::Video)
            .unwrap()
            .clips[0]
            .start
    });
    assert!(
        (start - Time::from_secs(4)).0.abs() <= Time::from_millis(40).0,
        "{start:?}"
    );
    // 一次拖拽一步撤销
    press(cx, &ui, "ctrl-z");
    let back = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .track_of_kind(TrackKind::Video)
            .unwrap()
            .clips[0]
            .start
    });
    assert_eq!(back, Time::ZERO);
}

#[gpui_kit::test]
fn ruler_click_seeks_and_wheel_zooms(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", point(px(72.0 + 50.0), px(10.0)), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let t = ui.view.read_with(cx, |w, _| w.engine().position());
    assert_eq!(t, Time::from_secs(1));
    cx.update_window(ui.win, |_, window, cx| {
        window.scroll(
            "timeline",
            ScrollDelta::Pixels(point(px(0.), px(-120.))),
            cx,
        );
    })
    .unwrap();
    press(cx, &ui, "ctrl-=");
    press(cx, &ui, "ctrl--");
}

#[gpui_kit::test]
fn open_project_with_missing_media_still_opens(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("clip.mp4");
    std::fs::copy(fixture(Fixture::FrameIndexed30), &media).unwrap();
    import(cx, &ui, vec![media], true);
    let file = dir.path().join("p.vep");
    // 存盘时把素材路径改成不存在的位置，模拟素材被删 / 移走（Windows 下正被解码的文件删不掉）
    ui.view.update(cx, |w, _| {
        let mut doc = w.document().clone();
        for a in doc.project.assets.values_mut() {
            a.path = dir.path().join("已删除").join("clip.mp4");
        }
        ve_app::project_io::save(&mut doc, &file).unwrap();
    });
    cx.update_window(ui.win, |_, window, cx| {
        ui.view.update(cx, |w, cx| w.open_path(&file, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(clips(cx, &ui, TrackKind::Video), 1);
    assert_eq!(
        ui.view
            .read_with(cx, |w, _| w.document().path().map(PathBuf::from)),
        Some(file)
    );
}

#[gpui_kit::test]
fn export_runs_and_reports_done(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(
        cx,
        &ui,
        vec![
            fixture(Fixture::FrameIndexed30),
            fixture(Fixture::ToneStereo48k),
        ],
        true,
    );
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    let settings = ve_export::ExportSettings {
        path: out.clone(),
        width: 64,
        height: 36,
        frame_rate: ve_core::FrameRate::new(30, 1),
        video_bitrate_kbps: 500,
        encoder: ve_export::Encoder::X264,
        render: ve_export::RenderConfig { software: true },
    };
    cx.update_window(ui.win, |_, window, cx| {
        ui.view
            .update(cx, |w, cx| w.start_export(settings, window, cx));
    })
    .unwrap();
    assert!(ui.view.read_with(cx, |w, _| w.is_exporting()));
    frame(cx, &ui);
    wait(&mut *cx, &ui, |w| !w.is_exporting());
    assert!(out.exists());
    assert!(ve_media::probe(&out).unwrap().audio.is_some());
    cx.update_window(ui.win, |_, window, _| {
        assert!(window.try_find("export-reveal").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn late_analysis_for_replaced_asset_is_dropped(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], false);
    let id = ui.view.read_with(cx, |w, _| {
        *w.document().project.assets.keys().next().unwrap()
    });
    wait(&mut *cx, &ui, |w| w.media_path(id).is_some());
    // 撤销导入后再导入另一个文件：编号相同、路径不同；旧文件的分析结果晚到时不能写进来
    press(cx, &ui, "ctrl-z");
    import(cx, &ui, vec![fixture(Fixture::Image)], false);
    let reused = ui.view.read_with(cx, |w, _| {
        *w.document().project.assets.keys().next().unwrap()
    });
    assert_eq!(reused, id);
    // 导入时路径被规范化成绝对路径（去掉 ..）
    let image = std::path::absolute(fixture(Fixture::Image)).unwrap();
    wait(&mut *cx, &ui, |w| w.media_path(id) == Some(image.as_path()));
    let stale = std::path::absolute(fixture(Fixture::FrameIndexed30)).unwrap();
    let accepted = cx
        .update_window(ui.win, |_, window, cx| {
            ui.view.update(cx, |w, cx| {
                w.accept_analysis(id, &stale, ve_app::import::Analysis::default(), window, cx)
            })
        })
        .unwrap();
    assert!(!accepted);
    assert_eq!(
        ui.view
            .read_with(cx, |w, _| w.media_path(id).map(PathBuf::from)),
        Some(image)
    );
}

#[gpui_kit::test]
fn mouse_released_outside_window_ends_drag_gesture(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    let mut vcx = gpui_kit::VisualTestContext::from_window(ui.win, cx);
    let none = gpui_kit::Modifiers::default();
    vcx.simulate_mouse_down(
        origin + timeline_point(1.0, 0.0),
        gpui_kit::MouseButton::Left,
        none,
    );
    vcx.simulate_mouse_move(
        origin + timeline_point(3.0, 0.0),
        Some(gpui_kit::MouseButton::Left),
        none,
    );
    // 在窗口外松开：收不到 mouse up，下一次移动时左键已经没按着
    vcx.simulate_mouse_move(origin + timeline_point(6.0, 0.0), None, none);
    cx.run_until_parked();
    let (in_gesture, start) = ui.view.read_with(cx, |w, _| {
        let d = w.document();
        (
            d.in_gesture(),
            d.project
                .timeline
                .track_of_kind(TrackKind::Video)
                .unwrap()
                .clips[0]
                .start,
        )
    });
    assert!(!in_gesture, "手势没有结束");
    assert!(
        (start - Time::from_secs(2)).0.abs() <= Time::from_millis(40).0,
        "{start:?}"
    );
    press(cx, &ui, "ctrl-z");
    let back = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .track_of_kind(TrackKind::Video)
            .unwrap()
            .clips[0]
            .start
    });
    assert_eq!(back, Time::ZERO);
}

#[gpui_kit::test]
fn subtitle_edit_survives_clicking_elsewhere(cx: &mut TestAppContext) {
    let ui = open(cx);
    press(cx, &ui, "t"); // 加一条字幕并选中，属性面板显示文字框
    frame(cx, &ui);
    cx.update_window(ui.win, |_, window, cx| {
        window.click("subtitle-text", cx);
        window.press("ctrl-a", cx);
        window.input("改过的字幕", cx);
        // 不按 Ctrl+Enter，直接点时间轴空白处
        window.click_at("timeline", timeline_point(20.0, 2.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let text = ui.view.read_with(cx, |w, _| {
        let t = w
            .document()
            .project
            .timeline
            .track_of_kind(TrackKind::Subtitle)
            .unwrap();
        t.clips[0].subtitle_text().map(str::to_owned)
    });
    assert_eq!(text.as_deref(), Some("改过的字幕"));
}

#[gpui_kit::test]
fn save_from_unsaved_prompt_then_continues_new_project(cx: &mut TestAppContext) {
    let ui = open(cx);
    press(cx, &ui, "t"); // 有未保存的修改，且工程还没有路径
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("先存一下.vep");
    press(cx, &ui, "ctrl-n");
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("保存");
    cx.run_until_parked();
    // 没有路径 → 弹另存为；选好路径后应保存并接着新建
    let chosen = target.clone();
    cx.simulate_new_path_selection(move |_| Some(chosen));
    cx.run_until_parked();
    assert!(target.exists(), "工程没有保存");
    let (subs, dirty, path) = ui.view.read_with(cx, |w, _| {
        let d = w.document();
        (
            d.project
                .timeline
                .track_of_kind(TrackKind::Subtitle)
                .unwrap()
                .clips
                .len(),
            d.is_dirty(),
            d.path().map(PathBuf::from),
        )
    });
    assert_eq!(
        (subs, dirty, path),
        (0, false, None),
        "保存后应接着新建空工程"
    );
}

/// 字幕文字底边相对画面底边的位置（应 = 底边距，与行数无关）。
fn subtitle_bottom_gap(cx: &mut TestAppContext, ui: &Ui) -> f32 {
    frame(cx, ui);
    cx.update_window(ui.win, |_, window, _| {
        let area = window.find("preview").bounds();
        let video = ve_app::preview::fit_rect(area, 1920, 1080);
        let text = window.find("subtitle-main").bounds();
        f32::from(video.origin.y + video.size.height) - f32::from(text.origin.y + text.size.height)
    })
    .unwrap()
}

#[gpui_kit::test]
fn multi_line_subtitle_grows_upward_like_export(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let srt = dir.path().join("a.srt");
    std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:02,000\n第一行\n\n2\n00:00:03,000 --> 00:00:05,000\n第一行\n第二行\n").unwrap();
    cx.update_window(ui.win, |_, window, cx| {
        ui.view
            .update(cx, |w, cx| w.import_srt_path(&srt, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let one = subtitle_bottom_gap(cx, &ui);
    ui.view
        .read_with(cx, |w, _| w.engine().seek(Time::from_secs(4)));
    let two = subtitle_bottom_gap(cx, &ui);
    assert!(
        (one - two).abs() <= 2.0,
        "一行时离底 {one}px，两行时 {two}px：多行字幕应向上长"
    );
    assert!(one > 0.0, "{one}");
}

/// Ctrl+E → 选输出文件 → 弹出导出选项对话框。
fn open_export_dialog(cx: &mut TestAppContext, ui: &Ui, out: &std::path::Path) {
    import(cx, ui, vec![fixture(Fixture::FrameIndexed30)], true);
    press(cx, ui, "ctrl-e");
    let chosen = out.to_path_buf();
    cx.simulate_new_path_selection(move |_| Some(chosen));
    cx.run_until_parked();
    frame(cx, ui);
}

#[gpui_kit::test]
fn export_dialog_has_confirm_button_that_starts_export(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("对话框导出.mp4");
    open_export_dialog(cx, &ui, &out);
    cx.update_window(ui.win, |_, window, cx| {
        assert!(
            window.try_find("export-cancel-dialog").is_some(),
            "缺少「取消」按钮"
        );
        window.click("export-confirm", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        ui.view.read_with(cx, |w, _| w.is_exporting()),
        "点「开始导出」后应开始导出"
    );
    wait(&mut *cx, &ui, |w| !w.is_exporting());
    assert!(out.exists());
}

#[gpui_kit::test]
fn export_dialog_cancel_closes_without_exporting(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("不导出.mp4");
    open_export_dialog(cx, &ui, &out);
    cx.update_window(ui.win, |_, window, cx| {
        window.click("export-cancel-dialog", cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    assert!(!ui.view.read_with(cx, |w, _| w.is_exporting()));
    cx.update_window(ui.win, |_, window, _| {
        assert!(window.try_find("export-confirm").is_none(), "对话框没关");
    })
    .unwrap();
    assert!(!out.exists());
}

/// 存一个只有一条字幕的工程，返回路径。
fn write_project(dir: &std::path::Path, name: &str) -> PathBuf {
    let mut p = ve_core::Project::new();
    ve_core::edit::add_subtitle(&mut p, Time::ZERO, Time::from_secs(1), name.into()).unwrap();
    let file = dir.join(format!("{name}.vep"));
    ve_core::save_project(&p, &file).unwrap();
    file
}

/// 打开窗口，最近列表改用临时文件（不碰用户真实的列表）。
fn open_with_recent(cx: &mut TestAppContext, recent: &std::path::Path) -> Ui {
    let ui = open(cx);
    let file = recent.to_path_buf();
    ui.view.update(cx, |w, _| w.use_recent_file(file));
    ui
}

#[gpui_kit::test]
fn opening_and_saving_record_recent_projects(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("recent.json");
    let ui = open_with_recent(cx, &store);
    let a = write_project(dir.path(), "甲");
    cx.update_window(ui.win, |_, window, cx| {
        ui.view.update(cx, |w, cx| w.open_path(&a, window, cx));
    })
    .unwrap();
    press(cx, &ui, "t");
    let b = dir.path().join("乙.vep");
    press(cx, &ui, "ctrl-shift-s");
    let chosen = b.clone();
    cx.simulate_new_path_selection(move |_| Some(chosen));
    cx.run_until_parked();
    let recent = ui.view.read_with(cx, |w, _| w.recent().paths().to_vec());
    assert_eq!(recent, vec![b, a]);
    // 存到了文件里，下次启动还在
    assert_eq!(
        ve_app::recent::RecentList::load(&store).paths(),
        recent.as_slice()
    );
}

#[gpui_kit::test]
fn recent_menu_opens_chosen_project(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let ui = open_with_recent(cx, &dir.path().join("recent.json"));
    let a = write_project(dir.path(), "甲");
    let b = write_project(dir.path(), "乙");
    for f in [&a, &b] {
        cx.update_window(ui.win, |_, window, cx| {
            ui.view.update(cx, |w, cx| w.open_path(f, window, cx));
        })
        .unwrap();
    }
    cx.run_until_parked();
    cx.update_window(ui.win, |_, window, cx| {
        window.click("tb-recent", cx);
        // 列表里最近的是「乙」，第二个是「甲」
        window.click("recent-1", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        ui.view
            .read_with(cx, |w, _| w.document().path().map(PathBuf::from)),
        Some(a.clone())
    );
    let first = ui.view.read_with(cx, |w, _| w.recent().paths()[0].clone());
    assert_eq!(first, a);
}

#[gpui_kit::test]
fn recent_entry_for_deleted_file_is_removed(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let ui = open_with_recent(cx, &dir.path().join("recent.json"));
    let a = write_project(dir.path(), "甲");
    cx.update_window(ui.win, |_, window, cx| {
        ui.view.update(cx, |w, cx| w.open_path(&a, window, cx));
    })
    .unwrap();
    std::fs::remove_file(&a).unwrap();
    cx.update_window(ui.win, |_, window, cx| {
        window.click("tb-recent", cx);
        window.click("recent-0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(ui.view.read_with(cx, |w, _| w.recent().paths().len()), 0);
}

fn track_count(cx: &TestAppContext, ui: &Ui, kind: TrackKind) -> usize {
    ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == kind)
            .count()
    })
}

fn click(cx: &mut TestAppContext, ui: &Ui, id: &'static str) {
    cx.update_window(ui.win, |_, window, cx| window.click(id, cx))
        .unwrap();
    cx.run_until_parked();
    frame(cx, ui);
}

#[gpui_kit::test]
fn add_and_remove_tracks(cx: &mut TestAppContext) {
    let ui = open(cx);
    click(cx, &ui, "add-video-track");
    assert_eq!(track_count(cx, &ui, TrackKind::Video), 2);
    click(cx, &ui, "add-audio-track");
    assert_eq!(track_count(cx, &ui, TrackKind::Audio), 2);
    // 每次加轨都是一步撤销
    press(cx, &ui, "ctrl-z");
    assert_eq!(track_count(cx, &ui, TrackKind::Audio), 1);
    press(cx, &ui, "ctrl-z");
    assert_eq!(track_count(cx, &ui, TrackKind::Video), 1);
    press(cx, &ui, "ctrl-y");
    assert_eq!(track_count(cx, &ui, TrackKind::Video), 2);
    frame(cx, &ui);
    // 新视频轨在最上面（第 0 行），空轨直接删，不用确认
    click(cx, &ui, "track-del-0");
    assert_eq!(track_count(cx, &ui, TrackKind::Video), 1);
    // 只剩一条视频轨：没有删除按钮
    cx.update_window(ui.win, |_, window, _| {
        assert!(window.try_find("track-del-0").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn deleting_track_with_clips_asks_first(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click(cx, &ui, "add-video-track");
    // 第 0 行是新的空视频轨，第 1 行是放了片段的原视频轨
    click(cx, &ui, "track-del-1");
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    assert_eq!(track_count(cx, &ui, TrackKind::Video), 2);
    click(cx, &ui, "track-del-1");
    cx.simulate_prompt_answer("删除");
    cx.run_until_parked();
    assert_eq!(track_count(cx, &ui, TrackKind::Video), 1);
    assert_eq!(clips(cx, &ui, TrackKind::Video), 0, "片段随轨道一起删了");
    // 能撤销回来
    press(cx, &ui, "ctrl-z");
    assert_eq!(track_count(cx, &ui, TrackKind::Video), 2);
}

#[gpui_kit::test]
fn clip_drags_between_video_tracks(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click(cx, &ui, "add-video-track");
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    // 片段在第 1 行（V1），拖到第 0 行（V2）
    let from = origin + timeline_point(1.0, 1.0);
    let to = origin + timeline_point(1.0, 0.0);
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let top = ui.view.read_with(cx, |w, _| {
        w.document().project.timeline.tracks[0].clips.len()
    });
    assert_eq!(top, 1);
}

fn first_video_transform(cx: &TestAppContext, ui: &Ui) -> ve_core::Transform {
    ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .track_of_kind(TrackKind::Video)
            .unwrap()
            .clips[0]
            .transform
    })
}

fn type_into(cx: &mut TestAppContext, ui: &Ui, id: &'static str, text: &str) {
    cx.update_window(ui.win, |_, window, cx| {
        window.click(id, cx);
        window.press("ctrl-a", cx);
        window.input(text, cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, ui);
}

#[gpui_kit::test]
fn transform_fields_edit_clip(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(
        cx,
        &ui,
        vec![
            fixture(Fixture::FrameIndexed30),
            fixture(Fixture::ToneStereo48k),
        ],
        true,
    );
    // 音频片段没有「画面」区
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 1.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    cx.update_window(ui.win, |_, window, _| {
        assert!(window.try_find("tf-x").is_none());
    })
    .unwrap();
    // 选中视频片段
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 0.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    type_into(cx, &ui, "tf-x", "25");
    assert!((first_video_transform(cx, &ui).x - 0.25).abs() < 1e-6);
    type_into(cx, &ui, "tf-scale", "50");
    assert!((first_video_transform(cx, &ui).scale - 0.5).abs() < 1e-6);
    // 超出范围的缩放夹到 1000%
    type_into(cx, &ui, "tf-scale", "5000");
    assert!((first_video_transform(cx, &ui).scale - 10.0).abs() < 1e-6);
    // 左右裁剪加起来到 100% 以上：这次修改不生效
    type_into(cx, &ui, "tf-crop-l", "30");
    type_into(cx, &ui, "tf-crop-r", "80");
    let t = first_video_transform(cx, &ui);
    assert!(
        (t.crop.left - 0.3).abs() < 1e-6 && t.crop.right == 0.0,
        "{t:?}"
    );
    // 重置
    cx.update_window(ui.win, |_, window, cx| window.click("tf-reset", cx))
        .unwrap();
    cx.run_until_parked();
    assert!(first_video_transform(cx, &ui).is_default());
    // 光标还在输入框里时 Ctrl+Z 是输入框自己的撤销，这里用工具栏的撤销按钮
    click(cx, &ui, "tb-undo");
    assert!((first_video_transform(cx, &ui).crop.left - 0.3).abs() < 1e-6);
}

#[gpui_kit::test]
fn opacity_slider_drag_is_one_undo_step(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 0.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let bounds = cx
        .update_window(ui.win, |_, window, _| window.find("tf-opacity").bounds())
        .unwrap();
    let y = bounds.origin.y + bounds.size.height / 2.0;
    let from = point(bounds.origin.x + bounds.size.width - px(2.0), y);
    let to = point(bounds.origin.x + bounds.size.width / 2.0, y);
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let opacity = first_video_transform(cx, &ui).opacity;
    assert!(opacity < 0.8 && opacity > 0.2, "{opacity}");
    press(cx, &ui, "ctrl-z");
    assert_eq!(first_video_transform(cx, &ui).opacity, 1.0);
}

/// 预览里画面的中心（窗口坐标）。
fn preview_center(cx: &mut TestAppContext, ui: &Ui) -> Point<Pixels> {
    let b = cx
        .update_window(ui.win, |_, window, _| window.find("preview").bounds())
        .unwrap();
    point(
        b.origin.x + b.size.width / 2.0,
        b.origin.y + b.size.height / 2.0,
    )
}

#[gpui_kit::test]
fn drag_in_preview_moves_selected_clip(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 0.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let from = preview_center(cx, &ui);
    let to = from + point(px(40.0), px(0.0));
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let t = first_video_transform(cx, &ui);
    assert!(t.x > 0.01, "{t:?}");
    assert_eq!(t.y, 0.0);
    click(cx, &ui, "tb-undo");
    assert!(
        first_video_transform(cx, &ui).is_default(),
        "一次拖动一步撤销"
    );
}

#[gpui_kit::test]
fn click_in_preview_selects_top_layer(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click(cx, &ui, "add-video-track");
    // 双击素材：接在主画面（V1，第 1 行）后面，2–4 秒；再把它拖到上层 V2（第 0 行）的 0 秒
    let id = ui.view.read_with(cx, |w, _| {
        *w.document().project.assets.keys().next().unwrap()
    });
    cx.update_window(ui.win, |_, window, cx| {
        window.double_click(format!("asset-{}", id.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    let (from, to) = (
        origin + timeline_point(3.0, 1.0),
        origin + timeline_point(1.0, 0.0),
    );
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let (pip, bottom) = ui.view.read_with(cx, |w, _| {
        let t = &w.document().project.timeline.tracks;
        (t[0].clips[0].id, t[1].clips[0].id)
    });
    // 选中上层片段，在属性面板里缩小一半（居中）
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 0.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    type_into(cx, &ui, "tf-scale", "50");
    // 点时间轴空白处取消选择
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(20.0, 2.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let size = cx
        .update_window(ui.win, |_, window, _| window.find("preview").bounds().size)
        .unwrap();
    // 预览中心落在上层画中画里
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("preview", point(size.width / 2.0, size.height / 2.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let selected = ui
        .view
        .read_with(cx, |w, _| w.document().selection().to_vec());
    assert_eq!(selected, [pip], "中心点在上层画中画里");
    // 画中画外面、下层画面里：选中下层
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at(
            "preview",
            point(size.width / 2.0, size.height / 2.0 - size.height * 0.3),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let second = ui
        .view
        .read_with(cx, |w, _| w.document().selection().to_vec());
    assert_eq!(second, [bottom], "点到下层画面");
    // 选中铺满画面的下层以后，再点上层画中画：照样选中上层（上层盖住的地方点的是上层）
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("preview", point(size.width / 2.0, size.height / 2.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let third = ui
        .view
        .read_with(cx, |w, _| w.document().selection().to_vec());
    assert_eq!(third, [pip], "下层选中时仍能点到上层");
}

#[gpui_kit::test]
fn many_tracks_scroll_vertically_over_headers(cx: &mut TestAppContext) {
    let ui = open(cx);
    for _ in 0..3 {
        click(cx, &ui, "add-video-track");
    }
    // 6 条轨（V4 V3 V2 V1 A1 S1），时间轴放不下：在轨道头上滚轮往下滚
    cx.update_window(ui.win, |_, window, _| {
        assert!(window.try_find("track-del-0").is_some());
    })
    .unwrap();
    cx.update_window(ui.win, |_, window, cx| {
        window.scroll(
            "track-headers",
            ScrollDelta::Pixels(point(px(0.), px(-120.))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    // 第 0 行滚到标尺下面去了：它的删除按钮不再显示
    cx.update_window(ui.win, |_, window, _| {
        assert!(window.try_find("track-del-0").is_none());
        assert!(window.try_find("track-del-3").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn switching_selection_after_drag_adds_no_hidden_edit(cx: &mut TestAppContext) {
    // 预览上拖出来的值精度比输入框显示的高；换选中时不能把四舍五入后的值再写回去
    let ui = open(cx);
    import(
        cx,
        &ui,
        vec![
            fixture(Fixture::FrameIndexed30),
            fixture(Fixture::ToneStereo48k),
        ],
        true,
    );
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 0.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let from = preview_center(cx, &ui);
    let to = from + point(px(37.3), px(11.7));
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let dragged = first_video_transform(cx, &ui);
    assert!(dragged.x != 0.0);
    // 点音频片段（换选中）
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 1.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        first_video_transform(cx, &ui),
        dragged,
        "换选中不应改动画面"
    );
    click(cx, &ui, "tb-undo");
    assert!(
        first_video_transform(cx, &ui).is_default(),
        "撤销一步就回到拖动之前"
    );
}

#[gpui_kit::test]
fn scroll_is_clamped_after_tracks_go_away(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    for _ in 0..3 {
        click(cx, &ui, "add-video-track");
    }
    cx.update_window(ui.win, |_, window, cx| {
        window.scroll(
            "track-headers",
            ScrollDelta::Pixels(point(px(0.), px(-200.))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    // 撤销三次加轨：剩 3 条轨，放得下，滚动位置要回到 0
    for _ in 0..3 {
        click(cx, &ui, "tb-undo");
    }
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 0.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let selected = ui.view.read_with(cx, |w, _| w.document().selection().len());
    assert_eq!(selected, 1, "第 0 行就是视频轨上的片段");
}

/// 只按下、移动，不松开（模拟拖到一半）。
fn press_and_move(cx: &mut TestAppContext, ui: &Ui, from: Point<Pixels>, to: Point<Pixels>) {
    press_and_move_with(cx, ui, from, to, gpui_kit::Modifiers::default());
}

/// 同上，按住修饰键。
fn press_and_move_with(
    cx: &mut TestAppContext,
    ui: &Ui,
    from: Point<Pixels>,
    to: Point<Pixels>,
    modifiers: gpui_kit::Modifiers,
) {
    use gpui_kit::{InputEvent as _, MouseButton, MouseDownEvent, MouseMoveEvent};
    cx.update_window(ui.win, |_, window, cx| {
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position: from,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(
            MouseMoveEvent {
                position: to,
                pressed_button: Some(MouseButton::Left),
                modifiers,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// 松开鼠标。
fn release(cx: &mut TestAppContext, ui: &Ui, at: Point<Pixels>) {
    use gpui_kit::{InputEvent as _, Modifiers, MouseButton, MouseUpEvent};
    cx.update_window(ui.win, |_, window, cx| {
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn first_video_start(cx: &TestAppContext, ui: &Ui) -> Time {
    ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .track_of_kind(TrackKind::Video)
            .unwrap()
            .clips[0]
            .start
    })
}

#[gpui_kit::test]
fn escape_clears_selection(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 0.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.document().selection().len()),
        1
    );
    press(cx, &ui, "escape");
    assert!(
        ui.view
            .read_with(cx, |w, _| w.document().selection().is_empty())
    );
}

#[gpui_kit::test]
fn escape_mid_preview_drag_restores_and_adds_no_undo_step(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 0.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let from = preview_center(cx, &ui);
    let to = from + point(px(40.0), px(0.0));
    press_and_move(cx, &ui, from, to);
    assert!(first_video_transform(cx, &ui).x > 0.01);
    press(cx, &ui, "escape");
    assert!(
        first_video_transform(cx, &ui).is_default(),
        "拖到一半按 Esc 回到原位"
    );
    // 拖动取消后：选择保留（第一下 Esc 只取消拖动），松开鼠标不再改动
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.document().selection().len()),
        1
    );
    release(cx, &ui, to);
    assert!(first_video_transform(cx, &ui).is_default());
    // 没有留下撤销步骤：撤销的是导入，不是拖动
    let before = clips(cx, &ui, TrackKind::Video);
    click(cx, &ui, "tb-undo");
    assert_ne!(clips(cx, &ui, TrackKind::Video), before);
}

#[gpui_kit::test]
fn escape_mid_timeline_drag_restores_clip(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    let from = origin + timeline_point(0.5, 0.0);
    let to = from + point(px(100.0), px(0.0));
    press_and_move(cx, &ui, from, to);
    assert!(first_video_start(cx, &ui) > Time::ZERO, "拖动生效了");
    press(cx, &ui, "escape");
    assert_eq!(
        first_video_start(cx, &ui),
        Time::ZERO,
        "拖到一半按 Esc 回到原位"
    );
    release(cx, &ui, to);
    assert_eq!(first_video_start(cx, &ui), Time::ZERO);
}

#[gpui_kit::test]
fn ctrl_a_selects_all_and_ctrl_shift_a_clears(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    ui.view
        .read_with(cx, |w, _| w.engine().seek(Time::from_secs(1)));
    frame(cx, &ui);
    press(cx, &ui, "s");
    let total: usize = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .tracks
            .iter()
            .map(|t| t.clips.len())
            .sum()
    });
    assert!(total >= 2);
    press(cx, &ui, "ctrl-a");
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.document().selection().len()),
        total
    );
    press(cx, &ui, "ctrl-shift-a");
    assert!(
        ui.view
            .read_with(cx, |w, _| w.document().selection().is_empty())
    );
}

/// 视频轨上的片段（按时间先后）。
fn video_clips(cx: &TestAppContext, ui: &Ui) -> Vec<(ve_core::ClipId, Time)> {
    ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .track_of_kind(TrackKind::Video)
            .unwrap()
            .clips
            .iter()
            .map(|c| (c.id, c.start))
            .collect()
    })
}

fn ids(mut v: Vec<ve_core::ClipId>) -> Vec<ve_core::ClipId> {
    v.sort_unstable();
    v
}

fn selected(cx: &TestAppContext, ui: &Ui) -> Vec<ve_core::ClipId> {
    let mut v = ui
        .view
        .read_with(cx, |w, _| w.document().selection().to_vec());
    v.sort_unstable();
    v
}

/// 在时间轴上点一下，可带修饰键。
fn click_timeline(cx: &mut TestAppContext, ui: &Ui, secs: f32, track: f32, m: gpui_kit::Modifiers) {
    cx.update_window(ui.win, |_, window, cx| {
        window.click_with_options(
            "timeline",
            gpui_kit::test::ClickOptions::new()
                .with_offset(timeline_point(secs, track))
                .with_modifiers(m),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
}

fn ctrl() -> gpui_kit::Modifiers {
    gpui_kit::Modifiers {
        control: true,
        ..Default::default()
    }
}

/// 视频轨上 0–1、1–2、2–4 秒三段。
fn three_clips(cx: &mut TestAppContext, ui: &Ui) -> Vec<ve_core::ClipId> {
    import(cx, ui, vec![fixture(Fixture::FrameIndexed30)], true);
    import(cx, ui, vec![fixture(Fixture::FrameIndexed30)], true);
    ui.view
        .read_with(cx, |w, _| w.engine().seek(Time::from_secs(1)));
    frame(cx, ui);
    press(cx, ui, "s");
    let ids: Vec<_> = video_clips(cx, ui).into_iter().map(|(c, _)| c).collect();
    assert_eq!(ids.len(), 3);
    press(cx, ui, "escape");
    ids
}

#[gpui_kit::test]
fn ctrl_click_toggles_and_shift_click_selects_range(cx: &mut TestAppContext) {
    let ui = open(cx);
    let c = three_clips(cx, &ui);
    let none = gpui_kit::Modifiers::default();
    let shift = gpui_kit::Modifiers {
        shift: true,
        ..Default::default()
    };
    click_timeline(cx, &ui, 0.5, 0.0, none);
    click_timeline(cx, &ui, 3.0, 0.0, shift);
    assert_eq!(
        selected(cx, &ui),
        ids(c.clone()),
        "Shift 点击：起点到这里的一段"
    );
    click_timeline(cx, &ui, 1.5, 0.0, ctrl());
    assert_eq!(selected(cx, &ui), ids(vec![c[0], c[2]]), "Ctrl 点击：减选");
    click_timeline(cx, &ui, 1.5, 0.0, ctrl());
    assert_eq!(selected(cx, &ui), ids(c.clone()), "Ctrl 点击：加选");
    // 多选里单击其中一个（不拖）：只剩它
    click_timeline(cx, &ui, 0.5, 0.0, none);
    assert_eq!(selected(cx, &ui), ids(vec![c[0]]));
}

#[gpui_kit::test]
fn dragging_one_of_selected_clips_moves_them_together(cx: &mut TestAppContext) {
    let ui = open(cx);
    let c = three_clips(cx, &ui);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    // 先删掉第三段（2–4 秒）腾出地方；再选中前两段，抓住第二段往后拖 1 秒（50 像素）
    click_timeline(cx, &ui, 3.0, 0.0, gpui_kit::Modifiers::default());
    press(cx, &ui, "delete");
    click_timeline(cx, &ui, 0.5, 0.0, gpui_kit::Modifiers::default());
    click_timeline(cx, &ui, 1.5, 0.0, ctrl());
    let from = origin + timeline_point(1.5, 0.0);
    cx.update_window(ui.win, |_, window, cx| {
        window.drag(from, from + point(px(50.0), px(0.0)), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let starts: Vec<Time> = video_clips(cx, &ui).into_iter().map(|(_, t)| t).collect();
    assert_eq!(starts, vec![Time::from_secs(1), Time::from_secs(2)]);
    assert_eq!(selected(cx, &ui), ids(vec![c[0], c[1]]), "拖完选择不变");
}

#[gpui_kit::test]
fn marquee_selects_touched_clips_and_escape_restores(cx: &mut TestAppContext) {
    let ui = open(cx);
    let c = three_clips(cx, &ui);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    click_timeline(cx, &ui, 0.5, 0.0, gpui_kit::Modifiers::default());
    // 从空白处（5 秒）框到 1.5 秒：碰到第二、三段，换掉原来的选择
    let from = origin + timeline_point(5.0, 0.0);
    let to = origin + timeline_point(1.5, 0.0);
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(selected(cx, &ui), ids(vec![c[1], c[2]]));
    // 按住 Ctrl 框：在原有选择上加；框到一半按 Esc 回到框之前
    click_timeline(cx, &ui, 0.5, 0.0, gpui_kit::Modifiers::default());
    press_and_move_with(cx, &ui, from, origin + timeline_point(3.0, 0.0), ctrl());
    assert_eq!(selected(cx, &ui), ids(vec![c[0], c[2]]));
    press(cx, &ui, "escape");
    assert_eq!(selected(cx, &ui), ids(vec![c[0]]));
    release(cx, &ui, to);
    assert_eq!(selected(cx, &ui), ids(vec![c[0]]));
}

#[gpui_kit::test]
fn alt_drag_in_preview_skips_snapping(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 0.0, gpui_kit::Modifiers::default());
    frame(cx, &ui);
    let from = preview_center(cx, &ui);
    let to = from + point(px(3.0), px(0.0));
    // 不按 Alt：离中线 3 像素，吸回中间
    press_and_move(cx, &ui, from, to);
    release(cx, &ui, to);
    assert_eq!(first_video_transform(cx, &ui).x, 0.0);
    let alt = gpui_kit::Modifiers {
        alt: true,
        ..Default::default()
    };
    press_and_move_with(cx, &ui, from, to, alt);
    release(cx, &ui, to);
    assert!(first_video_transform(cx, &ui).x > 0.0, "按住 Alt 不吸附");
}

/// 预览里画面区域（按工程宽高比放进预览框、居中）：（左, 上, 宽, 高）。
fn video_rect(cx: &mut TestAppContext, ui: &Ui) -> (f32, f32, f32, f32) {
    let b = cx
        .update_window(ui.win, |_, window, _| window.find("preview").bounds())
        .unwrap();
    let (pw, ph) = (f32::from(b.size.width), f32::from(b.size.height));
    let (cw, ch) = ui.view.read_with(cx, |w, _| {
        let s = w.document().project.settings;
        #[expect(clippy::cast_precision_loss, reason = "测试里的小尺寸")]
        (s.width as f32, s.height as f32)
    });
    let k = (pw / cw).min(ph / ch);
    let (w, h) = (cw * k, ch * k);
    (
        f32::from(b.origin.x) + (pw - w) / 2.0,
        f32::from(b.origin.y) + (ph - h) / 2.0,
        w,
        h,
    )
}

#[gpui_kit::test]
fn rotate_handle_drag_and_rotation_field(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 0.0, gpui_kit::Modifiers::default());
    frame(cx, &ui);
    // 缩到一半，旋转手柄（上边中点再往上 24 像素）就落在画面里
    type_into(cx, &ui, "tf-scale", "50");
    let (vx, vy, vw, vh) = video_rect(cx, &ui);
    let center = point(px(vx + vw / 2.0), px(vy + vh / 2.0));
    let handle = point(center.x, px(vy + vh / 4.0 - 24.0));
    // 从正上方拖到正右方：顺时针 90°（吸附开着，正好 90）
    let to = point(center.x + px(vw / 3.0), center.y + px(2.0));
    press_and_move(cx, &ui, handle, to);
    release(cx, &ui, to);
    let t = first_video_transform(cx, &ui);
    assert!((t.rotation - 90.0).abs() < 1e-3, "{t:?}");
    assert!((t.scale - 0.5).abs() < 1e-6, "旋转不改缩放");
    // 一次拖动一步撤销
    click(cx, &ui, "tb-undo");
    assert_eq!(first_video_transform(cx, &ui).rotation, 0.0);
    // 属性面板输入角度
    type_into(cx, &ui, "tf-rotation", "-30");
    assert!((first_video_transform(cx, &ui).rotation + 30.0).abs() < 1e-4);
}

#[gpui_kit::test]
fn snapping_toggle_by_button_and_n_key(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 0.0, gpui_kit::Modifiers::default());
    frame(cx, &ui);
    let from = preview_center(cx, &ui);
    let to = from + point(px(3.0), px(0.0));
    // N 关掉吸附：离中线 3 像素也不吸
    press(cx, &ui, "n");
    press_and_move(cx, &ui, from, to);
    release(cx, &ui, to);
    let off = first_video_transform(cx, &ui).x;
    assert!(off > 0.0, "吸附关着不吸");
    // 关着时按住 Alt：临时打开吸附，吸回中线
    let alt = gpui_kit::Modifiers {
        alt: true,
        ..Default::default()
    };
    let back = preview_center(cx, &ui) + point(px(3.0), px(0.0));
    press_and_move_with(cx, &ui, back, back + point(px(1.0), px(0.0)), alt);
    release(cx, &ui, back);
    assert_eq!(first_video_transform(cx, &ui).x, 0.0, "Alt 临时取反");
    // 工具栏按钮再打开：不按 Alt 也吸
    click(cx, &ui, "tb-snap");
    press_and_move(cx, &ui, from, to);
    release(cx, &ui, to);
    assert_eq!(first_video_transform(cx, &ui).x, 0.0);
}

/// 鼠标移到某处（不按键）。
fn hover_at(cx: &mut TestAppContext, ui: &Ui, at: Point<Pixels>) {
    use gpui_kit::{InputEvent as _, MouseMoveEvent};
    cx.update_window(ui.win, |_, window, cx| {
        window.dispatch_event(
            MouseMoveEvent {
                position: at,
                pressed_button: None,
                modifiers: gpui_kit::Modifiers::default(),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn cursors(cx: &TestAppContext, ui: &Ui) -> (gpui_kit::CursorStyle, gpui_kit::CursorStyle) {
    ui.view.read_with(cx, |w, _| w.pointer_cursors())
}

#[gpui_kit::test]
fn cursor_shows_what_a_press_would_do(cx: &mut TestAppContext) {
    use gpui_kit::CursorStyle;
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    // 时间轴：片段中间张开的手，边缘左右箭头，空白处箭头
    hover_at(cx, &ui, origin + timeline_point(1.0, 0.0));
    assert_eq!(cursors(cx, &ui).0, CursorStyle::OpenHand);
    hover_at(
        cx,
        &ui,
        origin + timeline_point(2.0, 0.0) - point(px(2.0), px(0.0)),
    );
    assert_eq!(cursors(cx, &ui).0, CursorStyle::ResizeLeftRight);
    hover_at(cx, &ui, origin + timeline_point(8.0, 0.0));
    assert_eq!(cursors(cx, &ui).0, CursorStyle::Arrow);
    // 预览：画面上张开的手，按下拖动握拳，松开回到张开的手
    let center = preview_center(cx, &ui);
    hover_at(cx, &ui, center);
    assert_eq!(cursors(cx, &ui).1, CursorStyle::OpenHand);
    let to = center + point(px(30.0), px(0.0));
    press_and_move(cx, &ui, center, to);
    assert_eq!(cursors(cx, &ui).1, CursorStyle::ClosedHand);
    release(cx, &ui, to);
    assert_eq!(cursors(cx, &ui).1, CursorStyle::OpenHand);
    // 选中后缩到一半：右上角是「/」向的缩放箭头
    type_into(cx, &ui, "tf-x", "0");
    type_into(cx, &ui, "tf-scale", "50");
    let (vx, vy, vw, vh) = video_rect(cx, &ui);
    hover_at(cx, &ui, point(px(vx + vw * 0.75), px(vy + vh * 0.25)));
    assert_eq!(cursors(cx, &ui).1, CursorStyle::ResizeUpRightDownLeft);
    // 旋转手柄：手指
    hover_at(cx, &ui, point(px(vx + vw / 2.0), px(vy + vh * 0.25 - 24.0)));
    assert_eq!(cursors(cx, &ui).1, CursorStyle::PointingHand);
}

#[gpui_kit::test]
fn dragging_just_outside_a_corner_rotates(cx: &mut TestAppContext) {
    use gpui_kit::CursorStyle;
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 0.0, gpui_kit::Modifiers::default());
    frame(cx, &ui);
    type_into(cx, &ui, "tf-scale", "50");
    let (vx, vy, vw, vh) = video_rect(cx, &ui);
    let center = (vx + vw / 2.0, vy + vh / 2.0);
    // 右下角外侧 14 像素（斜向）：旋转区
    let corner = (vx + vw * 0.75, vy + vh * 0.75);
    let from = point(px(corner.0 + 10.0), px(corner.1 + 10.0));
    hover_at(cx, &ui, from);
    // 无界面测试拿不到系统窗口，自定义旋转光标装不上，退回手指
    assert_eq!(cursors(cx, &ui).1, CursorStyle::PointingHand);
    // 绕中心拖到右上角外侧的同一方向：转了 -90°（逆时针）
    let to = point(
        px(center.0 + (f32::from(from.y) - center.1)),
        px(center.1 - (f32::from(from.x) - center.0)),
    );
    press_and_move(cx, &ui, from, to);
    release(cx, &ui, to);
    let t = first_video_transform(cx, &ui);
    assert!((t.rotation + 90.0).abs() < 1e-2, "{t:?}");
    assert!((t.scale - 0.5).abs() < 1e-6);
}
