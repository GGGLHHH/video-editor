//! 画面合成要用的解析：某个时间点上，各视频轨各露出哪个片段的哪一刻。
//!
//! 预览和导出都从这里取图层，保证两边取到的是同一组画面。

use crate::{AssetId, Clip, ClipId, Project, Time, Track, TrackId, Transform};

/// 时间点上的一个画面图层。
#[derive(Debug, Clone, PartialEq)]
pub struct LayerSource {
    /// 所在轨道。
    pub track: TrackId,
    /// 素材。
    pub asset: AssetId,
    /// 片段。
    pub clip: ClipId,
    /// 素材内时间。
    pub source_time: Time,
    /// 这一刻的画面属性（有关键帧动画时已经求过值）。
    pub transform: Transform,
    /// 整个片段里最大的缩放（导出按它决定解码尺寸，放大时画面不糊）。
    pub max_scale: f32,
    /// 调色：片段自己的调色，再接上压在它上面的调节片段（[`crate::ColorMatrix`]）。
    pub color: crate::ColorMatrix,
    /// 同一条轨上相邻的片段交替用 0 / 1：转场时前后两段同时出现，各用各的解码器。
    pub lane: u8,
    /// 外观：混合模式、圆角、描边、阴影、蒙版、色度抠图。
    pub look: crate::Look,
    /// 整条调色链（片段自己的，再接上压在它上面的调节片段）；只在用到进阶调色（HSL、曲线、
    /// 色轮、LUT）时才有，没有时按 [`Self::color`] 的矩阵调色。
    pub grades: crate::GradeChain,
    /// 锐化、颗粒、暗角（片段自己的「调整」）。
    pub detail: crate::adjust::Detail,
    /// 帧融合：慢放时把 `source_time` 前后两帧按时间混合（[`crate::Clip::blends_frames`]）。
    pub frame_blend: bool,
    /// 文字蒙版的字（外观里有文字形状的蒙版时才有）。
    pub mask_text: Option<std::sync::Arc<crate::MaskText>>,
}

/// 时间点 `t` 上要合成的东西：各层画面和底色的调色。
#[derive(Debug, Clone, PartialEq)]
pub struct Composition {
    /// 画面图层，从最下层到最上层。
    pub layers: Vec<LayerSource>,
    /// 黑底要做的调色（调节片段会把没有画面的地方也调了）。
    pub background: crate::ColorMatrix,
    /// 黑底的调色链（调节片段用了进阶调色时才有）。
    pub background_grades: crate::GradeChain,
    /// 画面特效，按从下到上的顺序（[`crate::EffectPass::after`] 指它压在哪几层上面）。
    pub effects: Vec<crate::EffectPass>,
    /// 半透明的复合片段：里面的几层要先合成、再整组一起半透明（按开始的层排好，同一层开始的
    /// 外层在前）。
    pub groups: Vec<GroupPass>,
}

/// 一段整组半透明的图层（复合片段里面那几层）：先照常把第 `start..end` 层（和压在里面的前
/// `effects_end` 道特效）画上去，再和画这组之前的画面按 `opacity` 混合——对普通叠加的图层，
/// 这正好等于「整组合成好再按不透明度叠上去」，重叠处不会透出下面那层。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupPass {
    /// 第一层。
    pub start: usize,
    /// 最后一层的下一层。
    pub end: usize,
    /// 组里的特效做到第几道为止（[`Composition::effects`] 的下标，不含）。
    pub effects_end: usize,
    /// 整组的不透明度。
    pub opacity: f32,
    /// 复合片段的蒙版、圆角（有的话）：组外面的部分露出下面（[`GroupShape`]）。
    pub shape: Option<GroupShape>,
}

/// 复合片段整组的形状：它是一个画布大小的图层，按 `transform`（已经和外面几层复合片段叠好）
/// 放在画布上，`look` 里的蒙版、圆角裁这一层（描边、阴影、混合模式、抠图对整组不生效）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupShape {
    /// 整组在画布上的位置大小（不透明度另算）。
    pub transform: Transform,
    /// 外观（只用蒙版和圆角）。
    pub look: crate::Look,
}

impl GroupPass {
    /// 有些图层这一帧没画（`kept` 为假）时，按实际画了的层重新数 `start`、`end`；组里一层都
    /// 没画的丢掉。
    #[must_use]
    pub fn remap(groups: &[Self], kept: &[bool]) -> Vec<Self> {
        let count = |n: usize| kept.iter().take(n).filter(|&&k| k).count();
        groups
            .iter()
            .map(|g| Self {
                start: count(g.start),
                end: count(g.end),
                ..*g
            })
            .filter(|g| g.end > g.start)
            .collect()
    }
}

/// 时间点 `t` 上所有视频轨的画面，从最下层到最上层（合成顺序）。
#[must_use]
pub fn resolve_layers(project: &Project, t: Time) -> Vec<LayerSource> {
    resolve_frame(project, t).layers
}

