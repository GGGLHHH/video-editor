//! 从素材面板拖到时间轴（剪映：拖着走的时候时间轴上就显示会落在哪，松手放下）。
//!
//! 能拖的：素材（视频 / 音频 / 图片）、默认文本、自定义调节、画面特效（含插件）、滤镜、转场。
//! 前四种在时间轴上新建一段片段：落在鼠标所在的同类轨道上（起点吸附到播放头和片段边缘），
//! 那里被占了就在旁边新建一条轨道；拖到两条轨道之间的缝上也新建轨道；主轨磁吸开着时，
//! 素材拖到主轨上按位置插队。滤镜、转场套到鼠标下面那段视频片段上。
//!
//! 这里只算落点（[`plan`]）和放下（[`apply`]），不碰界面。

use super::TimelineGeometry;
use super::interaction::Snapper;
use crate::document::Document;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
};
use ve_core::edit::{EditError, insert_slots};
use ve_core::{AssetId, ClipId, Effect, Project, Time, TrackKind, TransitionKind};

/// 新拖进来的文字片段多长。
pub const TEXT_LENGTH: Time = Time::from_secs(2);
/// 新拖进来的文字片段写什么。
pub const TEXT_DEFAULT: &str = "新字幕";

/// 从素材面板拖的是什么。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinItem {
    /// 素材。
    Asset(AssetId),
    /// 默认文本（一条新字幕）。
    Text,
    /// 自定义调节。
    Adjustment,
    /// 画面特效。
    Effect(Effect),
    /// 滤镜（[`ve_core::FILTERS`] 里的第几个）。
    Filter(usize),
    /// 转场。
    Transition(TransitionKind),
}

/// 拖动中的东西（跟着鼠标的小标签）。
#[derive(Debug, Clone)]
pub struct BinDrag {
    /// 拖的是什么。
    pub item: BinItem,
    /// 标签上的名字。
    pub name: SharedString,
}

impl Render for BinDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(cx.theme().primary)
            .text_color(cx.theme().primary_foreground)
            .text_sm()
            .child(self.name.clone())
    }
}

/// 让素材面板里的一张卡片能拖到时间轴上（松手处见 [`plan`]）。
pub fn draggable<E>(element: E, item: BinItem, name: impl Into<SharedString>) -> E
where
    E: gpui_kit::StatefulInteractiveElement,
{
    use gpui_kit::AppContext as _;
    element.on_drag(
        BinDrag {
            item,
            name: name.into(),
        },
        |d, _, _, cx| cx.new(|_| d.clone()),
    )
}

/// 落在哪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
    /// 放进第 `track` 条轨道（轨道列表下标）。
    Track {
        /// 轨道下标。
        track: usize,
        /// 起点。
        start: Time,
        /// 长度。
        duration: Time,
    },
    /// 在第 `slot` 条缝新建一条轨道放进去。
    NewTrack {
        /// 缝的下标（见 [`TimelineGeometry::insert_slot`]）。
        slot: usize,
        /// 起点。
        start: Time,
        /// 长度。
        duration: Time,
    },
    /// 主轨磁吸开着：插进主轨（第 `track` 条），排在 `at` 这个剪辑点上。
    Main {
        /// 主轨下标。
        track: usize,
        /// 插在哪个剪辑点（主轨上某段的终点或 0）。
        at: Time,
    },
    /// 套到这段片段上（滤镜、转场）。
    Clip(ClipId),
}

/// 一次落点计算的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropPlan {
    /// 落在哪。
    pub target: DropTarget,
    /// 吸附线（吸到播放头 / 片段边缘时）。
    pub snap: Option<Time>,
}

/// 新建片段的东西要放到哪类轨道上；滤镜、转场返回 `None`。
fn kind_of(project: &Project, item: BinItem) -> Option<TrackKind> {
    match item {
        BinItem::Asset(a) => {
            let a = project.assets.get(&a)?;
            if a.video.is_some() {
                Some(TrackKind::Video)
            } else {
                a.audio.is_some().then_some(TrackKind::Audio)
            }
        }
        BinItem::Text => Some(TrackKind::Subtitle),
        BinItem::Adjustment | BinItem::Effect(_) => Some(TrackKind::Video),
        BinItem::Filter(_) | BinItem::Transition(_) => None,
    }
}

