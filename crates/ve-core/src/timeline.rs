//! 时间轴：轨道、片段，以及模型不变式检查。

use crate::{Asset, AssetId, ClipId, IdGen, Time, TrackId, Transform};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use thiserror::Error;

/// 轨道类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackKind {
    /// 视频（带素材原声）。
    Video,
    /// 音频。
    Audio,
    /// 字幕。
    Subtitle,
}

/// 片段内容。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClipContent {
    /// 引用素材的一段，从 `source_in` 开始。
    Media {
        /// 素材。
        asset: AssetId,
        /// 素材内起点。
        source_in: Time,
    },
    /// 字幕文字。
    Subtitle {
        /// 文字，可含换行。
        text: String,
        /// 入场 / 出场动画（第 6 版工程文件加的；没有时不写）。
        #[serde(default, skip_serializing_if = "crate::TextMotion::is_none")]
        motion: crate::TextMotion,
        /// 这一条自己的样式（剪映「应用到全部字幕」关着时改的）；`None` 用工程的字幕样式。
        /// 第 7 版工程文件加的。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<crate::SubtitleStyle>,
    },
    /// 调节（剪映素材面板「调节」的「自定义调节」）：放在视频轨上，用片段的调色参数给它下面所有轨道的
    /// 画面调色（调节层）。没有素材，长度随意。第 6 版工程文件加的。
    Adjustment,
}

/// 时间轴上的一个片段。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// id。
    pub id: ClipId,
    /// 时间轴上的起点。
    pub start: Time,
    /// 长度（> 0）。
    pub duration: Time,
    /// 内容。
    pub content: ClipContent,
    /// 音量增益（dB）。
    pub gain_db: f32,
    /// 淡入长度。
    pub fade_in: Time,
    /// 淡出长度。
    pub fade_out: Time,
    /// 画面属性（只对视频轨上的媒体片段起作用）。第 1 版工程文件里没有这个字段，取默认值。
    #[serde(default)]
    pub transform: Transform,
    /// 画面属性的关键帧动画（时间相对片段起点）；为空时不写进文件。第 4 版工程文件加的。
    #[serde(default, skip_serializing_if = "crate::Animation::is_empty")]
    pub animation: crate::Animation,
    /// 调色（只对视频轨上的媒体片段起作用）；没调时不写进文件。第 5 版工程文件加的。
    #[serde(default, skip_serializing_if = "crate::ColorAdjust::is_identity")]
    pub adjust: crate::ColorAdjust,
    /// 停用（剪映的 V）：片段留在时间轴上，但不播放、不导出。第 5 版工程文件加的。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// 进入这一段时的转场（和同一条轨上紧挨着的前一段之间）。第 5 版工程文件加的。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_in: Option<crate::Transition>,
    /// 变速倍数（剪映「变速」）：素材用掉的长度 = 时间轴上的长度 × 倍数。第 5 版工程文件加的。
    #[serde(default = "normal_speed", skip_serializing_if = "is_normal_speed")]
    pub speed: f32,
    /// 变速时保持音调（剪映「声音变调」关着）；`false` 时音调跟着快慢变。第 5 版工程文件加的。
    #[serde(default = "keep", skip_serializing_if = "is_kept")]
    pub keep_pitch: bool,
    /// 颜色标签（剪映右键菜单的 8 个色点，`0..LABEL_COLORS`）；没有时不写进文件。第 7 版工程文件加的。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<u8>,
    /// 外观：混合模式、圆角、描边、阴影、蒙版、色度抠图（只对视频轨上的媒体片段起作用）。第 7 版工程文件加的。
    #[serde(default, skip_serializing_if = "crate::Look::is_default")]
    pub look: crate::Look,
    /// 倒放（剪映「倒放」）：现在用的素材是倒着生成的那一段；这里记着原来的素材和素材内起点，
    /// 再点一次倒放就换回去。第 7 版工程文件加的。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversed: Option<(AssetId, Time)>,
    /// 曲线变速（剪映「曲线变速」）：片段里各处快慢不同；平均倍数仍是 [`Self::speed`]。
    /// 第 7 版工程文件加的。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<crate::SpeedCurve>,
    /// 进阶调色（HSL、曲线、色轮、LUT）；没调时不写进文件。第 7 版工程文件加的。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade: Option<Arc<crate::Grade>>,
}

