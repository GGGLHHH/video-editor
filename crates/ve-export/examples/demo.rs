//! 端到端演示：视频 + 背景音乐 + 字幕 → 保存工程 → 导出 mp4。
//!
//! 用法：`cargo run --release -p ve-export --example demo -- <视频> <音频> <输出目录>`

#![expect(clippy::print_stdout, clippy::unwrap_used, reason = "命令行演示工具")]

use std::sync::atomic::AtomicBool;
use ve_core::edit::{add_subtitle, append_asset, set_fades, set_gain, split_at, trim_end};
use ve_core::{Asset, AssetId, Project, Time};

fn asset(p: &mut Project, path: &str) -> AssetId {
    let path = std::path::PathBuf::from(path);
    let r = ve_media::probe(&path).unwrap();
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    p.add_asset(Asset {
        id: AssetId(0),
        name,
        path,
        duration: r.duration,
        video: r.video,
        audio: r.audio,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out_dir = std::path::PathBuf::from(&args[2]);
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut p = Project::new();
    let video = asset(&mut p, &args[0]);
    let bgm = asset(&mut p, &args[1]);
    let first = append_asset(&mut p, video).unwrap();
    trim_end(&mut p, first, Time::from_secs(6)).unwrap();
    let second = split_at(&mut p, first, Time::from_secs(3)).unwrap();
    // 第二段的原声压低
    set_gain(&mut p, second, -12.0).unwrap();
    let music = append_asset(&mut p, bgm).unwrap();
    trim_end(&mut p, music, Time::from_secs(6)).unwrap();
    set_fades(
        &mut p,
        music,
        Time::from_millis(500),
        Time::from_millis(500),
    )
    .unwrap();
    add_subtitle(
        &mut p,
        Time::ZERO,
        Time::from_secs(3),
        "第一句：示例字幕 {测试}".into(),
    )
    .unwrap();
    add_subtitle(
        &mut p,
        Time::from_secs(3),
        Time::from_secs(3),
        "第二句\n两行字幕".into(),
    )
    .unwrap();
    let vep = out_dir.join("demo.vep");
    ve_core::save_project(&p, &vep).unwrap();
    let mp4 = out_dir.join("demo.mp4");
    let settings = ve_export::default_settings(&p, mp4.clone());
    let started = std::time::Instant::now();
    ve_export::export(&p, &settings, |_| {}, &AtomicBool::new(false)).unwrap();
    let r = ve_media::probe(&mp4).unwrap();
    println!(
        "工程 {}\n导出 {}（{:.2}s，用时 {:.2}s，视频 {:?}，音频 {:?}）",
        vep.display(),
        mp4.display(),
        r.duration.as_secs_f64(),
        started.elapsed().as_secs_f64(),
        r.video.map(|info| (info.width, info.height)),
        r.audio.map(|info| (info.sample_rate, info.channels))
    );
}
