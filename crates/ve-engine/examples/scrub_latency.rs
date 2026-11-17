//! 拖动预览延迟：随机定位 N 次，测 `seek()` 到新帧可取的时间。
//!
//! 用法：`cargo run --release -p ve-engine --example scrub_latency -- <视频文件> [次数] [层数]`

#![expect(
    clippy::print_stdout,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "命令行测量工具"
)]

use std::sync::Arc;
use std::time::{Duration, Instant};
use ve_core::{Asset, AssetId, Project, Time};
use ve_engine::{AudioBackend, Engine, EngineConfig};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("视频文件"));
    let count: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(50);
    let layers: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let r = ve_media::probe(&path).unwrap();
    let mut p = Project::new();
    let id = p.add_asset(Asset {
        id: AssetId(0),
        name: "v".into(),
        path,
        duration: r.duration,
        video: r.video,
        audio: r.audio,
    });
    ve_core::bench::stack_layers(&mut p, id, layers, r.duration).unwrap();
    let duration = p.timeline.duration();
    let engine = Engine::new(EngineConfig {
        audio: AudioBackend::Null,
        ..EngineConfig::default()
    })
    .unwrap();
    engine.set_project(Arc::new(p));
    // 等第一帧，排除打开解码器的开销
    while engine.take_frame().is_none() {
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut seed = 0x2545_f491_u64;
    let mut ms = Vec::new();
    for _ in 0..count {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let t = Time(i64::try_from(seed % u64::try_from(duration.0).unwrap()).unwrap());
        let start = Instant::now();
        engine.seek(t);
        loop {
            if let Some(u) = engine.take_frame()
                && u.time == t
            {
                break;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    ms.sort_by(f64::total_cmp);
    let pct = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    println!(
        "layers={layers} seeks={count} p50={:.1}ms p90={:.1}ms p99={:.1}ms max={:.1}ms",
        pct(0.5),
        pct(0.9),
        pct(0.99),
        ms[ms.len() - 1]
    );
}
