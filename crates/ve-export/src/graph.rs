//! ffmpeg 命令与滤镜图：把时间轴翻译成 ffmpeg 能执行的形式。
//!
//! - 每个片段一个输入，用输入端 `-ss/-t` 截取（快且帧精确）；静态图片用 `-loop 1`。
//! - 视频：各段缩放 + 补边到输出尺寸，空隙用黑场补齐，`concat` 拼接，再烧录字幕。
//! - 音频：各段重采样、调音量、淡入淡出、`adelay` 到时间轴位置，和一条静音底轨 `amix`（不归一化）。
//! - 滤镜图写成文件交给 `-/filter_complex`，避开 Windows 命令行长度上限；
//!   ffmpeg 在临时目录里运行，字幕用相对文件名 `subs.ass`，不用处理盘符冒号转义。

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::PathBuf;
use thiserror::Error;
use ve_core::{Clip, FrameRate, Project, Time, TrackKind};

/// 视频编码器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoder {
    /// NVIDIA 硬件编码（失败时自动退回 x264）。
    Nvenc,
    /// libx264 软件编码。
    X264,
}

/// 导出设置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSettings {
    /// 输出文件。
    pub path: PathBuf,
    /// 宽。
    pub width: u32,
    /// 高。
    pub height: u32,
    /// 帧率。
    pub frame_rate: FrameRate,
    /// 视频码率（kbps）。
    pub video_bitrate_kbps: u32,
    /// 编码器。
    pub encoder: Encoder,
}

/// 一次 ffmpeg 调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegJob {
    /// 命令行参数（不含程序名）。
    pub args: Vec<OsString>,
    /// 滤镜图，写入工作目录下的 `filter.txt`。
    pub filter_graph: String,
    /// 输出时长。
    pub duration: Time,
}

/// 导出错误。
#[derive(Debug, Error)]
pub enum ExportError {
    /// 时间轴是空的。
    #[error("时间轴是空的，没有可导出的内容")]
    EmptyTimeline,
    /// 读写临时文件失败。
    #[error("读写临时文件失败：{0}")]
    Io(#[from] std::io::Error),
    /// 找不到 ffmpeg。
    #[error("找不到 ffmpeg，请确认已安装并在 PATH 中")]
    FfmpegNotFound,
    /// ffmpeg 执行失败。
    #[error("ffmpeg 失败（退出码 {code:?}）：{message}")]
    Ffmpeg {
        /// 退出码。
        code: Option<i32>,
        /// stderr 末尾。
        message: String,
    },
    /// 用户取消。
    #[error("已取消")]
    Cancelled,
    /// 时间轴用到的素材文件找不到。
    #[error("找不到这些素材文件：{}", .0.join("、"))]
    MissingMedia(Vec<String>),
    /// 输出文件就是某个素材。
    #[error("输出文件不能是工程里的素材文件")]
    OutputIsInput,
}

/// 混音采样率。
const MIX_RATE: u32 = 48_000;

fn secs(t: Time) -> String {
    format!("{:.6}", t.as_secs_f64())
}

fn media_clips(project: &Project, kind: TrackKind) -> Vec<&Clip> {
    let mut clips: Vec<&Clip> = project
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == kind)
        .flat_map(|t| t.clips.iter().map(AsRef::as_ref))
        .filter(|c| c.asset().is_some())
        .collect();
    clips.sort_by_key(|c| c.start);
    clips
}

/// 输入参数；返回有音频的（输入下标，片段）。
fn push_inputs<'a>(
    project: &'a Project,
    clips: &[&'a Clip],
    args: &mut Vec<OsString>,
) -> Vec<(usize, &'a Clip)> {
    let mut audio_inputs = Vec::new();
    for (index, &clip) in clips.iter().enumerate() {
        let Some(asset) = clip.asset().and_then(|a| project.assets.get(&a)) else {
            continue;
        };
        if asset.is_still() {
            args.extend([
                "-loop".into(),
                "1".into(),
                "-t".into(),
                secs(clip.duration).into(),
            ]);
        } else {
            args.extend([
                "-ss".into(),
                secs(clip.source_in()).into(),
                "-t".into(),
                secs(clip.duration).into(),
            ]);
        }
        args.extend(["-i".into(), asset.path.clone().into_os_string()]);
        if asset.audio.is_some() {
            audio_inputs.push((index, clip));
        }
    }
    audio_inputs
}

