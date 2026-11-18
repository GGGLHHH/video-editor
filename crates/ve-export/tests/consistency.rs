//! 预览和导出一致：同一个三层工程、同一时刻，预览合成出的画面和导出文件里那一帧要对得上。

#![expect(clippy::unwrap_used, clippy::panic, reason = "集成测试")]

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};
use ve_core::edit::{
    add_keyframe, add_track, append_asset, apply_transform_at, insert_asset_at, set_transform,
};
use ve_core::{Asset, AssetId, FrameRate, Project, Time, TrackKind, Transform};
use ve_engine::{AudioBackend, Engine, EngineConfig};
use ve_export::{Encoder, ExportSettings, export};
use ve_media::testing::{Fixture, fixture};
use ve_media::{ScaleTarget, VideoDecoder, VideoFrame};
use ve_render::RenderConfig;

const RATE: FrameRate = FrameRate::new(30, 1);

fn add(p: &mut Project, f: Fixture) -> AssetId {
    let path = fixture(f);
    let r = ve_media::probe(&path).unwrap();
    p.add_asset(Asset {
        id: AssetId(0),
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        path,
        duration: r.duration,
        video: r.video,
        audio: r.audio,
    })
}

/// 三层：灰度视频铺满；红色图片画中画在右下；彩条视频小窗在左上、70% 不透明。
fn three_layers(width: u32, height: u32) -> Project {
    let mut p = Project::new();
    p.settings.width = width;
    p.settings.height = height;
    p.settings.frame_rate = RATE;
    let gray = add(&mut p, Fixture::FrameIndexed30);
    let red = add(&mut p, Fixture::Image);
    let bars = add(&mut p, Fixture::VideoWithAudio);
    append_asset(&mut p, gray).unwrap();
    for (asset, t) in [
        (
            red,
            Transform {
                x: 0.3,
                y: 0.3,
                scale: 0.33,
                ..Transform::default()
            },
        ),
        (
            bars,
            Transform {
                x: -0.3,
                y: -0.3,
                scale: 0.25,
                opacity: 0.7,
                // 转过的图层：导出和预览也得一致
                rotation: 20.0,
                ..Transform::default()
            },
        ),
    ] {
        let track = add_track(&mut p, TrackKind::Video).unwrap();
        let clip = insert_asset_at(&mut p, asset, track, Time::ZERO).unwrap();
        set_transform(&mut p, clip, t).unwrap();
        if asset == bars {
            // 调过色的图层：导出和预览也得一致
            let adjust = ve_core::ColorAdjust {
                saturation: -0.6,
                temperature: 0.4,
                contrast: 0.2,
                ..ve_core::ColorAdjust::NONE
            };
            ve_core::edit::set_adjust(&mut p, clip, adjust).unwrap();
        }
    }
    p
}

/// 预览引擎在时刻 `t` 合成出的画面。
fn preview_frame(p: &Project, t: Time) -> Arc<VideoFrame> {
    let e = Engine::new(EngineConfig {
        audio: AudioBackend::Capture,
        render: RenderConfig { software: true },
        ..EngineConfig::default()
    })
    .unwrap();
    e.set_project(Arc::new(p.clone()));
    e.seek(t);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(u) = e.take_frame() {
            return u.frame.unwrap();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("预览没出帧");
}

/// 导出成文件，再解出时刻 `t` 的那一帧。
fn exported_frame(p: &Project, width: u32, height: u32, t: Time) -> VideoFrame {
    let dir = tempfile::tempdir().unwrap();
    let settings = ExportSettings {
        path: dir.path().join("out.mp4"),
        width,
        height,
        frame_rate: RATE,
        video_bitrate_kbps: 20_000,
        encoder: Encoder::X264,
        codec: ve_export::Codec::H264,
        render: RenderConfig { software: true },
    };
    export(p, &settings, |_| {}, &AtomicBool::new(false)).unwrap();
    let mut d = VideoDecoder::open(
        &settings.path,
        ScaleTarget {
            max_width: width,
            max_height: height,
        },
    )
    .unwrap();
    d.seek_to(t).unwrap()
}

fn psnr(a: &VideoFrame, b: &VideoFrame) -> f64 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let mut sum = 0.0;
    let mut count = 0.0;
    for (pa, pb) in a
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.data.as_chunks::<4>().0)
    {
        for c in 0..3 {
            let d = f64::from(pa[c]) - f64::from(pb[c]);
            sum += d * d;
            count += 1.0;
        }
    }
    let mse = sum / count;
    if mse == 0.0 {
        return 99.0;
    }
    10.0 * (255.0 * 255.0 / mse).log10()
}

