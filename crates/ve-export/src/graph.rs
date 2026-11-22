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
    /// AV1：比 HEVC 还小，新设备才放得了（硬件编码要 RTX 40 系以上，不行时用软件编码）。
    Av1,
}

/// 视频里的声音怎么编（剪映导出的「音频」：格式、码率、采样率）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoAudio {
    /// 编码。
    pub codec: AudioCodec,
    /// 采样率（Hz）。
    pub sample_rate: u32,
}

impl VideoAudio {
    /// 默认：AAC 192 kbps、48 kHz。
    pub const DEFAULT: Self = Self {
        codec: AudioCodec::Aac(192),
        sample_rate: 48_000,
    };
}

impl Default for VideoAudio {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 视频里声音的编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    /// AAC，码率 kbps。
    Aac(u32),
    /// PCM（16 位无损，只能放进 MOV）。
    Pcm,
}

/// 导出什么（剪映导出对话框的「视频导出」/「音频导出」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Output {
    /// 视频（带声音）。
    #[default]
    Video,
    /// 只有声音。
    Audio(AudioFormat),
}

/// 音频导出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioFormat {
    /// MP3（320 kbps）。
    #[default]
    Mp3,
    /// WAV（16 位无损）。
    Wav,
    /// AAC（m4a，256 kbps）。
    Aac,
}

impl AudioFormat {
    /// 文件扩展名。
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Aac => "m4a",
        }
    }

    /// 大致码率（kbps，估计大小用）。
    #[must_use]
    pub const fn kbps(self) -> u32 {
        match self {
            Self::Mp3 => 320,
            Self::Wav => 1536,
            Self::Aac => 256,
        }
    }

    const fn codec_args(self) -> &'static [&'static str] {
        match self {
            Self::Mp3 => &["-c:a", "libmp3lame", "-b:a", "320k"],
            Self::Wav => &["-c:a", "pcm_s16le"],
            Self::Aac => &["-c:a", "aac", "-b:a", "256k"],
        }
    }
}

/// SVT-AV1 画面短边小于这个（像素）就不能按码率编码。
const SVT_AV1_MIN_RC_SIDE: u32 = 72;

/// 推荐码率（kbps）：按每像素每帧 0.15 比特估，720p30 约 4 Mbps、1080p30 约 9 Mbps；HEVC 再省三成，
/// AV1 约是 H.264 的一半多一点。
#[must_use]
pub fn recommended_kbps(width: u32, height: u32, frame_rate: FrameRate, codec: Codec) -> u32 {
    let pixels = f64::from(width) * f64::from(height);
    let kbps = pixels * frame_rate.as_f64() * 0.15 / 1000.0;
    let kbps = match codec {
        Codec::H264 => kbps,
        Codec::Hevc => kbps * 0.7,
        // AV1 比 HEVC 再省两成左右
        Codec::Av1 => kbps * 0.55,
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
    /// 导出视频还是只导出声音。
    pub output: Output,
    /// 目标响度（LUFS，如 -14）；`None` 不调整。
    pub loudness_lufs: Option<i8>,
    /// 只导出时间轴上的这一段（入出点区域）；`None` 导出全部。
    pub range: Option<(Time, Time)>,
    /// 视频里的声音（编码、码率、采样率）。MP4 还是 MOV 看 `path` 的扩展名。
    pub video_audio: VideoAudio,
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
        .filter(|t| t.kind == kind && t.sounds())
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
    first_index: usize,
    args: &mut Vec<OsString>,
) -> Vec<AudioInput<'a>> {
    let mut audio_inputs = Vec::new();
    for &clip in clips {
        let Some(asset) = clip.asset().and_then(|a| project.assets.get(&a)) else {
            continue;
        };
        // 音频已分离的视频片段：声音由分离出来的那段出，这里不再导一遍
        if asset.audio.is_none() || clip.audio_split {
            continue;
        }
        // 转场处声音越过片段边界（和预览同一套算法）
        let Some(sound) = ve_core::compose::audio_span(project, clip.id) else {
            continue;
        };
        // 曲线变速的片段分成若干小段（和预览同一套切法）
        let pieces = ve_core::compose::audio_pieces(clip, &sound);
        let whole = pieces.len() == 1;
        for piece in pieces {
            args.extend([
                "-ss".into(),
                secs(piece.source_start).into(),
                "-t".into(),
                // 变速：截取的是素材里用到的那一段
                secs(piece.len.scaled(f64::from(piece.speed))).into(),
                "-i".into(),
                asset.path.clone().into_os_string(),
            ]);
            audio_inputs.push(AudioInput {
                index: audio_inputs.len() + first_index,
                clip,
                sound,
                piece,
                whole,
            });
        }
    }
    audio_inputs
}