/// 颜色标签的个数。
pub const LABEL_COLORS: u8 = 8;

const fn keep() -> bool {
    true
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde 的 skip_serializing_if 要引用"
)]
const fn is_kept(b: &bool) -> bool {
    *b
}

/// 变速倍数的范围。
pub const SPEED_RANGE: (f32, f32) = (0.1, 10.0);

const fn normal_speed() -> f32 {
    1.0
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde 的 skip_serializing_if 要引用"
)]
#[expect(clippy::float_cmp, reason = "和默认值 1.0 精确比较")]
fn is_normal_speed(s: &f32) -> bool {
    *s == 1.0
}

impl Clip {
    /// 片段内 `offset` 时刻（相对片段起点）的画面属性：有关键帧的属性按动画求值，其余用固定值。
    #[must_use]
    pub fn transform_at(&self, offset: Time) -> Transform {
        self.animation.apply(&self.transform, offset)
    }

    /// 片段内 `offset` 时刻的调色：有关键帧的项按动画求值，其余用固定值。
    #[must_use]
    pub fn adjust_at(&self, offset: Time) -> crate::ColorAdjust {
        self.animation.apply_adjust(&self.adjust, offset)
    }

    /// 片段内 `offset` 时刻的音量（dB）：有音量关键帧时按动画求值。
    #[must_use]
    pub fn gain_db_at(&self, offset: Time) -> f32 {
        self.animation
            .value_at(crate::Prop::Volume, offset)
            .unwrap_or(self.gain_db)
    }

    /// 片段内 `offset` 时刻某个可打关键帧属性的值。
    #[must_use]
    pub fn value_at(&self, prop: crate::Prop, offset: Time) -> f32 {
        if prop.is_transform() {
            prop.get(&self.transform_at(offset))
        } else if let Some(item) = prop.adjust_item() {
            item.get(&self.adjust_at(offset))
        } else {
            self.gain_db_at(offset)
        }
    }

    /// 改某个属性的固定值（没有关键帧时用的值）。
    pub const fn set_base(&mut self, prop: crate::Prop, value: f32) {
        if prop.is_transform() {
            prop.set(&mut self.transform, value);
        } else if let Some(item) = prop.adjust_item() {
            item.set(&mut self.adjust, value);
        } else {
            self.gain_db = value;
        }
    }

