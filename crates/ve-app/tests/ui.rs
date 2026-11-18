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

/// 时间轴里某轨某时刻的局部坐标（默认几何：轨道头 72、标尺 28、轨高 56、50 像素 / 秒）。
/// `track` 是屏幕上的行号：最上面是字幕轨（第 0 行），下面依次是视频轨、音频轨。
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
    press(cx, &ui, "s");
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
        codec: ve_export::Codec::H264,
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
    let to = origin + point(px(72.0 + 2.0 * 50.0), px(28.0 + 56.0 + 2.0));
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