/// 时间点 `t` 上的合成：从下往上一条条视频轨看，素材片段是一层画面；调节片段不出画面，
/// 而是把它的调色接在下面已有的每一层和黑底后面（调节层）。
#[must_use]
pub fn resolve_frame(project: &Project, t: Time) -> Composition {
    let frame = project.settings.frame_rate.frame_duration();
    let canvas = (project.settings.width, project.settings.height);
    let mut out: Vec<LayerSource> = Vec::new();
    let mut background = crate::ColorMatrix::IDENTITY;
    let mut background_grades: crate::GradeChain = Vec::new();
    let mut effects = Vec::new();
    // 每层、每道特效外面套着哪些复合片段（从外到里，带这一刻的不透明度和形状）
    let mut layer_chains: Vec<Vec<Outer>> = Vec::new();
    let mut effect_chains: Vec<Vec<Outer>> = Vec::new();
    // 每层外面各层复合片段自己的调色（从里到外），和调节片段（压在哪几层上、在哪些复合片段里面）：
    // 最后按「里面的调节片段 → 复合片段自己 → 外面的调节片段」的顺序接到每层后面
    let mut layer_parent_grades: Vec<Vec<ParentGrade>> = Vec::new();
    let mut adjustments: Vec<Adjustment> = Vec::new();
    for tr in project
        .timeline
        .video_tracks_bottom_up()
        .filter(|tr| tr.shows())
    {
        if let Some(mut pair) = transition_layers(tr, t, frame) {
            for layer in &mut pair {
                let clip = tr.clips.iter().find(|c| c.id == layer.clip);
                if let Some(clip) = clip {
                    under_parents(layer, clip, t, canvas);
                }
                layer_chains.push(clip.map(|c| parents_of(c, t, canvas)).unwrap_or_default());
                layer_parent_grades.push(clip.map(|c| parent_grades(c, t)).unwrap_or_default());
            }
            out.extend(pair);
            continue;
        }
        let Some(i) = tr.clips.iter().position(|c| c.start <= t && t < c.end()) else {
            continue;
        };
        let clip = &tr.clips[i];
        if !clip.enabled() {
            continue;
        }
        if let Some(effect) = clip.effect_at(t - clip.start) {
            #[expect(clippy::cast_possible_truncation, reason = "片段内的秒数")]
            let seconds = (t - clip.start).as_secs_f64() as f32 * effect.time_scale();
            effects.push(crate::EffectPass {
                after: out.len(),
                effect,
                seconds,
            });
            effect_chains.push(parents_of(clip, t, canvas));
            continue;
        }
        if clip.is_adjustment() {
            let a = clip.adjust_at(t - clip.start);
            let m = a.matrix();
            let grade = clip.grade_at(t - clip.start);
            if !m.is_identity() || grade.is_some() || a.tone().is_some() {
                let step = crate::GradeStep {
                    input: None,
                    matrix: m,
                    tone: a.tone(),
                    grade,
                };
                background = background.then(&m);
                background_grades.push(step.clone());
                adjustments.push(Adjustment {
                    below: out.len(),
                    inside: parents_of(clip, t, canvas)
                        .into_iter()
                        .map(|o| o.id)
                        .collect(),
                    matrix: m,
                    step,
                });
            }
            continue;
        }
        if let Some(mut layer) = layer_of(tr.id, clip, i, t - clip.start, None) {
            under_parents(&mut layer, clip, t, canvas);
            out.push(layer);
            layer_chains.push(parents_of(clip, t, canvas));
            layer_parent_grades.push(parent_grades(clip, t));
        }
    }
    grade_in_order(&mut out, &layer_parent_grades, &adjustments);
    // 整条链都没用进阶调色：只留矩阵（着色器走快的那条路）
    let plain = |chain: &crate::GradeChain| {
        chain
            .iter()
            .all(|s| s.grade.is_none() && s.input.is_none() && s.tone.is_none())
    };
    for l in &mut out {
        if plain(&l.grades) {
            l.grades.clear();
        }
    }
    if plain(&background_grades) {
        background_grades.clear();
    }
    let groups = groups_of(&layer_chains, &effects, &effect_chains);
    Composition {
        layers: out,
        background,
        background_grades,
        effects,
        groups,
    }
}

/// 给每层接上调色：调节片段按从下到上的顺序；调节片段不在里面的那几层复合片段，它们自己的
/// 调色在它之前做（里面的调节片段 → 复合片段自己 → 外面的调节片段）。
fn grade_in_order(
    layers: &mut [LayerSource],
    parents: &[Vec<ParentGrade>],
    adjustments: &[Adjustment],
) {
    let push = |l: &mut LayerSource, m: &crate::ColorMatrix, step: &crate::GradeStep| {
        l.color = l.color.then(m);
        l.grades.push(step.clone());
    };
    for (i, (l, own)) in layers.iter_mut().zip(parents).enumerate() {
        let mut pending = own.iter().peekable();
        for a in adjustments.iter().filter(|a| a.below > i) {
            while let Some(p) = pending.next_if(|p| !a.inside.contains(&p.clip)) {
                push(l, &p.matrix, &p.step);
            }
            push(l, &a.matrix, &a.step);
        }
        for p in pending {
            push(l, &p.matrix, &p.step);
        }
    }
}

/// 外面一层复合片段自己的调色。
struct ParentGrade {
    clip: ClipId,
    matrix: crate::ColorMatrix,
    step: crate::GradeStep,
}

