//! 轨道头：轨道名，锁定 / 隐藏 / 静音开关，加轨 / 删轨按钮。

use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _, h_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, MouseButton,
    ParentElement as _, PromptLevel, SharedString, Styled as _, Window, div, px,
};
use ve_core::edit::{TrackFlag, add_track, remove_track, set_track_flag};
use ve_core::{Timeline, TrackId, TrackKind};

/// 轨道头上的类型图标（剪映的轨道头是图标）：视频 ▣、音频 ♪、文字 T。
#[must_use]
pub const fn track_icon(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "▣",
        TrackKind::Audio => "♪",
        TrackKind::Subtitle => "T",
    }
}

/// 轨道名，和轨道列表一一对应。视频轨从下往上数 V1、V2…（越上层号越大），
/// 音频轨从上往下数 A1、A2…，字幕轨 S1…。
#[must_use]
pub fn track_names(timeline: &Timeline) -> Vec<String> {
    let videos = timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .count();
    let texts = timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Subtitle)
        .count();
    let (mut v, mut a, mut s) = (0, 0, 0);
    timeline
        .tracks
        .iter()
        .map(|t| match t.kind {
            TrackKind::Video => {
                v += 1;
                format!("V{}", videos + 1 - v)
            }
            TrackKind::Audio => {
                a += 1;
                format!("A{a}")
            }
            // 文字轨也从下往上数：贴着视频的是 S1（字幕），叠在上面的标题是 S2、S3…
            TrackKind::Subtitle => {
                s += 1;
                format!("S{}", texts + 1 - s)
            }
        })
        .collect()
}

/// 这条轨能不能删：同类型至少留一条。
#[must_use]
pub fn deletable(timeline: &Timeline, index: usize) -> bool {
    timeline
        .tracks
        .get(index)
        .is_some_and(|t| timeline.tracks.iter().filter(|o| o.kind == t.kind).count() > 1)
}

impl Workspace {
    fn add_track_clicked(
        &mut self,
        kind: TrackKind,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.apply(window, cx, |p| add_track(p, kind));
    }

    /// 删轨：空轨直接删；有片段时先确认（片段会一起删，能撤销）。
    fn remove_track_clicked(
        &mut self,
        track: TrackId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let has_clips = self
            .doc
            .project
            .timeline
            .track(track)
            .is_some_and(|t| !t.clips.is_empty());
        if !has_clips {
            self.apply(window, cx, |p| remove_track(p, track));
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "删除这条轨道？",
            Some("轨道上的片段会一起删除，可以撤销。"),
            &["删除", "取消"],
            cx,
        );
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    this.apply(window, cx, |p| remove_track(p, track));
                });
            }
        }));
    }

    /// 叠在时间轴左侧的轨道头按钮：左上角加轨，每条轨右侧一排开关（锁定、隐藏、静音）和删除。
    /// 每个按钮各自拦下按下事件，否则时间轴会把它当成在标尺 / 轨道头上点了一下。
    pub(crate) fn track_buttons(&self, cx: &Context<'_, Self>) -> Vec<AnyElement> {
        let geo = self.timeline.geometry;
        let timeline = &self.doc.project.timeline;
        let add = h_flex()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .absolute()
            .top_0()
            .left_0()
            .w(px(geo.header_width))
            .h(px(geo.ruler_height))
            .justify_center()
            .gap_0p5()
            .child(
                Button::new("add-video-track")
                    .ghost()
                    .compact()
                    .label("+V")
                    .tooltip("添加视频轨（放在最上层）")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_track_clicked(TrackKind::Video, window, cx);
                    })),
            )
            .child(
                Button::new("add-audio-track")
                    .ghost()
                    .compact()
                    .label("+A")
                    .tooltip("添加音频轨")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_track_clicked(TrackKind::Audio, window, cx);
                    })),
            );
        let rows = timeline.tracks.iter().enumerate().filter_map(|(i, track)| {
            let top = geo.track_top(i) + (geo.row_height(i) - HEADER_BUTTON) / 2.0;
            // 滚到标尺下面或时间轴下边以外的不画
            if top < geo.ruler_height || top > f32::from(self.timeline.bounds.size.height) {
                return None;
            }
            Some(
                h_flex()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .absolute()
                    .top(px(top))
                    .left(px(geo.header_width - 4.0 * HEADER_BUTTON - 4.0))
                    .children(header_slots(timeline, i).map(|slot| {
                        slot.map_or_else(
                            || div().size(px(HEADER_BUTTON)).into_any_element(),
                            |slot| Self::header_button(track, i, slot, cx).into_any_element(),
                        )
                    }))
                    .into_any_element(),
            )
        });
        // 只有主轨有「封面」（剪映）
        let main = ve_core::magnet::main_track(timeline);
        let cover = timeline
            .tracks
            .iter()
            .position(|t| Some(t.id) == main)
            .map(|i| geo.track_top(i) + (geo.row_height(i) - HEADER_BUTTON) / 2.0)
            .filter(|top| {
                *top >= geo.ruler_height && *top <= f32::from(self.timeline.bounds.size.height)
            })
            .map(|top| self.render_cover_button(top, cx));
        std::iter::once(add.into_any_element())
            .chain(rows)
            .chain(cover)
            .collect()
    }

    /// 轨道头的一个图标按钮：开着的开关用主色青。
    fn header_button(
        track: &ve_core::Track,
        index: usize,
        slot: HeaderSlot,
        cx: &Context<'_, Self>,
    ) -> Button {
        let id = track.id;
        let (icon, on, tip) = match slot {
            HeaderSlot::Lock => (
                if track.locked {
                    Lucide::Lock
                } else {
                    Lucide::LockOpen
                },
                track.locked,
                if track.locked {
                    "解锁轨道"
                } else {
                    "锁定轨道：片段不能选中、不能改"
                },
            ),
            HeaderSlot::Eye => (
                if track.hidden {
                    Lucide::EyeOff
                } else {
                    Lucide::Eye
                },
                track.hidden,
                if track.hidden {
                    "显示轨道"
                } else {
                    "隐藏轨道：画面、字幕不进预览和导出"
                },
            ),
            HeaderSlot::Sound => (
                if track.muted {
                    Lucide::VolumeX
                } else {
                    Lucide::Volume2
                },
                track.muted,
                if track.muted {
                    "取消静音"
                } else {
                    "静音轨道：声音不进预览和导出"
                },
            ),
            HeaderSlot::Delete => (Lucide::X, false, "删除这条轨道"),
        };
        let icon = Icon::new(icon).size(px(14.));
        let icon = if on {
            icon.text_color(palette::accent())
        } else {
            icon.text_color(palette::tick())
        };
        Button::new(SharedString::from(format!("track-{}-{index}", slot.name())))
            .ghost()
            .xsmall()
            .icon(icon)
            .tooltip(tip)
            .on_click(cx.listener(move |this, _, window, cx| {
                let flag = match slot {
                    HeaderSlot::Lock => TrackFlag::Locked,
                    HeaderSlot::Eye => TrackFlag::Hidden,
                    HeaderSlot::Sound => TrackFlag::Muted,
                    HeaderSlot::Delete => {
                        this.remove_track_clicked(id, window, cx);
                        return;
                    }
                };
                this.apply(window, cx, |p| {
                    let now = p.timeline.track(id).is_some_and(|t| match flag {
                        TrackFlag::Locked => t.locked,
                        TrackFlag::Hidden => t.hidden,
                        TrackFlag::Muted => t.muted,
                    });
                    set_track_flag(p, id, flag, !now)
                });
            }))
    }
}

