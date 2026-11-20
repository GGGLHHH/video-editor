//! 执行导出：写临时文件、启动 ffmpeg、逐帧渲染画面写进它的标准输入、取消、NVENC 失败退回 x264。

use crate::{Encoder, ExportError, ExportSettings, build_job, has_subtitles, write_ass};
use std::fmt::Write as _;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use ve_core::Project;

/// ffmpeg 可执行文件：环境变量 `VE_FFMPEG` 优先，否则 PATH 上的 `ffmpeg`
/// （Windows 上先找程序自己所在的目录，打包时 ffmpeg.exe 就放在那里）。
pub(crate) fn ffmpeg_program() -> std::ffi::OsString {
    std::env::var_os("VE_FFMPEG").unwrap_or_else(|| "ffmpeg".into())
}

/// 启动 ffmpeg 的命令：Windows 上不弹命令行窗口（发布版是图形程序，没有自己的控制台，
/// 不加这个标志每次导出都会冒出一个黑窗口）。
pub(crate) fn ffmpeg_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        /// `CREATE_NO_WINDOW`
        const NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(NO_WINDOW);
    }
    cmd
}

/// 取素材 `input` 在 `at`（素材时间）那一帧，存成 PNG `out`（定格用）。
///
/// # Errors
/// 找不到 ffmpeg、ffmpeg 失败（错误信息取它的输出）、没写出文件。
pub fn grab_frame(
    input: &std::path::Path,
    at: ve_core::Time,
    out: &std::path::Path,
) -> Result<(), String> {
    let secs = format!("{:.6}", at.as_secs_f64().max(0.0));
    let output = ffmpeg_command(ffmpeg_program())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-ss",
            &secs,
            "-i",
        ])
        .arg(input)
        .args(["-frames:v", "1", "-update", "1"])
        .arg(out)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "找不到 ffmpeg".to_owned(),
            _ => e.to_string(),
        })?;
    if !output.status.success() || !out.is_file() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if err.is_empty() {
            "ffmpeg 没有取出画面".to_owned()
        } else {
            err
        });
    }
    Ok(())
}

/// 倒放时每段的长度：`reverse` 滤镜要把整段的帧都存在内存里，分段倒、再倒着拼起来。
const REVERSE_CHUNK: ve_core::Time = ve_core::Time::from_secs(10);

/// 跑一次 ffmpeg，失败时带上它的报错。
fn run_ffmpeg(args: &[std::ffi::OsString]) -> Result<(), String> {
    run_ffmpeg_in(None, args)
}

/// 同 [`run_ffmpeg`]，在目录 `dir` 里运行（滤镜参数里的文件名用相对路径，免得 Windows 盘符的冒号被当成分隔符）。
fn run_ffmpeg_in(dir: Option<&std::path::Path>, args: &[std::ffi::OsString]) -> Result<(), String> {
    let mut cmd = ffmpeg_command(ffmpeg_program());
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    let output = cmd
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "找不到 ffmpeg".to_owned(),
            _ => e.to_string(),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(failure_message(output.status, &output.stderr))
    }
}

/// ffmpeg 失败时给人看的原因：它的报错；什么都没说（比如崩了）时写退出码，别给一句空话。
fn failure_message(status: std::process::ExitStatus, stderr: &[u8]) -> String {
    let said = String::from_utf8_lossy(stderr).trim().to_owned();
    if !said.is_empty() {
        return said;
    }
    // Windows 的崩溃码（0xC0000005 访问违例这类）按十六进制写，好查
    status.code().map_or_else(
        || format!("ffmpeg 异常退出（{status}），没有输出错误信息"),
        |code| format!("ffmpeg 异常退出，退出码 0x{code:08X}，没有输出错误信息"),
    )
}

/// 倒放（剪映「倒放」）：把素材 `input` 里 `[from, from + len)` 这一段倒着生成到 `out`
/// （视频 H.264 + AAC 的 mp4；`video` 为假时只有声音，写 m4a）。按 10 秒一段倒，再倒着拼起来。
///
/// # Errors
/// 找不到 ffmpeg、ffmpeg 失败、读写临时文件失败。
pub fn reverse_segment(
    input: &std::path::Path,
    from: ve_core::Time,
    len: ve_core::Time,
    video: bool,
    audio: bool,
    out: &std::path::Path,
) -> Result<(), String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let secs = |t: ve_core::Time| format!("{:.6}", t.as_secs_f64().max(0.0));
    let ext = if video { "mp4" } else { "m4a" };
    let mut parts = Vec::new();
    let mut at = ve_core::Time::ZERO;
    while at < len {
        let piece = REVERSE_CHUNK.min(len - at);
        let part = dir.path().join(format!("{}.{ext}", parts.len()));
        let mut args: Vec<std::ffi::OsString> = vec![
            "-ss".into(),
            secs(from + at).into(),
            "-t".into(),
            secs(piece).into(),
            "-i".into(),
            input.as_os_str().to_owned(),
        ];
        if video {
            args.extend(
                [
                    "-vf", "reverse", "-c:v", "libx264", "-preset", "veryfast", "-crf", "16",
                    "-pix_fmt", "yuv420p",
                ]
                .map(std::ffi::OsString::from),
            );
        } else {
            args.push("-vn".into());
        }
        if audio {
            args.extend(
                ["-af", "areverse", "-c:a", "aac", "-b:a", "256k"].map(std::ffi::OsString::from),
            );
        } else {
            args.push("-an".into());
        }
        args.push(part.clone().into_os_string());
        run_ffmpeg(&args)?;
        parts.push(part);
        at += piece;
    }
    // 倒着拼：最后一段（倒过来以后）放最前面
    let list = dir.path().join("list.txt");
    let mut text = String::new();
    for p in parts.iter().rev() {
        let _ = writeln!(
            text,
            "file '{}'",
            p.display().to_string().replace('\'', "'\\''")
        );
    }
    std::fs::write(&list, text).map_err(|e| e.to_string())?;
    let args: Vec<std::ffi::OsString> = vec![
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        list.into_os_string(),
        "-c".into(),
        "copy".into(),
        out.as_os_str().to_owned(),
    ];
    run_ffmpeg(&args)?;
    if out.is_file() {
        Ok(())
    } else {
        Err("ffmpeg 没有写出倒放文件".to_owned())
    }
}

