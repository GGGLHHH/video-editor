//! SRT 字幕文件：解析、生成、导入工程、从工程导出；也能读 LRC 歌词、ASS 字幕的对白。

use crate::edit::{EditError, add_caption};
use crate::{ClipId, Project, Time, TrackKind};
use std::fmt::Write as _;
use thiserror::Error;

/// 一条字幕。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    /// 开始。
    pub start: Time,
    /// 结束。
    pub end: Time,
    /// 文字（多行用换行符连接）。
    pub text: String,
}

/// SRT 解析错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("SRT 第 {line} 行：{message}")]
pub struct SrtError {
    /// 行号（从 1 开始）。
    pub line: usize,
    /// 说明。
    pub message: &'static str,
}

/// 解析 `HH:MM:SS,mmm`（毫秒分隔符也接受 `.`）。
fn parse_timestamp(s: &str) -> Option<Time> {
    let (hms, millis) = s.trim().split_once([',', '.'])?;
    let mut parts = hms.split(':');
    let mut field = || parts.next().and_then(|p| p.trim().parse::<i64>().ok());
    let (h, m, sec) = (field()?, field()?, field()?);
    let millis: i64 = millis.trim().parse().ok()?;
    if h < 0 || !(0..60).contains(&m) || !(0..60).contains(&sec) || !(0..1000).contains(&millis) {
        return None;
    }
    Some(Time::from_millis(((h * 60 + m) * 60 + sec) * 1000 + millis))
}

fn parse_timing(line: &str) -> Option<(Time, Time)> {
    let (a, b) = line.split_once("-->")?;
    // 结束时间后面可能跟位置信息（X1:.. Y1:..）
    let b = b.split_whitespace().next()?;
    Some((parse_timestamp(a)?, parse_timestamp(b)?))
}

/// 解析 SRT 文本。容忍 BOM、CRLF、缺序号行、多余空行。
///
/// # Errors
/// 时间行缺失或格式错误、结束早于开始。
pub fn parse_srt(src: &str) -> Result<Vec<Cue>, SrtError> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let lines: Vec<&str> = src.lines().collect();
    let mut cues = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim().is_empty() {
            i += 1;
            continue;
        }
        // 序号行可选
        if !lines[i].contains("-->") && lines[i].trim().chars().all(|c| c.is_ascii_digit()) {
            i += 1;
        }
        let line_no = i + 1;
        let err = |message| SrtError {
            line: line_no,
            message,
        };
        let timing = lines.get(i).ok_or_else(|| err("缺少时间行"))?;
        if !timing.contains("-->") {
            return Err(err("缺少时间行"));
        }
        let (start, end) = parse_timing(timing).ok_or_else(|| err("时间格式错误"))?;
        if end < start {
            return Err(err("结束时间早于开始时间"));
        }
        i += 1;
        let mut text = Vec::new();
        while i < lines.len() && !lines[i].trim().is_empty() {
            text.push(lines[i]);
            i += 1;
        }
        cues.push(Cue {
            start,
            end,
            text: text.join("\n"),
        });
    }
    Ok(cues)
}

fn write_timestamp(out: &mut String, t: Time) {
    let total = t.as_millis().max(0);
    let _ = write!(
        out,
        "{:02}:{:02}:{:02},{:03}",
        total / 3_600_000,
        total / 60_000 % 60,
        total / 1000 % 60,
        total % 1000
    );
}

/// ASS 的时间 `H:MM:SS.cc`。
fn ass_time(s: &str) -> Option<Time> {
    let mut parts = s.trim().split(':');
    let h: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let (sec, cs) = parts.next()?.split_once('.')?;
    let sec: i64 = sec.parse().ok()?;
    let ms: i64 = format!("{cs:0<3}")[..3].parse().ok()?;
    (parts.next().is_none() && (0..60).contains(&m) && (0..60).contains(&sec))
        .then(|| Time::from_millis(((h * 60 + m) * 60 + sec) * 1000 + ms))
}

/// ASS 文字：去掉 `{\...}` 样式标签，`\N` / `\n` 换行，`\h` 空格。
fn ass_text(raw: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in raw.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.replace("\\N", "\n")
        .replace("\\n", "\n")
        .replace("\\h", " ")
        .trim()
        .to_owned()
}

