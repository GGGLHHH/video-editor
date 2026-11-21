//! 关键帧动画：片段的画面属性、调色和音量随时间变化。
//!
//! 每个属性单独一串关键帧（时间相对片段起点）；没有关键帧的属性用片段原来的固定值。
//! 求值是两端值按缓动曲线的凸组合，结果再整理成合法的画面属性（见 [`Animation::apply`]）。

use crate::{AdjustItem, ColorAdjust, Time, Transform};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 缓动：作用在「这个关键帧到下一个关键帧」这一段上。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Easing {
    /// 匀速。
    Linear,
    /// 慢慢加速。
    EaseIn,
    /// 慢慢减速。
    EaseOut,
    /// 两头慢、中间快（默认，动起来最自然）。
    #[default]
    EaseInOut,
}

impl Easing {
    /// 把 0–1 的进度换算成缓动后的进度（也在 0–1 之间，两端不变）。
    #[must_use]
    pub fn apply(self, u: f32) -> f32 {
        let u = u.clamp(0.0, 1.0);
        match self {
            Self::Linear => u,
            Self::EaseIn => u * u * u,
            Self::EaseOut => 1.0 - (1.0 - u).powi(3),
            Self::EaseInOut => {
                if u < 0.5 {
                    4.0 * u * u * u
                } else {
                    1.0 - 2.0f32.mul_add(-u, 2.0).powi(3) / 2.0
                }
            }
        }
    }
}

/// 一个关键帧。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    /// 相对片段起点的时间。
    pub at: Time,
    /// 值（单位和画面属性相同）。
    pub value: f32,
    /// 到下一个关键帧这一段的缓动。
    #[serde(default)]
    pub easing: Easing,
}

/// 能打关键帧的属性。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Prop {
    /// 水平位置。
    X,
    /// 垂直位置。
    Y,
    /// 缩放。
    Scale,
    /// 旋转。
    Rotation,
    /// 不透明度。
    Opacity,
    /// 左裁剪。
    CropLeft,
    /// 上裁剪。
    CropTop,
    /// 右裁剪。
    CropRight,
    /// 下裁剪。
    CropBottom,
    /// 亮度（调色，-1–1）。
    Brightness,
    /// 对比度。
    Contrast,
    /// 饱和度。
    Saturation,
    /// 色温。
    Temperature,
    /// 色调。
    Tint,
    /// 音量（dB，-60–12）。
    Volume,
    /// 圆角（外观）。
    Radius,
    /// 描边粗细。
    StrokeWidth,
    /// 阴影不透明度。
    ShadowOpacity,
    /// 阴影模糊。
    ShadowBlur,
    /// 阴影距离。
    ShadowDistance,
    /// 阴影角度（度）。
    ShadowAngle,
    /// 色度抠图强度。
    KeyIntensity,
    /// 色度抠图阴影。
    KeyShadow,
    /// 蒙版水平位置。
    MaskX,
    /// 蒙版垂直位置。
    MaskY,
    /// 蒙版宽。
    MaskWidth,
    /// 蒙版高。
    MaskHeight,
    /// 蒙版旋转（度）。
    MaskRotation,
    /// 蒙版羽化。
    MaskFeather,
    /// 蒙版圆角。
    MaskRound,
    /// 画面特效强度。
    EffectAmount,
}

/// 属性面板里一行一个关键帧按钮（剪映电脑版按属性打帧）：位置（X、Y 一起）、缩放、旋转、
/// 不透明度、裁剪（四边一起）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyGroup {
    /// 位置。
    Position,
    /// 缩放。
    Scale,
    /// 旋转。
    Rotation,
    /// 不透明度。
    Opacity,
    /// 裁剪。
    Crop,
    /// 调色的一项（「调整」标签每个滑条一组）。
    Adjust(AdjustItem),
    /// 音量（「音频」标签）。
    Volume,
    /// 外观的一项（圆角、描边、阴影、抠图、蒙版的滑条，[`Prop::LOOK`] 里的一个）。
    Look(Prop),
    /// 画面特效的强度。
    Effect,
}

impl KeyGroup {
    /// 画面的分组（属性面板「画面」里从上到下的顺序）。
    pub const ALL: [Self; 5] = [
        Self::Scale,
        Self::Position,
        Self::Rotation,
        Self::Opacity,
        Self::Crop,
    ];

