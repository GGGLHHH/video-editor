//! SRT 字幕文件：解析、生成、导入工程、从工程导出。

use crate::edit::{EditError, add_subtitle};
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
        match add_subtitle(&mut draft, start, cue.end - start, cue.text.clone()) {
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
    let mut cues: Vec<Cue> = project
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Subtitle && t.shows())
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
}
