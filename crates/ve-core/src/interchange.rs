//! 工程交换：和 Premiere、达芬奇、Final Cut 互通。
//!
//! - `OpenTimelineIO`（`.otio`，JSON）：导出视频轨、音频轨（片段、空隙、变速、标记），也能导入；
//!   别家软件都能读写，交换「剪辑结构」用它。
//! - EDL（CMX3600）：只导出主轨，老式调色 / 套底流程用。
//!
//! 字幕、调色、特效、转场这些各家不通用，不导出；导入时只认素材片段。

use crate::{Asset, AssetId, Clip, ClipContent, ClipId, FrameRate, Project, Time, TrackKind};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// 导入时读出来的一段。
#[derive(Debug, Clone, PartialEq)]
pub struct ExClip {
    /// 素材文件。
    pub path: PathBuf,
    /// 时间轴上的起点。
    pub start: Time,
    /// 时间轴上的长度。
    pub duration: Time,
    /// 素材内起点。
    pub source_in: Time,
    /// 变速倍数。
    pub speed: f32,
}

/// 导入时读出来的一条轨道。
#[derive(Debug, Clone, PartialEq)]
pub struct ExTrack {
    /// 视频还是音频。
    pub kind: TrackKind,
    /// 片段，按起点排好。
    pub clips: Vec<ExClip>,
}

/// 读 OTIO 出错。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InterchangeError {
    /// 不是 JSON，或者不是 OTIO 的时间轴。
    #[error("不是 OpenTimelineIO 时间轴：{0}")]
    NotOtio(String),
}

fn rational(t: Time, rate: f64) -> Value {
    json!({
        "OTIO_SCHEMA": "RationalTime.1",
        "rate": rate,
        "value": (t.as_secs_f64() * rate * 1000.0).round() / 1000.0,
    })
}

fn range(start: Time, duration: Time, rate: f64) -> Value {
    json!({
        "OTIO_SCHEMA": "TimeRange.1",
        "start_time": rational(start, rate),
        "duration": rational(duration, rate),
    })
}

/// 文件路径 → `file:///` 网址（斜杠统一、空格和 % 转义）。
#[must_use]
pub fn file_url(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let s = s.replace('%', "%25").replace(' ', "%20");
    if s.starts_with('/') {
        format!("file://{s}")
    } else if s.get(1..2) == Some(":") {
        format!("file:///{s}")
    } else {
        // 相对路径（OTIO 也认）：原样写
        s
    }
}

/// `file:///` 网址 → 文件路径（[`file_url`] 的反过来）。
#[must_use]
pub fn url_path(url: &str) -> PathBuf {
    let Some(s) = url.strip_prefix("file://") else {
        return PathBuf::from(url.replace("%20", " ").replace("%25", "%"));
    };
    // Windows：`/C:/…` 去掉开头的斜杠
    let s = if s.len() > 2 && s.as_bytes()[0] == b'/' && s.as_bytes()[2] == b':' {
        &s[1..]
    } else {
        s
    };
    PathBuf::from(s.replace("%20", " ").replace("%25", "%"))
}

fn track_json(
    project: &Project,
    kind: TrackKind,
    name: &str,
    clips: &[std::sync::Arc<Clip>],
) -> Value {
    let rate = project.settings.frame_rate.as_f64();
    let mut children = Vec::new();
    let mut at = Time::ZERO;
    for c in clips {
        let ClipContent::Media { asset, source_in } = c.content else {
            continue;
        };
        let Some(a) = project.assets.get(&asset) else {
            continue;
        };
        if c.start > at {
            children.push(json!({
                "OTIO_SCHEMA": "Gap.1",
                "name": "",
                "source_range": range(Time::ZERO, c.start - at, rate),
            }));
        }
        let mut clip = json!({
            "OTIO_SCHEMA": "Clip.2",
            "name": a.name,
            "source_range": range(source_in, c.source_span(), rate),
            "media_references": {
                "DEFAULT_MEDIA": {
                    "OTIO_SCHEMA": "ExternalReference.1",
                    "target_url": file_url(&a.path),
                }
            },
            "active_media_reference_key": "DEFAULT_MEDIA",
            "effects": [],
            "markers": [],
            "enabled": c.enabled(),
        });
        #[expect(clippy::float_cmp, reason = "原速是精确的 1.0")]
        if c.speed != 1.0 {
            clip["effects"] = json!([{
                "OTIO_SCHEMA": "LinearTimeWarp.1",
                "name": "",
                "effect_name": "LinearTimeWarp",
                "time_scalar": c.speed,
            }]);
        }
        children.push(clip);
        at = c.end();
    }
    json!({
        "OTIO_SCHEMA": "Track.1",
        "name": name,
        "kind": if kind == TrackKind::Audio { "Audio" } else { "Video" },
        "children": children,
        "markers": [],
        "effects": [],
    })
}

