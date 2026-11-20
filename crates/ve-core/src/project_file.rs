//! 工程文件（JSON）读写。
//!
//! 顶层 `{"format":"ve-project","version":7,...}`（第 1–6 版也能读）。素材路径同时存绝对路径和
//! 相对工程文件的路径：整个文件夹搬走后仍能找到素材。

use crate::{Asset, AssetId, IdGen, InvariantError, Project, ProjectSettings, Timeline};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::{fs, io};
use thiserror::Error;

const FORMAT: &str = "ve-project";
/// 当前写出的版本。第 2 版加了片段的 `transform`，第 3 版加了 `transform.rotation`，
/// 第 4 版加了片段的关键帧动画 `animation`，第 5 版加了调色 `adjust`、停用 `disabled`、时间轴标记 `markers` 和字幕样式 `subtitle_style`，
/// 第 6 版加了轨道的 `hidden` / `muted` / `locked`、画面属性的 `mirror`、调节片段 `Adjustment` 和字幕动画 `motion`，
/// 第 7 版加了调色 / 音量关键帧、片段颜色标签 `label`、入出点区域 `range`。
/// 升版本是为了让旧版程序拒绝读新文件，而不是悄悄丢掉它不认识的字段。
const VERSION: u32 = 7;
/// 能读的最旧版本（缺的字段取默认值）。
const OLDEST_READABLE: u32 = 1;