    /// 这一组包含的属性。
    #[must_use]
    pub const fn props(self) -> &'static [Prop] {
        match self {
            Self::Position => &[Prop::X, Prop::Y],
            Self::Scale => &[Prop::Scale],
            Self::Rotation => &[Prop::Rotation],
            Self::Opacity => &[Prop::Opacity],
            Self::Crop => &[
                Prop::CropLeft,
                Prop::CropTop,
                Prop::CropRight,
                Prop::CropBottom,
            ],
            Self::Adjust(AdjustItem::Brightness) => &[Prop::Brightness],
            Self::Adjust(AdjustItem::Contrast) => &[Prop::Contrast],
            Self::Adjust(AdjustItem::Saturation) => &[Prop::Saturation],
            Self::Adjust(AdjustItem::Temperature) => &[Prop::Temperature],
            Self::Adjust(AdjustItem::Tint) => &[Prop::Tint],
            Self::Volume => &[Prop::Volume],
            Self::Effect => &[Prop::EffectAmount],
            Self::Look(p) => look_slice(p),
        }
    }
}

/// 一个外观属性自己组成的一组。
const fn look_slice(p: Prop) -> &'static [Prop] {
    match p {
        Prop::Radius => &[Prop::Radius],
        Prop::StrokeWidth => &[Prop::StrokeWidth],
        Prop::ShadowOpacity => &[Prop::ShadowOpacity],
        Prop::ShadowBlur => &[Prop::ShadowBlur],
        Prop::ShadowDistance => &[Prop::ShadowDistance],
        Prop::ShadowAngle => &[Prop::ShadowAngle],
        Prop::KeyIntensity => &[Prop::KeyIntensity],
        Prop::KeyShadow => &[Prop::KeyShadow],
        Prop::MaskX => &[Prop::MaskX],
        Prop::MaskY => &[Prop::MaskY],
        Prop::MaskWidth => &[Prop::MaskWidth],
        Prop::MaskHeight => &[Prop::MaskHeight],
        Prop::MaskRotation => &[Prop::MaskRotation],
        Prop::MaskFeather => &[Prop::MaskFeather],
        Prop::MaskRound => &[Prop::MaskRound],
        _ => &[],
    }
}

impl Prop {
    /// 画面属性的全部字段。
    pub const TRANSFORM: [Self; 9] = [
        Self::X,
        Self::Y,
        Self::Scale,
        Self::Rotation,
        Self::Opacity,
        Self::CropLeft,
        Self::CropTop,
        Self::CropRight,
        Self::CropBottom,
    ];

    /// 调色的五项。
    pub const ADJUST: [Self; 5] = [
        Self::Brightness,
        Self::Contrast,
        Self::Saturation,
        Self::Temperature,
        Self::Tint,
    ];

    /// 对应的调色项（不是调色属性时 `None`）。
    #[must_use]
    pub const fn adjust_item(self) -> Option<AdjustItem> {
        match self {
            Self::Brightness => Some(AdjustItem::Brightness),
            Self::Contrast => Some(AdjustItem::Contrast),
            Self::Saturation => Some(AdjustItem::Saturation),
            Self::Temperature => Some(AdjustItem::Temperature),
            Self::Tint => Some(AdjustItem::Tint),
            _ => None,
        }
    }

    /// 是不是画面属性（位置、缩放、旋转、不透明度、裁剪）。
    #[must_use]
    pub const fn is_transform(self) -> bool {
        matches!(
            self,
            Self::X
                | Self::Y
                | Self::Scale
                | Self::Rotation
                | Self::Opacity
                | Self::CropLeft
                | Self::CropTop
                | Self::CropRight
                | Self::CropBottom
        )
    }

    /// 从画面属性里读这个属性（不是画面属性时返回 0）。
    #[must_use]
    pub const fn get(self, t: &Transform) -> f32 {
        match self {
            Self::X => t.x,
            Self::Y => t.y,
            Self::Scale => t.scale,
            Self::Rotation => t.rotation,
            Self::Opacity => t.opacity,
            Self::CropLeft => t.crop.left,
            Self::CropTop => t.crop.top,
            Self::CropRight => t.crop.right,
            Self::CropBottom => t.crop.bottom,
            _ => 0.0,
        }
    }

