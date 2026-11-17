//! 探测：读出媒体文件的时长和流信息。

use crate::MediaError;
use ffmpeg_next as ff;
use std::path::Path;
use std::sync::OnceLock;
use ve_core::{AudioInfo, FrameRate, Time, VideoInfo};

/// 静态图片放进时间轴时的默认长度。
pub const STILL_DEFAULT_DURATION: Time = Time::from_secs(5);

/// 探测结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeResult {
    /// 时长（静态图片为 [`STILL_DEFAULT_DURATION`]）。
    pub duration: Time,
    /// 视频流。
    pub video: Option<VideoInfo>,
    /// 音频流。
    pub audio: Option<AudioInfo>,
}

/// 初始化 `FFmpeg`（只执行一次，之后直接返回第一次的结果）。
///
/// # Errors
/// 初始化失败。
pub fn init() -> Result<(), MediaError> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        ff::init().map_err(|e| e.to_string())?;
        ff::util::log::set_level(ff::util::log::Level::Error);
        Ok(())
    })
    .clone()
    .map_err(MediaError::Init)
}

/// 打开输入文件。
pub(crate) fn open_input(path: &Path) -> Result<ff::format::context::Input, MediaError> {
    init()?;
    ff::format::input(path).map_err(|source| MediaError::Open {
        path: path.to_path_buf(),
        source,
    })
}

/// `Rational` 转 [`Time`]：`value * tb`。
pub(crate) fn ts_to_time(value: i64, tb: ff::Rational) -> Time {
    let (num, den) = (i128::from(tb.numerator()), i128::from(tb.denominator()));
    if den == 0 {
        return Time::ZERO;
    }
    let flicks = i128::from(value) * num * i128::from(ve_core::FLICKS_PER_SECOND) / den;
    Time(i64::try_from(flicks).unwrap_or(i64::MAX))
}

/// [`Time`] 转时间基 `tb` 下的时间戳（向下取整）。
pub(crate) fn time_to_ts(t: Time, tb: ff::Rational) -> i64 {
    let (num, den) = (i128::from(tb.numerator()), i128::from(tb.denominator()));
    if num == 0 {
        return 0;
    }
    let ts = (i128::from(t.0) * den).div_euclid(num * i128::from(ve_core::FLICKS_PER_SECOND));
    i64::try_from(ts).unwrap_or(i64::MAX)
}

fn frame_rate_of(stream: &ff::format::stream::Stream<'_>) -> FrameRate {
    let r = stream.avg_frame_rate();
    let r = if r.numerator() > 0 && r.denominator() > 0 {
        r
    } else {
        stream.rate()
    };
    match (u32::try_from(r.numerator()), u32::try_from(r.denominator())) {
        (Ok(n), Ok(d)) if n > 0 && d > 0 => FrameRate::new(n, d),
        _ => FrameRate::new(30, 1),
    }
}

/// 图片格式（单帧）：容器名以这些开头的视为静态图片。
fn is_image_format(name: &str) -> bool {
    name.starts_with("image2") || name.ends_with("_pipe")
}

/// 文件的起始时间戳（各流起点的最小值；TS / MTS 常见 1.4 秒）。
/// 对外的素材内时间都从 0 算，和 ffmpeg 命令行 `-ss` 的含义一致。
pub(crate) fn start_offset(input: &ff::format::context::Input) -> Time {
    input
        .streams()
        .filter(|s| s.start_time() != ff::ffi::AV_NOPTS_VALUE)
        .map(|s| ts_to_time(s.start_time(), s.time_base()))
        .min()
        .unwrap_or(Time::ZERO)
}

/// 真正的视频流：跳过封面图（`ATTACHED_PIC`，比如 mp3 / m4a 里嵌的专辑封面）。
pub(crate) fn video_stream(
    input: &ff::format::context::Input,
) -> Option<ff::format::stream::Stream<'_>> {
    let is_cover = |s: &ff::format::stream::Stream<'_>| {
        s.disposition()
            .contains(ff::format::stream::Disposition::ATTACHED_PIC)
    };
    match input.streams().best(ff::media::Type::Video) {
        Some(s) if !is_cover(&s) => Some(s),
        _ => input
            .streams()
            .find(|s| s.parameters().medium() == ff::media::Type::Video && !is_cover(s)),
    }
}

/// 容器给不出时长时（裸码流、部分录屏文件）：先看各流自己的时长，再扫一遍数据包推算。
fn estimate_duration(mut input: ff::format::context::Input, rate: Option<FrameRate>) -> Time {
    let from_streams = input
        .streams()
        .map(|s| ts_to_time(s.duration().max(0), s.time_base()))
        .max()
        .unwrap_or(Time::ZERO);
    if from_streams > Time::ZERO {
        return from_streams;
    }
    let best = video_stream(&input)
        .or_else(|| input.streams().best(ff::media::Type::Audio))
        .map(|s| (s.index(), s.time_base()));
    let Some((index, tb)) = best else {
        return Time::ZERO;
    };
    let (mut end, mut packets) = (Time::ZERO, 0_i64);
    for (stream, packet) in input.packets() {
        if stream.index() != index {
            continue;
        }
        packets += 1;
        if let Some(pts) = packet.pts() {
            end = end.max(ts_to_time(pts + packet.duration().max(0), tb));
        }
    }
    if end > Time::ZERO {
        return end;
    }
    // 裸码流常常没有时间戳：按包数 × 帧时长算
    rate.map_or(Time::ZERO, |r| r.time_of_frame(packets))
}

