//! 时间轴解析：某一时刻该显示哪个素材的哪一帧、某段时间该播哪些音频。

use ve_core::{AssetId, ClipId, Project, Time, TrackKind};

/// 某一时刻的画面来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoSource {
    /// 素材。
    pub asset: AssetId,
    /// 素材内时间。
    pub source_time: Time,
    /// 片段。
    pub clip: ClipId,
}

/// 一段要播放的音频。
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSegment {
    /// 素材。
    pub asset: AssetId,
    /// 片段。
    pub clip: ClipId,
    /// 素材内起点。
    pub source_start: Time,
    /// 时间轴上起点。
    pub timeline_start: Time,
    /// 长度。
    pub duration: Time,
    /// 线性增益。
    pub gain: f32,
    /// 片段淡入长度。
    pub fade_in: Time,
    /// 片段淡出长度。
    pub fade_out: Time,
    /// 片段在时间轴上的起点（淡入从这里算）。
    pub clip_start: Time,
    /// 片段在时间轴上的终点（淡出到这里结束）。
    pub clip_end: Time,
}

/// dB 转线性增益。
#[must_use]
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// 时刻 `t` 的画面：最上面一条在 `t` 处有片段的视频轨。
#[must_use]
pub fn resolve_video(project: &Project, t: Time) -> Option<VideoSource> {
    project
        .timeline
        .tracks
        .iter()
        .filter(|tr| tr.kind == TrackKind::Video)
        .find_map(|tr| {
            let clip = tr.clip_at(t)?;
            Some(VideoSource {
                asset: clip.asset()?,
                source_time: clip.source_in() + (t - clip.start),
                clip: clip.id,
            })
        })
}

/// 时间轴区间 `[from, from + len)` 内要播的所有音频段（视频轨的原声和音频轨）。
#[must_use]
pub fn resolve_audio(project: &Project, from: Time, len: Time) -> Vec<AudioSegment> {
    let to = from + len;
    let mut out = Vec::new();
    for track in &project.timeline.tracks {
        if !matches!(track.kind, TrackKind::Video | TrackKind::Audio) {
            continue;
        }
        let first = track.clips.partition_point(|c| c.end() <= from);
        for clip in track.clips[first..].iter().take_while(|c| c.start < to) {
            let Some(asset) = clip.asset() else { continue };
            if project.assets.get(&asset).is_none_or(|a| a.audio.is_none()) {
                continue;
            }
            let start = clip.start.max(from);
            let end = clip.end().min(to);
            out.push(AudioSegment {
                asset,
                clip: clip.id,
                source_start: clip.source_in() + (start - clip.start),
                timeline_start: start,
                duration: end - start,
                gain: db_to_gain(clip.gain_db),
                fade_in: clip.fade_in,
                fade_out: clip.fade_out,
                clip_start: clip.start,
                clip_end: clip.end(),
            });
        }
    }
    out
}

/// 时刻 `t` 要显示的字幕文字。
#[must_use]
pub fn resolve_subtitles(project: &Project, t: Time) -> Vec<&str> {
    project
        .timeline
        .tracks
        .iter()
        .filter(|tr| tr.kind == TrackKind::Subtitle)
        .filter_map(|tr| tr.clip_at(t).and_then(|c| c.subtitle_text()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::edit::{set_fades, set_gain};
    use ve_core::testing::sample_project;
    use ve_core::{Asset, AssetId, Time, TrackKind, VideoInfo};

    fn secs(s: i64) -> Time {
        Time::from_secs(s)
    }

    #[test]
    fn resolve_maps_source_offset() {
        let (p, ids) = sample_project();
        let v = resolve_video(&p, Time::from_millis(6500)).unwrap();
        assert_eq!((v.asset, v.clip), (ids.video_asset, ids.v2));
        // v2 在时间轴 5 秒处、从素材 5 秒开始 → 6.5 秒对应素材 6.5 秒
        assert_eq!(v.source_time, Time::from_millis(6500));
        let first = resolve_video(&p, secs(1)).unwrap();
        assert_eq!((first.clip, first.source_time), (ids.v1, secs(1)));
    }

    #[test]
    fn resolve_gap_returns_none() {
        let (p, _) = sample_project();
        assert!(resolve_video(&p, secs(8)).is_none());
        assert!(resolve_video(&p, secs(-1)).is_none());
    }

    #[test]
    fn audio_segments_cover_both_tracks() {
        let (p, ids) = sample_project();
        let segs = resolve_audio(&p, Time::from_millis(4500), secs(1));
        let mut found: Vec<_> = segs
            .iter()
            .map(|s| (s.clip, s.timeline_start, s.duration, s.source_start))
            .collect();
        found.sort_by_key(|f| f.1);
        assert_eq!(segs.len(), 3, "{segs:?}");
        // v1 的 4.5–5、v2 的 5–5.5、a1 的 4.5–5.5
        assert!(found.contains(&(
            ids.v1,
            Time::from_millis(4500),
            Time::from_millis(500),
            Time::from_millis(4500)
        )));
        assert!(found.contains(&(ids.v2, secs(5), Time::from_millis(500), secs(5))));
        assert!(found.contains(&(
            ids.a1,
            Time::from_millis(4500),
            secs(1),
            Time::from_millis(4500)
        )));
    }

    #[test]
    fn audio_segment_carries_gain_and_fades() {
        let (mut p, ids) = sample_project();
        set_gain(&mut p, ids.a1, -6.0).unwrap();
        set_fades(&mut p, ids.a1, secs(1), secs(2)).unwrap();
        let seg = resolve_audio(&p, Time::ZERO, secs(10))
            .into_iter()
            .find(|s| s.clip == ids.a1)
            .unwrap();
        assert!((seg.gain - 0.501).abs() < 0.01, "{}", seg.gain);
        assert_eq!(
            (seg.fade_in, seg.fade_out, seg.clip_start, seg.clip_end),
            (secs(1), secs(2), Time::ZERO, secs(8))
        );
    }

    #[test]
    fn mix_clip_without_audio_is_silent() {
        let (mut p, ids) = sample_project();
        let silent = p.add_asset(Asset {
            id: AssetId(0),
            path: "silent.mp4".into(),
            name: "silent.mp4".into(),
            duration: secs(10),
            video: Some(VideoInfo {
                still: false,
                ..ve_core::testing::video_info()
            }),
            audio: None,
        });
        let v = p.timeline.track_of_kind_mut(TrackKind::Video).unwrap();
        let mut clip = (*v.clips[0]).clone();
        clip.content = ve_core::ClipContent::Media {
            asset: silent,
            source_in: Time::ZERO,
        };
        v.clips[0] = std::sync::Arc::new(clip);
        let segs = resolve_audio(&p, Time::ZERO, secs(1));
        assert!(segs.iter().all(|s| s.clip != ids.v1), "{segs:?}");
    }

    #[test]
    fn subtitles_at_time() {
        let (p, _) = sample_project();
        assert_eq!(resolve_subtitles(&p, Time::from_millis(1500)), vec!["你好"]);
        assert_eq!(resolve_subtitles(&p, secs(3)), Vec::<&str>::new());
    }
}
