//! 启动：初始化组件库、主题、快捷键、播放引擎，打开主窗口。

use crate::{actions, theme, workspace};
use gpui_kit::{App, Bounds, WindowBounds, WindowOptions, px, size};
use std::path::PathBuf;
use ve_engine::{Engine, EngineConfig};

/// 主窗口选项：不要系统标题栏（剪映），顶栏自己画最小化 / 最大化 / 关闭，空白处拖动窗口。
#[must_use]
pub fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1440.), px(900.)),
            cx,
        ))),
        window_min_size: Some(size(px(960.), px(600.))),
        ..gpui_kit::component::TitleBar::window_options()
    }
}

/// 初始化组件库、主题、快捷键（测试里也调用）。
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    theme::apply(cx);
    actions::bind_keys(cx);
    crate::keymap::install(cx, None);
}

/// 命令行参数。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Args {
    /// 要打开的工程。
    pub project: Option<PathBuf>,
    /// 要导入并放进时间轴的素材。
    pub media: Vec<PathBuf>,
    /// 播放性能测试：播放这么多秒后在日志里输出帧率并退出。
    pub bench_seconds: Option<u64>,
    /// 播放性能测试时把素材铺成几层（见 `ve_core::bench`），默认 1。
    pub bench_layers: usize,
    /// 截图核对用：打开后选中第一段视频、切到这个面板（见 `Workspace::show_panel`）。
    pub show: Option<String>,
}

/// 解析命令行：`ve-app [工程.vep] [素材...] [--bench-playback 素材 --seconds N --layers N]
/// [--show 面板]`。
#[must_use]
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Args {
    let mut out = Args::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--bench-playback" => {
                out.media.extend(it.next().map(PathBuf::from));
                out.bench_seconds = out.bench_seconds.or(Some(10));
            }
            "--seconds" => out.bench_seconds = it.next().and_then(|s| s.parse().ok()),
            "--layers" => out.bench_layers = it.next().and_then(|s| s.parse().ok()).unwrap_or(1),
            "--show" => out.show = it.next(),
            _ if a.to_ascii_lowercase().ends_with(".vep") => out.project = Some(PathBuf::from(a)),
            _ => out.media.push(PathBuf::from(a)),
        }
    }
    out
}

/// 运行应用（阻塞到窗口关闭）。
pub fn run() {
    let args = parse_args(std::env::args().skip(1));
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();
    if let Err(e) = ve_media::probe::init() {
        log::error!("{e}");
        return;
    }
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx: &mut App| {
            init(cx);
            // 改过的快捷键
            crate::keymap::use_file(cx, crate::keymap::default_path());
            let engine = match Engine::new(EngineConfig::default()) {
                Ok(e) => e,
                Err(e) => {
                    log::error!("播放引擎启动失败：{e}");
                    cx.quit();
                    return;
                }
            };
            let opened = gpui_kit::open_window(window_options(cx), cx, |window, cx| {
                let view = workspace::build(engine, window, cx);
                view.update(cx, |ws, cx| {
                    ws.startup(&args, window, cx);
                    // 性能测试时不自动保存（别往草稿目录里写东西）
                    if args.bench_seconds.is_none() {
                        ws.enable_autosave(crate::autosave::default_dir(), window, cx);
                        // 全局设置：草稿位置、缓存位置、图片默认时长、新草稿帧率
                        let file = crate::settings::AppSettings::default_file();
                        ws.use_settings(crate::settings::AppSettings::load(&file), Some(file), cx);
                        // 特效插件：第一次用时放两个示例
                        let dir = crate::plugins::default_dir();
                        if let Err(e) = crate::plugins::seed_examples(&dir) {
                            log::warn!("插件目录建不了：{e}");
                        }
                        ws.use_plugin_dir(dir, window, cx);
                        // 没带工程、素材启动：先进草稿首页（剪映）
                        if args.project.is_none() && args.media.is_empty() {
                            ws.show_home(cx);
                        }
                        // 第一次打开：新手引导（等这一轮更新完再弹，主窗口还在搭）
                        cx.defer_in(window, workspace::Workspace::show_guide_if_new);
                        ws.enable_recovery(
                            crate::recovery::Sessions::new(
                                crate::recovery::Sessions::default_dir(),
                            ),
                            std::process::id(),
                            window,
                            cx,
                        );
                    }
                });
                view
            });
            if let Err(e) = opened {
                log::error!("打不开主窗口：{e}");
                cx.quit();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Args {
        parse_args(v.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn parses_project_media_and_bench() {
        assert_eq!(args(&[]), Args::default());
        let a = args(&["a.VEP", "x.mp4", "y.wav"]);
        assert_eq!(a.project, Some(PathBuf::from("a.VEP")));
        assert_eq!(
            a.media,
            vec![PathBuf::from("x.mp4"), PathBuf::from("y.wav")]
        );
        let b = args(&["--bench-playback", "z.mp4", "--seconds", "30"]);
        assert_eq!(
            (b.media, b.bench_seconds),
            (vec![PathBuf::from("z.mp4")], Some(30))
        );
        assert_eq!(args(&["--bench-playback", "z.mp4"]).bench_seconds, Some(10));
        assert_eq!(
            args(&["p.vep", "--show", "mask"]).show.as_deref(),
            Some("mask")
        );
        assert_eq!(
            args(&["--bench-playback", "z.mp4", "--layers", "3"]).bench_layers,
            3
        );
    }
}