/// 新片段的长度。
fn length_of(project: &Project, item: BinItem) -> Option<Time> {
    match item {
        BinItem::Asset(a) => project
            .assets
            .get(&a)
            .map(|a| a.duration)
            .filter(|d| *d > Time::ZERO),
        BinItem::Text => Some(TEXT_LENGTH),
        BinItem::Adjustment | BinItem::Effect(_) => Some(ve_core::edit::ADJUSTMENT_LENGTH),
        BinItem::Filter(_) | BinItem::Transition(_) => None,
    }
}

/// 鼠标在时间轴本地坐标 `(x, y)` 时，`item` 会落在哪；落不下（在标尺上、滤镜没对着视频片段……）
/// 返回 `None`。`snapping` 为假（按住 Alt）时只吸到帧。
#[must_use]
pub fn plan(
    doc: &Document,
    geo: &TimelineGeometry,
    item: BinItem,
    (x, y): (f32, f32),
    playhead: Time,
    snapping: bool,
) -> Option<DropPlan> {
    if y < geo.ruler_height || x < geo.header_width {
        return None;
    }
    let tl = &doc.project.timeline;
    let Some(kind) = kind_of(&doc.project, item) else {
        // 滤镜、转场：鼠标下面那段视频片段
        let super::Hit::Clip { clip, .. } = geo.hit_test(x, y, tl) else {
            return None;
        };
        let (track, target) = tl.find_clip(clip)?;
        let on_video = tl
            .track(track)
            .is_some_and(|t| t.kind == TrackKind::Video && !t.locked);
        // 素材片段才行：滤镜、转场套在复合片段上不生效（决策 114）
        let picture = target.asset().is_some();
        return (on_video && picture).then_some(DropPlan {
            target: DropTarget::Clip(clip),
            snap: None,
        });
    };
    let duration = length_of(&doc.project, item)?;
    let snapper = Snapper::new(doc, geo, playhead, &[], snapping);
    let (start, snap) = snapper.place_range(geo.x_to_time(x).max(Time::ZERO), duration);
    let start = start.max(Time::ZERO);
    let slots = insert_slots(tl, kind);
    // 对着别类轨道或轨道下面时：这类轨道最上面（视频）/ 最下面（音频）新建一条
    let fallback = slots.as_ref().map(|r| {
        if kind == TrackKind::Audio {
            *r.end()
        } else {
            // 视频、文字：同类轨道最上面
            *r.start()
        }
    });
    let can_insert = |slot: usize| slots.as_ref().is_some_and(|r| r.contains(&slot));
    let new_track = |slot: usize| {
        can_insert(slot).then_some(DropPlan {
            target: DropTarget::NewTrack {
                slot,
                start,
                duration,
            },
            snap,
        })
    };
    let into = |track: usize| DropPlan {
        target: DropTarget::Track {
            track,
            start,
            duration,
        },
        snap,
    };
    // 两条轨道之间的缝：新建
    if let Some(slot) = geo
        .insert_slot(y, tl.tracks.len())
        .filter(|&s| can_insert(s))
    {
        return new_track(slot);
    }
    // 文字：对着哪条文字轨、那里空着就放；别处按「最下面一条空着的文字轨，都占了在最上面新开一条」
    if kind == TrackKind::Subtitle {
        let open = |i: usize| {
            let t = &tl.tracks[i];
            t.kind == kind && !t.locked && t.is_free(start, start + duration, &[])
        };
        if let Some(i) = geo
            .track_index_at(y, tl.tracks.len())
            .filter(|&i| tl.tracks[i].kind == kind)
        {
            return if open(i) { Some(into(i)) } else { new_track(i) };
        }
        let bottom = tl.tracks.iter().rposition(|t| t.kind == kind)?;
        return match (0..=bottom).rev().find(|&i| open(i)) {
            Some(i) => Some(into(i)),
            None => new_track(fallback?),
        };
    }
    let hovered = geo.track_index_at(y, tl.tracks.len());
    if let Some(index) = hovered.filter(|&i| tl.tracks[i].kind == kind && !tl.tracks[i].locked) {
        let here = &tl.tracks[index];
        let main = ve_core::magnet::main_track(tl);
        let on_main = Some(here.id) == main;
        if doc.project.settings.main_magnet && on_main && matches!(item, BinItem::Asset(_)) {
            // 主轨磁吸：按位置插队（越过某段的中点就排到它后面）
            let at = main_insert_point(here, geo.x_to_time(x).max(Time::ZERO));
            return Some(DropPlan {
                target: DropTarget::Main { track: index, at },
                snap: None,
            });
        }
        // 调节、特效不放在主轨上（剪映：它们各占一条轨）
        let tool_on_main = on_main && matches!(item, BinItem::Adjustment | BinItem::Effect(_));
        if !tool_on_main && here.is_free(start, start + duration, &[]) {
            return Some(into(index));
        }
        // 被占了：视频在它上面、音频在它下面新建一条
        return new_track(if kind == TrackKind::Audio {
            index + 1
        } else {
            index
        });
    }
    new_track(fallback?)
}