/// 一路音频输入：ffmpeg 输入序号、片段、声音实际放的那一段、这一路是其中哪一小段。
struct AudioInput<'a> {
    index: usize,
    clip: &'a Clip,
    sound: ve_core::compose::AudioSpan,
    piece: ve_core::compose::AudioPiece,
    /// 整段只有这一路（没有曲线变速）：淡入淡出用 afade；分了段时用音量表达式接起来。
    whole: bool,
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
fn speed_filter(clip: &Clip, speed: f32) -> String {
    #[expect(clippy::float_cmp, reason = "原速是精确的 1.0")]
    if speed == 1.0 {
        return String::new();
    }
    if !clip.keep_pitch {
        return format!(
            ",asetrate={:.3},aresample={MIX_RATE}",
            f64::from(MIX_RATE) * f64::from(speed)
        );
    }
    let speed = f64::from(speed);
    // 每级最多 ×2 或 ÷2：级数 = ⌈|log2 倍数|⌉，每级一样大
    #[expect(clippy::cast_possible_truncation, reason = "倍数 0.1–10，最多 4 级")]
    let stages = (speed.log2().abs().ceil() as i32).max(1);
    let step = speed.powf(1.0 / f64::from(stages));
    format!(",atempo={step:.6}").repeat(usize::try_from(stages).unwrap_or(1))
}

/// 音量滤镜：没有音量关键帧时是固定倍数；有的时候是按帧求值的表达式，和预览的
/// [`ve_core::keyframe::value_of`] 同一套插值（dB 按缓动曲线插，再换成倍数）。
/// 滤镜里的 `t` 从这一路声音开始放的时刻算起，`shift` 把它换成相对片段起点的时间。
fn volume_filter(clip: &Clip, shift: Time) -> String {
    volume_filter_with(clip, shift, None)
}

/// 同 [`volume_filter`]；`fade` 给了的时候（分段的声音），淡入淡出也写进表达式：
/// （这一路开始的时刻离声音开始多远、声音总长、淡入、淡出）。
fn volume_filter_with(clip: &Clip, shift: Time, fade: Option<(Time, Time, Time, Time)>) -> String {
    let keys = clip.animation.keys(ve_core::Prop::Volume);
    let envelope = fade.map(|(lead, total, fi, fo)| {
        let at = format!("(t+{})", secs(lead));
        let mut e = String::from("1");
        if fi > Time::ZERO {
            e = format!("min({e},max(0,{at}/{}))", secs(fi));
        }
        if fo > Time::ZERO {
            e = format!("min({e},max(0,({}-{at})/{}))", secs(total), secs(fo));
        }
        e
    });
    if keys.is_empty() {
        let gain = 10f64.powf(f64::from(clip.gain_db) / 20.0);
        return envelope.map_or_else(
            || format!("volume={gain:.6}"),
            |e| format!("volume='{gain:.6}*{e}':eval=frame"),
        );
    }
    let at = format!("(t+{})", secs(shift));
    let mut expr = format!("{:.4}", keys[keys.len() - 1].value);
    // 从后往前套：if(T<k[i+1].at, 第 i 段, 后面的)
    for pair in keys.windows(2).rev() {
        let (a, b) = (pair[0], pair[1]);
        let u = format!("(({at}-{})/{})", secs(a.at), secs(b.at - a.at));
        let eased = match a.easing {
            ve_core::Easing::Linear => u,
            ve_core::Easing::EaseIn => format!("pow({u},3)"),
            ve_core::Easing::EaseOut => format!("(1-pow(1-{u},3))"),
            ve_core::Easing::EaseInOut => {
                format!("if(lt({u},0.5),4*pow({u},3),1-pow(2-2*{u},3)/2)")
            }
        };
        expr = format!(
            "if(lt({at},{}),{:.4}+({:.4})*{eased},{expr})",
            secs(b.at),
            a.value,
            b.value - a.value
        );
    }
    expr = format!(
        "if(lt({at},{}),{:.4},{expr})",
        secs(keys[0].at),
        keys[0].value
    );
    envelope.map_or_else(
        || format!("volume='pow(10,({expr})/20)':eval=frame"),
        |e| format!("volume='pow(10,({expr})/20)*{e}':eval=frame"),
    )
}

