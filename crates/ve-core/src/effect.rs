//! 画面特效（剪映素材面板「特效 › 画面特效」）：特效片段放在视频轨上，对它下面已经合成好的
//! 整个画面做处理（模糊、锐化、暗角……）。合成器里每种特效一段着色器，预览和导出共用。

use serde::{Deserialize, Serialize};

/// 特效种类。编号写进工程文件、也传给着色器，不能改。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EffectKind {
    /// 模糊。
    Blur,
    /// 锐化。
    Sharpen,
    /// 暗角：四周压暗。
    Vignette,
    /// 颗粒：胶片噪点，每帧不同。
    Grain,
    /// 马赛克：方块。
    Mosaic,
    /// RGB 色差：红蓝两个通道左右错开。
    ChromaShift,
    /// 闪白：每秒一次白闪。
    Flash,
    /// 发光：亮的地方向外晕开。
    Glow,
    /// 插件特效（插件目录里的 `.wgsl`，见 [`Effect::plugin`]）。
    Plugin,
}

impl EffectKind {
    /// 全部（素材面板里的顺序）。
    pub const ALL: [Self; 8] = [
        Self::Blur,
        Self::Sharpen,
        Self::Vignette,
        Self::Grain,
        Self::Mosaic,
        Self::ChromaShift,
        Self::Flash,
        Self::Glow,
    ];

    /// 显示名。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Blur => "模糊",
            Self::Sharpen => "锐化",
            Self::Vignette => "暗角",
            Self::Grain => "颗粒",
            Self::Mosaic => "马赛克",
            Self::ChromaShift => "RGB 色差",
            Self::Flash => "闪白",
            Self::Glow => "发光",
            Self::Plugin => "插件",
        }
    }

    /// 着色器里的编号。
    #[must_use]
    pub const fn code(self) -> f32 {
        match self {
            Self::Blur => 0.0,
            Self::Sharpen => 1.0,
            Self::Vignette => 2.0,
            Self::Grain => 3.0,
            Self::Mosaic => 4.0,
            Self::ChromaShift => 5.0,
            Self::Flash => 6.0,
            Self::Glow => 7.0,
            Self::Plugin => 100.0,
        }
    }
}

/// 插件名（插件文件名去掉扩展名，最多 [`PluginName::MAX`] 字节）。定长存放，特效可以按值复制。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PluginName {
    bytes: [u8; Self::MAX],
    len: u8,
}

impl PluginName {
    /// 最长多少字节。
    pub const MAX: usize = 48;
    /// 空名字（内置特效）。
    pub const EMPTY: Self = Self {
        bytes: [0; Self::MAX],
        len: 0,
    };

    /// 用 `name`；太长或空的返回 `None`。
    #[must_use]
    pub fn new(name: &str) -> Option<Self> {
        if name.is_empty() || name.len() > Self::MAX {
            return None;
        }
        let mut bytes = [0; Self::MAX];
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        Some(Self {
            bytes,
            len: u8::try_from(name.len()).ok()?,
        })
    }

    /// 名字。
    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or_default()
    }

    /// 是不是空的。
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for PluginName {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl std::fmt::Debug for PluginName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl Serialize for PluginName {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PluginName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s.is_empty() {
            return Ok(Self::EMPTY);
        }
        Self::new(&s).ok_or_else(|| serde::de::Error::custom("插件名太长"))
    }
}

/// 一个特效：种类和强度。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    /// 种类。
    pub kind: EffectKind,
    /// 强度 0–1。
    pub amount: f32,
    /// 插件名（种类是 [`EffectKind::Plugin`] 时）；内置特效为空，不写进文件。
    #[serde(default, skip_serializing_if = "PluginName::is_empty")]
    pub plugin: PluginName,
}

impl Effect {
    /// 新加的特效默认强度（剪映的特效默认值大多在中间）。
    pub const DEFAULT_AMOUNT: f32 = 0.5;

    /// 默认强度的特效。
    #[must_use]
    pub const fn new(kind: EffectKind) -> Self {
        Self {
            kind,
            amount: Self::DEFAULT_AMOUNT,
            plugin: PluginName::EMPTY,
        }
    }

