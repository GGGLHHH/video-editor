//! 文档：一个打开的工程 + 撤销历史 + 选择 + 保存状态。不依赖界面框架。

use std::path::{Path, PathBuf};
use ve_core::{Asset, AssetId, ClipId, EditError, History, Project};

/// 撤销步数上限。
const HISTORY_LIMIT: usize = 200;

/// 一个打开的工程。
#[derive(Debug, Clone)]
pub struct Document {
    /// 工程。
    pub project: Project,
    history: History,
    selection: Vec<ClipId>,
    path: Option<PathBuf>,
    dirty: bool,
    revision: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// 空工程。
    #[must_use]
    pub fn new() -> Self {
        Self::from_project(Project::new(), None)
    }

    /// 打开已有工程。
    #[must_use]
    pub fn from_project(project: Project, path: Option<PathBuf>) -> Self {
        Self {
            project,
            history: History::new(HISTORY_LIMIT),
            selection: Vec::new(),
            path,
            dirty: false,
            revision: 0,
        }
    }

    /// 修改计数：每次工程内容变化加一（界面据此把新工程推给播放引擎）。
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    fn changed(&mut self) {
        self.dirty = true;
        self.revision += 1;
        let timeline = &self.project.timeline;
        self.selection.retain(|c| timeline.find_clip(*c).is_some());
    }

    /// 执行一次编辑：成功且确有变化才记撤销步骤；失败时什么都不变。
    ///
    /// # Errors
    /// 编辑操作本身的错误。
    pub fn edit<T>(
        &mut self,
        f: impl FnOnce(&mut Project) -> Result<T, EditError>,
    ) -> Result<T, EditError> {
        let before = self.project.clone();
        let out = f(&mut self.project)?;
        if self.project != before {
            self.history.record(&before);
            self.changed();
        }
        Ok(out)
    }

    /// 开始一个连续手势（拖拽）：期间的编辑合成一步撤销。
    pub fn begin_gesture(&mut self) {
        self.history.begin_transaction(&self.project);
    }

    /// 结束连续手势。
    pub fn end_gesture(&mut self) {
        self.history.commit_transaction(&self.project);
    }

    /// 是否在连续手势中。
    #[must_use]
    pub const fn in_gesture(&self) -> bool {
        self.history.in_transaction()
    }

    /// 撤销（手势进行中忽略，返回 `false`）。
    pub fn undo(&mut self) -> bool {
        if self.in_gesture() {
            return false;
        }
        let done = self.history.undo(&mut self.project);
        if done {
            self.changed();
        }
        done
    }

    /// 重做（手势进行中忽略，返回 `false`）。
    pub fn redo(&mut self) -> bool {
        if self.in_gesture() {
            return false;
        }
        let done = self.history.redo(&mut self.project);
        if done {
            self.changed();
        }
        done
    }

    /// 能否撤销。
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// 能否重做。
    #[must_use]
    pub const fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// 导入素材（可撤销）。第一个视频素材决定工程的尺寸和帧率。
    pub fn add_asset(&mut self, asset: Asset) -> AssetId {
        let before = self.project.clone();
        let has_video = self
            .project
            .assets
            .values()
            .any(|a| a.video.is_some_and(|v| !v.still));
        if let Some(v) = asset.video
            && !v.still
            && !has_video
        {
            // yuv420p 编码要求偶数宽高
            self.project.settings.width = (v.width & !1).max(2);
            self.project.settings.height = (v.height & !1).max(2);
            self.project.settings.frame_rate = v.frame_rate;
        }
        let id = self.project.add_asset(asset);
        self.history.record(&before);
        self.changed();
        id
    }

    /// 选中片段；`additive` 时切换该片段的选中状态。
    pub fn select(&mut self, clip: ClipId, additive: bool) {
        if !additive {
            self.selection = vec![clip];
        } else if let Some(i) = self.selection.iter().position(|c| *c == clip) {
            self.selection.remove(i);
        } else {
            self.selection.push(clip);
        }
    }

    /// 取消选择。
    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// 选中的片段。
    #[must_use]
    pub fn selection(&self) -> &[ClipId] {
        &self.selection
    }

