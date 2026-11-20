//! 进程内的 ffmpeg 音频滤镜图：交错立体声 f32 进、同样格式出。预览用它跑片段的声音处理
//! （降噪、均衡、压缩），和导出时写进 ffmpeg 滤镜图的是同一串滤镜。

use crate::MediaError;
use crate::audio::OUTPUT_CHANNELS;
use ffmpeg_next as ff;

/// 一条音频滤镜链。滤镜有状态（降噪、压缩跟着前面的声音走），跳着放时要重建。
pub struct AudioFilter {
    graph: ff::filter::Graph,
    rate: u32,
    /// 下一帧的时间戳（样本数）。
    pts: i64,
    /// 开头还要丢掉的样本帧（滤镜的固定延迟）。
    skip: usize,
}

impl std::fmt::Debug for AudioFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioFilter")
            .field("rate", &self.rate)
            .field("pts", &self.pts)
            .finish_non_exhaustive()
    }
}

fn filter_error(what: &str) -> MediaError {
    MediaError::Init(format!("找不到 ffmpeg 滤镜 {what}"))
}

impl AudioFilter {
    /// 用滤镜串 `spec`（ffmpeg 写法，如 `bass=g=3,acompressor`）建一条链，采样率 `rate`；
    /// 输出开头丢掉 `latency` 个样本帧，抵消滤镜的固定延迟。
    ///
    /// # Errors
    /// 滤镜串写错了、ffmpeg 里没有这个滤镜。
    pub fn new(spec: &str, rate: u32, latency: usize) -> Result<Self, MediaError> {
        let mut graph = ff::filter::Graph::new();
        let args =
            format!("time_base=1/{rate}:sample_rate={rate}:sample_fmt=flt:channel_layout=stereo");
        graph.add(
            &ff::filter::find("abuffer").ok_or_else(|| filter_error("abuffer"))?,
            "in",
            &args,
        )?;
        graph.add(
            &ff::filter::find("abuffersink").ok_or_else(|| filter_error("abuffersink"))?,
            "out",
            "",
        )?;
        graph.output("in", 0)?.input("out", 0)?.parse(&format!(
            "{spec},aformat=sample_fmts=flt:sample_rates={rate}:channel_layouts=stereo"
        ))?;
        graph.validate()?;
        Ok(Self {
            graph,
            rate,
            pts: 0,
            skip: latency,
        })
    }

    /// 送进一段交错立体声样本，把已经出来的结果接到 `out` 后面（滤镜可能还压着一点没吐出来）。
    ///
    /// # Errors
    /// 滤镜处理失败。
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), MediaError> {
        let frames = input.len() / OUTPUT_CHANNELS;
        if frames > 0 {
            let mut frame = ff::frame::Audio::new(
                ff::format::Sample::F32(ff::format::sample::Type::Packed),
                frames,
                ff::ChannelLayout::default(2),
            );
            frame.set_rate(self.rate);
            frame.set_pts(Some(self.pts));
            self.pts += i64::try_from(frames).unwrap_or(0);
            let bytes = frame.data_mut(0);
            for (dst, v) in bytes
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(&input[..frames * OUTPUT_CHANNELS])
            {
                *dst = v.to_ne_bytes();
            }
            if let Some(mut ctx) = self.graph.get("in") {
                ctx.source().add(&frame)?;
            }
        }
        self.pull(out)
    }

    /// 输入结束：把滤镜里压着的都取出来。
    ///
    /// # Errors
    /// 滤镜处理失败。
    pub fn finish(&mut self, out: &mut Vec<f32>) -> Result<(), MediaError> {
        if let Some(mut ctx) = self.graph.get("in") {
            ctx.source().flush()?;
        }
        self.pull(out)
    }

    fn pull(&mut self, out: &mut Vec<f32>) -> Result<(), MediaError> {
        let mut frame = ff::frame::Audio::empty();
        loop {
            let got = self
                .graph
                .get("out")
                .map(|mut c| c.sink().frame(&mut frame));
            match got {
                Some(Ok(())) => {}
                Some(Err(ff::Error::Eof | ff::Error::Other { .. })) | None => return Ok(()),
                Some(Err(e)) => return Err(e.into()),
            }
            let values = frame.samples() * OUTPUT_CHANNELS;
            let data = frame.data(0);
            let data = data.get(..values * 4).unwrap_or(data);
            let skip = (self.skip * OUTPUT_CHANNELS).min(values);
            self.skip -= skip / OUTPUT_CHANNELS;
            out.extend(
                data.as_chunks::<4>()
                    .0
                    .iter()
                    .skip(skip)
                    .map(|b| f32::from_ne_bytes(*b)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize, rate: u32, hz: f32, amp: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                #[expect(clippy::cast_precision_loss, reason = "测试信号")]
                let v = amp * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin();
                [v, v]
            })
            .collect()
    }

    fn rms(v: &[f32]) -> f32 {
        #[expect(clippy::cast_precision_loss, reason = "测试")]
        let n = v.len().max(1) as f32;
        (v.iter().map(|x| x * x).sum::<f32>() / n).sqrt()
    }

    #[test]
    fn bass_boost_raises_a_low_tone_and_keeps_the_length() {
        crate::probe::init().unwrap();
        let rate = 48_000;
        let input = tone(48_000, rate, 60.0, 0.1);
        let mut f = AudioFilter::new("bass=g=12:f=100", rate, 0).unwrap();
        let mut out = Vec::new();
        for chunk in input.chunks(2048) {
            f.process(chunk, &mut out).unwrap();
        }
        f.finish(&mut out).unwrap();
        assert_eq!(out.len(), input.len());
        let gain = rms(&out[20_000..]) / rms(&input[20_000..]);
        assert!(gain > 2.5, "低音 +12 dB：{gain}");
    }

    #[test]
    fn latency_is_dropped_so_the_sound_stays_in_place() {
        crate::probe::init().unwrap();
        let rate = 48_000;
        // 0.5 秒处一个脉冲
        let mut input = vec![0.0f32; 48_000 * 2];
        input[24_000 * 2] = 0.8;
        input[24_000 * 2 + 1] = 0.8;
        let mut f = AudioFilter::new("afftdn=nr=10:nf=-50", rate, 1200).unwrap();
        let mut out = Vec::new();
        for chunk in input.chunks(1024) {
            f.process(chunk, &mut out).unwrap();
        }
        f.finish(&mut out).unwrap();
        let peak = out
            .iter()
            .step_by(2)
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(i, _)| i)
            .unwrap();
        assert!(peak.abs_diff(24_000) <= 2, "脉冲在 {peak}");
    }

    #[test]
    fn a_bad_spec_is_an_error() {
        crate::probe::init().unwrap();
        assert!(AudioFilter::new("没有这个滤镜", 48_000, 0).is_err());
    }
}