/// 导出成 `OpenTimelineIO`（`Timeline.1`）。轨道从下往上：视频轨在前（最下面的主轨第一），音频轨在后。
#[must_use]
pub fn to_otio(project: &Project, name: &str) -> String {
    let rate = project.settings.frame_rate.as_f64();
    let mut tracks = Vec::new();
    for (i, t) in project.timeline.video_tracks_bottom_up().enumerate() {
        tracks.push(track_json(
            project,
            TrackKind::Video,
            &format!("V{}", i + 1),
            &t.clips,
        ));
    }
    for (i, t) in project
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .enumerate()
    {
        tracks.push(track_json(
            project,
            TrackKind::Audio,
            &format!("A{}", i + 1),
            &t.clips,
        ));
    }
    let markers: Vec<Value> = project
        .timeline
        .markers
        .iter()
        .map(|&m| {
            json!({
                "OTIO_SCHEMA": "Marker.2",
                "name": "",
                "color": "RED",
                "marked_range": range(m, Time::ZERO, rate),
            })
        })
        .collect();
    let timeline = json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": name,
        "global_start_time": rational(Time::ZERO, rate),
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "children": tracks,
            "markers": markers,
            "effects": [],
        },
    });
    serde_json::to_string_pretty(&timeline).unwrap_or_default()
}

fn time_of(v: &Value) -> Time {
    let rate = v["rate"].as_f64().filter(|r| *r > 0.0).unwrap_or(1.0);
    Time::from_secs_f64(v["value"].as_f64().unwrap_or(0.0) / rate)
}

/// 读 `OpenTimelineIO`：只认视频 / 音频轨上的外部素材片段（和空隙），变速按 `LinearTimeWarp`。
///
/// # Errors
/// 不是 JSON，或者顶层不是 `Timeline`。
pub fn from_otio(text: &str) -> Result<Vec<ExTrack>, InterchangeError> {
    let v: Value =
        serde_json::from_str(text).map_err(|e| InterchangeError::NotOtio(e.to_string()))?;
    let schema = v["OTIO_SCHEMA"].as_str().unwrap_or_default();
    if !schema.starts_with("Timeline.") {
        return Err(InterchangeError::NotOtio(format!("顶层是 {schema}")));
    }
    let mut out = Vec::new();
    for t in v["tracks"]["children"].as_array().into_iter().flatten() {
        if !t["OTIO_SCHEMA"]
            .as_str()
            .unwrap_or_default()
            .starts_with("Track.")
        {
            continue;
        }
        let kind = if t["kind"] == "Audio" {
            TrackKind::Audio
        } else {
            TrackKind::Video
        };
        let mut at = Time::ZERO;
        let mut clips = Vec::new();
        for c in t["children"].as_array().into_iter().flatten() {
            let kind_of = c["OTIO_SCHEMA"].as_str().unwrap_or_default();
            let src = &c["source_range"];
            let source_in = time_of(&src["start_time"]);
            let span = time_of(&src["duration"]);
            #[expect(clippy::cast_possible_truncation, reason = "倍数 0.1–10")]
            let speed = c["effects"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|e| {
                    e["OTIO_SCHEMA"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("LinearTimeWarp."))
                })
                .and_then(|e| e["time_scalar"].as_f64())
                .filter(|s| *s > 0.0)
                .unwrap_or(1.0) as f32;
            let duration = span.scaled(1.0 / f64::from(speed));
            if kind_of.starts_with("Clip.") {
                let key = c["active_media_reference_key"]
                    .as_str()
                    .unwrap_or("DEFAULT_MEDIA");
                let reference = if c["media_references"].is_object() {
                    &c["media_references"][key]
                } else {
                    &c["media_reference"]
                };
                if let Some(url) = reference["target_url"].as_str() {
                    clips.push(ExClip {
                        path: url_path(url),
                        start: at,
                        duration,
                        source_in,
                        speed,
                    });
                }
            }
            at += duration;
        }
        out.push(ExTrack { kind, clips });
    }
    Ok(out)
}