/// 一个调节片段：压在前 `below` 层上，在 `inside` 这些复合片段里面。
struct Adjustment {
    below: usize,
    inside: Vec<ClipId>,
    matrix: crate::ColorMatrix,
    step: crate::GradeStep,
}

/// 片段外面各层复合片段这一刻自己的调色（从里到外，没调色的跳过）。
fn parent_grades(clip: &Clip, t: Time) -> Vec<ParentGrade> {
    let mut out = Vec::new();
    let mut next = clip.parent.as_deref();
    while let Some(p) = next {
        let at = (t - p.start).clamp(Time::ZERO, p.duration);
        let a = p.adjust_at(at);
        let m = a.matrix();
        let grade = p.grade_at(at);
        if !m.is_identity() || grade.is_some() || a.tone().is_some() {
            out.push(ParentGrade {
                clip: p.id,
                matrix: m,
                step: crate::GradeStep {
                    input: None,
                    matrix: m,
                    tone: a.tone(),
                    grade,
                },
            });
        }
        next = p.parent.as_deref();
    }
    out
}

/// 外面一层复合片段在这一刻：编号、不透明度、整组的形状（有蒙版或圆角时）。
#[derive(Debug, Clone, Copy)]
struct Outer {
    id: ClipId,
    opacity: f32,
    shape: Option<GroupShape>,
}

/// 片段外面套着的复合片段（从外到里）。形状的位置是外面几层叠起来的（画布 `w`×`h`）。
fn parents_of(clip: &Clip, t: Time, (w, h): (u32, u32)) -> Vec<Outer> {
    let mut chain = Vec::new();
    let mut next = clip.parent.as_deref();
    while let Some(p) = next {
        let at = (t - p.start).clamp(Time::ZERO, p.duration);
        chain.push((p.id, p.transform_at(at), p.look_at(at)));
        next = p.parent.as_deref();
    }
    chain.reverse();
    let mut placed: Option<Transform> = None;
    chain
        .into_iter()
        .map(|(id, mut own, look)| {
            let opacity = own.opacity;
            own.opacity = 1.0;
            let transform = placed.map_or(own, |outer| outer.compose(&own, w, h));
            placed = Some(transform);
            let shaped = look.mask.is_some() || look.radius > 0.0;
            Outer {
                id,
                opacity,
                shape: shaped.then_some(GroupShape { transform, look }),
            }
        })
        .collect()
}

/// 半透明复合片段的分组：每一层嵌套里，套着同一个复合片段的连续几层是一组（摊开时里面的
/// 轨道挨着放，同一个复合片段的层一定连着）。完全不透明的不用分组。
fn groups_of(
    layers: &[Vec<Outer>],
    effects: &[crate::EffectPass],
    effect_chains: &[Vec<Outer>],
) -> Vec<GroupPass> {
    let depth = layers.iter().map(Vec::len).max().unwrap_or(0);
    let mut groups = Vec::new();
    for d in 0..depth {
        let mut i = 0;
        while i < layers.len() {
            let Some(&Outer { id, opacity, shape }) = layers[i].get(d) else {
                i += 1;
                continue;
            };
            let end = (i + 1..layers.len())
                .find(|&j| layers[j].get(d).map(|x| x.id) != Some(id))
                .unwrap_or(layers.len());
            if opacity < 1.0 - 1e-6 || shape.is_some() {
                // 组里的特效：压在组里最后一层上、又在这个复合片段里面的那几道
                let inside = |k: usize| effect_chains[k].get(d).map(|x| x.id) == Some(id);
                let effects_end = effects
                    .iter()
                    .enumerate()
                    .position(|(k, e)| e.after > end || (e.after == end && !inside(k)))
                    .unwrap_or(effects.len());
                groups.push(GroupPass {
                    start: i,
                    end,
                    effects_end,
                    opacity,
                    shape,
                });
            }
            i = end;
        }
    }
    // 先开始的在前；同一层开始的，外层（更长的）在前
    groups.sort_by_key(|g| (g.start, std::cmp::Reverse(g.end)));
    groups
}

/// 片段内 `offset` 时刻的图层；`transform` 给了就用它（转场时改过的属性）。
/// 摊开的复合片段里的一层：从里往外套上外面各层复合片段这一刻的画面属性（位置、缩放、旋转、
/// 不透明度、镜像）和调色（[`Clip::parent`]）。
fn under_parents(layer: &mut LayerSource, clip: &Clip, t: Time, (w, h): (u32, u32)) {
    let mut next = clip.parent.as_deref();
    while let Some(p) = next {
        let at = (t - p.start).clamp(Time::ZERO, p.duration);
        // 整组的不透明度不乘到每一层上（重叠处会互相透出来），另外按组算（[`GroupPass`]）
        let mut group = p.transform_at(at);
        group.opacity = 1.0;
        layer.transform = group.compose(&layer.transform, w, h);
        layer.max_scale *= p
            .animation
            .max_value(crate::Prop::Scale)
            .map_or(p.transform.scale, |m| m.max(p.transform.scale));
        // 调色另外按顺序接（[`parent_grades`]）
        next = p.parent.as_deref();
    }
}

