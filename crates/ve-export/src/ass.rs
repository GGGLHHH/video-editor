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
        .any(|t| t.kind == TrackKind::Subtitle && t.clips.iter().any(|c| c.enabled()))
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
        .filter(|t| t.kind == TrackKind::Subtitle)
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.enabled())
        .filter_map(|c| c.subtitle_text().map(|text| (c.start, c.end(), text)))
        .collect();
    clips.sort_by_key(|c| c.0);
    for (start, end, text) in clips {
        out.push_str("Dialogue: 0,");
        ass_time(&mut out, start);
        out.push(',');
        ass_time(&mut out, end);
        let _ = writeln!(out, ",Default,,0,0,0,,{}", escape(text));
    }
    out
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
    fn disabled_subtitles_are_not_burned_in() {
        let (mut p, ids) = sample_project();
        ve_core::edit::set_disabled(&mut p, &[ids.s1], true).unwrap();
        assert!(!has_subtitles(&p));
        assert!(!write_ass(&p).contains("你好"));
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