/// 解析 ASS / SSA 字幕：按 `[Events]` 的 `Format` 找开始、结束、文字三列，读 `Dialogue` 行
/// （`Comment` 行跳过）；样式、位置、特效都不要，只留文字。
///
/// # Errors
/// 一条能用的对白都没有。
pub fn parse_ass(src: &str) -> Result<Vec<Cue>, SrtError> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    // 没写 Format 时按标准的十列
    let mut columns: Vec<String> = "Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text"
        .split(',')
        .map(str::to_owned)
        .collect();
    let mut cues = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        if let Some(f) = line.strip_prefix("Format:") {
            columns = f.split(',').map(|c| c.trim().to_owned()).collect();
            continue;
        }
        let Some(body) = line.strip_prefix("Dialogue:") else {
            continue;
        };
        let col = |name: &str| columns.iter().position(|c| c.eq_ignore_ascii_case(name));
        let (Some(start_col), Some(end_col), Some(text_col)) =
            (col("Start"), col("End"), col("Text"))
        else {
            continue;
        };
        // 文字是最后一列，里面可以有逗号
        let fields: Vec<&str> = body.splitn(columns.len(), ',').collect();
        let (Some(a), Some(b), Some(t)) = (
            fields.get(start_col).and_then(|v| ass_time(v)),
            fields.get(end_col).and_then(|v| ass_time(v)),
            fields.get(text_col),
        ) else {
            continue;
        };
        let text = ass_text(t);
        if b > a && !text.is_empty() {
            cues.push(Cue {
                start: a,
                end: b,
                text,
            });
        }
    }
    cues.sort_by_key(|c| c.start);
    if cues.is_empty() {
        return Err(SrtError {
            line: 1,
            message: "没有能用的对白（Dialogue）",
        });
    }
    Ok(cues)
}

/// 贴的文稿里一个字念多久。
const SCRIPT_PER_CHAR: Time = Time::from_millis(220);
/// 一句最短、最长放多久。
const SCRIPT_RANGE: (Time, Time) = (Time::from_secs(1), Time::from_secs(6));

/// 贴一段文稿按句拆成字幕（剪映「文稿匹配」的手动版）：按句号、问号、感叹号、分号、省略号和换行断句，
/// 句末标点不要；从 `start` 起一句接一句，每句按字数估长度（每字 0.22 秒，夹在 1–6 秒）。
#[must_use]
pub fn script_cues(text: &str, start: Time) -> Vec<Cue> {
    const ENDS: [char; 10] = ['。', '！', '？', '；', '!', '?', ';', '…', '\n', '\r'];
    let mut at = start;
    text.split(ENDS)
        .map(|s| {
            s.trim()
                .trim_end_matches(['，', ',', '、', '：', ':'])
                .trim()
        })
        .filter(|s| !s.is_empty())
        .map(|sentence| {
            let chars = i64::try_from(sentence.chars().count()).unwrap_or(i64::MAX);
            let len =
                Time(SCRIPT_PER_CHAR.0.saturating_mul(chars)).clamp(SCRIPT_RANGE.0, SCRIPT_RANGE.1);
            let cue = Cue {
                start: at,
                end: at + len,
                text: sentence.to_owned(),
            };
            at += len;
            cue
        })
        .collect()
}

/// 一条歌词最后没有下一句时放多久。
const LRC_LAST: Time = Time::from_secs(3);

/// `[mm:ss.xx]` 里的时间（百分秒或毫秒都行）；`[ti:…]` 这类标签返回 `None`。
fn lrc_time(tag: &str) -> Option<Time> {
    let (m, rest) = tag.split_once(':')?;
    let m: i64 = m.trim().parse().ok()?;
    let (sec, frac) = rest.split_once('.').unwrap_or((rest, "0"));
    let sec: i64 = sec.trim().parse().ok()?;
    let frac = frac.trim();
    if frac.is_empty() || !frac.chars().all(|c| c.is_ascii_digit()) || !(0..60).contains(&sec) {
        return None;
    }
    // 两位是百分秒、三位是毫秒
    let ms: i64 = format!("{frac:0<3}")[..3].parse().ok()?;
    Some(Time::from_millis((m * 60 + sec) * 1000 + ms))
}

