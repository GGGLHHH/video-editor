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
    _watchdog: Watchdog,
}

/// 单个测试卡住时别让整轮测试一直挂着：超过时限就报出测试名并直接结束进程（CI 上也能看到是哪个）。
/// 只用一个早早建好的巡查线程、超时用 `abort`：卡死时往往连新线程都起不来、正常退出也会卡住。
struct Watchdog(u64);

type Running = std::collections::HashMap<u64, (String, std::time::Instant)>;

impl Watchdog {
    const LIMIT: Duration = Duration::from_secs(120);

    fn running() -> &'static std::sync::Mutex<Running> {
        static RUNNING: std::sync::OnceLock<std::sync::Mutex<Running>> = std::sync::OnceLock::new();
        RUNNING.get_or_init(|| {
            std::thread::spawn(|| {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let late = Self::running()
                        .lock()
                        .ok()
                        .and_then(|m| m.values().find(|(_, t)| t.elapsed() > Self::LIMIT).cloned());
                    if let Some((name, _)) = late {
                        #[expect(
                            clippy::print_stderr,
                            reason = "进程马上结束，只能这样留下是哪个测试"
                        )]
                        {
                            eprintln!(
                                "界面测试 {name} 超过 {} 秒没结束，结束进程",
                                Self::LIMIT.as_secs()
                            );
                        }
                        std::process::abort();
                    }
                }
            });
            std::sync::Mutex::new(Running::new())
        })
    }

    fn start() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = std::thread::current()
            .name()
            .unwrap_or("（无名测试）")
            .to_owned();
        if let Ok(mut m) = Self::running().lock() {
            m.insert(id, (name, std::time::Instant::now()));
        }
        Self(id)
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        if let Ok(mut m) = Self::running().lock() {
            m.remove(&self.0);
        }
    }
}

fn open(cx: &mut TestAppContext) -> Ui {
    // 警告以上的日志：测试失败时和失败信息一起打出来（后台任务失败的原因，比如 ffmpeg 的退出码）
    drop(
        env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Warn)
            .try_init(),
    );
    let watchdog = Watchdog::start();
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
    let ui = Ui {
        win,
        view,
        _watchdog: watchdog,
    };
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
fn the_top_bar_is_the_window_title_bar_with_its_own_controls(cx: &mut TestAppContext) {
    let ui = open(cx);
    for id in ["title-bar", "toolbar", "minimize", "close"] {
        assert!(exists(cx, &ui, id), "{id}");
    }
    // 顶栏的按钮照样能点（菜单）
    click(cx, &ui, "tb-menu");
    assert!(exists(cx, &ui, "menu-home"));
}

#[gpui_kit::test]
fn show_argument_opens_a_panel_for_screenshots(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    for (name, id) in [
        ("animation", "insp-motion"),
        ("mask", "insp-mask"),
        ("hsl", "insp-hsl"),
        ("speed", "insp-speed"),
        ("bin-effect", "bin-effects"),
        ("bin-sticker,guides", "preview-guides"),
        ("adjust,bin-adjust", "bin-adjust"),
        ("crop", "crop-1-1"),
    ] {
        cx.update_window(ui.win, |_, window, wcx| {
            ui.view
                .update(wcx, |w, vcx| w.show_panel(name, window, vcx));
        })
        .unwrap();
        cx.run_until_parked();
        assert!(exists(cx, &ui, id), "{name}");
    }
}

#[gpui_kit::test]
fn the_first_run_guide_shows_once(cx: &mut TestAppContext) {
    let ui = open(cx);
    let seen = |app: &TestAppContext| ui.view.read_with(app, |w, _| w.settings().seen_guide);
    assert!(!seen(cx));
    cx.update_window(ui.win, |_, window, wcx| {
        ui.view
            .update(wcx, |w, vcx| w.show_guide_if_new(window, vcx));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(exists(cx, &ui, "guide"));
    click(cx, &ui, "guide-ok");
    assert!(seen(cx), "看过了就记下来");
    assert!(!exists(cx, &ui, "guide"));
    // 再来一次不弹
    cx.update_window(ui.win, |_, window, wcx| {
        ui.view
            .update(wcx, |w, vcx| w.show_guide_if_new(window, vcx));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!exists(cx, &ui, "guide"));
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
            "level-meter",
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
fn dragging_files_onto_the_window_imports_them(cx: &mut TestAppContext) {
    use gpui_kit::{ExternalPaths, FileDropEvent, InputEvent as _};
    let ui = open(cx);
    let position = point(px(400.), px(300.));
    let paths = ExternalPaths(vec![fixture(Fixture::VideoWithAudio)].into_iter().collect());
    cx.update_window(ui.win, |_, window, cx| {
        window.dispatch_event(
            FileDropEvent::Entered { position, paths }.to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
    })
    .unwrap();
    wait(&mut *cx, &ui, |w| w.document().project.assets.len() == 1);
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
fn adding_text_where_there_already_is_some_stacks_it_on_a_new_text_track(cx: &mut TestAppContext) {
    let ui = open(cx);
    press(cx, &ui, "t");
    press(cx, &ui, "t");
    let texts = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Subtitle)
            .map(|t| t.clips.len())
            .collect::<Vec<_>>()
    });
    // 上面新开的一条放标题，下面原来的字幕轨放字幕
    assert_eq!(texts, [1, 1]);
    // 一步撤销：标题和新开的文字轨一起没了
    press(cx, &ui, "ctrl-z");
    let tracks = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Subtitle)
            .count()
    });
    assert_eq!(tracks, 1);
}

#[gpui_kit::test]
fn global_settings_set_the_still_length_and_clean_the_proxy_cache(cx: &mut TestAppContext) {
    let ui = open(cx);
    let cache = tempfile::tempdir().unwrap();
    std::fs::write(cache.path().join("0123456789abcdef.mp4"), [0u8; 2048]).unwrap();
    std::fs::write(cache.path().join("留着.txt"), "x").unwrap();
    ui.view.update(cx, |w, cx| {
        let s = ve_app::settings::AppSettings {
            cache_dir: Some(cache.path().to_path_buf()),
            ..ve_app::settings::AppSettings::default()
        };
        w.use_settings(s, None, cx);
    });
    click(cx, &ui, "tb-menu");
    click(cx, &ui, "menu-settings");
    frame(cx, &ui);
    assert!(exists(cx, &ui, "settings-form"));
    // 代理缓存 2 KB：清理只删代理文件
    click(cx, &ui, "settings-clear-cache");
    assert!(!cache.path().join("0123456789abcdef.mp4").exists());
    assert!(cache.path().join("留着.txt").exists());
    // 图片默认 8 秒，保存
    click(cx, &ui, "settings-still-8");
    click(cx, &ui, "settings-save");
    assert_eq!(ui.view.read_with(cx, |w, _| w.settings().still_secs), 8);
    import(cx, &ui, vec![fixture(Fixture::Image)], false);
    let len = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .assets
            .values()
            .next()
            .unwrap()
            .duration
    });
    assert_eq!(len, Time::from_secs(8));
}

