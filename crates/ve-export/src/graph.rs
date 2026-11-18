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

/// 视频编码格式（剪映导出对话框的「视频编码」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Codec {
    /// H.264：最通用。
    #[default]
    H264,
    /// HEVC（H.265）：同样画质文件更小。
    Hevc,
}

/// 推荐码率（kbps）：按每像素每帧 0.15 比特估，720p30 约 4 Mbps、1080p30 约 9 Mbps；HEVC 再省三成。
#[must_use]
pub fn recommended_kbps(width: u32, height: u32, frame_rate: FrameRate, codec: Codec) -> u32 {
    let pixels = f64::from(width) * f64::from(height);
    let kbps = pixels * frame_rate.as_f64() * 0.15 / 1000.0;
    let kbps = match codec {
        Codec::H264 => kbps,
        Codec::Hevc => kbps * 0.7,
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "码率为正且远小于 u32 上限"
    )]
    let rounded = ((kbps / 100.0).round() * 100.0) as u32;
    rounded.clamp(500, 200_000)
}

/// 估计的文件大小（字节）：视频码率 + 音频 192 kbps，乘以时长。
#[must_use]
pub fn estimated_bytes(video_kbps: u32, duration: Time) -> u64 {
    let kbps = u64::from(video_kbps) + 192;
    let ms = u64::try_from(duration.as_millis().max(0)).unwrap_or(0);
    kbps * ms / 8
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
    /// 编码格式。
    pub codec: Codec,
    /// 画面合成器配置（测试里用软件显卡）。
    pub render: ve_render::RenderConfig,
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
    /// 解码或显卡合成画面失败。
    #[error("渲染画面失败：{0}")]
    Render(String),
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
        .filter(|c| c.asset().is_some() && c.enabled())
        .collect();
    clips.sort_by_key(|c| c.start);
    clips
}

/// 第 0 个输入：合成好的画面，NV12 原始帧从标准输入进来。
fn push_video_input(args: &mut Vec<OsString>, settings: &ExportSettings) {
    let (w, h) = crate::render::output_size(settings);
    args.extend(
        [
            "-f".to_owned(),
            "rawvideo".to_owned(),
            "-pix_fmt".to_owned(),
            "nv12".to_owned(),
            "-s".to_owned(),
            format!("{w}x{h}"),
            "-framerate".to_owned(),
            rate_str(settings.frame_rate),
            "-color_range".to_owned(),
            "tv".to_owned(),
            "-colorspace".to_owned(),
            "bt709".to_owned(),
            // 帧本身带上原色和传递特性：否则输出端的同名选项会被帧上的「未指定」盖掉
            "-color_primaries".to_owned(),
            "bt709".to_owned(),
            "-color_trc".to_owned(),
            "bt709".to_owned(),
            "-thread_queue_size".to_owned(),
            "64".to_owned(),
            "-i".to_owned(),
            "pipe:0".to_owned(),
        ]
        .map(OsString::from),
    );
}

/// 有声音的片段各作一个输入（从第 1 个开始编号），返回 (输入编号, 片段)。
fn push_audio_inputs<'a>(
    project: &'a Project,
    clips: &[&'a Clip],
    args: &mut Vec<OsString>,
) -> Vec<AudioInput<'a>> {
    let mut audio_inputs = Vec::new();
    for &clip in clips {
        let Some(asset) = clip.asset().and_then(|a| project.assets.get(&a)) else {
            continue;
        };
        if asset.audio.is_none() {
            continue;
        }
        // 转场处声音越过片段边界（和预览同一套算法）
        let Some(sound) = ve_core::compose::audio_span(project, clip.id) else {
            continue;
        };
        args.extend([
            "-ss".into(),
            secs(clip.source_at(sound.start - clip.start).max(Time::ZERO)).into(),
            "-t".into(),
            // 变速：截取的是素材里用到的那一段
            secs((sound.end - sound.start).scaled(f64::from(clip.speed))).into(),
            "-i".into(),
            asset.path.clone().into_os_string(),
        ]);
        audio_inputs.push(AudioInput {
            index: audio_inputs.len() + 1,
            clip,
            sound,
        });
    }
    audio_inputs
}

/// 一路音频输入：ffmpeg 输入序号、片段、声音实际放的那一段。
struct AudioInput<'a> {
    index: usize,
    clip: &'a Clip,
    sound: ve_core::compose::AudioSpan,
}

