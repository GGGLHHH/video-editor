//! 缩略图：在素材里均匀取若干帧。

use crate::{MediaError, ScaleTarget, VideoDecoder, VideoFrame};
use std::path::Path;
use ve_core::Time;

/// 均匀取 `count` 张缩略图（每段的中点），高度 `height`、宽度按比例。
///
/// # Errors
/// 打不开文件、没有视频流、解码失败。
pub fn thumbnails(path: &Path, count: usize, height: u32) -> Result<Vec<VideoFrame>, MediaError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let duration = crate::probe(path)?.duration;
    let mut decoder = VideoDecoder::open(
        path,
        ScaleTarget {
            max_width: height.saturating_mul(8),
            max_height: height,
        },
    )?;
    let n = i64::try_from(count).unwrap_or(i64::MAX);
    (0..n)
        .map(|i| {
            let t = i128::from(duration.0) * i128::from(2 * i + 1) / i128::from(2 * n);
            decoder.seek_to(Time(i64::try_from(t).unwrap_or(0)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, expected_gray, fixture, frame_index_from_gray};

    #[test]
    fn thumbnail_count_and_size() {
        let thumbs = thumbnails(&fixture(Fixture::FrameIndexed30), 4, 18).unwrap();
        assert_eq!(thumbs.len(), 4);
        for t in &thumbs {
            assert_eq!((t.width, t.height), (32, 18));
        }
        // 均匀分布：取的是每段中点，2 秒 4 张 → 0.25、0.75、1.25、1.75 秒 → 第 7、22、37、52 帧
        let idx: Vec<i64> = thumbs
            .iter()
            .map(|t| frame_index_from_gray(f64::from(t.data[(9 * 32 + 16) * 4])))
            .collect();
        assert_eq!(idx, [7, 22, 37, 52]);
        assert!(expected_gray(52) > 0.0);
    }

    #[test]
    fn still_image_thumbnails() {
        let thumbs = thumbnails(&fixture(Fixture::Image), 3, 18).unwrap();
        assert_eq!(thumbs.len(), 3);
        assert_eq!((thumbs[0].width, thumbs[0].height), (32, 18));
    }

    #[test]
    fn zero_count_is_empty() {
        assert_eq!(
            thumbnails(&fixture(Fixture::FrameIndexed30), 0, 18).unwrap(),
            []
        );
    }
}