    /// 片段内 `offset`（时间轴时间）对应的素材时间（算上变速）。
    #[must_use]
    pub fn source_at(&self, offset: Time) -> Time {
        let Some(curve) = &self.curve else {
            return self.source_in() + offset.scaled(f64::from(self.speed));
        };
        // 曲线变速：素材进度按曲线积分；片段外面（转场越过边界时）按两头的倍数接着走
        let mean = f64::from(curve.mean().max(1e-3));
        let edge = |at: f32| f64::from(self.speed) * f64::from(curve.speed_at(at)) / mean;
        if offset < Time::ZERO {
            return self.source_in() + offset.scaled(edge(0.0));
        }
        if offset > self.duration {
            return self.source_in()
                + self.source_span()
                + (offset - self.duration).scaled(edge(1.0));
        }
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "进度只需 f32 精度"
        )]
        let u = (offset.0 as f64 / self.duration.0.max(1) as f64) as f32;
        self.source_in() + self.source_span().scaled(curve.progress(u))
    }

    /// 片段内 `offset` 时刻素材走得多快（素材时间 / 时间轴时间）：没有曲线时就是 [`Self::speed`]。
    #[must_use]
    pub fn speed_at(&self, offset: Time) -> f32 {
        self.curve.as_ref().map_or(self.speed, |c| {
            #[expect(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                reason = "进度只需 f32 精度"
            )]
            let u = (offset.0 as f64 / self.duration.0.max(1) as f64) as f32;
            self.speed * c.speed_at(u) / c.mean().max(1e-3)
        })
    }

    /// 片段用掉的素材长度（算上变速）。
    #[must_use]
    pub fn source_span(&self) -> Time {
        self.duration.scaled(f64::from(self.speed))
    }

    /// 会不会播放、导出（没停用）。
    #[must_use]
    pub const fn enabled(&self) -> bool {
        !self.disabled
    }

    /// 时间轴上的终点（不含）。
    #[must_use]
    pub fn end(&self) -> Time {
        self.start + self.duration
    }

    /// 引用的素材（字幕返回 `None`）。
    #[must_use]
    pub const fn asset(&self) -> Option<AssetId> {
        match &self.content {
            ClipContent::Media { asset, .. } => Some(*asset),
            ClipContent::Subtitle { .. } | ClipContent::Adjustment => None,
        }
    }

    /// 素材内起点（字幕为 0）。
    #[must_use]
    pub const fn source_in(&self) -> Time {
        match &self.content {
            ClipContent::Media { source_in, .. } => *source_in,
            ClipContent::Subtitle { .. } | ClipContent::Adjustment => Time::ZERO,
        }
    }

    /// 字幕文字。
    #[must_use]
    pub fn subtitle_text(&self) -> Option<&str> {
        match &self.content {
            ClipContent::Subtitle { text, .. } => Some(text),
            ClipContent::Media { .. } | ClipContent::Adjustment => None,
        }
    }

    /// 字幕的入场 / 出场动画（不是字幕时没有）。
    #[must_use]
    pub const fn text_motion(&self) -> Option<crate::TextMotion> {
        match &self.content {
            ClipContent::Subtitle { motion, .. } => Some(*motion),
            _ => None,
        }
    }

    /// 字幕自己的样式（没有时用工程的）。
    #[must_use]
    pub const fn own_subtitle_style(&self) -> Option<&crate::SubtitleStyle> {
        match &self.content {
            ClipContent::Subtitle { style, .. } => style.as_ref(),
            _ => None,
        }
    }

    /// 是不是调节片段。
    #[must_use]
    pub const fn is_adjustment(&self) -> bool {
        matches!(self.content, ClipContent::Adjustment)
    }
}

/// 一条轨道：片段按 `start` 升序、互不重叠。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// id。
    pub id: TrackId,
    /// 类型。
    pub kind: TrackKind,
    /// 片段。
    pub clips: Vec<Arc<Clip>>,
    /// 隐藏（剪映轨道头的 👁）：画面、字幕不进预览和导出。没有时不写进文件。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// 静音（轨道头的 🔊）：声音不进预览和导出。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub muted: bool,
    /// 锁定（轨道头的 🔒）：片段不能选中、不能改。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
}

impl Track {
    /// 空轨道。
    #[must_use]
    pub const fn new(id: TrackId, kind: TrackKind) -> Self {
        Self {
            id,
            kind,
            clips: Vec::new(),
            hidden: false,
            muted: false,
            locked: false,
        }
    }

    /// 画面 / 字幕是否进预览和导出（没隐藏）。
    #[must_use]
    pub const fn shows(&self) -> bool {
        !self.hidden
    }

    /// 声音是否进预览和导出（没静音）。
    #[must_use]
    pub const fn sounds(&self) -> bool {
        !self.muted
    }

    /// 轨道终点（最后一个片段的终点）。
    #[must_use]
    pub fn end(&self) -> Time {
        self.clips.last().map_or(Time::ZERO, |c| c.end())
    }

    /// 片段下标。
    #[must_use]
    pub fn index_of(&self, clip: ClipId) -> Option<usize> {
        self.clips.iter().position(|c| c.id == clip)
    }

    /// 时间点 `t` 处的片段（`start <= t < end`）。
    #[must_use]
    pub fn clip_at(&self, t: Time) -> Option<&Arc<Clip>> {
        let idx = self.clips.partition_point(|c| c.start <= t);
        idx.checked_sub(1)
            .and_then(|i| self.clips.get(i))
            .filter(|c| t < c.end())
    }

    /// 区间 `[start, end)` 是否空闲（忽略 `ignore` 里的片段）。
    #[must_use]
    pub fn is_free(&self, start: Time, end: Time, ignore: &[ClipId]) -> bool {
        self.clips
            .iter()
            .filter(|c| !ignore.contains(&c.id))
            .all(|c| c.end() <= start || end <= c.start)
    }

