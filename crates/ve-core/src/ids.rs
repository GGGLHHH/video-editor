//! 标识符与发号器。

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);
    };
}

id_type!(
    /// 素材 id。
    AssetId
);
id_type!(
    /// 片段 id。
    ClipId
);
id_type!(
    /// 轨道 id。
    TrackId
);

/// 单调递增的发号器，随工程一起保存，保证 id 在工程内永不重复。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdGen {
    next: u64,
}

impl IdGen {
    /// 新发号器，从 1 开始。
    #[must_use]
    pub const fn new() -> Self {
        Self { next: 1 }
    }

    const fn take(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    /// 新素材 id。
    pub const fn asset(&mut self) -> AssetId {
        AssetId(self.take())
    }

    /// 新片段 id。
    pub const fn clip(&mut self) -> ClipId {
        ClipId(self.take())
    }

    /// 新轨道 id。
    pub const fn track(&mut self) -> TrackId {
        TrackId(self.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_across_kinds() {
        let mut ids = IdGen::new();
        assert_eq!(ids.asset(), AssetId(1));
        assert_eq!(ids.clip(), ClipId(2));
        assert_eq!(ids.track(), TrackId(3));
    }
}
