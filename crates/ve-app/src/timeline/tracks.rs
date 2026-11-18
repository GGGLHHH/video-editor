//! 轨道头：轨道名、加轨 / 删轨按钮。

use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::h_flex;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, MouseButton,
    ParentElement as _, PromptLevel, SharedString, Styled as _, Window, div, px,
};
use ve_core::edit::{add_track, remove_track};
use ve_core::{Timeline, TrackId, TrackKind};

/// 轨道名，和轨道列表一一对应。视频轨从下往上数 V1、V2…（越上层号越大），
/// 音频轨从上往下数 A1、A2…，字幕轨 S1…。
#[must_use]
pub fn track_names(timeline: &Timeline) -> Vec<String> {
    let videos = timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
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
            TrackKind::Subtitle => {
                s += 1;
                format!("S{s}")
            }
        })
        .collect()
}

/// 这条轨能不能删：同类型至少留一条，字幕轨这一版只有一条。
#[must_use]
pub fn deletable(timeline: &Timeline, index: usize) -> bool {
    timeline.tracks.get(index).is_some_and(|t| {
        t.kind != TrackKind::Subtitle
            && timeline.tracks.iter().filter(|o| o.kind == t.kind).count() > 1
    })
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

    /// 叠在时间轴左侧的轨道头按钮：左上角加轨，每条可删的轨右侧一个删除按钮。
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
        let deletes = timeline.tracks.iter().enumerate().filter_map(|(i, track)| {
            if !deletable(timeline, i) {
                return None;
            }
            let id = track.id;
            let top = geo.track_top(i) + 4.0;
            // 滚到标尺下面或时间轴下边以外的不画
            if top < geo.ruler_height || top > f32::from(self.timeline.bounds.size.height) {
                return None;
            }
            Some(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .absolute()
                    .top(px(top))
                    .left(px(geo.header_width - 26.0))
                    .child(
                        Button::new(SharedString::from(format!("track-del-{i}")))
                            .ghost()
                            .compact()
                            .label("×")
                            .tooltip("删除这条轨道")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_track_clicked(id, window, cx);
                            })),
                    )
                    .into_any_element(),
            )
        });
        std::iter::once(add.into_any_element())
            .chain(deletes)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