    /// 插入并保持排序（不检查重叠）。
    pub fn insert_sorted(&mut self, clip: Arc<Clip>) {
        let idx = self.clips.partition_point(|c| c.start <= clip.start);
        self.clips.insert(idx, clip);
    }
}

/// 时间轴：若干轨道。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    /// 轨道，从上到下。
    pub tracks: Vec<Track>,
    /// 标记（剪映的 M），按时间排好、不重复；没有时不写进文件。第 5 版工程文件加的。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Time>,
    /// 入出点区域（剪映 I / O）：`(入点, 出点)`，入点 < 出点；没有时不写进文件。第 7 版工程文件加的。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<(Time, Time)>,
    /// 草稿封面（剪映主轨的「封面」）：没有时不写进文件。第 7 版工程文件加的。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<Cover>,
}

/// 草稿封面。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cover {
    /// 成片里这一刻的画面（时间轴时间）。
    Frame(Time),
    /// 一张本地图片。
    Image(std::path::PathBuf),
}

/// 模型不变式被破坏。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvariantError {
    /// 同轨片段重叠或未排序。
    #[error("片段 {clip:?} 与前一个片段重叠")]
    Overlap {
        /// 轨道。
        track: TrackId,
        /// 后一个片段。
        clip: ClipId,
    },
    /// 起点为负。
    #[error("片段 {clip:?} 起点为负")]
    NegativeStart {
        /// 片段。
        clip: ClipId,
    },
    /// 长度不为正。
    #[error("片段 {clip:?} 长度为零")]
    ZeroLength {
        /// 片段。
        clip: ClipId,
    },
    /// 超出素材范围。
    #[error("片段 {clip:?} 超出素材范围")]
    OutOfSource {
        /// 片段。
        clip: ClipId,
    },
    /// 引用的素材不存在。
    #[error("片段 {clip:?} 引用的素材 {asset:?} 不存在")]
    MissingAsset {
        /// 片段。
        clip: ClipId,
        /// 素材。
        asset: AssetId,
    },
    /// 入出点区域不合法。
    #[error("入出点区域不对（入点要在出点之前、不能是负数）")]
    BadRange,
    /// 片段内容与轨道类型不符。
    #[error("片段 {clip:?} 放错了轨道")]
    WrongTrackKind {
        /// 片段。
        clip: ClipId,
    },
    /// 淡入淡出为负或超过片段长度。
    #[error("片段 {clip:?} 淡入淡出无效")]
    BadFade {
        /// 片段。
        clip: ClipId,
    },
    /// 画面属性违反不变式。
    #[error("片段 {clip:?} 画面属性无效")]
    BadTransform {
        /// 片段。
        clip: ClipId,
    },
    /// 增益不是有限数。
    #[error("片段 {clip:?} 增益无效")]
    BadGain {
        /// 片段。
        clip: ClipId,
    },
}

impl Timeline {
    /// 新工程的轨道：字幕、视频、音频各一条。界面上从上到下就是这个顺序（剪映的排法：
    /// 文字轨在最上，视频主轨在视频轨的最下面，音频轨在最下面）。
    #[must_use]
    pub fn mvp(ids: &mut IdGen) -> Self {
        let (s, v, a) = (ids.track(), ids.track(), ids.track());
        Self {
            tracks: vec![
                Track::new(s, TrackKind::Subtitle),
                Track::new(v, TrackKind::Video),
                Track::new(a, TrackKind::Audio),
            ],
            markers: Vec::new(),
            range: None,
            cover: None,
        }
    }

    /// `at` 之后的第一个标记。
    #[must_use]
    pub fn next_marker(&self, at: Time) -> Option<Time> {
        self.markers.iter().copied().find(|m| *m > at)
    }

    /// `at` 之前的最后一个标记。
    #[must_use]
    pub fn prev_marker(&self, at: Time) -> Option<Time> {
        self.markers.iter().rev().copied().find(|m| *m < at)
    }

    /// 在 `at` 打标记；那里已经有就去掉。返回现在那里有没有标记。
    pub fn toggle_marker(&mut self, at: Time) -> bool {
        match self.markers.binary_search(&at) {
            Ok(i) => {
                self.markers.remove(i);
                false
            }
            Err(i) => {
                self.markers.insert(i, at);
                true
            }
        }
    }

