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

/// 文件路径 → `file:///` 网址（斜杠统一；`%`、空格、`#`、`?` 转义，中文等原样）。
#[must_use]
pub fn file_url(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let s = s
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('?', "%3F");
    if s.starts_with('/') {
        format!("file://{s}")
    } else if s.get(1..2) == Some(":") {
        format!("file:///{s}")
    } else {
        // 相对路径（OTIO 也认）：原样写
        s
    }
}

/// 百分号转义还原（`%E7%B4%A0` 这类按 UTF-8 拼回去；不成对的 `%` 原样留着）。
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |c: u8| char::from(c).to_digit(16);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let pair = (b[i] == b'%')
            .then(|| Some((hex(*b.get(i + 1)?)?, hex(*b.get(i + 2)?)?)))
            .flatten();
        if let Some((h, l)) = pair {
            out.extend(u8::try_from(h * 16 + l).ok());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `file:///` 网址 → 文件路径（[`file_url`] 的反过来；也认 `file://localhost/…`）。
#[must_use]
pub fn url_path(url: &str) -> PathBuf {
    let Some(s) = url.strip_prefix("file://") else {
        return PathBuf::from(percent_decode(url));
    };
    let s = percent_decode(s.strip_prefix("localhost").unwrap_or(s));
    // Windows：`/C:/…` 去掉开头的斜杠
    let b = s.as_bytes();
    if b.len() > 2 && b[0] == b'/' && b[2] == b':' {
        PathBuf::from(&s[1..])
    } else {
        PathBuf::from(s)
    }
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
            // OTIO：片段占轨道的长度就是 source_range 的长度，变速（LinearTimeWarp）不改它
            "source_range": range(source_in, c.duration, rate),
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

/// 读一个 `RationalTime`：读不懂当 0，夹到 ±10 万小时以内（别让后面的加法溢出）。
fn time_of(v: &Value) -> Time {
    const LIMIT: f64 = 360_000_000.0;
    let rate = v["rate"].as_f64().filter(|r| *r > 0.0).unwrap_or(1.0);
    let secs = v["value"].as_f64().unwrap_or(0.0) / rate;
    Time::from_secs_f64(if secs.is_finite() {
        secs.clamp(-LIMIT, LIMIT)
    } else {
        0.0
    })
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
            // 片段占轨道的长度（负数当 0）
            let duration = time_of(&src["duration"]).max(Time::ZERO);
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
                    // 别家常按素材时间码记起点（比如 01:00:00:00）：减掉素材自己的起点
                    let media_start = time_of(&reference["available_range"]["start_time"]);
                    clips.push(ExClip {
                        path: url_path(url),
                        start: at,
                        duration,
                        source_in: source_in - media_start,
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
        mask_text: None,
        parent: None,
        audio_split: false,
        motion: crate::motion::ClipMotion::NONE,
        group: None,
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

/// 让导入的片段落在素材范围内：素材起点不早于 0，用到的素材不超过结尾（超出就截短）。
/// 截完没有长度了返回 `None`。
fn fit_to_media(mut clip: Clip, limit: Time) -> Option<Clip> {
    let ClipContent::Media { source_in, .. } = &mut clip.content else {
        return None;
    };
    *source_in = (*source_in).max(Time::ZERO);
    let room = limit - *source_in;
    if room <= Time::ZERO || clip.start < Time::ZERO {
        return None;
    }
    clip.duration = clip.duration.min(room.scaled(1.0 / f64::from(clip.speed)));
    // 两次取整可能多出一点
    while clip.duration > Time::ZERO && clip.source_in() + clip.source_span() > limit {
        clip.duration -= Time(1);
    }
    (clip.duration > Time::ZERO).then_some(clip)
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
        let usable = |a: &&Asset| match t.kind {
            TrackKind::Video => a.video.is_some(),
            _ => a.audio.is_some(),
        };
        let Some((&asset, a)) = ids.get(&c.path).zip(assets.get(&c.path).filter(usable)) else {
            skipped += 1;
            continue;
        };
        let id = p.ids.clip();
        let Some(clip) = fit_to_media(media_clip(id, asset, c), a.source_limit()) else {
            skipped += 1;
            continue;
        };
        let clip = std::sync::Arc::new(clip);
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
    fn urls_from_other_apps_decode_utf8_and_localhost() {
        assert_eq!(
            url_path("file:///C:/%E7%B4%A0%E6%9D%90/a%23b%20c.mp4"),
            PathBuf::from("C:/素材/a#b c.mp4")
        );
        assert_eq!(
            url_path("file://localhost/C:/v/a.mp4"),
            PathBuf::from("C:/v/a.mp4")
        );
        assert_eq!(
            url_path("file://localhost/home/u/a.mp4"),
            PathBuf::from("/home/u/a.mp4")
        );
        assert_eq!(
            url_path("a%2.mp4"),
            PathBuf::from("a%2.mp4"),
            "不成对的 % 原样"
        );
        assert_eq!(
            url_path(&file_url(Path::new("C:/a#1?.mp4"))),
            PathBuf::from("C:/a#1?.mp4")
        );
    }

    #[test]
    fn speed_keeps_the_otio_duration_as_the_length_on_the_track() {
        let (mut p, ids) = sample_project();
        crate::edit::set_speed(&mut p, ids.v2, 2.0).unwrap();
        let v: Value = serde_json::from_str(&to_otio(&p, "x")).unwrap();
        let clip = &v["tracks"]["children"][0]["children"][1];
        let rate = clip["source_range"]["duration"]["rate"].as_f64().unwrap();
        let frames = clip["source_range"]["duration"]["value"].as_f64().unwrap();
        // 3 秒素材 2 倍速：轨道上 1.5 秒
        assert!((frames / rate - 1.5).abs() < 1e-6, "{frames}/{rate}");
    }

    /// 一段别家软件写的 OTIO：一条视频轨，`children` 原样放进去。
    fn foreign(children: &Value) -> String {
        json!({
            "OTIO_SCHEMA": "Timeline.1",
            "tracks": {"OTIO_SCHEMA": "Stack.1", "children": [
                {"OTIO_SCHEMA": "Track.1", "kind": "Video", "children": children}
            ]}
        })
        .to_string()
    }

    fn foreign_clip(start: f64, duration: f64, media_start: f64, speed: f64) -> Value {
        let t = |v: f64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": 25.0, "value": v});
        json!({
            "OTIO_SCHEMA": "Clip.1",
            "source_range": {"start_time": t(start), "duration": t(duration)},
            "media_reference": {
                "OTIO_SCHEMA": "ExternalReference.1",
                "target_url": "clip.mp4",
                "available_range": {"start_time": t(media_start), "duration": t(1e6)}
            },
            "effects": [{"OTIO_SCHEMA": "LinearTimeWarp.1", "time_scalar": speed}]
        })
    }

    #[test]
    fn foreign_timelines_import_into_a_valid_project() {
        let (p, _) = sample_project();
        let assets: BTreeMap<PathBuf, Asset> = p
            .assets
            .values()
            .map(|a| (a.path.clone(), a.clone()))
            .collect();
        let gap = |d: f64| {
            json!({"OTIO_SCHEMA": "Gap.1", "source_range": {
                "start_time": {"rate": 25.0, "value": 0.0},
                "duration": {"rate": 25.0, "value": d}}})
        };
        let text = foreign(&json!([
            // 按时间码记的起点：素材自己从 01:00:00:00 开始，片段从素材第 1 秒用 2 秒
            foreign_clip(90_025.0, 50.0, 90_000.0, 1.0),
            // 负数空隙当 0
            gap(-500.0),
            // 用到素材结尾之后：截短
            foreign_clip(225.0, 100.0, 0.0, 1.0),
            // 0.05 倍速被夹到 0.1：素材不够用，截短
            foreign_clip(0.0, 100.0, 0.0, 0.05),
            // 起点在素材之后：跳过
            foreign_clip(1000.0, 10.0, 0.0, 1.0),
            // 大得离谱的空隙不溢出
            gap(1e300),
        ]));
        let tracks = from_otio(&text).unwrap();
        let c = &tracks[0].clips;
        assert_eq!(c[0].source_in, Time::from_secs(1), "减掉了素材起点");
        assert_eq!(c[1].start, Time::from_secs(2), "负数空隙不往回走");
        let (q, skipped) = build_project(&tracks, &assets, p.settings.frame_rate);
        q.timeline.check_invariants(&q.assets).unwrap();
        assert_eq!(skipped, 1);
        let main = crate::magnet::main_track(&q.timeline).unwrap();
        let clips = &q.timeline.track(main).unwrap().clips;
        assert_eq!(clips.len(), 3);
        assert_eq!(
            clips[1].duration,
            Time::from_secs(1),
            "素材 10 秒，从第 9 秒起只剩 1 秒"
        );
        assert!((clips[2].speed - 0.1).abs() < 1e-6);
        assert_eq!(clips[2].start, Time::from_secs(6));
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