/// 主轨磁吸时 `pointer` 处插队插在哪个剪辑点：越过某段的中点就排到它后面。
fn main_insert_point(main: &ve_core::Track, pointer: Time) -> Time {
    main.clips
        .iter()
        .take_while(|c| c.start + Time(c.duration.0 / 2) < pointer)
        .last()
        .map_or(Time::ZERO, |c| c.end())
}

/// 素材卡片上的「+」（剪映）：素材加到播放头 `at` 处。有画面的放主轨（磁吸开着按位置插队；
/// 关着时那里空着就放，被占了在主轨上面新建一条），纯音频放第一条音频轨（被占了在下面新建一条）。
#[must_use]
pub fn at_playhead(project: &Project, asset: AssetId, at: Time) -> Option<DropTarget> {
    let item = BinItem::Asset(asset);
    let kind = kind_of(project, item)?;
    let duration = length_of(project, item)?;
    let tl = &project.timeline;
    let index = match kind {
        TrackKind::Video => {
            let main = ve_core::magnet::main_track(tl)?;
            tl.tracks.iter().position(|t| t.id == main)?
        }
        _ => tl.tracks.iter().position(|t| t.kind == kind)?,
    };
    let track = &tl.tracks[index];
    if kind == TrackKind::Video && project.settings.main_magnet {
        return Some(DropTarget::Main {
            track: index,
            at: main_insert_point(track, at),
        });
    }
    if !track.locked && track.is_free(at, at + duration, &[]) {
        return Some(DropTarget::Track {
            track: index,
            start: at,
            duration,
        });
    }
    Some(DropTarget::NewTrack {
        slot: if kind == TrackKind::Audio {
            index + 1
        } else {
            index
        },
        start: at,
        duration,
    })
}

/// 按落点 `target` 放下 `item`；返回新片段（滤镜、转场返回套上的那段）。
/// `transition` 是转场的时长。失败时工程不变。
///
/// # Errors
/// 落点和东西对不上、轨道锁着、重叠等。
pub fn apply(
    project: &mut Project,
    item: BinItem,
    target: DropTarget,
    transition: Time,
) -> Result<ClipId, EditError> {
    let mut next = project.clone();
    let placed = place(&mut next, item, target, transition)?;
    *project = next;
    Ok(placed)
}

