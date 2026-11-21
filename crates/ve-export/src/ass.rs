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

/// 一行 ASS 样式：`Default`（字有描边）或 `Box`（字底下一块背景框，ASS 的 `BorderStyle=3`：
/// 框用描边色、框比字多出的边用描边宽度；这时字本身不描边）。
fn style_line(name: &str, style: &SubtitleStyle, h: u32, boxed: bool) -> String {
    let flag = |on: bool| if on { -1 } else { 0 };
    let (edge, border, width) = if boxed {
        (
            SubtitleStyle::ass_color_alpha(style.background_color, style.background_opacity),
            3,
            style.padding_px(h),
        )
    } else {
        (
            SubtitleStyle::ass_color_alpha(style.outline_color, style.opacity),
            1,
            style.outline_px(h),
        )
    };
    format!(
        "Style: {name},{},{},{},&H000000FF,{edge},{},{},{},{},0,100,100,{:.1},0,{border},{width:.1},{:.1},2,20,20,{},1\n",
        style.font,
        style.font_px(h),
        SubtitleStyle::ass_color_alpha(style.color, style.opacity),
        SubtitleStyle::ass_color_alpha(style.shadow_color, style.shadow_opacity),
        flag(style.bold),
        flag(style.italic),
        flag(style.underline),
        style.spacing_px(h),
        style.shadow_px(h),
        style.margin_px(h),
    )
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
         Alignment, MarginL, MarginR, MarginV, Encoding\n{}{}\n\
         [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
        style_line("Default", style, h, false),
        style_line("Box", style, h, true),
    );
    // 文字轨从下往上数的序号当 ASS 的 Layer：上面的文字轨盖在下面的上面（和预览一样）
    let mut clips: Vec<(&ve_core::Clip, usize)> = project
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Subtitle)
        .rev()
        .enumerate()
        .filter(|(_, t)| t.shows())
        .flat_map(|(layer, t)| t.clips.iter().map(move |c| (c.as_ref(), layer)))
        .filter(|(c, _)| c.enabled() && c.subtitle_text().is_some())
        .collect();
    clips.sort_by_key(|(c, _)| c.start);
    for (clip, layer) in clips {
        write_clip(&mut out, project, clip, layer);
    }
    out
}

/// 一条字幕最多切成这么多段。
const MAX_SLICES: usize = 400;

/// 字幕要切开的时刻（相对片段起点）：入场 / 出场的分界、逐字入场结束、关键帧，
/// 循环动画和有缓动的关键帧再细分（每段里都按线性过渡，细分后和预览的曲线贴得很近）。
fn slice_points(clip: &ve_core::Clip) -> Vec<Time> {
    use ve_core::text_motion::{CHAR_FADE, char_len, motion_len};
    let d = clip.duration;
    let motion = clip.text_motion().unwrap_or_default();
    let mut pts = vec![Time::ZERO, d];
    let len = motion_len(d);
    if motion.intro != ve_core::TextIn::None && !motion.intro.per_char() {
        pts.push(len);
    }
    if motion.intro.per_char() {
        let n = clip.subtitle_text().map_or(1, |t| t.chars().count());
        pts.push(char_len(d, n) + CHAR_FADE);
    }
    if motion.outro != ve_core::TextOut::None {
        pts.push(d - len);
    }
    let keys = clip.animation.times_of(&ve_core::Prop::TRANSFORM);
    pts.extend(keys.iter().copied());
    // 有缓动的关键帧段：每段再分 6 份
    for w in keys.windows(2) {
        if clip.animation.easing_of(&ve_core::Prop::TRANSFORM, w[0])
            != Some(ve_core::Easing::Linear)
        {
            let step = Time((w[1] - w[0]).0 / 6);
            pts.extend((1..6).map(|k| w[0] + Time(step.0 * k)));
        }
    }
    // 循环动画：每个周期分 8 段
    if motion.looping != ve_core::TextLoop::None {
        let step = Time(motion.looping.period().0 / 8).max(Time::from_millis(20));
        let mut t = step;
        while t < d && pts.len() < MAX_SLICES {
            pts.push(t);
            t += step;
        }
    }
    pts.retain(|t| *t >= Time::ZERO && *t <= d);
    pts.sort_unstable();
    pts.dedup();
    pts
}