/// 视频防抖（剪映「视频防抖」）：把素材 `input` 里 `[from, from + len)` 这一段两遍处理——先分析抖动
/// （`vidstabdetect`），再按平滑窗口 `smoothing` 帧把画面拉稳并自动放大盖住黑边（`vidstabtransform`）——
/// 写到 `out`（H.264 + 原声 AAC 的 mp4）。
///
/// # Errors
/// 找不到 ffmpeg、ffmpeg 失败（比如这个 ffmpeg 没带 vidstab）、读写临时文件失败。
pub fn stabilize_segment(
    input: &std::path::Path,
    from: ve_core::Time,
    len: ve_core::Time,
    smoothing: u32,
    out: &std::path::Path,
) -> Result<(), String> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    // ffmpeg 在临时目录里跑（t.trf 用相对路径，避开盘符冒号）：输入输出都要换成绝对路径
    let input = std::path::absolute(input).map_err(|e| e.to_string())?;
    let out_abs = std::path::absolute(out).map_err(|e| e.to_string())?;
    let secs = |t: ve_core::Time| format!("{:.6}", t.as_secs_f64().max(0.0));
    let range = |args: &mut Vec<std::ffi::OsString>| {
        args.extend([
            "-ss".into(),
            secs(from).into(),
            "-t".into(),
            secs(len).into(),
            "-i".into(),
            input.as_os_str().to_owned(),
        ]);
    };
    // 第一遍：分析，写 t.trf
    let mut detect = Vec::new();
    range(&mut detect);
    detect.extend(
        [
            "-vf",
            "vidstabdetect=shakiness=6:accuracy=15:result=t.trf",
            "-f",
            "null",
            "-",
        ]
        .map(std::ffi::OsString::from),
    );
    run_ffmpeg_in(Some(dir.path()), &detect)?;
    // 第二遍：按分析结果拉稳
    let mut transform = Vec::new();
    range(&mut transform);
    let vf = format!(
        "vidstabtransform=input=t.trf:smoothing={smoothing}:optzoom=1:interpol=bicubic,unsharp=5:5:0.6:3:3:0.3"
    );
    transform.extend(
        [
            "-map", "0:v:0", "-map", "0:a:0?", "-vf", &vf, "-c:v", "libx264", "-preset",
            "veryfast", "-crf", "16", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "256k",
        ]
        .map(std::ffi::OsString::from),
    );
    transform.push(out_abs.clone().into_os_string());
    if let Err(e) = run_ffmpeg_in(Some(dir.path()), &transform) {
        // 写了一半的文件留着没用
        drop(std::fs::remove_file(&out_abs));
        return Err(e);
    }
    if out.is_file() {
        Ok(())
    } else {
        Err("ffmpeg 没有写出防抖文件".to_owned())
    }
}

/// 代理文件的短边（剪映代理模式的低清副本）。
pub const PROXY_SHORT_SIDE: u32 = 540;

/// 给素材 `input` 生成预览用的代理文件 `out`：短边 [`PROXY_SHORT_SIDE`]（原片更小时不放大），
/// H.264、关键帧间隔短（拖动快），声音一起转。导出时还是用原片。
///
/// # Errors
/// 找不到 ffmpeg、ffmpeg 失败、没写出文件。
pub fn make_proxy(input: &std::path::Path, out: &std::path::Path) -> Result<(), String> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let s = PROXY_SHORT_SIDE;
    // 横的压高、竖的压宽；另一边按比例取偶数
    let scale = format!("scale='if(gt(iw,ih),-2,min(iw,{s}))':'if(gt(iw,ih),min(ih,{s}),-2)'");
    let mut args: Vec<std::ffi::OsString> = vec!["-i".into(), input.as_os_str().to_owned()];
    args.extend(
        [
            "-map", "0:v:0", "-map", "0:a:0?", "-vf", &scale, "-c:v", "libx264", "-preset",
            "veryfast", "-crf", "23", "-g", "15", "-bf", "0", "-pix_fmt", "yuv420p", "-c:a", "aac",
            "-b:a", "128k",
        ]
        .map(std::ffi::OsString::from),
    );
    args.push(out.as_os_str().to_owned());
    run_ffmpeg(&args)?;
    if out.is_file() {
        Ok(())
    } else {
        Err("ffmpeg 没有写出代理文件".to_owned())
    }
}

