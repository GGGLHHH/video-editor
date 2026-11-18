//! 测试辅助：示例工程（供本 crate 和下游 crate 的测试使用，需 `testing` 特性）。

use crate::{
    Asset, AssetId, AudioInfo, Clip, ClipContent, ClipId, FrameRate, Project, Time, TrackId,
    TrackKind, VideoInfo,
};
use std::sync::Arc;

/// 示例工程里各对象的 id。
#[derive(Clone, Copy, Debug)]
pub struct SampleIds {
    /// 10 秒 1080p30 带音频的视频素材 `clip.mp4`。
    pub video_asset: AssetId,
    /// 20 秒纯音频素材 `bgm.mp3`。
    pub audio_asset: AssetId,
    /// V1 上 0–5 秒，素材 0–5 秒。
    pub v1: ClipId,
    /// V1 上 5–8 秒，素材 5–8 秒。
    pub v2: ClipId,
    /// A1 上 0–8 秒。
    pub a1: ClipId,
    /// S1 上 1–3 秒，「你好」。
    pub s1: ClipId,
    /// 视频轨。
    pub video_track: TrackId,
    /// 音频轨。
    pub audio_track: TrackId,
    /// 字幕轨。
    pub subtitle_track: TrackId,
}

/// 片段：从素材开头取，增益 0 dB，无淡入淡出。
#[must_use]
pub const fn media_clip(id: ClipId, asset: AssetId, start: Time, duration: Time) -> Clip {
    Clip {
        id,
        start,
        duration,
        content: ClipContent::Media {
            asset,
            source_in: Time::ZERO,
        },
        gain_db: 0.0,
        fade_in: Time::ZERO,
        fade_out: Time::ZERO,
        transform: crate::Transform::DEFAULT,
        animation: crate::Animation::new(),
        adjust: crate::ColorAdjust::NONE,
        disabled: false,
        transition_in: None,
        speed: 1.0,
        keep_pitch: true,
    }
}

/// 视频素材的流信息（1080p30）。
#[must_use]
pub const fn video_info() -> VideoInfo {
    VideoInfo {
        width: 1920,
        height: 1080,
        frame_rate: FrameRate::new(30, 1),
        still: false,
    }
}

/// 音频流信息（48k 立体声）。
#[must_use]
pub const fn audio_info() -> AudioInfo {
    AudioInfo {
        sample_rate: 48_000,
        channels: 2,
    }
}

/// 构造示例工程，见 [`SampleIds`]。
#[must_use]
pub fn sample_project() -> (Project, SampleIds) {
    let mut p = Project::new();
    let video_asset = p.add_asset(Asset {
        id: AssetId(0),
        path: "clip.mp4".into(),
        name: "clip.mp4".into(),
        duration: Time::from_secs(10),
        video: Some(video_info()),
        audio: Some(audio_info()),
    });
    let audio_asset = p.add_asset(Asset {
        id: AssetId(0),
        path: "bgm.mp3".into(),
        name: "bgm.mp3".into(),
        duration: Time::from_secs(20),
        video: None,
        audio: Some(audio_info()),
    });
    let (v1, v2, a1, s1) = (p.ids.clip(), p.ids.clip(), p.ids.clip(), p.ids.clip());

    let mut second = media_clip(v2, video_asset, Time::from_secs(5), Time::from_secs(3));
    second.content = ClipContent::Media {
        asset: video_asset,
        source_in: Time::from_secs(5),
    };
    let subtitle = Clip {
        id: s1,
        start: Time::from_secs(1),
        duration: Time::from_secs(2),
        content: ClipContent::Subtitle {
            text: "你好".into(),
        },
        gain_db: 0.0,
        fade_in: Time::ZERO,
        fade_out: Time::ZERO,
        transform: crate::Transform::DEFAULT,
        animation: crate::Animation::new(),
        adjust: crate::ColorAdjust::NONE,
        disabled: false,
        transition_in: None,
        speed: 1.0,
        keep_pitch: true,
    };
    // Project::new() 的轨道顺序固定为 字幕、视频、音频
    let [subs, video, audio] = &mut p.timeline.tracks[..] else {
        return (p, SampleIds::empty());
    };
    debug_assert_eq!(
        (video.kind, audio.kind, subs.kind),
        (TrackKind::Video, TrackKind::Audio, TrackKind::Subtitle)
    );
    video.clips = vec![
        Arc::new(media_clip(v1, video_asset, Time::ZERO, Time::from_secs(5))),
        Arc::new(second),
    ];
    audio.clips = vec![Arc::new(media_clip(
        a1,
        audio_asset,
        Time::ZERO,
        Time::from_secs(8),
    ))];
    subs.clips = vec![Arc::new(subtitle)];
    let ids = SampleIds {
        video_asset,
        audio_asset,
        v1,
        v2,
        a1,
        s1,
        video_track: video.id,
        audio_track: audio.id,
        subtitle_track: subs.id,
    };
    (p, ids)
}

impl SampleIds {
    const fn empty() -> Self {
        Self {
            video_asset: AssetId(0),
            audio_asset: AssetId(0),
            v1: ClipId(0),
            v2: ClipId(0),
            a1: ClipId(0),
            s1: ClipId(0),
            video_track: TrackId(0),
            audio_track: TrackId(0),
            subtitle_track: TrackId(0),
        }
    }
}
