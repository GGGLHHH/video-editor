//! 解码帧缓存：按字节数限制的 LRU，拖动预览来回时避免重复解码。

use lru::LruCache;
use std::fmt;
use std::sync::Arc;
use ve_core::AssetId;
use ve_media::VideoFrame;

/// 缓存键：（素材，素材内帧号）。
pub type FrameKey = (AssetId, i64);

/// 按字节限制的帧缓存。
pub struct FrameCache {
    lru: LruCache<FrameKey, Arc<VideoFrame>>,
    bytes: usize,
    capacity: usize,
}

impl fmt::Debug for FrameCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameCache")
            .field("frames", &self.lru.len())
            .field("bytes", &self.bytes)
            .field("capacity", &self.capacity)
            .finish()
    }
}

impl FrameCache {
    /// 最多 `capacity` 字节。
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            lru: LruCache::unbounded(),
            bytes: 0,
            capacity,
        }
    }

    /// 取（并标为最近使用）。
    pub fn get(&mut self, key: FrameKey) -> Option<Arc<VideoFrame>> {
        self.lru.get(&key).cloned()
    }

    /// 放入；单帧超过容量则不缓存。
    pub fn insert(&mut self, key: FrameKey, frame: Arc<VideoFrame>) {
        if let Some(old) = self.lru.pop(&key) {
            self.bytes -= old.data.len();
        }
        let size = frame.data.len();
        if size > self.capacity {
            return;
        }
        self.lru.put(key, frame);
        self.bytes += size;
        while self.bytes > self.capacity {
            match self.lru.pop_lru() {
                Some((_, f)) => self.bytes -= f.data.len(),
                None => break,
            }
        }
    }

    /// 删掉某个素材的所有帧（素材换了文件或被移除时）。
    pub fn remove_asset(&mut self, asset: AssetId) {
        let keys: Vec<FrameKey> = self
            .lru
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| k.0 == asset)
            .collect();
        for k in keys {
            if let Some(f) = self.lru.pop(&k) {
                self.bytes -= f.data.len();
            }
        }
    }

    /// 已用字节。
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.bytes
    }

    /// 帧数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.lru.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lru.is_empty()
    }

    /// 清空。
    pub fn clear(&mut self) {
        self.lru.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::{AssetId, Time};

    fn frame(bytes: usize) -> Arc<VideoFrame> {
        Arc::new(VideoFrame {
            pts: Time::ZERO,
            width: 1,
            height: 1,
            data: vec![0; bytes],
        })
    }

    #[test]
    fn frame_cache_evicts_by_bytes() {
        let mut c = FrameCache::new(1000);
        let k = |n| (AssetId(1), n);
        c.insert(k(1), frame(400));
        c.insert(k(2), frame(400));
        assert_eq!(c.bytes(), 800);
        // 访问 1，让 2 成为最久未用
        assert!(c.get(k(1)).is_some());
        c.insert(k(3), frame(400));
        assert!(c.get(k(2)).is_none());
        assert!(c.get(k(1)).is_some());
        assert!(c.get(k(3)).is_some());
        assert_eq!((c.len(), c.bytes()), (2, 800));
    }

    #[test]
    fn remove_asset_drops_only_that_asset() {
        let mut c = FrameCache::new(10_000);
        c.insert((AssetId(1), 1), frame(100));
        c.insert((AssetId(1), 2), frame(100));
        c.insert((AssetId(2), 1), frame(100));
        c.remove_asset(AssetId(1));
        assert_eq!((c.len(), c.bytes()), (1, 100));
        assert!(c.get((AssetId(2), 1)).is_some());
    }

    #[test]
    fn oversized_frame_not_cached_and_reinsert_replaces() {
        let mut c = FrameCache::new(1000);
        c.insert((AssetId(1), 1), frame(2000));
        assert_eq!(c.len(), 0);
        c.insert((AssetId(1), 1), frame(100));
        c.insert((AssetId(1), 1), frame(300));
        assert_eq!((c.len(), c.bytes()), (1, 300));
        c.clear();
        assert_eq!((c.len(), c.bytes()), (0, 0));
    }
}
