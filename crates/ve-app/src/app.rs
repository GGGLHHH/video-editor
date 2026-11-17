//! 启动：初始化组件库、主题、快捷键、播放引擎，打开主窗口。

use crate::{actions, theme, workspace};
use gpui_kit::{App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use std::path::PathBuf;
use ve_engine::{Engine, EngineConfig};

/// 主窗口选项。
#[must_use]
pub fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("视频剪辑器".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1440.), px(900.)),
            cx,
        ))),
        window_min_size: Some(size(px(960.), px(600.))),
        ..Default::default()
    }
}

/// 初始化组件库、主题、快捷键（测试里也调用）。
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    theme::apply(cx);
    actions::bind_keys(cx);
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
}

/// 解析命令行：`ve-app [工程.vep] [素材...] [--bench-playback 素材 --seconds N --layers N]`。
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
                view.update(cx, |ws, cx| ws.startup(&args, window, cx));
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
            args(&["--bench-playback", "z.mp4", "--layers", "3"]).bench_layers,
            3
        );
    }
}