    /// 设入点（I）：已有出点且在入点之后就保留，否则出点放到 `end`（时间轴末尾）。
    /// `at` 不在 `end` 之前时什么都不做，返回 `false`。
    pub fn set_in_point(&mut self, at: Time, end: Time) -> bool {
        let out = self.range.map(|r| r.1).filter(|o| *o > at).unwrap_or(end);
        if at < Time::ZERO || at >= out {
            return false;
        }
        self.range = Some((at, out));
        true
    }

    /// 设出点（O）：已有入点且在出点之前就保留，否则入点放到开头。
    /// `at` 不在 0 之后时什么都不做，返回 `false`。
    pub fn set_out_point(&mut self, at: Time) -> bool {
        let start = self
            .range
            .map(|r| r.0)
            .filter(|i| *i < at)
            .unwrap_or(Time::ZERO);
        if at <= start {
            return false;
        }
        self.range = Some((start, at));
        true
    }

    /// 把字幕轨挪到最前面（其余轨道的相对顺序不变），标记排好序去重。旧版工程文件里字幕轨在最后。
    pub fn normalize_order(&mut self) {
        self.tracks.sort_by_key(|t| t.kind != TrackKind::Subtitle);
        // 手改过的文件里标记可能乱序或重复
        self.markers.sort_unstable();
        self.markers.dedup();
    }

    /// 视频轨，从最下层到最上层（合成顺序）。轨道列表的顺序就是界面上从上到下的顺序。
    pub fn video_tracks_bottom_up(&self) -> impl Iterator<Item = &Track> {
        self.tracks
            .iter()
            .rev()
            .filter(|t| t.kind == TrackKind::Video)
    }

    /// 按 id 找轨道。
    #[must_use]
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// 按 id 找轨道（可变）。
    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// 第一条该类型的轨道。
    #[must_use]
    pub fn track_of_kind(&self, kind: TrackKind) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == kind)
    }

    /// 第一条该类型的轨道（可变）。
    pub fn track_of_kind_mut(&mut self, kind: TrackKind) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.kind == kind)
    }

    /// 起点在 `at` 之前的片段（剪映「向左全选」）。
    #[must_use]
    pub fn clips_before(&self, at: Time) -> Vec<ClipId> {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter().filter(|c| c.start < at).map(|c| c.id))
            .collect()
    }

    /// 终点在 `at` 之后的片段（剪映「向右全选」）。
    #[must_use]
    pub fn clips_after(&self, at: Time) -> Vec<ClipId> {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter().filter(|c| c.end() > at).map(|c| c.id))
            .collect()
    }

    /// 找片段及其所在轨道。
    #[must_use]
    pub fn find_clip(&self, id: ClipId) -> Option<(TrackId, &Arc<Clip>)> {
        self.tracks
            .iter()
            .find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t.id, c)))
    }

    /// 时间轴总长（所有轨道终点的最大值）。
    #[must_use]
    pub fn duration(&self) -> Time {
        self.tracks
            .iter()
            .map(Track::end)
            .max()
            .unwrap_or(Time::ZERO)
    }

    /// 检查所有不变式。
    ///
    /// # Errors
    /// 返回第一个被破坏的不变式。
    pub fn check_invariants(
        &self,
        assets: &BTreeMap<AssetId, Asset>,
    ) -> Result<(), InvariantError> {
        for track in &self.tracks {
            let mut prev_end: Option<Time> = None;
            for clip in &track.clips {
                check_clip(track.kind, clip, assets)?;
                if prev_end.is_some_and(|end| clip.start < end) {
                    return Err(InvariantError::Overlap {
                        track: track.id,
                        clip: clip.id,
                    });
                }
                prev_end = Some(clip.end());
            }
        }
        if self.range.is_some_and(|(a, b)| a < Time::ZERO || a >= b)
            || matches!(self.cover, Some(Cover::Frame(t)) if t < Time::ZERO)
        {
            return Err(InvariantError::BadRange);
        }
        Ok(())
    }
}