/// 一段素材片段（默认属性）。
fn media_clip(id: ClipId, asset: AssetId, c: &ExClip) -> Clip {
    Clip {
        id,
        start: c.start,
        duration: c.duration,
        content: ClipContent::Media {
            asset,
            source_in: c.source_in,
        },
        gain_db: 0.0,
        fade_in: Time::ZERO,
        fade_out: Time::ZERO,
        transform: crate::Transform::default(),
        animation: crate::Animation::default(),
        adjust: crate::ColorAdjust::NONE,
        disabled: false,
        transition_in: None,
        speed: c.speed.clamp(
            crate::timeline::SPEED_RANGE.0,
            crate::timeline::SPEED_RANGE.1,
        ),
        keep_pitch: true,
        label: None,
        look: crate::Look::NONE,
        reversed: None,
        curve: None,
        grade: None,
        audio_fx: crate::AudioFx::NONE,
        frame_blend: false,
        stabilized: None,
        input_space: None,
    }
}

/// 按导入的轨道建工程：`assets` 是探测好的素材（按路径；探测不了的片段跳过）。
/// 视频轨的第一条是主轨（最下面），音频轨照原样往下排。返回工程和跳过的片段数。
#[must_use]
pub fn build_project(
    tracks: &[ExTrack],
    assets: &BTreeMap<PathBuf, Asset>,
    rate: FrameRate,
) -> (Project, usize) {
    let mut p = Project::new();
    p.settings.frame_rate = rate;
    let mut ids: BTreeMap<PathBuf, AssetId> = BTreeMap::new();
    for (path, a) in assets {
        ids.insert(path.clone(), p.add_asset(a.clone()));
    }
    // 第一条视频素材的大小当画布
    if let Some(v) = assets.values().find_map(|a| a.video.filter(|v| !v.still)) {
        p.settings.width = (v.width & !1).max(2);
        p.settings.height = (v.height & !1).max(2);
    }
    let mut skipped = 0;
    for (n_video, t) in tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .enumerate()
    {
        let track = if n_video == 0 {
            crate::magnet::main_track(&p.timeline)
        } else {
            crate::edit::add_track(&mut p, TrackKind::Video).ok()
        };
        skipped += fill(&mut p, track, t, &ids, assets);
    }
    for (n_audio, t) in tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .enumerate()
    {
        let track = if n_audio == 0 {
            p.timeline.track_of_kind(TrackKind::Audio).map(|a| a.id)
        } else {
            crate::edit::add_track(&mut p, TrackKind::Audio).ok()
        };
        skipped += fill(&mut p, track, t, &ids, assets);
    }
    (p, skipped)
}

/// 把一条导入的轨道放进 `track`；返回跳过的片段数。
fn fill(
    p: &mut Project,
    track: Option<crate::TrackId>,
    t: &ExTrack,
    ids: &BTreeMap<PathBuf, AssetId>,
    assets: &BTreeMap<PathBuf, Asset>,
) -> usize {
    let Some(track) = track else {
        return t.clips.len();
    };
    let mut skipped = 0;
    for c in &t.clips {
        let usable = assets.get(&c.path).is_some_and(|a| match t.kind {
            TrackKind::Video => a.video.is_some(),
            _ => a.audio.is_some(),
        });
        let Some(&asset) = ids
            .get(&c.path)
            .filter(|_| usable && c.duration > Time::ZERO)
        else {
            skipped += 1;
            continue;
        };
        let id = p.ids.clip();
        let clip = std::sync::Arc::new(media_clip(id, asset, c));
        let Some(tr) = p.timeline.track_mut(track) else {
            skipped += 1;
            continue;
        };
        if tr.is_free(c.start, c.start + c.duration, &[]) {
            tr.insert_sorted(clip);
        } else {
            skipped += 1;
        }
    }
    skipped
}

