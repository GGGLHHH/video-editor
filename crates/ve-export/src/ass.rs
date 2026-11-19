//! ASS 字幕：导出时交给 libass 烧录；样式与预览共用 [`SubtitleStyle`]。

use std::fmt::Write as _;
use ve_core::{Project, Time, TrackKind};

/// 字幕样式（预览和导出共用），存在工程里。
pub use ve_core::SubtitleStyle;

/// 工程里是否有字幕。
#[must_use]
pub fn has_subtitles(project: &Project) -> bool {
    project
        .timeline
        .tracks
        .iter()
        .any(|t| t.kind == TrackKind::Subtitle && t.shows() && t.clips.iter().any(|c| c.enabled()))
}

fn ass_time(out: &mut String, t: Time) {
    let cs = t.as_millis().max(0) / 10;
    let _ = write!(
        out,
        "{}:{:02}:{:02}.{:02}",
        cs / 360_000,
        cs / 6000 % 60,
        cs / 100 % 60,
        cs % 100
    );
}

/// ASS 文本转义：`{` `}` 防止被当成样式标签，反斜杠换成全角，换行变 `\N`。
fn escape(text: &str) -> String {
    text.replace('\r', "")
        .replace('\\', "＼")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace('\n', "\\N")
}

/// 生成 ASS 字幕文件内容。
#[must_use]
pub fn write_ass(project: &Project) -> String {
    let style = &project.subtitle_style;
    let (w, h) = (project.settings.width, project.settings.height);
    let mut out = String::new();
    let _ = write!(
        out,
        "[Script Info]\nScriptType: v4.00+\nPlayResX: {w}\nPlayResY: {h}\nWrapStyle: 0\nScaledBorderAndShadow: yes\n\n\
         [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, \
         Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, \
         Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Default,{},{},{},&H000000FF,{},&H64000000,{},0,0,0,100,100,0,0,1,{:.1},0,2,20,20,{},1\n\n\
         [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
        style.font,
        style.font_px(h),
        SubtitleStyle::ass_color(style.color),
        SubtitleStyle::ass_color(style.outline_color),
        if style.bold { -1 } else { 0 },
        style.outline_px(h),
        style.margin_px(h),
    );
    let mut clips: Vec<_> = project
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Subtitle && t.shows())
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.enabled())
        .filter_map(|c| {
            c.subtitle_text()
                .map(|text| (c.start, c.end(), text, c.text_motion().unwrap_or_default()))
        })
        .collect();
    clips.sort_by_key(|c| c.0);
    // 有入场 / 出场动画的字幕拆成三段（入场、中间、出场），各段用自己的标签：
    // 一行 ASS 只能有一个 \move，入场、出场都滑动时只能拆开
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "边距已取整，比画面高度小"
    )]
    let margin = style.margin_px(h) as u32;
    let (x, base_y) = (w / 2, h.saturating_sub(margin));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "滑动距离是画面高度的 5%"
    )]
    let slide = (f64::from(h) * f64::from(ve_core::text_motion::SLIDE)).round() as u32;
    let zoom = (ve_core::text_motion::ZOOM_FROM * 100.0).round();
    for (start, end, text, motion) in clips {
        let len = ve_core::text_motion::motion_len(end - start);
        let intro = intro_tags(motion.intro, len, x, base_y, slide, zoom);
        let outro = outro_tags(motion.outro, len, x, base_y, slide, zoom);
        let mid_start = if intro.is_some() { start + len } else { start };
        let mid_end = if outro.is_some() { end - len } else { end };
        let text = escape(text);
        let mut line = |from: Time, to: Time, tags: &str| {
            if to <= from {
                return;
            }
            out.push_str("Dialogue: 0,");
            ass_time(&mut out, from);
            out.push(',');
            ass_time(&mut out, to);
            let _ = writeln!(out, ",Default,,0,0,0,,{tags}{text}");
        };
        if let Some(tags) = &intro {
            line(start, mid_start, tags);
        }
        line(mid_start, mid_end, "");
        if let Some(tags) = &outro {
            line(mid_end, end, tags);
        }
    }
    out
}

/// 入场那一段的标签（`None` 是没有入场动画）。和 [`ve_core::text_pose`] 一样都是线性的。
fn intro_tags(
    intro: ve_core::TextIn,
    len: Time,
    x: u32,
    y: u32,
    slide: u32,
    zoom: f32,
) -> Option<String> {
    let ms = len.as_millis().max(0);
    match intro {
        ve_core::TextIn::None => None,
        ve_core::TextIn::FadeIn => Some(format!("{{\\fad({ms},0)}}")),
        ve_core::TextIn::SlideUp => Some(format!(
            "{{\\an2\\move({x},{},{x},{y})\\fad({ms},0)}}",
            y + slide
        )),
        ve_core::TextIn::ZoomIn => Some(format!(
            "{{\\fscx{zoom}\\fscy{zoom}\\t(\\fscx100\\fscy100)}}"
        )),
    }
}