/// 这一条自己的样式（和工程的不一样时）写成行内标签。
fn style_tags(own: &SubtitleStyle, h: u32) -> String {
    let (edge, edge_alpha, width) = if own.has_background() {
        (
            own.background_color,
            own.background_opacity,
            own.padding_px(h),
        )
    } else {
        (own.outline_color, own.opacity, own.outline_px(h))
    };
    format!(
        "\\fn{}\\fs{}\\c{}\\3c{}\\4c{}\\1a{}\\3a{}\\4a{}\\bord{width:.1}\\shad{:.1}\\fsp{:.1}\\b{}\\i{}\\u{}",
        own.font,
        own.font_px(h),
        SubtitleStyle::ass_color(own.color),
        SubtitleStyle::ass_color(edge),
        SubtitleStyle::ass_color(own.shadow_color),
        alpha(own.opacity),
        alpha(edge_alpha),
        alpha(own.shadow_opacity),
        own.shadow_px(h),
        own.spacing_px(h),
        u8::from(own.bold),
        u8::from(own.italic),
        u8::from(own.underline),
    )
}

/// ASS 单独一个透明度值：`&HAA&`。
fn alpha(opacity: f32) -> String {
    format!("&H{:02X}&", SubtitleStyle::ass_alpha_byte(opacity))
}

/// 入场 / 出场动画在这一刻的不透明度 `a`：字、描边（或背景框）、阴影各自乘上去（ASS 的 `\1a` `\3a` `\4a`）。
fn fade_tags(style: &SubtitleStyle, a: f32) -> String {
    let edge = if style.has_background() {
        style.background_opacity
    } else {
        style.opacity
    };
    format!(
        "\\1a{}\\3a{}\\4a{}",
        alpha(a * style.opacity),
        alpha(a * edge),
        alpha(a * style.shadow_opacity)
    )
}

/// 逐字入场：每个字单独的标签（时间相对这一行开头 `line_start`）。
fn per_char_text(
    clip: &ve_core::Clip,
    text: &str,
    line_start: Time,
    line_end: Time,
    style: &SubtitleStyle,
    line_alpha: f32,
    final_scale: f32,
) -> String {
    use ve_core::text_motion::{CHAR_FADE, ZOOM_FROM, char_len, char_step};
    let motion = clip.text_motion().unwrap_or_default();
    let n = text.chars().filter(|c| *c != '\n').count();
    let step = char_step(char_len(clip.duration, n), n);
    let ms = |t: Time| t.as_millis();
    let full = (final_scale * 100.0).round();
    let mut out = String::new();
    let mut i: i64 = 0;
    for ch in text.chars() {
        if ch == '\n' {
            out.push_str("\\N");
            continue;
        }
        let begin = Time(step.0 * i);
        i += 1;
        let done = match motion.intro {
            ve_core::TextIn::Typewriter => begin,
            _ => begin + CHAR_FADE,
        };
        let esc = escape(&ch.to_string());
        if done <= line_start || begin >= line_end + CHAR_FADE {
            out.push_str(&esc);
            continue;
        }
        let (a0, s0) = ve_core::text_motion::char_reveal(
            motion.intro,
            usize::try_from(i - 1).unwrap_or(0),
            n,
            line_start,
            clip.duration,
        );
        let t0 = ms(begin.max(line_start) - line_start);
        let t1 = ms(done - line_start).max(t0 + 1);
        // 字、描边（框）、阴影各自乘上自己的不透明度（\alpha 会把四个通道一起改掉，框和阴影就不对了）
        let start_alpha = fade_tags(style, a0 * line_alpha);
        let final_alpha = fade_tags(style, line_alpha);
        let _ = match motion.intro {
            ve_core::TextIn::CharZoom => {
                let s = (s0.max(ZOOM_FROM) * final_scale * 100.0).round();
                write!(
                    out,
                    "{{{start_alpha}\\fscx{s}\\fscy{s}\\t({t0},{t1},{final_alpha}\\fscx{full}\\fscy{full})}}{esc}"
                )
            }
            _ => write!(out, "{{{start_alpha}\\t({t0},{t1},{final_alpha})}}{esc}"),
        };
    }
    out
}