    /// 默认强度的插件特效；名字太长或空的返回 `None`。
    #[must_use]
    pub fn plugin(name: &str) -> Option<Self> {
        Some(Self {
            kind: EffectKind::Plugin,
            amount: Self::DEFAULT_AMOUNT,
            plugin: PluginName::new(name)?,
        })
    }

    /// 显示名：插件显示插件名，内置的显示种类名。
    #[must_use]
    pub fn name(&self) -> &str {
        if self.kind == EffectKind::Plugin {
            self.plugin.as_str()
        } else {
            self.kind.label()
        }
    }

    /// 强度在 0–1 里（不是 NaN）；插件特效要有名字。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        (0.0..=1.0).contains(&self.amount)
            && (self.kind != EffectKind::Plugin || !self.plugin.is_empty())
    }
}

/// 合成时的一道特效：画完下面 `after` 层（加底色）以后，对整个画面做 `effect`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectPass {
    /// 作用在最下面这么多层上（`resolve_frame` 给的图层下标；合成前按真正画出来的层数换算）。
    pub after: usize,
    /// 特效。
    pub effect: Effect,
    /// 特效片段里过了多久（秒）：颗粒、闪白按它变化。
    pub seconds: f32,
}

impl EffectPass {
    /// 有的图层没画出来（解不出画面）时，把 `after` 换算成真正画出来的层数：
    /// `kept[i]` 是 `resolve_frame` 给的第 `i` 层画没画。
    #[must_use]
    pub fn remap(passes: &[Self], kept: &[bool]) -> Vec<Self> {
        passes
            .iter()
            .map(|p| Self {
                after: kept.iter().take(p.after).filter(|&&k| k).count(),
                ..*p
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_counts_only_the_layers_that_were_drawn() {
        let e = EffectPass {
            after: 3,
            effect: Effect::new(EffectKind::Blur),
            seconds: 0.0,
        };
        let top = EffectPass { after: 4, ..e };
        let got = EffectPass::remap(&[e, top], &[true, false, true, true]);
        assert_eq!((got[0].after, got[1].after), (2, 3));
    }

    #[test]
    fn kinds_have_distinct_codes_and_labels() {
        let mut codes: Vec<i32> = EffectKind::ALL
            .iter()
            .map(|k| {
                #[expect(clippy::cast_possible_truncation, reason = "编号是小整数")]
                let c = k.code() as i32;
                c
            })
            .collect();
        codes.dedup();
        assert_eq!(codes, (0..8).collect::<Vec<_>>());
        assert!(
            !EffectKind::ALL.contains(&EffectKind::Plugin),
            "插件不在内置列表里"
        );
        let labels: std::collections::HashSet<_> =
            EffectKind::ALL.iter().map(|k| k.label()).collect();
        assert_eq!(labels.len(), 8);
    }

    #[test]
    fn plugin_effects_carry_their_name_through_the_project_file() {
        let e = Effect::plugin("老电视").unwrap();
        assert_eq!(e.name(), "老电视");
        assert!(e.is_valid());
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("老电视"), "{json}");
        let back: Effect = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
        // 内置特效不写插件名
        let blur = Effect::new(EffectKind::Blur);
        assert!(!serde_json::to_string(&blur).unwrap().contains("plugin"));
        assert_eq!(blur.name(), "模糊");
        assert!(Effect::plugin("").is_none());
        assert!(Effect::plugin(&"长".repeat(20)).is_none(), "超过 48 字节");
        let mut nameless = Effect::new(EffectKind::Plugin);
        assert!(!nameless.is_valid());
        nameless.plugin = PluginName::new("x").unwrap();
        assert!(nameless.is_valid());
    }

    #[test]
    fn amount_must_be_between_zero_and_one() {
        let mut e = Effect::new(EffectKind::Blur);
        assert!(e.is_valid());
        e.amount = 1.5;
        assert!(!e.is_valid());
        e.amount = f32::NAN;
        assert!(!e.is_valid());
    }
}
