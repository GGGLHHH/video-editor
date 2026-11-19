//! 时间轴解析：某一时刻该显示哪个素材的哪一帧、某段时间该播哪些音频。

pub use ve_core::compose::{LayerSource, resolve_layers};
use ve_core::{AssetId, ClipId, Project, ProjectSettings, Time, TrackKind};
use ve_media::ScaleTarget;

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
    /// 淡入长度。
    pub fade_in: Time,
    /// 淡出长度。
    pub fade_out: Time,
    /// 这个片段的声音在时间轴上开始放的时刻（淡入从这里算；转场处会早于片段起点）。
    pub sound_start: Time,
    /// 这个片段的声音放完的时刻（淡出到这里结束；转场处会晚于片段终点）。
    pub sound_end: Time,
    /// 变速倍数（素材时间 / 时间轴时间）。
    pub speed: f32,
    /// 变速时保持音调。
    pub keep_pitch: bool,
}

/// dB 转线性增益。
#[must_use]
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// 时刻 `t` 的画面：最上面一条在 `t` 处有片段的视频轨。
#[must_use]
pub fn resolve_video(project: &Project, t: Time) -> Option<VideoSource> {
    resolve_layers(project, t).pop().map(|l| VideoSource {
        asset: l.asset,
        source_time: l.source_time,
        clip: l.clip,
    })
}

/// 预览画布尺寸：工程尺寸等比缩进预览上限（偶数）。
#[must_use]
pub fn preview_canvas(settings: &ProjectSettings, max: ScaleTarget) -> (u32, u32) {
    ve_media::fit_size(settings.width, settings.height, max)
}

/// 时间轴区间 `[from, from + len)` 内要播的所有音频段（视频轨的原声和音频轨）。
#[must_use]
pub fn resolve_audio(project: &Project, from: Time, len: Time) -> Vec<AudioSegment> {
    let to = from + len;
    let mut out = Vec::new();
    for track in &project.timeline.tracks {
        if !matches!(track.kind, TrackKind::Video | TrackKind::Audio) || !track.sounds() {
            continue;
        }
        // 转场处声音会越过片段边界，但最多伸进相邻片段的一半：从前一个片段看起，
        // 起点已经不早于区间终点的片段再往后的，声音都不会伸进区间
        let first = track.clips.partition_point(|c| c.end() <= from);
        let mut past_end = false;
        for clip in &track.clips[first.saturating_sub(1)..] {
            if past_end {
                break;
            }
            past_end = clip.start >= to;
            if !clip.enabled() {
                continue;
            }
            let Some(asset) = clip.asset() else { continue };
            if project.assets.get(&asset).is_none_or(|a| a.audio.is_none()) {
                continue;
            }
            // 转场处两段的声音叠在一起交叉淡化（预览和导出同一套算法）
            let Some(sound) = ve_core::compose::audio_span(project, clip.id) else {
                continue;
            };
            let start = sound.start.max(from);
            let end = sound.end.min(to);
            if end <= start {
                continue;
            }
            out.push(AudioSegment {
                asset,
                clip: clip.id,
                source_start: clip.source_at(start - clip.start).max(Time::ZERO),
                timeline_start: start,
                duration: end - start,
                gain: db_to_gain(clip.gain_db),
                fade_in: sound.fade_in,
                fade_out: sound.fade_out,
                sound_start: sound.start,
                sound_end: sound.end,
                speed: clip.speed,
                keep_pitch: clip.keep_pitch,
            });
        }
    }
    out
}

