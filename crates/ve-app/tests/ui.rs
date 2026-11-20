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
    // 最多等约 10 秒：全部测试并行跑、机器又忙（比如在打游戏）时，真实的导出会慢不少
    for _ in 0..1000 {
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

/// 时间轴里某轨某时刻的局部坐标（默认几何：轨道头 132、标尺 28、轨高 56、50 像素 / 秒）。
/// `track` 是屏幕上的行号：最上面是字幕轨（第 0 行），下面依次是视频轨、音频轨。
fn timeline_point(secs: f32, track: f32) -> Point<Pixels> {
    point(px(132.0 + secs * 50.0), px(28.0 + 56.0 * track + 28.0))
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
    // J / K / L：L 正放、再按翻倍；J 倒放；K 暂停；空格回原速
    let rate = |app: &mut TestAppContext| ui.view.read_with(app, |w, _| w.engine().rate());
    press(cx, &ui, "l");
    assert!((rate(cx) - 1.0).abs() < f32::EPSILON);
    press(cx, &ui, "l");
    press(cx, &ui, "l");
    assert!((rate(cx) - 4.0).abs() < f32::EPSILON);
    assert!(exists(cx, &ui, "tr-rate"), "时间码旁显示 4x");
    for _ in 0..3 {
        press(cx, &ui, "l");
    }
    assert!((rate(cx) - 8.0).abs() < f32::EPSILON, "最多 8 倍");
    press(cx, &ui, "j");
    assert!((rate(cx) + 1.0).abs() < f32::EPSILON, "换方向从 1 倍开始");
    press(cx, &ui, "j");
    assert!((rate(cx) + 2.0).abs() < f32::EPSILON);
    press(cx, &ui, "k");
    assert!(!ui.view.read_with(cx, |w, _| w.engine().is_playing()));
    assert!(!exists(cx, &ui, "tr-rate"));
    press(cx, &ui, "l");
    press(cx, &ui, "l");
    press(cx, &ui, "space");
    press(cx, &ui, "space");
    assert!((rate(cx) - 1.0).abs() < f32::EPSILON, "空格回到原速");
    press(cx, &ui, "k");
}

#[gpui_kit::test]
fn split_key_splits_at_playhead_and_delete_removes_selection(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    ui.view
        .read_with(cx, |w, _| w.engine().seek(Time::from_secs(1)));
    frame(cx, &ui);
    press(cx, &ui, "ctrl-b");
    assert_eq!(clips(cx, &ui, TrackKind::Video), 2);
    // 单击时间轴上第一段（0–1 秒）选中，再按 Delete
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 1.0), cx);
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
    magnet_off(cx, &ui);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    let from = origin + timeline_point(1.0, 1.0);
    let to = origin + timeline_point(5.0, 1.0);
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
        window.click_at("timeline", point(px(132.0 + 50.0), px(10.0)), cx);
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
        codec: ve_export::Codec::H264,
        render: ve_export::RenderConfig { software: true },
        output: ve_export::Output::Video,
        loudness_lufs: None,
        range: None,
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
    magnet_off(cx, &ui);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    let mut vcx = gpui_kit::VisualTestContext::from_window(ui.win, cx);
    let none = gpui_kit::Modifiers::default();
    vcx.simulate_mouse_down(
        origin + timeline_point(1.0, 1.0),
        gpui_kit::MouseButton::Left,
        none,
    );
    vcx.simulate_mouse_move(
        origin + timeline_point(3.0, 1.0),
        Some(gpui_kit::MouseButton::Left),
        none,
    );
    // 在窗口外松开：收不到 mouse up，下一次移动时左键已经没按着
    vcx.simulate_mouse_move(origin + timeline_point(6.0, 1.0), None, none);
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
        window.click_at("timeline", timeline_point(20.0, 3.0), cx);
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
        window.click("tb-menu", cx);
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
        window.click("tb-menu", cx);
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
    // 新视频轨在视频轨的最上面（第 1 行，字幕轨在第 0 行），空轨直接删，不用确认
    click(cx, &ui, "track-del-1");
    assert_eq!(track_count(cx, &ui, TrackKind::Video), 1);
    // 只剩一条视频轨：没有删除按钮
    cx.update_window(ui.win, |_, window, _| {
        assert!(window.try_find("track-del-1").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn deleting_track_with_clips_asks_first(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click(cx, &ui, "add-video-track");
    // 第 1 行是新的空视频轨，第 2 行是放了片段的原视频轨（第 0 行是字幕轨）
    click(cx, &ui, "track-del-2");
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    assert_eq!(track_count(cx, &ui, TrackKind::Video), 2);
    click(cx, &ui, "track-del-2");
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
    let from = origin + timeline_point(1.0, 2.0);
    let to = origin + timeline_point(1.0, 1.0);
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let top = ui.view.read_with(cx, |w, _| {
        w.document().project.timeline.tracks[1].clips.len()
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
        window.click_at("timeline", timeline_point(0.5, 2.0), cx);
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
        window.click_at("timeline", timeline_point(0.5, 1.0), cx);
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
        window.click_at("timeline", timeline_point(0.5, 1.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    // 「混合」组在面板可见范围下面：先折起「位置大小」
    click(cx, &ui, "tf-title");
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
        window.click_at("timeline", timeline_point(0.5, 1.0), cx);
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
        origin + timeline_point(3.0, 2.0),
        origin + timeline_point(1.0, 1.0),
    );
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let (pip, bottom) = ui.view.read_with(cx, |w, _| {
        let t = &w.document().project.timeline.tracks;
        (t[1].clips[0].id, t[2].clips[0].id)
    });
    // 选中上层片段，在属性面板里缩小一半（居中）
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(0.5, 1.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    type_into(cx, &ui, "tf-scale", "50");
    // 点时间轴空白处取消选择
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(20.0, 3.0), cx);
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
    // 画中画外面、下层画面里（画面上边往下 10%）：选中下层
    let (_, vy, _, vh) = video_rect(cx, &ui);
    let top = cx
        .update_window(ui.win, |_, window, _| {
            window.find("preview").bounds().origin.y
        })
        .unwrap();
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at(
            "preview",
            point(size.width / 2.0, px(vy + vh * 0.1) - top),
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
    for _ in 0..5 {
        click(cx, &ui, "add-video-track");
    }
    // 8 条轨（S1 V6…V1 A1），时间轴放不下：在轨道头上滚轮往下滚
    cx.update_window(ui.win, |_, window, _| {
        assert!(window.try_find("track-del-1").is_some());
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
    // 第 1 行滚到标尺下面去了：它的删除按钮不再显示
    cx.update_window(ui.win, |_, window, _| {
        assert!(window.try_find("track-del-1").is_none());
        assert!(window.try_find("track-del-4").is_some());
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
        window.click_at("timeline", timeline_point(0.5, 1.0), cx);
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
        window.click_at("timeline", timeline_point(0.5, 2.0), cx);
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
        window.click_at("timeline", timeline_point(0.5, 1.0), cx);
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
        window.click_at("timeline", timeline_point(0.5, 1.0), cx);
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
        window.click_at("timeline", timeline_point(0.5, 1.0), cx);
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
    magnet_off(cx, &ui);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    let from = origin + timeline_point(0.5, 1.0);
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
    press(cx, &ui, "ctrl-b");
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
    press(cx, ui, "ctrl-b");
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
    click_timeline(cx, &ui, 0.5, 1.0, none);
    click_timeline(cx, &ui, 3.0, 1.0, shift);
    assert_eq!(
        selected(cx, &ui),
        ids(c.clone()),
        "Shift 点击：起点到这里的一段"
    );
    click_timeline(cx, &ui, 1.5, 1.0, ctrl());
    assert_eq!(selected(cx, &ui), ids(vec![c[0], c[2]]), "Ctrl 点击：减选");
    click_timeline(cx, &ui, 1.5, 1.0, ctrl());
    assert_eq!(selected(cx, &ui), ids(c.clone()), "Ctrl 点击：加选");
    // 多选里单击其中一个（不拖）：只剩它
    click_timeline(cx, &ui, 0.5, 1.0, none);
    assert_eq!(selected(cx, &ui), ids(vec![c[0]]));
}

#[gpui_kit::test]
fn dragging_one_of_selected_clips_moves_them_together(cx: &mut TestAppContext) {
    let ui = open(cx);
    let c = three_clips(cx, &ui);
    magnet_off(cx, &ui);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    // 先删掉第三段（2–4 秒）腾出地方；再选中前两段，抓住第二段往后拖 1 秒（50 像素）
    click_timeline(cx, &ui, 3.0, 1.0, gpui_kit::Modifiers::default());
    press(cx, &ui, "delete");
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click_timeline(cx, &ui, 1.5, 1.0, ctrl());
    let from = origin + timeline_point(1.5, 1.0);
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
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    // 从空白处（5 秒）框到 1.5 秒：碰到第二、三段，换掉原来的选择
    let from = origin + timeline_point(5.0, 1.0);
    let to = origin + timeline_point(1.5, 1.0);
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(selected(cx, &ui), ids(vec![c[1], c[2]]));
    // 按住 Ctrl 框：在原有选择上加；框到一半按 Esc 回到框之前
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    press_and_move_with(cx, &ui, from, origin + timeline_point(3.0, 1.0), ctrl());
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
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
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
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    frame(cx, &ui);
    // 缩到一半，旋转手柄（下边中点再往下 24 像素，剪映的位置）就落在画面里
    type_into(cx, &ui, "tf-scale", "50");
    let (vx, vy, vw, vh) = video_rect(cx, &ui);
    let center = point(px(vx + vw / 2.0), px(vy + vh / 2.0));
    let handle = point(center.x, px(vy + vh * 0.75 + 24.0));
    // 从正下方拖到正左方：顺时针 90°（吸附开着，正好 90）
    let to = point(center.x - px(vw / 3.0), center.y - px(2.0));
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
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
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
    hover_at(cx, &ui, origin + timeline_point(1.0, 1.0));
    assert_eq!(cursors(cx, &ui).0, CursorStyle::OpenHand);
    hover_at(
        cx,
        &ui,
        origin + timeline_point(2.0, 1.0) - point(px(2.0), px(0.0)),
    );
    assert_eq!(cursors(cx, &ui).0, CursorStyle::ResizeLeftRight);
    hover_at(cx, &ui, origin + timeline_point(8.0, 1.0));
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
    // 上边中点：上下箭头；左边中点：左右箭头
    hover_at(cx, &ui, point(px(vx + vw / 2.0), px(vy + vh * 0.25)));
    assert_eq!(cursors(cx, &ui).1, CursorStyle::ResizeUpDown);
    hover_at(cx, &ui, point(px(vx + vw * 0.25), px(vy + vh / 2.0)));
    assert_eq!(cursors(cx, &ui).1, CursorStyle::ResizeLeftRight);
    // 旋转手柄（框下方）：手指
    hover_at(cx, &ui, point(px(vx + vw / 2.0), px(vy + vh * 0.75 + 24.0)));
    assert_eq!(cursors(cx, &ui).1, CursorStyle::PointingHand);
}

#[gpui_kit::test]
fn dragging_just_outside_a_corner_rotates(cx: &mut TestAppContext) {
    use gpui_kit::CursorStyle;
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
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

/// 视频轨第一个片段（完整的，含动画）。
fn first_video_clip(cx: &TestAppContext, ui: &Ui) -> ve_core::Clip {
    ui.view.read_with(cx, |w, _| {
        (*w.document()
            .project
            .timeline
            .track_of_kind(TrackKind::Video)
            .unwrap()
            .clips[0])
            .clone()
    })
}

fn seek_to(cx: &mut TestAppContext, ui: &Ui, t: Time) {
    ui.view.read_with(cx, |w, _| w.engine().seek(t));
    frame(cx, ui);
}

fn position(cx: &TestAppContext, ui: &Ui) -> Time {
    ui.view.read_with(cx, |w, _| w.engine().position())
}

#[gpui_kit::test]
fn keyframe_buttons_add_jump_ease_and_remove(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    seek_to(cx, &ui, Time::ZERO);
    click(cx, &ui, "tf-key");
    assert_eq!(
        first_video_clip(cx, &ui).animation.times(),
        vec![Time::ZERO]
    );
    // 1 秒处改缩放：有动画了，自动在 1 秒打关键帧
    seek_to(cx, &ui, Time::from_secs(1));
    type_into(cx, &ui, "tf-scale", "50");
    let c = first_video_clip(cx, &ui);
    assert_eq!(c.animation.times(), vec![Time::ZERO, Time::from_secs(1)]);
    let mid = c.transform_at(Time::from_millis(500)).scale;
    assert!(mid > 0.5 && mid < 1.0, "中间在两者之间：{mid}");
    // 跳到上一个、下一个关键帧
    click(cx, &ui, "tf-key-prev");
    assert_eq!(position(cx, &ui), Time::ZERO);
    // 在 0 秒的关键帧上改缓动
    click(cx, &ui, "tf-ease-linear");
    assert_eq!(
        first_video_clip(cx, &ui).animation.easing_at(Time::ZERO),
        Some(ve_core::Easing::Linear)
    );
    click(cx, &ui, "tf-key-next");
    assert_eq!(position(cx, &ui), Time::from_secs(1));
    // 在关键帧上再点 ◆：删掉它
    click(cx, &ui, "tf-key");
    assert_eq!(
        first_video_clip(cx, &ui).animation.times(),
        vec![Time::ZERO]
    );
    // 撤销回来
    click(cx, &ui, "tb-undo");
    assert_eq!(first_video_clip(cx, &ui).animation.times().len(), 2);
}

#[gpui_kit::test]
fn dragging_an_animated_clip_keys_the_playhead(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    seek_to(cx, &ui, Time::ZERO);
    click(cx, &ui, "tf-key");
    seek_to(cx, &ui, Time::from_secs(1));
    let from = preview_center(cx, &ui);
    let to = from + point(px(40.0), px(0.0));
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let c = first_video_clip(cx, &ui);
    assert_eq!(c.animation.times(), vec![Time::ZERO, Time::from_secs(1)]);
    assert!(c.transform_at(Time::from_secs(1)).x > 0.01);
    assert_eq!(c.transform_at(Time::ZERO).x, 0.0, "0 秒的关键帧不动");
    // 一次拖动一步撤销：撤销后 1 秒的关键帧没了
    click(cx, &ui, "tb-undo");
    assert_eq!(
        first_video_clip(cx, &ui).animation.times(),
        vec![Time::ZERO]
    );
}

#[gpui_kit::test]
fn keyframe_button_does_nothing_off_the_clip(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    // 片段只有 2 秒，播放头放到 3 秒：按钮不可用
    seek_to(cx, &ui, Time::from_secs(3));
    click(cx, &ui, "tf-key");
    assert!(first_video_clip(cx, &ui).animation.is_empty());
    // 重置会清掉动画
    seek_to(cx, &ui, Time::ZERO);
    click(cx, &ui, "tf-key");
    assert!(!first_video_clip(cx, &ui).animation.is_empty());
    click(cx, &ui, "tf-reset");
    assert!(first_video_clip(cx, &ui).animation.is_empty());
}

fn exists(cx: &mut TestAppContext, ui: &Ui, id: &'static str) -> bool {
    frame(cx, ui);
    cx.update_window(ui.win, |_, window, _| window.try_find(id).is_some())
        .unwrap()
}

#[gpui_kit::test]
fn menu_items_run_their_actions_and_close_the_menu(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    assert!(!exists(cx, &ui, "app-menu"));
    click(cx, &ui, "tb-menu");
    assert!(exists(cx, &ui, "app-menu"));
    click(cx, &ui, "menu-select-all");
    assert!(!exists(cx, &ui, "app-menu"), "点了菜单项就收起");
    assert_eq!(selected(cx, &ui).len(), 1);
    click(cx, &ui, "tb-menu");
    click(cx, &ui, "menu-delete");
    assert_eq!(clips(cx, &ui, TrackKind::Video), 0);
    click(cx, &ui, "tb-menu");
    click(cx, &ui, "menu-undo");
    assert_eq!(clips(cx, &ui, TrackKind::Video), 1);
}

#[gpui_kit::test]
fn bin_tabs_switch_content(cx: &mut TestAppContext) {
    let ui = open(cx);
    assert!(exists(cx, &ui, "bin-import"), "默认是「素材」标签");
    click(cx, &ui, "bin-tab-transition");
    assert!(!exists(cx, &ui, "bin-import"));
    click(cx, &ui, "bin-tab-text");
    click(cx, &ui, "bin-text-default");
    assert_eq!(
        clips(cx, &ui, TrackKind::Subtitle),
        1,
        "「默认文本」在播放头处加一条字幕"
    );
    click(cx, &ui, "bin-tab-media");
    assert!(exists(cx, &ui, "bin-import"));
}

#[gpui_kit::test]
fn inspector_tabs_follow_clip_kind(cx: &mut TestAppContext) {
    let ui = open(cx);
    assert!(exists(cx, &ui, "insp-tab-draft"), "没选中时是「草稿参数」");
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    press(cx, &ui, "ctrl-a");
    assert!(exists(cx, &ui, "tf-scale"), "视频片段默认在「画面」");
    click(cx, &ui, "insp-tab-audio");
    assert!(exists(cx, &ui, "insp-audio"));
    assert!(!exists(cx, &ui, "tf-scale"));
    // 字幕没有「音频」标签：显示「文本」
    press(cx, &ui, "t");
    assert!(exists(cx, &ui, "subtitle-text"));
    assert!(!exists(cx, &ui, "insp-audio"));
}

/// 视频轨上各片段的（起点，终点）。
fn video_spans(cx: &TestAppContext, ui: &Ui) -> Vec<(Time, Time)> {
    ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video)
            .flat_map(|t| t.clips.iter().map(|c| (c.start, c.end())))
            .collect()
    })
}

#[gpui_kit::test]
fn ctrl_b_splits_and_selects_the_right_half(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    press(cx, &ui, "ctrl-a");
    seek_to(cx, &ui, Time::from_secs(1));
    press(cx, &ui, "ctrl-b");
    assert_eq!(
        video_spans(cx, &ui),
        [
            (Time::ZERO, Time::from_secs(1)),
            (Time::from_secs(1), Time::from_secs(2))
        ]
    );
    let right = ui.view.read_with(cx, |w, _| {
        w.document().project.timeline.tracks[1].clips[1].id
    });
    assert_eq!(selected(cx, &ui), [right], "剪映：分割后右半段自动选中");
}

#[gpui_kit::test]
fn ctrl_shift_b_splits_every_track_at_the_playhead(cx: &mut TestAppContext) {
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
    seek_to(cx, &ui, Time::from_secs(1));
    press(cx, &ui, "ctrl-shift-b");
    assert_eq!(clips(cx, &ui, TrackKind::Video), 2);
    assert_eq!(clips(cx, &ui, TrackKind::Audio), 2);
}

#[gpui_kit::test]
fn q_and_w_trim_at_the_playhead(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    // 没选中：作用于播放头下的片段
    seek_to(cx, &ui, Time::from_secs_f64(1.5));
    press(cx, &ui, "w");
    assert_eq!(
        video_spans(cx, &ui),
        [(Time::ZERO, Time::from_secs_f64(1.5))]
    );
    seek_to(cx, &ui, Time::from_secs_f64(0.5));
    press(cx, &ui, "q");
    // 主轨磁吸（默认开）：剪掉左边 0.5 秒后，片段补回 0 秒
    assert_eq!(video_spans(cx, &ui), [(Time::ZERO, Time::from_secs(1))]);
    // 一步一撤销
    press(cx, &ui, "ctrl-z");
    assert_eq!(
        video_spans(cx, &ui),
        [(Time::ZERO, Time::from_secs_f64(1.5))]
    );
}

#[gpui_kit::test]
fn brackets_select_everything_left_or_right_of_the_playhead(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    // 两段：0–2、2–4；播放头在 3 秒
    seek_to(cx, &ui, Time::from_secs(3));
    press(cx, &ui, "[");
    assert_eq!(selected(cx, &ui).len(), 2, "起点在播放头左边的都选上");
    press(cx, &ui, "]");
    let second = ui.view.read_with(cx, |w, _| {
        w.document().project.timeline.tracks[1].clips[1].id
    });
    assert_eq!(selected(cx, &ui), [second], "终点在播放头右边的");
}

#[gpui_kit::test]
fn dropping_a_clip_between_tracks_creates_a_new_track(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    frame(cx, &ui);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    // 视频片段（第 1 行）拖到字幕轨和视频轨的分界（第 1 行上沿）
    let from = origin + timeline_point(0.5, 1.0);
    let to = origin + point(px(132.0 + 2.0 * 50.0), px(28.0 + 56.0 + 2.0));
    press_and_move(cx, &ui, from, to);
    let tracks = track_count(cx, &ui, TrackKind::Video);
    assert_eq!(tracks, 1, "拖动中还没建轨");
    release(cx, &ui, to);
    assert_eq!(track_count(cx, &ui, TrackKind::Video), 2);
    let (on_new, start) = ui.view.read_with(cx, |w, _| {
        let t = &w.document().project.timeline.tracks;
        (t[1].clips.len(), t[1].clips.first().map(|c| c.start))
    });
    assert_eq!(on_new, 1, "片段进了新建的那条（视频块最上面）");
    assert_eq!(start, Some(Time::from_secs_f64(1.5)));
    // 建轨和移动是一步撤销
    press(cx, &ui, "ctrl-z");
    assert_eq!(track_count(cx, &ui, TrackKind::Video), 1);
}

/// 关掉主轨磁吸（P）：测在主轨上自由拖动的用例用。
fn magnet_off(cx: &mut TestAppContext, ui: &Ui) {
    press(cx, ui, "p");
    let on = ui
        .view
        .read_with(cx, |w, _| w.document().project.settings.main_magnet);
    assert!(!on, "P 关掉了主轨磁吸");
}

#[gpui_kit::test]
fn magnet_toggle_packs_main_track_when_turned_on(cx: &mut TestAppContext) {
    let ui = open(cx);
    let c = three_clips(cx, &ui);
    magnet_off(cx, &ui);
    // 关着时删掉第一段：留出空隙
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    press(cx, &ui, "delete");
    assert_eq!(video_clips(cx, &ui)[0], (c[1], Time::from_secs(1)));
    // 工具栏按钮打开：立刻排紧；撤销一步回到有空隙
    click(cx, &ui, "tb-magnet");
    assert_eq!(video_clips(cx, &ui)[0], (c[1], Time::ZERO));
    press(cx, &ui, "ctrl-z");
    assert_eq!(video_clips(cx, &ui)[0], (c[1], Time::from_secs(1)));
}

#[gpui_kit::test]
fn linkage_toggle_is_a_project_setting(cx: &mut TestAppContext) {
    let ui = open(cx);
    let linked = |app: &TestAppContext| {
        ui.view
            .read_with(app, |w, _| w.document().project.settings.linkage)
    };
    assert!(linked(cx), "新工程默认联动");
    click(cx, &ui, "tb-linkage");
    assert!(!linked(cx));
    press(cx, &ui, "`");
    assert!(linked(cx));
}

#[gpui_kit::test]
fn ratio_menu_switches_between_landscape_and_portrait(cx: &mut TestAppContext) {
    let ui = open(cx);
    let size = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            let s = w.document().project.settings;
            (s.width, s.height)
        })
    };
    assert_eq!(size(cx), (1920, 1080));
    click(cx, &ui, "tr-ratio");
    assert!(exists(cx, &ui, "ratio-menu"));
    click(cx, &ui, "ratio-9-16");
    assert_eq!(size(cx), (1080, 1920), "竖屏：短边不变");
    assert!(!exists(cx, &ui, "ratio-menu"), "选了就收起");
    // 一步撤销
    press(cx, &ui, "ctrl-z");
    assert_eq!(size(cx), (1920, 1080));
}

#[gpui_kit::test]
fn each_row_keys_only_its_own_property(cx: &mut TestAppContext) {
    use ve_core::Prop;
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    seek_to(cx, &ui, Time::ZERO);
    // 「缩放」一行的 ◇：只有缩放有关键帧
    click(cx, &ui, "tf-key-scale");
    let keyed = |app: &TestAppContext| -> Vec<Prop> {
        first_video_clip(app, &ui).animation.props().collect()
    };
    assert_eq!(keyed(cx), [Prop::Scale]);
    // 1 秒处改缩放：自动打第二帧；改位置：只改固定值
    seek_to(cx, &ui, Time::from_secs(1));
    type_into(cx, &ui, "tf-scale", "50");
    type_into(cx, &ui, "tf-x", "10");
    let c = first_video_clip(cx, &ui);
    assert_eq!(c.animation.keys(Prop::Scale).len(), 2);
    assert_eq!(c.animation.keys(Prop::X), []);
    assert!((c.transform.x - 0.1).abs() < 1e-6);
    // 「‹」跳回这一行的上一个关键帧
    click(cx, &ui, "tf-key-scale-prev");
    assert_eq!(position(cx, &ui), Time::ZERO);
    // 在关键帧上再点 ◆：删掉这一帧
    click(cx, &ui, "tf-key-scale");
    assert_eq!(
        first_video_clip(cx, &ui).animation.keys(Prop::Scale).len(),
        1
    );
}

#[gpui_kit::test]
fn align_buttons_snap_the_layer_to_canvas_edges(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    frame(cx, &ui);
    type_into(cx, &ui, "tf-scale", "50");
    // 64×36 的素材铺满 1920×1080 的画布；缩到一半后框宽占画布一半
    click(cx, &ui, "tf-align-left");
    assert!((first_video_transform(cx, &ui).x + 0.25).abs() < 1e-6);
    click(cx, &ui, "tf-align-bottom");
    let t = first_video_transform(cx, &ui);
    assert!((t.y - 0.25).abs() < 1e-6 && (t.x + 0.25).abs() < 1e-6);
    click(cx, &ui, "tf-align-cx");
    assert_eq!(first_video_transform(cx, &ui).x, 0.0);
}

#[gpui_kit::test]
fn draft_params_change_ratio_resolution_and_frame_rate(cx: &mut TestAppContext) {
    let ui = open(cx);
    let settings =
        |app: &TestAppContext| ui.view.read_with(app, |w, _| w.document().project.settings);
    click(cx, &ui, "draft-ratio-9-16");
    let portrait = settings(cx);
    assert_eq!((portrait.width, portrait.height), (1080, 1920));
    click(cx, &ui, "draft-res-720");
    let small = settings(cx);
    assert_eq!((small.width, small.height), (720, 1280));
    click(cx, &ui, "draft-fps-60");
    assert_eq!(settings(cx).frame_rate, ve_core::FrameRate::new(60, 1));
    press(cx, &ui, "ctrl-z");
    assert_eq!(settings(cx).frame_rate, ve_core::FrameRate::new(30, 1));
}

#[gpui_kit::test]
fn animation_tab_applies_entrance_and_exit_presets(cx: &mut TestAppContext) {
    use ve_core::Prop;
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-animation");
    // 入场渐显 0.5 秒
    click(cx, &ui, "anim-in-fade");
    let c = first_video_clip(cx, &ui);
    assert_eq!(c.transform_at(Time::ZERO).opacity, 0.0);
    assert_eq!(c.transform_at(Time::from_secs(1)).opacity, 1.0);
    // 出场：时长改成 1 秒，向左滑出
    click(cx, &ui, "anim-phase-out");
    click(cx, &ui, "anim-dur-1s");
    click(cx, &ui, "anim-out-left");
    let out = first_video_clip(cx, &ui);
    assert_eq!(
        out.animation.times_of(&[Prop::X]),
        [Time::from_secs(1), Time::from_secs(2)]
    );
    assert_eq!(out.transform_at(Time::from_secs(2)).x, -1.0);
    // 一次套用一步撤销
    press(cx, &ui, "ctrl-z");
    assert_eq!(first_video_clip(cx, &ui).animation.keys(Prop::X), []);
}

/// 字幕轨上各条字幕的（起点，文字）。
fn subtitles(cx: &TestAppContext, ui: &Ui) -> Vec<(Time, String)> {
    ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .track_of_kind(TrackKind::Subtitle)
            .map(|t| {
                t.clips
                    .iter()
                    .map(|c| (c.start, c.subtitle_text().unwrap_or_default().to_owned()))
                    .collect()
            })
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
fn subtitle_list_selects_inserts_and_deletes(cx: &mut TestAppContext) {
    let ui = open(cx);
    // 0 秒加一条（2 秒长），选中后属性面板是字幕列表
    press(cx, &ui, "t");
    assert!(exists(cx, &ui, "subtitle-list"));
    assert!(exists(cx, &ui, "subtitle-text"), "选中那行展开文字框");
    // 第一条后面插一条：紧接在 2 秒，新的那条被选中、播放头跳过去
    click(cx, &ui, "sub-add-0");
    assert_eq!(
        subtitles(cx, &ui),
        [
            (Time::ZERO, "新字幕".to_owned()),
            (Time::from_secs(2), "新字幕".to_owned())
        ]
    );
    assert_eq!(position(cx, &ui), Time::from_secs(2));
    // 点第一行：选中它、播放头回到 0
    click(cx, &ui, "sub-row-0");
    assert_eq!(position(cx, &ui), Time::ZERO);
    let first = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .track_of_kind(TrackKind::Subtitle)
            .unwrap()
            .clips[0]
            .id
    });
    assert_eq!(selected(cx, &ui), [first]);
    // 删掉第二条
    click(cx, &ui, "sub-del-1");
    assert_eq!(subtitles(cx, &ui).len(), 1);
}

#[gpui_kit::test]
fn autosave_names_new_drafts_by_date_after_edits_settle(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let drafts = dir.path().join("drafts");
    let recent = dir.path().join("recent.json");
    cx.update_window(ui.win, |_, window, cx| {
        ui.view.update(cx, |w, cx| {
            w.use_recent_file(recent.clone());
            w.enable_autosave(drafts.clone(), window, cx);
        });
    })
    .unwrap();
    press(cx, &ui, "t");
    // 刚改完不存，停下 2 秒后存
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    assert!(ui.view.read_with(cx, |w, _| w.document().is_dirty()));
    for _ in 0..8 {
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
    }
    let (dirty, path) = ui.view.read_with(cx, |w, _| {
        (
            w.document().is_dirty(),
            w.document().path().map(PathBuf::from),
        )
    });
    assert!(!dirty);
    let path = path.unwrap();
    assert_eq!(path.parent(), Some(drafts.as_path()));
    let name = path.file_stem().unwrap().to_string_lossy().into_owned();
    assert!(name.contains('月') && name.contains('日'), "{name}");
    assert!(path.exists());
}

#[gpui_kit::test]
fn adjust_tab_sliders_color_the_clip(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-adjust");
    assert!(exists(cx, &ui, "insp-adjust"));
    let bounds = cx
        .update_window(ui.win, |_, window, _| {
            window.find("adj-saturation").bounds()
        })
        .unwrap();
    // 从中间（0）拖到最左（-100）：变黑白
    let y = bounds.origin.y + bounds.size.height / 2.0;
    let from = point(bounds.origin.x + bounds.size.width / 2.0, y);
    let to = point(bounds.origin.x + px(1.0), y);
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let adjust = first_video_clip(cx, &ui).adjust;
    assert!(adjust.saturation < -0.9, "{adjust:?}");
    assert_eq!(adjust.brightness, 0.0);
    // 一次拖动一步撤销；「重置」回到不调色
    press(cx, &ui, "ctrl-z");
    assert!(first_video_clip(cx, &ui).adjust.is_identity());
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    click(cx, &ui, "adj-reset");
    assert!(first_video_clip(cx, &ui).adjust.is_identity());
}

/// 在时间轴上右键点一下。
fn right_click_timeline(cx: &mut TestAppContext, ui: &Ui, secs: f32, track: f32) {
    use gpui_kit::{InputEvent as _, MouseButton, MouseDownEvent, MouseUpEvent};
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    let at = origin + timeline_point(secs, track);
    cx.update_window(ui.win, |_, window, cx| {
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Right,
                position: at,
                modifiers: gpui_kit::Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Right,
                position: at,
                modifiers: gpui_kit::Modifiers::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, ui);
}

#[gpui_kit::test]
fn context_menu_copies_pastes_and_disables(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    // 右键片段：选中它、弹出菜单
    right_click_timeline(cx, &ui, 0.5, 1.0);
    assert!(exists(cx, &ui, "clip-menu"));
    assert_eq!(selected(cx, &ui).len(), 1);
    click(cx, &ui, "cm-copy");
    assert!(!exists(cx, &ui, "clip-menu"), "点了就收起");
    // 播放头放到末尾粘贴：主轨磁吸开着，接在后面
    seek_to(cx, &ui, Time::from_secs(2));
    press(cx, &ui, "ctrl-v");
    assert_eq!(
        video_spans(cx, &ui),
        [
            (Time::ZERO, Time::from_secs(2)),
            (Time::from_secs(2), Time::from_secs(4))
        ]
    );
    // 停用（V）：片段还在，菜单里变成「启用片段」
    press(cx, &ui, "v");
    assert!(ui.view.read_with(cx, |w, _| {
        let id = w.document().selection()[0];
        w.document()
            .project
            .timeline
            .find_clip(id)
            .unwrap()
            .1
            .disabled
    }));
    right_click_timeline(cx, &ui, 2.5, 1.0);
    click(cx, &ui, "cm-disable");
    assert!(ui.view.read_with(cx, |w, _| {
        let id = w.document().selection()[0];
        !w.document()
            .project
            .timeline
            .find_clip(id)
            .unwrap()
            .1
            .disabled
    }));
}

#[gpui_kit::test]
fn copy_attributes_to_another_clip(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    type_into(cx, &ui, "tf-scale", "40");
    press(cx, &ui, "escape");
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    press(cx, &ui, "ctrl-shift-c");
    click_timeline(cx, &ui, 2.5, 1.0, gpui_kit::Modifiers::default());
    press(cx, &ui, "ctrl-shift-v");
    let scales: Vec<f32> = ui.view.read_with(cx, |w, _| {
        w.document().project.timeline.tracks[1]
            .clips
            .iter()
            .map(|c| c.transform.scale)
            .collect()
    });
    assert_eq!(scales.len(), 2);
    assert!((scales[1] - 0.4).abs() < 1e-6, "{scales:?}");
}

#[gpui_kit::test]
fn markers_toggle_and_jump(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    seek_to(cx, &ui, Time::from_secs(1));
    press(cx, &ui, "m");
    click(cx, &ui, "tb-undo");
    assert!(
        ui.view
            .read_with(cx, |w, _| w.document().project.timeline.markers.is_empty())
    );
    press(cx, &ui, "ctrl-y");
    seek_to(cx, &ui, Time::ZERO);
    press(cx, &ui, "shift-m");
    assert_eq!(position(cx, &ui), Time::from_secs(1));
    seek_to(cx, &ui, Time::from_secs(2));
    press(cx, &ui, "alt-shift-m");
    assert_eq!(position(cx, &ui), Time::from_secs(1));
    click(cx, &ui, "tb-marker");
    let markers = ui
        .view
        .read_with(cx, |w, _| w.document().project.timeline.markers.clone());
    assert_eq!(markers, [], "同一处再按一次去掉");
}

#[gpui_kit::test]
fn text_tab_styles_all_subtitles(cx: &mut TestAppContext) {
    let ui = open(cx);
    press(cx, &ui, "t");
    click(cx, &ui, "insp-tab-text");
    assert!(exists(cx, &ui, "insp-text-style"));
    click(cx, &ui, "ts-preset-yellow-black");
    click(cx, &ui, "ts-font-hei");
    click(cx, &ui, "ts-bold");
    let style = ui
        .view
        .read_with(cx, |w, _| w.document().project.subtitle_style.clone());
    assert_eq!(
        (style.color, style.font.as_str(), style.bold),
        (0xFF_D700, "SimHei", true)
    );
    // 预览里的字幕跟着变（粗体黄字）
    frame(cx, &ui);
    assert!(exists(cx, &ui, "subtitle-main"));
    // 每次点选一步撤销
    press(cx, &ui, "escape");
    press(cx, &ui, "ctrl-z");
    let bold = ui
        .view
        .read_with(cx, |w, _| w.document().project.subtitle_style.bold);
    assert!(!bold);
}

#[gpui_kit::test]
fn export_dialog_choices_reach_the_output_file(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("hevc.mp4");
    open_export_dialog(cx, &ui, &out);
    for id in ["codec-hevc", "br-low", "export-advanced"] {
        cx.update_window(ui.win, |_, window, cx| window.click(id, cx))
            .unwrap();
        cx.run_until_parked();
        frame(cx, &ui);
    }
    cx.update_window(ui.win, |_, window, cx| {
        assert!(
            window.try_find("export-estimate").is_some(),
            "底部显示估计大小"
        );
        window.click("enc-x264", cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    cx.update_window(ui.win, |_, window, cx| window.click("export-confirm", cx))
        .unwrap();
    cx.run_until_parked();
    wait(&mut *cx, &ui, |w| !w.is_exporting());
    let probe = ve_media::probe(&out).unwrap();
    let v = probe.video.unwrap();
    // 「原始」分辨率：和工程一样（素材 64×36）；720P 等按短边换算见 export_ui 的单元测试
    assert_eq!((v.width, v.height), (64, 36));
}

#[gpui_kit::test]
fn portrait_layout_puts_the_player_in_a_full_height_column(cx: &mut TestAppContext) {
    let ui = open(cx);
    let bounds = |app: &mut TestAppContext, id: &'static str| {
        app.update_window(ui.win, |_, window, _| window.find(id).bounds())
            .unwrap()
    };
    let before = bounds(cx, "preview");
    let timeline = bounds(cx, "timeline");
    assert!(
        before.origin.y < timeline.origin.y,
        "默认：播放器在时间轴上面"
    );
    click(cx, &ui, "tb-layout");
    click(cx, &ui, "layout-portrait");
    frame(cx, &ui);
    let after = bounds(cx, "preview");
    let lower = bounds(cx, "timeline");
    assert!(
        after.origin.x > lower.origin.x + lower.size.width - px(1.0),
        "播放器在时间轴右边"
    );
    assert!(
        after.size.height > before.size.height,
        "竖屏创作：播放器更高"
    );
    click(cx, &ui, "tb-layout");
    click(cx, &ui, "layout-default");
    frame(cx, &ui);
    assert!(bounds(cx, "preview").origin.y < bounds(cx, "timeline").origin.y);
}

#[gpui_kit::test]
fn shortcuts_can_be_rebound_and_reset(cx: &mut TestAppContext) {
    let ui = open(cx);
    let tracks = |app: &TestAppContext| {
        // 字幕片段数（T 在播放头加一条字幕）
        ui.view.read_with(app, |w, _| {
            w.document()
                .project
                .timeline
                .tracks
                .iter()
                .map(|t| t.clips.len())
                .sum::<usize>()
        })
    };
    // 把「加字幕」从 T 改成 Ctrl+K
    click(cx, &ui, "tb-shortcuts");
    click(cx, &ui, "sc-t");
    press(cx, &ui, "ctrl-k");
    let before = tracks(cx);
    press(cx, &ui, "escape");
    press(cx, &ui, "t");
    assert_eq!(tracks(cx), before, "T 不再加字幕");
    press(cx, &ui, "ctrl-k");
    assert_eq!(tracks(cx), before + 1, "Ctrl+K 加字幕");
    // 把「停用片段」(V) 改成 Ctrl+K：加字幕的 Ctrl+K 被去掉
    click(cx, &ui, "tb-shortcuts");
    click(cx, &ui, "sc-v");
    press(cx, &ui, "ctrl-k");
    let note = ui
        .view
        .read_with(cx, |w, _| w.shortcut_note().map(str::to_owned));
    assert!(note.is_some_and(|n| n.contains("Ctrl+K")));
    press(cx, &ui, "escape");
    let n = tracks(cx);
    press(cx, &ui, "ctrl-k");
    assert_eq!(tracks(cx), n, "Ctrl+K 现在是停用片段，不加字幕");
    // 正在改时按 Esc：不改
    click(cx, &ui, "tb-shortcuts");
    click(cx, &ui, "sc-ctrl-b");
    press(cx, &ui, "escape");
    assert_eq!(
        cx.update(|cx| ve_app::keymap::current(cx, "editor::Split|ctrl-b")),
        "ctrl-b"
    );
    // 恢复默认
    click(cx, &ui, "shortcuts-reset");
    press(cx, &ui, "escape");
    // 播放头挪到已有那条字幕后面（同一处已经有字幕时加不了）
    press(cx, &ui, "end");
    let after_reset = tracks(cx);
    press(cx, &ui, "t");
    assert_eq!(tracks(cx), after_reset + 1, "T 又能加字幕");
}

#[gpui_kit::test]
fn player_title_and_menu(cx: &mut TestAppContext) {
    let ui = open(cx);
    click(cx, &ui, "player-menu-btn");
    click(cx, &ui, "player-quality-smooth");
    assert_eq!(
        ui.view
            .read_with(cx, |w, _| w.engine().preview_max().max_width),
        1280,
        "清晰画质"
    );
    assert!(!exists(cx, &ui, "player-menu"), "选了就关");
    click(cx, &ui, "player-menu-btn");
    click(cx, &ui, "player-axis");
    assert!(ui.view.read_with(cx, |w, _| w.preview_axis_on()));
    // Esc 关菜单
    click(cx, &ui, "player-menu-btn");
    press(cx, &ui, "escape");
    assert!(!exists(cx, &ui, "player-menu"));
}

#[gpui_kit::test]
fn media_inspector_and_audio_layouts(cx: &mut TestAppContext) {
    let ui = open(cx);
    let bounds = |app: &mut TestAppContext, id: &'static str| {
        app.update_window(ui.win, |_, window, _| window.find(id).bounds())
            .unwrap()
    };
    let pick = |app: &mut TestAppContext, id: &'static str| {
        click(app, &ui, "tb-layout");
        click(app, &ui, id);
        frame(app, &ui);
    };
    let default_timeline = bounds(cx, "timeline");
    // 媒体素材优先：素材从上到下占满，时间轴在它右边
    pick(cx, "layout-media");
    let bin = bounds(cx, "media-bin");
    let tl = bounds(cx, "timeline");
    assert!(
        bin.size.height > tl.origin.y - bin.origin.y,
        "素材比时间轴顶边还长"
    );
    assert!(
        tl.origin.x > bin.origin.x + bin.size.width - px(1.0),
        "时间轴在素材右边"
    );
    // 属性调节优先：属性占满右边，时间轴在它左边
    pick(cx, "layout-inspector");
    let insp = bounds(cx, "inspector");
    let left_tl = bounds(cx, "timeline");
    assert!(insp.size.height > left_tl.origin.y - insp.origin.y);
    assert!(
        left_tl.origin.x + left_tl.size.width < insp.origin.x + px(1.0),
        "时间轴在属性左边"
    );
    // 音频创作：时间轴更高，素材栏切到音频
    pick(cx, "layout-audio");
    let audio_tl = bounds(cx, "timeline");
    assert!(
        audio_tl.size.height > default_timeline.size.height * 1.3,
        "{audio_tl:?} / {default_timeline:?}"
    );
    assert!(exists(cx, &ui, "bin-audio"), "素材栏切到音频");
}

#[gpui_kit::test]
fn transitions_tab_puts_a_transition_on_the_selected_clip(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click(cx, &ui, "bin-tab-transition");
    assert!(exists(cx, &ui, "bin-transitions"));
    // 选中第二段（2–4 秒），0.5 秒叠化
    click_timeline(cx, &ui, 3.0, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "tr-dur-0.5s");
    click(cx, &ui, "tr-dissolve");
    let second = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            w.document().project.timeline.tracks[1].clips[1].transition_in
        })
    };
    let t = second(cx).unwrap();
    assert_eq!(
        (t.kind, t.duration),
        (ve_core::TransitionKind::Dissolve, Time::from_secs_f64(0.5))
    );
    // 切点处预览里有两层
    let layers = ui.view.read_with(cx, |w, _| {
        ve_core::compose::resolve_layers(&w.document().project, Time::from_secs(2)).len()
    });
    assert_eq!(layers, 2);
    click(cx, &ui, "tr-none");
    assert!(second(cx).is_none());
}

#[gpui_kit::test]
fn speed_tab_changes_clip_length(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-speed");
    assert!(exists(cx, &ui, "insp-speed"));
    click(cx, &ui, "speed-2");
    let c = first_video_clip(cx, &ui);
    assert_eq!((c.speed, c.duration), (2.0, Time::from_secs(1)));
    click(cx, &ui, "speed-0-5");
    assert_eq!(first_video_clip(cx, &ui).duration, Time::from_secs(4));
    press(cx, &ui, "ctrl-z");
    assert_eq!(first_video_clip(cx, &ui).duration, Time::from_secs(1));
    // 「声音变调」：默认关着（保持音调），点一下打开、再点关上
    assert!(first_video_clip(cx, &ui).keep_pitch);
    click(cx, &ui, "speed-pitch");
    assert!(!first_video_clip(cx, &ui).keep_pitch);
    click(cx, &ui, "speed-pitch");
    assert!(first_video_clip(cx, &ui).keep_pitch);
}

#[gpui_kit::test]
fn wheel_and_scrollbar_scroll_the_timeline_sideways(cx: &mut TestAppContext) {
    let ui = open(cx);
    frame(cx, &ui);
    // 空时间轴：不能横着滚，也没有滚动条
    cx.update_window(ui.win, |_, window, _| {
        assert!(window.try_find("timeline-hscroll").is_none());
    })
    .unwrap();
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    frame(cx, &ui);
    // 标尺上同一个位置点一下，看对应的时间，就知道滚了多少
    let ruler_time = |test: &mut TestAppContext| {
        test.update_window(ui.win, |_, window, app| {
            window.click_at("timeline", point(px(400.0), px(10.0)), app);
        })
        .unwrap();
        test.run_until_parked();
        ui.view.read_with(test, |w, _| w.engine().position())
    };
    for _ in 0..20 {
        press(cx, &ui, "ctrl-=");
    }
    // 放大会以视图中间为准，先往左滚到头
    cx.update_window(ui.win, |_, window, cx| {
        window.scroll(
            "timeline",
            ScrollDelta::Pixels(point(px(0.), px(100_000.))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let before = ruler_time(cx);
    // 只有 3 条轨、上下没得滚：普通滚轮往下滚就往右滚
    cx.update_window(ui.win, |_, window, cx| {
        window.scroll(
            "timeline",
            ScrollDelta::Pixels(point(px(0.), px(-600.))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let wheeled = ruler_time(cx);
    assert!(wheeled > before, "{before:?} → {wheeled:?}");
    // 拖滚动条的滑块往右
    let thumb = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline-hscroll-thumb").bounds()
        })
        .unwrap();
    let from = point(
        thumb.origin.x + thumb.size.width / 2.0,
        thumb.origin.y + thumb.size.height / 2.0,
    );
    cx.update_window(ui.win, |_, window, cx| {
        window.drag(from, from + point(px(200.0), px(0.0)), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let dragged = ruler_time(cx);
    assert!(dragged > wheeled, "{wheeled:?} → {dragged:?}");
    // 一直往右滚，最多滚到结尾贴着轨道头：再往回滚一点，轨道头旁边就是结尾前的内容
    for delta in [-100_000.0, 200.0] {
        cx.update_window(ui.win, |_, window, cx| {
            window.scroll(
                "timeline",
                ScrollDelta::Pixels(point(px(0.), px(delta))),
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
    }
    frame(cx, &ui);
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", point(px(133.0), px(10.0)), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let near_end = ui.view.read_with(cx, |w, _| w.engine().position());
    assert!(
        near_end < Time::from_millis(1990) && near_end > Time::from_secs(1),
        "滚过头了：{near_end:?}"
    );
}

#[gpui_kit::test]
fn zoom_buttons_keep_the_playhead_in_place(cx: &mut TestAppContext) {
    let ui = open(cx);
    // 20 秒的内容（放大后能左右滚，播放头才能留在原处）
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30); 10], true);
    // 播放头放到 10 秒（50 像素 / 秒：标尺 132 + 500 处）
    let at = point(px(132.0 + 500.0), px(10.0));
    cx.update_window(ui.win, |_, window, cx| window.click_at("timeline", at, cx))
        .unwrap();
    cx.run_until_parked();
    for _ in 0..6 {
        press(cx, &ui, "ctrl-=");
    }
    frame(cx, &ui);
    // 剪映：以播放头为中心放大，播放头还在原来的位置
    cx.update_window(ui.win, |_, window, cx| window.click_at("timeline", at, cx))
        .unwrap();
    cx.run_until_parked();
    let t = ui.view.read_with(cx, |w, _| w.engine().position());
    assert!(
        (t - Time::from_secs(10)).0.abs() <= Time::from_millis(40).0,
        "{t:?}"
    );
}

#[gpui_kit::test]
fn track_header_locks_hides_and_mutes(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    let track = |app: &mut TestAppContext| {
        ui.view
            .read_with(app, |w, _| w.document().project.timeline.tracks[1].clone())
    };
    // 第 1 行是主轨（V1）：锁上后点不中、删不掉
    click(cx, &ui, "track-lock-1");
    assert!(track(cx).locked);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    assert_eq!(
        selected(cx, &ui),
        [] as [ve_core::ClipId; 0],
        "锁着的片段点不中"
    );
    press(cx, &ui, "ctrl-a");
    assert_eq!(selected(cx, &ui), [] as [ve_core::ClipId; 0]);
    press(cx, &ui, "ctrl-b");
    assert_eq!(track(cx).clips.len(), 1, "锁着的轨不切");
    // 解锁后照常
    click(cx, &ui, "track-lock-1");
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    assert_eq!(selected(cx, &ui).len(), 1);
    // 隐藏、静音：开关各自独立，能撤销
    click(cx, &ui, "track-eye-1");
    click(cx, &ui, "track-sound-1");
    let both = track(cx);
    assert!(both.hidden && both.muted && !both.locked);
    click(cx, &ui, "tb-undo");
    let undone = track(cx);
    assert!(undone.hidden && !undone.muted);
    // 字幕轨没有静音按钮，唯一的视频轨没有删除按钮
    assert!(exists(cx, &ui, "track-eye-0"));
    assert!(!exists(cx, &ui, "track-sound-0"));
    assert!(!exists(cx, &ui, "track-del-1"));
}

#[gpui_kit::test]
fn fullscreen_preview_shows_only_the_picture(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    assert!(exists(cx, &ui, "transport"));
    press(cx, &ui, "ctrl-shift-f");
    assert!(exists(cx, &ui, "preview"), "画面还在");
    assert!(!exists(cx, &ui, "transport"), "底栏、时间轴、面板都不画");
    assert!(!exists(cx, &ui, "timeline"));
    // 空格照常播放 / 暂停
    press(cx, &ui, "space");
    assert!(ui.view.read_with(cx, |w, _| w.engine().is_playing()));
    press(cx, &ui, "space");
    // Esc 退出全屏，不动选择
    press(cx, &ui, "escape");
    assert!(exists(cx, &ui, "transport") && exists(cx, &ui, "timeline"));
    // 底栏的全屏按钮
    click(cx, &ui, "tr-fullscreen");
    assert!(!exists(cx, &ui, "transport"));
    press(cx, &ui, "ctrl-shift-f");
    assert!(exists(cx, &ui, "transport"));
}

#[gpui_kit::test]
fn toolbar_mirrors_rotates_and_fits_the_timeline(cx: &mut TestAppContext) {
    let ui = open(cx);
    // 20 秒的内容
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30); 10], true);
    let first = |app: &mut TestAppContext| {
        ui.view.read_with(app, |w, _| {
            let clip = &w.document().project.timeline.tracks[1].clips[0];
            (clip.transform.mirror, clip.transform.rotation)
        })
    };
    // 没选中时点了没反应
    click(cx, &ui, "tb-mirror");
    assert_eq!(first(cx), (false, 0.0));
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "tb-mirror");
    click(cx, &ui, "tb-rotate");
    click(cx, &ui, "tb-rotate");
    assert_eq!(first(cx), (true, 180.0));
    click(cx, &ui, "tb-mirror");
    assert_eq!(first(cx), (false, 180.0), "再点一次取消镜像");
    // Shift+Z：20 秒占轨道区的九成，点九成处就是结尾
    press(cx, &ui, "shift-z");
    frame(cx, &ui);
    let width = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().size.width
        })
        .unwrap();
    let x = 132.0 + 0.9 * (f32::from(width) - 132.0);
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", point(px(x), px(10.0)), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let t = ui.view.read_with(cx, |w, _| w.engine().position());
    assert!(
        (t - Time::from_secs(20)).0.abs() <= Time::from_millis(100).0,
        "{t:?}"
    );
    assert!(exists(cx, &ui, "tb-zoom-slider"));
}

#[gpui_kit::test]
fn freeze_inserts_three_seconds_of_the_current_frame(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let freeze_dir = dir.path().join("freeze");
    ui.view
        .update(cx, |w, _| w.use_freeze_dir(freeze_dir.clone()));
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    // 没选中：提示，不改
    seek_to(cx, &ui, Time::from_secs(1));
    press(cx, &ui, "ctrl-shift-a");
    click(cx, &ui, "tb-freeze");
    let spans = |app: &mut TestAppContext| {
        ui.view.read_with(app, |w, _| {
            w.document().project.timeline.tracks[1]
                .clips
                .iter()
                .map(|c| (c.start, c.end()))
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(spans(cx).len(), 1);
    // 选中后定格：0–1 | 定格 1–4 | 4–5
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    seek_to(cx, &ui, Time::from_secs(1));
    click(cx, &ui, "tb-freeze");
    wait(cx, &ui, |w| {
        w.document().project.timeline.tracks[1].clips.len() == 3
    });
    let s = Time::from_secs;
    assert_eq!(spans(cx), [(s(0), s(1)), (s(1), s(4)), (s(4), s(5))]);
    let (still, selected_mid) = ui.view.read_with(cx, |w, _| {
        let doc = w.document();
        let mid = &doc.project.timeline.tracks[1].clips[1];
        let asset = &doc.project.assets[&mid.asset().unwrap()];
        (
            asset.is_still()
                && asset.path.starts_with(&freeze_dir)
                && asset.name == "定格 · frame_indexed_30.mp4",
            doc.selection() == [mid.id],
        )
    });
    assert!(still, "定格是存在定格目录里的图片");
    assert!(selected_mid, "定格片段被选中");
    // 一步撤销回到原样
    click(cx, &ui, "tb-undo");
    assert_eq!(spans(cx), [(s(0), s(2))]);
}

#[gpui_kit::test]
fn audio_tab_extracts_sound_and_filter_tab_applies_presets(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::VideoWithAudio)], true);
    // 音频标签：视频文件也列出来，「+」只放声音到音频轨
    click(cx, &ui, "bin-tab-audio");
    let id = ui.view.read_with(cx, |w, _| {
        *w.document().project.assets.keys().next().unwrap()
    });
    let (row, add): (&'static str, &'static str) = (
        format!("audio-{}", id.0).leak(),
        format!("add-audio-{}", id.0).leak(),
    );
    assert!(exists(cx, &ui, row));
    click(cx, &ui, add);
    let audio_clips = ui.view.read_with(cx, |w, _| {
        let tl = &w.document().project.timeline;
        tl.tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .map(|t| t.clips.len())
            .sum::<usize>()
    });
    assert_eq!(audio_clips, 1);
    // 滤镜：没选中视频片段时不改；选中后套上「黑白」，再点「原图」去掉
    click(cx, &ui, "bin-tab-filter");
    click_timeline(cx, &ui, 0.2, 1.0, gpui_kit::Modifiers::default());
    let adjust = |app: &mut TestAppContext| {
        ui.view.read_with(app, |w, _| {
            w.document().project.timeline.tracks[1].clips[0].adjust
        })
    };
    click(cx, &ui, "filter-11");
    assert_eq!(adjust(cx), ve_core::FILTERS[11].adjust);
    click(cx, &ui, "filter-0");
    assert!(adjust(cx).is_identity());
}

#[gpui_kit::test]
fn custom_adjustment_adds_an_adjustment_layer_at_the_playhead(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    seek_to(cx, &ui, Time::from_millis(500));
    click(cx, &ui, "bin-tab-adjust");
    click(cx, &ui, "bin-adjust-custom");
    let (adjustment, span) = ui.view.read_with(cx, |w, _| {
        let doc = w.document();
        let id = doc.selection()[0];
        let clip = doc.project.timeline.find_clip(id).unwrap().1;
        (clip.is_adjustment(), (clip.start, clip.end()))
    });
    assert!(adjustment, "新加的调节片段被选中");
    assert_eq!(span, (Time::from_millis(500), Time::from_millis(3500)));
    // 属性面板只有「调整」，镜像 / 定格这些对它不可用；滤镜可以套
    assert!(exists(cx, &ui, "insp-adjust"));
    assert!(!exists(cx, &ui, "insp-tab-picture"));
    click(cx, &ui, "bin-tab-filter");
    click(cx, &ui, "filter-11");
    let colored = ui.view.read_with(cx, |w, _| {
        let doc = w.document();
        let id = doc.selection()[0];
        doc.project.timeline.find_clip(id).unwrap().1.adjust
    });
    assert_eq!(colored, ve_core::FILTERS[11].adjust);
}

#[gpui_kit::test]
fn shortcuts_popup_opens_and_replaces_the_menu(cx: &mut TestAppContext) {
    let ui = open(cx);
    click(cx, &ui, "tb-menu");
    assert!(exists(cx, &ui, "app-menu"));
    // 同时只开一个弹出框
    click(cx, &ui, "tb-shortcuts");
    assert!(exists(cx, &ui, "shortcuts"));
    assert!(!exists(cx, &ui, "app-menu"));
    click(cx, &ui, "tb-shortcuts");
    assert!(!exists(cx, &ui, "shortcuts"), "再点一次收起");
}

#[gpui_kit::test]
fn number_fields_step_with_the_plus_and_minus_buttons(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    assert!(exists(cx, &ui, "tf-scale-slider"));
    let x = |app: &mut TestAppContext| {
        ui.view.read_with(app, |w, _| {
            w.document().project.timeline.tracks[1].clips[0].transform.x
        })
    };
    // X 框右端是「+」、左端是「−」：每格 1%，立即生效
    let size = cx
        .update_window(ui.win, |_, window, _| window.find("tf-x").bounds().size)
        .unwrap();
    let mid = size.height / 2.0;
    for at in [size.width - px(6.0), size.width - px(6.0), px(6.0)] {
        cx.update_window(ui.win, |_, window, cx| {
            window.click_at("tf-x", point(at, mid), cx);
        })
        .unwrap();
        cx.run_until_parked();
        frame(cx, &ui);
    }
    assert!((x(cx) - 0.01).abs() < 1e-6, "{}", x(cx));
    // 一格一步撤销
    click(cx, &ui, "tb-undo");
    assert!((x(cx) - 0.02).abs() < 1e-6, "{}", x(cx));
}

#[gpui_kit::test]
fn subtitle_animation_tab_sets_intro_outro_and_applies_to_all(cx: &mut TestAppContext) {
    let ui = open(cx);
    // 两条字幕
    press(cx, &ui, "t");
    seek_to(cx, &ui, Time::from_secs(5));
    press(cx, &ui, "t");
    click(cx, &ui, "insp-tab-animation");
    click(cx, &ui, "text-in-2");
    click(cx, &ui, "text-out-1");
    let motions = |app: &mut TestAppContext| {
        ui.view.read_with(app, |w, _| {
            w.document().project.timeline.tracks[0]
                .clips
                .iter()
                .map(|c| c.text_motion().unwrap())
                .collect::<Vec<_>>()
        })
    };
    let want = ve_core::TextMotion {
        intro: ve_core::TextIn::SlideUp,
        outro: ve_core::TextOut::FadeOut,
        ..ve_core::TextMotion::default()
    };
    assert_eq!(
        motions(cx),
        [ve_core::TextMotion::default(), want],
        "只改选中的那条"
    );
    click(cx, &ui, "text-motion-all");
    assert_eq!(motions(cx), [want, want]);
}

#[gpui_kit::test]
fn adjust_and_volume_rows_have_their_own_keyframes(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-adjust");
    seek_to(cx, &ui, Time::ZERO);
    click(cx, &ui, "tf-key-saturation");
    let keys = |app: &mut TestAppContext, prop: ve_core::Prop| {
        first_video_clip(app, &ui).animation.keys(prop).len()
    };
    assert_eq!(keys(cx, ve_core::Prop::Saturation), 1, "只给饱和度打了一个");
    assert_eq!(keys(cx, ve_core::Prop::Brightness), 0);
    // 换个时刻拖饱和度：自动在那一刻打关键帧
    seek_to(cx, &ui, Time::from_millis(500));
    let bounds = cx
        .update_window(ui.win, |_, window, _| {
            window.find("adj-saturation").bounds()
        })
        .unwrap();
    let y = bounds.origin.y + bounds.size.height / 2.0;
    let from = point(bounds.origin.x + bounds.size.width / 2.0, y);
    let to = point(bounds.origin.x + px(1.0), y);
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let c = first_video_clip(cx, &ui);
    assert_eq!(c.animation.keys(ve_core::Prop::Saturation).len(), 2);
    assert!(
        c.adjust_at(Time::ZERO).saturation.abs() < 1e-6,
        "0 秒还是不调"
    );
    assert!(c.adjust_at(Time::from_millis(500)).saturation < -0.9);
    // 音量那一行也有 ◇
    click(cx, &ui, "insp-tab-audio");
    click(cx, &ui, "tf-key-volume");
    assert_eq!(keys(cx, ve_core::Prop::Volume), 1);
    // 再点一次删掉
    click(cx, &ui, "tf-key-volume");
    assert_eq!(keys(cx, ve_core::Prop::Volume), 0);
}

fn range(cx: &TestAppContext, ui: &Ui) -> Option<(Time, Time)> {
    ui.view
        .read_with(cx, |w, _| w.document().project.timeline.range)
}

#[gpui_kit::test]
fn in_out_points_mark_a_range_that_can_be_played_and_cleared(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    seek_to(cx, &ui, Time::from_millis(500));
    press(cx, &ui, "i");
    assert_eq!(
        range(cx, &ui),
        Some((Time::from_millis(500), Time::from_secs(2)))
    );
    seek_to(cx, &ui, Time::from_millis(1500));
    press(cx, &ui, "o");
    assert_eq!(
        range(cx, &ui),
        Some((Time::from_millis(500), Time::from_millis(1500)))
    );
    // 一步撤销
    press(cx, &ui, "ctrl-z");
    assert_eq!(
        range(cx, &ui),
        Some((Time::from_millis(500), Time::from_secs(2)))
    );
    press(cx, &ui, "ctrl-y");
    // Shift+Enter：从入点开始播
    press(cx, &ui, "shift-enter");
    assert!(ui.view.read_with(cx, |w, _| w.engine().is_playing()));
    assert!(ui.view.read_with(cx, |w, _| w.engine().position()) >= Time::from_millis(500));
    press(cx, &ui, "k");
    press(cx, &ui, "alt-x");
    assert_eq!(range(cx, &ui), None);
    // Shift+X：以选中的片段选定区域
    press(cx, &ui, "ctrl-a");
    press(cx, &ui, "shift-x");
    assert_eq!(range(cx, &ui), Some((Time::ZERO, Time::from_secs(2))));
    assert!(exists(cx, &ui, "timeline"));
}

fn clip_label(cx: &TestAppContext, ui: &Ui, id: ve_core::ClipId) -> Option<u8> {
    ui.view.read_with(cx, |w, _| {
        w.document().project.timeline.find_clip(id).unwrap().1.label
    })
}

#[gpui_kit::test]
fn context_menu_labels_clips_and_selects_same_color(cx: &mut TestAppContext) {
    let ui = open(cx);
    let clips = three_clips(cx, &ui);
    // 第一段和第三段标成绿色
    for secs in [0.5, 3.0] {
        right_click_timeline(cx, &ui, secs, 1.0);
        assert!(exists(cx, &ui, "cm-label-3"));
        click(cx, &ui, "cm-label-3");
    }
    assert_eq!(clip_label(cx, &ui, clips[0]), Some(3));
    assert_eq!(clip_label(cx, &ui, clips[1]), None);
    assert_eq!(clip_label(cx, &ui, clips[2]), Some(3));
    // 选中同色片段
    right_click_timeline(cx, &ui, 0.5, 1.0);
    click(cx, &ui, "cm-same-color");
    assert_eq!(ids(selected(cx, &ui)), ids(vec![clips[0], clips[2]]));
    // 还原
    right_click_timeline(cx, &ui, 0.5, 1.0);
    click(cx, &ui, "cm-label-clear");
    assert_eq!(clip_label(cx, &ui, clips[0]), None);
}

#[gpui_kit::test]
fn replace_clip_keeps_position_and_attributes(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    let (id, before) = ui.view.read_with(cx, |w, _| {
        let tr = w
            .document()
            .project
            .timeline
            .video_tracks_bottom_up()
            .next()
            .unwrap();
        (tr.clips[0].id, (*tr.clips[0]).clone())
    });
    right_click_timeline(cx, &ui, 0.5, 1.0);
    assert!(exists(cx, &ui, "cm-replace"));
    cx.update_window(ui.win, |_, window, cx| {
        ui.view.update(cx, |w, cx| {
            w.replace_clip_with(id, fixture(Fixture::VideoWithAudio), window, cx);
        });
    })
    .unwrap();
    wait(&mut *cx, &ui, |w| {
        w.document()
            .project
            .timeline
            .find_clip(id)
            .unwrap()
            .1
            .asset()
            != before.asset()
    });
    let after = ui.view.read_with(cx, |w, _| {
        (**w.document().project.timeline.find_clip(id).unwrap().1).clone()
    });
    assert_eq!(
        (after.start, after.duration),
        (before.start, before.duration)
    );
    assert_eq!(after.transform, before.transform);
    // 一步撤销回到原素材
    press(cx, &ui, "ctrl-z");
    let undone = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .find_clip(id)
            .unwrap()
            .1
            .asset()
    });
    assert_eq!(undone, before.asset());
}

#[gpui_kit::test]
fn export_dialog_also_writes_audio_and_cover(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("成片.mp4");
    open_export_dialog(cx, &ui, &out);
    for id in ["export-audio", "afmt-wav", "cover-first", "loud-14"] {
        cx.update_window(ui.win, |_, window, cx| window.click(id, cx))
            .unwrap();
        cx.run_until_parked();
        frame(cx, &ui);
    }
    cx.update_window(ui.win, |_, window, cx| window.click("export-confirm", cx))
        .unwrap();
    cx.run_until_parked();
    wait(&mut *cx, &ui, |w| !w.is_exporting());
    assert!(out.exists());
    let wav = dir.path().join("成片.wav");
    let probe = ve_media::probe(&wav).unwrap();
    assert!(probe.video.is_none() && probe.audio.is_some());
    assert!(dir.path().join("成片_封面.png").exists());
}

#[gpui_kit::test]
fn quality_menu_switches_preview_resolution(cx: &mut TestAppContext) {
    let ui = open(cx);
    let max = |app: &TestAppContext| {
        ui.view
            .read_with(app, |w, _| w.engine().preview_max().max_width)
    };
    assert_eq!(max(cx), 3840, "默认原画质");
    click(cx, &ui, "tr-quality");
    click(cx, &ui, "quality-smooth");
    assert_eq!(max(cx), 1280);
    assert!(!exists(cx, &ui, "quality-menu"));
}

#[gpui_kit::test]
fn property_groups_fold_and_blend_check_resets_opacity(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    assert!(exists(cx, &ui, "tf-scale"));
    click(cx, &ui, "tf-title");
    assert!(!exists(cx, &ui, "tf-scale"), "折叠「位置大小」");
    click(cx, &ui, "tf-title");
    assert!(exists(cx, &ui, "tf-scale"));
    // 改了不透明度，「混合」自动勾上；取消勾选回到 100%（「混合」在可见范围下面，先折起「位置大小」）
    click(cx, &ui, "tf-title");
    let bounds = cx
        .update_window(ui.win, |_, window, _| window.find("tf-opacity").bounds())
        .unwrap();
    let y = bounds.origin.y + bounds.size.height / 2.0;
    cx.update_window(ui.win, |_, window, cx| {
        window.drag(
            point(bounds.origin.x + bounds.size.width - px(2.0), y),
            point(bounds.origin.x + bounds.size.width / 2.0, y),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    assert!(first_video_transform(cx, &ui).opacity < 0.9);
    click(cx, &ui, "tf-blend-check");
    assert!((first_video_transform(cx, &ui).opacity - 1.0).abs() < 1e-6);
}

#[gpui_kit::test]
fn dragging_sideways_on_a_number_box_changes_the_value(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    let bounds = cx
        .update_window(ui.win, |_, window, _| window.find("tf-rotation").bounds())
        .unwrap();
    let from = bounds.center();
    let to = point(from.x + px(30.0), from.y);
    press_and_move(cx, &ui, from, to);
    release(cx, &ui, to);
    let rot = first_video_transform(cx, &ui).rotation;
    assert!((rot - 30.0).abs() < 1.0, "拖 30 像素 = 30°，实际 {rot}");
    // 整次拖动一步撤销
    press(cx, &ui, "ctrl-z");
    assert!(first_video_transform(cx, &ui).rotation.abs() < 1e-6);
}

fn first_video_look(cx: &TestAppContext, ui: &Ui) -> ve_core::Look {
    first_video_clip(cx, ui).look
}

#[gpui_kit::test]
fn picture_segments_set_blend_stroke_mask_and_chroma(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    // 基础：混合模式（「混合」组在面板下面，先折起「位置大小」）
    click(cx, &ui, "tf-title");
    click(cx, &ui, "blend-4");
    assert_eq!(
        first_video_look(cx, &ui).blend,
        ve_core::BlendMode::Multiply
    );
    // 取消勾选「混合」：回到正常
    click(cx, &ui, "tf-blend-check");
    assert_eq!(first_video_look(cx, &ui).blend, ve_core::BlendMode::Normal);
    // 描边：勾上、换颜色
    click(cx, &ui, "look-stroke-check");
    assert!(first_video_look(cx, &ui).stroke.is_some());
    click(cx, &ui, "stroke-color-2");
    assert_eq!(first_video_look(cx, &ui).stroke.unwrap().color, 0xE5_484D);
    press(cx, &ui, "ctrl-z");
    assert_eq!(
        first_video_look(cx, &ui).stroke.unwrap().color,
        0xFF_FFFF,
        "一步撤销"
    );
    // 蒙版
    click(cx, &ui, "seg-mask");
    assert!(exists(cx, &ui, "insp-mask"));
    click(cx, &ui, "mask-2");
    assert_eq!(
        first_video_look(cx, &ui).mask.map(|m| m.shape),
        Some(ve_core::MaskShape::Circle)
    );
    click(cx, &ui, "mask-invert");
    assert!(first_video_look(cx, &ui).mask.unwrap().invert);
    assert!(exists(cx, &ui, "look-mask-feather"));
    click(cx, &ui, "mask-none");
    assert_eq!(first_video_look(cx, &ui).mask, None);
    // 抠像：勾上色度抠图，换蓝幕，取色器在播放器上点一下取色
    click(cx, &ui, "seg-key");
    click(cx, &ui, "look-key-check");
    assert_eq!(first_video_look(cx, &ui).chroma.unwrap().color, 0x00_FF00);
    click(cx, &ui, "key-blue");
    assert_eq!(first_video_look(cx, &ui).chroma.unwrap().color, 0x00_00FF);
    click(cx, &ui, "key-pick");
    wait(&mut *cx, &ui, Workspace::showing_frame);
    let at = preview_center(cx, &ui);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("preview").bounds().origin
        })
        .unwrap();
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("preview", at - origin, cx);
    })
    .unwrap();
    cx.run_until_parked();
    wait(&mut *cx, &ui, |w| {
        w.document()
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .any(|c| c.look.chroma.is_some_and(|k| k.color != 0x00_00FF))
    });
    let picked = first_video_look(cx, &ui).chroma.unwrap().color;
    let [r, g, b] = [picked >> 16, (picked >> 8) & 0xFF, picked & 0xFF];
    assert!(r == g && g == b, "灰度素材取到的是灰色：{picked:06X}");
    assert_eq!(selected(cx, &ui).len(), 1, "取色那一下没有改选择");
}

#[gpui_kit::test]
fn blade_mode_splits_where_clicked(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    press(cx, &ui, "b");
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    let spans = video_spans(cx, &ui);
    assert_eq!(spans.len(), 2, "点一下切成两段：{spans:?}");
    let cut = spans[0].1;
    assert!(
        (cut.as_secs_f64() - 0.5).abs() < 0.05,
        "切在点的地方：{cut:?}"
    );
    // 回到选择模式：点一下只是选中
    press(cx, &ui, "a");
    click_timeline(cx, &ui, 1.5, 1.0, gpui_kit::Modifiers::default());
    assert_eq!(video_spans(cx, &ui).len(), 2);
    // 工具栏菜单也能切
    click(cx, &ui, "tb-tool");
    click(cx, &ui, "tool-blade");
    click_timeline(cx, &ui, 1.5, 1.0, gpui_kit::Modifiers::default());
    assert_eq!(video_spans(cx, &ui).len(), 3);
}

#[gpui_kit::test]
fn preview_axis_shows_the_hovered_frame_without_moving_the_playhead(cx: &mut TestAppContext) {
    use gpui_kit::{InputEvent as _, MouseMoveEvent};
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    seek_to(cx, &ui, Time::from_millis(200));
    press(cx, &ui, "s");
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    let at = origin + timeline_point(1.5, 1.0);
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
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    let (engine_at, playhead) = ui
        .view
        .read_with(cx, |w, _| (w.engine().position(), w.playhead()));
    assert!(
        (engine_at.as_secs_f64() - 1.5).abs() < 0.05,
        "播放器在鼠标那一刻：{engine_at:?}"
    );
    assert_eq!(playhead, Time::from_millis(200), "播放头不动");
    // 关掉预览轴：回到播放头
    press(cx, &ui, "s");
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.engine().position()),
        Time::from_millis(200)
    );
}

#[gpui_kit::test]
fn reverse_plays_the_clip_backwards_and_toggles_back(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let rev_dir = dir.path().to_path_buf();
    ui.view.update(cx, |w, _| w.use_reverse_dir(rev_dir));
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    let original = first_video_clip(cx, &ui);
    click(cx, &ui, "tb-reverse");
    wait(&mut *cx, &ui, |w| !w.is_reversing());
    let reversed = first_video_clip(cx, &ui);
    assert_ne!(reversed.asset(), original.asset(), "换成了倒放的文件");
    assert_eq!(
        reversed.reversed,
        Some((original.asset().unwrap(), original.source_in()))
    );
    assert_eq!(
        (reversed.start, reversed.duration),
        (original.start, original.duration)
    );
    // 片段开头放的是原片的最后一帧
    let first = ui.view.read_with(cx, |w, _| {
        let p = &w.document().project;
        p.assets[&reversed.asset().unwrap()].path.clone()
    });
    let mut d = ve_media::VideoDecoder::open(
        &first,
        ve_media::ScaleTarget {
            max_width: 64,
            max_height: 36,
        },
    )
    .unwrap();
    let f = d.next_frame().unwrap().unwrap();
    let idx =
        ve_media::testing::frame_index_from_gray(f64::from(f.data[((18 * 64 + 32) * 4) as usize]));
    assert!(idx >= 57, "倒放文件第一帧是原来的第 {idx} 帧");
    // 再点一次：换回原来的
    click(cx, &ui, "tb-reverse");
    assert_eq!(first_video_clip(cx, &ui), original);
}

#[gpui_kit::test]
fn curve_speed_presets_and_custom_points(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-speed");
    click(cx, &ui, "speed-seg-curve");
    click(cx, &ui, "curve-hero");
    let c = first_video_clip(cx, &ui);
    assert_eq!(c.curve, Some(ve_core::CurvePreset::Hero.curve()));
    // 自定义：五个滑条，拖第一个点
    click(cx, &ui, "curve-custom");
    assert!(exists(cx, &ui, "curve-point-0"));
    let bounds = cx
        .update_window(ui.win, |_, window, _| window.find("curve-point-0").bounds())
        .unwrap();
    let y = bounds.origin.y + bounds.size.height / 2.0;
    cx.update_window(ui.win, |_, window, cx| {
        window.drag(
            point(bounds.origin.x + bounds.size.width / 2.0, y),
            point(bounds.origin.x + bounds.size.width - px(1.0), y),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let custom = first_video_clip(cx, &ui);
    let first = custom.curve.as_ref().unwrap().speed_at(0.0);
    assert!(first > 5.0, "第一个点拖到最右接近 10x：{first}");
    // 回到「无」：去掉曲线
    click(cx, &ui, "curve-none");
    assert!(first_video_clip(cx, &ui).curve.is_none());
}

#[gpui_kit::test]
fn a_finished_recording_lands_at_the_start_on_a_free_audio_track(cx: &mut TestAppContext) {
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
    let wav = dir.path().join("rec.wav");
    let samples: Vec<f32> = (0..48_000u16)
        .map(|i| (f32::from(i) * 0.05).sin() * 0.3)
        .collect();
    ve_engine::write_wav(&wav, &samples, 48_000, 2).unwrap();
    let audio_tracks = |app: &TestAppContext| track_count(app, &ui, TrackKind::Audio);
    let before = audio_tracks(cx);
    cx.update_window(ui.win, |_, window, cx| {
        ui.view.update(cx, |w, cx| {
            w.place_recording(wav, Time::from_millis(500), window, cx);
        });
    })
    .unwrap();
    wait(&mut *cx, &ui, |w| w.document().project.assets.len() == 3);
    assert_eq!(
        audio_tracks(cx),
        before + 1,
        "原来的音频轨在 0.5 秒有声音：新建一条"
    );
    let placed = ui.view.read_with(cx, |w, _| {
        let tl = &w.document().project.timeline;
        tl.tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .flat_map(|t| t.clips.iter())
            .find(|c| c.start == Time::from_millis(500))
            .map(|c| c.duration)
    });
    let len = placed.expect("录音放在 0.5 秒");
    assert!(
        (len - Time::from_millis(500)).0.abs() <= Time::from_millis(30).0,
        "{len:?}"
    );
    assert!(exists(cx, &ui, "tb-record"));
}

#[gpui_kit::test]
fn main_track_cover_is_used_by_export(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("带封面.mp4");
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    seek_to(cx, &ui, Time::from_secs(1));
    click(cx, &ui, "track-cover");
    click(cx, &ui, "cover-use-frame");
    let cover = ui
        .view
        .read_with(cx, |w, _| w.document().project.timeline.cover.clone());
    assert_eq!(cover, Some(ve_core::Cover::Frame(Time::from_secs(1))));
    // 导出对话框默认带上草稿封面
    press(cx, &ui, "ctrl-e");
    cx.simulate_new_path_selection(move |_| Some(out));
    cx.run_until_parked();
    frame(cx, &ui);
    assert!(exists(cx, &ui, "cover-draft"));
    cx.update_window(ui.win, |_, window, cx| window.click("export-confirm", cx))
        .unwrap();
    cx.run_until_parked();
    wait(&mut *cx, &ui, |w| !w.is_exporting());
    let png = dir.path().join("带封面_封面.png");
    assert!(png.exists());
    // 封面是第 1 秒那一帧（第 30 帧）
    let mut d = ve_media::VideoDecoder::open(
        &png,
        ve_media::ScaleTarget {
            max_width: 64,
            max_height: 36,
        },
    )
    .unwrap();
    let f = d.next_frame().unwrap().unwrap();
    let idx =
        ve_media::testing::frame_index_from_gray(f64::from(f.data[((18 * 64 + 32) * 4) as usize]));
    assert!((29..=31).contains(&idx), "封面是第 {idx} 帧");
    // 清除
    click(cx, &ui, "track-cover");
    click(cx, &ui, "cover-clear");
    assert!(
        ui.view
            .read_with(cx, |w, _| w.document().project.timeline.cover.is_none())
    );
}

#[gpui_kit::test]
fn advanced_grading_hsl_curves_wheels_and_lut(cx: &mut TestAppContext) {
    use std::fmt::Write as _;
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-adjust");
    let grade = |app: &TestAppContext| first_video_clip(app, &ui).grade.as_deref().cloned();
    let drag_right = |app: &mut TestAppContext, id: &'static str| {
        let b = app
            .update_window(ui.win, |_, window, _| window.find(id).bounds())
            .unwrap();
        let y = b.origin.y + b.size.height / 2.0;
        app.update_window(ui.win, |_, window, inner| {
            window.drag(
                point(b.origin.x + b.size.width / 2.0, y),
                point(b.origin.x + b.size.width - px(1.0), y),
                inner,
            );
        })
        .unwrap();
        app.run_until_parked();
    };
    // HSL：选绿色，饱和度拉满
    click(cx, &ui, "adj-seg-hsl");
    click(cx, &ui, "hsl-3");
    drag_right(cx, "hsl-sat");
    let g = grade(cx).unwrap();
    assert!(
        g.hsl[ve_core::HslBand::Green.index()].saturation > 0.9,
        "{g:?}"
    );
    // 一次拖动一步撤销
    press(cx, &ui, "ctrl-z");
    assert!(grade(cx).is_none());
    // 曲线：点一下加一个点
    click(cx, &ui, "adj-seg-curves");
    let b = cx
        .update_window(ui.win, |_, window, _| window.find("curve-editor").bounds())
        .unwrap();
    let at = point(
        b.origin.x + b.size.width / 2.0,
        b.origin.y + b.size.height / 4.0,
    );
    press_and_move(cx, &ui, at, at);
    release(cx, &ui, at);
    let pts = grade(cx).unwrap().curves.luma;
    assert_eq!(pts.len(), 1, "{pts:?}");
    assert!(
        (pts[0].0 - 0.5).abs() < 0.02 && (pts[0].1 - 0.75).abs() < 0.02,
        "{pts:?}"
    );
    // 色轮：暗部往右拖（偏红；第一排，在面板可见范围里）
    click(cx, &ui, "adj-seg-wheels");
    let wheel = cx
        .update_window(ui.win, |_, window, _| window.find("wheel-shadows").bounds())
        .unwrap();
    let c = wheel.center();
    press_and_move(cx, &ui, c, point(c.x + wheel.size.width / 2.0, c.y));
    release(cx, &ui, point(c.x + wheel.size.width / 2.0, c.y));
    let shadows = grade(cx).unwrap().wheels.shadows;
    assert!(
        shadows[0] > shadows[1] && shadows[0] > shadows[2],
        "{shadows:?}"
    );
    // LUT
    let dir = tempfile::tempdir().unwrap();
    let cube = dir.path().join("x.cube");
    let mut text = String::from("LUT_3D_SIZE 2\n");
    for blue in 0..2 {
        for green in 0..2 {
            for red in 0..2 {
                let _ = writeln!(text, "{red} {green} {blue}");
            }
        }
    }
    std::fs::write(&cube, text).unwrap();
    click(cx, &ui, "adj-seg-basic");
    cx.update_window(ui.win, |_, window, cx| {
        ui.view
            .update(cx, |w, cx| w.use_lut(cube.clone(), window, cx));
    })
    .unwrap();
    assert_eq!(grade(cx).unwrap().lut.map(|l| l.path), Some(cube));
    assert!(exists(cx, &ui, "lut-intensity"));
}

#[gpui_kit::test]
fn subtitle_own_style_position_keyframes_and_loops(cx: &mut TestAppContext) {
    let ui = open(cx);
    press(cx, &ui, "t");
    let sub = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            (*w.document().project.timeline.tracks[0].clips[0]).clone()
        })
    };
    click(cx, &ui, "insp-tab-text");
    // 关掉「应用到全部字幕」：改字号只改这一条
    click(cx, &ui, "ts-apply-all");
    assert!(sub(cx).own_subtitle_style().is_some());
    let shared = ui
        .view
        .read_with(cx, |w, _| w.document().project.subtitle_style.clone());
    let big = cx
        .update_window(ui.win, |_, window, _| {
            window.try_find("ts-size-xl").is_some()
        })
        .unwrap();
    assert!(big);
    click(cx, &ui, "ts-size-xl");
    let own = sub(cx).own_subtitle_style().cloned().unwrap();
    assert_ne!(own.size_ratio, shared.size_ratio, "这一条变了");
    assert_eq!(
        ui.view
            .read_with(cx, |w, _| w.document().project.subtitle_style.clone()),
        shared,
        "共用的样式没变"
    );
    // 位置大小：字幕也有，能打关键帧（在「文本」标签下面，先往下滚）
    assert!(exists(cx, &ui, "tf-y"));
    cx.update_window(ui.win, |_, window, cx| {
        window.scroll(
            "inspector-body",
            ScrollDelta::Pixels(point(px(0.), px(-600.))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    click(cx, &ui, "tf-key-pos");
    assert_eq!(sub(cx).animation.keys(ve_core::Prop::X).len(), 1);
    // 循环动画
    click(cx, &ui, "insp-tab-animation");
    click(cx, &ui, "text-loop-1");
    assert_eq!(
        sub(cx).text_motion().unwrap().looping,
        ve_core::TextLoop::Breathe
    );
    click(cx, &ui, "text-in-4");
    assert_eq!(
        sub(cx).text_motion().unwrap().intro,
        ve_core::TextIn::Typewriter
    );
}