    /// 是否有未保存的修改。
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 工程文件路径。
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// 已保存到 `path`。
    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.dirty = false;
    }

    /// 窗口标题：工程名，未保存时加 ` *`。
    #[must_use]
    pub fn title(&self) -> String {
        let name = self
            .path
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(|| "未命名".to_owned(), |s| s.to_string_lossy().into_owned());
        if self.dirty {
            format!("{name} *")
        } else {
            name
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::edit::{delete_clips, move_clip, set_gain};
    use ve_core::testing::{sample_project, video_info};
    use ve_core::{Asset, AssetId, FrameRate, Time, VideoInfo};

    fn doc() -> (Document, ve_core::testing::SampleIds) {
        let (p, ids) = sample_project();
        (Document::from_project(p, None), ids)
    }

    #[test]
    fn edit_records_undo_and_marks_dirty() {
        let (mut d, ids) = doc();
        assert!(!d.is_dirty() && !d.can_undo());
        let r0 = d.revision();
        d.edit(|p| delete_clips(p, &[ids.v1])).unwrap();
        assert!(d.is_dirty() && d.can_undo());
        assert!(d.revision() > r0);
        assert!(d.undo());
        assert!(d.project.timeline.find_clip(ids.v1).is_some());
        assert!(d.redo());
        assert!(d.project.timeline.find_clip(ids.v1).is_none());
    }

    #[test]
    fn failed_edit_records_nothing() {
        let (mut d, ids) = doc();
        let r0 = d.revision();
        let err = d.edit(|p| move_clip(p, ids.v2, ids.video_track, Time::from_secs(4)));
        assert!(err.is_err());
        assert!(!d.can_undo() && !d.is_dirty());
        assert_eq!(d.revision(), r0);
    }

    #[test]
    fn drag_transaction_is_one_undo_step() {
        let (mut d, ids) = doc();
        d.begin_gesture();
        for s in 10..15 {
            d.edit(|p| move_clip(p, ids.v2, ids.video_track, Time::from_secs(s)))
                .unwrap();
        }
        d.end_gesture();
        assert!(d.undo());
        assert_eq!(
            d.project.timeline.find_clip(ids.v2).unwrap().1.start,
            Time::from_secs(5)
        );
        assert!(!d.can_undo());
    }

    #[test]
    fn undo_during_gesture_is_ignored() {
        // 拖拽中按 Ctrl+Z：忽略，不能拆掉进行中的事务（否则之后每次鼠标移动各记一步）
        let (mut d, ids) = doc();
        d.begin_gesture();
        assert!(d.in_gesture());
        d.edit(|p| move_clip(p, ids.v2, ids.video_track, Time::from_secs(20)))
            .unwrap();
        assert!(!d.undo());
        assert!(!d.redo());
        d.edit(|p| move_clip(p, ids.v2, ids.video_track, Time::from_secs(30)))
            .unwrap();
        d.end_gesture();
        assert!(!d.in_gesture());
        assert!(d.undo());
        assert_eq!(
            d.project.timeline.find_clip(ids.v2).unwrap().1.start,
            Time::from_secs(5)
        );
        assert!(!d.can_undo());
    }

    #[test]
    fn selection_is_pruned_after_delete_and_undo_keeps_valid_ids() {
        let (mut d, ids) = doc();
        d.select(ids.v1, false);
        d.select(ids.a1, true);
        assert_eq!(d.selection(), &[ids.v1, ids.a1]);
        d.select(ids.a1, true); // 再点一次取消
        assert_eq!(d.selection(), &[ids.v1]);
        d.edit(|p| delete_clips(p, &[ids.v1])).unwrap();
        assert_eq!(d.selection(), &[] as &[ClipId]);
        d.select(ids.a1, false);
        d.clear_selection();
        assert_eq!(d.selection(), &[] as &[ClipId]);
    }

    #[test]
    fn save_state_and_title() {
        let (mut d, ids) = doc();
        assert_eq!(d.title(), "未命名");
        d.edit(|p| set_gain(p, ids.a1, -3.0)).unwrap();
        assert_eq!(d.title(), "未命名 *");
        d.mark_saved("D:/work/片子.vep".into());
        assert_eq!(d.title(), "片子");
        assert!(!d.is_dirty());
        assert_eq!(d.path().unwrap().to_str(), Some("D:/work/片子.vep"));
    }

    #[test]
    fn first_video_import_sets_project_settings() {
        let mut d = Document::new();
        let asset = |name: &str, video: Option<VideoInfo>| Asset {
            id: AssetId(0),
            path: name.into(),
            name: name.into(),
            duration: Time::from_secs(3),
            video,
            audio: None,
        };
        let vertical = VideoInfo {
            width: 1080,
            height: 1920,
            frame_rate: FrameRate::new(60, 1),
            still: false,
        };
        let a = d.add_asset(asset("v.mp4", Some(vertical)));
        assert_eq!(
            (d.project.settings.width, d.project.settings.height),
            (1080, 1920)
        );
        assert_eq!(d.project.settings.frame_rate, FrameRate::new(60, 1));
        // 第二个视频不再改设置
        d.add_asset(asset("w.mp4", Some(video_info())));
        assert_eq!(d.project.settings.width, 1080);
        assert!(d.project.assets.contains_key(&a));
        assert!(d.can_undo() && d.is_dirty());
    }

    #[test]
    fn odd_video_size_becomes_even_project_size() {
        let mut d = Document::new();
        d.add_asset(Asset {
            id: AssetId(0),
            path: "odd.mp4".into(),
            name: "odd.mp4".into(),
            duration: Time::from_secs(3),
            video: Some(VideoInfo {
                width: 853,
                height: 481,
                frame_rate: FrameRate::new(30, 1),
                still: false,
            }),
            audio: None,
        });
        assert_eq!(
            (d.project.settings.width, d.project.settings.height),
            (852, 480)
        );
    }

    #[test]
    fn still_image_does_not_set_settings() {
        let mut d = Document::new();
        d.add_asset(Asset {
            id: AssetId(0),
            path: "a.png".into(),
            name: "a.png".into(),
            duration: Time::from_secs(5),
            video: Some(VideoInfo {
                width: 10,
                height: 10,
                frame_rate: FrameRate::new(25, 1),
                still: true,
            }),
            audio: None,
        });
        assert_eq!(d.project.settings.width, 1920);
    }

    #[test]
    fn undo_on_empty_history_is_noop() {
        let mut d = Document::new();
        assert!(!d.undo());
        assert!(!d.redo());
        assert!(!d.can_redo());
    }
}