fn layer_of(
    track: TrackId,
    clip: &Clip,
    index: usize,
    offset: Time,
    transform: Option<Transform>,
) -> Option<LayerSource> {
    Some(LayerSource {
        track,
        asset: clip.asset()?,
        clip: clip.id,
        source_time: clip.source_at(offset),
        transform: transform.unwrap_or_else(|| clip.transform_at(offset)),
        max_scale: clip
            .animation
            .max_value(crate::Prop::Scale)
            .map_or(clip.transform.scale, |m| m.max(clip.transform.scale))
            * clip.motion.max_scale()
            * clip.transform.stretch.max(1.0),
        color: clip.adjust_at(offset).matrix(),
        lane: u8::from(index % 2 == 1),
        look: clip.look_at(offset),
        grades: vec![crate::GradeStep {
            input: clip.input_space,
            matrix: clip.adjust_at(offset).matrix(),
            tone: clip.adjust_at(offset).tone(),
            grade: clip.grade_at(offset),
        }],
        detail: {
            let a = clip.adjust_at(offset);
            // 颗粒每 1/24 秒换一次图案
            #[expect(clippy::cast_possible_truncation, reason = "0–96 的整数")]
            let seed = ((clip.source_at(offset).as_secs_f64() * 24.0).floor() % 97.0) as f32;
            crate::adjust::Detail { seed, ..a.detail() }
        },
        frame_blend: clip.blends_frames(),
        mask_text: {
            let look = clip.look_at(offset);
            look.uses_text().then(|| clip.mask_text.clone()).flatten()
        },
    })
}

/// `t` 落在这条轨某个转场里时，返回（前一段，后一段）两层；切点前后各占一半时长，
/// 前一段过了切点停在最后一帧，后一段在切点前停在第一帧。
fn transition_layers(track: &Track, t: Time, frame: Time) -> Option<[LayerSource; 2]> {
    let clips = &track.clips;
    // 后一段的下标：t 所在片段自己有转场且 t 在它的前半个转场里，或者下一段的转场伸到了 t
    let here = clips.iter().position(|c| c.start <= t && t < c.end());
    let candidates = here.into_iter().flat_map(|i| [i, i + 1]);
    for next in candidates {
        let (Some(incoming), Some(prev)) = (
            clips.get(next),
            next.checked_sub(1).and_then(|i| clips.get(i)),
        ) else {
            continue;
        };
        let Some((tr, span)) = transition_between(prev, incoming) else {
            continue;
        };
        let half = Time(span.0 / 2);
        let (from, to) = (incoming.start - half, incoming.start - half + span);
        if t < from || t >= to || span <= Time::ZERO {
            continue;
        }
        #[expect(clippy::cast_precision_loss, reason = "转场进度只要大致精确")]
        let progress = (t - from).0 as f32 / span.0 as f32;
        let out_offset = (t - prev.start).min(prev.duration - frame).max(Time::ZERO);
        let in_offset = (t - incoming.start).max(Time::ZERO);
        let (ot, it) = tr.kind.blend(
            progress,
            prev.transform_at(out_offset),
            incoming.transform_at(in_offset),
        );
        let first = layer_of(track.id, prev, next - 1, out_offset, Some(ot))?;
        let second = layer_of(track.id, incoming, next, in_offset, Some(it))?;
        return Some([first, second]);
    }
    None
}

/// `incoming` 挂的转场在它和 `prev` 之间真正起作用时，返回（转场，实际时长）：两段要紧挨着、
/// 都没停用，时长不超过任何一段。
fn transition_between(prev: &Clip, incoming: &Clip) -> Option<(crate::Transition, Time)> {
    let tr = incoming.transition_in?;
    // 两段都得是素材片段（调节片段没有画面，做不了转场）
    if prev.end() != incoming.start
        || !prev.enabled()
        || !incoming.enabled()
        || prev.asset().is_none()
        || incoming.asset().is_none()
    {
        return None;
    }
    let span = tr.duration.min(prev.duration).min(incoming.duration);
    (span > Time::ZERO).then_some((tr, span))
}

/// 片段的声音在时间轴上实际放的那一段：转场处会越过片段边界，两段的声音叠在一起交叉淡化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSpan {
    /// 开始放的时刻（淡入从这里起）。
    pub start: Time,
    /// 放完的时刻（淡出到这里止，不含）。
    pub end: Time,
    /// 淡入长度。
    pub fade_in: Time,
    /// 淡出长度。
    pub fade_out: Time,
}

/// 一小段声音：时间轴上从 `start` 放 `len` 长，素材从 `source_start` 起、按 `speed` 倍速。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioPiece {
    /// 时间轴上的起点。
    pub start: Time,
    /// 长度（时间轴时间）。
    pub len: Time,
    /// 素材里的起点。
    pub source_start: Time,
    /// 倍速（素材时间 / 时间轴时间）。
    pub speed: f32,
}

/// 曲线变速的声音分成多少小段（每段按那一段的平均倍数放；预览和导出同一套切法）。
pub const CURVE_AUDIO_PIECES: i64 = 16;

