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
