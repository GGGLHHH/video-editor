//! 文档：一个打开的工程 + 撤销历史 + 选择 + 保存状态。不依赖界面框架。

use std::path::{Path, PathBuf};
use ve_core::{Asset, AssetId, ClipId, EditError, History, Project, Timeline, TrackId};

/// 轨道没锁（找不到的轨道当成锁着）。
fn unlocked(timeline: &Timeline, track: TrackId) -> bool {
    timeline.track(track).is_some_and(|t| !t.locked)
}

/// 撤销步数上限。
const HISTORY_LIMIT: usize = 200;

/// 一个打开的工程。
#[derive(Debug, Clone)]
pub struct Document {
    /// 工程。
    pub project: Project,
    history: History,
    selection: Vec<ClipId>,
    /// Shift 点击选区间的起点：最后一次单击 / Ctrl 点击的片段。
    anchor: Option<ClipId>,
    path: Option<PathBuf>,
    dirty: bool,
    revision: u64,
    /// 手势（拖拽）开始时的时间轴：手势结束时看哪些轨道被拖空了。
    gesture_before: Option<Timeline>,
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
            anchor: None,
            path,
            dirty: false,
            revision: 0,
            gesture_before: None,
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
        // 删掉的、所在轨道刚锁上的都不再选中
        self.selection.retain(|c| {
            timeline
                .find_clip(*c)
                .is_some_and(|(t, _)| unlocked(timeline, t))
        });
    }

    /// 执行一次编辑：成功且确有变化才记撤销步骤；失败时什么都不变。
    /// 编辑后按工程设置做主轨磁吸和联动（见 [`ve_core::magnet`]）。
    ///
    /// # Errors
    /// 编辑操作本身的错误。
    pub fn edit<T>(
        &mut self,
        f: impl FnOnce(&mut Project) -> Result<T, EditError>,
    ) -> Result<T, EditError> {
        let before = self.project.clone();
        let layout = ve_core::magnet::Layout::of(&self.project.timeline);
        let out = match f(&mut self.project) {
            Ok(out) => out,
            Err(e) => {
                self.project = before;
                return Err(e);
            }
        };
        // 主轨磁吸 / 联动：每次编辑后收尾（开关在工程设置里）
        ve_core::magnet::settle(&mut self.project, &layout);
        // 锁定的轨道：动到了就整步撤回
        if let Err(e) = ve_core::edit::check_locked(&before.timeline, &self.project.timeline) {
            self.project = before;
            return Err(e);
        }
        // 被这一步拖空 / 删空的轨道收掉；拖动中先不收（轨道一少，鼠标下面的轨道就变了），松手时再收
        if !self.in_gesture() {
            ve_core::edit::prune_emptied_tracks(&before.timeline, &mut self.project);
        }
        if self.project != before {
            self.history.record(&before);
            self.changed();
        }
        Ok(out)
    }

    /// 开始一个连续手势（拖拽）：期间的编辑合成一步撤销。
    pub fn begin_gesture(&mut self) {
        self.history.begin_transaction(&self.project);
        self.gesture_before = Some(self.project.timeline.clone());
    }

    /// 结束连续手势（拖空的轨道这时收掉，和拖动算同一步撤销）。
    pub fn end_gesture(&mut self) {
        if let Some(before) = self.gesture_before.take()
            && ve_core::edit::prune_emptied_tracks(&before, &mut self.project)
        {
            self.changed();
        }
        self.history.commit_transaction(&self.project);
    }

    /// 放弃进行中的手势（拖到一半按 Esc）：回到手势开始前，不记撤销。没有手势返回 `false`。
    pub fn cancel_gesture(&mut self) -> bool {
        self.gesture_before = None;
        let restored = self.history.abort_transaction(&mut self.project);
        if restored {
            self.changed();
        }
        restored
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
        // 锁定轨道上的片段点不中
        if !self.selectable(clip) {
            return;
        }
        self.anchor = Some(clip);
        if !additive {
            self.selection = vec![clip];
        } else if let Some(i) = self.selection.iter().position(|c| *c == clip) {
            self.selection.remove(i);
        } else {
            self.selection.push(clip);
        }
    }

    /// Shift 点击：选中起点片段和 `to` 之间的一块——两者所在轨道之间的每条轨上，
    /// 和两者合起来的时间范围有交集的片段。没有起点（或起点已删）时只选中 `to`。起点不变。
    pub fn select_range(&mut self, to: ClipId) {
        let tl = &self.project.timeline;
        let place = |clip: ClipId| {
            tl.tracks.iter().enumerate().find_map(|(i, t)| {
                t.clips
                    .iter()
                    .find(|c| c.id == clip)
                    .map(|c| (i, c.start, c.end()))
            })
        };
        let Some(target) = place(to) else { return };
        let Some(from) = self.anchor.and_then(place) else {
            self.selection = vec![to];
            self.anchor = Some(to);
            return;
        };
        let (top, bottom) = (from.0.min(target.0), from.0.max(target.0));
        let (lo, hi) = (from.1.min(target.1), from.2.max(target.2));
        self.selection = tl.tracks[top..=bottom]
            .iter()
            .filter(|t| !t.locked)
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.start < hi && c.end() > lo)
            .map(|c| c.id)
            .collect();
    }

    /// 换成一组选择（框选）：去重，去掉不存在的片段。
    pub fn set_selection(&mut self, clips: Vec<ClipId>) {
        let tl = &self.project.timeline;
        let mut out: Vec<ClipId> = Vec::with_capacity(clips.len());
        for c in clips {
            if !out.contains(&c) && tl.find_clip(c).is_some_and(|(t, _)| unlocked(tl, t)) {
                out.push(c);
            }
        }
        self.selection = out;
    }

    /// 片段是否选中。
    #[must_use]
    pub fn is_selected(&self, clip: ClipId) -> bool {
        self.selection.contains(&clip)
    }

    /// 选中所有片段。
    pub fn select_all(&mut self) {
        self.selection = self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| !t.locked)
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect();
    }

    /// 片段能不能选中：存在、且不在锁定的轨道上。
    #[must_use]
    pub fn selectable(&self, clip: ClipId) -> bool {
        let tl = &self.project.timeline;
        tl.find_clip(clip).is_some_and(|(t, _)| unlocked(tl, t))
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

    /// 内容还没存过（导入出来的新工程）：标成有未保存修改，自动保存、关窗口提示都会管它。
    pub const fn mark_unsaved(&mut self) {
        self.dirty = true;
        self.revision += 1;
    }

    /// 已保存到 `path`。
    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.dirty = false;
    }

    /// 工程名：文件名（不带扩展名），没保存过的叫「未命名」。
    #[must_use]
    pub fn name(&self) -> String {
        self.path
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(|| "未命名".to_owned(), |s| s.to_string_lossy().into_owned())
    }

    /// 窗口标题：工程名，未保存时加 ` *`。
    #[must_use]
    pub fn title(&self) -> String {
        let name = self.name();
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

    /// 撤销、选择等基本行为在磁吸、联动都关着时测。
    fn doc() -> (Document, ve_core::testing::SampleIds) {
        let (mut p, ids) = sample_project();
        p.settings.main_magnet = false;
        p.settings.linkage = false;
        (Document::from_project(p, None), ids)
    }

    fn sorted(d: &Document) -> Vec<ClipId> {
        let mut v = d.selection().to_vec();
        v.sort_unstable();
        v
    }

    fn set(mut v: Vec<ClipId>) -> Vec<ClipId> {
        v.sort_unstable();
        v
    }

    #[test]
    fn shift_range_covers_tracks_and_time_between_anchor_and_target() {
        let (mut d, ids) = doc();
        // 没有起点：只选中点到的
        d.select_range(ids.v2);
        assert_eq!(d.selection(), [ids.v2]);
        // 轨道从上到下是 字幕、视频、音频。起点 s1（字幕轨 1–3 秒）到 a1（音频轨 0–8 秒）：
        // 三条轨上 0–8 秒内的全选上
        d.select(ids.s1, false);
        d.select_range(ids.a1);
        assert_eq!(sorted(&d), set(vec![ids.v1, ids.v2, ids.a1, ids.s1]));
        // 起点不变：再 Shift 点 v1，范围改成 字幕—视频 两条轨、0–5 秒
        d.select_range(ids.v1);
        assert_eq!(sorted(&d), set(vec![ids.v1, ids.s1]));
        // 同一轨：v1 到 v2
        d.select(ids.v1, false);
        d.select_range(ids.v2);
        assert_eq!(sorted(&d), set(vec![ids.v1, ids.v2]));
    }

    #[test]
    fn ctrl_toggle_moves_anchor_and_set_selection_replaces() {
        let (mut d, ids) = doc();
        d.select(ids.v1, false);
        d.select(ids.a1, true);
        assert!(d.is_selected(ids.v1) && d.is_selected(ids.a1));
        // 起点跟着最后一次点击走：a1（音频，最下）到 s1（字幕，最上），三条轨 0–8 秒
        d.select_range(ids.s1);
        assert_eq!(sorted(&d), set(vec![ids.v1, ids.v2, ids.a1, ids.s1]));
        d.select(ids.v2, false);
        d.select_range(ids.a1);
        assert_eq!(
            sorted(&d),
            set(vec![ids.v1, ids.v2, ids.a1]),
            "视频—音频两条轨"
        );
        d.set_selection(vec![ids.v2, ids.v2, ClipId(999)]);
        assert_eq!(d.selection(), [ids.v2], "去重、去掉不存在的");
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
    fn locked_tracks_cannot_be_selected_or_edited() {
        use ve_core::edit::{TrackFlag, set_track_flag};
        let (mut d, ids) = doc();
        d.select(ids.a1, false);
        d.edit(|p| set_track_flag(p, ids.audio_track, TrackFlag::Locked, true))
            .unwrap();
        assert_eq!(d.selection(), &[] as &[ClipId], "锁上后不再选中");
        d.select(ids.a1, false);
        assert_eq!(d.selection(), &[] as &[ClipId]);
        d.select_all();
        assert!(!d.selection().contains(&ids.a1));
        d.set_selection(vec![ids.a1, ids.v1]);
        assert_eq!(d.selection(), &[ids.v1]);
        d.select(ids.s1, false);
        d.select_range(ids.v2);
        assert!(!d.selection().contains(&ids.a1));
        // 直接改锁着轨道上的片段：整步撤回，不记撤销
        let before = d.project.clone();
        assert_eq!(
            d.edit(|p| delete_clips(p, &[ids.a1])),
            Err(EditError::TrackLocked)
        );
        assert_eq!(d.project, before);
        // 锁定本身能撤销
        assert!(d.undo());
        assert!(!d.project.timeline.track(ids.audio_track).unwrap().locked);
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

    #[test]
    fn edits_settle_main_magnet_and_linkage() {
        let (p, ids) = sample_project();
        let mut d = Document::from_project(p, None);
        // 删掉主轨第一段：后面的补上来，一步撤销能回去
        d.edit(|p| delete_clips(p, &[ids.v1])).unwrap();
        let v2 = d.project.timeline.find_clip(ids.v2).unwrap().1.start;
        assert_eq!(v2, Time::ZERO);
        assert!(d.undo());
        let v2 = d.project.timeline.find_clip(ids.v2).unwrap().1.start;
        assert_eq!(v2, Time::from_secs(5));
    }
}