/// 视频链：黑场补空隙 + 各段规范化，按时间拼接，烧录字幕，输出 `[vout]`。
fn video_chain(
    graph: &mut String,
    video: &[&Clip],
    duration: Time,
    settings: &ExportSettings,
    with_subtitles: bool,
) {
    // yuv420p 要求偶数宽高
    let (w, h) = (even(settings.width), even(settings.height));
    let rate = rate_str(settings.frame_rate);
    let mut labels = String::new();
    let mut gaps = 0;
    let mut cursor = Time::ZERO;
    let mut push_gap = |out: &mut String, gap_labels: &mut String, len: Time| {
        let _ = writeln!(
            out,
            "color=c=black:s={w}x{h}:r={rate}:d={},format=yuv420p,setsar=1[g{gaps}];",
            secs(len)
        );
        let _ = write!(gap_labels, "[g{gaps}]");
        gaps += 1;
    };
    for (index, clip) in video.iter().enumerate() {
        if clip.start > cursor {
            push_gap(graph, &mut labels, clip.start - cursor);
        }
        // 素材时长取的是容器时长，视频流可能更短：用最后一帧补足再截到片段长度，
        // 否则 concat 会把后面的片段整体前移
        let len = secs(clip.duration);
        let _ = writeln!(
            graph,
            "[{index}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={rate},format=yuv420p,setpts=PTS-STARTPTS,tpad=stop_mode=clone:stop_duration={len},trim=duration={len},setpts=PTS-STARTPTS[v{index}];"
        );
        let _ = write!(labels, "[v{index}]");
        cursor = clip.end();
    }
    if cursor < duration {
        push_gap(graph, &mut labels, duration - cursor);
    }
    let parts = video.len() + gaps;
    let _ = writeln!(graph, "{labels}concat=n={parts}:v=1:a=0[vcat];");
    let subs = if with_subtitles {
        "ass=subs.ass"
    } else {
        "null"
    };
    let _ = writeln!(graph, "[vcat]{subs}[vout];");
}

/// 音频链：静音底轨 + 各段（音量、淡入淡出、延时），不归一化混合，输出 `[aout]`。
fn audio_chain(graph: &mut String, audio_inputs: &[(usize, &Clip)], duration: Time) {
    let _ = writeln!(
        graph,
        "anullsrc=r={MIX_RATE}:cl=stereo,atrim=duration={}[abase];",
        secs(duration)
    );
    let mut mix_labels = String::from("[abase]");
    for &(index, clip) in audio_inputs {
        let _ = write!(
            graph,
            "[{index}:a]aresample={MIX_RATE},aformat=sample_fmts=fltp:channel_layouts=stereo,volume={:.6}",
            10f64.powf(f64::from(clip.gain_db) / 20.0)
        );
        if clip.fade_in > Time::ZERO {
            let _ = write!(graph, ",afade=t=in:st=0:d={}", secs(clip.fade_in));
        }
        if clip.fade_out > Time::ZERO {
            let _ = write!(
                graph,
                ",afade=t=out:st={}:d={}",
                secs(clip.duration - clip.fade_out),
                secs(clip.fade_out)
            );
        }
        let _ = writeln!(
            graph,
            ",adelay=delays={}S:all=1[a{index}];",
            clip.start.to_samples(MIX_RATE)
        );
        let _ = write!(mix_labels, "[a{index}]");
    }
    if audio_inputs.is_empty() {
        graph.push_str("[abase]anull[aout]");
    } else {
        let _ = write!(
            graph,
            "{mix_labels}amix=inputs={}:normalize=0:duration=first[aout]",
            audio_inputs.len() + 1
        );
    }
}

/// 向下取偶数（至少 2）。
const fn even(v: u32) -> u32 {
    let e = v & !1;
    if e < 2 { 2 } else { e }
}

fn rate_str(rate: FrameRate) -> String {
    format!("{}/{}", rate.num, rate.den)
}