/// 视频链：合成好的画面直接用，有字幕时再烧录。
fn video_chain(graph: &mut String, with_subtitles: bool) {
    let subs = if with_subtitles {
        "ass=subs.ass"
    } else {
        "null"
    };
    let _ = writeln!(graph, "[0:v]{subs}[vout];");
}

/// 变速的音频滤镜（原速是空串）。保持音调：`atempo`（每级只能 0.5–2，超出就等分成几级串起来）；
/// 变调：把采样率当成「倍数 × 48k」再重采样回 48k，快慢和音调一起变。都和预览一致。
fn speed_filter(clip: &Clip) -> String {
    #[expect(clippy::float_cmp, reason = "原速是精确的 1.0")]
    if clip.speed == 1.0 {
        return String::new();
    }
    if !clip.keep_pitch {
        return format!(
            ",asetrate={:.3},aresample={MIX_RATE}",
            f64::from(MIX_RATE) * f64::from(clip.speed)
        );
    }
    let speed = f64::from(clip.speed);
    // 每级最多 ×2 或 ÷2：级数 = ⌈|log2 倍数|⌉，每级一样大
    #[expect(clippy::cast_possible_truncation, reason = "倍数 0.1–10，最多 4 级")]
    let stages = (speed.log2().abs().ceil() as i32).max(1);
    let step = speed.powf(1.0 / f64::from(stages));
    format!(",atempo={step:.6}").repeat(usize::try_from(stages).unwrap_or(1))
}