/// 片段的声音处理（和预览同一串滤镜，[`ve_core::AudioFx::filter_spec`]），前面带逗号；
/// 不处理是空串。降噪的固定延迟：末尾先补同样长的静音，处理完再把开头那么长剪掉，长度和位置都不变。
fn fx_filter(fx: &ve_core::AudioFx) -> String {
    let Some(spec) = fx.filter_spec() else {
        return String::new();
    };
    match fx.latency_samples(MIX_RATE) {
        0 => format!(",{spec}"),
        l => format!(",apad=pad_len={l},{spec},atrim=start_sample={l},asetpts=N/SR/TB"),
    }
}

fn audio_chain(
    graph: &mut String,
    audio_inputs: &[AudioInput<'_>],
    duration: Time,
    loudness: Option<i8>,
) {
    let _ = writeln!(
        graph,
        "anullsrc=r={MIX_RATE}:cl=stereo,atrim=duration={}[abase];",
        secs(duration)
    );
    let mut mix_labels = String::from("[abase]");
    for &AudioInput {
        index,
        clip,
        sound,
        piece,
        whole,
    } in audio_inputs
    {
        let speed = speed_filter(clip, piece.speed);
        let (fade_in, fade_out) = (sound.fade_in, sound.fade_out);
        let volume = if whole {
            volume_filter(clip, sound.start - clip.start)
        } else {
            volume_filter_with(
                clip,
                piece.start - clip.start,
                Some((
                    piece.start - sound.start,
                    sound.end - sound.start,
                    fade_in,
                    fade_out,
                )),
            )
        };
        let fx = fx_filter(&clip.audio_fx);
        let _ = write!(
            graph,
            "[{index}:a]aresample={MIX_RATE}{speed},aformat=sample_fmts=fltp:channel_layouts=stereo{fx},{volume}"
        );
        // 和预览一样：转场处两段的声音叠在一起交叉淡化
        if whole && fade_in > Time::ZERO {
            let _ = write!(graph, ",afade=t=in:st=0:d={}", secs(fade_in));
        }
        if whole && fade_out > Time::ZERO {
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
            piece.start.to_samples(MIX_RATE)
        );
        let _ = write!(mix_labels, "[a{index}]");
    }
    if audio_inputs.is_empty() {
        graph.push_str("[abase]anull[amixed];");
    } else {
        let _ = writeln!(
            graph,
            "{mix_labels}amix=inputs={}:normalize=0:duration=first[amixed];",
            audio_inputs.len() + 1
        );
    }
    // 目标响度（剪映导出的「目标响度」）：EBU R128 单遍，真峰值留 1.5 dB；loudnorm 会升到 192k，再降回来。
    // 整段都是静音时 loudnorm 会算出 NaN（AAC 编码器直接报错）：没有声音输入就不做，
    // 有输入也把 NaN 换成 0 兜底
    match loudness.filter(|_| !audio_inputs.is_empty()) {
        Some(lufs) => {
            let _ = write!(
                graph,
                "[amixed]loudnorm=I={lufs}:TP=-1.5:LRA=11,aresample={MIX_RATE},\
                 aeval=exprs='if(isnan(val(0)),0,val(0))|if(isnan(val(1)),0,val(1))':c=same[aout]"
            );
        }
        None => graph.push_str("[amixed]anull[aout]"),
    }
}

/// 向下取偶数（至少 2）。
fn rate_str(rate: FrameRate) -> String {
    format!("{}/{}", rate.num, rate.den)
}

/// 只有声音时的输出参数。
fn push_audio_output(
    args: &mut Vec<OsString>,
    settings: &ExportSettings,
    format: AudioFormat,
    duration: Time,
) {
    args.extend(["-/filter_complex", "filter.txt", "-map", "[aout]", "-vn"].map(OsString::from));
    args.extend(format.codec_args().iter().map(OsString::from));
    args.extend(["-t".into(), secs(duration).into(), "-y".into()]);
    args.push(settings.path.clone().into_os_string());
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
        (Encoder::Nvenc, Codec::Av1) => &["-c:v", "av1_nvenc", "-preset", "p5", "-rc", "vbr"],
        (Encoder::X264, Codec::Av1) => &["-c:v", "libsvtav1", "-preset", "8"],
    };
    args.extend(codec.iter().map(OsString::from));
    let a = settings.video_audio;
    let audio: Vec<String> = match a.codec {
        AudioCodec::Aac(audio_kbps) => vec![
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            format!("{audio_kbps}k"),
        ],
        AudioCodec::Pcm => vec!["-c:a".into(), "pcm_s16le".into()],
    };
    args.extend(audio.into_iter().map(OsString::from));
    args.extend(["-ar".to_owned(), a.sample_rate.to_string()].map(OsString::from));
    let svt_av1 = matches!(
        (settings.encoder, settings.codec),
        (Encoder::X264, Codec::Av1)
    );
    // SVT-AV1：画面很小时它会关掉自适应量化，按码率编码又不许关，编码器打不开——这时按画质（CRF）编
    if svt_av1 && settings.width.min(settings.height) < SVT_AV1_MIN_RC_SIDE {
        args.extend(["-crf", "35"].map(OsString::from));
    } else {
        args.extend(["-b:v".to_owned(), format!("{kbps}k")].map(OsString::from));
    }
    // SVT-AV1 按码率编码时不支持峰值码率（只有 CRF 模式才行），给了就打不开编码器
    if !svt_av1 {
        args.extend(
            [
                "-maxrate".to_owned(),
                format!("{}k", kbps * 3 / 2),
                "-bufsize".to_owned(),
                format!("{}k", kbps * 2),
            ]
            .map(OsString::from),
        );
    }
    args.extend(
        [
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
    let with_video = settings.output == Output::Video;
    if with_video {
        push_video_input(&mut args, settings);
    }
    // 声音：视频轨片段的原声在前，音频轨片段在后
    let video = media_clips(project, TrackKind::Video);
    let audio = media_clips(project, TrackKind::Audio);
    let all: Vec<&Clip> = video.iter().chain(audio.iter()).copied().collect();
    let audio_inputs = push_audio_inputs(project, &all, usize::from(with_video), &mut args);
    let mut graph = String::new();
    if with_video {
        video_chain(&mut graph, with_subtitles);
    }
    audio_chain(&mut graph, &audio_inputs, duration, settings.loudness_lufs);
    match settings.output {
        Output::Video => push_output(&mut args, settings, duration),
        Output::Audio(format) => push_audio_output(&mut args, settings, format, duration),
    }
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
            output: Output::Video,
            loudness_lufs: None,
            range: None,
            video_audio: VideoAudio::default(),
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
        assert_eq!(
            speed_filter(&c, c.speed),
            ",atempo=0.500000,atempo=0.500000"
        );
        c.speed = 1.5;
        assert_eq!(speed_filter(&c, c.speed), ",atempo=1.500000");
        c.speed = 1.0;
        assert_eq!(speed_filter(&c, c.speed), "");
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
    fn av1_and_audio_choices_reach_the_command_line() {
        let (p, _) = sample_project();
        let mut s = settings();
        s.codec = Codec::Av1;
        let args = args_text(&build_job(&p, &s, false).unwrap());
        assert!(args.contains("-c:v libsvtav1"), "{args}");
        s.encoder = Encoder::Nvenc;
        assert!(args_text(&build_job(&p, &s, false).unwrap()).contains("-c:v av1_nvenc"));
        // 默认：AAC 192k、48 kHz
        let plain = args_text(&build_job(&p, &settings(), false).unwrap());
        assert!(plain.contains("-c:a aac -b:a 192k -ar 48000"), "{plain}");
        // AAC 320k、44.1 kHz
        s.video_audio = VideoAudio {
            codec: AudioCodec::Aac(320),
            sample_rate: 44_100,
        };
        let aac = args_text(&build_job(&p, &s, false).unwrap());
        assert!(aac.contains("-c:a aac -b:a 320k -ar 44100"), "{aac}");
        // PCM（放进 MOV）
        s.video_audio.codec = AudioCodec::Pcm;
        s.path = PathBuf::from("C:/out/final.mov");
        let pcm = args_text(&build_job(&p, &s, false).unwrap());
        assert!(pcm.contains("-c:a pcm_s16le -ar 44100"), "{pcm}");
        assert!(pcm.ends_with("final.mov"), "{pcm}");
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
    fn muted_tracks_are_left_out_of_the_mix() {
        use ve_core::edit::{TrackFlag, set_track_flag};
        let (mut p, ids) = sample_project();
        set_track_flag(&mut p, ids.audio_track, TrackFlag::Muted, true).unwrap();
        let args = args_text(&build_job(&p, &settings(), true).unwrap());
        assert!(!args.contains("bgm.mp3"), "{args}");
        assert!(args.contains("clip.mp4"), "{args}");
        // 隐藏视频轨不影响它的声音（声音只看静音）
        set_track_flag(&mut p, ids.video_track, TrackFlag::Hidden, true).unwrap();
        let hidden = args_text(&build_job(&p, &settings(), true).unwrap());
        assert!(hidden.contains("clip.mp4"), "{hidden}");
    }

    #[test]
    fn audio_fx_go_before_the_volume_with_the_denoise_delay_cut() {
        let (mut p, ids) = sample_project();
        let fx = ve_core::AudioFx {
            denoise: 0.5,
            eq: [3.0, 0.0, 0.0],
            compress: 0.0,
            ..ve_core::AudioFx::NONE
        };
        ve_core::edit::set_audio_fx(&mut p, ids.a1, fx).unwrap();
        let g = build_job(&p, &settings(), true).unwrap().filter_graph;
        assert!(
            g.contains("stereo,apad=pad_len=1200,afftdn=nr=18.00:nf=-50,bass=g=3.00:f=100,atrim=start_sample=1200,asetpts=N/SR/TB,volume="),
            "{g}"
        );
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
            g.contains("[abase][a1][a2][a3]amix=inputs=4:normalize=0:duration=first[amixed];"),
            "{g}"
        );
        assert!(g.ends_with("[amixed]anull[aout]"), "{g}");
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

    /// 导出的音量表达式交给 ffmpeg 真算一遍，和预览的插值（dB 按缓动插、再换成倍数）对得上。
    #[test]
    fn volume_keyframes_export_like_the_preview() {
        use ve_core::{Easing, Prop};
        let (mut p, ids) = sample_project();
        let a1 = ids.a1;
        ve_core::edit::add_keyframe_props(&mut p, a1, Time::ZERO, &[Prop::Volume]).unwrap();
        for (at, db) in [(0.5, -6.0), (1.0, -20.0), (1.5, -3.0), (2.0, 6.0)] {
            ve_core::edit::apply_gain_at(&mut p, a1, Time::from_secs_f64(at), db).unwrap();
        }
        for (at, e) in [
            (0.0, Easing::Linear),
            (0.5, Easing::EaseIn),
            (1.0, Easing::EaseOut),
            (1.5, Easing::EaseInOut),
        ] {
            ve_core::edit::set_keyframe_easing(
                &mut p,
                a1,
                Time::from_secs_f64(at),
                &[Prop::Volume],
                e,
            )
            .unwrap();
        }
        let clip = (**p.timeline.find_clip(a1).unwrap().1).clone();
        // 这一路声音比片段早 0.25 秒开始放（转场时会这样）
        let shift = Time::from_millis(-250);
        let filter = volume_filter(&clip, shift);
        let out = crate::run::ffmpeg_command(crate::run::ffmpeg_program())
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("aevalsrc=1:s=48000:d=2.5")
            .args([
                "-af",
                &format!("asetnsamples=n=64,{filter}"),
                "-f",
                "f32le",
                "-ac",
                "1",
                "pipe:1",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let samples: Vec<f32> = out
            .stdout
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        for i in (0..samples.len()).step_by(64 * 20) {
            #[expect(clippy::cast_precision_loss, reason = "测试")]
            let t = Time::from_secs_f64(i as f64 / 48_000.0) + shift;
            let want = 10f32.powf(clip.gain_db_at(t) / 20.0);
            let got = samples[i];
            assert!(
                (got - want).abs() <= want * 0.02 + 1e-4,
                "片段内 {:.3} 秒：导出 {got}，预览 {want}",
                t.as_secs_f64()
            );
        }
    }

    #[test]
    fn separated_audio_is_not_exported_twice() {
        let (mut p, ids) = sample_project();
        ve_core::edit::separate_audio(&mut p, &[ids.v1]).unwrap();
        let v1 = std::sync::Arc::clone(p.timeline.find_clip(ids.v1).unwrap().1);
        let mut args = Vec::new();
        assert_eq!(push_audio_inputs(&p, &[v1.as_ref()], 0, &mut args).len(), 0);
    }
}