/// 输出参数：映射、编码器、码率、容器。
fn push_output(args: &mut Vec<OsString>, settings: &ExportSettings, duration: Time) {
    args.extend(
        [
            "-/filter_complex",
            "filter.txt",
            "-map",
            "[vout]",
            "-map",
            "[aout]",
        ]
        .map(OsString::from),
    );
    let kbps = settings.video_bitrate_kbps.max(100);
    let codec: &[&str] = match settings.encoder {
        Encoder::Nvenc => &["-c:v", "h264_nvenc", "-preset", "p5", "-rc", "vbr"],
        Encoder::X264 => &["-c:v", "libx264", "-preset", "medium"],
    };
    args.extend(codec.iter().map(OsString::from));
    args.extend(
        [
            "-b:v".to_owned(),
            format!("{kbps}k"),
            "-maxrate".to_owned(),
            format!("{}k", kbps * 3 / 2),
            "-bufsize".to_owned(),
            format!("{}k", kbps * 2),
            "-pix_fmt".to_owned(),
            "yuv420p".to_owned(),
            "-r".to_owned(),
            rate_str(settings.frame_rate),
            "-c:a".to_owned(),
            "aac".to_owned(),
            "-b:a".to_owned(),
            "192k".to_owned(),
            "-movflags".to_owned(),
            "+faststart".to_owned(),
            "-t".to_owned(),
            secs(duration),
            "-y".to_owned(),
        ]
        .map(OsString::from),
    );
    args.push(settings.path.clone().into_os_string());
}

