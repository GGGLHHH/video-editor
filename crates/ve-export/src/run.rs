//! 执行导出：写临时文件、启动 ffmpeg、解析进度、取消、NVENC 失败退回 x264。

use crate::{
    Encoder, ExportError, ExportSettings, SubtitleStyle, build_job, has_subtitles, write_ass,
};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use ve_core::{Project, Time};

/// ffmpeg 可执行文件：环境变量 `VE_FFMPEG` 优先，否则 PATH 上的 `ffmpeg`。
fn ffmpeg_program() -> std::ffi::OsString {
    std::env::var_os("VE_FFMPEG").unwrap_or_else(|| "ffmpeg".into())
}

/// 导出前检查并准备：路径转绝对（ffmpeg 在临时目录里运行），素材都在，输出不是素材本身。
/// 返回（绝对路径版工程，最终输出路径，临时输出路径）。
fn prepare(
    project: &Project,
    settings: &ExportSettings,
) -> Result<(Project, std::path::PathBuf, std::path::PathBuf), ExportError> {
    let mut project = project.clone();
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

/// 解析 `-progress` 输出里的 `out_time_us=` 行，返回 0–1 的进度。
#[must_use]
pub fn parse_progress(line: &str, total: Time) -> Option<f32> {
    let us: i64 = line.strip_prefix("out_time_us=")?.trim().parse().ok()?;
    let total_us = (total.as_millis() * 1000).max(1);
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "进度只需大致精度"
    )]
    let p = (us as f64 / total_us as f64).clamp(0.0, 1.0) as f32;
    Some(p)
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
    match run_once(program, project, settings, &mut on_progress, cancel) {
        Err(ExportError::Ffmpeg { message, .. }) if settings.encoder == Encoder::Nvenc => {
            log::warn!("NVENC 导出失败，改用 x264 重试：{message}");
            let fallback = ExportSettings {
                encoder: Encoder::X264,
                ..settings.clone()
            };
            run_once(program, project, &fallback, &mut on_progress, cancel)
        }
        other => other,
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
        std::fs::write(
            dir.path().join("subs.ass"),
            write_ass(project, &SubtitleStyle::default()),
        )?;
    }
    std::fs::write(dir.path().join("filter.txt"), &job.filter_graph)?;
    if cancel.load(Ordering::Acquire) {
        return Err(ExportError::Cancelled);
    }
    let mut child = Command::new(program)
        .args(&job.args)
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
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
    let mut last = 0.0f32;
    let mut cancelled = false;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if cancel.load(Ordering::Acquire) {
                cancelled = true;
                drop(child.kill());
                break;
            }
            if let Some(p) = parse_progress(&line, job.duration)
                && p > last
            {
                last = p;
                on_progress(p);
            }
        }
    }
    let status = child.wait()?;
    let message = stderr.and_then(|h| h.join().ok()).unwrap_or_default();
    if cancelled || cancel.load(Ordering::Acquire) {
        drop(std::fs::remove_file(&part));
        return Err(ExportError::Cancelled);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use ve_core::edit::{add_subtitle, append_asset};
    use ve_core::{Asset, AssetId, FrameRate, Project};
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
    fn parses_progress_lines() {
        let total = Time::from_secs(10);
        assert_eq!(parse_progress("out_time_us=5000000", total), Some(0.5));
        assert_eq!(parse_progress("out_time_us=N/A", total), None);
        assert_eq!(parse_progress("out_time_us=-5", total), Some(0.0));
        assert_eq!(parse_progress("out_time_us=99000000", total), Some(1.0));
        assert_eq!(parse_progress("frame=3", total), None);
    }
}