    /// 把值写进画面属性（不是画面属性时什么都不做）。
    pub const fn set(self, t: &mut Transform, value: f32) {
        match self {
            Self::X => t.x = value,
            Self::Y => t.y = value,
            Self::Scale => t.scale = value,
            Self::Rotation => t.rotation = value,
            Self::Opacity => t.opacity = value,
            Self::CropLeft => t.crop.left = value,
            Self::CropTop => t.crop.top = value,
            Self::CropRight => t.crop.right = value,
            Self::CropBottom => t.crop.bottom = value,
            _ => {}
        }
    }

    /// 外观（圆角、描边、阴影、抠图、蒙版）的属性。
    pub const LOOK: [Self; 15] = [
        Self::Radius,
        Self::StrokeWidth,
        Self::ShadowOpacity,
        Self::ShadowBlur,
        Self::ShadowDistance,
        Self::ShadowAngle,
        Self::KeyIntensity,
        Self::KeyShadow,
        Self::MaskX,
        Self::MaskY,
        Self::MaskWidth,
        Self::MaskHeight,
        Self::MaskRotation,
        Self::MaskFeather,
        Self::MaskRound,
    ];

    /// 描边那一组。
    pub const STROKE: [Self; 1] = [Self::StrokeWidth];
    /// 阴影那一组。
    pub const SHADOW: [Self; 4] = [
        Self::ShadowOpacity,
        Self::ShadowBlur,
        Self::ShadowDistance,
        Self::ShadowAngle,
    ];
    /// 色度抠图那一组。
    pub const KEY: [Self; 2] = [Self::KeyIntensity, Self::KeyShadow];
    /// 蒙版那一组。
    pub const MASK: [Self; 7] = [
        Self::MaskX,
        Self::MaskY,
        Self::MaskWidth,
        Self::MaskHeight,
        Self::MaskRotation,
        Self::MaskFeather,
        Self::MaskRound,
    ];

    /// 外观里这个属性现在的值；这一项没开（没有描边、没有蒙版……）或不是外观属性时 `None`。
    #[must_use]
    pub fn look_get(self, l: &crate::Look) -> Option<f32> {
        let (stroke, shadow, key, mask) = (l.stroke, l.shadow, l.chroma, l.mask);
        match self {
            Self::Radius => Some(l.radius),
            Self::StrokeWidth => stroke.map(|v| v.width),
            Self::ShadowOpacity => shadow.map(|v| v.opacity),
            Self::ShadowBlur => shadow.map(|v| v.blur),
            Self::ShadowDistance => shadow.map(|v| v.distance),
            Self::ShadowAngle => shadow.map(|v| v.angle),
            Self::KeyIntensity => key.map(|v| v.intensity),
            Self::KeyShadow => key.map(|v| v.shadow),
            Self::MaskX => mask.map(|v| v.x),
            Self::MaskY => mask.map(|v| v.y),
            Self::MaskWidth => mask.map(|v| v.width),
            Self::MaskHeight => mask.map(|v| v.height),
            Self::MaskRotation => mask.map(|v| v.rotation),
            Self::MaskFeather => mask.map(|v| v.feather),
            Self::MaskRound => mask.map(|v| v.round),
            _ => None,
        }
    }

    /// 改外观里这个属性（这一项没开时不改）。
    pub fn look_set(self, l: &mut crate::Look, v: f32) {
        match self {
            Self::Radius => l.radius = v,
            Self::StrokeWidth => {
                if let Some(s) = l.stroke.as_mut() {
                    s.width = v;
                }
            }
            Self::ShadowOpacity | Self::ShadowBlur | Self::ShadowDistance | Self::ShadowAngle => {
                if let Some(s) = l.shadow.as_mut() {
                    match self {
                        Self::ShadowOpacity => s.opacity = v,
                        Self::ShadowBlur => s.blur = v,
                        Self::ShadowDistance => s.distance = v,
                        _ => s.angle = v,
                    }
                }
            }
            Self::KeyIntensity | Self::KeyShadow => {
                if let Some(c) = l.chroma.as_mut() {
                    if self == Self::KeyIntensity {
                        c.intensity = v;
                    } else {
                        c.shadow = v;
                    }
                }
            }
            _ => {
                if let Some(m) = l.mask.as_mut() {
                    match self {
                        Self::MaskX => m.x = v,
                        Self::MaskY => m.y = v,
                        Self::MaskWidth => m.width = v,
                        Self::MaskHeight => m.height = v,
                        Self::MaskRotation => m.rotation = v,
                        Self::MaskFeather => m.feather = v,
                        Self::MaskRound => m.round = v,
                        _ => {}
                    }
                }
            }
        }
    }

