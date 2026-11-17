//! 测试素材：用 ffmpeg 命令行按需生成，缓存在 `target/ve-fixtures/`。
//!
//! 只在测试或 `test-fixtures` 特性下编译。

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

/// 测试素材种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixture {
    /// 64x36、30fps、2 秒、无音轨、关键帧间隔 15、x264 无损；
    /// 第 N 帧亮度 Y = 16 + 3N（见 [`frame_index_from_gray`]）。
    FrameIndexed30,
    /// 2 秒 440Hz 正弦波，48kHz 立体声 wav，幅度 0.5。
    ToneStereo48k,
    /// 64x36、30fps、2 秒画面 + 440Hz 立体声 aac（幅度 0.5）。
    VideoWithAudio,
    /// 64x36 纯红 png。
    Image,
    /// 2 秒 30fps 的裸 H.264 码流（容器里没有时长）。
    RawH264,
    /// 视频只有 1 秒（帧号亮度同 [`Fixture::FrameIndexed30`]），音频 2 秒：容器时长 2 秒。
    ShortVideoLongAudio,
    /// MPEG-TS：帧号亮度视频（同 [`Fixture::FrameIndexed30`]）+ 440Hz 音频；时间戳不从 0 开始。
    TsWithOffset,
    /// 2 秒 mp3，带封面图（封面是一条 `attached_pic` 视频流）。
    Mp3WithCover,
}

impl Fixture {
    const fn file_name(self) -> &'static str {
        match self {
            Self::FrameIndexed30 => "frame_indexed_30.mp4",
            Self::ToneStereo48k => "tone_stereo_48k.wav",
            Self::VideoWithAudio => "video_with_audio.mp4",
            Self::Image => "red.png",
            Self::RawH264 => "raw.h264",
            Self::Mp3WithCover => "with_cover.mp3",
            Self::TsWithOffset => "offset.ts",
            Self::ShortVideoLongAudio => "short_video_long_audio.mp4",
        }
    }

    /// ffmpeg 参数（不含输出文件）。参数里没有空格，按空格切分。
    fn ffmpeg_args(self) -> Vec<&'static str> {
        const SINE: &str =
            "sine=frequency=440:sample_rate=48000:duration=2,volume=4,pan=stereo|c0=c0|c1=c0";
        let line = match self {
            Self::FrameIndexed30 => {
                "-f lavfi -i nullsrc=s=64x36:r=30:d=2,geq=lum='16+3*N':cb=128:cr=128 -c:v libx264 -qp 0 -g 15 -pix_fmt yuv420p"
            }
            // lavfi 的 sine 固定幅度 1/8，乘 4 得 0.5；用 pan 原样复制成双声道（-ac 2 会按 -3dB 混音）
            Self::ToneStereo48k => "-f lavfi -i SINE -c:a pcm_f32le",
            Self::VideoWithAudio => {
                "-f lavfi -i testsrc2=s=64x36:r=30:d=2 -f lavfi -i SINE -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest"
            }
            Self::Image => "-f lavfi -i color=c=red:s=64x36 -frames:v 1",
            Self::RawH264 => {
                "-f lavfi -i testsrc2=s=64x36:r=30:d=2 -c:v libx264 -pix_fmt yuv420p -f h264"
            }
            Self::ShortVideoLongAudio => {
                "-f lavfi -i nullsrc=s=64x36:r=30:d=1,geq=lum='16+3*N':cb=128:cr=128 -f lavfi -i SINE -c:v libx264 -qp 0 -g 15 -pix_fmt yuv420p -c:a aac"
            }
            Self::TsWithOffset => {
                "-f lavfi -i nullsrc=s=64x36:r=30:d=2,geq=lum='16+3*N':cb=128:cr=128 -f lavfi -i SINE -c:v libx264 -qp 0 -g 15 -pix_fmt yuv420p -c:a aac -f mpegts"
            }
            // 只出一帧作封面（不用 -frames:v，那会把整个文件截断）
            Self::Mp3WithCover => {
                "-f lavfi -i sine=frequency=440:sample_rate=48000:duration=2 -f lavfi -i color=c=red:s=64x36:d=0.04:r=25 -map 0:a -map 1:v -c:a libmp3lame -c:v mjpeg -disposition:v attached_pic -id3v2_version 3"
            }
        };
        line.split_whitespace()
            .map(|a| if a == "SINE" { SINE } else { a })
            .collect()
    }
}

static LOCK: Mutex<()> = Mutex::new(());

/// 测试素材目录。
#[must_use]
pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ve-fixtures")
}

/// 取素材路径，不存在则生成。
///
/// # Panics
/// ffmpeg 不在 PATH 上或生成失败（只在测试里用）。
#[must_use]
#[expect(clippy::panic, reason = "测试辅助：素材生成失败就该让测试失败")]
pub fn fixture(kind: Fixture) -> PathBuf {
    let _guard = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = fixture_dir();
    let path = dir.join(kind.file_name());
    if path.exists() {
        return path;
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        panic!("创建 {} 失败：{e}", dir.display());
    }
    let tmp = dir.join(format!("tmp-{}-{}", std::process::id(), kind.file_name()));
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(kind.ffmpeg_args())
        .arg(&tmp)
        .status();
    match status {
        Ok(s) if s.success() => {}
        other => panic!("生成测试素材 {kind:?} 失败：{other:?}"),
    }
    if let Err(e) = std::fs::rename(&tmp, &path)
        && !path.exists()
    {
        panic!("改名 {} 失败：{e}", tmp.display());
    }
    path
}

/// [`Fixture::FrameIndexed30`] 第 `frame` 帧解码成 BGRA 后的灰度期望值（有限范围 → 全范围）。
#[must_use]
pub fn expected_gray(frame: i64) -> f64 {
    #[expect(clippy::cast_precision_loss, reason = "帧号很小")]
    let y = 3.0f64.mul_add(frame as f64, 16.0);
    ((y - 16.0) * 255.0 / 219.0).clamp(0.0, 255.0)
}

/// 由解码出的灰度反推 [`Fixture::FrameIndexed30`] 的帧号。
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "结果是很小的帧号")]
pub fn frame_index_from_gray(gray: f64) -> i64 {
    (gray * 219.0 / 255.0 / 3.0).round() as i64
}