/// 片段在 `span` 里要放的声音，切成若干段：普通片段一整段；曲线变速的片段按
/// [`CURVE_AUDIO_PIECES`] 等分（越过片段边界的部分并进头尾两段）。
#[must_use]
pub fn audio_pieces(clip: &Clip, span: &AudioSpan) -> Vec<AudioPiece> {
    let piece = |start: Time, end: Time| {
        let len = end - start;
        let a = clip.source_at(start - clip.start).max(Time::ZERO);
        let b = clip.source_at(end - clip.start).max(a);
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "倍数"
        )]
        let speed = ((b - a).0 as f64 / len.0.max(1) as f64) as f32;
        AudioPiece {
            start,
            len,
            source_start: a,
            speed: speed.clamp(0.05, 20.0),
        }
    };
    if clip.curve.is_none() {
        return vec![AudioPiece {
            start: span.start,
            len: span.end - span.start,
            source_start: clip.source_at(span.start - clip.start).max(Time::ZERO),
            speed: clip.speed,
        }];
    }
    let mut cuts: Vec<Time> = (1..CURVE_AUDIO_PIECES)
        .map(|i| clip.start + Time(clip.duration.0 * i / CURVE_AUDIO_PIECES))
        .filter(|t| *t > span.start && *t < span.end)
        .collect();
    cuts.insert(0, span.start);
    cuts.push(span.end);
    cuts.windows(2)
        .filter(|w| w[1] > w[0])
        .map(|w| piece(w[0], w[1]))
        .collect()
}

