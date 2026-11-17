//! 撤销重做：每步保存整个工程的快照（片段是 `Arc`，复制便宜）。

use crate::Project;
use std::collections::VecDeque;

/// 撤销重做历史。
#[derive(Debug, Clone)]
pub struct History {
    undo: VecDeque<Project>,
    redo: Vec<Project>,
    limit: usize,
    transaction: Option<Project>,
}

impl History {
    /// 最多保留 `limit` 步。
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            limit: limit.max(1),
            transaction: None,
        }
    }

    fn push(&mut self, before: Project) {
        self.redo.clear();
        self.undo.push_back(before);
        while self.undo.len() > self.limit {
            self.undo.pop_front();
        }
    }

    /// 在一次编辑之前调用，记下编辑前的状态。事务进行中时忽略（整个事务算一步）。
    pub fn record(&mut self, before: &Project) {
        if self.transaction.is_none() {
            self.push(before.clone());
        }
    }

    /// 开始一个事务（如拖拽）：期间的所有编辑合并成一步。已在事务中则忽略。
    pub fn begin_transaction(&mut self, before: &Project) {
        if self.transaction.is_none() {
            self.transaction = Some(before.clone());
        }
    }

    /// 是否在事务中。
    #[must_use]
    pub const fn in_transaction(&self) -> bool {
        self.transaction.is_some()
    }

    /// 结束事务；工程确实变了才记一步。
    pub fn commit_transaction(&mut self, current: &Project) {
        if let Some(before) = self.transaction.take()
            && before != *current
        {
            self.push(before);
        }
    }

    /// 撤销一步；没有可撤销的返回 `false`。
    pub fn undo(&mut self, current: &mut Project) -> bool {
        self.transaction = None;
        let Some(prev) = self.undo.pop_back() else {
            return false;
        };
        self.redo.push(std::mem::replace(current, prev));
        true
    }

    /// 重做一步；没有可重做的返回 `false`。
    pub fn redo(&mut self, current: &mut Project) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push_back(std::mem::replace(current, next));
        true
    }

    /// 能否撤销。
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// 能否重做。
    #[must_use]
    pub const fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{delete_clips, move_clip, set_gain};
    use crate::testing::sample_project;
    use crate::{Project, Time};
    use proptest::prelude::*;

    #[test]
    fn undo_redo_round_trip() {
        let (mut p, ids) = sample_project();
        let original = p.clone();
        let mut h = History::new(100);
        assert!(!h.can_undo());
        h.record(&p);
        delete_clips(&mut p, &[ids.v1]).unwrap();
        let edited = p.clone();
        assert!(h.can_undo());
        assert!(h.undo(&mut p));
        assert_eq!(p, original);
        assert!(h.can_redo());
        assert!(h.redo(&mut p));
        assert_eq!(p, edited);
        assert!(!h.redo(&mut p));
        assert!(h.undo(&mut p));
        assert!(!h.undo(&mut p));
    }

    #[test]
    fn new_edit_clears_redo() {
        let (mut p, ids) = sample_project();
        let mut h = History::new(100);
        h.record(&p);
        delete_clips(&mut p, &[ids.v1]).unwrap();
        h.undo(&mut p);
        h.record(&p);
        set_gain(&mut p, ids.a1, -6.0).unwrap();
        assert!(!h.can_redo());
    }

    #[test]
    fn transaction_merges_steps() {
        let (mut p, ids) = sample_project();
        let original = p.clone();
        let mut h = History::new(100);
        h.begin_transaction(&p);
        for s in 10..20 {
            h.record(&p); // 事务内的 record 不单独成步
            move_clip(&mut p, ids.v2, ids.video_track, Time::from_secs(s)).unwrap();
        }
        assert!(h.in_transaction());
        h.commit_transaction(&p);
        assert!(!h.in_transaction());
        assert!(h.undo(&mut p));
        assert_eq!(p, original);
        assert!(!h.can_undo());
    }

    #[test]
    fn empty_transaction_adds_no_step() {
        let (p, _) = sample_project();
        let mut h = History::new(100);
        h.begin_transaction(&p);
        h.commit_transaction(&p);
        assert!(!h.can_undo());
        h.commit_transaction(&p); // 没开事务时提交无效果
        assert!(!h.can_undo());
    }

    #[test]
    fn limit_drops_oldest() {
        let (mut p, ids) = sample_project();
        let mut h = History::new(3);
        for g in 0..5 {
            h.record(&p);
            set_gain(&mut p, ids.a1, -f32::from(u8::try_from(g).unwrap())).unwrap();
        }
        let mut n = 0;
        while h.undo(&mut p) {
            n += 1;
        }
        assert_eq!(n, 3);
        // 快照依次是 0,0,-1,-2,-3，只留最近 3 个，全部撤销后回到 -1
        assert!((p.timeline.find_clip(ids.a1).unwrap().1.gain_db + 1.0).abs() < f32::EPSILON);
    }

    proptest! {
        #[test]
        fn undo_all_returns_initial(moves in proptest::collection::vec(0i64..100, 1..30)) {
            let (mut p, ids) = sample_project();
            let original: Project = p.clone();
            let mut h = History::new(1000);
            for m in moves {
                h.record(&p);
                if move_clip(&mut p, ids.v2, ids.video_track, Time::from_secs(10 + m)).is_err() {
                    h.undo(&mut p);
                }
            }
            while h.undo(&mut p) {}
            prop_assert_eq!(p, original);
        }
    }
}