/// 工程文件错误。
#[derive(Debug, Error)]
pub enum ProjectFileError {
    /// 读写失败。
    #[error("读写 {path} 失败：{source}")]
    Io {
        /// 文件。
        path: PathBuf,
        /// 原因。
        source: io::Error,
    },
    /// JSON 解析失败。
    #[error("工程文件格式错误：{0}")]
    Json(#[from] serde_json::Error),
    /// 不是本程序的工程文件。
    #[error("不是视频剪辑器的工程文件")]
    WrongFormat,
    /// 版本不支持。
    #[error("不支持的工程文件版本 {0}")]
    UnsupportedVersion(u32),
    /// 设置无效（帧率、尺寸、采样率为 0）。
    #[error("工程设置无效（帧率或画面尺寸为 0）")]
    BadSettings,
    /// 内容违反模型不变式。
    #[error("工程内容无效：{0}")]
    Invalid(InvariantError),
}

/// 加载结果：工程，以及找不到文件的素材。
#[derive(Debug)]
pub struct LoadedProject {
    /// 工程。
    pub project: Project,
    /// 磁盘上找不到的素材（工程里保留原路径）。
    pub missing_assets: Vec<AssetId>,
}

#[derive(Deserialize)]
struct Header {
    format: String,
    version: u32,
}

#[derive(Serialize, Deserialize)]
struct FileAsset {
    #[serde(flatten)]
    asset: Asset,
    relative_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct FileV1 {
    format: String,
    version: u32,
    settings: ProjectSettings,
    ids: IdGen,
    assets: Vec<FileAsset>,
    timeline: Timeline,
    /// 第 5 版加的；默认样式不写。
    #[serde(default, skip_serializing_if = "crate::SubtitleStyle::is_default")]
    subtitle_style: crate::SubtitleStyle,
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ProjectFileError + '_ {
    move |source| ProjectFileError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// 保存工程（先写临时文件再改名，写一半崩溃不会损坏原文件）。
///
/// # Errors
/// 读写失败或序列化失败。
pub fn save_project(project: &Project, path: &Path) -> Result<(), ProjectFileError> {
    // 都换成绝对路径再算相对路径：素材路径若相对于当前目录，换个目录启动就找不到了
    let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let file_abs = absolute(path);
    let dir = file_abs.parent().unwrap_or_else(|| Path::new("."));
    let file = FileV1 {
        format: FORMAT.into(),
        version: VERSION,
        settings: project.settings,
        ids: project.ids.clone(),
        assets: project
            .assets
            .values()
            .map(|a| {
                let abs = absolute(&a.path);
                FileAsset {
                    relative_path: pathdiff::diff_paths(&abs, dir),
                    asset: Asset {
                        path: abs,
                        ..a.clone()
                    },
                }
            })
            .collect(),
        timeline: project.timeline.clone(),
        subtitle_style: project.subtitle_style.clone(),
    };
    let json = serde_json::to_vec_pretty(&file)?;
    let tmp = path.with_extension("vep.tmp");
    fs::write(&tmp, json).map_err(io_err(&tmp))?;
    fs::rename(&tmp, path).map_err(io_err(path))
}

/// 读取工程。素材按「绝对路径 → 相对工程文件的路径」顺序查找。
///
/// # Errors
/// 读不到文件、格式或版本不对、内容违反不变式。
pub fn load_project(path: &Path) -> Result<LoadedProject, ProjectFileError> {
    let bytes = fs::read(path).map_err(io_err(path))?;
    let header: Header = serde_json::from_slice(&bytes)?;
    if header.format != FORMAT {
        return Err(ProjectFileError::WrongFormat);
    }
    if !(OLDEST_READABLE..=VERSION).contains(&header.version) {
        return Err(ProjectFileError::UnsupportedVersion(header.version));
    }
    let file: FileV1 = serde_json::from_slice(&bytes)?;
    let st = file.settings;
    if st.frame_rate.num == 0
        || st.frame_rate.den == 0
        || st.width == 0
        || st.height == 0
        || st.sample_rate == 0
    {
        return Err(ProjectFileError::BadSettings);
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut missing_assets = Vec::new();
    let mut assets = std::collections::BTreeMap::new();
    for FileAsset {
        mut asset,
        relative_path,
    } in file.assets
    {
        if !asset.path.exists() {
            match relative_path.map(|r| dir.join(r)).filter(|p| p.exists()) {
                Some(found) => asset.path = found,
                None => missing_assets.push(asset.id),
            }
        }
        assets.insert(asset.id, asset);
    }
    let mut timeline = file.timeline;
    timeline.normalize_order();
    let project = Project {
        settings: file.settings,
        assets,
        timeline,
        ids: file.ids,
        subtitle_style: if file.subtitle_style.is_valid() {
            file.subtitle_style
        } else {
            crate::SubtitleStyle::default()
        },
    };
    project
        .timeline
        .check_invariants(&project.assets)
        .map_err(ProjectFileError::Invalid)?;
    Ok(LoadedProject {
        project,
        missing_assets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::sample_project;
    use std::fs;
    use std::sync::Arc;

    fn touch(path: &Path) {
        fs::write(path, b"x").unwrap();
    }

    /// 把示例工程的素材路径换成 dir 下真实存在的文件。
    fn project_in(dir: &Path) -> Project {
        let (mut project, _) = sample_project();
        fs::create_dir_all(dir.join("media")).unwrap();
        for asset in project.assets.values_mut() {
            let p = dir.join("media").join(&asset.name);
            touch(&p);
            asset.path = p;
        }
        project
    }

    #[test]
    fn save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_in(dir.path());
        let file = dir.path().join("a.vep");
        save_project(&project, &file).unwrap();
        let loaded = load_project(&file).unwrap();
        assert_eq!(loaded.missing_assets, []);
        assert_eq!(loaded.project, project);
    }

    #[test]
    fn version_6_fields_round_trip() {
        use crate::edit::{
            ADJUSTMENT_LENGTH, TrackFlag, add_adjustment, set_adjust, set_mirror, set_track_flag,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut project = project_in(dir.path());
        let (track, v1) = {
            let t = project
                .timeline
                .tracks
                .iter()
                .find(|t| t.kind == crate::TrackKind::Video)
                .unwrap();
            (t.id, t.clips[0].id)
        };
        set_track_flag(&mut project, track, TrackFlag::Muted, true).unwrap();
        set_track_flag(&mut project, track, TrackFlag::Locked, true).unwrap();
        set_mirror(&mut project, &[v1], true).unwrap();
        let adj = add_adjustment(&mut project, crate::Time::ZERO, ADJUSTMENT_LENGTH).unwrap();
        let warm = crate::FILTERS[3].adjust;
        set_adjust(&mut project, adj, warm).unwrap();
        let file = dir.path().join("v6.vep");
        save_project(&project, &file).unwrap();
        let loaded = load_project(&file).unwrap().project;
        assert_eq!(loaded, project);
        let text = fs::read_to_string(&file).unwrap();
        assert!(
            text.contains("\"Adjustment\"") && text.contains("\"mirror\": true"),
            "{text}"
        );
    }

    #[test]
    fn version_7_fields_round_trip() {
        use crate::edit::{
            add_keyframe_props, apply_gain_at, set_in_point, set_label, set_look, set_out_point,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut project = project_in(dir.path());
        let v1 = project
            .timeline
            .tracks
            .iter()
            .find(|t| t.kind == crate::TrackKind::Video)
            .unwrap()
            .clips[0]
            .id;
        set_label(&mut project, &[v1], Some(5)).unwrap();
        set_look(
            &mut project,
            v1,
            crate::Look {
                blend: crate::BlendMode::Screen,
                radius: 0.3,
                stroke: Some(crate::Stroke::default()),
                shadow: Some(crate::Shadow::default()),
                mask: Some(crate::Mask::new(crate::MaskShape::Heart)),
                chroma: Some(crate::ChromaKey::default()),
            },
        )
        .unwrap();
        add_keyframe_props(&mut project, v1, crate::Time::ZERO, &[crate::Prop::Tint]).unwrap();
        add_keyframe_props(&mut project, v1, crate::Time::ZERO, &[crate::Prop::Volume]).unwrap();
        apply_gain_at(&mut project, v1, crate::Time::from_secs(1), -6.0).unwrap();
        set_in_point(&mut project, crate::Time::from_millis(200)).unwrap();
        set_out_point(&mut project, crate::Time::from_millis(900)).unwrap();
        let file = dir.path().join("v7.vep");
        save_project(&project, &file).unwrap();
        let loaded = load_project(&file).unwrap().project;
        assert_eq!(loaded, project);
        let text = fs::read_to_string(&file).unwrap();
        assert!(
            text.contains("\"Screen\"")
                && text.contains("\"range\"")
                && text.contains("\"Volume\""),
            "{text}"
        );
    }

    #[test]
    fn subtitle_style_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = project_in(dir.path());
        project.subtitle_style.color = 0xFF_D700;
        project.subtitle_style.bold = true;
        let file = dir.path().join("style.vep");
        save_project(&project, &file).unwrap();
        let loaded = load_project(&file).unwrap().project;
        assert_eq!(loaded.subtitle_style, project.subtitle_style);
        // 默认样式不写进文件
        let plain = project_in(dir.path());
        save_project(&plain, &file).unwrap();
        let json = fs::read_to_string(&file).unwrap();
        assert!(!json.contains("subtitle_style"));
    }

    #[test]
    fn old_files_load_with_text_track_on_top() {
        // 第 4 版以前的文件里字幕轨在最后；读进来统一放到最上面（剪映的排法）
        let dir = tempfile::tempdir().unwrap();
        let mut project = project_in(dir.path());
        project.timeline.tracks.rotate_left(1);
        assert_eq!(
            project.timeline.tracks.last().map(|t| t.kind),
            Some(crate::TrackKind::Subtitle)
        );
        let file = dir.path().join("old.vep");
        save_project(&project, &file).unwrap();
        let loaded = load_project(&file).unwrap().project;
        assert_eq!(loaded.timeline.tracks[0].kind, crate::TrackKind::Subtitle);
        assert_eq!(loaded.timeline.tracks.len(), project.timeline.tracks.len());
    }

    #[test]
    fn loads_v1_file_with_default_transform() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = project_in(dir.path());
        let first = project
            .timeline
            .track_of_kind(crate::TrackKind::Video)
            .unwrap()
            .clips[0]
            .id;
        crate::edit::set_transform(
            &mut project,
            first,
            crate::Transform {
                scale: 0.5,
                ..crate::Transform::default()
            },
        )
        .unwrap();
        let file = dir.path().join("a.vep");
        save_project(&project, &file).unwrap();
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(json["version"], 7);
        // 第 2 版文件：transform 里没有 rotation，读进来是 0
        let mut v2 = json.clone();
        v2["version"] = 2.into();
        for track in v2["timeline"]["tracks"].as_array_mut().unwrap() {
            for clip in track["clips"].as_array_mut().unwrap() {
                clip["transform"]
                    .as_object_mut()
                    .unwrap()
                    .remove("rotation");
            }
        }
        fs::write(&file, serde_json::to_vec(&v2).unwrap()).unwrap();
        let from_v2 = load_project(&file).unwrap().project;
        let t = from_v2.timeline.find_clip(first).unwrap().1.transform;
        assert_eq!((t.scale, t.rotation), (0.5, 0.0));
        // 改成第 1 版文件的样子：版本 1、片段没有 transform 字段
        json["version"] = 1.into();
        for track in json["timeline"]["tracks"].as_array_mut().unwrap() {
            for clip in track["clips"].as_array_mut().unwrap() {
                clip.as_object_mut().unwrap().remove("transform");
            }
        }
        fs::write(&file, serde_json::to_vec(&json).unwrap()).unwrap();
        let loaded = load_project(&file).unwrap().project;
        for track in &loaded.timeline.tracks {
            for clip in &track.clips {
                assert!(clip.transform.is_default());
            }
        }
        save_project(&loaded, &file).unwrap();
        let again: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(again["version"], 7);
    }

    #[test]
    fn animation_round_trips_and_v3_file_loads() {
        let (mut project, ids) = sample_project();
        let tr = project
            .timeline
            .track_of_kind_mut(crate::TrackKind::Video)
            .unwrap();
        let mut c = (*tr.clips[0]).clone();
        c.animation
            .upsert(crate::Prop::Scale, crate::Time::ZERO, 1.0);
        c.animation
            .upsert(crate::Prop::Scale, crate::Time::from_secs(2), 1.5);
        tr.clips[0] = Arc::new(c);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.vep");
        save_project(&project, &file).unwrap();
        let loaded = load_project(&file).unwrap().project;
        assert_eq!(
            loaded.timeline.find_clip(ids.v1).unwrap().1.animation,
            project.timeline.find_clip(ids.v1).unwrap().1.animation
        );
        // 第 3 版文件（没有 animation 字段）照常能读
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        json["version"] = 3.into();
        for track in json["timeline"]["tracks"].as_array_mut().unwrap() {
            for clip in track["clips"].as_array_mut().unwrap() {
                clip.as_object_mut().unwrap().remove("animation");
            }
        }
        fs::write(&file, serde_json::to_vec(&json).unwrap()).unwrap();
        let v3 = load_project(&file).unwrap().project;
        assert!(
            v3.timeline
                .find_clip(ids.v1)
                .unwrap()
                .1
                .animation
                .is_empty()
        );
    }

    #[test]
    fn load_rejects_unknown_version() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.vep");
        fs::write(&file, r#"{"format":"ve-project","version":99}"#).unwrap();
        assert!(matches!(
            load_project(&file),
            Err(ProjectFileError::UnsupportedVersion(99))
        ));
        fs::write(&file, r#"{"format":"other","version":1}"#).unwrap();
        assert!(matches!(
            load_project(&file),
            Err(ProjectFileError::WrongFormat)
        ));
        fs::write(&file, "not json").unwrap();
        assert!(matches!(
            load_project(&file),
            Err(ProjectFileError::Json(_))
        ));
        assert!(matches!(
            load_project(&dir.path().join("nope.vep")),
            Err(ProjectFileError::Io { .. })
        ));
    }

    #[test]
    fn load_project_missing_asset_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_in(dir.path());
        let file = dir.path().join("a.vep");
        save_project(&project, &file).unwrap();
        let (_, ids) = sample_project();
        let gone = project.assets[&ids.audio_asset].path.clone();
        fs::remove_file(gone).unwrap();
        let loaded = load_project(&file).unwrap();
        assert_eq!(loaded.missing_assets, vec![ids.audio_asset]);
        assert_eq!(loaded.project.timeline, project.timeline);
    }

    #[test]
    fn load_resolves_relative_path_after_move() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        let project = project_in(&old);
        save_project(&project, &old.join("a.vep")).unwrap();
        let new = root.path().join("new");
        fs::rename(&old, &new).unwrap();
        let loaded = load_project(&new.join("a.vep")).unwrap();
        assert!(
            loaded.missing_assets.is_empty(),
            "{:?}",
            loaded.missing_assets
        );
        for asset in loaded.project.assets.values() {
            assert!(asset.path.starts_with(&new), "{}", asset.path.display());
        }
    }

    #[test]
    fn load_rejects_broken_invariants() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = project_in(dir.path());
        let v = project
            .timeline
            .track_of_kind_mut(crate::TrackKind::Video)
            .unwrap();
        let first = Arc::clone(&v.clips[0]);
        v.clips.push(first);
        let file = dir.path().join("a.vep");
        save_project(&project, &file).unwrap();
        assert!(matches!(
            load_project(&file),
            Err(ProjectFileError::Invalid(_))
        ));
    }

    #[test]
    fn load_rejects_invalid_settings() {
        // 帧率为 0 会在后面除以零：加载时就拒绝
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.vep");
        for (num, den, w, h) in [
            (0, 1, 1920, 1080),
            (30, 0, 1920, 1080),
            (30, 1, 0, 1080),
            (30, 1, 1920, 0),
        ] {
            let mut project = project_in(dir.path());
            project.settings.frame_rate = crate::FrameRate::new(num, den);
            project.settings.width = w;
            project.settings.height = h;
            save_project(&project, &file).unwrap();
            assert!(
                matches!(load_project(&file), Err(ProjectFileError::BadSettings)),
                "{num}/{den} {w}x{h}"
            );
        }
    }

    #[test]
    fn save_stores_absolute_and_relative_paths_even_for_relative_input() {
        // 素材路径是相对路径（相对当前目录）时，换个目录启动就找不到：保存时要存成绝对路径，并算出相对工程文件的路径
        let cwd = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir_in(&cwd).unwrap();
        let mut project = project_in(dir.path());
        for a in project.assets.values_mut() {
            a.path = pathdiff::diff_paths(&a.path, &cwd).unwrap();
            assert!(a.path.is_relative());
        }
        let file = dir.path().join("a.vep");
        save_project(&project, &file).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        for a in json["assets"].as_array().unwrap() {
            assert!(Path::new(a["path"].as_str().unwrap()).is_absolute(), "{a}");
            assert!(
                a["relative_path"].as_str().unwrap().starts_with("media"),
                "{a}"
            );
        }
    }

    #[test]
    fn save_to_unwritable_path_errors() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_in(dir.path());
        let file = dir.path().join("no_such_dir").join("a.vep");
        assert!(matches!(
            save_project(&project, &file),
            Err(ProjectFileError::Io { .. })
        ));
    }
}