    /// 是外观属性。
    #[must_use]
    pub fn is_look(self) -> bool {
        Self::LOOK.contains(&self)
    }

    /// 这个属性单独看是否合法（裁剪之和另算）。
    fn accepts(self, v: f32) -> bool {
        v.is_finite()
            && match self {
                Self::X
                | Self::Y
                | Self::Rotation
                | Self::MaskX
                | Self::MaskY
                | Self::MaskRotation
                | Self::ShadowAngle => true,
                Self::Scale => (Transform::SCALE_RANGE.0..=Transform::SCALE_RANGE.1).contains(&v),
                Self::Opacity
                | Self::Radius
                | Self::ShadowOpacity
                | Self::KeyIntensity
                | Self::KeyShadow
                | Self::MaskFeather
                | Self::MaskRound
                | Self::EffectAmount => (0.0..=1.0).contains(&v),
                Self::CropLeft | Self::CropTop | Self::CropRight | Self::CropBottom => {
                    (0.0..1.0).contains(&v)
                }
                Self::Brightness
                | Self::Contrast
                | Self::Saturation
                | Self::Temperature
                | Self::Tint => (-1.0..=1.0).contains(&v),
                Self::Volume => (VOLUME_RANGE_DB.0..=VOLUME_RANGE_DB.1).contains(&v),
                Self::StrokeWidth | Self::ShadowBlur | Self::ShadowDistance => {
                    (0.0..=0.1).contains(&v)
                }
                Self::MaskWidth | Self::MaskHeight => (0.01..=4.0).contains(&v),
            }
    }
}

/// 片段的动画：按属性分开的关键帧。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Animation {
    tracks: BTreeMap<Prop, Vec<Key>>,
}