/// 轨道头按钮的边长。
pub const HEADER_BUTTON: f32 = 22.0;

/// 轨道头右侧的按钮位（从左到右：锁定、隐藏、静音、删除）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderSlot {
    /// 🔒
    Lock,
    /// 👁（视频轨、字幕轨）
    Eye,
    /// 🔊（视频轨、音频轨）
    Sound,
    /// ×（能删的轨）
    Delete,
}

impl HeaderSlot {
    const fn name(self) -> &'static str {
        match self {
            Self::Lock => "lock",
            Self::Eye => "eye",
            Self::Sound => "sound",
            Self::Delete => "del",
        }
    }
}

/// 第 `index` 条轨道头上四个按钮位放什么（`None` 留空，列对齐）。
#[must_use]
pub fn header_slots(timeline: &Timeline, index: usize) -> [Option<HeaderSlot>; 4] {
    let Some(track) = timeline.tracks.get(index) else {
        return [None; 4];
    };
    let picture = matches!(track.kind, TrackKind::Video | TrackKind::Subtitle);
    let sound = matches!(track.kind, TrackKind::Video | TrackKind::Audio);
    [
        Some(HeaderSlot::Lock),
        picture.then_some(HeaderSlot::Eye),
        sound.then_some(HeaderSlot::Sound),
        deletable(timeline, index).then_some(HeaderSlot::Delete),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_track_kind_has_its_own_icon() {
        let icons = [TrackKind::Video, TrackKind::Audio, TrackKind::Subtitle].map(track_icon);
        assert_eq!(icons, ["▣", "♪", "T"]);
    }
    use ve_core::Project;

    #[test]
    fn names_count_video_bottom_up_and_audio_top_down() {
        let mut p = Project::new();
        add_track(&mut p, TrackKind::Video).unwrap();
        add_track(&mut p, TrackKind::Audio).unwrap();
        // [S, V新, V, A, A新]
        assert_eq!(track_names(&p.timeline), ["S1", "V2", "V1", "A1", "A2"]);
        assert!(deletable(&p.timeline, 1));
        assert!(deletable(&p.timeline, 4));
        assert!(!deletable(&p.timeline, 0), "字幕轨不能删");
        assert!(!deletable(&p.timeline, 9));
        let single = Project::new();
        assert!(!deletable(&single.timeline, 1), "最后一条视频轨不能删");
    }

    #[test]
    fn header_slots_follow_the_track_kind() {
        use HeaderSlot::{Delete, Eye, Lock, Sound};
        let mut p = Project::new();
        add_track(&mut p, TrackKind::Video).unwrap();
        // [S, V新, V, A]
        let slots = |i| header_slots(&p.timeline, i);
        assert_eq!(
            slots(0),
            [Some(Lock), Some(Eye), None, None],
            "字幕轨没有声音、不能删"
        );
        assert_eq!(slots(1), [Some(Lock), Some(Eye), Some(Sound), Some(Delete)]);
        assert_eq!(
            slots(3),
            [Some(Lock), None, Some(Sound), None],
            "唯一的音频轨不能删"
        );
        assert_eq!(slots(9), [None; 4]);
    }
}