/// 一条字幕：按 [`slice_points`] 切开，每段从这一段开头的样子线性过渡到结尾的样子
/// （位置 `\move`、缩放 `\fscx`、不透明度 `\alpha`，和预览按同一个公式算的 [`ve_core::text_state`]）。
fn write_clip(out: &mut String, project: &Project, clip: &ve_core::Clip, layer: usize) {
    let Some(text) = clip.subtitle_text() else {
        return;
    };
    let (w, h) = (project.settings.width, project.settings.height);
    let own = clip.own_subtitle_style();
    let style = own.unwrap_or(&project.subtitle_style);
    let own_tags = own.map(|s| style_tags(s, h)).unwrap_or_default();
    let base_y = f64::from(h) - style.margin_px(h);
    let pos = |s: &ve_core::TextState| {
        (
            (f64::from(w) * (0.5 + f64::from(s.x))).round(),
            f64::from(s.y).mul_add(f64::from(h), base_y).round(),
        )
    };
    let per_char = clip.text_motion().is_some_and(|m| m.intro.per_char());
    let pts = slice_points(clip);
    for win in pts.windows(2) {
        let (a, b) = (win[0], win[1]);
        if b <= a {
            continue;
        }
        let s0 = ve_core::text_state(clip, a);
        // 段尾取结束前一点点的样子（出场到了最后一刻是全透明的，那一刻已经不显示了）
        let s1 = ve_core::text_state(clip, b - Time(1).min(Time((b - a).0 / 2)));
        let (x0, y0) = pos(&s0);
        let (x1, y1) = pos(&s1);
        let mut tags = format!("{{\\an2{own_tags}");
        if (x0 - x1).abs() < 0.5 && (y0 - y1).abs() < 0.5 {
            let _ = write!(tags, "\\pos({x0},{y0})");
        } else {
            let _ = write!(tags, "\\move({x0},{y0},{x1},{y1})");
        }
        let (sc0, sc1) = ((s0.scale * 100.0).round(), (s1.scale * 100.0).round());
        if (sc0 - 100.0).abs() > 0.1 || (sc1 - 100.0).abs() > 0.1 {
            let _ = write!(tags, "\\fscx{sc0}\\fscy{sc0}");
        }
        let fading = |opacity: f32| (opacity - 1.0).abs() > 1e-4;
        // 逐字入场只管每个字什么时候出来；整条的淡入淡出（出场、关键帧不透明度）照样写在行上
        if fading(s0.alpha) || fading(s1.alpha) {
            tags.push_str(&fade_tags(style, s0.alpha));
        }
        let mut change = String::new();
        if (sc0 - sc1).abs() > 0.1 {
            let _ = write!(change, "\\fscx{sc1}\\fscy{sc1}");
        }
        if (s0.alpha - s1.alpha).abs() > 1e-4 {
            change.push_str(&fade_tags(style, s1.alpha));
        }
        if !change.is_empty() {
            let _ = write!(tags, "\\t({change})");
        }
        tags.push('}');
        let body = if per_char {
            per_char_text(clip, text, a, b, style, s1.alpha, s1.scale)
        } else {
            escape(text)
        };
        let _ = write!(out, "Dialogue: {layer},");
        ass_time(out, clip.start + a);
        out.push(',');
        ass_time(out, clip.start + b);
        let name = if style.has_background() {
            "Box"
        } else {
            "Default"
        };
        let _ = writeln!(out, ",{name},,0,0,0,,{tags}{body}");
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
            ass.contains(&format!(
                "Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,{{\\an2\\pos(960,{})}}你好\n",
                (1080.0 - p.subtitle_style.margin_px(1080)).round()
            )),
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
                "Style: Default,SimHei,59,&H0000D7FF,&H000000FF,&H00000000,&H66000000,-1,0,0,"
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
    fn new_text_styles_reach_the_ass_file() {
        let (mut p, ids) = sample_project();
        let s = &mut p.subtitle_style;
        s.italic = true;
        s.underline = true;
        s.spacing = 10.0;
        s.opacity = 0.5;
        s.shadow = 4.0;
        s.shadow_color = 0xFF_0000;
        let ass = write_ass(&p);
        // 半透明的白字（AA=80）、红阴影、斜体、下划线、字间距 10、阴影 4
        assert!(
            ass.contains("Style: Default,Microsoft YaHei,59,&H80FFFFFF,&H000000FF,&H80000000,&H660000FF,0,-1,-1,0,100,100,10.0,0,1,2.5,4.0,2,"),
            "{ass}"
        );
        // 背景框：用 Box 样式（BorderStyle=3，框是描边色）
        p.subtitle_style.background_opacity = 0.8;
        p.subtitle_style.background_color = 0x33_3333;
        let boxed = write_ass(&p);
        assert!(boxed.contains("Style: Box,"), "{boxed}");
        assert!(boxed.contains(",&H33333333,&H660000FF,"), "{boxed}");
        assert!(dialogues(&boxed)[0].contains(",Box,,"), "{boxed}");
        // 单独一条自己的样式：用覆盖标签写全
        let own = SubtitleStyle {
            italic: true,
            shadow: 2.0,
            ..SubtitleStyle::default()
        };
        ve_core::edit::set_own_subtitle_style(&mut p, &[ids.s1], Some(&own)).unwrap();
        let with_own = write_ass(&p);
        let line = dialogues(&with_own)[0];
        assert!(
            line.contains("\\i1") && line.contains("\\shad2.0") && line.contains(",Default,,"),
            "{line}"
        );
    }

    /// 所有 Dialogue 行（不含头部）。
    fn dialogues(ass: &str) -> Vec<&str> {
        ass.lines().filter(|l| l.starts_with("Dialogue:")).collect()
    }

    #[test]
    fn animated_subtitles_are_sliced_and_move_linearly() {
        let (mut p, ids) = sample_project();
        let m = ve_core::TextMotion {
            intro: ve_core::TextIn::SlideUp,
            outro: ve_core::TextOut::ZoomOut,
            ..ve_core::TextMotion::default()
        };
        ve_core::edit::set_text_motion(&mut p, &[ids.s1], m).unwrap();
        let ass = write_ass(&p);
        let lines = dialogues(&ass);
        // s1 在 1–3 秒：入场 1.0–1.5，中间 1.5–2.5，出场 2.5–3.0
        assert_eq!(lines.len(), 3, "{ass}");
        let y = (1080.0 - p.subtitle_style.margin_px(1080)).round();
        assert!(
            lines[0].starts_with("Dialogue: 0,0:00:01.00,0:00:01.50,"),
            "{ass}"
        );
        assert!(
            lines[0].contains(&format!("\\move(960,{},960,{y})", y + 54.0))
                // 字、描边、阴影分开淡入（阴影本身是六成不透明）
                && lines[0].contains("\\1a&HFF&\\3a&HFF&\\4a&HFF&\\t(\\1a&H00&\\3a&H00&\\4a&H66&)"),
            "入场：从下面 54 像素升上来、从透明到不透明：{}",
            lines[0]
        );
        assert!(
            lines[1].contains(&format!("\\pos(960,{y})")),
            "{}",
            lines[1]
        );
        assert!(
            lines[2].contains("\\t(\\fscx50\\fscy50)"),
            "出场缩到一半：{}",
            lines[2]
        );
        // 没动画的字幕还是一行
        let (plain, _) = sample_project();
        assert_eq!(dialogues(&write_ass(&plain)).len(), 1);
    }

    #[test]
    fn keyframed_position_and_own_style_reach_the_ass() {
        use ve_core::edit::{add_keyframe_props, apply_transform_at, set_own_subtitle_style};
        let (mut p, ids) = sample_project();
        add_keyframe_props(&mut p, ids.s1, Time::ZERO, &[ve_core::Prop::X]).unwrap();
        let right = ve_core::Transform {
            x: 0.25,
            ..ve_core::Transform::default()
        };
        apply_transform_at(&mut p, ids.s1, Time::from_secs(2), right).unwrap();
        ve_core::edit::set_keyframe_easing(
            &mut p,
            ids.s1,
            Time::ZERO,
            &[ve_core::Prop::X],
            ve_core::Easing::Linear,
        )
        .unwrap();
        let mut own = p.subtitle_style.clone();
        own.color = 0xFF_0000;
        own.size_ratio = 0.1;
        set_own_subtitle_style(&mut p, &[ids.s1], Some(&own)).unwrap();
        let ass = write_ass(&p);
        let lines = dialogues(&ass);
        assert_eq!(lines.len(), 1, "匀速关键帧：一段就够：{ass}");
        assert!(
            lines[0].contains("\\move(960,") && lines[0].contains(",1440,"),
            "从中间移到右边四分之一：{}",
            lines[0]
        );
        assert!(
            lines[0].contains(&format!(
                "\\fs{}\\c{}",
                own.font_px(1080),
                SubtitleStyle::ass_color(0xFF_0000)
            )),
            "自己的样式写成行内标签：{}",
            lines[0]
        );
    }

    #[test]
    fn per_char_intro_tags_each_character() {
        let (mut p, ids) = sample_project();
        let m = ve_core::TextMotion {
            intro: ve_core::TextIn::Typewriter,
            ..ve_core::TextMotion::default()
        };
        ve_core::edit::set_text_motion(&mut p, &[ids.s1], m).unwrap();
        let ass = write_ass(&p);
        let lines = dialogues(&ass);
        // 两个字：第二个字 120 毫秒后出现
        assert!(
            lines[0].contains("}你{"),
            "第一个字一开始就在：{}",
            lines[0]
        );
        assert!(
            lines[0].contains(
                "{\\1a&HFF&\\3a&HFF&\\4a&HFF&\\t(120,121,\\1a&H00&\\3a&H00&\\4a&H66&)}好"
            ),
            "{}",
            lines[0]
        );
        // 逐字入场结束后的那一段不带逐字标签
        assert!(lines.last().unwrap().ends_with("}你好"), "{ass}");
        // 循环动画：按周期切成很多段
        let looping = ve_core::TextMotion {
            looping: ve_core::TextLoop::Breathe,
            ..ve_core::TextMotion::default()
        };
        ve_core::edit::set_text_motion(&mut p, &[ids.s1], looping).unwrap();
        assert!(dialogues(&write_ass(&p)).len() > 10);
    }

    #[test]
    fn ass_escapes_special_chars() {
        let (mut p, ids) = sample_project();
        set_subtitle_text(&mut p, ids.s1, "a{b}c\\d,e\n第二行".into()).unwrap();
        let ass = write_ass(&p);
        assert!(ass.contains("}a\\{b\\}c＼d,e\\N第二行\n"), "{ass}");
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

    #[test]
    fn per_char_intros_keep_box_and_shadow_alpha_and_still_fade_out() {
        let (mut p, ids) = sample_project();
        p.subtitle_style.background_opacity = 0.5;
        p.subtitle_style.shadow = 4.0;
        let m = ve_core::TextMotion {
            intro: ve_core::TextIn::Typewriter,
            outro: ve_core::TextOut::FadeOut,
            ..ve_core::TextMotion::default()
        };
        ve_core::edit::set_text_motion(&mut p, &[ids.s1], m).unwrap();
        let ass = write_ass(&p);
        let lines = dialogues(&ass);
        // 逐字的标签只动字、框、阴影各自的透明度，不能用一把改四个通道的 \alpha
        assert!(lines.iter().all(|l| !l.contains("\\alpha")), "{ass}");
        assert!(lines[0].contains("\\1a&HFF&"), "{}", lines[0]);
        // 框（描边色）出来后是五成透明：&H7F / &H80，不是 &H00
        assert!(
            lines[0].contains("\\3a&H7F&") || lines[0].contains("\\3a&H80&"),
            "{}",
            lines[0]
        );
        // 出场淡出那一段照样淡出
        let last = lines.last().unwrap();
        assert!(
            last.contains("\\t(") && last.contains("\\1a&HFF&"),
            "{last}"
        );
    }

    #[test]
    fn upper_text_tracks_get_higher_ass_layers() {
        let (mut p, _) = sample_project();
        add_subtitle(
            &mut p,
            Time::from_secs(1),
            Time::from_secs(2),
            "标题".into(),
        )
        .unwrap();
        let ass = write_ass(&p);
        let lines = dialogues(&ass);
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("Dialogue: 0,") && l.ends_with("你好")),
            "{ass}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("Dialogue: 1,") && l.ends_with("标题")),
            "{ass}"
        );
    }
}