/// 片段声音实际放的那一段（预览和导出共用）。转场的时长为 `span`、切点为 `cut` 时，
/// 两段的声音都在 `[cut − span/2, cut + span/2]` 里交叉淡化：前一段越过切点接着放出点后面的
/// 素材、后一段提前放入点前面的素材。哪一侧素材不够，就只越过够用的那部分（一点都没有时
/// 就是前一段淡出到切点、后一段从切点淡入）。片段自己的淡入淡出更长时用长的。
/// 找不到片段时返回 `None`。
#[must_use]
pub fn audio_span(project: &Project, clip: ClipId) -> Option<AudioSpan> {
    let (track, index) = project
        .timeline
        .tracks
        .iter()
        .find_map(|tr| tr.clips.iter().position(|c| c.id == clip).map(|i| (tr, i)))?;
    let clips = &track.clips;
    let me = &clips[index];
    let half = |span: Time| Time(span.0 / 2);
    let to_timeline = |source: Time| source.max(Time::ZERO).scaled(1.0 / f64::from(me.speed));
    // 入点前、出点后还剩多少素材（换成时间轴时间）
    let before = to_timeline(me.source_in());
    let after = me
        .asset()
        .and_then(|a| project.assets.get(&a))
        .map_or(Time::ZERO, |a| {
            to_timeline(a.duration - me.source_at(me.duration))
        });
    let (lead, fade_in) = index
        .checked_sub(1)
        .and_then(|i| transition_between(&clips[i], me))
        .map_or((Time::ZERO, me.fade_in), |(_, span)| {
            let lead = half(span).min(before);
            (lead, lead + me.fade_in.max(half(span)))
        });
    let (tail, fade_out) = clips
        .get(index + 1)
        .and_then(|next| transition_between(me, next))
        .map_or((Time::ZERO, me.fade_out), |(_, span)| {
            let tail = half(span).min(after);
            (tail, tail + me.fade_out.max(half(span)))
        });
    let length = me.duration + lead + tail;
    Some(AudioSpan {
        start: me.start - lead,
        end: me.end() + tail,
        fade_in: fade_in.min(length),
        fade_out: fade_out.min(length),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{add_keyframe, apply_transform_at};
    use crate::testing::sample_project;

    #[test]
    fn layers_carry_the_animated_picture_and_its_largest_scale() {
        let (mut p, ids) = sample_project();
        let at = |scale: f32| Transform {
            scale,
            ..Transform::default()
        };
        // v1（0–5 秒）：片段内 0 秒缩放 1，2 秒缩放 3
        add_keyframe(&mut p, ids.v1, Time::ZERO).unwrap();
        apply_transform_at(&mut p, ids.v1, Time::from_secs(2), at(3.0)).unwrap();
        let layer = resolve_layers(&p, Time::from_secs(1)).swap_remove(0);
        assert_eq!(layer.clip, ids.v1);
        assert!((layer.transform.scale - 2.0).abs() < 1e-6, "{layer:?}");
        assert!((layer.max_scale - 3.0).abs() < 1e-6, "整个片段里最大的缩放");
        // 没有缩放动画：最大缩放就是固定值
        let still = resolve_layers(&p, Time::from_secs(6)).swap_remove(0);
        assert_eq!(still.clip, ids.v2);
        assert_eq!(still.max_scale, still.transform.scale);
    }

    fn dissolve(p: &mut Project, clip: ClipId, secs: i64) {
        crate::edit::set_transition(
            p,
            clip,
            Some(crate::Transition {
                kind: crate::TransitionKind::Dissolve,
                duration: Time::from_secs(secs),
            }),
        )
        .unwrap();
    }

    #[test]
    fn transition_shows_both_clips_around_the_cut() {
        // v1 0–5、v2 5–8 紧挨着；v2 上挂 2 秒叠化：4–6 秒两段都在
        let (mut p, ids) = sample_project();
        dissolve(&mut p, ids.v2, 2);
        assert_eq!(
            resolve_layers(&p, Time::from_millis(3900)).len(),
            1,
            "转场之前"
        );
        let mid = resolve_layers(&p, Time::from_secs(5));
        assert_eq!(mid.len(), 2);
        assert_eq!(
            (mid[0].clip, mid[1].clip),
            (ids.v1, ids.v2),
            "前一段在下、后一段在上"
        );
        assert!((mid[1].transform.opacity - 0.5).abs() < 1e-3, "正中间一半");
        assert_ne!(mid[0].lane, mid[1].lane, "各用各的解码器");
        // 切点之后前一段停在最后一帧；切点之前后一段停在第一帧
        let after = resolve_layers(&p, Time::from_millis(5500));
        let last = Time::from_secs(5) - p.settings.frame_rate.frame_duration();
        assert_eq!(after[0].source_time, last);
        let before = resolve_layers(&p, Time::from_millis(4500));
        assert_eq!(before[1].source_time, Time::from_secs(5), "v2 的素材起点");
        assert_eq!(resolve_layers(&p, Time::from_secs(6)).len(), 1, "转场之后");
        assert!(p.timeline.check_invariants(&p.assets).is_ok());
    }

    #[test]
    fn tone_items_make_a_chain_and_detail_reaches_the_layer() {
        use crate::edit::set_adjust;
        let (mut p, ids) = sample_project();
        let a = crate::ColorAdjust {
            shadows: 0.5,
            vignette: 0.3,
            ..crate::ColorAdjust::NONE
        };
        set_adjust(&mut p, ids.v1, a).unwrap();
        let f = resolve_frame(&p, Time::from_secs(1));
        let l = &f.layers[0];
        assert_eq!(l.grades.len(), 1, "高光阴影光感走查找表");
        assert!(l.grades[0].tone.is_some());
        assert!((l.detail.vignette - 0.3).abs() < 1e-6);
    }

    #[test]
    fn advanced_grades_make_a_chain_only_when_used() {
        use crate::edit::{ADJUSTMENT_LENGTH, add_adjustment, set_grade};
        let (mut p, ids) = sample_project();
        let f = resolve_frame(&p, Time::from_secs(1));
        assert!(f.layers[0].grades.is_empty(), "没有进阶调色：只用矩阵");
        let mut g = crate::Grade::default();
        g.curves.luma = vec![(0.5, 0.8)];
        let adj = add_adjustment(&mut p, Time::ZERO, ADJUSTMENT_LENGTH).unwrap();
        set_grade(&mut p, adj, Some(g.clone())).unwrap();
        let graded = resolve_frame(&p, Time::from_secs(1));
        let chain = &graded.layers[0].grades;
        assert_eq!(chain.len(), 2, "片段自己一步、调节片段一步");
        assert!(chain[0].grade.is_none() && chain[1].grade.as_deref() == Some(&g));
        assert_eq!(graded.background_grades.len(), 1, "黑底也过调节片段");
        // 片段自己的进阶调色
        set_grade(&mut p, ids.v1, Some(g)).unwrap();
        assert!(
            resolve_frame(&p, Time::from_secs(1)).layers[0].grades[0]
                .grade
                .is_some()
        );
        set_grade(&mut p, ids.v1, None).unwrap();
        assert!(
            resolve_frame(&p, Time::from_secs(1)).layers[0].grades[0]
                .grade
                .is_none()
        );
    }

    #[test]
    fn colour_keyframes_reach_the_layer() {
        let (mut p, ids) = sample_project();
        crate::edit::add_keyframe_props(&mut p, ids.v1, Time::ZERO, &[crate::Prop::Saturation])
            .unwrap();
        let bw = crate::ColorAdjust {
            saturation: -1.0,
            ..crate::ColorAdjust::NONE
        };
        crate::edit::apply_adjust_at(&mut p, ids.v1, Time::from_secs(2), bw).unwrap();
        assert!(resolve_frame(&p, Time::ZERO).layers[0].color.is_identity());
        assert_eq!(
            resolve_frame(&p, Time::from_secs(3)).layers[0].color,
            bw.matrix()
        );
    }

    #[test]
    fn effects_sit_on_the_layers_below_them_with_their_own_clock() {
        use crate::edit::{add_effect, add_track, insert_asset_at, set_effect};
        let (mut p, ids) = sample_project();
        let blur = crate::Effect::new(crate::EffectKind::Blur);
        let fx = add_effect(&mut p, Time::from_secs(1), Time::from_secs(3), blur).unwrap();
        // 特效上面再叠一层画中画：特效只压在主轨上
        let top = add_track(&mut p, crate::TrackKind::Video).unwrap();
        insert_asset_at(&mut p, ids.video_asset, top, Time::ZERO).unwrap();
        let frame = resolve_frame(&p, Time::from_millis(2500));
        assert_eq!(frame.layers.len(), 2, "特效片段本身不出画面");
        assert_eq!(frame.effects.len(), 1);
        let pass = frame.effects[0];
        assert_eq!((pass.after, pass.effect), (1, blur));
        assert!((pass.seconds - 1.5).abs() < 1e-6, "从特效片段开头算");
        assert_eq!(resolve_frame(&p, Time::from_millis(500)).effects, []);
        // 换种类、调强度；强度越界不行
        let mut grain = crate::Effect::new(crate::EffectKind::Grain);
        grain.amount = 0.8;
        set_effect(&mut p, fx, grain).unwrap();
        assert_eq!(
            resolve_frame(&p, Time::from_secs(2)).effects[0].effect,
            grain
        );
        grain.amount = 2.0;
        assert_eq!(
            set_effect(&mut p, fx, grain),
            Err(crate::EditError::InvalidValue)
        );
        assert_eq!(
            set_effect(&mut p, ids.v1, crate::Effect::new(crate::EffectKind::Blur)),
            Err(crate::EditError::WrongTrackKind),
            "素材片段不能当特效改"
        );
        let saved = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&saved).unwrap();
        assert_eq!(back, p, "特效片段存得进工程文件");
    }

    #[test]
    fn adjustments_color_everything_below_them_and_the_background() {
        use crate::edit::{
            ADJUSTMENT_LENGTH, TrackFlag, add_adjustment, add_track, insert_asset_at, set_adjust,
            set_disabled, set_track_flag,
        };
        let (mut p, ids) = sample_project();
        let adj = add_adjustment(&mut p, Time::ZERO, ADJUSTMENT_LENGTH).unwrap();
        let bw = crate::FILTERS[11].adjust;
        set_adjust(&mut p, adj, bw).unwrap();
        // 调节上面再叠一层画中画：它不受调节影响
        let top = add_track(&mut p, crate::TrackKind::Video).unwrap();
        insert_asset_at(&mut p, ids.video_asset, top, Time::ZERO).unwrap();
        let frame = resolve_frame(&p, Time::from_secs(1));
        assert_eq!(frame.layers.len(), 2, "调节片段本身不出画面");
        assert_eq!(frame.layers[0].color, bw.matrix(), "主轨被调了");
        assert!(frame.layers[1].color.is_identity(), "上面那层没被调");
        assert_eq!(frame.background, bw.matrix());
        // 片段自己的调色在前，调节层在后
        let warm = crate::FILTERS[3].adjust;
        set_adjust(&mut p, ids.v1, warm).unwrap();
        let both = resolve_frame(&p, Time::from_secs(1)).layers[0].color;
        assert_eq!(both, warm.matrix().then(&bw.matrix()));
        // 调节片段之外、停用、所在轨隐藏：都不起作用
        assert!(
            resolve_frame(&p, Time::from_secs(4))
                .background
                .is_identity()
        );
        set_disabled(&mut p, &[adj], true).unwrap();
        assert!(
            resolve_frame(&p, Time::from_secs(1))
                .background
                .is_identity()
        );
        set_disabled(&mut p, &[adj], false).unwrap();
        let track = p.timeline.find_clip(adj).unwrap().0;
        set_track_flag(&mut p, track, TrackFlag::Hidden, true).unwrap();
        assert!(
            resolve_frame(&p, Time::from_secs(1))
                .background
                .is_identity()
        );
    }

    #[test]
    fn hidden_video_tracks_are_not_composited() {
        use crate::edit::{TrackFlag, set_track_flag};
        let (mut p, ids) = sample_project();
        assert_eq!(resolve_layers(&p, Time::from_secs(1)).len(), 1);
        set_track_flag(&mut p, ids.video_track, TrackFlag::Hidden, true).unwrap();
        assert_eq!(resolve_layers(&p, Time::from_secs(1)).len(), 0);
        set_track_flag(&mut p, ids.video_track, TrackFlag::Hidden, false).unwrap();
        assert_eq!(resolve_layers(&p, Time::from_secs(1)).len(), 1);
    }

    #[test]
    fn transition_needs_adjacent_enabled_clips_and_fits_both() {
        let (mut p, ids) = sample_project();
        // 5 秒的转场：被前后两段较短的一段（v2 只有 3 秒）限制成 3 秒，3.5–6.5
        dissolve(&mut p, ids.v2, 5);
        assert_eq!(resolve_layers(&p, Time::from_millis(3400)).len(), 1);
        assert_eq!(resolve_layers(&p, Time::from_millis(3600)).len(), 2);
        // 停用前一段：不做转场
        crate::edit::set_disabled(&mut p, &[ids.v1], true).unwrap();
        assert_eq!(resolve_layers(&p, Time::from_millis(5100)).len(), 1);
        crate::edit::set_disabled(&mut p, &[ids.v1], false).unwrap();
        // 中间留了空隙：不做转场
        crate::edit::move_clip(&mut p, ids.v2, ids.video_track, Time::from_secs(6)).unwrap();
        assert_eq!(
            resolve_layers(&p, Time::from_millis(5900)).len(),
            0,
            "空隙里没画面"
        );
        assert_eq!(resolve_layers(&p, Time::from_millis(6100)).len(), 1);
        // 字幕不能挂转场、时长要为正
        assert_eq!(
            crate::edit::set_transition(&mut p, ids.s1, None),
            Err(crate::EditError::WrongTrackKind)
        );
    }

    #[test]
    fn split_keeps_the_transition_on_the_left_half_only() {
        let (mut p, ids) = sample_project();
        dissolve(&mut p, ids.v2, 1);
        let right = crate::edit::split_at(&mut p, ids.v2, Time::from_secs(7)).unwrap();
        let (_, r) = p.timeline.find_clip(right).unwrap();
        assert!(r.transition_in.is_none());
        let (_, l) = p.timeline.find_clip(ids.v2).unwrap();
        assert!(l.transition_in.is_some());
    }

    #[test]
    fn transitions_crossfade_the_sound_over_the_spare_material() {
        let (mut p, ids) = sample_project();
        let secs = Time::from_secs;
        let span = |project: &Project, c| audio_span(project, c).unwrap();
        assert_eq!(
            span(&p, ids.v1),
            AudioSpan {
                start: Time::ZERO,
                end: secs(5),
                fade_in: Time::ZERO,
                fade_out: Time::ZERO,
            }
        );
        dissolve(&mut p, ids.v2, 2);
        // v1 用素材 0–5 秒、后面还有 5 秒；v2 用素材 5–8 秒、前面还有 5 秒：
        // 2 秒转场里两段的声音叠在一起，前一段放到 6 秒淡完，后一段从 4 秒淡起
        assert_eq!(
            span(&p, ids.v1),
            AudioSpan {
                start: Time::ZERO,
                end: secs(6),
                fade_in: Time::ZERO,
                fade_out: secs(2),
            }
        );
        assert_eq!(
            span(&p, ids.v2),
            AudioSpan {
                start: secs(4),
                end: secs(8),
                fade_in: secs(2),
                fade_out: Time::ZERO,
            }
        );
        // 入点前只剩 0.4 秒素材：只提前 0.4 秒，淡到切点后 1 秒
        let mut v2 = Clip::clone(p.timeline.find_clip(ids.v2).unwrap().1);
        v2.content = crate::ClipContent::Media {
            asset: v2.asset().unwrap(),
            source_in: Time::from_millis(400),
        };
        set_clip(&mut p, &v2);
        assert_eq!(
            span(&p, ids.v2),
            AudioSpan {
                start: secs(5) - Time::from_millis(400),
                end: secs(8),
                fade_in: Time::from_millis(1400),
                fade_out: Time::ZERO,
            }
        );
        // 片段自己的淡入更长：用长的
        crate::edit::set_fades(&mut p, ids.v2, secs(2), Time::ZERO).unwrap();
        assert_eq!(span(&p, ids.v2).fade_in, Time::from_millis(2400));
        // 停用前一段：转场不起作用，也不越过片段边界
        crate::edit::set_disabled(&mut p, &[ids.v1], true).unwrap();
        assert_eq!(span(&p, ids.v2).start, secs(5));
        assert_eq!(span(&p, ids.v2).fade_in, secs(2));
        assert_eq!(span(&p, ids.v1).end, secs(5));
        assert_eq!(audio_span(&p, ClipId(999)), None);
    }

    /// 直接换掉时间轴上的片段（测试用）。
    fn set_clip(p: &mut Project, clip: &Clip) {
        for tr in &mut p.timeline.tracks {
            for c in &mut tr.clips {
                if c.id == clip.id {
                    *c = std::sync::Arc::new(clip.clone());
                }
            }
        }
    }

    #[test]
    fn groups_follow_the_layers_that_were_actually_drawn() {
        let g = GroupPass {
            start: 1,
            end: 3,
            effects_end: 0,
            opacity: 0.5,
            shape: None,
        };
        // 第 0 层没画：组往前挪一层
        assert_eq!(
            GroupPass::remap(&[g], &[false, true, true, true]),
            [GroupPass {
                start: 0,
                end: 2,
                ..g
            }]
        );
        // 组里一层都没画：丢掉
        assert_eq!(GroupPass::remap(&[g], &[true, false, false, true]), []);
    }

    #[test]
    fn effect_speed_runs_the_effects_clock_faster_or_slower() {
        let (mut p, _) = sample_project();
        let mut fast = crate::Effect::new(crate::EffectKind::Grain);
        fast.speed = 1.0;
        let fx = crate::edit::add_effect(&mut p, Time::ZERO, Time::from_secs(4), fast).unwrap();
        let secs = |proj: &Project| resolve_frame(proj, Time::from_secs(1)).effects[0].seconds;
        // 速度拉满：4 倍
        assert!((secs(&p) - 4.0).abs() < 1e-4, "{}", secs(&p));
        // 默认（中间）：原速
        let mut normal = fast;
        normal.speed = crate::Effect::DEFAULT_SPEED;
        crate::edit::set_effect(&mut p, fx, normal).unwrap();
        assert!((secs(&p) - 1.0).abs() < 1e-4);
        // 旧工程文件没有速度这一项：按原速读
        let old: crate::Effect = serde_json::from_str(r#"{"kind":"Grain","amount":0.5}"#).unwrap();
        assert!((old.speed - crate::Effect::DEFAULT_SPEED).abs() < 1e-6);
    }
}