/// 探测媒体文件。
///
/// # Errors
/// 打不开文件、不是媒体文件、既没有视频也没有音频。
pub fn probe(path: &Path) -> Result<ProbeResult, MediaError> {
    let input = open_input(path)?;
    let still = is_image_format(input.format().name());
    let video = match video_stream(&input) {
        Some(stream) => {
            let ctx = ff::codec::context::Context::from_parameters(stream.parameters())?;
            let dec = ctx.decoder().video()?;
            Some(VideoInfo {
                width: dec.width(),
                height: dec.height(),
                frame_rate: frame_rate_of(&stream),
                still,
            })
        }
        None => None,
    };
    let audio = match input.streams().best(ff::media::Type::Audio) {
        Some(stream) => {
            let ctx = ff::codec::context::Context::from_parameters(stream.parameters())?;
            let dec = ctx.decoder().audio()?;
            Some(AudioInfo {
                sample_rate: dec.rate(),
                channels: dec.channels(),
            })
        }
        None => None,
    };
    if video.is_none() && audio.is_none() {
        return Err(MediaError::NoStream {
            path: path.to_path_buf(),
            kind: "视频流或音频流",
        });
    }
    let duration = if still {
        STILL_DEFAULT_DURATION
    } else {
        let container = ts_to_time(
            input.duration().max(0),
            ff::Rational::new(1, ff::ffi::AV_TIME_BASE),
        );
        if container > Time::ZERO {
            container
        } else {
            estimate_duration(input, video.map(|v| v.frame_rate))
        }
    };
    Ok(ProbeResult {
        duration,
        video,
        audio,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, fixture};
    use ve_core::{FrameRate, Time};

    fn close(a: Time, b: Time, tol: Time) -> bool {
        (a - b).0.abs() <= tol.0
    }

    #[test]
    fn probe_frame_indexed() {
        let r = probe(&fixture(Fixture::FrameIndexed30)).unwrap();
        let v = r.video.unwrap();
        assert_eq!(
            (v.width, v.height, v.frame_rate, v.still),
            (64, 36, FrameRate::new(30, 1), false)
        );
        assert!(
            close(r.duration, Time::from_secs(2), Time::from_millis(40)),
            "{:?}",
            r.duration
        );
    }

    #[test]
    fn probe_video_without_audio() {
        let r = probe(&fixture(Fixture::FrameIndexed30)).unwrap();
        assert!(r.audio.is_none());
    }

    #[test]
    fn probe_video_with_audio() {
        let r = probe(&fixture(Fixture::VideoWithAudio)).unwrap();
        assert!(r.video.is_some());
        let a = r.audio.unwrap();
        assert_eq!((a.sample_rate, a.channels), (48_000, 2));
    }

    #[test]
    fn probe_audio_only() {
        let r = probe(&fixture(Fixture::ToneStereo48k)).unwrap();
        assert!(r.video.is_none());
        assert_eq!(r.audio.unwrap().sample_rate, 48_000);
        assert!(
            close(r.duration, Time::from_secs(2), Time::from_millis(5)),
            "{:?}",
            r.duration
        );
    }

    #[test]
    fn probe_image_is_still() {
        let r = probe(&fixture(Fixture::Image)).unwrap();
        let v = r.video.unwrap();
        assert!(v.still);
        assert_eq!((v.width, v.height), (64, 36));
        assert_eq!(r.duration, STILL_DEFAULT_DURATION);
    }

    #[test]
    fn probe_raw_stream_estimates_duration() {
        // 裸码流没有时间信息：60 帧按探测到的帧率（FFmpeg 默认按 25fps）推算，和解码器实际播出的一致
        let r = probe(&fixture(Fixture::RawH264)).unwrap();
        let rate = r.video.unwrap().frame_rate;
        assert!(r.duration > Time::ZERO);
        assert_eq!(
            r.duration,
            rate.time_of_frame(60),
            "{:?} @ {rate:?}",
            r.duration
        );
    }

    #[test]
    fn mp3_cover_art_is_not_video() {
        let r = probe(&fixture(Fixture::Mp3WithCover)).unwrap();
        assert!(r.video.is_none(), "封面图被当成了视频：{:?}", r.video);
        assert!(r.audio.is_some());
        assert!(r.duration > Time::from_millis(1900));
    }

    #[test]
    fn probe_missing_file_errors() {
        let err = probe(Path::new("definitely/not/here.mp4")).unwrap_err();
        assert!(matches!(err, MediaError::Open { .. }), "{err:?}");
        assert!(err.to_string().contains("here.mp4"));
    }

    #[test]
    fn probe_non_media_file_errors() {
        let dir = std::env::temp_dir().join(format!("ve-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notes.txt");
        std::fs::write(&path, "hello").unwrap();
        assert!(probe(&path).is_err());
    }
}