/// 按工程和导出设置生成 ffmpeg 调用。
///
/// # Errors
/// 时间轴为空。
pub fn build_job(
    project: &Project,
    settings: &ExportSettings,
    with_subtitles: bool,
) -> Result<FfmpegJob, ExportError> {
    let duration = project.timeline.duration();
    if duration <= Time::ZERO {
        return Err(ExportError::EmptyTimeline);
    }
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-progress",
        "pipe:1",
        "-nostats",
        "-stats_period",
        "0.1",
    ]
    .map(OsString::from)
    .to_vec();
    // 输入：视频轨片段在前，音频轨片段在后
    let video = media_clips(project, TrackKind::Video);
    let audio = media_clips(project, TrackKind::Audio);
    let all: Vec<&Clip> = video.iter().chain(audio.iter()).copied().collect();
    let audio_inputs = push_inputs(project, &all, &mut args);
    let mut graph = String::new();
    video_chain(&mut graph, &video, duration, settings, with_subtitles);
    audio_chain(&mut graph, &audio_inputs, duration);
    push_output(&mut args, settings, duration);
    Ok(FfmpegJob {
        args,
        filter_graph: graph,
        duration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use ve_core::edit::{delete_clips, move_clip, set_fades, set_gain};
    use ve_core::testing::sample_project;

    fn settings() -> ExportSettings {
        ExportSettings {
            path: PathBuf::from("C:/out/final.mp4"),
            width: 1280,
            height: 720,
            frame_rate: FrameRate::new(30, 1),
            video_bitrate_kbps: 8000,
            encoder: Encoder::X264,
        }
    }

    fn args_text(job: &FfmpegJob) -> String {
        job.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn graph_two_clips_bgm_subs() {
        let (mut p, ids) = sample_project();
        set_gain(&mut p, ids.a1, -6.0).unwrap();
        set_fades(&mut p, ids.a1, Time::from_secs(1), Time::from_secs(2)).unwrap();
        let job = build_job(&p, &settings(), true).unwrap();
        let args = args_text(&job);
        // 每个片段一个输入，输入端精确截取
        assert!(
            args.contains("-ss 0.000000 -t 5.000000 -i clip.mp4"),
            "{args}"
        );
        assert!(
            args.contains("-ss 5.000000 -t 3.000000 -i clip.mp4"),
            "{args}"
        );
        assert!(
            args.contains("-ss 0.000000 -t 8.000000 -i bgm.mp3"),
            "{args}"
        );
        assert!(args.contains("-c:v libx264"), "{args}");
        assert!(args.contains("-b:v 8000k"));
        assert!(args.ends_with("-y C:/out/final.mp4"), "{args}");
        let g = &job.filter_graph;
        assert!(g.contains("[0:v]scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=30/1,format=yuv420p,setpts=PTS-STARTPTS,tpad=stop_mode=clone:stop_duration=5.000000,trim=duration=5.000000,setpts=PTS-STARTPTS[v0]"), "{g}");
        assert!(g.contains("[v0][v1]concat=n=2:v=1:a=0[vcat]"), "{g}");
        assert!(g.contains("[vcat]ass=subs.ass[vout]"), "{g}");
        // bgm：-6dB、淡入 1 秒、淡出 2 秒（片段 8 秒 → 第 6 秒开始淡出）、从 0 开始
        assert!(g.contains("[2:a]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,volume=0.501187,afade=t=in:st=0:d=1.000000,afade=t=out:st=6.000000:d=2.000000,adelay=delays=0S:all=1[a2]"), "{g}");
        // 第二段视频的原声延后 5 秒 = 240000 个样本
        assert!(g.contains("adelay=delays=240000S:all=1[a1]"), "{g}");
        assert!(
            g.contains("anullsrc=r=48000:cl=stereo,atrim=duration=8.000000[abase]"),
            "{g}"
        );
        assert!(
            g.contains("[abase][a0][a1][a2]amix=inputs=4:normalize=0:duration=first[aout]"),
            "{g}"
        );
    }

    #[test]
    fn odd_output_size_is_rounded_to_even() {
        // yuv420p 要求宽高为偶数，853x480 这类尺寸直接导出会失败
        let (p, _) = sample_project();
        let mut s = settings();
        s.width = 853;
        s.height = 481;
        let job = build_job(&p, &s, false).unwrap();
        assert!(
            job.filter_graph.contains("scale=852:480:"),
            "{}",
            job.filter_graph
        );
        assert!(!job.filter_graph.contains("853"), "{}", job.filter_graph);
    }

    #[test]
    fn graph_fills_gaps_with_black() {
        let (mut p, ids) = sample_project();
        move_clip(&mut p, ids.v1, ids.video_track, Time::from_secs(20)).unwrap();
        let job = build_job(&p, &settings(), false).unwrap();
        let g = &job.filter_graph;
        // 0–5 秒空着：v2 在 5–8 秒前面要补 5 秒黑场，8–20 秒再补 12 秒
        assert!(
            g.contains("color=c=black:s=1280x720:r=30/1:d=5.000000,format=yuv420p,setsar=1[g0]"),
            "{g}"
        );
        assert!(
            g.contains("color=c=black:s=1280x720:r=30/1:d=12.000000,format=yuv420p,setsar=1[g1]"),
            "{g}"
        );
        assert!(
            g.contains("[g0][v0][g1][v1]concat=n=4:v=1:a=0[vcat]"),
            "{g}"
        );
        assert!(g.contains("[vcat]null[vout]"), "{g}");
        assert_eq!(job.duration, Time::from_secs(25));
    }

    #[test]
    fn audio_only_project_gets_black_video() {
        let (mut p, ids) = sample_project();
        delete_clips(&mut p, &[ids.v1, ids.v2]).unwrap();
        let job = build_job(&p, &settings(), false).unwrap();
        assert!(
            job.filter_graph
                .contains("color=c=black:s=1280x720:r=30/1:d=8.000000"),
            "{}",
            job.filter_graph
        );
    }

    #[test]
    fn still_image_input_loops() {
        let (mut p, ids) = sample_project();
        let asset = p.assets.get_mut(&ids.video_asset).unwrap();
        asset.video = asset.video.map(|v| ve_core::VideoInfo { still: true, ..v });
        asset.audio = None;
        let job = build_job(&p, &settings(), false).unwrap();
        let args = args_text(&job);
        assert!(args.contains("-loop 1 -t 5.000000 -i clip.mp4"), "{args}");
        assert!(!job.filter_graph.contains("[0:a]"), "{}", job.filter_graph);
    }

    #[test]
    fn nvenc_args_and_empty_timeline_error() {
        let (p, _) = sample_project();
        let mut s = settings();
        s.encoder = Encoder::Nvenc;
        let args = args_text(&build_job(&p, &s, false).unwrap());
        assert!(args.contains("-c:v h264_nvenc -preset p5"), "{args}");
        assert!(matches!(
            build_job(&Project::new(), &s, false),
            Err(ExportError::EmptyTimeline)
        ));
    }
}