fn place(
    p: &mut Project,
    item: BinItem,
    target: DropTarget,
    transition: Time,
) -> Result<ClipId, EditError> {
    use ve_core::edit;
    let (track, start, duration) = match target {
        DropTarget::Clip(clip) => {
            match item {
                BinItem::Filter(i) => {
                    let f = ve_core::FILTERS.get(i).ok_or(EditError::InvalidValue)?;
                    edit::set_adjust(p, clip, f.adjust)?;
                }
                BinItem::Transition(kind) => edit::set_transition(
                    p,
                    clip,
                    Some(ve_core::Transition {
                        kind,
                        duration: transition,
                    }),
                )?,
                _ => return Err(EditError::WrongTrackKind),
            }
            return Ok(clip);
        }
        DropTarget::Main { track, at } => {
            let BinItem::Asset(asset) = item else {
                return Err(EditError::WrongTrackKind);
            };
            let t = p
                .timeline
                .tracks
                .get(track)
                .ok_or(EditError::WrongTrackKind)?;
            let (id, end) = (t.id, t.end());
            let clip = edit::insert_asset_at(p, asset, id, end)?;
            ve_core::magnet::insert_into_main(p, clip, at)?;
            return Ok(clip);
        }
        DropTarget::Track {
            track,
            start,
            duration,
        } => (
            p.timeline
                .tracks
                .get(track)
                .ok_or(EditError::WrongTrackKind)?
                .id,
            start,
            duration,
        ),
        DropTarget::NewTrack {
            slot,
            start,
            duration,
        } => {
            let kind = kind_of(p, item).ok_or(EditError::WrongTrackKind)?;
            (edit::insert_track_at(p, kind, slot)?, start, duration)
        }
    };
    match item {
        BinItem::Asset(asset) => edit::insert_asset_at(p, asset, track, start),
        BinItem::Text => edit::add_subtitle_on(p, track, start, duration, TEXT_DEFAULT.to_owned()),
        BinItem::Adjustment => edit::add_adjustment_on(p, track, start, duration),
        BinItem::Effect(e) => edit::add_effect_on(p, track, start, duration, e),
        BinItem::Filter(_) | BinItem::Transition(_) => Err(EditError::WrongTrackKind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::testing::{SampleIds, sample_project};

    /// 字幕轨（0）、视频轨（1）、音频轨（2）；每秒 100 像素。
    fn setup(magnet: bool) -> (Document, SampleIds, TimelineGeometry) {
        let (mut p, ids) = sample_project();
        p.settings.main_magnet = magnet;
        let g = TimelineGeometry {
            px_per_second: 100.0,
            ..TimelineGeometry::default()
        };
        (Document::from_project(p, None), ids, g)
    }

    fn at(g: &TimelineGeometry, secs: f64, track: usize) -> (f32, f32) {
        (
            g.time_to_x(Time::from_secs_f64(secs)),
            g.track_top(track) + g.track_height / 2.0,
        )
    }

    fn target(
        doc: &Document,
        g: &TimelineGeometry,
        item: BinItem,
        p: (f32, f32),
    ) -> Option<DropTarget> {
        plan(doc, g, item, p, Time::ZERO, true).map(|p| p.target)
    }

    #[test]
    fn media_lands_in_free_space_and_snaps_to_clip_edges() {
        let (doc, ids, g) = setup(false);
        let video = BinItem::Asset(ids.video_asset);
        assert_eq!(
            target(&doc, &g, video, at(&g, 9.0, 1)),
            Some(DropTarget::Track {
                track: 1,
                start: Time::from_secs(9),
                duration: Time::from_secs(10),
            })
        );
        // 离片段终点 8 秒只差 3 像素：吸过去，画吸附线
        let p = plan(&doc, &g, video, at(&g, 8.03, 1), Time::ZERO, true).unwrap();
        assert_eq!(p.snap, Some(Time::from_secs(8)));
        // 按住 Alt：不吸
        let free = plan(&doc, &g, video, at(&g, 8.03, 1), Time::ZERO, false).unwrap();
        assert_eq!(free.snap, None);
    }

    #[test]
    fn an_occupied_spot_or_another_kind_of_track_opens_a_new_track() {
        let (doc, ids, g) = setup(false);
        let video = BinItem::Asset(ids.video_asset);
        // 视频轨 2 秒处有片段：在它上面新建
        assert!(matches!(
            target(&doc, &g, video, at(&g, 2.0, 1)),
            Some(DropTarget::NewTrack { slot: 1, .. })
        ));
        // 视频轨和音频轨之间的缝
        let seam = (g.time_to_x(Time::from_secs(2)), g.track_top(2));
        assert!(matches!(
            target(&doc, &g, video, seam),
            Some(DropTarget::NewTrack { slot: 2, .. })
        ));
        // 纯音频拖到视频轨上：在音频轨最下面新建
        assert!(matches!(
            target(&doc, &g, BinItem::Asset(ids.audio_asset), at(&g, 2.0, 1)),
            Some(DropTarget::NewTrack { slot: 3, .. })
        ));
        // 标尺上落不下
        assert_eq!(
            target(&doc, &g, video, (g.time_to_x(Time::ZERO) + 50.0, 5.0)),
            None
        );
    }

    #[test]
    fn with_the_magnet_on_media_queues_into_the_main_track() {
        let (doc, ids, g) = setup(true);
        let video = BinItem::Asset(ids.video_asset);
        // 越过 v1（0–5）的中点：排到它后面
        assert_eq!(
            target(&doc, &g, video, at(&g, 4.0, 1)),
            Some(DropTarget::Main {
                track: 1,
                at: Time::from_secs(5)
            })
        );
        assert_eq!(
            target(&doc, &g, video, at(&g, 1.0, 1)),
            Some(DropTarget::Main {
                track: 1,
                at: Time::ZERO
            })
        );
        // 调节不进主轨：新建一条
        assert!(matches!(
            target(&doc, &g, BinItem::Adjustment, at(&g, 9.0, 1)),
            Some(DropTarget::NewTrack { slot: 1, .. })
        ));
    }

    #[test]
    fn text_goes_onto_the_text_track_and_looks_need_a_video_clip() {
        let (doc, ids, g) = setup(false);
        assert_eq!(
            target(&doc, &g, BinItem::Text, at(&g, 4.0, 2)),
            Some(DropTarget::Track {
                track: 0,
                start: Time::from_secs(4),
                duration: TEXT_LENGTH,
            })
        );
        // 1–3 秒已经有字幕：在上面新开一条文字轨叠放（标题）
        assert!(matches!(
            target(&doc, &g, BinItem::Text, at(&g, 1.5, 0)),
            Some(DropTarget::NewTrack { slot: 0, .. })
        ));
        let filter = BinItem::Filter(1);
        assert_eq!(
            target(&doc, &g, filter, at(&g, 1.0, 1)),
            Some(DropTarget::Clip(ids.v1))
        );
        assert_eq!(
            target(&doc, &g, filter, at(&g, 1.0, 2)),
            None,
            "音频片段不行"
        );
        assert_eq!(target(&doc, &g, filter, at(&g, 9.0, 1)), None, "空白处不行");
        assert_eq!(
            target(
                &doc,
                &g,
                BinItem::Transition(TransitionKind::Dissolve),
                at(&g, 6.0, 1)
            ),
            Some(DropTarget::Clip(ids.v2))
        );
    }

    #[test]
    fn the_plus_button_adds_at_the_playhead() {
        let secs = Time::from_secs;
        let (doc, ids, _) = setup(false);
        // 主轨 9 秒处空着：就放那里
        assert_eq!(
            at_playhead(&doc.project, ids.video_asset, secs(9)),
            Some(DropTarget::Track {
                track: 1,
                start: secs(9),
                duration: secs(10)
            })
        );
        // 2 秒处被占了：主轨上面新建一条
        assert!(matches!(
            at_playhead(&doc.project, ids.video_asset, secs(2)),
            Some(DropTarget::NewTrack { slot: 1, .. })
        ));
        // 纯音频：音频轨被占了，在下面新建
        assert!(matches!(
            at_playhead(&doc.project, ids.audio_asset, secs(2)),
            Some(DropTarget::NewTrack { slot: 3, .. })
        ));
        // 磁吸开着：插队
        let (doc, ids, _) = setup(true);
        assert_eq!(
            at_playhead(&doc.project, ids.video_asset, secs(6)),
            Some(DropTarget::Main {
                track: 1,
                at: secs(5)
            })
        );
    }

    #[test]
    fn applying_each_kind_of_drop_keeps_the_project_valid() {
        let (doc, ids, g) = setup(false);
        let effect = BinItem::Effect(Effect::new(ve_core::EffectKind::Blur));
        let cases = [
            (BinItem::Asset(ids.video_asset), at(&g, 9.0, 1)),
            (BinItem::Asset(ids.video_asset), at(&g, 2.0, 1)),
            (BinItem::Asset(ids.audio_asset), at(&g, 9.0, 2)),
            (BinItem::Text, at(&g, 4.0, 0)),
            (BinItem::Adjustment, at(&g, 2.0, 1)),
            (effect, at(&g, 2.0, 1)),
            (BinItem::Filter(2), at(&g, 1.0, 1)),
            (BinItem::Transition(TransitionKind::Push), at(&g, 6.0, 1)),
        ];
        for (item, p) in cases {
            let target = target(&doc, &g, item, p).unwrap();
            let mut project = doc.project.clone();
            let clip = apply(&mut project, item, target, Time::from_millis(500)).unwrap();
            project.timeline.check_invariants(&project.assets).unwrap();
            assert!(project.timeline.find_clip(clip).is_some(), "{item:?}");
        }
        // 主轨插队
        let (doc, ids, g) = setup(true);
        let item = BinItem::Asset(ids.video_asset);
        let target = target(&doc, &g, item, at(&g, 4.0, 1)).unwrap();
        let mut project = doc.project;
        let clip = apply(&mut project, item, target, Time::ZERO).unwrap();
        let (_, c) = project.timeline.find_clip(clip).unwrap();
        assert_eq!(c.start, Time::from_secs(5), "排在 v1 后面");
    }

    #[test]
    fn filters_land_on_media_clips_but_not_on_compounds() {
        let (mut doc, ids, g) = setup(false);
        let filter = BinItem::Filter(0);
        assert_eq!(
            target(&doc, &g, filter, at(&g, 2.0, 1)),
            Some(DropTarget::Clip(ids.v1))
        );
        // 打成复合片段：滤镜套在复合片段上不生效，就不让放
        doc.edit(|p| ve_core::compound::make_compound(p, &[ids.v1, ids.v2]).map(|_| ()))
            .unwrap();
        assert_eq!(target(&doc, &g, filter, at(&g, 2.0, 1)), None);
    }
}