impl Animation {
    /// 空动画（常量版本，供 `const fn` 使用）。
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tracks: BTreeMap::new(),
        }
    }

    /// 没有任何关键帧。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// 某个属性的关键帧（按时间排好）。
    #[must_use]
    pub fn keys(&self, prop: Prop) -> &[Key] {
        self.tracks.get(&prop).map_or(&[], Vec::as_slice)
    }

    /// 有关键帧的属性。
    pub fn props(&self) -> impl Iterator<Item = Prop> + '_ {
        self.tracks.keys().copied()
    }

    /// 所有属性的关键帧时间（去重、排好）。
    #[must_use]
    pub fn times(&self) -> Vec<Time> {
        let mut all: Vec<Time> = self.tracks.values().flatten().map(|k| k.at).collect();
        all.sort_unstable();
        all.dedup();
        all
    }

    /// 换掉某个属性的全部关键帧（`keys` 为空时这个属性不再有动画）。`keys` 要按时间排好。
    pub fn set_keys(&mut self, prop: Prop, keys: Vec<Key>) {
        if keys.is_empty() {
            self.tracks.remove(&prop);
        } else {
            self.tracks.insert(prop, keys);
        }
    }

    /// 某个属性在 `at` 时刻的值；这个属性没有关键帧时返回 `None`。
    #[must_use]
    pub fn value_at(&self, prop: Prop, at: Time) -> Option<f32> {
        value_of(self.tracks.get(&prop)?, at)
    }

    /// `at` 时刻的外观：有关键帧的项按时间插值，其余用 `base`（那一项没开时不管）。
    #[must_use]
    pub fn apply_look(&self, base: &crate::Look, at: Time) -> crate::Look {
        let mut l = *base;
        for &prop in self.tracks.keys().filter(|p| p.is_look()) {
            if let Some(v) = self.value_at(prop, at) {
                prop.look_set(&mut l, v);
            }
        }
        l
    }

    /// `at` 时刻的调色：有关键帧的项按时间插值，其余用 `base`。
    #[must_use]
    pub fn apply_adjust(&self, base: &ColorAdjust, at: Time) -> ColorAdjust {
        let mut a = *base;
        for prop in Prop::ADJUST {
            if let (Some(item), Some(v)) = (prop.adjust_item(), self.value_at(prop, at)) {
                item.set(&mut a, v);
            }
        }
        a
    }

    /// `at` 时刻的画面属性：有关键帧的属性按时间插值，其余用 `base`。结果总是合法的
    /// （插值是凸组合；左右或上下裁剪各自插值后加起来可能 ≥ 1，按比例缩回去）。
    #[must_use]
    pub fn apply(&self, base: &Transform, at: Time) -> Transform {
        let mut t = *base;
        for &prop in self.tracks.keys().filter(|p| p.is_transform()) {
            if let Some(v) = self.value_at(prop, at) {
                prop.set(&mut t, v);
            }
        }
        // 两边裁剪各自插值，加起来可能 ≥ 1：按比例缩到刚好留一点画面
        let fit = |a: &mut f32, b: &mut f32| {
            let sum = *a + *b;
            if sum >= MAX_CROP_SUM {
                let k = MAX_CROP_SUM / sum;
                *a *= k;
                *b *= k;
            }
        };
        fit(&mut t.crop.left, &mut t.crop.right);
        fit(&mut t.crop.top, &mut t.crop.bottom);
        t
    }

    /// 在 `at` 处给属性设关键帧：已有就改值（缓动不变），没有就新建（默认缓动）。
    pub fn upsert(&mut self, prop: Prop, at: Time, value: f32) {
        let keys = self.tracks.entry(prop).or_default();
        match keys.binary_search_by_key(&at, |k| k.at) {
            Ok(i) => keys[i].value = value,
            Err(i) => keys.insert(
                i,
                Key {
                    at,
                    value,
                    easing: Easing::default(),
                },
            ),
        }
    }

    /// 若干属性的关键帧时间（去重、排好）。
    #[must_use]
    pub fn times_of(&self, props: &[Prop]) -> Vec<Time> {
        let mut all: Vec<Time> = props
            .iter()
            .flat_map(|p| self.keys(*p))
            .map(|k| k.at)
            .collect();
        all.sort_unstable();
        all.dedup();
        all
    }

    /// 删掉这些属性在 `at` 处的关键帧，返回是否删了；删光了的属性不再算有动画。
    pub fn remove_props_at(&mut self, props: &[Prop], at: Time) -> bool {
        let mut removed = false;
        for prop in props {
            if let Some(keys) = self.tracks.get_mut(prop) {
                let before = keys.len();
                keys.retain(|k| k.at != at);
                removed |= keys.len() != before;
            }
        }
        self.tracks.retain(|_, keys| !keys.is_empty());
        removed
    }

    /// 所有关键帧的时间乘 `factor`（变速时动画跟着内容伸缩）。
    pub fn scale_times(&mut self, factor: f64) {
        for keys in self.tracks.values_mut() {
            for k in keys.iter_mut() {
                k.at = k.at.scaled(factor);
            }
        }
    }

    /// 删掉一个属性在 `[from, to]` 之间的关键帧（含两端）。
    pub fn remove_between(&mut self, prop: Prop, from: Time, to: Time) {
        if let Some(keys) = self.tracks.get_mut(&prop) {
            keys.retain(|k| k.at < from || k.at > to);
        }
        self.tracks.retain(|_, keys| !keys.is_empty());
    }

    /// 删掉所有属性在 `at` 处的关键帧，返回是否删了；属性的关键帧删光了就不再算有动画。
    pub fn remove_at(&mut self, at: Time) -> bool {
        let mut removed = false;
        for keys in self.tracks.values_mut() {
            let before = keys.len();
            keys.retain(|k| k.at != at);
            removed |= keys.len() != before;
        }
        self.tracks.retain(|_, keys| !keys.is_empty());
        removed
    }

    /// `at` 处关键帧的缓动（各属性一样时；没有关键帧时 `None`）。
    #[must_use]
    pub fn easing_at(&self, at: Time) -> Option<Easing> {
        self.tracks
            .values()
            .flatten()
            .find(|k| k.at == at)
            .map(|k| k.easing)
    }

    /// 这些属性在 `at` 处关键帧的缓动（没有关键帧时 `None`）。
    #[must_use]
    pub fn easing_of(&self, props: &[Prop], at: Time) -> Option<Easing> {
        props
            .iter()
            .flat_map(|p| self.keys(*p))
            .find(|k| k.at == at)
            .map(|k| k.easing)
    }

    /// 把 `at` 处所有属性关键帧的缓动改成 `easing`，返回是否改了。
    pub fn set_easing(&mut self, at: Time, easing: Easing) -> bool {
        let props: Vec<Prop> = self.props().collect();
        self.set_easing_of(&props, at, easing)
    }

    /// 只改这些属性在 `at` 处关键帧的缓动，返回是否改了。
    pub fn set_easing_of(&mut self, props: &[Prop], at: Time, easing: Easing) -> bool {
        let mut changed = false;
        for (_, keys) in self.tracks.iter_mut().filter(|(p, _)| props.contains(p)) {
            for k in keys.iter_mut().filter(|k| k.at == at) {
                k.easing = easing;
                changed = true;
            }
        }
        changed
    }

    /// 所有关键帧平移 `delta`（修剪片段起点时，关键帧跟着内容走）。
    pub fn shift(&mut self, delta: Time) {
        for k in self.tracks.values_mut().flatten() {
            k.at += delta;
        }
    }

    /// 在 `at` 处切成两段：左段保留之前的关键帧并在切点补一个当时的值；
    /// 右段在 0 处补同样的值，再接上之后的关键帧（时间平移）。
    #[must_use]
    pub fn split(&self, at: Time) -> (Self, Self) {
        let (mut left, mut right) = (Self::default(), Self::default());
        for (&prop, keys) in &self.tracks {
            let Some(value) = self.value_at(prop, at) else {
                continue;
            };
            // 切点所在这一段的缓动（切点在第一个关键帧之前时用第一个的）
            let easing = keys
                .iter()
                .rev()
                .find(|k| k.at <= at)
                .or_else(|| keys.first())
                .map_or_else(Easing::default, |k| k.easing);
            let cut = Key { at, value, easing };
            let mut l: Vec<Key> = keys.iter().copied().filter(|k| k.at < at).collect();
            l.push(cut);
            let mut r = vec![Key {
                at: Time::ZERO,
                ..cut
            }];
            r.extend(keys.iter().filter(|k| k.at > at).map(|k| Key {
                at: k.at - at,
                ..*k
            }));
            left.tracks.insert(prop, l);
            right.tracks.insert(prop, r);
        }
        (left, right)
    }

    /// 所有关键帧的缩放里最大的（没有缩放关键帧时 `None`）。
    #[must_use]
    pub fn max_value(&self, prop: Prop) -> Option<f32> {
        self.tracks
            .get(&prop)?
            .iter()
            .map(|k| k.value)
            .reduce(f32::max)
    }

    /// 是否满足不变式：时间排好、不重复，值都是有限数且对属性单独合法。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.tracks.iter().all(|(&prop, keys)| {
            !keys.is_empty()
                && keys.windows(2).all(|w| w[0].at < w[1].at)
                && keys.iter().all(|k| prop.accepts(k.value))
        })
    }
}