/// 解析 LRC 歌词：每句从它的时间开始、到下一句开始结束（最后一句放 3 秒）；
/// 一行几个时间就出现几次；空歌词只当分隔（上一句到这里结束）。
///
/// # Errors
/// 一句带时间的歌词都没有。
pub fn parse_lrc(src: &str) -> Result<Vec<Cue>, SrtError> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut stamps: Vec<(Time, String)> = Vec::new();
    for line in src.lines() {
        let mut rest = line.trim();
        let mut times = Vec::new();
        while let Some(body) = rest.strip_prefix('[') {
            let Some((tag, after)) = body.split_once(']') else {
                break;
            };
            match lrc_time(tag) {
                Some(t) => times.push(t),
                None if times.is_empty() => {}
                None => break,
            }
            rest = after;
        }
        let text = rest.trim().to_owned();
        stamps.extend(times.into_iter().map(|t| (t, text.clone())));
    }
    stamps.sort_by_key(|(t, _)| *t);
    let cues: Vec<Cue> = stamps
        .iter()
        .enumerate()
        .filter(|(_, (_, text))| !text.is_empty())
        .map(|(i, (start, text))| Cue {
            start: *start,
            end: stamps.get(i + 1).map_or(*start + LRC_LAST, |(t, _)| *t),
            text: text.clone(),
        })
        .filter(|c| c.end > c.start)
        .collect();
    if cues.is_empty() {
        return Err(SrtError {
            line: 1,
            message: "没有带时间的歌词",
        });
    }
    Ok(cues)
}

/// 生成 SRT 文本（`\n` 换行，每条后空一行）。
#[must_use]
pub fn write_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (i, cue) in cues.iter().enumerate() {
        let _ = writeln!(out, "{}", i + 1);
        write_timestamp(&mut out, cue.start);
        out.push_str(" --> ");
        write_timestamp(&mut out, cue.end);
        let _ = write!(out, "\n{}\n\n", cue.text);
    }
    out
}

/// 导入结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrtImport {
    /// 新加的字幕片段。
    pub added: Vec<ClipId>,
    /// 跳过的条数（和已有字幕重叠，或长度为零）。
    pub skipped: usize,
}

/// 导入字幕到字幕轨。按开始时间排序；和前一条导入的重叠时，起点推到前一条终点；
/// 和字幕轨上已有字幕重叠、或推完长度不为正的跳过，其余照常导入。
///
/// # Errors
/// 工程里没有字幕轨。
pub fn import_srt(project: &mut Project, cues: &[Cue]) -> Result<SrtImport, EditError> {
    let mut sorted: Vec<&Cue> = cues.iter().collect();
    sorted.sort_by_key(|c| c.start);
    let mut draft = project.clone();
    let mut report = SrtImport {
        added: Vec::new(),
        skipped: 0,
    };
    let mut prev_end = Time(i64::MIN);
    for cue in sorted {
        let start = cue.start.max(prev_end);
        if cue.end <= start {
            report.skipped += 1;
            continue;
        }
        match add_caption(&mut draft, start, cue.end - start, cue.text.clone()) {
            Ok(id) => {
                report.added.push(id);
                prev_end = cue.end;
            }
            Err(EditError::Overlap | EditError::ZeroLength) => report.skipped += 1,
            Err(e) => return Err(e),
        }
    }
    *project = draft;
    Ok(report)
}