/// 导出 EDL（CMX3600）：主轨上的素材片段，一段一条，时间码按工程帧率（非丢帧）。
#[must_use]
pub fn to_edl(project: &Project, title: &str) -> String {
    let rate = project.settings.frame_rate;
    let tc = |t: Time| crate::format_timecode(t, rate);
    let mut s = String::new();
    let _ = writeln!(s, "TITLE: {title}");
    let _ = writeln!(s, "FCM: NON-DROP FRAME");
    let _ = writeln!(s);
    let main = crate::magnet::main_track(&project.timeline);
    let clips = main
        .and_then(|id| project.timeline.track(id))
        .map(|t| t.clips.as_slice())
        .unwrap_or_default();
    let mut n = 0;
    for c in clips {
        let ClipContent::Media { asset, source_in } = c.content else {
            continue;
        };
        let Some(a) = project.assets.get(&asset) else {
            continue;
        };
        n += 1;
        let _ = writeln!(
            s,
            "{n:03}  AX       V     C        {} {} {} {}",
            tc(source_in),
            tc(source_in + c.source_span()),
            tc(c.start),
            tc(c.end())
        );
        let _ = writeln!(s, "* FROM CLIP NAME: {}", a.name);
        #[expect(clippy::float_cmp, reason = "原速是精确的 1.0")]
        if c.speed != 1.0 {
            let _ = writeln!(
                s,
                "M2   AX       {:05.1}                {}",
                rate.as_f64() * f64::from(c.speed),
                tc(source_in)
            );
        }
        let _ = writeln!(s);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::sample_project;

    #[test]
    fn urls_round_trip_windows_and_unix_paths() {
        for p in [r"C:\素材\a b.mp4", "/home/u/100%.mov"] {
            let url = file_url(Path::new(p));
            assert!(url.starts_with("file:///"), "{url}");
            assert_eq!(
                url_path(&file_url(Path::new("相对/a.mp4"))),
                PathBuf::from("相对/a.mp4")
            );
            assert!(!url.contains(' '));
            assert_eq!(url_path(&url), PathBuf::from(p.replace('\\', "/")));
        }
    }

    #[test]
    fn otio_round_trips_positions_sources_gaps_and_speed() {
        let (mut p, ids) = sample_project();
        // v2 挪到 6 秒，前面留 1 秒空隙；变速 2 倍
        crate::edit::move_clip(&mut p, ids.v2, ids.video_track, Time::from_secs(6)).unwrap();
        crate::edit::set_speed(&mut p, ids.v2, 2.0).unwrap();
        p.timeline.toggle_marker(Time::from_secs(2));
        let text = to_otio(&p, "测试");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["OTIO_SCHEMA"], "Timeline.1");
        assert_eq!(v["tracks"]["markers"].as_array().unwrap().len(), 1);
        let tracks = from_otio(&text).unwrap();
        let video: Vec<&ExTrack> = tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video)
            .collect();
        assert_eq!(video.len(), 1);
        let clips = &video[0].clips;
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].start, Time::ZERO);
        assert_eq!(clips[0].duration, Time::from_secs(5));
        assert_eq!(clips[1].start, Time::from_secs(6), "空隙保留");
        assert_eq!(clips[1].source_in, Time::from_secs(5));
        assert!((clips[1].speed - 2.0).abs() < 1e-6);
        assert_eq!(clips[1].duration, Time::from_millis(1500));
        assert_eq!(clips[1].path, PathBuf::from("clip.mp4"));
        let audio: Vec<&ExTrack> = tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .collect();
        assert_eq!(audio[0].clips[0].duration, Time::from_secs(8));
    }

    #[test]
    fn importing_builds_tracks_and_skips_what_it_cannot_place() {
        let (p, _) = sample_project();
        let tracks = from_otio(&to_otio(&p, "x")).unwrap();
        let assets: BTreeMap<PathBuf, Asset> = p
            .assets
            .values()
            .map(|a| (a.path.clone(), a.clone()))
            .collect();
        let (q, skipped) = build_project(&tracks, &assets, p.settings.frame_rate);
        assert_eq!(skipped, 0);
        q.timeline.check_invariants(&q.assets).unwrap();
        let count = |pr: &Project, k: TrackKind| {
            pr.timeline
                .tracks
                .iter()
                .filter(|t| t.kind == k)
                .map(|t| t.clips.len())
                .sum::<usize>()
        };
        assert_eq!(count(&q, TrackKind::Video), 2);
        assert_eq!(count(&q, TrackKind::Audio), 1);
        // 找不到的素材跳过
        let (_, missing) = build_project(&tracks, &BTreeMap::new(), p.settings.frame_rate);
        assert_eq!(missing, 3);
        assert!(from_otio("{}").is_err());
        assert!(from_otio("不是 json").is_err());
    }

    #[test]
    fn edl_lists_the_main_track_with_timecodes() {
        let (p, _) = sample_project();
        let edl = to_edl(&p, "测试");
        assert!(
            edl.starts_with("TITLE: 测试\nFCM: NON-DROP FRAME\n"),
            "{edl}"
        );
        assert!(
            edl.contains(
                "001  AX       V     C        00:00:00:00 00:00:05:00 00:00:00:00 00:00:05:00"
            ),
            "{edl}"
        );
        assert!(
            edl.contains(
                "002  AX       V     C        00:00:05:00 00:00:08:00 00:00:05:00 00:00:08:00"
            ),
            "{edl}"
        );
        assert!(edl.contains("* FROM CLIP NAME: clip.mp4"));
    }
}