/// 导出前检查并准备：路径转绝对（ffmpeg 在临时目录里运行），素材都在，输出不是素材本身。
/// 返回（绝对路径版工程，最终输出路径，临时输出路径）。
fn prepare(
    project: &Project,
    settings: &ExportSettings,
) -> Result<(Project, std::path::PathBuf, std::path::PathBuf), ExportError> {
    // 只导出入出点区域：先把那一段切出来挪到从 0 开始
    let mut project = match settings.range {
        Some((from, to)) => ve_core::edit::extract_range(project, from, to)
            .map_err(|_| ExportError::EmptyTimeline)?,
        None => project.clone(),
    };
    for asset in project.assets.values_mut() {
        asset.path = std::path::absolute(&asset.path)?;
    }
    // 先查素材文件都在：否则只能拿到 ffmpeg 的原始报错（还会白跑一次 NVENC 再重试 x264）
    let mut missing: Vec<String> = project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter_map(|c| c.asset().and_then(|a| project.assets.get(&a)))
        .filter(|a| !a.path.exists())
        .map(|a| a.name.clone())
        .collect();
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        return Err(ExportError::MissingMedia(missing));
    }
    let final_path = std::path::absolute(&settings.path)?;
    // Windows 路径不区分大小写
    let key = |p: &std::path::Path| p.to_string_lossy().to_lowercase();
    if project
        .assets
        .values()
        .any(|a| key(&a.path) == key(&final_path))
    {
        return Err(ExportError::OutputIsInput);
    }
    // 先写同目录的临时文件，成功后再改名：失败 / 取消不会动到已有的同名文件
    let part = part_path(&final_path);
    Ok((project, final_path, part))
}

/// 临时输出文件：`名字.ve-part.扩展名`（保留扩展名，ffmpeg 靠它选容器）。
fn part_path(path: &std::path::Path) -> std::path::PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map_or_else(|| "mp4".into(), |e| e.to_string_lossy().into_owned());
    path.with_file_name(format!("{stem}.ve-part.{ext}"))
}

/// 导出。`on_progress` 收到单调不减的 0–1 进度，成功时最后一次为 1.0；
/// `cancel` 置真后尽快停止并删除半成品。NVENC 失败时自动用 x264 重试一次。
///
/// # Errors
/// 时间轴为空、找不到 ffmpeg、ffmpeg 失败、被取消。
pub fn export(
    project: &Project,
    settings: &ExportSettings,
    mut on_progress: impl FnMut(f32),
    cancel: &AtomicBool,
) -> Result<(), ExportError> {
    export_with(
        &ffmpeg_program(),
        project,
        settings,
        &mut on_progress,
        cancel,
    )
}

/// 同 [`export`]，指定 ffmpeg 程序。
fn export_with(
    program: &std::ffi::OsStr,
    project: &Project,
    settings: &ExportSettings,
    mut on_progress: impl FnMut(f32),
    cancel: &AtomicBool,
) -> Result<(), ExportError> {
    let mut progress = Monotonic::new(&mut on_progress);
    let mut report = |p| progress.report(p);
    match run_once(program, project, settings, &mut report, cancel) {
        Err(ExportError::Ffmpeg { message, .. })
            if settings.encoder == Encoder::Nvenc && settings.output == crate::Output::Video =>
        {
            log::warn!("NVENC 导出失败，改用 x264 重试：{message}");
            let fallback = ExportSettings {
                encoder: Encoder::X264,
                ..settings.clone()
            };
            run_once(program, project, &fallback, &mut report, cancel)
        }
        other => other,
    }
}

/// 只往前报的进度：NVENC 失败改用 x264 从头重来时，进度条不倒退。
struct Monotonic<F: FnMut(f32)> {
    last: f32,
    f: F,
}

impl<F: FnMut(f32)> Monotonic<F> {
    const fn new(f: F) -> Self {
        Self { last: 0.0, f }
    }

    fn report(&mut self, p: f32) {
        if p > self.last {
            self.last = p;
            (self.f)(p);
        }
    }
}

fn run_once(
    program: &std::ffi::OsStr,
    project: &Project,
    settings: &ExportSettings,
    on_progress: &mut impl FnMut(f32),
    cancel: &AtomicBool,
) -> Result<(), ExportError> {
    let (project, final_path, part) = prepare(project, settings)?;
    let settings = &ExportSettings {
        path: part.clone(),
        ..settings.clone()
    };
    let project = &project;
    let with_subs = has_subtitles(project);
    let job = build_job(project, settings, with_subs)?;
    let dir = tempfile::tempdir()?;
    if with_subs {
        std::fs::write(dir.path().join("subs.ass"), write_ass(project))?;
    }
    std::fs::write(dir.path().join("filter.txt"), &job.filter_graph)?;
    if cancel.load(Ordering::Acquire) {
        return Err(ExportError::Cancelled);
    }
    let with_video = settings.output == crate::Output::Video;
    let mut child = ffmpeg_command(program)
        .args(&job.args)
        .current_dir(dir.path())
        .stdin(if with_video {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ExportError::FfmpegNotFound,
            _ => ExportError::Io(e),
        })?;
    // stderr 另起线程读，避免管道写满卡住 ffmpeg
    let stderr = child.stderr.take().map(|mut err| {
        std::thread::spawn(move || {
            let mut text = String::new();
            drop(err.read_to_string(&mut text));
            text
        })
    });
    let mut on_frame = |done: u64, total: u64| {
        // 写完最后一帧还要等 ffmpeg 收尾、改名，1.0 留到真正完成时再报
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "进度只需大致精度"
        )]
        let p = (done as f64 / total.max(1) as f64 * 0.99) as f32;
        on_progress(p);
    };
    // 只导出声音时没有画面要渲染：ffmpeg 自己读素材
    let rendered = if with_video {
        child.stdin.take().map_or(Ok(false), |stdin| {
            crate::render::render_video(project, settings, stdin, &mut on_frame, cancel)
        })
    } else {
        Ok(true)
    };
    if rendered.is_err() {
        drop(child.kill());
    }
    let status = child.wait()?;
    let message = stderr.and_then(|h| h.join().ok()).unwrap_or_default();
    if cancel.load(Ordering::Acquire) || matches!(rendered, Err(ExportError::Cancelled)) {
        drop(std::fs::remove_file(&part));
        return Err(ExportError::Cancelled);
    }
    if let Err(e) = rendered {
        drop(std::fs::remove_file(&part));
        return Err(e);
    }
    if !status.success() {
        drop(std::fs::remove_file(&part));
        let tail: String = message
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ExportError::Ffmpeg {
            code: status.code(),
            message: tail,
        });
    }
    if let Err(e) = std::fs::rename(&part, &final_path) {
        drop(std::fs::remove_file(&part));
        return Err(ExportError::Io(e));
    }
    on_progress(1.0);
    Ok(())
}