/// 导出字幕轨上的所有字幕（按时间排序）。
#[must_use]
pub fn export_srt(project: &Project) -> Vec<Cue> {
    // 只导字幕轨（最下面那条文字轨）：上面叠的是标题，和字幕列表一样不算字幕
    let mut cues: Vec<Cue> = project
        .timeline
        .tracks
        .iter()
        .rfind(|t| t.kind == TrackKind::Subtitle)
        .filter(|t| t.shows())
        .into_iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.enabled())
        .filter_map(|c| {
            c.subtitle_text().map(|text| Cue {
                start: c.start,
                end: c.end(),
                text: text.to_owned(),
            })
        })
        .collect();
    cues.sort_by_key(|c| c.start);
    cues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_script_splits_into_back_to_back_sentences() {
        let cues = script_cues(
            "你好世界。今天天气不错！\n\n  第三句话没有标点\n短",
            Time::from_secs(10),
        );
        let texts: Vec<&str> = cues.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(
            texts,
            ["你好世界", "今天天气不错", "第三句话没有标点", "短"]
        );
        assert_eq!(cues[0].start, Time::from_secs(10));
        // 一句接一句，长度按字数（每字 0.22 秒，最短 1 秒）
        for w in cues.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        assert_eq!(
            cues[0].end - cues[0].start,
            Time::from_millis(1000),
            "4 个字不到 1 秒按 1 秒"
        );
        assert_eq!(
            cues[2].end - cues[2].start,
            Time::from_millis(1760),
            "8 个字"
        );
        assert_eq!(script_cues(" \n。！", Time::ZERO), []);
    }

    #[test]
    fn ass_dialogue_lines_become_cues_without_style_tags() {
        let src = "[Script Info]\nTitle: x\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:02.50,Default,,0,0,0,,{\\b1}你好，{\\i1}世界{\\i0}\nComment: 0,0:00:03.00,0:00:04.00,Default,,0,0,0,,注释不要\nDialogue: 0,0:00:05.20,0:00:06.00,Default,,0,0,0,,第一行\\N第二行, 带逗号\n";
        let cues = parse_ass(src).unwrap();
        assert_eq!(
            cues,
            [
                Cue {
                    start: Time::from_secs(1),
                    end: Time::from_millis(2500),
                    text: "你好，世界".into()
                },
                Cue {
                    start: Time::from_millis(5200),
                    end: Time::from_secs(6),
                    text: "第一行\n第二行, 带逗号".into()
                },
            ]
        );
        assert!(parse_ass("[Events]\n").is_err());
    }

    #[test]
    fn lrc_lines_become_cues_that_end_where_the_next_starts() {
        let src = "\u{feff}[ti:歌名]\n[ar:歌手]\n[00:01.00]第一句\n[00:03.50]第二句\n\n[00:05.250]最后一句\n[01:00.00]\n";
        let cues = parse_lrc(src).unwrap();
        assert_eq!(
            cues,
            [
                Cue {
                    start: Time::from_secs(1),
                    end: Time::from_millis(3500),
                    text: "第一句".into()
                },
                Cue {
                    start: Time::from_millis(3500),
                    end: Time::from_millis(5250),
                    text: "第二句".into()
                },
                Cue {
                    start: Time::from_millis(5250),
                    end: Time::from_secs(60),
                    text: "最后一句".into()
                },
            ]
        );
        // 一行几个时间：同一句出现几次；最后一句没有下一句时放 3 秒
        let repeat = parse_lrc("[00:10.00][00:02.00]副歌").unwrap();
        assert_eq!(repeat[0].start, Time::from_secs(2));
        assert_eq!(repeat[1].end, Time::from_secs(13));
        assert!(parse_lrc("没有时间").is_err());
    }
    use crate::testing::sample_project;
    use proptest::prelude::*;

    fn ms(v: i64) -> Time {
        Time::from_millis(v)
    }

    #[test]
    fn parses_crlf_bom() {
        let src = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,500\r\n你好\r\n\r\n2\r\n00:00:03.000 --> 00:00:04,000\r\nworld\r\n";
        let cues = parse_srt(src).unwrap();
        assert_eq!(
            cues,
            vec![
                Cue {
                    start: ms(1000),
                    end: ms(2500),
                    text: "你好".into()
                },
                Cue {
                    start: ms(3000),
                    end: ms(4000),
                    text: "world".into()
                },
            ]
        );
    }

    #[test]
    fn multi_line_text_and_extra_blank_lines() {
        let src = "\n\n1\n00:00:00,000 --> 01:00:00,001 X1:0 Y1:0\n第一行\n第二行\n\n\n\n2\n00:00:05,000 --> 00:00:06,000\nb\n";
        let cues = parse_srt(src).unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "第一行\n第二行");
        assert_eq!(cues[0].end, ms(3_600_001));
    }

    #[test]
    fn missing_index_line_is_tolerated() {
        let cues = parse_srt("00:00:01,000 --> 00:00:02,000\nhi\n").unwrap();
        assert_eq!(cues[0].text, "hi");
    }

    #[test]
    fn bad_timestamp_reports_line() {
        let line_of = |src: &str| parse_srt(src).unwrap_err().line;
        assert_eq!(
            line_of("1\n00:00:01,000 --> 00:00:02,000\na\n\n2\n00:00:xx,000 --> 00:00:03,000\nb\n"),
            6
        );
        assert_eq!(line_of("1\nhello\n"), 2);
        assert_eq!(line_of("1\n00:00:02,000 --> 00:00:01,000\nx\n"), 2);
        assert_eq!(line_of("1\n"), 2);
    }

    #[test]
    fn write_format() {
        let out = write_srt(&[Cue {
            start: ms(3_723_004),
            end: ms(3_724_000),
            text: "a\nb".into(),
        }]);
        assert_eq!(out, "1\n01:02:03,004 --> 01:02:04,000\na\nb\n\n");
    }

    #[test]
    fn import_pushes_overlaps_and_export_reads_back() {
        let (mut p, ids) = sample_project();
        crate::edit::delete_clips(&mut p, &[ids.s1]).unwrap();
        let cues = vec![
            Cue {
                start: ms(1000),
                end: ms(3000),
                text: "a".into(),
            },
            Cue {
                start: ms(2000),
                end: ms(4000),
                text: "b".into(),
            },
            Cue {
                start: ms(5000),
                end: ms(5000),
                text: "空".into(),
            },
        ];
        let report = import_srt(&mut p, &cues).unwrap();
        assert_eq!((report.added.len(), report.skipped), (2, 1));
        let out = export_srt(&p);
        assert_eq!(
            out,
            vec![
                Cue {
                    start: ms(1000),
                    end: ms(3000),
                    text: "a".into()
                },
                Cue {
                    start: ms(3000),
                    end: ms(4000),
                    text: "b".into()
                },
            ]
        );
        p.timeline.check_invariants(&p.assets).unwrap();
    }

    #[test]
    fn import_skips_cues_overlapping_existing_subtitles() {
        // 和字幕轨上已有字幕重叠的那一条跳过，其余照常导入（以前是整次失败）
        let (mut p, _) = sample_project();
        let cues = vec![
            Cue {
                start: ms(5000),
                end: ms(6000),
                text: "ok".into(),
            },
            Cue {
                start: ms(1500),
                end: ms(2000),
                text: "撞上已有字幕".into(),
            },
            Cue {
                start: ms(7000),
                end: ms(8000),
                text: "ok2".into(),
            },
        ];
        let report = import_srt(&mut p, &cues).unwrap();
        assert_eq!((report.added.len(), report.skipped), (2, 1));
        let texts: Vec<String> = export_srt(&p).into_iter().map(|c| c.text).collect();
        assert_eq!(texts, vec!["你好", "ok", "ok2"]);
        p.timeline.check_invariants(&p.assets).unwrap();
    }

    fn cue_strategy() -> impl Strategy<Value = Cue> {
        (
            0i64..10_000_000,
            1i64..100_000,
            "[a-z0-9中文]{1,8}(\n[a-z]{1,6})?",
        )
            .prop_map(|(start, len, text)| Cue {
                start: ms(start),
                end: ms(start + len),
                text,
            })
    }

    proptest! {
        #[test]
        fn write_parse_round_trip(cues in proptest::collection::vec(cue_strategy(), 0..20)) {
            let parsed = parse_srt(&write_srt(&cues)).unwrap();
            prop_assert_eq!(parsed, cues);
        }
    }

    #[test]
    fn srt_export_takes_only_the_caption_track_not_titles() {
        let (mut p, _) = sample_project();
        crate::edit::add_subtitle(
            &mut p,
            Time::from_secs(1),
            Time::from_secs(1),
            "标题".into(),
        )
        .unwrap();
        let cues = export_srt(&p);
        assert_eq!(cues.len(), 1, "{cues:?}");
        assert_eq!(cues[0].text, "你好");
    }
}