fn check_clip(
    kind: TrackKind,
    clip: &Clip,
    assets: &BTreeMap<AssetId, Asset>,
) -> Result<(), InvariantError> {
    let id = clip.id;
    if clip.start < Time::ZERO {
        return Err(InvariantError::NegativeStart { clip: id });
    }
    if clip.duration <= Time::ZERO {
        return Err(InvariantError::ZeroLength { clip: id });
    }
    if clip.label.is_some_and(|l| l >= LABEL_COLORS) {
        return Err(InvariantError::BadTransform { clip: clip.id });
    }
    if !clip.gain_db.is_finite() {
        return Err(InvariantError::BadGain { clip: id });
    }
    if !clip.transform.is_valid()
        || !clip.animation.is_valid()
        || !clip.adjust.is_valid()
        || !clip.look.is_valid()
        || clip.curve.as_ref().is_some_and(|c| !c.is_valid())
        || clip.grade.as_ref().is_some_and(|g| !g.is_valid())
        || !(SPEED_RANGE.0..=SPEED_RANGE.1).contains(&clip.speed)
    {
        return Err(InvariantError::BadTransform { clip: id });
    }
    if clip.fade_in < Time::ZERO
        || clip.fade_out < Time::ZERO
        || clip.fade_in + clip.fade_out > clip.duration
    {
        return Err(InvariantError::BadFade { clip: id });
    }
    match (&clip.content, kind) {
        (ClipContent::Subtitle { .. }, TrackKind::Subtitle)
        | (ClipContent::Adjustment, TrackKind::Video) => Ok(()),
        (ClipContent::Media { asset, source_in }, TrackKind::Video | TrackKind::Audio) => {
            let a = assets.get(asset).ok_or(InvariantError::MissingAsset {
                clip: id,
                asset: *asset,
            })?;
            let fits = match kind {
                TrackKind::Video => a.video.is_some(),
                _ => a.audio.is_some(),
            };
            if !fits {
                return Err(InvariantError::WrongTrackKind { clip: id });
            }
            if *source_in < Time::ZERO || *source_in + clip.source_span() > a.source_limit() {
                return Err(InvariantError::OutOfSource { clip: id });
            }
            Ok(())
        }
        _ => Err(InvariantError::WrongTrackKind { clip: id }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{media_clip, sample_project};
    use crate::{Project, Time, VideoInfo};
    use std::sync::Arc;

    #[test]
    fn mvp_timeline_has_three_tracks() {
        let project = Project::new();
        let kinds: Vec<TrackKind> = project.timeline.tracks.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [TrackKind::Subtitle, TrackKind::Video, TrackKind::Audio]
        );
        assert!(project.timeline.track_of_kind(TrackKind::Audio).is_some());
        assert_eq!(project.timeline.duration(), Time::ZERO);
        project.timeline.check_invariants(&project.assets).unwrap();
    }

    #[test]
    fn transform_at_follows_animation_and_invalid_animation_rejected() {
        let (mut project, ids) = sample_project();
        let tr = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        let mut c = (*tr.clips[0]).clone();
        assert_eq!(
            c.transform_at(Time::from_secs(1)),
            c.transform,
            "没动画：固定值"
        );
        c.animation.upsert(crate::Prop::Opacity, Time::ZERO, 0.0);
        c.animation
            .upsert(crate::Prop::Opacity, Time::from_secs(2), 1.0);
        c.animation.set_easing(Time::ZERO, crate::Easing::Linear);
        assert!((c.transform_at(Time::from_secs(1)).opacity - 0.5).abs() < 1e-6);
        tr.clips[0] = Arc::new(c.clone());
        project.timeline.check_invariants(&project.assets).unwrap();
        // 关键帧值不合法：不变式拦住
        c.animation.upsert(crate::Prop::Scale, Time::ZERO, 0.0);
        let video = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        video.clips[0] = Arc::new(c);
        assert_eq!(
            project.timeline.check_invariants(&project.assets),
            Err(InvariantError::BadTransform { clip: ids.v1 })
        );
    }

    #[test]
    fn find_clip_and_duration() {
        let (project, ids) = sample_project();
        let (track, clip) = project.timeline.find_clip(ids.v1).unwrap();
        assert_eq!(
            project.timeline.track(track).unwrap().kind,
            TrackKind::Video
        );
        assert_eq!(clip.start, Time::ZERO);
        assert_eq!(project.timeline.duration(), Time::from_secs(8));
        assert!(project.timeline.find_clip(ClipId(9999)).is_none());
        project.timeline.check_invariants(&project.assets).unwrap();
    }

    #[test]
    fn invariants_reject_overlap() {
        let (mut project, ids) = sample_project();
        let v = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        let extra = media_clip(
            ClipId(500),
            ids.video_asset,
            Time::from_secs(1),
            Time::from_secs(2),
        );
        v.clips.push(Arc::new(extra));
        v.clips.sort_by_key(|c| c.start);
        let err = project
            .timeline
            .check_invariants(&project.assets)
            .unwrap_err();
        assert!(matches!(err, InvariantError::Overlap { .. }), "{err:?}");
    }

    #[test]
    fn invariants_reject_unsorted() {
        let (mut project, _) = sample_project();
        let v = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        v.clips.swap(0, 1);
        let err = project
            .timeline
            .check_invariants(&project.assets)
            .unwrap_err();
        assert!(matches!(err, InvariantError::Overlap { .. }), "{err:?}");
    }

    #[test]
    fn invariants_reject_source_out_of_bounds() {
        let (mut project, ids) = sample_project();
        let v = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        let mut clip = (*v.clips[0]).clone();
        clip.content = ClipContent::Media {
            asset: ids.video_asset,
            source_in: Time::from_secs(9),
        };
        v.clips[0] = Arc::new(clip);
        let err = project
            .timeline
            .check_invariants(&project.assets)
            .unwrap_err();
        assert!(matches!(err, InvariantError::OutOfSource { .. }), "{err:?}");
    }

    /// 改示例工程里某轨第一个片段后检查不变式。
    fn check_after(kind: TrackKind, edit: impl FnOnce(&mut Clip)) -> Result<(), InvariantError> {
        let (mut project, _) = sample_project();
        let track = project.timeline.track_of_kind_mut(kind).unwrap();
        let mut clip = (*track.clips[0]).clone();
        edit(&mut clip);
        track.clips[0] = Arc::new(clip);
        project.timeline.check_invariants(&project.assets)
    }

    #[test]
    fn invariants_reject_bad_fade() {
        let err = check_after(TrackKind::Video, |c| {
            c.fade_in = Time::from_secs(3);
            c.fade_out = Time::from_secs(3);
        });
        assert!(
            matches!(err, Err(InvariantError::BadFade { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn invariants_reject_zero_length() {
        let err = check_after(TrackKind::Audio, |c| c.duration = Time::ZERO);
        assert!(
            matches!(err, Err(InvariantError::ZeroLength { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn invariants_reject_negative_start() {
        let err = check_after(TrackKind::Audio, |c| c.start = Time::from_secs(-1));
        assert!(
            matches!(err, Err(InvariantError::NegativeStart { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn invariants_reject_nan_gain() {
        let err = check_after(TrackKind::Audio, |c| c.gain_db = f32::NAN);
        assert!(
            matches!(err, Err(InvariantError::BadGain { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn invariants_reject_media_on_subtitle_track() {
        let (mut project, ids) = sample_project();
        let s = project
            .timeline
            .track_of_kind_mut(TrackKind::Subtitle)
            .unwrap();
        let clip = media_clip(ClipId(501), ids.video_asset, Time::ZERO, Time::from_secs(1));
        s.clips.insert(0, Arc::new(clip));
        let err = project.timeline.check_invariants(&project.assets);
        assert!(
            matches!(err, Err(InvariantError::WrongTrackKind { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn invariants_reject_missing_asset() {
        let (mut project, ids) = sample_project();
        project.assets.remove(&ids.audio_asset);
        let err = project.timeline.check_invariants(&project.assets);
        assert!(
            matches!(err, Err(InvariantError::MissingAsset { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn still_image_has_no_source_limit() {
        let (mut project, ids) = sample_project();
        let asset = project.assets.get_mut(&ids.video_asset).unwrap();
        asset.video = asset.video.map(|v| VideoInfo { still: true, ..v });
        let track = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        let mut clip = (*track.clips[1]).clone();
        clip.duration = Time::from_secs(3600);
        track.clips[1] = Arc::new(clip);
        project.timeline.check_invariants(&project.assets).unwrap();
    }

    #[test]
    fn track_queries() {
        let (project, ids) = sample_project();
        let v = project.timeline.track(ids.video_track).unwrap();
        assert_eq!(v.clip_at(Time::from_secs(5)).map(|c| c.id), Some(ids.v2));
        assert_eq!(v.clip_at(Time::from_secs(8)).map(|c| c.id), None);
        assert!(v.is_free(Time::from_secs(8), Time::from_secs(9), &[]));
        assert!(!v.is_free(Time::from_secs(4), Time::from_secs(6), &[ids.v1]));
        assert!(v.is_free(Time::from_secs(4), Time::from_secs(5), &[ids.v1]));
        assert_eq!(v.index_of(ids.v2), Some(1));
        let s = &project.timeline.find_clip(ids.s1).unwrap().1;
        assert_eq!(s.subtitle_text(), Some("你好"));
        assert_eq!(s.asset(), None);
        assert_eq!(s.source_in(), Time::ZERO);
    }

    #[test]
    fn audio_only_asset_rejected_on_video_track() {
        let (mut project, ids) = sample_project();
        let v = project
            .timeline
            .track_of_kind_mut(TrackKind::Video)
            .unwrap();
        let mut clip = (*v.clips[0]).clone();
        clip.content = ClipContent::Media {
            asset: ids.audio_asset,
            source_in: Time::ZERO,
        };
        v.clips[0] = Arc::new(clip);
        assert!(matches!(
            project.timeline.check_invariants(&project.assets),
            Err(InvariantError::WrongTrackKind { .. })
        ));
    }

    #[test]
    fn clips_before_and_after_a_time() {
        let (p, ids) = sample_project();
        let mut before = p.timeline.clips_before(Time::from_secs(4));
        before.sort_unstable();
        let mut want_before = vec![ids.v1, ids.a1, ids.s1];
        want_before.sort_unstable();
        assert_eq!(before, want_before);
        let mut after = p.timeline.clips_after(Time::from_secs(4));
        after.sort_unstable();
        let mut want_after = vec![ids.v1, ids.v2, ids.a1];
        want_after.sort_unstable();
        assert_eq!(after, want_after);
        assert_eq!(
            p.timeline.clips_after(Time::from_secs(8)),
            Vec::<ClipId>::new()
        );
    }

    #[test]
    fn normalize_moves_text_tracks_first_keeping_the_rest() {
        let mut ids = IdGen::default();
        let (v2, v1, a1, s1) = (ids.track(), ids.track(), ids.track(), ids.track());
        let mut tl = Timeline {
            markers: Vec::new(),
            range: None,
            cover: None,
            tracks: vec![
                Track::new(v2, TrackKind::Video),
                Track::new(v1, TrackKind::Video),
                Track::new(a1, TrackKind::Audio),
                Track::new(s1, TrackKind::Subtitle),
            ],
        };
        tl.markers = vec![Time::from_secs(3), Time::from_secs(1), Time::from_secs(3)];
        tl.normalize_order();
        let order: Vec<_> = tl.tracks.iter().map(|t| t.id).collect();
        assert_eq!(order, [s1, v2, v1, a1]);
        assert_eq!(tl.markers, [Time::from_secs(1), Time::from_secs(3)]);
    }

    #[test]
    fn markers_toggle_and_navigate() {
        let mut tl = Timeline::mvp(&mut IdGen::default());
        assert!(tl.toggle_marker(Time::from_secs(5)));
        assert!(tl.toggle_marker(Time::from_secs(2)));
        assert_eq!(
            tl.markers,
            [Time::from_secs(2), Time::from_secs(5)],
            "排好序"
        );
        assert_eq!(tl.next_marker(Time::from_secs(2)), Some(Time::from_secs(5)));
        assert_eq!(tl.prev_marker(Time::from_secs(2)), None);
        assert_eq!(tl.prev_marker(Time::from_secs(9)), Some(Time::from_secs(5)));
        assert!(!tl.toggle_marker(Time::from_secs(5)), "再按一次去掉");
        assert_eq!(tl.markers, [Time::from_secs(2)]);
    }
}