/// 时刻 `t` 要显示的字幕文字。
#[must_use]
pub fn resolve_subtitles(project: &Project, t: Time) -> Vec<&str> {
    resolve_subtitle_poses(project, t)
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

/// 时刻 `t` 要显示的字幕：文字和这一刻的样子（入场 / 出场动画）。
#[must_use]
pub fn resolve_subtitle_poses(project: &Project, t: Time) -> Vec<(&str, ve_core::TextPose)> {
    project
        .timeline
        .tracks
        .iter()
        .filter(|tr| tr.kind == TrackKind::Subtitle && tr.shows())
        .filter_map(|tr| {
            let c = tr.clip_at(t).filter(|c| c.enabled())?;
            let text = c.subtitle_text()?;
            let motion = c.text_motion().unwrap_or_default();
            Some((text, ve_core::text_pose(motion, t - c.start, c.duration)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::edit::{set_fades, set_gain};
    use ve_core::testing::sample_project;
    use ve_core::{Asset, AssetId, Time, TrackKind, Transform, VideoInfo};

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
            (seg.fade_in, seg.fade_out, seg.sound_start, seg.sound_end),
            (secs(1), secs(2), Time::ZERO, secs(8))
        );
    }

    #[test]
    fn transition_overlaps_the_two_clips_sound() {
        let (mut p, ids) = sample_project();
        ve_core::edit::set_transition(
            &mut p,
            ids.v2,
            Some(ve_core::Transition {
                kind: ve_core::TransitionKind::Dissolve,
                duration: secs(2),
            }),
        )
        .unwrap();
        // 4.5–5.5 秒：v1 越过切点放到 5.5（素材 4.5–5.5），v2 从 4.5 起提前放（素材 4.5–5.5）
        let segs = resolve_audio(&p, Time::from_millis(4500), secs(1));
        let of = |c| segs.iter().find(|s| s.clip == c).unwrap();
        let v1 = of(ids.v1);
        assert_eq!(
            (v1.timeline_start, v1.duration, v1.source_start),
            (Time::from_millis(4500), secs(1), Time::from_millis(4500))
        );
        assert_eq!((v1.sound_end, v1.fade_out), (secs(6), secs(2)));
        let v2 = of(ids.v2);
        assert_eq!(
            (v2.timeline_start, v2.duration, v2.source_start),
            (Time::from_millis(4500), secs(1), Time::from_millis(4500))
        );
        assert_eq!((v2.sound_start, v2.fade_in), (secs(4), secs(2)));
        // 5.8 秒：已过切点 0.8 秒，v1 的声音还剩最后 0.2 秒
        let late = resolve_audio(&p, Time::from_millis(5800), Time::from_millis(100));
        assert!(late.iter().any(|s| s.clip == ids.v1), "{late:?}");
        let after = resolve_audio(&p, secs(6), Time::from_millis(100));
        assert!(after.iter().all(|s| s.clip != ids.v1), "{after:?}");
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
    fn muted_tracks_are_silent_and_hidden_tracks_show_no_subtitles() {
        use ve_core::edit::{TrackFlag, set_track_flag};
        let (mut p, ids) = sample_project();
        set_track_flag(&mut p, ids.audio_track, TrackFlag::Muted, true).unwrap();
        set_track_flag(&mut p, ids.subtitle_track, TrackFlag::Hidden, true).unwrap();
        let segs = resolve_audio(&p, Time::ZERO, secs(8));
        assert!(
            segs.iter().all(|s| s.clip != ids.a1),
            "静音轨的背景音乐不播"
        );
        assert!(segs.iter().any(|s| s.clip == ids.v1), "视频轨的原声照常");
        assert_eq!(
            resolve_subtitles(&p, Time::from_millis(1500)),
            Vec::<&str>::new()
        );
        set_track_flag(&mut p, ids.video_track, TrackFlag::Muted, true).unwrap();
        let all_muted = resolve_audio(&p, Time::ZERO, secs(8));
        assert_eq!(all_muted.len(), 0, "{all_muted:?}");
    }

    #[test]
    fn disabled_clips_are_silent_and_hidden() {
        let (mut p, ids) = sample_project();
        ve_core::edit::set_disabled(&mut p, &[ids.a1, ids.s1], true).unwrap();
        let segs = resolve_audio(&p, Time::ZERO, secs(8));
        assert!(segs.iter().all(|s| s.clip != ids.a1), "停用的背景音乐不播");
        assert!(segs.iter().any(|s| s.clip == ids.v1), "视频原声照常");
        assert_eq!(
            resolve_subtitles(&p, Time::from_millis(1500)),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn subtitle_poses_follow_the_motion() {
        let (mut p, ids) = sample_project();
        let m = ve_core::TextMotion {
            intro: ve_core::TextIn::FadeIn,
            outro: ve_core::TextOut::None,
        };
        ve_core::edit::set_text_motion(&mut p, &[ids.s1], m).unwrap();
        // s1 在 1–3 秒：1.25 秒时渐显到一半
        let poses = resolve_subtitle_poses(&p, Time::from_millis(1250));
        assert_eq!(poses.len(), 1);
        assert!((poses[0].1.alpha - 0.5).abs() < 1e-4, "{poses:?}");
        assert_eq!(
            resolve_subtitle_poses(&p, Time::from_secs(2))[0].1,
            ve_core::TextPose::STILL
        );
    }

    #[test]
    fn subtitles_at_time() {
        let (p, _) = sample_project();
        assert_eq!(resolve_subtitles(&p, Time::from_millis(1500)), vec!["你好"]);
        assert_eq!(resolve_subtitles(&p, secs(3)), Vec::<&str>::new());
    }
    #[test]
    fn resolve_layers_bottom_up() {
        let (mut p, ids) = sample_project();
        let top = ve_core::edit::add_track(&mut p, TrackKind::Video).unwrap();
        let pip = ve_core::edit::insert_asset_at(&mut p, ids.video_asset, top, secs(1)).unwrap();
        let t = Transform {
            scale: 0.3,
            ..Transform::default()
        };
        ve_core::edit::set_transform(&mut p, pip, t).unwrap();
        let layers = resolve_layers(&p, secs(2));
        assert_eq!(layers.len(), 2);
        assert_eq!((layers[0].track, layers[0].clip), (ids.video_track, ids.v1));
        assert_eq!(layers[0].source_time, secs(2));
        assert_eq!((layers[1].track, layers[1].clip), (top, pip));
        assert_eq!(layers[1].source_time, secs(1));
        assert_eq!(layers[1].transform, t);
        // 最上层就是 resolve_video 的结果
        assert_eq!(resolve_video(&p, secs(2)).unwrap().clip, pip);
        // 上层还没开始：只有下层
        assert_eq!(resolve_layers(&p, Time::from_millis(500)).len(), 1);
        assert_eq!(resolve_layers(&p, secs(30)), []);
    }
}