/// 红色画中画（在右下四分之一）的外框（左、上、右、下，含）。
/// 按「红色比其他通道高出 100 以上」判定，取的是过渡带的中点：
/// 4:2:0 色度抽样会把颜色边缘抹开约 2 像素，用纯红的阈值会把边量窄。
fn red_box(f: &VideoFrame) -> (u32, u32, u32, u32) {
    let mut b = (u32::MAX, u32::MAX, 0, 0);
    for y in f.height / 2..f.height {
        for x in f.width / 2..f.width {
            let i = ((y * f.width + x) * 4) as usize;
            let (blue, green, red) = (f.data[i], f.data[i + 1], f.data[i + 2]);
            if i32::from(red) - i32::from(green.max(blue)) > 100 {
                b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
            }
        }
    }
    assert!(b.0 <= b.2, "没找到红色画中画");
    b
}

fn assert_box_close(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32), what: &str) {
    let d = [
        a.0.abs_diff(b.0),
        a.1.abs_diff(b.1),
        a.2.abs_diff(b.2),
        a.3.abs_diff(b.3),
    ];
    assert!(d.iter().all(|v| *v <= 1), "{what}：预览 {a:?}，导出 {b:?}");
}

#[test]
fn export_matches_preview_three_layers() {
    let p = three_layers(640, 360);
    let t = RATE.time_of_frame(30);
    let preview = preview_frame(&p, t);
    let exported = exported_frame(&p, 640, 360, t);
    let score = psnr(&preview, &exported);
    assert!(score >= 35.0, "PSNR {score:.1} dB");
    assert_box_close(red_box(&preview), red_box(&exported), "画中画边界");
}

#[test]
fn export_size_differs_from_project() {
    // 1280×720 的工程导出 640×360：画中画位置、大小按比例缩小
    let p = three_layers(1280, 720);
    let t = RATE.time_of_frame(30);
    let preview = red_box(&preview_frame(&p, t));
    let half = (preview.0 / 2, preview.1 / 2, preview.2 / 2, preview.3 / 2);
    let exported = red_box(&exported_frame(&p, 640, 360, t));
    assert_box_close(half, exported, "按比例换算后的画中画边界");
}

#[test]
fn export_matches_preview_mid_animation() {
    // 红色画中画：片段内 0 秒在 (0.3, 0.3)、缩放 0.33，2 秒移到 (0.1, 0.3)、缩放 0.2
    let mut p = three_layers(640, 360);
    // 红色画中画是第二个导入的素材（静态图）
    let red_asset = *p.assets.keys().nth(1).unwrap();
    let red = p
        .timeline
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.asset() == Some(red_asset))
        .unwrap()
        .id;
    add_keyframe(&mut p, red, Time::ZERO).unwrap();
    let later = Transform {
        x: 0.1,
        y: 0.3,
        scale: 0.2,
        ..Transform::default()
    };
    apply_transform_at(&mut p, red, Time::from_secs(2), later).unwrap();
    let t = RATE.time_of_frame(30);
    let preview = preview_frame(&p, t);
    let exported = exported_frame(&p, 640, 360, t);
    let score = psnr(&preview, &exported);
    assert!(score >= 35.0, "PSNR {score:.1} dB");
    let moving = red_box(&preview);
    assert_box_close(moving, red_box(&exported), "动画中途的画中画边界");
    // 确实动了：和第 0 帧的位置不同
    let start = red_box(&preview_frame(&p, Time::ZERO));
    assert!(
        moving.0 + 10 < start.0,
        "画中画应往左移：{start:?} → {moving:?}"
    );
}

#[test]
fn export_matches_preview_mid_transition() {
    // 两段紧挨着的片段（灰帧号视频 2 秒 + 彩条 2 秒），后一段挂 1 秒推近（缩放 + 渐显）
    let mut p = Project::new();
    p.settings.width = 640;
    p.settings.height = 360;
    p.settings.frame_rate = RATE;
    let gray = add(&mut p, Fixture::FrameIndexed30);
    let bars = add(&mut p, Fixture::VideoWithAudio);
    append_asset(&mut p, gray).unwrap();
    let second = append_asset(&mut p, bars).unwrap();
    ve_core::edit::set_transition(
        &mut p,
        second,
        Some(ve_core::Transition {
            kind: ve_core::TransitionKind::ZoomIn,
            duration: Time::from_secs(1),
        }),
    )
    .unwrap();
    // 切点后 0.2 秒：两段都在画面里
    let t = RATE.time_of_frame(66);
    assert_eq!(ve_core::compose::resolve_layers(&p, t).len(), 2);
    let preview = preview_frame(&p, t);
    let exported = exported_frame(&p, 640, 360, t);
    let score = psnr(&preview, &exported);
    assert!(score >= 35.0, "PSNR {score:.1} dB");
}