/// 出场那一段的标签（`None` 是没有出场动画）。
fn outro_tags(
    outro: ve_core::TextOut,
    len: Time,
    x: u32,
    y: u32,
    slide: u32,
    zoom: f32,
) -> Option<String> {
    let ms = len.as_millis().max(0);
    match outro {
        ve_core::TextOut::None => None,
        ve_core::TextOut::FadeOut => Some(format!("{{\\fad(0,{ms})}}")),
        ve_core::TextOut::SlideDown => Some(format!(
            "{{\\an2\\move({x},{y},{x},{})\\fad(0,{ms})}}",
            y + slide
        )),
        ve_core::TextOut::ZoomOut => Some(format!("{{\\t(\\fscx{zoom}\\fscy{zoom})}}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::edit::{add_subtitle, set_subtitle_text};
    use ve_core::testing::sample_project;

    #[test]
    fn ass_header_and_dialogue() {
        let (p, _) = sample_project();
        let ass = write_ass(&p);
        assert!(ass.starts_with("[Script Info]\n"));
        assert!(ass.contains("PlayResX: 1920\nPlayResY: 1080\n"));
        // 1080 × 0.055 ≈ 59
        assert!(ass.contains("Style: Default,Microsoft YaHei,59,"), "{ass}");
        assert!(
            ass.contains("Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,你好\n"),
            "{ass}"
        );
    }

    #[test]
    fn ass_uses_the_project_style() {
        let (mut p, _) = sample_project();
        p.subtitle_style.color = 0xFF_D700;
        p.subtitle_style.bold = true;
        p.subtitle_style.font = "SimHei".into();
        let ass = write_ass(&p);
        assert!(
            ass.contains(
                "Style: Default,SimHei,59,&H0000D7FF,&H000000FF,&H00000000,&H64000000,-1,"
            ),
            "{ass}"
        );
    }

    #[test]
    fn hidden_subtitle_tracks_are_not_burned_in() {
        use ve_core::edit::{TrackFlag, set_track_flag};
        let (mut p, ids) = sample_project();
        set_track_flag(&mut p, ids.subtitle_track, TrackFlag::Hidden, true).unwrap();
        assert!(!has_subtitles(&p));
        assert!(!write_ass(&p).contains("你好"));
        assert_eq!(ve_core::srt::export_srt(&p).len(), 0);
    }

    #[test]
    fn disabled_subtitles_are_not_burned_in() {
        let (mut p, ids) = sample_project();
        ve_core::edit::set_disabled(&mut p, &[ids.s1], true).unwrap();
        assert!(!has_subtitles(&p));
        assert!(!write_ass(&p).contains("你好"));
    }

    #[test]
    fn animated_subtitles_are_split_into_intro_middle_and_outro() {
        let (mut p, ids) = sample_project();
        let m = ve_core::TextMotion {
            intro: ve_core::TextIn::SlideUp,
            outro: ve_core::TextOut::ZoomOut,
        };
        ve_core::edit::set_text_motion(&mut p, &[ids.s1], m).unwrap();
        let ass = write_ass(&p);
        // s1 在 1–3 秒：入场 1.0–1.5，中间 1.5–2.5，出场 2.5–3.0
        // 1080 高：底边离 1080 − 边距；滑动 54 像素
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "边距已取整"
        )]
        let y = 1080 - p.subtitle_style.margin_px(1080) as u32;
        assert!(
            ass.contains(&format!(
                "Dialogue: 0,0:00:01.00,0:00:01.50,Default,,0,0,0,,{{\\an2\\move(960,{},960,{y})\\fad(500,0)}}你好\n",
                y + 54
            )),
            "{ass}"
        );
        assert!(
            ass.contains("Dialogue: 0,0:00:01.50,0:00:02.50,Default,,0,0,0,,你好\n"),
            "{ass}"
        );
        assert!(
            ass.contains(
                "Dialogue: 0,0:00:02.50,0:00:03.00,Default,,0,0,0,,{\\t(\\fscx50\\fscy50)}你好\n"
            ),
            "{ass}"
        );
        // 没动画的字幕还是一行
        let (plain, _) = sample_project();
        assert_eq!(write_ass(&plain).matches("Dialogue:").count(), 1);
    }

    #[test]
    fn ass_escapes_special_chars() {
        let (mut p, ids) = sample_project();
        set_subtitle_text(&mut p, ids.s1, "a{b}c\\d,e\n第二行".into()).unwrap();
        let ass = write_ass(&p);
        assert!(ass.contains(",,a\\{b\\}c＼d,e\\N第二行\n"), "{ass}");
    }

    #[test]
    fn dialogues_sorted_and_centiseconds_rounded_down() {
        let (mut p, _) = sample_project();
        add_subtitle(
            &mut p,
            Time::from_millis(3_723_456),
            Time::from_millis(1000),
            "late".into(),
        )
        .unwrap();
        let ass = write_ass(&p);
        let late = ass.find("late").unwrap();
        let early = ass.find("你好").unwrap();
        assert!(early < late);
        assert!(ass.contains("1:02:03.45,1:02:04.45"), "{ass}");
        assert!(has_subtitles(&p));
    }
}