/// 两边裁剪加起来的上限（略小于 1，总留一点画面）。
const MAX_CROP_SUM: f32 = 0.999;

/// 音量的范围（dB）。
pub const VOLUME_RANGE_DB: (f32, f32) = (-60.0, 12.0);

/// 一串排好的关键帧在 `at` 时刻的值（没有关键帧时 `None`）：两端之外取端点的值，
/// 中间按前一个关键帧的缓动插值。
#[must_use]
pub fn value_of(keys: &[Key], at: Time) -> Option<f32> {
    let (first, last) = (keys.first()?, keys.last()?);
    if at <= first.at {
        return Some(first.value);
    }
    if at >= last.at {
        return Some(last.value);
    }
    // 第一个时间大于 at 的关键帧；前一个就是这一段的起点
    let next = keys.partition_point(|k| k.at <= at);
    let (a, b) = (keys[next - 1], keys[next]);
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "进度在 0–1 之间，f32 精度足够"
    )]
    let u = ((at - a.at).0 as f64 / (b.at - a.at).0 as f64) as f32;
    Some((b.value - a.value).mul_add(a.easing.apply(u), a.value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const fn s(secs: i64) -> Time {
        Time::from_secs(secs)
    }

    fn key(at: Time, value: f32, easing: Easing) -> Key {
        Key { at, value, easing }
    }

    /// 缩放：0 秒 1.0（匀速）→ 2 秒 2.0。
    fn zoom() -> Animation {
        let mut a = Animation::default();
        a.upsert(Prop::Scale, s(0), 1.0);
        a.upsert(Prop::Scale, s(2), 2.0);
        assert!(a.set_easing(s(0), Easing::Linear));
        a
    }

    #[test]
    fn easing_curves_keep_ends_and_shape() {
        for e in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
        ] {
            assert_eq!(e.apply(0.0), 0.0, "{e:?}");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-6, "{e:?}");
            let mut last = 0.0;
            for i in 0..=20 {
                #[expect(clippy::cast_precision_loss, reason = "测试")]
                let v = e.apply(i as f32 / 20.0);
                assert!(
                    (0.0..=1.0).contains(&v) && v >= last,
                    "{e:?} 单调、在 0–1 内"
                );
                last = v;
            }
        }
        assert!((Easing::Linear.apply(0.25) - 0.25).abs() < 1e-6);
        assert!(Easing::EaseIn.apply(0.5) < 0.5);
        assert!(Easing::EaseOut.apply(0.5) > 0.5);
        assert!((Easing::EaseInOut.apply(0.5) - 0.5).abs() < 1e-6);
        // 超出范围的进度先夹到 0–1
        assert_eq!(Easing::EaseIn.apply(-1.0), 0.0);
        assert!((Easing::EaseIn.apply(2.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn values_interpolate_and_hold_outside_keys() {
        let a = zoom();
        assert!(!a.is_empty());
        assert_eq!(a.value_at(Prop::Scale, s(1)), Some(1.5));
        assert_eq!(
            a.value_at(Prop::Scale, s(-1)),
            Some(1.0),
            "第一个之前取第一个"
        );
        assert_eq!(
            a.value_at(Prop::Scale, s(5)),
            Some(2.0),
            "最后一个之后取最后一个"
        );
        assert_eq!(a.value_at(Prop::X, s(1)), None, "没有关键帧的属性");
        let mut one = Animation::default();
        one.upsert(Prop::X, s(3), 0.25);
        assert_eq!(
            one.value_at(Prop::X, s(0)),
            Some(0.25),
            "只有一个关键帧：常数"
        );
    }

    #[test]
    fn apply_overrides_only_animated_props() {
        let base = Transform {
            x: 0.1,
            ..Transform::default()
        };
        let t = zoom().apply(&base, s(1));
        assert!((t.scale - 1.5).abs() < 1e-6);
        assert!((t.x - 0.1).abs() < 1e-6, "没动画的属性用固定值");
        assert_eq!(Animation::default().apply(&base, s(1)), base);
    }

    #[test]
    fn upsert_remove_easing_and_times() {
        let mut a = zoom();
        a.upsert(Prop::X, s(1), 0.5);
        assert_eq!(a.times(), vec![s(0), s(1), s(2)]);
        // 同一时刻再设：改值、缓动不变
        a.upsert(Prop::Scale, s(0), 1.2);
        assert_eq!(a.keys(Prop::Scale)[0], key(s(0), 1.2, Easing::Linear));
        assert_eq!(a.easing_at(s(0)), Some(Easing::Linear));
        assert_eq!(
            a.easing_at(s(1)),
            Some(Easing::EaseInOut),
            "新建的用默认缓动"
        );
        assert_eq!(a.easing_at(s(7)), None);
        // 删 1 秒处：X 的唯一关键帧没了，X 不再算有动画
        assert!(a.remove_at(s(1)));
        assert!(!a.remove_at(s(1)));
        assert_eq!(a.props().collect::<Vec<_>>(), vec![Prop::Scale]);
        assert!(!a.set_easing(s(9), Easing::EaseOut));
        assert!(a.remove_at(s(0)) && a.remove_at(s(2)));
        assert!(a.is_empty());
    }

    #[test]
    fn shift_and_split_keep_the_motion() {
        let mut a = zoom();
        a.shift(s(-1));
        assert_eq!(a.times(), vec![s(-1), s(1)]);
        let (left, right) = zoom().split(s(1));
        assert_eq!(left.times(), vec![s(0), s(1)]);
        assert_eq!(right.times(), vec![s(0), s(1)]);
        assert_eq!(
            left.value_at(Prop::Scale, Time::from_millis(500)),
            Some(1.25)
        );
        assert_eq!(
            right.value_at(Prop::Scale, Time::from_millis(500)),
            Some(1.75)
        );
        assert_eq!(left.value_at(Prop::Scale, s(1)), Some(1.5));
        assert_eq!(right.value_at(Prop::Scale, s(0)), Some(1.5));
        // 切点正好在关键帧上：不重复补
        let (l2, r2) = zoom().split(s(2));
        assert_eq!(l2.times(), vec![s(0), s(2)]);
        assert_eq!(r2.times(), vec![s(0)]);
    }

    #[test]
    fn max_value_and_validity() {
        let a = zoom();
        assert_eq!(a.max_value(Prop::Scale), Some(2.0));
        assert_eq!(a.max_value(Prop::X), None);
        assert!(a.is_valid());
        let bad = |prop: Prop, value: f32| {
            let mut b = Animation::default();
            b.upsert(prop, s(0), value);
            b.is_valid()
        };
        assert!(!bad(Prop::Scale, 0.0));
        assert!(!bad(Prop::Opacity, 1.5));
        assert!(!bad(Prop::CropLeft, -0.1));
        assert!(!bad(Prop::CropTop, 1.0));
        assert!(!bad(Prop::X, f32::NAN));
        // 时间重复 / 没排好：反序列化进来的坏数据
        let unsorted: Animation =
            serde_json::from_str(r#"{"X":[{"at":2,"value":0.0},{"at":1,"value":0.0}]}"#).unwrap();
        assert!(!unsorted.is_valid());
        let dup: Animation =
            serde_json::from_str(r#"{"X":[{"at":1,"value":0.0},{"at":1,"value":0.5}]}"#).unwrap();
        assert!(!dup.is_valid());
        let empty: Animation = serde_json::from_str(r#"{"X":[]}"#).unwrap();
        assert!(!empty.is_valid(), "空的关键帧列表不该出现");
    }

    #[test]
    fn crop_pairs_stay_below_full_frame() {
        // 左裁剪快速变大、右裁剪慢慢变小：中间两者加起来会超过 1，求值时要缩回去
        let mut a = Animation::default();
        a.upsert(Prop::CropLeft, s(0), 0.0);
        a.upsert(Prop::CropLeft, s(2), 0.9);
        a.upsert(Prop::CropRight, s(0), 0.9);
        a.upsert(Prop::CropRight, s(2), 0.0);
        assert!(a.set_easing(s(0), Easing::EaseOut));
        a.upsert(Prop::CropRight, s(0), 0.9);
        let mut b = a.clone();
        b.tracks.get_mut(&Prop::CropRight).unwrap()[0].easing = Easing::EaseIn;
        let t = b.apply(&Transform::default(), s(1));
        assert!(t.is_valid(), "{t:?}");
        assert!(t.crop.left + t.crop.right < 1.0);
    }

    #[test]
    fn prop_get_set_round_trip() {
        let mut t = Transform::default();
        for (i, p) in Prop::TRANSFORM.into_iter().enumerate() {
            #[expect(clippy::cast_precision_loss, reason = "测试")]
            let v = 0.01 * (i + 1) as f32;
            p.set(&mut t, v);
            assert!((p.get(&t) - v).abs() < 1e-6, "{p:?}");
        }
        assert!((t.crop.bottom - 0.09).abs() < 1e-6 && (t.rotation - 0.04).abs() < 1e-6);
    }

    #[test]
    fn serde_round_trip() {
        let a = zoom();
        let json = serde_json::to_string(&a).unwrap();
        let back: Animation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a);
    }

    proptest! {
        #[test]
        fn evaluation_is_always_valid(
            keys in proptest::collection::vec(
                (0i64..100, 0.0f32..0.6, 0.0f32..0.6, 0.01f32..10.0, 0.0f32..1.0, 0usize..4),
                1..8,
            ),
            at in -10i64..120,
        ) {
            let easings = [Easing::Linear, Easing::EaseIn, Easing::EaseOut, Easing::EaseInOut];
            let mut a = Animation::default();
            for (t, left, right, scale, opacity, e) in keys {
                let when = Time::from_millis(t * 100);
                a.upsert(Prop::CropLeft, when, left);
                a.upsert(Prop::CropRight, when, right);
                a.upsert(Prop::Scale, when, scale);
                a.upsert(Prop::Opacity, when, opacity);
                a.set_easing(when, easings[e]);
            }
            prop_assert!(a.is_valid());
            let t = a.apply(&Transform::default(), Time::from_millis(at * 100));
            prop_assert!(t.is_valid(), "{:?}", t);
        }
    }
}