/// 默认导出设置：工程尺寸、帧率，12 Mbps，NVENC。
#[must_use]
pub const fn default_settings(project: &Project, path: std::path::PathBuf) -> ExportSettings {
    ExportSettings {
        path,
        width: project.settings.width,
        height: project.settings.height,
        frame_rate: project.settings.frame_rate,
        video_bitrate_kbps: 12_000,
        encoder: Encoder::Nvenc,
        codec: crate::Codec::H264,
        render: ve_render::RenderConfig { software: false },
        output: crate::Output::Video,
        loudness_lufs: None,
        range: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use ve_core::edit::{add_subtitle, append_asset};
    use ve_core::{Asset, AssetId, FrameRate, Project, Time};
    use ve_media::testing::{Fixture, fixture};

    fn project() -> Project {
        let mut p = Project::new();
        p.settings.width = 64;
        p.settings.height = 36;
        for f in [Fixture::FrameIndexed30, Fixture::ToneStereo48k] {
            let path = fixture(f);
            let r = ve_media::probe(&path).unwrap();
            let id = p.add_asset(Asset {
                id: AssetId(0),
                name: "x".into(),
                path,
                duration: r.duration,
                video: r.video,
                audio: r.audio,
            });
            append_asset(&mut p, id).unwrap();
        }
        add_subtitle(&mut p, Time::ZERO, Time::from_secs(1), "字幕 {测试}".into()).unwrap();
        p
    }

    fn settings(dir: &std::path::Path, encoder: Encoder) -> ExportSettings {
        ExportSettings {
            path: dir.join("out.mp4"),
            width: 64,
            height: 36,
            frame_rate: FrameRate::new(30, 1),
            video_bitrate_kbps: 500,
            encoder,
            codec: crate::Codec::H264,
            render: ve_render::RenderConfig { software: true },
            output: crate::Output::Video,
            loudness_lufs: None,
            range: None,
        }
    }

    #[test]
    fn export_produces_expected_duration() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings(dir.path(), Encoder::X264);
        let mut progress = Vec::new();
        export(
            &project(),
            &s,
            |p| progress.push(p),
            &AtomicBool::new(false),
        )
        .unwrap();
        let r = ve_media::probe(&s.path).unwrap();
        assert!(
            (r.duration - Time::from_secs(2)).0.abs() <= FrameRate::new(30, 1).frame_duration().0,
            "{:?}",
            r.duration
        );
        let v = r.video.unwrap();
        assert_eq!((v.width, v.height), (64, 36));
        assert!(r.audio.is_some());
        assert_eq!(progress.last().copied(), Some(1.0));
        assert!(progress.windows(2).all(|w| w[0] <= w[1]), "{progress:?}");
    }

    /// 整段的积分响度（LUFS），用 ffmpeg 的 ebur128 量。
    fn integrated_lufs(path: &std::path::Path) -> f64 {
        let out = ffmpeg_command(ffmpeg_program())
            .args(["-hide_banner", "-nostats", "-i"])
            .arg(path)
            .args(["-af", "ebur128", "-f", "null", "-"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stderr);
        let summary = &text[text.rfind("Summary").unwrap()..];
        let line = summary
            .lines()
            .find(|l| l.trim_start().starts_with("I:"))
            .unwrap();
        line.trim_start()[2..]
            .trim()
            .trim_end_matches("LUFS")
            .trim()
            .parse()
            .unwrap()
    }

    #[test]
    fn audio_fx_are_exported_with_the_same_length() {
        // 压缩把 440 Hz 正弦压小；降噪的延迟补回来，长度不变
        let dir = tempfile::tempdir().unwrap();
        let mut p = project();
        let tone = p
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .find(|c| {
                c.asset()
                    .and_then(|a| p.assets.get(&a))
                    .is_some_and(|a| a.video.is_none())
            })
            .unwrap()
            .id;
        let plain = |proj: &Project, name: &str| {
            let s = ExportSettings {
                path: dir.path().join(name),
                output: crate::Output::Audio(crate::AudioFormat::Wav),
                loudness_lufs: None,
                ..settings(dir.path(), Encoder::Nvenc)
            };
            export(proj, &s, |_| {}, &AtomicBool::new(false)).unwrap();
            let r = ve_media::probe(&s.path).unwrap();
            (integrated_lufs(&s.path), r.duration)
        };
        let (before, len_before) = plain(&p, "plain.wav");
        let fx = ve_core::AudioFx {
            denoise: 0.3,
            eq: [0.0; 3],
            compress: 1.0,
        };
        ve_core::edit::set_audio_fx(&mut p, tone, fx).unwrap();
        let (after, len_after) = plain(&p, "fx.wav");
        assert!(after < before - 3.0, "压缩后更小：{before} → {after}");
        assert!(
            (len_after - len_before).0.abs() <= Time::from_millis(30).0,
            "{len_before:?} vs {len_after:?}"
        );
    }

    #[test]
    fn audio_only_export_writes_just_the_sound_at_the_target_loudness() {
        let dir = tempfile::tempdir().unwrap();
        for (format, lufs) in [
            (crate::AudioFormat::Wav, -14),
            (crate::AudioFormat::Mp3, -23),
            (crate::AudioFormat::Aac, -16),
        ] {
            let s = ExportSettings {
                path: dir.path().join(format!("out.{}", format.extension())),
                output: crate::Output::Audio(format),
                loudness_lufs: Some(lufs),
                ..settings(dir.path(), Encoder::Nvenc)
            };
            let mut progress = Vec::new();
            export(
                &project(),
                &s,
                |p| progress.push(p),
                &AtomicBool::new(false),
            )
            .unwrap();
            let r = ve_media::probe(&s.path).unwrap();
            assert!(r.video.is_none(), "{format:?} 不该有画面");
            assert!(r.audio.is_some());
            assert!(
                (r.duration - Time::from_secs(2)).0.abs() <= Time::from_millis(60).0,
                "{:?}",
                r.duration
            );
            let got = integrated_lufs(&s.path);
            assert!(
                (got - f64::from(lufs)).abs() < 1.5,
                "{format:?}：目标 {lufs}，实际 {got}"
            );
            assert_eq!(progress.last().copied(), Some(1.0));
        }
    }

    #[test]
    fn video_export_with_target_loudness_works() {
        let dir = tempfile::tempdir().unwrap();
        let s = ExportSettings {
            loudness_lufs: Some(-14),
            ..settings(dir.path(), Encoder::X264)
        };
        export(&project(), &s, |_| {}, &AtomicBool::new(false)).unwrap();
        assert!(ve_media::probe(&s.path).unwrap().video.is_some());
        assert!((integrated_lufs(&s.path) + 14.0).abs() < 1.5);
    }

    #[test]
    fn curve_speed_export_keeps_the_timeline_length() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings(dir.path(), Encoder::X264);
        let mut p = project();
        p.settings.main_magnet = false;
        let clips: Vec<_> = p
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().filter(|c| c.asset().is_some()).map(|c| c.id))
            .collect();
        for c in clips {
            ve_core::edit::set_speed_curve(&mut p, c, Some(ve_core::CurvePreset::Montage.curve()))
                .unwrap();
        }
        let want = p.timeline.duration();
        export(&p, &s, |_| {}, &AtomicBool::new(false)).unwrap();
        let r = ve_media::probe(&s.path).unwrap();
        assert!(
            (r.duration - want).0.abs() <= FrameRate::new(30, 1).frame_duration().0 * 2,
            "{:?} / {want:?}",
            r.duration
        );
        assert!(r.audio.is_some());
    }

    /// 字幕位置的关键帧真的被 libass 照着画：匀速往右移，中途那一帧字的重心在预览算出来的位置。
    #[test]
    #[expect(
        clippy::many_single_char_names,
        reason = "测试里的工程 p、帧 f、坐标 x/y"
    )]
    fn subtitle_keyframes_render_where_the_preview_puts_them() {
        use ve_core::edit::{add_keyframe_props, apply_transform_at, set_keyframe_easing};
        let dir = tempfile::tempdir().unwrap();
        let mut p = project();
        p.settings.width = 320;
        p.settings.height = 180;
        let sub = p
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .find(|c| c.subtitle_text().is_some())
            .unwrap()
            .id;
        ve_core::edit::set_subtitle_text(&mut p, sub, "口口".into()).unwrap();
        add_keyframe_props(&mut p, sub, Time::ZERO, &[ve_core::Prop::X]).unwrap();
        let right = ve_core::Transform {
            x: 0.3,
            ..ve_core::Transform::default()
        };
        apply_transform_at(&mut p, sub, Time::from_secs(1), right).unwrap();
        set_keyframe_easing(
            &mut p,
            sub,
            Time::ZERO,
            &[ve_core::Prop::X],
            ve_core::Easing::Linear,
        )
        .unwrap();
        let s = ExportSettings {
            width: 320,
            height: 180,
            ..settings(dir.path(), Encoder::X264)
        };
        export(&p, &s, |_| {}, &AtomicBool::new(false)).unwrap();
        let target = ve_media::ScaleTarget {
            max_width: 320,
            max_height: 180,
        };
        let mut d = ve_media::VideoDecoder::open(&s.path, target).unwrap();
        let f = d.seek_to(Time::from_millis(500)).unwrap();
        // 白字（三个通道都亮；10 像素的细笔画抗锯齿后没有纯白）的横向重心
        let (mut sum, mut n) = (0.0f64, 0.0f64);
        for y in 0..f.height {
            for x in 0..f.width {
                let i = ((y * f.width + x) * 4) as usize;
                if f.data[i..i + 3].iter().all(|v| *v > 140) {
                    sum += f64::from(x);
                    n += 1.0;
                }
            }
        }
        assert!(n > 5.0, "没找到字");
        let clip = Arc::clone(p.timeline.find_clip(sub).unwrap().1);
        let state = ve_core::text_state(&clip, Time::from_millis(500));
        let want = 320.0 * (0.5 + f64::from(state.x));
        let got = sum / n;
        assert!(
            (got - want).abs() < 4.0,
            "字的重心 {got:.1}，预览算出来 {want:.1}"
        );
    }

    #[test]
    fn range_export_keeps_only_the_marked_part() {
        let dir = tempfile::tempdir().unwrap();
        let s = ExportSettings {
            range: Some((Time::from_millis(500), Time::from_millis(1500))),
            ..settings(dir.path(), Encoder::X264)
        };
        export(&project(), &s, |_| {}, &AtomicBool::new(false)).unwrap();
        let r = ve_media::probe(&s.path).unwrap();
        assert!(
            (r.duration - Time::from_secs(1)).0.abs() <= FrameRate::new(30, 1).frame_duration().0,
            "{:?}",
            r.duration
        );
        // 第 0 帧是原来的第 15 帧（灰度 = 帧号）
        let target = ve_media::ScaleTarget {
            max_width: 64,
            max_height: 36,
        };
        let frame = ve_media::VideoDecoder::open(&s.path, target)
            .unwrap()
            .next_frame()
            .unwrap()
            .unwrap();
        let i = ((18 * frame.width + 32) * 4) as usize;
        assert_eq!(
            ve_media::testing::frame_index_from_gray(f64::from(frame.data[i])),
            15,
            "导出的第一帧是区域起点那一帧"
        );
    }

    #[test]
    fn double_speed_export_is_half_as_long() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings(dir.path(), Encoder::X264);
        let mut p = project();
        // 视频 2 秒、音频 2 秒，都 2 倍速：成片 1 秒
        let clips: Vec<_> = p
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().filter(|c| c.asset().is_some()).map(|c| c.id))
            .collect();
        for c in clips {
            ve_core::edit::set_speed(&mut p, c, 2.0).unwrap();
        }
        export(&p, &s, |_| {}, &AtomicBool::new(false)).unwrap();
        let r = ve_media::probe(&s.path).unwrap();
        assert!(
            (r.duration - Time::from_secs(1)).0.abs()
                <= FrameRate::new(30, 1).frame_duration().0 * 2,
            "{:?}",
            r.duration
        );
    }

    #[test]
    fn hevc_export_plays_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = settings(dir.path(), Encoder::X264);
        s.codec = crate::Codec::Hevc;
        export(&project(), &s, |_| {}, &AtomicBool::new(false)).unwrap();
        let r = ve_media::probe(&s.path).unwrap();
        let v = r.video.unwrap();
        assert_eq!((v.width, v.height), (64, 36));
        assert!(r.audio.is_some());
    }

    #[test]
    fn export_cancel_removes_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings(dir.path(), Encoder::X264);
        let cancel = Arc::new(AtomicBool::new(true));
        let err = export(&project(), &s, |_| {}, &cancel).unwrap_err();
        assert!(matches!(err, ExportError::Cancelled), "{err:?}");
        assert!(!s.path.exists());
    }

    #[test]
    fn failure_reports_ffmpeg_message() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = settings(dir.path(), Encoder::X264);
        s.path = dir.path().join("no_such_dir").join("out.mp4");
        let err = export(&project(), &s, |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, ExportError::Ffmpeg { .. }), "{err:?}");
        assert!(err.to_string().contains("ffmpeg"), "{err}");
    }

    #[test]
    fn relative_media_and_output_paths_work() {
        // ffmpeg 在临时目录里运行，相对路径必须先转成绝对路径
        let mut p = project();
        let cwd = std::env::current_dir().unwrap();
        for a in p.assets.values_mut() {
            a.path = pathdiff::diff_paths(&a.path, &cwd).unwrap();
            assert!(a.path.is_relative());
        }
        let dir = tempfile::tempdir_in(&cwd).unwrap();
        let rel_dir = pathdiff::diff_paths(dir.path(), &cwd).unwrap();
        let s = settings(&rel_dir, Encoder::X264);
        export(&p, &s, |_| {}, &AtomicBool::new(false)).unwrap();
        assert!(dir.path().join("out.mp4").exists());
    }

    #[test]
    fn clip_longer_than_video_stream_does_not_shift_later_clips() {
        // 素材时长取的是容器时长（这里音频 2 秒、视频只有 1 秒）：前一段不够长时要补帧，
        // 否则后面的片段整体前移
        let mut proj = Project::new();
        proj.settings.width = 64;
        proj.settings.height = 36;
        let path = fixture(Fixture::ShortVideoLongAudio);
        let probed = ve_media::probe(&path).unwrap();
        assert_eq!(probed.duration, Time::from_secs(2));
        let id = proj.add_asset(Asset {
            id: AssetId(0),
            name: "x".into(),
            path,
            duration: probed.duration,
            video: probed.video,
            audio: probed.audio,
        });
        append_asset(&mut proj, id).unwrap();
        append_asset(&mut proj, id).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut out = settings(dir.path(), Encoder::X264);
        out.video_bitrate_kbps = 4000;
        export(&proj, &out, |_| {}, &AtomicBool::new(false)).unwrap();
        let mut decoder = ve_media::VideoDecoder::open(
            &out.path,
            ve_media::ScaleTarget {
                max_width: 64,
                max_height: 36,
            },
        )
        .unwrap();
        // 2.5 秒处应是第二段的第 15 帧
        let frame = decoder.seek_to(Time::from_millis(2500)).unwrap();
        let gray = f64::from(frame.data[(18 * 64 + 32) * 4]);
        let want = ve_media::testing::expected_gray(15);
        assert!(
            (gray - want).abs() <= 8.0,
            "2.5 秒处灰度 {gray}，期望 {want}（第二段第 15 帧）"
        );
    }

    #[test]
    fn output_same_as_input_is_rejected_and_input_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = project();
        let media = dir.path().join("素材.mp4");
        std::fs::copy(fixture(Fixture::FrameIndexed30), &media).unwrap();
        for a in p.assets.values_mut().filter(|a| a.video.is_some()) {
            a.path = media.clone();
        }
        let mut s = settings(dir.path(), Encoder::X264);
        s.path = media.clone();
        let err = export(&p, &s, |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, ExportError::OutputIsInput), "{err:?}");
        assert!(media.exists(), "原始素材被删了");
    }

    #[test]
    fn failed_or_cancelled_export_keeps_existing_output() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings(dir.path(), Encoder::X264);
        std::fs::write(&s.path, b"old export").unwrap();
        let mut broken = project();
        for a in broken.assets.values_mut() {
            a.path = dir.path().join("不存在.mp4");
        }
        assert!(export(&broken, &s, |_| {}, &AtomicBool::new(false)).is_err());
        assert_eq!(std::fs::read(&s.path).unwrap(), b"old export");
        let cancel = AtomicBool::new(true);
        assert!(export(&project(), &s, |_| {}, &cancel).is_err());
        assert_eq!(std::fs::read(&s.path).unwrap(), b"old export");
        // 成功时才替换
        export(&project(), &s, |_| {}, &AtomicBool::new(false)).unwrap();
        assert!(ve_media::probe(&s.path).is_ok());
        let leftovers = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 1, "临时文件没清掉");
    }

    #[test]
    fn missing_media_is_reported_by_name_before_running_ffmpeg() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = project();
        for a in p
            .assets
            .values_mut()
            .filter(|a| a.audio.is_some() && a.video.is_none())
        {
            a.path = dir.path().join("配乐.wav");
            a.name = "配乐.wav".into();
        }
        let s = settings(dir.path(), Encoder::Nvenc);
        let err = export(&p, &s, |_| {}, &AtomicBool::new(false)).unwrap_err();
        match &err {
            ExportError::MissingMedia(names) => assert_eq!(names, &vec!["配乐.wav".to_owned()]),
            other => panic!("应报素材缺失，实际 {other:?}"),
        }
        assert!(err.to_string().contains("配乐.wav"));
    }

    #[test]
    fn ts_with_start_offset_exports_same_frames_as_preview() {
        // 预览（解码器减掉起始时间戳）和导出（ffmpeg -ss）对同一时刻要取到同一帧
        let mut proj = Project::new();
        proj.settings.width = 64;
        proj.settings.height = 36;
        let path = fixture(Fixture::TsWithOffset);
        let probed = ve_media::probe(&path).unwrap();
        let id = proj.add_asset(Asset {
            id: AssetId(0),
            name: "ts".into(),
            path: path.clone(),
            duration: probed.duration,
            video: probed.video,
            audio: probed.audio,
        });
        let clip = append_asset(&mut proj, id).unwrap();
        ve_core::edit::trim_start(&mut proj, clip, Time::from_millis(500)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut out = settings(dir.path(), Encoder::X264);
        out.video_bitrate_kbps = 4000;
        export(&proj, &out, |_| {}, &AtomicBool::new(false)).unwrap();
        let target = ve_media::ScaleTarget {
            max_width: 64,
            max_height: 36,
        };
        let center = |f: &ve_media::VideoFrame| f64::from(f.data[(18 * 64 + 32) * 4]);
        // 时间轴 1.0 秒 = 素材内 1.5 秒（片段从素材 0.5 秒开始，又被拖到时间轴 0.5 秒处）
        let start = proj.timeline.find_clip(clip).unwrap().1.start;
        let preview = ve_media::VideoDecoder::open(&path, target)
            .unwrap()
            .seek_to(Time::from_millis(1500))
            .unwrap();
        let exported = ve_media::VideoDecoder::open(&out.path, target)
            .unwrap()
            .seek_to(start + Time::from_millis(1000))
            .unwrap();
        assert!(
            (center(&preview) - center(&exported)).abs() <= 8.0,
            "预览 {} vs 导出 {}",
            center(&preview),
            center(&exported)
        );
    }

    #[test]
    fn grab_frame_saves_the_exact_frame_as_png() {
        use ve_media::testing::frame_index_from_gray;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("freeze.png");
        grab_frame(&fixture(Fixture::FrameIndexed30), Time::from_secs(1), &out).unwrap();
        let probed = ve_media::probe(&out).unwrap();
        let video = probed.video.unwrap();
        assert!(
            video.still && (video.width, video.height) == (64, 36),
            "{video:?}"
        );
        let target = ve_media::ScaleTarget {
            max_width: 64,
            max_height: 36,
        };
        let frame = ve_media::VideoDecoder::open(&out, target)
            .unwrap()
            .next_frame()
            .unwrap()
            .unwrap();
        let i = ((18 * frame.width + 32) * 4) as usize;
        assert_eq!(
            frame_index_from_gray(f64::from(frame.data[i])),
            30,
            "第 1 秒是第 30 帧"
        );
        // 素材不存在：带上 ffmpeg 的报错
        let err = grab_frame(&dir.path().join("nope.mp4"), Time::ZERO, &out).unwrap_err();
        assert_ne!(err, "");
    }

    #[test]
    fn reverse_segment_plays_the_frames_backwards() {
        use ve_media::testing::frame_index_from_gray;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("rev.mp4");
        // 第 10 帧到第 40 帧（1 秒）倒过来：第一帧是原来的第 39 帧
        reverse_segment(
            &fixture(Fixture::FrameIndexed30),
            FrameRate::new(30, 1).time_of_frame(10),
            Time::from_secs(1),
            true,
            false,
            &out,
        )
        .unwrap();
        let r = ve_media::probe(&out).unwrap();
        assert!(
            (r.duration - Time::from_secs(1)).0.abs() <= FrameRate::new(30, 1).frame_duration().0
        );
        let target = ve_media::ScaleTarget {
            max_width: 64,
            max_height: 36,
        };
        let mut d = ve_media::VideoDecoder::open(&out, target).unwrap();
        let i = ((18 * 64 + 32) * 4) as usize;
        let first = d.next_frame().unwrap().unwrap();
        let first = frame_index_from_gray(f64::from(first.data[i]));
        assert!((38..=40).contains(&first), "第一帧是原来的第 {first} 帧");
        let last = d.seek_to(Time::from_millis(960)).unwrap();
        let last = frame_index_from_gray(f64::from(last.data[i]));
        assert!((9..=12).contains(&last), "最后一帧是原来的第 {last} 帧");
    }

    #[test]
    fn proxies_shrink_big_video_keep_small_video_and_carry_the_sound() {
        let dir = tempfile::tempdir().unwrap();
        // 1280×720 → 短边 540
        let big = dir.path().join("big.mp4");
        make_proxy(&fixture(Fixture::Bt709Red), &big).unwrap();
        let r = ve_media::probe(&big).unwrap();
        let v = r.video.unwrap();
        assert_eq!((v.width, v.height), (960, 540));
        // 64×36 带声音：不放大，声音还在，时长不变
        let small = dir.path().join("small.mp4");
        make_proxy(&fixture(Fixture::VideoWithAudio), &small).unwrap();
        let kept = ve_media::probe(&small).unwrap();
        let size = kept.video.map(|x| (x.width, x.height));
        assert_eq!(size, Some((64, 36)));
        assert!(kept.audio.is_some());
        assert!((kept.duration - Time::from_secs(2)).0.abs() <= Time::from_millis(60).0);
        // 坏文件：报错
        let bad = dir.path().join("坏.mp4");
        std::fs::write(&bad, "不是视频").unwrap();
        assert!(make_proxy(&bad, &dir.path().join("x.mp4")).is_err());
    }

    #[test]
    fn a_silent_ffmpeg_failure_still_says_something() {
        let status = Command::new(ffmpeg_program())
            .args([
                "-hide_banner",
                "-loglevel",
                "quiet",
                "-i",
                "没有这个文件.mp4",
            ])
            .status()
            .unwrap();
        assert!(!status.success());
        let message = failure_message(
            status, b"  
",
        );
        assert!(message.contains("退出码"), "{message}");
        assert_eq!(
            failure_message(
                status,
                b" No such file 
"
            ),
            "No such file"
        );
    }

    #[test]
    fn stabilizing_writes_the_segment_with_its_sound() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("子目录").join("稳.mp4");
        stabilize_segment(
            &fixture(Fixture::VideoWithAudio),
            Time::from_millis(500),
            Time::from_secs(1),
            12,
            &out,
        )
        .unwrap();
        let r = ve_media::probe(&out).unwrap();
        assert!(r.video.is_some() && r.audio.is_some());
        assert!(
            (r.duration - Time::from_secs(1)).0.abs() <= Time::from_millis(70).0,
            "{:?}",
            r.duration
        );
    }

    #[test]
    fn missing_ffmpeg_reports_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings(dir.path(), Encoder::X264);
        let program = std::ffi::OsStr::new("ve-no-such-ffmpeg-binary");
        let err =
            export_with(program, &project(), &s, |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, ExportError::FfmpegNotFound), "{err:?}");
    }

    #[test]
    fn nvenc_export_succeeds_or_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings(dir.path(), Encoder::Nvenc);
        export(&project(), &s, |_| {}, &AtomicBool::new(false)).unwrap();
        assert!(ve_media::probe(&s.path).unwrap().video.is_some());
    }

    #[test]
    fn default_settings_follow_project() {
        let p = project();
        let s = default_settings(&p, "a.mp4".into());
        assert_eq!(
            (s.width, s.height, s.frame_rate, s.encoder),
            (64, 36, p.settings.frame_rate, Encoder::Nvenc)
        );
    }

    #[test]
    fn progress_never_goes_backwards_across_retries() {
        // NVENC 中途失败改用 x264 重来时，进度条不能从 60% 跳回 0
        let mut seen = Vec::new();
        {
            let mut m = Monotonic::new(|p| seen.push(p));
            for p in [0.2, 0.6, 0.0, 0.3, 0.6, 0.8, 1.0] {
                m.report(p);
            }
        }
        assert_eq!(seen, [0.2, 0.6, 0.8, 1.0]);
    }
}