#[gpui_kit::test]
fn the_default_frame_rate_does_not_change_an_opened_project(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("25帧.vep");
    let mut p = ve_core::Project::new();
    p.settings.frame_rate = ve_core::FrameRate::new(25, 1);
    ve_core::save_project(&p, &path).unwrap();
    cx.update_window(ui.win, |_, window, cx| {
        ui.view.update(cx, |w, cx| w.open_path(&path, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    ui.view.update(cx, |w, cx| {
        w.use_settings(ve_app::settings::AppSettings::default(), None, cx);
    });
    let rate = ui
        .view
        .read_with(cx, |w, _| w.document().project.settings.frame_rate);
    assert_eq!(rate, ve_core::FrameRate::new(25, 1), "打开的工程不改帧率");
}

/// 选中的复合片段（克隆一份）。
fn selected_compound(cx: &TestAppContext, ui: &Ui) -> ve_core::Clip {
    ui.view.read_with(cx, |w, _| {
        let doc = w.document();
        let c = doc
            .project
            .timeline
            .find_clip(doc.selection()[0])
            .unwrap()
            .1;
        assert!(c.compound().is_some());
        (**c).clone()
    })
}

#[gpui_kit::test]
fn a_compound_has_animation_and_adjust_tabs_like_a_video(cx: &mut TestAppContext) {
    let ui = open(cx);
    magnet_off(cx, &ui);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    press(cx, &ui, "ctrl-a");
    press(cx, &ui, "alt-g");
    // 「画面」标签：原来的复合片段面板（进入、解除、位置大小）
    click(cx, &ui, "insp-tab-picture");
    assert!(exists(cx, &ui, "insp-enter-compound") && exists(cx, &ui, "tf-scale"));
    // 「动画」：整组渐显
    click(cx, &ui, "insp-tab-animation");
    click(cx, &ui, "anim-in-fade");
    let faded = selected_compound(cx, &ui);
    assert_eq!(faded.transform_at(Time::ZERO).opacity, 0.0);
    // 「调整」：整组降饱和度
    click(cx, &ui, "insp-tab-adjust");
    type_into(cx, &ui, "adj-saturation-value", "-40");
    let c = selected_compound(cx, &ui);
    assert!(
        (c.adjust.saturation + 0.4).abs() < 1e-4,
        "{}",
        c.adjust.saturation
    );
    // 合成时里面每一层都套上了整组的调色
    let flat = ui.view.read_with(cx, |w, _| {
        ve_core::compound::flatten(&w.document().project).unwrap()
    });
    let comp = ve_core::compose::resolve_frame(&flat, Time::from_secs(1));
    assert_ne!(comp.layers.len(), 0);
    assert!(comp.layers.iter().all(|l| !l.color.is_identity()));
}

#[gpui_kit::test]
fn a_compound_takes_a_mask_and_round_corners_as_a_whole(cx: &mut TestAppContext) {
    let ui = open(cx);
    magnet_off(cx, &ui);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    press(cx, &ui, "ctrl-a");
    press(cx, &ui, "alt-g");
    click(cx, &ui, "insp-tab-picture");
    // 复合片段的画面分页没有「抠像」
    assert!(exists(cx, &ui, "seg-mask") && !exists(cx, &ui, "seg-key"));
    // 圆角在位置大小下面，要往下滚才看得到
    cx.update_window(ui.win, |_, window, wcx| {
        window.scroll(
            "inspector-body",
            ScrollDelta::Pixels(point(px(0.), px(-300.))),
            wcx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    type_into(cx, &ui, "look-radius-value", "40");
    assert!(selected_compound(cx, &ui).look.radius > 0.3);
    cx.update_window(ui.win, |_, window, wcx| {
        window.scroll(
            "inspector-body",
            ScrollDelta::Pixels(point(px(0.), px(300.))),
            wcx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    click(cx, &ui, "seg-mask");
    click(cx, &ui, "mask-2");
    let c = selected_compound(cx, &ui);
    assert_eq!(
        c.look.mask.map(|m| m.shape),
        Some(ve_core::MaskShape::Circle)
    );
    // 合成时整组按它裁
    let flat = ui.view.read_with(cx, |w, _| {
        ve_core::compound::flatten(&w.document().project).unwrap()
    });
    let frame = ve_core::compose::resolve_frame(&flat, Time::from_secs(1));
    assert!(frame.groups.iter().any(|g| g.shape.is_some()));
    // 回到「基础」还是复合片段的面板
    click(cx, &ui, "seg-basic");
    assert!(exists(cx, &ui, "insp-enter-compound"));
}

#[gpui_kit::test]
fn a_compound_can_be_sped_up_as_a_whole(cx: &mut TestAppContext) {
    let ui = open(cx);
    magnet_off(cx, &ui);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    press(cx, &ui, "ctrl-a");
    press(cx, &ui, "alt-g");
    let before = selected_compound(cx, &ui).duration;
    click(cx, &ui, "insp-tab-speed");
    // 复合片段只有常规变速
    assert!(!exists(cx, &ui, "speed-seg-curve"));
    click(cx, &ui, "speed-2");
    let c = selected_compound(cx, &ui);
    assert!((c.speed - 2.0).abs() < 1e-4);
    assert_eq!(c.duration, Time(before.0 / 2));
    // 「声音变调」管整组
    click(cx, &ui, "speed-pitch");
    assert!(!selected_compound(cx, &ui).keep_pitch);
    // 一步撤销回到原速
    click(cx, &ui, "tb-undo");
    click(cx, &ui, "tb-undo");
    assert_eq!(selected_compound(cx, &ui).duration, before);
}

#[gpui_kit::test]
fn entering_a_compound_edits_whats_inside_and_writes_it_back(cx: &mut TestAppContext) {
    let ui = open(cx);
    magnet_off(cx, &ui);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    press(cx, &ui, "ctrl-a");
    press(cx, &ui, "alt-g");
    let inside = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            let p = &w.document().project;
            p.timeline
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .find_map(|c| {
                    c.compound()
                        .map(|(inner, _)| inner.tracks.iter().map(|t| t.clips.len()).sum::<usize>())
                })
        })
    };
    assert_eq!(inside(cx), Some(2));
    // 复合片段的裁剪不生效：面板里不给改
    assert!(exists(cx, &ui, "tf-scale") && exists(cx, &ui, "tf-rotation"));
    assert!(!exists(cx, &ui, "tf-crop-l"));
    // 整组缩到 60% 再撤销：面板跟着回到 100（面板按复合片段自己的属性填）
    type_into(cx, &ui, "tf-scale", "60");
    click(cx, &ui, "tb-undo");
    assert_eq!(
        ui.view
            .read_with(cx, |w, _| w.transform_field_texts()[2].clone()),
        "100"
    );
    // 播放器上拖：整组一起挪（复合片段算一个画布大小的图层）
    frame(cx, &ui);
    let from = preview_center(cx, &ui);
    cx.update_window(ui.win, |_, window, cx| {
        window.drag(from, from + point(px(40.0), px(0.0)), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let moved = ui.view.read_with(cx, |w, _| {
        let doc = w.document();
        let id = doc.selection()[0];
        let c = doc.project.timeline.find_clip(id).unwrap().1;
        (c.compound().is_some(), c.transform.x)
    });
    assert!(moved.0 && moved.1 > 0.01, "{moved:?}");
    click(cx, &ui, "tb-undo");
    click(cx, &ui, "insp-enter-compound");
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.compound_path()),
        ["复合片段1"]
    );
    assert!(exists(cx, &ui, "compound-back"));
    // 里面：两段视频，在 1 秒处切开第一段
    assert_eq!(video_clips(cx, &ui).len(), 2);
    seek_to(cx, &ui, Time::from_secs(1));
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    press(cx, &ui, "ctrl-b");
    assert_eq!(video_clips(cx, &ui).len(), 3);
    click(cx, &ui, "compound-back");
    assert!(ui.view.read_with(cx, |w, _| w.compound_path().is_empty()));
    assert_eq!(inside(cx), Some(3), "改过的写回了复合片段");
    // 外面一步撤销
    press(cx, &ui, "ctrl-z");
    assert_eq!(inside(cx), Some(2));
}

#[gpui_kit::test]
fn work_inside_a_compound_is_autosaved_exported_and_locked_ones_stay_closed(
    cx: &mut TestAppContext,
) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let drafts = dir.path().join("drafts");
    cx.update_window(ui.win, |_, window, cx| {
        ui.view.update(cx, |w, cx| {
            w.use_recent_file(dir.path().join("recent.json"));
            w.enable_autosave(drafts.clone(), window, cx);
        });
    })
    .unwrap();
    magnet_off(cx, &ui);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    press(cx, &ui, "ctrl-a");
    press(cx, &ui, "alt-g");
    // 复合片段后面再接一段（在外面）：里面和整个工程的时间线不一样
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-enter-compound");
    seek_to(cx, &ui, Time::from_secs(1));
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    press(cx, &ui, "ctrl-b");
    // 还在里面：停手后自动保存存的是整个工程（复合片段里已经是 3 段）
    for _ in 0..10 {
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
    }
    assert_eq!(ui.view.read_with(cx, |w, _| w.compound_path().len()), 1);
    let saved = std::fs::read_dir(&drafts)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "vep"))
        .expect("自动保存了");
    let loaded = ve_core::project_file::load_project(&saved).unwrap().project;
    let inner = loaded
        .timeline
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .find_map(|c| {
            c.compound()
                .map(|(i, _)| i.tracks.iter().map(|t| t.clips.len()).sum::<usize>())
        });
    assert_eq!(inner, Some(3), "存的是整个工程，里面的改动也在");
    // 在里面导出 EDL：导出的是整个工程，和回到外面再导出一模一样
    let export = |app: &mut TestAppContext, name: &str| {
        let path = dir.path().join(name);
        ui.view
            .read_with(app, |w, _| {
                w.export_interchange(ve_app::interchange_ui::Interchange::Edl, &path)
            })
            .unwrap();
        std::fs::read_to_string(&path).unwrap()
    };
    let inside = export(cx, "inside.edl");
    click(cx, &ui, "compound-back");
    assert_eq!(inside, export(cx, "outside.edl"));
    // 锁住的轨道上的复合片段：不让进（进去改了也写不回去）
    let compound = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .find(|c| c.compound().is_some())
            .unwrap()
            .id
    });
    click(cx, &ui, "track-lock-1");
    cx.update_window(ui.win, |_, window, cx| {
        ui.view
            .update(cx, |w, cx| w.enter_compound(compound, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(ui.view.read_with(cx, |w, _| w.compound_path().is_empty()));
}

#[gpui_kit::test]
fn jianying_navigation_and_panel_shortcuts(cx: &mut TestAppContext) {
    use ve_app::inspector::InspectorTab;
    use ve_app::media_bin::BinTab;
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    let end = ui
        .view
        .read_with(cx, |w, _| w.document().project.timeline.duration());
    let half = Time(end.0 / 2);
    seek_to(cx, &ui, Time::ZERO);
    // ↓ / ↑：下一个 / 上一个分割点（片段的头尾）
    press(cx, &ui, "down");
    assert_eq!(position(cx, &ui), half);
    press(cx, &ui, "down");
    assert_eq!(position(cx, &ui), end);
    press(cx, &ui, "up");
    assert_eq!(position(cx, &ui), half);
    // Shift+→ / ←：一次 10 帧
    seek_to(cx, &ui, Time::ZERO);
    press(cx, &ui, "shift-right");
    let ten = ui.view.read_with(cx, |w, _| {
        w.document().project.settings.frame_rate.time_of_frame(10)
    });
    assert_eq!(position(cx, &ui), ten);
    press(cx, &ui, "shift-left");
    assert_eq!(position(cx, &ui), Time::ZERO);
    // Tab：素材面板换到下一个分类
    let tab = |app: &TestAppContext| ui.view.read_with(app, |w, _| w.bin_tab());
    assert_eq!(tab(cx), BinTab::Media);
    press(cx, &ui, "tab");
    assert_eq!(tab(cx), BinTab::ALL[1]);
    // 选中视频片段：Ctrl+R 唤起变速面板
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    press(cx, &ui, "ctrl-r");
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.inspector_tab()),
        InspectorTab::Speed
    );
    // Ctrl+. / Ctrl+,：音量 ±0.1 dB
    press(cx, &ui, "ctrl-.");
    press(cx, &ui, "ctrl-.");
    assert!((first_video_clip(cx, &ui).gain_db - 0.2).abs() < 1e-4);
    press(cx, &ui, "ctrl-,");
    assert!((first_video_clip(cx, &ui).gain_db - 0.1).abs() < 1e-4);
    // Shift+Alt+K：在播放头处给画面属性打关键帧
    seek_to(cx, &ui, Time::from_millis(500));
    press(cx, &ui, "shift-alt-k");
    assert!(!first_video_clip(cx, &ui).animation.is_empty());
}

#[gpui_kit::test]
fn quitting_saves_unsaved_work_first(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let drafts = dir.path().join("drafts");
    cx.update_window(ui.win, |_, window, cx| {
        ui.view.update(cx, |w, cx| {
            w.use_recent_file(dir.path().join("recent.json"));
            w.enable_autosave(drafts.clone(), window, cx);
        });
    })
    .unwrap();
    press(cx, &ui, "t");
    // 刚改完就退出（还没到自动保存的时候）：先存
    ui.view.update(cx, Workspace::save_before_quit);
    let saved = std::fs::read_dir(&drafts).map_or(0, Iterator::count);
    assert_eq!(saved, 1);
    assert!(!ui.view.read_with(cx, |w, _| w.document().is_dirty()));
}

#[gpui_kit::test]
fn the_menu_has_clipboard_help_and_quit(cx: &mut TestAppContext) {
    let ui = open(cx);
    click(cx, &ui, "tb-menu");
    for id in [
        "menu-copy",
        "menu-cut",
        "menu-paste",
        "menu-copy-attrs",
        "menu-paste-attrs",
        "menu-shortcuts",
        "menu-about",
        "menu-quit",
    ] {
        assert!(exists(cx, &ui, id), "菜单里有 {id}");
    }
    // 帮助 › 快捷键：打开快捷键一览
    click(cx, &ui, "menu-shortcuts");
    assert!(exists(cx, &ui, "shortcuts"));
    press(cx, &ui, "escape");
    // 帮助 › 关于：显示版本号
    click(cx, &ui, "tb-menu");
    click(cx, &ui, "menu-about");
    frame(cx, &ui);
    assert!(exists(cx, &ui, "about-version"));
}

#[gpui_kit::test]
fn adjust_has_highlights_shadows_light_sharpen_grain_and_vignette(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-adjust");
    let type_in = |app: &mut TestAppContext, id: &'static str, text: &str| {
        click(app, &ui, id);
        app.update_window(ui.win, |_, window, wcx| {
            window.press("ctrl-a", wcx);
            window.input(text, wcx);
            window.press("enter", wcx);
        })
        .unwrap();
        app.run_until_parked();
    };
    // 色温、色调的滑条下面有冷暖 / 绿品红的渐变条
    assert!(exists(cx, &ui, "adj-temperature-gradient"));
    assert!(exists(cx, &ui, "adj-tint-gradient"));
    type_in(cx, "adj-shadows-value", "40");
    // 下面几项要往下滚才看得到
    cx.update_window(ui.win, |_, window, wcx| {
        window.scroll(
            "inspector-body",
            ScrollDelta::Pixels(point(px(0.), px(-300.))),
            wcx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    type_in(cx, "adj-vignette-value", "-30");
    type_in(cx, "adj-grain-value", "500");
    let a = first_video_clip(cx, &ui).adjust;
    assert!((a.shadows - 0.4).abs() < 1e-4, "{a:?}");
    assert!((a.vignette + 0.3).abs() < 1e-4, "{a:?}");
    assert!((a.grain - 1.0).abs() < 1e-4, "颗粒夹到 100 {a:?}");
    // 改阴影不会把别的几项冲掉
    type_in(cx, "adj-shadows-value", "45");
    assert!((first_video_clip(cx, &ui).adjust.vignette + 0.3).abs() < 1e-4);
    // 滚回上面，重置全部归零；撤销一步回来
    cx.update_window(ui.win, |_, window, wcx| {
        window.scroll(
            "inspector-body",
            ScrollDelta::Pixels(point(px(0.), px(600.))),
            wcx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    click(cx, &ui, "adj-reset");
    assert!(first_video_clip(cx, &ui).adjust.is_identity());
    press(cx, &ui, "ctrl-z");
    assert!((first_video_clip(cx, &ui).adjust.shadows - 0.45).abs() < 1e-4);
}

#[gpui_kit::test]
fn hsl_rows_have_keyframe_buttons(cx: &mut TestAppContext) {
    use ve_core::{HslBand, HslChannel, Prop};
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-adjust");
    click(cx, &ui, "adj-seg-hsl");
    let sat = Prop::hsl(HslBand::Red, HslChannel::Saturation);
    // 第 0 秒在「饱和度」那行打一帧
    seek_to(cx, &ui, Time::ZERO);
    click(cx, &ui, "tf-key-hsl-sat");
    assert_eq!(first_video_clip(cx, &ui).animation.keys(sat).len(), 1);
    // 第 1 秒输入 -80：自动在这里再打一帧，中间渐变
    seek_to(cx, &ui, Time::from_secs(1));
    click(cx, &ui, "hsl-sat-value");
    cx.update_window(ui.win, |_, window, wcx| {
        window.press("ctrl-a", wcx);
        window.input("-80", wcx);
        window.press("enter", wcx);
    })
    .unwrap();
    cx.run_until_parked();
    let c = first_video_clip(cx, &ui);
    assert_eq!(c.animation.keys(sat).len(), 2);
    let half = c.grade_at(Time::from_millis(500)).unwrap().hsl[0].saturation;
    assert!((half + 0.4).abs() < 0.02, "{half}");
    // 别的颜色、别的行没有关键帧
    assert_eq!(
        c.animation
            .keys(Prop::hsl(HslBand::Red, HslChannel::Hue))
            .len(),
        0
    );
}

#[gpui_kit::test]
fn the_extra_adjust_rows_take_keyframes(cx: &mut TestAppContext) {
    use ve_core::Prop;
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-adjust");
    cx.update_window(ui.win, |_, window, wcx| {
        window.scroll(
            "inspector-body",
            ScrollDelta::Pixels(point(px(0.), px(-300.))),
            wcx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    // 第 0 秒在「颗粒」那行打一帧
    seek_to(cx, &ui, Time::ZERO);
    click(cx, &ui, "tf-key-grain");
    assert_eq!(
        first_video_clip(cx, &ui).animation.keys(Prop::Grain).len(),
        1
    );
    // 第 1 秒输入 80：自动在这里再打一帧，中间渐变
    seek_to(cx, &ui, Time::from_secs(1));
    click(cx, &ui, "adj-grain-value");
    cx.update_window(ui.win, |_, window, wcx| {
        window.press("ctrl-a", wcx);
        window.input("80", wcx);
        window.press("enter", wcx);
    })
    .unwrap();
    cx.run_until_parked();
    let c = first_video_clip(cx, &ui);
    assert_eq!(c.animation.keys(Prop::Grain).len(), 2);
    let half = c.adjust_at(Time::from_millis(500)).grain;
    assert!((half - 0.4).abs() < 0.02, "{half}");
    assert_eq!(c.animation.keys(Prop::Vignette).len(), 0);
}

#[gpui_kit::test]
fn the_group_title_keys_every_adjust_item_at_once(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-adjust");
    seek_to(cx, &ui, Time::from_millis(500));
    click(cx, &ui, "tf-key-adjust-all");
    let c = first_video_clip(cx, &ui);
    for p in ve_core::Prop::ADJUST_ALL {
        assert_eq!(c.animation.keys(p).len(), 1, "{p:?}");
    }
    // 再点一次：这一组在这一刻的关键帧都删掉
    click(cx, &ui, "tf-key-adjust-all");
    let cleared = first_video_clip(cx, &ui);
    assert!(
        ve_core::Prop::ADJUST_ALL
            .iter()
            .all(|p| cleared.animation.keys(*p).is_empty())
    );
}

#[gpui_kit::test]
fn apply_to_all_copies_the_adjustments_to_every_video_clip(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(
        cx,
        &ui,
        vec![
            fixture(Fixture::FrameIndexed30),
            fixture(Fixture::FrameIndexed30),
        ],
        true,
    );
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-adjust");
    type_into(cx, &ui, "adj-saturation-value", "-40");
    click(cx, &ui, "adj-apply-all");
    let sats = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .track_of_kind(TrackKind::Video)
            .unwrap()
            .clips
            .iter()
            .map(|c| c.adjust.saturation)
            .collect::<Vec<_>>()
    });
    assert_eq!(sats.len(), 2);
    assert!(sats.iter().all(|s| (s + 0.4).abs() < 1e-4), "{sats:?}");
    press(cx, &ui, "ctrl-z");
    let second = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .timeline
            .track_of_kind(TrackKind::Video)
            .unwrap()
            .clips[1]
            .adjust
            .saturation
    });
    assert!(second.abs() < 1e-6, "一步撤销");
}

#[gpui_kit::test]
fn slider_values_can_be_typed_in(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-adjust");
    // 点饱和度右边的数字，输入 -50 回车
    click(cx, &ui, "adj-saturation-value");
    cx.update_window(ui.win, |_, window, cx| {
        window.press("ctrl-a", cx);
        window.input("-50", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let sat = |app: &mut TestAppContext| first_video_clip(app, &ui).adjust.saturation;
    assert!((sat(cx) + 0.5).abs() < 1e-4, "{}", sat(cx));
    // 超出范围的夹到边上
    click(cx, &ui, "adj-saturation-value");
    cx.update_window(ui.win, |_, window, cx| {
        window.press("ctrl-a", cx);
        window.input("500", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!((sat(cx) - 1.0).abs() < 1e-4, "{}", sat(cx));
    // 一次输入一步撤销
    click(cx, &ui, "tb-undo");
    assert!((sat(cx) + 0.5).abs() < 1e-4, "{}", sat(cx));
    // 输完以后快捷键照样能用
    press(cx, &ui, "ctrl-z");
    assert!(sat(cx).abs() < 1e-4, "{}", sat(cx));
}

#[gpui_kit::test]
fn keyframe_diamonds_on_the_timeline_can_be_clicked_and_dragged(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    seek_to(cx, &ui, Time::from_millis(500));
    press(cx, &ui, "shift-alt-k");
    let times = |app: &mut TestAppContext| first_video_clip(app, &ui).animation.times();
    assert_eq!(times(cx), [Time::from_millis(500)]);
    // 点菱形（选中片段的中部）：播放头跳过去
    seek_to(cx, &ui, Time::from_millis(1500));
    let b = timeline_bounds(cx, &ui);
    let diamond = b.origin + row_point(cx, &ui, 0.5, 1);
    press_and_move(cx, &ui, diamond, diamond);
    release(cx, &ui, diamond);
    assert_eq!(position(cx, &ui), Time::from_millis(500));
    assert_eq!(times(cx), [Time::from_millis(500)], "只是点一下，不挪");
    // 拖菱形到 1 秒：关键帧跟过去，片段不动，一步撤销
    let to = b.origin + row_point(cx, &ui, 1.0, 1);
    press_and_move(cx, &ui, diamond, to);
    release(cx, &ui, to);
    assert_eq!(times(cx), [Time::from_secs(1)]);
    assert_eq!(first_video_clip(cx, &ui).start, Time::ZERO, "片段没被拖走");
    click(cx, &ui, "tb-undo");
    assert_eq!(times(cx), [Time::from_millis(500)]);
}

#[gpui_kit::test]
fn clicking_a_media_card_previews_it_and_right_click_deletes_it(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], false);
    let ids: Vec<ve_core::AssetId> = ui.view.read_with(cx, |w, _| {
        w.document().project.assets.keys().copied().collect()
    });
    let second = ids[1];
    seek_to(cx, &ui, Time::from_millis(700));
    // 单击第二个素材：播放器单独放它，时间轴播放头不动
    cx.update_window(ui.win, |_, window, cx| {
        window.click(format!("asset-{}", second.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.previewing_asset()),
        Some(second)
    );
    assert!(exists(cx, &ui, "player-previewing"));
    let engine_clips = ui.view.read_with(cx, |w, _| {
        w.engine_project()
            .timeline
            .tracks
            .iter()
            .map(|t| t.clips.len())
            .sum::<usize>()
    });
    assert_eq!(engine_clips, 1);
    // Esc 结束预览，回到时间轴原来的位置
    press(cx, &ui, "escape");
    assert_eq!(ui.view.read_with(cx, |w, _| w.previewing_asset()), None);
    assert_eq!(position(cx, &ui), Time::from_millis(700));
    // 再预览一次，点时间轴也回到时间轴
    cx.update_window(ui.win, |_, window, cx| {
        window.click(format!("asset-{}", second.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    assert_eq!(ui.view.read_with(cx, |w, _| w.previewing_asset()), None);
    // 右键第一个素材（时间轴上用着）→ 删除：片段一起删，一步撤销
    let first = ids[0];
    cx.update_window(ui.win, |_, window, cx| {
        window.right_click(format!("asset-{}", first.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    click(cx, &ui, "asset-menu-delete");
    let left = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            let p = &w.document().project;
            (p.assets.contains_key(&first), video_clip_count(p))
        })
    };
    assert_eq!(left(cx), (false, 0));
    click(cx, &ui, "tb-undo");
    assert_eq!(left(cx), (true, 1));
}

fn video_clip_count(p: &ve_core::Project) -> usize {
    p.timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .map(|t| t.clips.len())
        .sum()
}

#[gpui_kit::test]
fn audio_fades_have_handles_on_the_timeline(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::ToneStereo48k)], true);
    let row = ui
        .view
        .read_with(cx, |w, _| {
            w.document()
                .project
                .timeline
                .tracks
                .iter()
                .position(|t| t.kind == TrackKind::Audio && !t.clips.is_empty())
        })
        .expect("音频放在音频轨上");
    let fade_in = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            w.document().project.timeline.tracks[row].clips[0].fade_in
        })
    };
    let top = ui.view.read_with(cx, |w, _| w.timeline_row(row).0) + 4.0;
    let b = timeline_bounds(cx, &ui);
    // 淡入拖点在片段左上角：往右拖 25 像素（50 像素 / 秒，0.5 秒）
    let from = b.origin + point(px(132.0 + 1.0), px(top));
    let to = b.origin + point(px(132.0 + 25.0), px(top));
    press_and_move(cx, &ui, from, to);
    release(cx, &ui, to);
    assert_eq!(fade_in(cx), Time::from_millis(500));
    click(cx, &ui, "tb-undo");
    assert_eq!(fade_in(cx), Time::ZERO);
}

#[gpui_kit::test]
fn effects_have_a_speed(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click(cx, &ui, "bin-tab-effect");
    click(cx, &ui, "bin-effect-0");
    let speed = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            let doc = w.document();
            let c = doc
                .project
                .timeline
                .find_clip(doc.selection()[0])
                .unwrap()
                .1;
            c.effect().unwrap().speed
        })
    };
    assert!((speed(cx) - ve_core::Effect::DEFAULT_SPEED).abs() < 1e-6);
    // 「速度」直接输入 100：拉满
    click(cx, &ui, "effect-speed-value");
    cx.update_window(ui.win, |_, window, cx| {
        window.press("ctrl-a", cx);
        window.input("100", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!((speed(cx) - 1.0).abs() < 1e-6, "{}", speed(cx));
}

#[gpui_kit::test]
fn every_effect_card_has_a_preview_picture(cx: &mut TestAppContext) {
    let ui = open(cx);
    click(cx, &ui, "bin-tab-effect");
    for i in 0..ve_core::EffectKind::ALL.len() {
        let id = format!("bin-effect-{i}-preview");
        frame(cx, &ui);
        let shown = cx
            .update_window(ui.win, |_, window, _| window.try_find(id.clone()).is_some())
            .unwrap();
        assert!(shown, "{i}");
    }
}

#[gpui_kit::test]
fn the_sticker_tab_lists_imported_images(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(
        cx,
        &ui,
        vec![fixture(Fixture::Image), fixture(Fixture::FrameIndexed30)],
        false,
    );
    click(cx, &ui, "bin-tab-sticker");
    assert!(exists(cx, &ui, "bin-sticker"));
    let ids: Vec<(ve_core::AssetId, bool)> = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .assets
            .values()
            .map(|a| (a.id, a.video.is_some_and(|v| v.still)))
            .collect()
    });
    for (id, still) in ids {
        let shown = cx
            .update_window(ui.win, |_, window, _| {
                window.try_find(format!("asset-{}", id.0)).is_some()
            })
            .unwrap();
        assert_eq!(shown, still, "只列图片");
    }
}

#[gpui_kit::test]
fn the_media_bin_searches_by_file_name(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], false);
    import(cx, &ui, vec![fixture(Fixture::ToneStereo48k)], false);
    let cards = |app: &mut TestAppContext| {
        let ids: Vec<ve_core::AssetId> = ui.view.read_with(app, |w, _| {
            w.document().project.assets.keys().copied().collect()
        });
        app.update_window(ui.win, |_, window, _| {
            ids.iter()
                .filter(|id| window.try_find(format!("asset-{}", id.0)).is_some())
                .count()
        })
        .unwrap()
    };
    assert_eq!(cards(cx), 2);
    let name = ui.view.read_with(cx, |w, _| {
        w.document()
            .project
            .assets
            .values()
            .last()
            .unwrap()
            .name
            .clone()
    });
    let part: String = name.chars().take(4).collect();
    type_into(cx, &ui, "bin-search", &part);
    assert_eq!(cards(cx), 1, "只剩名字里有「{part}」的");
}

#[gpui_kit::test]
fn the_ratio_menu_groups_landscape_and_portrait_and_ticks_original(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click(cx, &ui, "tr-ratio");
    for id in [
        "ratio-group-landscape",
        "ratio-group-portrait",
        "ratio-9-16",
    ] {
        assert!(exists(cx, &ui, id), "{id}");
    }
    // 素材是 16:9、工程也是 16:9：「原始」打勾
    assert!(exists(cx, &ui, "ratio-original-on"));
    click(cx, &ui, "ratio-9-16");
    click(cx, &ui, "tr-ratio");
    assert!(
        !exists(cx, &ui, "ratio-original-on"),
        "换成竖屏后不是原始了"
    );
}

#[gpui_kit::test]
fn ctrl_semicolon_toggles_reference_lines_on_the_player(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    frame(cx, &ui);
    assert!(!exists(cx, &ui, "preview-guides"));
    press(cx, &ui, "ctrl-;");
    assert!(exists(cx, &ui, "preview-guides"));
    assert!(
        exists(cx, &ui, "preview-ruler-top") && exists(cx, &ui, "preview-ruler-left"),
        "标尺"
    );
    press(cx, &ui, "ctrl-;");
    assert!(!exists(cx, &ui, "preview-guides"));
}

#[gpui_kit::test]
fn preview_zooms_in_and_out_with_shortcuts(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    frame(cx, &ui);
    let rect = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            let r = w.video_rect();
            (f32::from(r.size.width), f32::from(r.center().x))
        })
    };
    let zoom = |app: &TestAppContext| ui.view.read_with(app, |w, _| w.preview_zoom());
    let (fit_w, fit_cx) = rect(cx);
    assert!(fit_w > 0.0);
    assert!(!exists(cx, &ui, "preview-zoom"), "没放大时不显示比例");
    press(cx, &ui, "alt-shift-=");
    assert!((zoom(cx) - 1.5).abs() < 1e-6);
    let (w, center) = rect(cx);
    assert!((w - fit_w * 1.5).abs() < 1.0, "{w} vs {fit_w}");
    assert!((center - fit_cx).abs() < 1.0, "围着中心放大");
    assert!(exists(cx, &ui, "preview-zoom"));
    press(cx, &ui, "alt-shift--");
    press(cx, &ui, "alt-shift--");
    assert!((zoom(cx) - 0.75).abs() < 1e-6);
    // 一路缩到最小就停
    for _ in 0..10 {
        press(cx, &ui, "alt-shift--");
    }
    assert!((zoom(cx) - 0.25).abs() < 1e-6);
    press(cx, &ui, "alt-shift-z");
    assert!((zoom(cx) - 1.0).abs() < 1e-6);
    assert!((rect(cx).0 - fit_w).abs() < 1e-3);
}

#[gpui_kit::test]
fn track_height_grows_and_shrinks_with_shortcuts(cx: &mut TestAppContext) {
    let ui = open(cx);
    let height = |app: &TestAppContext| ui.view.read_with(app, |w, _| w.track_height());
    let start = height(cx);
    press(cx, &ui, "ctrl-alt-=");
    assert!(height(cx) > start);
    press(cx, &ui, "ctrl-alt--");
    press(cx, &ui, "ctrl-alt--");
    assert!(height(cx) < start);
    // 有上下限
    for _ in 0..20 {
        press(cx, &ui, "ctrl-alt--");
    }
    let low = height(cx);
    press(cx, &ui, "ctrl-alt--");
    assert!((height(cx) - low).abs() < f32::EPSILON && low > 20.0);
}

#[gpui_kit::test]
fn ctrl_shift_s_separates_and_restores_audio(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::VideoWithAudio)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    let audio_clips = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            w.document()
                .project
                .timeline
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Audio)
                .map(|t| t.clips.len())
                .sum::<usize>()
        })
    };
    assert_eq!(audio_clips(cx), 0);
    press(cx, &ui, "ctrl-shift-s");
    assert_eq!(audio_clips(cx), 1, "声音分到了音频轨上");
    assert!(first_video_clip(cx, &ui).audio_split);
    // 再按一次：还原
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    press(cx, &ui, "ctrl-shift-s");
    assert_eq!(audio_clips(cx), 0);
    assert!(!first_video_clip(cx, &ui).audio_split);
    // 右键菜单里也有
    right_click_timeline(cx, &ui, 0.5, 1.0);
    assert!(exists(cx, &ui, "cm-separate-audio"));
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

/// 时间轴视图在窗口里的位置和大小。
fn timeline_bounds(cx: &mut TestAppContext, ui: &Ui) -> Bounds<Pixels> {
    cx.update_window(ui.win, |_, window, _| window.find("timeline").bounds())
        .unwrap()
}

#[gpui_kit::test]
fn playback_pages_the_timeline_along_with_the_playhead(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    // 2 秒放大到 2000 像素：放到一半多播放头就走出视图了
    ui.view.update(cx, |w, _| w.set_timeline_zoom(1000.0));
    frame(cx, &ui);
    assert!(ui.view.read_with(cx, |w, _| w.timeline_scroll_x()) < f32::EPSILON);
    ui.view.read_with(cx, |w, _| w.engine().play());
    let mut paged = false;
    for _ in 0..400 {
        cx.executor().advance_clock(Duration::from_millis(20));
        cx.run_until_parked();
        frame(cx, &ui);
        if ui.view.read_with(cx, |w, _| w.timeline_scroll_x()) > 0.0 {
            paged = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(paged, "播放头走出视图后时间轴要翻页");
    // 翻页后播放头还在看得见的地方
    let (x, width) = ui.view.read_with(cx, |w, _| {
        (w.timeline_x_of(w.engine().position()), w.timeline_width())
    });
    assert!(x < width, "{x} / {width}");
    ui.view.read_with(cx, |w, _| w.engine().pause());
}

#[gpui_kit::test]
fn dragging_a_clip_against_the_edge_scrolls_the_timeline(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    magnet_off(cx, &ui);
    ui.view.update(cx, |w, _| w.set_timeline_zoom(400.0));
    frame(cx, &ui);
    let b = timeline_bounds(cx, &ui);
    let from = b.origin + point(px(132.0 + 0.5 * 400.0), px(28.0 + 56.0 + 28.0));
    // 按住片段拖到右边缘，鼠标不动
    let edge = point(b.origin.x + b.size.width - px(4.0), from.y);
    press_and_move(cx, &ui, from, edge);
    let start_at_edge = first_video_clip(cx, &ui).start;
    for _ in 0..30 {
        frame(cx, &ui);
    }
    assert!(
        ui.view.read_with(cx, |w, _| w.timeline_scroll_x()) > 0.0,
        "贴边自动滚"
    );
    let later = first_video_clip(cx, &ui).start;
    assert!(
        later > start_at_edge,
        "滚的同时片段接着往后挪：{start_at_edge:?} → {later:?}"
    );
    release(cx, &ui, edge);
}

#[gpui_kit::test]
fn pressing_a_clip_near_the_edge_without_moving_does_not_scroll_or_move_it(
    cx: &mut TestAppContext,
) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    magnet_off(cx, &ui);
    ui.view.update(cx, |w, _| w.set_timeline_zoom(1600.0));
    frame(cx, &ui);
    let b = timeline_bounds(cx, &ui);
    // 片段伸出视图右边：在离右边 20 像素处按下，只想选中它，鼠标不动
    let edge = point(
        b.origin.x + b.size.width - px(20.0),
        b.origin.y + px(28.0 + 56.0 + 28.0),
    );
    press_and_move(cx, &ui, edge, edge);
    for _ in 0..30 {
        frame(cx, &ui);
    }
    release(cx, &ui, edge);
    assert_eq!(ui.view.read_with(cx, |w, _| w.timeline_scroll_x()), 0.0);
    assert_eq!(first_video_clip(cx, &ui).start, Time::ZERO, "片段没被挪走");
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.document().selection().len()),
        1,
        "选中了"
    );
}

#[gpui_kit::test]
fn dragging_a_media_card_shows_where_it_lands_and_drops_it_there(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], false);
    magnet_off(cx, &ui);
    let id = ui.view.read_with(cx, |w, _| {
        *w.document().project.assets.keys().next().unwrap()
    });
    let card = cx
        .update_window(ui.win, |_, window, _| {
            window.find(format!("asset-{}", id.0)).bounds().center()
        })
        .unwrap();
    let b = timeline_bounds(cx, &ui);
    // 视频轨 3 秒处
    let target = b.origin + timeline_point(3.0, 1.0);
    press_and_move(cx, &ui, card, target);
    // 再挪一下：拖动中时间轴上显示落点
    press_and_move(cx, &ui, target, target + point(px(1.0), px(0.0)));
    assert!(
        ui.view.read_with(cx, |w, _| w.drop_preview().is_some()),
        "拖着经过时间轴要显示落点"
    );
    release(cx, &ui, target);
    let clip = first_video_clip(cx, &ui);
    assert!(
        (clip.start - Time::from_secs(3)).0.abs() <= Time::from_millis(40).0,
        "{:?}",
        clip.start
    );
    assert!(
        ui.view.read_with(cx, |w, _| w.drop_preview().is_none()),
        "放下后预览收掉"
    );
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
fn clicking_a_blank_part_of_a_track_moves_the_playhead(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    // 片段下面空的那一行、1 秒处
    click_timeline(cx, &ui, 1.0, 3.0, gpui_kit::Modifiers::default());
    assert_eq!(position(cx, &ui), Time::from_secs(1));
    assert!(
        ui.view
            .read_with(cx, |w, _| w.document().selection().is_empty()),
        "点空白处不选片段"
    );
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
fn dragging_the_playhead_plays_sound_unless_turned_off(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::VideoWithAudio)], true);
    let scrubs = |app: &TestAppContext| ui.view.read_with(app, |w, _| w.engine().scrubs());
    let before = scrubs(cx);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    let from = origin + point(px(132.0 + 50.0), px(10.0));
    cx.update_window(ui.win, |_, window, wcx| {
        window.drag(from, from + point(px(30.0), px(0.0)), wcx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(scrubs(cx) > before, "拖标尺出声");
    assert!(
        !ui.view.read_with(cx, |w, _| w.engine().is_playing()),
        "没开始播放"
    );
    // 关掉「时间轴声音」：拖动不出声
    click(cx, &ui, "tb-scrub-audio");
    assert!(!ui.view.read_with(cx, |w, _| w.scrub_audio()));
    let off = scrubs(cx);
    cx.update_window(ui.win, |_, window, wcx| {
        window.drag(from, from + point(px(30.0), px(0.0)), wcx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(scrubs(cx), off);
}

#[gpui_kit::test]
fn home_page_lists_drafts_newest_first_and_opens_them(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    // 两个草稿：「旧的」先存、「新的」后存
    let save = |name: &str, secs_ago: u64| {
        let mut p = ve_core::Project::new();
        let path = fixture(Fixture::FrameIndexed30);
        let r = ve_media::probe(&path).unwrap();
        let id = p.add_asset(ve_core::Asset {
            id: ve_core::AssetId(0),
            name: "x".into(),
            path,
            duration: r.duration,
            video: r.video,
            audio: r.audio,
        });
        ve_core::edit::append_asset(&mut p, id).unwrap();
        let file = dir.path().join(format!("{name}.vep"));
        ve_core::project_file::save_project(&p, &file).unwrap();
        let when = std::time::SystemTime::now() - Duration::from_secs(secs_ago);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(when)
            .unwrap();
        file
    };
    let _old = save("旧的", 7200);
    let newer = save("新的", 60);
    let ui = open(cx);
    ui.view.update(cx, |w, cx| w.show_home_in(dir.path(), cx));
    cx.run_until_parked();
    assert!(exists(cx, &ui, "home"));
    assert!(!exists(cx, &ui, "timeline"), "首页盖住编辑器");
    assert!(
        exists(cx, &ui, "close"),
        "首页也有窗口按钮（没有系统标题栏）"
    );
    let cards = ui.view.read_with(cx, |w, _| w.home_cards());
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].0, newer, "新改的在前");
    assert_eq!(cards[0].1, Some(Time::from_secs(2)), "卡片上有时长");
    wait(cx, &ui, |w| w.home_covers() == 2);
    // 点卡片：打开它，回到编辑器
    click(cx, &ui, "home-draft-0");
    assert!(!exists(cx, &ui, "home"));
    let opened = ui
        .view
        .read_with(cx, |w, _| w.document().path().map(ToOwned::to_owned));
    assert_eq!(opened, Some(newer));
    // 再回首页，点「开始创作」：直接进编辑器
    ui.view.update(cx, |w, cx| w.show_home_in(dir.path(), cx));
    cx.run_until_parked();
    click(cx, &ui, "home-new");
    assert!(!exists(cx, &ui, "home"));
    assert!(exists(cx, &ui, "timeline"));
}

#[gpui_kit::test]
fn the_draft_name_in_the_top_bar_renames_the_project_file(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("旧名字.vep");
    ve_core::project_file::save_project(&ve_core::Project::new(), &old).unwrap();
    let ui = open(cx);
    cx.update_window(ui.win, |_, window, wcx| {
        ui.view.update(wcx, |w, vcx| w.open_path(&old, window, vcx));
    })
    .unwrap();
    cx.run_until_parked();
    click(cx, &ui, "project-name");
    assert!(exists(cx, &ui, "project-rename"));
    type_into(cx, &ui, "project-rename", "新/名字");
    let new = dir.path().join("新_名字.vep");
    assert!(new.is_file(), "文件改了名（放不进文件名的字符换成 _）");
    assert!(!old.exists());
    let (name, path) = ui.view.read_with(cx, |w, _| {
        (
            w.document().name(),
            w.document().path().map(ToOwned::to_owned),
        )
    });
    assert_eq!((name.as_str(), path), ("新_名字", Some(new)));
    assert!(!exists(cx, &ui, "project-rename"), "改完收起输入框");
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
        video_audio: ve_export::VideoAudio::default(),
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
    press_export(cx, ui, out);
}

/// Ctrl+E 打开导出框，导出到 `out`（导出框的文件夹和标题预先设好）。
fn press_export(cx: &mut TestAppContext, ui: &Ui, out: &std::path::Path) {
    let dir = out.parent().unwrap().to_path_buf();
    let title = out.file_stem().unwrap().to_string_lossy().into_owned();
    ui.view
        .update(cx, |w, _| w.use_export_defaults(dir, Some(title)));
    press(cx, ui, "ctrl-e");
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
fn export_opens_its_dialog_directly_with_title_folder_and_cover(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    // 标题按草稿名；同名文件已经有了就不覆盖，加 (1)
    ui.view.update(cx, |w, _| {
        w.use_export_defaults(dir.path().to_path_buf(), None);
    });
    std::fs::write(dir.path().join("未命名.mp4"), "旧的").unwrap();
    press(cx, &ui, "ctrl-e");
    cx.run_until_parked();
    frame(cx, &ui);
    assert!(!cx.has_pending_prompt(), "不先弹系统的另存为");
    assert!(exists(cx, &ui, "export-title"));
    assert!(exists(cx, &ui, "export-dir"));
    cx.update_window(ui.win, |_, window, cx| window.click("export-confirm", cx))
        .unwrap();
    cx.run_until_parked();
    wait(&mut *cx, &ui, |w| !w.is_exporting());
    assert!(dir.path().join("未命名(1).mp4").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("未命名.mp4")).unwrap(),
        "旧的"
    );
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
    press(cx, &ui, "ctrl-alt-s");
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
fn scale_can_be_non_uniform_with_the_toggle_and_edge_handles(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    frame(cx, &ui);
    assert!(exists(cx, &ui, "tf-uniform"));
    assert!(!exists(cx, &ui, "tf-scale-y"), "等比时没有单独的 Y");
    // 关掉等比：Y 单独输入 50%
    click(cx, &ui, "tf-uniform");
    click(cx, &ui, "tf-scale-y-value");
    cx.update_window(ui.win, |_, window, wcx| {
        window.press("ctrl-a", wcx);
        window.input("50", wcx);
        window.press("enter", wcx);
    })
    .unwrap();
    cx.run_until_parked();
    let t = first_video_transform(cx, &ui);
    assert!(
        (t.scale - 1.0).abs() < 1e-4 && (t.stretch - 0.5).abs() < 1e-4,
        "{t:?}"
    );
    // 打开等比：回到等比
    click(cx, &ui, "tf-uniform");
    assert!((first_video_transform(cx, &ui).stretch - 1.0).abs() < 1e-6);
    // 缩到一半，预览上把右边中点往外拖：只变宽、高不变
    type_into(cx, &ui, "tf-scale", "50");
    let (vx, vy, vw, vh) = video_rect(cx, &ui);
    let from = point(px(vx + vw * 0.75), px(vy + vh / 2.0));
    let to = from + point(px(vw * 0.125), px(0.0));
    press_and_move(cx, &ui, from, to);
    release(cx, &ui, to);
    let dragged = first_video_transform(cx, &ui);
    assert!(
        (dragged.scale - 0.75).abs() < 0.02,
        "宽变成 1.5 倍 {dragged:?}"
    );
    assert!(
        (dragged.scale * dragged.stretch - 0.5).abs() < 1e-3,
        "高不变 {dragged:?}"
    );
}

#[gpui_kit::test]
fn several_masks_can_be_added_edited_and_removed(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "seg-mask");
    let look = |app: &TestAppContext| first_video_look(app, &ui);
    // 添加蒙版：第一个是主蒙版，再加一个是另外的，自动切到它
    click(cx, &ui, "mask-add");
    click(cx, &ui, "mask-add");
    assert_eq!(look(cx).masks().count(), 2);
    assert!(exists(cx, &ui, "mask-tab-1"));
    // 改的是第二个：第一个不动
    type_into(cx, &ui, "look-mask-x-value", "30");
    let l = look(cx);
    assert!((l.extra_masks[0].unwrap().x - 0.3).abs() < 1e-4, "{l:?}");
    assert!(l.mask.unwrap().x.abs() < 1e-6, "第一个没动");
    // 切回第一个改它
    click(cx, &ui, "mask-tab-0");
    type_into(cx, &ui, "look-mask-x-value", "-20");
    let both = look(cx);
    assert!((both.mask.unwrap().x + 0.2).abs() < 1e-4);
    assert!((both.extra_masks[0].unwrap().x - 0.3).abs() < 1e-4);
    // 删掉第二个；撤销回来
    click(cx, &ui, "mask-tab-1");
    click(cx, &ui, "mask-remove");
    assert_eq!(look(cx).masks().count(), 1);
    press(cx, &ui, "ctrl-z");
    assert_eq!(look(cx).masks().count(), 2);
}

#[gpui_kit::test]
fn on_the_mask_tab_dragging_in_the_preview_moves_the_mask_not_the_clip(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "seg-mask");
    click(cx, &ui, "mask-2");
    frame(cx, &ui);
    let from = preview_center(cx, &ui);
    let to = from + point(px(40.0), px(0.0));
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let mask = first_video_look(cx, &ui).mask.unwrap();
    assert!(mask.x > 0.01 && mask.y.abs() < 1e-4, "{mask:?}");
    assert!(first_video_transform(cx, &ui).is_default(), "图层没动");
    click(cx, &ui, "tb-undo");
    assert!(
        first_video_look(cx, &ui).mask.unwrap().x.abs() < 1e-6,
        "一次拖动一步撤销"
    );
    // 回到「基础」：再拖就是拖图层
    click(cx, &ui, "seg-basic");
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    assert!(first_video_transform(cx, &ui).x > 0.01);
}

#[gpui_kit::test]
fn mask_drags_cancel_on_escape_and_misses_leave_the_layer_alone(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "seg-mask");
    click(cx, &ui, "mask-2");
    frame(cx, &ui);
    let from = preview_center(cx, &ui);
    let to = from + point(px(40.0), px(0.0));
    // 拖到一半按 Esc：蒙版回到原位，选择还在，松开不再改
    press_and_move(cx, &ui, from, to);
    assert!(first_video_look(cx, &ui).mask.unwrap().x > 0.01);
    press(cx, &ui, "escape");
    assert!(first_video_look(cx, &ui).mask.unwrap().x.abs() < 1e-6);
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.document().selection().len()),
        1
    );
    release(cx, &ui, to);
    assert!(first_video_look(cx, &ui).mask.unwrap().x.abs() < 1e-6);
    // 蒙版模式下按在蒙版外面（画面左上角）拖：图层不动
    let corner = cx
        .update_window(ui.win, |_, window, _| window.find("preview").bounds())
        .unwrap();
    let near = point(
        corner.origin.x + corner.size.width * 0.1,
        corner.origin.y + corner.size.height * 0.2,
    );
    cx.update_window(ui.win, |_, window, cx| {
        window.drag(near, near + point(px(40.0), px(0.0)), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(first_video_transform(cx, &ui).is_default(), "图层没被拖走");
}

#[gpui_kit::test]
fn a_compound_with_nothing_showing_does_not_catch_preview_clicks(cx: &mut TestAppContext) {
    let ui = open(cx);
    magnet_off(cx, &ui);
    for _ in 0..3 {
        import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    }
    // 删掉中间那段（2–4 秒）留出空档，再把剩下两段打包
    click_timeline(cx, &ui, 3.0, 1.0, gpui_kit::Modifiers::default());
    press(cx, &ui, "delete");
    press(cx, &ui, "ctrl-a");
    press(cx, &ui, "alt-g");
    press(cx, &ui, "escape");
    seek_to(cx, &ui, Time::from_secs(3));
    frame(cx, &ui);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("preview").bounds().origin
        })
        .unwrap();
    let at = preview_center(cx, &ui) - origin;
    cx.update_window(ui.win, |_, window, cx| window.click_at("preview", at, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.document().selection().len()),
        0,
        "空档里点画面，不选中复合片段"
    );
    // 有画面的地方照样能点选
    seek_to(cx, &ui, Time::from_secs(1));
    frame(cx, &ui);
    cx.update_window(ui.win, |_, window, cx| window.click_at("preview", at, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.document().selection().len()),
        1
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
    // 点时间轴空白处取消选择（播放头会跳过去，再拉回原处）
    let before = position(cx, &ui);
    cx.update_window(ui.win, |_, window, cx| {
        window.click_at("timeline", timeline_point(20.0, 3.0), cx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    seek_to(cx, &ui, before);
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
fn the_crop_button_crops_to_a_ratio(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "tb-crop");
    click(cx, &ui, "crop-1-1");
    let c = first_video_transform(cx, &ui).crop;
    assert!(
        (c.left - 0.21875).abs() < 1e-4 && (c.right - 0.21875).abs() < 1e-4,
        "{c:?}"
    );
    click(cx, &ui, "tb-crop");
    click(cx, &ui, "crop-free");
    assert_eq!(
        first_video_transform(cx, &ui).crop,
        ve_core::Crop::default()
    );
}

#[gpui_kit::test]
fn the_rotation_dial_turns_the_clip(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    frame(cx, &ui);
    let b = cx
        .update_window(ui.win, |_, window, _| {
            window.find("tf-rotate-dial").bounds()
        })
        .unwrap();
    let c = b.center();
    // 从正上方拖到正右边：转 90°（顺时针为正）
    let from = point(c.x, b.origin.y + px(1.0));
    let to = point(b.origin.x + b.size.width - px(1.0), c.y);
    press_and_move(cx, &ui, from, to);
    release(cx, &ui, to);
    let r = first_video_transform(cx, &ui).rotation;
    assert!((r - 90.0).abs() < 1.0, "{r}");
    press(cx, &ui, "ctrl-z");
    assert!(
        first_video_transform(cx, &ui).rotation.abs() < 1e-6,
        "一次拖动一步撤销"
    );
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
    assert!(
        !exists(cx, &ui, "insp-tab-audio"),
        "没声音的视频没有「音频」标签"
    );
    press(cx, &ui, "ctrl-z");
    import(cx, &ui, vec![fixture(Fixture::VideoWithAudio)], true);
    press(cx, &ui, "ctrl-a");
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
    let from = origin + row_point(cx, &ui, 0.5, 1);
    let top = ui.view.read_with(cx, |w, _| w.timeline_row(1).0);
    let to = origin + point(px(132.0 + 2.0 * 50.0), px(top + 2.0));
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

/// 时间轴第 `row` 行正中、`secs` 秒处（按真实行高；文字轨、特效轨矮一些）。
fn row_point(cx: &TestAppContext, ui: &Ui, secs: f32, row: usize) -> Point<Pixels> {
    let (top, h) = ui.view.read_with(cx, |w, _| w.timeline_row(row));
    point(px(132.0 + secs * 50.0), px(top + h / 2.0))
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
fn editing_with_the_playhead_past_the_clip_keys_its_last_frame(cx: &mut TestAppContext) {
    use ve_core::Prop;
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    seek_to(cx, &ui, Time::ZERO);
    click(cx, &ui, "tf-key-scale");
    // 播放头停在片段末尾（不在片段上）改缩放：帧打在最后一帧，不是片段外面那一点
    let end = first_video_clip(cx, &ui).end();
    seek_to(cx, &ui, end);
    type_into(cx, &ui, "tf-scale", "50");
    let c = first_video_clip(cx, &ui);
    let keys = c.animation.keys(Prop::Scale);
    assert_eq!(keys.len(), 2);
    assert!(
        keys[1].at < c.duration,
        "{:?} / {:?}",
        keys[1].at,
        c.duration
    );
    // 跳过去能用 ◆ 删掉
    click(cx, &ui, "tf-key-scale-prev");
    click(cx, &ui, "tf-key-scale");
    assert_eq!(
        first_video_clip(cx, &ui).animation.keys(Prop::Scale).len(),
        1
    );
}

#[gpui_kit::test]
fn effect_strength_keys_have_an_easing_choice(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    seek_to(cx, &ui, Time::from_millis(500));
    click(cx, &ui, "bin-tab-effect");
    click(cx, &ui, "bin-effect-0");
    // 特效片段从 0.5 秒开始：播放头在它开头，给强度打一帧
    assert!(!exists(cx, &ui, "fx-ease-linear"), "还没有关键帧时不显示");
    click(cx, &ui, "tf-key-effect");
    assert!(exists(cx, &ui, "fx-ease-linear"));
    click(cx, &ui, "fx-ease-linear");
    let easing = ui.view.read_with(cx, |w, _| {
        let doc = w.document();
        let c = doc
            .project
            .timeline
            .find_clip(doc.selection()[0])
            .unwrap()
            .1;
        c.animation.keys(ve_core::Prop::EffectAmount)[0].easing
    });
    assert_eq!(easing, ve_core::Easing::Linear);
}

#[gpui_kit::test]
fn each_row_keys_only_its_own_property(cx: &mut TestAppContext) {
    use ve_core::Prop;
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    seek_to(cx, &ui, Time::ZERO);
    // 「缩放」一行的 ◇：只有缩放（连同缩放 Y）有关键帧
    click(cx, &ui, "tf-key-scale");
    let keyed = |app: &TestAppContext| -> Vec<Prop> {
        first_video_clip(app, &ui).animation.props().collect()
    };
    assert_eq!(keyed(cx), [Prop::Scale, Prop::Stretch]);
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
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-animation");
    // 入场渐显（默认 0.5 秒）：不打关键帧，卡片是选中的
    click(cx, &ui, "anim-in-fade");
    let c = first_video_clip(cx, &ui);
    assert!(c.animation.is_empty());
    assert_eq!(c.transform_at(Time::ZERO).opacity, 0.0);
    assert_eq!(c.transform_at(Time::from_secs(1)).opacity, 1.0);
    assert!(exists(cx, &ui, "anim-dur"), "选了动画才有时长");
    // 出场：向左滑出，时长输入 1 秒
    click(cx, &ui, "anim-phase-out");
    click(cx, &ui, "anim-out-left");
    click(cx, &ui, "anim-dur-value");
    cx.update_window(ui.win, |_, window, wcx| {
        window.press("ctrl-a", wcx);
        window.input("1", wcx);
        window.press("enter", wcx);
    })
    .unwrap();
    cx.run_until_parked();
    let out = first_video_clip(cx, &ui);
    let m = out.motion.outro.unwrap();
    assert_eq!(
        (m.motion, m.duration),
        (ve_core::motion::Motion::SlideLeft, Time::from_secs(1))
    );
    assert!((out.transform_at(out.duration).x + 1.0).abs() < 1e-6);
    assert!(out.motion.intro.is_some(), "入场还在");
    // 组合动画：和入场 / 出场二选一
    click(cx, &ui, "anim-phase-combo");
    click(cx, &ui, "anim-combo-spin");
    let combo = first_video_clip(cx, &ui).motion;
    assert_eq!(combo.combo, Some(ve_core::motion::Combo::Spin));
    assert!(combo.intro.is_none() && combo.outro.is_none());
    // 「无」去掉；一次一步撤销
    click(cx, &ui, "anim-combo-none");
    assert!(first_video_clip(cx, &ui).motion.is_none());
    press(cx, &ui, "ctrl-z");
    assert_eq!(
        first_video_clip(cx, &ui).motion.combo,
        Some(ve_core::motion::Combo::Spin)
    );
}

#[gpui_kit::test]
fn the_frame_at_the_playhead_exports_as_a_still_image(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    seek_to(cx, &ui, Time::from_secs(1));
    let out = dir.path().join("静帧.png");
    ui.view
        .update(cx, |w, cx| w.export_still_to(out.clone(), cx));
    wait(cx, &ui, |w| !w.is_exporting_still());
    let probed = ve_media::probe(&out).unwrap();
    let v = probed.video.unwrap();
    assert!(v.still, "是一张图片");
    // 和工程一样大（不是预览的小图）
    let size = ui.view.read_with(cx, |w, _| {
        let s = w.document().project.settings;
        (s.width, s.height)
    });
    assert_eq!((v.width, v.height), size);
}

#[gpui_kit::test]
fn a_pasted_script_becomes_subtitles_from_the_playhead(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    seek_to(cx, &ui, Time::from_secs(1));
    // 菜单里有这一项，点了弹出贴文稿的框
    click(cx, &ui, "tb-menu");
    click(cx, &ui, "menu-script");
    assert!(exists(cx, &ui, "script-text"));
    cx.update_window(ui.win, |_, window, wcx| {
        ui.view.update(wcx, |w, vcx| {
            w.import_script_text("第一句。第二句！", window, vcx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let subs = subtitles(cx, &ui);
    assert_eq!(
        subs,
        [
            (Time::from_secs(1), "第一句".to_owned()),
            (Time::from_secs(2), "第二句".to_owned())
        ]
    );
}

#[gpui_kit::test]
fn lrc_lyrics_import_as_subtitles(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let lrc = dir.path().join("歌.lrc");
    std::fs::write(
        &lrc,
        "[ti:歌]
[00:00.50]第一句
[00:02.00]第二句
",
    )
    .unwrap();
    cx.update_window(ui.win, |_, window, wcx| {
        ui.view
            .update(wcx, |w, vcx| w.import_srt_path(&lrc, window, vcx));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        subtitles(cx, &ui),
        [
            (Time::from_millis(500), "第一句".to_owned()),
            (Time::from_secs(2), "第二句".to_owned())
        ]
    );
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
fn subtitle_list_can_be_searched(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let srt = dir.path().join("s.srt");
    std::fs::write(
        &srt,
        "1\n00:00:00,000 --> 00:00:01,000\n你好世界\n\n2\n00:00:02,000 --> 00:00:03,000\n再见\n\n3\n00:00:04,000 --> 00:00:05,000\n世界和平\n",
    )
    .unwrap();
    cx.update_window(ui.win, |_, window, wcx| {
        ui.view
            .update(wcx, |w, vcx| w.import_srt_path(&srt, window, vcx));
    })
    .unwrap();
    cx.run_until_parked();
    click_timeline(cx, &ui, 0.5, 0.0, gpui_kit::Modifiers::default());
    assert!(exists(cx, &ui, "subtitle-list"));
    assert!(exists(cx, &ui, "sub-search"));
    type_into(cx, &ui, "sub-search", "世界");
    // 编号还是原来的：第 1 条和第 3 条
    assert!(exists(cx, &ui, "sub-row-0"));
    assert!(!exists(cx, &ui, "sub-row-1"));
    assert!(exists(cx, &ui, "sub-row-2"));
    // 清空搜索：全部回来
    cx.update_window(ui.win, |_, window, wcx| {
        window.click("sub-search", wcx);
        window.press("ctrl-a", wcx);
        window.press("backspace", wcx);
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    assert!(exists(cx, &ui, "sub-row-1"));
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

#[gpui_kit::test]
fn right_click_saves_the_colour_as_a_preset_to_reuse(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-adjust");
    type_into(cx, &ui, "adj-saturation-value", "-40");
    right_click_timeline(cx, &ui, 0.5, 1.0);
    click(cx, &ui, "cm-save-preset");
    click(cx, &ui, "adj-reset");
    assert!(first_video_clip(cx, &ui).adjust.is_identity());
    click(cx, &ui, "bin-tab-adjust");
    click(cx, &ui, "adjust-preset-0");
    let sat = first_video_clip(cx, &ui).adjust.saturation;
    assert!((sat + 0.4).abs() < 1e-4, "{sat}");
}

/// 在时间轴上右键点一下。
#[gpui_kit::test]
fn right_click_exports_the_selected_clip_and_reveals_its_file(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    right_click_timeline(cx, &ui, 0.5, 1.0);
    assert!(exists(cx, &ui, "cm-reveal"), "有「打开文件所在位置」");
    click(cx, &ui, "cm-export-selection");
    let range = ui
        .view
        .read_with(cx, |w, _| w.document().project.timeline.range);
    assert_eq!(
        range,
        Some((Time::ZERO, Time::from_secs(2))),
        "区域设成这一段"
    );
    assert!(exists(cx, &ui, "export-title"), "弹出导出框");
}

#[gpui_kit::test]
fn right_clicking_a_gap_offers_to_close_it(cx: &mut TestAppContext) {
    let ui = open(cx);
    magnet_off(cx, &ui);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    // 第二段（2–4 秒）往后拖 1 秒：空出 2–3 秒
    let origin = timeline_bounds(cx, &ui).origin;
    cx.update_window(ui.win, |_, window, cx| {
        window.drag(
            origin + timeline_point(3.0, 1.0),
            origin + timeline_point(4.0, 1.0),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let starts = |app: &TestAppContext| -> Vec<Time> {
        video_clips(app, &ui)
            .into_iter()
            .map(|(_, start)| start)
            .collect()
    };
    assert_eq!(starts(cx)[1], Time::from_secs(3));
    right_click_timeline(cx, &ui, 2.5, 1.0);
    frame(cx, &ui);
    assert!(exists(cx, &ui, "blank-menu"), "右键空白处弹菜单");
    assert!(!exists(cx, &ui, "clip-menu"), "不是片段菜单");
    click(cx, &ui, "bm-close-gap");
    assert_eq!(starts(cx), [Time::ZERO, Time::from_secs(2)]);
    assert!(!exists(cx, &ui, "blank-menu"));
    // 一步撤销
    press(cx, &ui, "ctrl-z");
    assert_eq!(starts(cx)[1], Time::from_secs(3));
}

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
fn markers_change_color_and_drag_along_the_ruler(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    seek_to(cx, &ui, Time::from_secs(1));
    press(cx, &ui, "m");
    let color = |app: &TestAppContext, t: Time| {
        ui.view
            .read_with(app, |w, _| w.document().project.timeline.marker_color(t))
    };
    // Alt+M：换颜色
    press(cx, &ui, "alt-m");
    assert_eq!(color(cx, Time::from_secs(1)), 1);
    // 在标尺上按住标记拖到 2 秒：颜色跟着走，一步撤销
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    let from = origin + point(px(132.0 + 50.0), px(4.0));
    let to = origin + point(px(132.0 + 100.0), px(4.0));
    press_and_move(cx, &ui, from, to);
    release(cx, &ui, to);
    let markers = |app: &TestAppContext| {
        ui.view
            .read_with(app, |w, _| w.document().project.timeline.markers.clone())
    };
    assert_eq!(markers(cx), [Time::from_secs(2)]);
    assert_eq!(color(cx, Time::from_secs(2)), 1);
    press(cx, &ui, "ctrl-z");
    assert_eq!(markers(cx), [Time::from_secs(1)]);
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
fn text_styles_can_be_saved_as_presets(cx: &mut TestAppContext) {
    let ui = open(cx);
    press(cx, &ui, "t");
    click(cx, &ui, "insp-tab-text");
    click(cx, &ui, "ts-preset-yellow-black");
    click(cx, &ui, "ts-bold");
    click(cx, &ui, "ts-save-preset");
    let saved = |app: &TestAppContext| {
        ui.view
            .read_with(app, |w, _| w.settings().text_presets.len())
    };
    assert_eq!(saved(cx), 1);
    let style = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            let s = &w.document().project.subtitle_style;
            (s.color, s.bold)
        })
    };
    // 换成别的样式，再点自己存的预设：整套样式回来
    click(cx, &ui, "ts-preset-white-black");
    click(cx, &ui, "ts-bold");
    assert_eq!(style(cx), (0xFF_FFFF, false));
    click(cx, &ui, "ts-mine-0");
    assert_eq!(style(cx), (0xFF_D700, true));
    // 右键删掉
    cx.update_window(ui.win, |_, window, cx| {
        window.right_click("ts-mine-0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(saved(cx), 0);
    assert!(!exists(cx, &ui, "ts-mine-0"));
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
fn portrait_left_puts_the_player_left_and_reset_restores_sizes(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    frame(cx, &ui);
    let bounds = |app: &mut TestAppContext, id: &'static str| {
        app.update_window(ui.win, |_, window, _| window.find(id).bounds())
            .unwrap()
    };
    click(cx, &ui, "tb-layout");
    click(cx, &ui, "layout-portrait-left");
    frame(cx, &ui);
    let player = bounds(cx, "preview");
    let timeline = bounds(cx, "timeline");
    assert!(
        player.origin.x + player.size.width <= timeline.origin.x + px(1.0),
        "播放器在时间轴左边"
    );
    let before = ui.view.read_with(cx, |w, _| w.layout_generation());
    click(cx, &ui, "tb-layout");
    click(cx, &ui, "layout-reset");
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.layout_generation()),
        before + 1,
        "重置：分隔条回到默认位置"
    );
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
fn a_conflicting_shortcut_can_be_cancelled_and_premiere_scheme_applies(cx: &mut TestAppContext) {
    let ui = open(cx);
    // 把分割（Ctrl+B）改成 M（M 是打标记）：取消就什么都不改
    click(cx, &ui, "tb-shortcuts");
    click(cx, &ui, "sc-ctrl-b");
    press(cx, &ui, "m");
    assert!(exists(cx, &ui, "sc-conflict"));
    click(cx, &ui, "sc-conflict-cancel");
    assert_eq!(
        cx.update(|cx| ve_app::keymap::current(cx, "editor::Split|ctrl-b")),
        "ctrl-b"
    );
    assert!(!exists(cx, &ui, "sc-conflict"));
    // Premiere 方案：Ctrl+K 分割
    click(cx, &ui, "sc-scheme-premiere");
    assert_eq!(
        cx.update(|cx| ve_app::keymap::current(cx, "editor::Split|ctrl-b")),
        "ctrl-k"
    );
    assert!(exists(cx, &ui, "sc-scheme-premiere-on"));
    // 换回剪映方案：恢复默认
    click(cx, &ui, "sc-scheme-default");
    assert_eq!(
        cx.update(|cx| ve_app::keymap::current(cx, "editor::Split|ctrl-b")),
        "ctrl-b"
    );
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
    // 把「停用片段」(V) 改成 Ctrl+K：先问要不要替换，确认后加字幕的 Ctrl+K 被去掉
    click(cx, &ui, "tb-shortcuts");
    click(cx, &ui, "sc-v");
    press(cx, &ui, "ctrl-k");
    assert!(exists(cx, &ui, "sc-conflict"), "冲突先问一下");
    click(cx, &ui, "sc-conflict-replace");
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
fn mask_and_look_sliders_take_keyframes(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "seg-mask");
    click(cx, &ui, "mask-2");
    seek_to(cx, &ui, Time::ZERO);
    click(cx, &ui, "tf-key-mask-feather");
    let feather = |app: &mut TestAppContext, at: Time| {
        first_video_clip(app, &ui).look_at(at).mask.unwrap().feather
    };
    let start = feather(cx, Time::ZERO);
    assert_eq!(
        first_video_clip(cx, &ui)
            .animation
            .keys(ve_core::Prop::MaskFeather)
            .len(),
        1
    );
    // 换个时刻把羽化拖满：自动在那一刻打关键帧，0 秒不变
    seek_to(cx, &ui, Time::from_millis(500));
    let bounds = cx
        .update_window(ui.win, |_, window, _| {
            window.find("look-mask-feather").bounds()
        })
        .unwrap();
    let y = bounds.origin.y + bounds.size.height / 2.0;
    let from = point(bounds.origin.x + bounds.size.width / 2.0, y);
    let to = point(bounds.origin.x + bounds.size.width - px(1.0), y);
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let c = first_video_clip(cx, &ui);
    assert_eq!(c.animation.keys(ve_core::Prop::MaskFeather).len(), 2);
    assert!((feather(cx, Time::ZERO) - start).abs() < 1e-6);
    assert!(feather(cx, Time::from_millis(500)) > 0.9);
    // 去掉蒙版：关键帧一起没了
    click(cx, &ui, "mask-none");
    assert_eq!(
        first_video_clip(cx, &ui)
            .animation
            .keys(ve_core::Prop::MaskFeather),
        []
    );
}

#[gpui_kit::test]
fn adjust_and_volume_rows_have_their_own_keyframes(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::VideoWithAudio)], true);
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
    press_export(cx, &ui, &out);
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
fn text_style_has_free_colors_italic_and_a_background(cx: &mut TestAppContext) {
    let ui = open(cx);
    press(cx, &ui, "t");
    click(cx, &ui, "insp-tab-text");
    let style = |app: &TestAppContext| {
        ui.view
            .read_with(app, |w, _| w.document().project.subtitle_style.clone())
    };
    click(cx, &ui, "ts-italic");
    assert!(style(cx).italic);
    cx.update_window(ui.win, |_, window, cx| {
        window.scroll(
            "inspector-body",
            ScrollDelta::Pixels(point(px(0.), px(-300.))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    // 色板第 3 个是红色
    click(cx, &ui, "ts-color-2");
    assert_eq!(style(cx).color, 0xE5_484D);
    assert!(exists(cx, &ui, "ts-color-picker"), "还有取色器选任意颜色");
    assert!(exists(cx, &ui, "ts-font-all"), "还能从全部字体里选");
    // 背景：选个颜色就显出来
    cx.update_window(ui.win, |_, window, cx| {
        window.scroll(
            "inspector-body",
            ScrollDelta::Pixels(point(px(0.), px(-3000.))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    click(cx, &ui, "ts-bg-color-1");
    let s = style(cx);
    assert!(s.has_background() && s.background_color == 0x00_0000);
    // 一次一步撤销
    press(cx, &ui, "ctrl-z");
    assert!(!style(cx).has_background());
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
        // 文本面板很长（字号、颜色、描边、阴影、背景……）：滚到底
        window.scroll(
            "inspector-body",
            ScrollDelta::Pixels(point(px(0.), px(-3000.))),
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

#[gpui_kit::test]
fn missing_media_is_flagged_and_relinking_finds_the_whole_folder(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let (old, new) = (root.path().join("旧"), root.path().join("新"));
    std::fs::create_dir(&new).unwrap();
    let mut files = Vec::new();
    for f in [Fixture::FrameIndexed30, Fixture::ToneStereo48k] {
        let src = fixture(f);
        let dst = new.join(src.file_name().unwrap());
        std::fs::copy(&src, &dst).unwrap();
        files.push(dst);
    }
    let ui = open(cx);
    import(cx, &ui, files.clone(), true);
    // 工程里记的是素材搬走前的位置（「旧」文件夹已经不在了）
    let mut project = ui.view.read_with(cx, |w, _| w.document().project.clone());
    for a in project.assets.values_mut() {
        a.path = old.join(a.path.file_name().unwrap());
    }
    let file = root.path().join("工程.vep");
    ve_core::save_project(&project, &file).unwrap();
    cx.update_window(ui.win, |_, window, cx| {
        ui.view.update(cx, |w, cx| w.open_path(&file, window, cx));
    })
    .unwrap();
    let ids: Vec<ve_core::AssetId> = ui.view.read_with(cx, |w, _| {
        w.document().project.assets.keys().copied().collect()
    });
    let missing = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            ids.iter().filter(|&&id| w.is_missing(id)).count()
        })
    };
    assert_eq!(missing(cx), 2);
    let button: &'static str = format!("relink-{}", ids[0].0).leak();
    assert!(exists(cx, &ui, button), "素材卡片上有「链接媒体」");
    // 给第一个指新位置：同文件夹里的另一个一起找回
    let target = new.join(files[0].file_name().unwrap());
    cx.update_window(ui.win, |_, window, cx| {
        ui.view
            .update(cx, |w, cx| w.relink_with(ids[0], target, window, cx));
    })
    .unwrap();
    wait(cx, &ui, |w| ids.iter().all(|&id| !w.is_missing(id)));
    assert!(!exists(cx, &ui, button));
    // 一步撤销
    press(cx, &ui, "ctrl-z");
    assert_eq!(missing(cx), 2);
}

#[gpui_kit::test]
fn a_crashed_session_reopens_its_project(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let project = write_project(dir.path(), "上次的");
    let sessions = ve_app::recovery::Sessions::new(dir.path().join("sessions"));
    // 上次的进程（早就不在了）开着这个工程，没来得及删记录
    let gone = u32::MAX - 1;
    sessions.record(gone, Some(&project)).unwrap();
    let ui = open(cx);
    let me = std::process::id();
    let s = sessions.clone();
    cx.update_window(ui.win, |_, window, cx| {
        ui.view
            .update(cx, |w, cx| w.enable_recovery(s, me, window, cx));
    })
    .unwrap();
    let opened = ui.view.read_with(cx, |w, _| {
        w.document().path().map(std::path::Path::to_path_buf)
    });
    assert_eq!(opened, Some(project.clone()));
    // 旧记录删了，本进程记着现在的工程
    let left = sessions.crashed(|_, _| false);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].pid, me);
    assert_eq!(left[0].session.project, Some(project));
}

#[gpui_kit::test]
fn picture_effects_are_added_from_the_bin_and_edited_in_the_inspector(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    seek_to(cx, &ui, Time::from_millis(500));
    click(cx, &ui, "bin-tab-effect");
    click(cx, &ui, "bin-effect-0");
    let effect = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            let doc = w.document();
            let id = doc.selection()[0];
            let clip = doc.project.timeline.find_clip(id).unwrap().1;
            (clip.effect(), (clip.start, clip.end()))
        })
    };
    let (added, span) = effect(cx);
    assert_eq!(
        added.map(|e| e.kind),
        Some(ve_core::EffectKind::Blur),
        "新加的特效片段被选中"
    );
    assert_eq!(span, (Time::from_millis(500), Time::from_millis(3500)));
    // 属性面板只有「特效」：换成暗角，一步撤销
    assert!(exists(cx, &ui, "insp-tab-effect"));
    assert!(!exists(cx, &ui, "insp-tab-picture"));
    click(cx, &ui, "effect-kind-2");
    assert_eq!(
        effect(cx).0.map(|e| e.kind),
        Some(ve_core::EffectKind::Vignette)
    );
    press(cx, &ui, "ctrl-z");
    assert_eq!(
        effect(cx).0.map(|e| e.kind),
        Some(ve_core::EffectKind::Blur)
    );
    // 预览里有这道特效
    let passes = ui.view.read_with(cx, |w, _| {
        ve_core::compose::resolve_frame(&w.document().project, Time::from_secs(1))
            .effects
            .len()
    });
    assert_eq!(passes, 1);
}

#[gpui_kit::test]
fn audio_denoise_eq_and_compression_are_set_in_the_audio_tab(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::ToneStereo48k)], true);
    // 纯音频片段在音频轨（第 2 行）
    click_timeline(cx, &ui, 0.5, 2.0, gpui_kit::Modifiers::default());
    let fx = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            let doc = w.document();
            let id = doc.selection()[0];
            doc.project.timeline.find_clip(id).unwrap().1.audio_fx
        })
    };
    assert!(fx(cx).is_none());
    // 勾上「音频降噪」：默认强度一半
    click(cx, &ui, "fx-dn-check");
    assert!((fx(cx).denoise - 0.5).abs() < 1e-6);
    click(cx, &ui, "fx-cp-check");
    assert!((fx(cx).compress - 0.5).abs() < 1e-6);
    // 取消勾选：关掉；撤销回到开着
    click(cx, &ui, "fx-dn-check");
    assert_eq!(fx(cx).denoise, 0.0);
    press(cx, &ui, "ctrl-z");
    assert!((fx(cx).denoise - 0.5).abs() < 1e-6);
}

#[gpui_kit::test]
fn voice_presets_are_picked_in_the_audio_tab(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::ToneStereo48k)], true);
    click_timeline(cx, &ui, 0.5, 2.0, gpui_kit::Modifiers::default());
    let voice = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            let doc = w.document();
            let id = doc.selection()[0];
            doc.project.timeline.find_clip(id).unwrap().1.audio_fx.voice
        })
    };
    assert_eq!(voice(cx), ve_core::Voice::None);
    // 面板放不下，收起上面的「均衡」让变声露出来
    click(cx, &ui, "fx-eq-title");
    click(cx, &ui, "fx-voice-Uncle");
    assert_eq!(voice(cx), ve_core::Voice::Uncle);
    click(cx, &ui, "fx-voice-Robot");
    assert_eq!(voice(cx), ve_core::Voice::Robot);
    // 撤销一步回到大叔；点「无」回到原声
    press(cx, &ui, "ctrl-z");
    assert_eq!(voice(cx), ve_core::Voice::Uncle);
    click(cx, &ui, "fx-voice-None");
    assert_eq!(voice(cx), ve_core::Voice::None);
}

#[gpui_kit::test]
fn ctrl_g_groups_clips_so_they_are_picked_together(cx: &mut TestAppContext) {
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
    let selected = |app: &TestAppContext| {
        ui.view
            .read_with(app, |w, _| w.document().selection().len())
    };
    // 视频（第 1 行）和音频（第 2 行）一起选中，Ctrl+G 组合
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click_timeline(cx, &ui, 0.5, 2.0, gpui_kit::Modifiers::control());
    press(cx, &ui, "ctrl-g");
    // 只点视频：两段都选上
    click_timeline(cx, &ui, 8.0, 1.0, gpui_kit::Modifiers::default());
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    assert_eq!(selected(cx), 2, "组合里的一起选中");
    // 再按 Ctrl+G 取消组合：点一段只选一段
    press(cx, &ui, "ctrl-g");
    click_timeline(cx, &ui, 8.0, 1.0, gpui_kit::Modifiers::default());
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    assert_eq!(selected(cx), 1);
    // 组合是一步撤销
    press(cx, &ui, "ctrl-z");
    click_timeline(cx, &ui, 8.0, 1.0, gpui_kit::Modifiers::default());
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    assert_eq!(selected(cx), 2);
}

#[gpui_kit::test]
fn compound_clips_pack_and_unpack_the_selection(cx: &mut TestAppContext) {
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
    let count = |app: &TestAppContext| {
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
    let before = count(cx);
    press(cx, &ui, "ctrl-a");
    press(cx, &ui, "alt-g");
    assert_eq!(count(cx), 1, "全部打包成一段");
    let compound = ui.view.read_with(cx, |w, _| {
        let doc = w.document();
        let id = doc.selection()[0];
        doc.project
            .timeline
            .find_clip(id)
            .unwrap()
            .1
            .compound()
            .is_some()
    });
    assert!(compound, "新的复合片段被选中");
    assert!(exists(cx, &ui, "insp-break-compound"));
    // 播放用的工程是摊开的：画面还在
    let layers = ui.view.read_with(cx, |w, _| {
        let flat = ve_core::compound::flatten(&w.document().project).unwrap();
        ve_core::compose::resolve_layers(&flat, Time::from_millis(500)).len()
    });
    assert_eq!(layers, 1);
    // 解除：放回去；撤销回到复合片段
    click(cx, &ui, "insp-break-compound");
    assert_eq!(count(cx), before);
    press(cx, &ui, "ctrl-z");
    assert_eq!(count(cx), 1);
    // 点选时间轴上的复合片段（视频轨，第 1 行），快捷键解除
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    press(cx, &ui, "alt-shift-g");
    assert_eq!(count(cx), before);
}

#[gpui_kit::test]
fn proxy_mode_plays_a_low_res_copy_and_exports_the_original(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let ui = open(cx);
    // 测试素材很小：门槛放到 0，什么视频都做副本
    let proxies = dir.path().join("proxies");
    ui.view
        .update(cx, |w, _| w.use_proxy_settings(proxies.clone(), 0));
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    let id = ui.view.read_with(cx, |w, _| {
        *w.document().project.assets.keys().next().unwrap()
    });
    let original = ui
        .view
        .read_with(cx, |w, _| w.document().project.assets[&id].path.clone());
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.playing_path(id)),
        Some(original.clone())
    );
    // ≡ 菜单里打开代理模式：做好副本后播放用副本
    click(cx, &ui, "player-menu-btn");
    click(cx, &ui, "player-proxy");
    assert!(ui.view.read_with(cx, |w, _| w.proxy_mode()));
    wait(cx, &ui, |w| w.has_proxy(id));
    let playing = ui.view.read_with(cx, |w, _| w.playing_path(id)).unwrap();
    assert!(playing.starts_with(&proxies), "{playing:?}");
    assert!(playing.is_file());
    // 时间轴 / 导出用的工程还是原片
    let in_doc = ui
        .view
        .read_with(cx, |w, _| w.document().project.assets[&id].path.clone());
    assert_eq!(in_doc, original);
    // 关掉：回到原片
    click(cx, &ui, "player-menu-btn");
    click(cx, &ui, "player-proxy");
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.playing_path(id)),
        Some(original)
    );
}

#[gpui_kit::test]
fn speed_has_a_slider_editable_duration_and_a_draggable_curve(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-speed");
    assert!(exists(cx, &ui, "speed-rate"));
    let type_in = |app: &mut TestAppContext, id: &'static str, text: &str| {
        click(app, &ui, id);
        app.update_window(ui.win, |_, window, wcx| {
            window.press("ctrl-a", wcx);
            window.input(text, wcx);
            window.press("enter", wcx);
        })
        .unwrap();
        app.run_until_parked();
    };
    // 倍数直接输入
    type_in(cx, "speed-rate-value", "2");
    let typed = first_video_clip(cx, &ui).speed;
    assert!((typed - 2.0).abs() < 1e-3, "{typed}");
    // 改时长 = 改倍速：输入 4 秒
    type_in(cx, "speed-duration-value", "4");
    let c = first_video_clip(cx, &ui);
    assert!(
        (c.duration.as_secs_f64() - 4.0).abs() < 0.05,
        "{:?}",
        c.duration
    );
    assert!((f64::from(c.speed) - c.source_span().as_secs_f64() / 4.0).abs() < 1e-3);
    // 曲线变速 › 自定义：把中间的点往上拖到顶 → 那里变快
    click(cx, &ui, "speed-seg-curve");
    click(cx, &ui, "curve-custom");
    frame(cx, &ui);
    let b = cx
        .update_window(ui.win, |_, window, _| window.find("curve-editor").bounds())
        .unwrap();
    let from = point(
        b.origin.x + b.size.width / 2.0,
        b.origin.y + b.size.height / 2.0,
    );
    let to = point(from.x, b.origin.y + px(1.0));
    cx.update_window(ui.win, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let fast = first_video_clip(cx, &ui).curve.unwrap().speed_at(0.5);
    assert!(fast > 5.0, "{fast}");
    // 一次拖动一步撤销
    press(cx, &ui, "ctrl-z");
    let undone = first_video_clip(cx, &ui).curve.unwrap().speed_at(0.5);
    assert!((undone - 1.0).abs() < 0.05, "{undone}");
}

#[gpui_kit::test]
fn frame_blending_is_toggled_in_the_speed_tab(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-speed");
    let blend = |app: &TestAppContext| first_video_clip(app, &ui).frame_blend;
    assert!(!blend(cx));
    click(cx, &ui, "speed-frame-blend");
    assert!(blend(cx));
    press(cx, &ui, "ctrl-z");
    assert!(!blend(cx));
}

#[gpui_kit::test]
fn video_stabilization_swaps_in_a_steadied_copy_and_back(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let ui = open(cx);
    ui.view
        .update(cx, |w, _| w.use_stabilize_dir(dir.path().to_path_buf()));
    import(cx, &ui, vec![fixture(Fixture::VideoWithAudio)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    let original = first_video_clip(cx, &ui).asset();
    // 「画面 › 基础」最下面的「视频防抖」：选「推荐」
    for _ in 0..3 {
        cx.update_window(ui.win, |_, window, cx| {
            window.scroll(
                "inspector-body",
                ScrollDelta::Pixels(point(px(0.), px(-800.))),
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
        frame(cx, &ui);
    }
    // 测试里后台任务在点击这一步就跑完了
    click(cx, &ui, "stab-1");
    wait(cx, &ui, |w| !w.is_stabilizing());
    let c = first_video_clip(cx, &ui);
    assert_eq!(c.stabilized.map(|s| s.2), Some(1));
    assert_ne!(c.asset(), original);
    // 关闭：换回原片
    click(cx, &ui, "stab-off");
    let back = first_video_clip(cx, &ui);
    assert_eq!(back.asset(), original);
    assert!(back.stabilized.is_none());
}

#[gpui_kit::test]
fn slip_slide_and_roll_tools_drag_the_cut_points(cx: &mut TestAppContext) {
    let ui = open(cx);
    let c = three_clips(cx, &ui);
    magnet_off(cx, &ui);
    let origin = cx
        .update_window(ui.win, |_, window, _| {
            window.find("timeline").bounds().origin
        })
        .unwrap();
    let span = |app: &TestAppContext, id: ve_core::ClipId| {
        ui.view.read_with(app, |w, _| {
            let (_, x) = w.document().project.timeline.find_clip(id).unwrap();
            (x.start, x.end(), x.source_in())
        })
    };
    let drag = |app: &mut TestAppContext, from: f32, to: f32| {
        let a = origin + timeline_point(from, 1.0);
        let b = origin + timeline_point(to, 1.0);
        app.update_window(ui.win, |_, window, wcx| window.drag(a, b, wcx))
            .unwrap();
        app.run_until_parked();
    };
    // 三段各 1 秒（0–1、1–2、2–3）。滚动（R）：拖第二段左边缘往右 0.2 秒
    press(cx, &ui, "r");
    let before = span(cx, c[1]);
    drag(cx, 1.0, 1.2);
    let after = span(cx, c[1]);
    assert_eq!(span(cx, c[0]).1, after.0, "剪辑点两边还是紧挨着");
    assert!(
        after.0 > before.0 && after.2 > before.2,
        "{before:?} → {after:?}"
    );
    // 滑动（U）：中间那段往右挪，前一段变长、后一段变短（后一段从素材开头放，不能再往左伸）
    press(cx, &ui, "u");
    let mid = span(cx, c[1]);
    drag(cx, 1.5, 1.7);
    let slid = span(cx, c[1]);
    assert!(slid.0 > mid.0, "{mid:?} → {slid:?}");
    assert_eq!(slid.2, mid.2, "内容不变");
    assert_eq!(span(cx, c[0]).1, slid.0);
    assert_eq!(span(cx, c[2]).0, slid.1);
    // 一次拖动一步撤销
    press(cx, &ui, "ctrl-z");
    assert_eq!(span(cx, c[1]), mid);
    // 选择模式（A）回到普通拖动
    press(cx, &ui, "a");
    assert!(exists(cx, &ui, "tb-tool"));
}

#[gpui_kit::test]
fn export_presets_fill_in_the_platform_settings(cx: &mut TestAppContext) {
    let ui = open(cx);
    let dir = tempfile::tempdir().unwrap();
    open_export_dialog(cx, &ui, &dir.path().join("b站.mp4"));
    cx.update_window(ui.win, |_, window, cx| window.click("preset-bilibili", cx))
        .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    // 选中的预设和它带的帧率都亮着
    cx.update_window(ui.win, |_, window, _| {
        assert!(window.try_find("preset-bilibili").is_some());
        assert!(window.try_find("fps-60").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn scopes_show_over_the_preview_from_the_player_menu(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    wait(cx, &ui, Workspace::showing_frame);
    assert!(!exists(cx, &ui, "scopes"));
    click(cx, &ui, "player-menu-btn");
    click(cx, &ui, "player-scopes");
    assert!(ui.view.read_with(cx, |w, _| w.scopes_on()));
    assert!(exists(cx, &ui, "scopes"), "画面右上角出现示波器");
    click(cx, &ui, "player-menu-btn");
    click(cx, &ui, "player-scopes");
    assert!(!exists(cx, &ui, "scopes"));
}

#[gpui_kit::test]
fn otio_export_and_import_round_trip_the_timeline(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
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
    let otio = dir.path().join("时间线.otio");
    let edl = dir.path().join("时间线.edl");
    ui.view.read_with(cx, |w, _| {
        w.export_interchange(ve_app::interchange_ui::Interchange::Otio, &otio)
            .unwrap();
        w.export_interchange(ve_app::interchange_ui::Interchange::Edl, &edl)
            .unwrap();
    });
    assert!(std::fs::read_to_string(&edl).unwrap().contains("001  AX"));
    let spans = |app: &TestAppContext| {
        ui.view.read_with(app, |w, _| {
            let mut v: Vec<(TrackKind, Time, Time)> = w
                .document()
                .project
                .timeline
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter().map(move |c| (t.kind, c.start, c.end())))
                .collect();
            v.sort_by_key(|x| (x.1, x.2));
            v
        })
    };
    let before = spans(cx);
    cx.update_window(ui.win, |_, window, cx| {
        ui.view
            .update(cx, |w, cx| w.import_otio_path(otio.clone(), window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    frame(cx, &ui);
    assert_eq!(spans(cx), before, "导回来是一样的剪辑结构");
    assert_eq!(
        ui.view.read_with(cx, |w, _| w
            .document()
            .path()
            .map(std::path::Path::to_path_buf)),
        None,
        "导入成新草稿"
    );
    assert!(
        ui.view.read_with(cx, |w, _| w.document().is_dirty()),
        "还没存过：算未保存，自动保存和关窗口提示都管它"
    );
}

#[gpui_kit::test]
fn input_color_space_is_picked_in_the_adjust_tab(cx: &mut TestAppContext) {
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click_timeline(cx, &ui, 0.5, 1.0, gpui_kit::Modifiers::default());
    click(cx, &ui, "insp-tab-adjust");
    for _ in 0..3 {
        cx.update_window(ui.win, |_, window, cx| {
            window.scroll(
                "inspector-body",
                ScrollDelta::Pixels(point(px(0.), px(-800.))),
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
        frame(cx, &ui);
    }
    click(cx, &ui, "cs-2");
    assert_eq!(
        first_video_clip(cx, &ui).input_space,
        Some(ve_core::color_space::InputSpace::SLog3)
    );
    click(cx, &ui, "cs-none");
    assert_eq!(first_video_clip(cx, &ui).input_space, None);
}

#[gpui_kit::test]
fn plugin_effects_load_from_the_folder_and_go_on_the_timeline(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("effects");
    ve_app::plugins::seed_examples(&plugins).unwrap();
    let ui = open(cx);
    cx.update_window(ui.win, |_, window, cx| {
        ui.view
            .update(cx, |w, cx| w.use_plugin_dir(plugins.clone(), window, cx));
    })
    .unwrap();
    assert_eq!(
        ui.view.read_with(cx, |w, _| w.plugins().to_vec()),
        ["反色", "老电视"]
    );
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    click(cx, &ui, "bin-tab-effect");
    click(cx, &ui, "bin-plugin-0");
    let effect = ui.view.read_with(cx, |w, _| {
        let doc = w.document();
        let id = doc.selection()[0];
        doc.project.timeline.find_clip(id).unwrap().1.effect()
    });
    assert_eq!(effect.map(|e| e.name().to_owned()), Some("反色".to_owned()));
    // 新放一个插件进去，「重新载入」后出现
    std::fs::write(
        plugins.join("变暗.wgsl"),
        "fn plugin_effect(c: vec3<f32>, p: vec2<f32>, dims: vec2<f32>, amount: f32, t: f32) -> vec3<f32> { return c * (1.0 - amount); }",
    )
    .unwrap();
    click(cx, &ui, "bin-plugin-reload");
    assert_eq!(ui.view.read_with(cx, |w, _| w.plugins().len()), 3);
}

#[gpui_kit::test]
fn opening_a_project_with_missing_media_at_startup_does_not_crash(cx: &mut TestAppContext) {
    // 命令行带工程启动（或崩溃恢复）时，主窗口还在搭建中就打开工程；有丢失的素材会弹提示，
    // 这时组件库的根视图还没建好，直接弹会 panic
    let dir = tempfile::tempdir().unwrap();
    let mut p = ve_core::Project::new();
    let gone = p.add_asset(ve_core::Asset {
        id: ve_core::AssetId(0),
        name: "不在了.mp4".into(),
        path: dir.path().join("不在了.mp4"),
        duration: Time::from_secs(2),
        video: None,
        audio: Some(ve_core::AudioInfo {
            sample_rate: 48_000,
            channels: 2,
        }),
    });
    ve_core::edit::append_audio(&mut p, gone).unwrap();
    let file = dir.path().join("有丢失素材.vep");
    ve_core::save_project(&p, &file).unwrap();
    cx.update(ve_app::app::init);
    let (win, view) = cx.update(|cx| {
        let engine = Engine::new(EngineConfig {
            audio: AudioBackend::Null,
            ..EngineConfig::default()
        })
        .unwrap();
        gpui_kit::open_window(WindowOptions::default(), cx, |w, cx| {
            let view = ve_app::workspace::build(engine, w, cx);
            view.update(cx, |ws, cx| ws.open_path(&file, w, cx));
            view
        })
        .unwrap()
    });
    cx.run_until_parked();
    let opened = view.read_with(cx, |w, _| {
        w.document().path().map(std::path::Path::to_path_buf)
    });
    assert_eq!(opened, Some(file));
    cx.update_window(win, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

#[gpui_kit::test]
fn clicking_a_popup_menu_item_does_not_reach_what_is_under_it(cx: &mut TestAppContext) {
    // ≡ 菜单盖在预览画面上：点菜单项不能顺带点到下面的画面（会选中那一层）
    let ui = open(cx);
    import(cx, &ui, vec![fixture(Fixture::FrameIndexed30)], true);
    wait(cx, &ui, Workspace::showing_frame);
    press(cx, &ui, "escape");
    assert_eq!(selected(cx, &ui), []);
    click(cx, &ui, "player-menu-btn");
    click(cx, &ui, "player-scopes");
    assert!(ui.view.read_with(cx, |w, _| w.scopes_on()));
    assert_eq!(selected(cx, &ui), [], "菜单下面的画面没被点到");
}
