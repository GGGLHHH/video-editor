//! 导出测速：用一个素材铺 N 层（见 `ve_core::bench`），导出到临时目录，打印帧/秒和几倍实时。
//!
//! 用法：`cargo run --release -p ve-export --example export_bench -- <素材> [层数=1] [秒数=30] [nvenc|x264] [输出文件]`
//!
//! 不给输出文件时导出到临时目录，测完删掉。

#![expect(
    clippy::print_stdout,
    clippy::unwrap_used,
    clippy::cast_precision_loss,
    reason = "命令行测速工具"
)]

use std::sync::atomic::AtomicBool;
use ve_core::{Asset, AssetId, Project, Time};
use ve_export::Encoder;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = std::path::PathBuf::from(&args[0]);
    let layers: usize = args.get(1).map_or(1, |s| s.parse().unwrap());
    let secs: i64 = args.get(2).map_or(30, |s| s.parse().unwrap());
    let encoder = match args.get(3).map(String::as_str) {
        Some("x264") => Encoder::X264,
        _ => Encoder::Nvenc,
    };
    let r = ve_media::probe(&path).unwrap();
    let mut p = Project::new();
    let id = p.add_asset(Asset {
        id: AssetId(0),
        name: "bench".into(),
        path,
        duration: r.duration,
        video: r.video,
        audio: r.audio,
    });
    ve_core::bench::stack_layers(&mut p, id, layers, Time::from_secs(secs)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = args
        .get(4)
        .map_or_else(|| dir.path().join("bench.mp4"), std::path::PathBuf::from);
    let mut settings = ve_export::default_settings(&p, out);
    settings.encoder = encoder;
    let started = std::time::Instant::now();
    ve_export::export(&p, &settings, |_| {}, &AtomicBool::new(false)).unwrap();
    let wall = started.elapsed().as_secs_f64();
    let frames = p.settings.frame_rate.frame_of(p.timeline.duration());
    println!(
        "{layers} 层 {}x{} {secs} 秒 {frames} 帧，编码器 {encoder:?}：用时 {wall:.2}s，{:.1} 帧/秒，{:.2} 倍实时",
        settings.width,
        settings.height,
        frames as f64 / wall,
        secs as f64 / wall
    );
}