fn audio_chain(graph: &mut String, audio_inputs: &[AudioInput<'_>], duration: Time) {
    let _ = writeln!(
        graph,
        "anullsrc=r={MIX_RATE}:cl=stereo,atrim=duration={}[abase];",
        secs(duration)
    );
    let mut mix_labels = String::from("[abase]");
    for &AudioInput { index, clip, sound } in audio_inputs {
        let speed = speed_filter(clip);
        let _ = write!(
            graph,
            "[{index}:a]aresample={MIX_RATE}{speed},aformat=sample_fmts=fltp:channel_layouts=stereo,volume={:.6}",
            10f64.powf(f64::from(clip.gain_db) / 20.0)
        );
        // 和预览一样：转场处两段的声音叠在一起交叉淡化
        let (fade_in, fade_out) = (sound.fade_in, sound.fade_out);
        if fade_in > Time::ZERO {
            let _ = write!(graph, ",afade=t=in:st=0:d={}", secs(fade_in));
        }
        if fade_out > Time::ZERO {
            let _ = write!(
                graph,
                ",afade=t=out:st={}:d={}",
                secs(sound.end - sound.start - fade_out),
                secs(fade_out)
            );
        }
        let _ = writeln!(
            graph,
            ",adelay=delays={}S:all=1[a{index}];",
            sound.start.to_samples(MIX_RATE)
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
    // HEVC 加 hvc1 标签：苹果设备和大多数播放器才认 mp4 里的 HEVC
    let codec: &[&str] = match (settings.encoder, settings.codec) {
        (Encoder::Nvenc, Codec::H264) => &["-c:v", "h264_nvenc", "-preset", "p5", "-rc", "vbr"],
        (Encoder::X264, Codec::H264) => &["-c:v", "libx264", "-preset", "medium"],
        (Encoder::Nvenc, Codec::Hevc) => &[
            "-c:v",
            "hevc_nvenc",
            "-preset",
            "p5",
            "-rc",
            "vbr",
            "-tag:v",
            "hvc1",
        ],
        (Encoder::X264, Codec::Hevc) => &["-c:v", "libx265", "-preset", "medium", "-tag:v", "hvc1"],
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
            "-colorspace".to_owned(),
            "bt709".to_owned(),
            "-color_primaries".to_owned(),
            "bt709".to_owned(),
            "-color_trc".to_owned(),
            "bt709".to_owned(),
            "-color_range".to_owned(),
            "tv".to_owned(),
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
    let mut args: Vec<OsString> = ["-hide_banner", "-nostats"].map(OsString::from).to_vec();
    push_video_input(&mut args, settings);
    // 声音：视频轨片段的原声在前，音频轨片段在后
    let video = media_clips(project, TrackKind::Video);
    let audio = media_clips(project, TrackKind::Audio);
    let all: Vec<&Clip> = video.iter().chain(audio.iter()).copied().collect();
    let audio_inputs = push_audio_inputs(project, &all, &mut args);
    let mut graph = String::new();
    video_chain(&mut graph, with_subtitles);
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
            codec: Codec::H264,
            render: ve_render::RenderConfig { software: true },
        }
    }

    #[test]
    fn transitions_fade_the_sound_around_the_cut() {
        let (mut p, ids) = sample_project();
        ve_core::edit::set_transition(
            &mut p,
            ids.v2,
            Some(ve_core::Transition {
                kind: ve_core::TransitionKind::Dissolve,
                duration: Time::from_secs(2),
            }),
        )
        .unwrap();
        let job = build_job(&p, &settings(), false).unwrap();
        let graph = &job.filter_graph;
        // 和预览一样两段叠在一起：v1 放到 6 秒（4–6 秒淡出），v2 从 4 秒起（4–6 秒淡入）
        assert!(
            graph.contains("afade=t=out:st=4.000000:d=2.000000"),
            "{graph}"
        );
        assert!(graph.contains("afade=t=in:st=0:d=2.000000"), "{graph}");
        assert!(
            graph.contains("adelay=delays=192000S"),
            "v2 从 4 秒起：{graph}"
        );
        let args: Vec<String> = job
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let joined = args.join(" ");
        // v1 取素材 0–6 秒，v2 取素材 4–8 秒
        assert!(joined.contains("-ss 0.000000 -t 6.000000"), "{joined}");
        assert!(joined.contains("-ss 4.000000 -t 4.000000"), "{joined}");
    }

    #[test]
    fn keeping_the_pitch_chains_atempo_within_its_range() {
        let (mut p, ids) = sample_project();
        p.settings.main_magnet = false;
        ve_core::edit::set_speed(&mut p, ids.v2, 5.0).unwrap();
        ve_core::edit::set_speed(&mut p, ids.v1, 0.25).unwrap_err();
        let graph = build_job(&p, &settings(), false).unwrap().filter_graph;
        // 5 倍：三级各 1.71 倍
        assert!(
            graph.contains("atempo=1.709976,atempo=1.709976,atempo=1.709976,"),
            "{graph}"
        );
        assert!(!graph.contains("asetrate"), "{graph}");
        let mut c = Clip::clone(p.timeline.find_clip(ids.v1).unwrap().1);
        c.speed = 0.25;
        assert_eq!(speed_filter(&c), ",atempo=0.500000,atempo=0.500000");
        c.speed = 1.5;
        assert_eq!(speed_filter(&c), ",atempo=1.500000");
        c.speed = 1.0;
        assert_eq!(speed_filter(&c), "");
    }

    #[test]
    fn sped_up_clips_take_more_source_and_shift_pitch() {
        let (mut p, ids) = sample_project();
        p.settings.main_magnet = false;
        ve_core::edit::set_speed(&mut p, ids.v2, 2.0).unwrap();
        ve_core::edit::set_keep_pitch(&mut p, ids.v2, false).unwrap();
        let job = build_job(&p, &settings(), false).unwrap();
        let args = args_text(&job);
        // v2 在时间轴上 1.5 秒，用掉素材 5–8 秒
        assert!(
            args.contains("-ss 5.000000 -t 3.000000 -i clip.mp4"),
            "{args}"
        );
        assert!(
            job.filter_graph
                .contains("asetrate=96000.000,aresample=48000"),
            "{}",
            job.filter_graph
        );
    }

    #[test]
    fn hevc_uses_hevc_encoders_with_hvc1_tag() {
        let (p, _) = sample_project();
        let mut s = settings();
        s.codec = Codec::Hevc;
        let args = args_text(&build_job(&p, &s, false).unwrap());
        assert!(
            args.contains("-c:v libx265 -preset medium -tag:v hvc1"),
            "{args}"
        );
        s.encoder = Encoder::Nvenc;
        let nvenc = args_text(&build_job(&p, &s, false).unwrap());
        assert!(nvenc.contains("-c:v hevc_nvenc"), "{nvenc}");
    }

    #[test]
    fn recommended_bitrate_and_size_estimate() {
        let fps30 = FrameRate::new(30, 1);
        assert_eq!(recommended_kbps(1920, 1080, fps30, Codec::H264), 9300);
        assert_eq!(recommended_kbps(1280, 720, fps30, Codec::H264), 4100);
        assert_eq!(recommended_kbps(1920, 1080, fps30, Codec::Hevc), 6500);
        assert_eq!(
            recommended_kbps(64, 36, fps30, Codec::H264),
            500,
            "最少 500"
        );
        // 10 秒、8000 + 192 kbps：约 10 MB
        assert_eq!(estimated_bytes(8000, Time::from_secs(10)), 10_240_000);
    }

    fn args_text(job: &FfmpegJob) -> String {
        job.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn disabled_audio_is_left_out_of_the_mix() {
        let (mut p, ids) = sample_project();
        ve_core::edit::set_disabled(&mut p, &[ids.a1], true).unwrap();
        let args = args_text(&build_job(&p, &settings(), true).unwrap());
        assert!(!args.contains("bgm.mp3"), "{args}");
        assert!(args.contains("clip.mp4"), "{args}");
    }

    #[test]
    fn graph_two_clips_bgm_subs() {
        let (mut p, ids) = sample_project();
        set_gain(&mut p, ids.a1, -6.0).unwrap();
        set_fades(&mut p, ids.a1, Time::from_secs(1), Time::from_secs(2)).unwrap();
        let job = build_job(&p, &settings(), true).unwrap();
        let args = args_text(&job);
        // 第 0 个输入是合成好的画面：NV12 原始帧从标准输入进来
        assert!(
            args.starts_with("-hide_banner -nostats -f rawvideo -pix_fmt nv12 -s 1280x720 -framerate 30/1 -color_range tv -colorspace bt709 -color_primaries bt709 -color_trc bt709 -thread_queue_size 64 -i pipe:0 "),
            "{args}"
        );
        // 有声音的片段各一个输入，输入端精确截取
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
        assert!(
            args.contains(
                "-colorspace bt709 -color_primaries bt709 -color_trc bt709 -color_range tv"
            ),
            "{args}"
        );
        assert!(args.ends_with("-y C:/out/final.mp4"), "{args}");
        let g = &job.filter_graph;
        assert!(g.contains("[0:v]ass=subs.ass[vout]"), "{g}");
        // bgm：-6dB、淡入 1 秒、淡出 2 秒（片段 8 秒 → 第 6 秒开始淡出）、从 0 开始
        assert!(g.contains("[3:a]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,volume=0.501187,afade=t=in:st=0:d=1.000000,afade=t=out:st=6.000000:d=2.000000,adelay=delays=0S:all=1[a3]"), "{g}");
        // 第二段视频的原声延后 5 秒 = 240000 个样本
        assert!(g.contains("adelay=delays=240000S:all=1[a2]"), "{g}");
        assert!(
            g.contains("anullsrc=r=48000:cl=stereo,atrim=duration=8.000000[abase]"),
            "{g}"
        );
        assert!(
            g.contains("[abase][a1][a2][a3]amix=inputs=4:normalize=0:duration=first[aout]"),
            "{g}"
        );
    }

    #[test]
    fn odd_output_size_is_rounded_to_even() {
        // NV12 / yuv420p 要求宽高为偶数，853x480 这类尺寸直接导出会失败
        let (p, _) = sample_project();
        let mut s = settings();
        s.width = 853;
        s.height = 481;
        let job = build_job(&p, &s, false).unwrap();
        let args = args_text(&job);
        assert!(args.contains("-s 852x480"), "{args}");
        assert!(!args.contains("853"), "{args}");
    }

    #[test]
    fn duration_covers_gaps() {
        // 空隙由合成器画成黑场，ffmpeg 这边只要时长对
        let (mut p, ids) = sample_project();
        move_clip(&mut p, ids.v1, ids.video_track, Time::from_secs(20)).unwrap();
        let job = build_job(&p, &settings(), false).unwrap();
        assert!(
            job.filter_graph.contains("[0:v]null[vout]"),
            "{}",
            job.filter_graph
        );
        assert_eq!(job.duration, Time::from_secs(25));
        assert!(args_text(&job).contains("-t 25.000000"));
    }

    #[test]
    fn audio_only_project_has_only_audio_inputs() {
        let (mut p, ids) = sample_project();
        delete_clips(&mut p, &[ids.v1, ids.v2]).unwrap();
        let job = build_job(&p, &settings(), false).unwrap();
        let args = args_text(&job);
        assert!(!args.contains("clip.mp4"), "{args}");
        assert!(job.filter_graph.contains("[1:a]"), "{}", job.filter_graph);
    }

    #[test]
    fn silent_still_image_is_not_an_input() {
        let (mut p, ids) = sample_project();
        let asset = p.assets.get_mut(&ids.video_asset).unwrap();
        asset.video = asset.video.map(|v| ve_core::VideoInfo { still: true, ..v });
        asset.audio = None;
        let job = build_job(&p, &settings(), false).unwrap();
        let args = args_text(&job);
        assert!(!args.contains("clip.mp4"), "{args}");
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
