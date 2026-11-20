//! 导出界面（剪映式）：选输出文件 → 导出对话框（视频导出：分辨率 / 帧率 / 码率 / 编码；
//! 音频导出：格式；封面；目标响度；导出范围；高级里选编码器；底部实时显示时长和估计大小）
//! → 后台依次导出，顶栏显示进度和取消。

use crate::actions as act;
use crate::chrome;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, px,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use ve_core::{FrameRate, ProjectSettings, Time, format_timecode};
use ve_export::{
    AudioFormat, Codec, Encoder, ExportSettings, Output, estimated_bytes, recommended_kbps,
};

/// 进行中的导出。
#[derive(Debug)]
pub struct ExportProgress {
    /// 0–1。
    pub progress: f32,
    /// 取消标志。
    pub cancel: Arc<AtomicBool>,
    /// 输出文件。
    pub path: PathBuf,
}

/// 输出分辨率选项（按短边，和剪映一样：竖屏的 720P 是 720×1280）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// 跟工程一样。
    Project,
    /// 短边 720。
    P720,
    /// 短边 1080。
    P1080,
    /// 短边 1440（2K）。
    P1440,
    /// 短边 2160（4K）。
    P2160,
}

/// 按目标短边等比缩放，宽高取偶数。
#[must_use]
pub fn scaled_size(width: u32, height: u32, res: Resolution) -> (u32, u32) {
    let target: u32 = match res {
        Resolution::Project => return (width, height),
        Resolution::P720 => 720,
        Resolution::P1080 => 1080,
        Resolution::P1440 => 1440,
        Resolution::P2160 => 2160,
    };
    let short = u64::from(width.min(height).max(1));
    let scale = |v: u32| {
        let s = u32::try_from(u64::from(v) * u64::from(target) / short).unwrap_or(u32::MAX);
        (s & !1).max(2)
    };
    (scale(width), scale(height))
}

/// 码率档位（剪映：更低 / 推荐 / 更高 / 自定义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bitrate {
    /// 推荐的一半。
    Lower,
    /// 按分辨率和帧率推荐。
    Recommended,
    /// 推荐的两倍。
    Higher,
    /// 自己填。
    Custom,
}

/// 导出对话框里选的东西（纯数据，方便测试）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportChoices {
    /// 分辨率。
    pub resolution: Resolution,
    /// 帧率；`None` 跟工程一样。
    pub frame_rate: Option<FrameRate>,
    /// 码率档位。
    pub bitrate: Bitrate,
    /// 自定义码率（kbps）。
    pub custom_kbps: u32,
    /// 编码格式。
    pub codec: Codec,
    /// 编码器。
    pub encoder: Encoder,
    /// 导出视频（剪映「视频导出」）。
    pub video: bool,
    /// 另外导出一份声音（剪映「音频导出」）及其格式。
    pub audio: Option<AudioFormat>,
    /// 目标响度（LUFS）。
    pub loudness: Option<i8>,
    /// 只导出入出点区域。
    pub only_range: bool,
    /// 封面：导出时取成片的一帧存成图片。
    pub cover: Option<CoverAt>,
}

/// 封面取哪一帧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverAt {
    /// 草稿封面（主轨上设的）。
    Draft,
    /// 第一帧。
    First,
    /// 播放头那一帧。
    Playhead,
}

/// 一次导出要做的事：依次导出这些文件，最后（有的话）从视频里取封面。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportPlan {
    /// 要导出的文件。
    pub jobs: Vec<ExportSettings>,
    /// 封面：（从哪个视频取，成片里的时间，存到哪）。
    pub cover: Option<(PathBuf, Time, PathBuf)>,
}

impl Default for ExportChoices {
    fn default() -> Self {
        Self {
            resolution: Resolution::Project,
            frame_rate: None,
            bitrate: Bitrate::Recommended,
            custom_kbps: 12_000,
            codec: Codec::H264,
            encoder: Encoder::Nvenc,
            video: true,
            audio: None,
            loudness: None,
            only_range: false,
            cover: None,
        }
    }
}

impl ExportChoices {
    /// 按工程设置换算成导出设置。
    #[must_use]
    pub fn settings(&self, path: PathBuf, project: &ProjectSettings) -> ExportSettings {
        let (width, height) = scaled_size(project.width, project.height, self.resolution);
        let frame_rate = self.frame_rate.unwrap_or(project.frame_rate);
        let recommended = recommended_kbps(width, height, frame_rate, self.codec);
        let video_bitrate_kbps = match self.bitrate {
            Bitrate::Lower => (recommended / 2).max(500),
            Bitrate::Recommended => recommended,
            Bitrate::Higher => recommended.saturating_mul(2),
            Bitrate::Custom => self.custom_kbps.clamp(100, 500_000),
        };
        ExportSettings {
            path,
            width,
            height,
            frame_rate,
            video_bitrate_kbps,
            encoder: self.encoder,
            codec: self.codec,
            render: ve_export::RenderConfig::default(),
            output: Output::Video,
            loudness_lufs: self.loudness,
            range: None,
        }
    }

    /// 整次导出：视频写到 `path`，音频写到同名、换扩展名的文件，封面是 `名字_封面.png`。
    /// `range` 是时间轴的入出点区域（勾了「只导出区域」才用），`playhead` 用来取封面。
    ///
    /// # Errors
    /// 视频和音频都没勾。
    pub fn plan(
        &self,
        path: &std::path::Path,
        project: &ProjectSettings,
        range: Option<(Time, Time)>,
        playhead: Time,
    ) -> Result<ExportPlan, &'static str> {
        self.plan_with_cover(path, project, range, playhead, None)
    }

    /// 同 [`Self::plan`]，带上草稿封面（[`CoverAt::Draft`] 用）。
    ///
    /// # Errors
    /// 视频和音频都没勾。
    pub fn plan_with_cover(
        &self,
        path: &std::path::Path,
        project: &ProjectSettings,
        range: Option<(Time, Time)>,
        playhead: Time,
        draft: Option<&ve_core::Cover>,
    ) -> Result<ExportPlan, &'static str> {
        let range = range.filter(|_| self.only_range);
        let mut jobs = Vec::new();
        let video_path = path.to_path_buf();
        if self.video {
            jobs.push(ExportSettings {
                range,
                ..self.settings(video_path.clone(), project)
            });
        }
        if let Some(format) = self.audio {
            jobs.push(ExportSettings {
                path: path.with_extension(format.extension()),
                output: Output::Audio(format),
                range,
                ..self.settings(video_path.clone(), project)
            });
        }
        if jobs.is_empty() {
            return Err("视频导出和音频导出至少选一个");
        }
        let offset = range.map_or(Time::ZERO, |r| r.0);
        let cover = self.cover.filter(|_| self.video).and_then(|at| {
            let (from, t) = match (at, draft) {
                (CoverAt::First, _) => (video_path.clone(), Time::ZERO),
                (CoverAt::Playhead, _) => (video_path.clone(), (playhead - offset).max(Time::ZERO)),
                (CoverAt::Draft, Some(ve_core::Cover::Frame(t))) => {
                    (video_path.clone(), (*t - offset).max(Time::ZERO))
                }
                (CoverAt::Draft, Some(ve_core::Cover::Image(img))) => (img.clone(), Time::ZERO),
                (CoverAt::Draft, None) => return None,
            };
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            Some((from, t, path.with_file_name(format!("{stem}_封面.png"))))
        });
        Ok(ExportPlan { jobs, cover })
    }
}

/// 估计大小的写法：「约 12.3 MB」。
#[must_use]
pub fn size_text(bytes: u64) -> String {
    #[expect(clippy::cast_precision_loss, reason = "只是给人看的估计值")]
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("约 {:.2} GB", mb / 1000.0)
    } else {
        format!("约 {mb:.1} MB")
    }
}

/// 打开导出对话框要的：工程设置、时长、入出点区域、播放头、草稿封面。
type DialogInput = (
    ProjectSettings,
    Time,
    Option<(Time, Time)>,
    Time,
    Option<ve_core::Cover>,
);

/// 导出选项表单。
#[derive(Debug)]
pub struct ExportForm {
    path: PathBuf,
    project: ProjectSettings,
    duration: Time,
    /// 时间轴的入出点区域。
    range: Option<(Time, Time)>,
    playhead: Time,
    /// 草稿封面。
    draft_cover: Option<ve_core::Cover>,
    choices: ExportChoices,
    custom: Entity<InputState>,
    advanced: bool,
}

impl ExportForm {
    fn new(
        path: PathBuf,
        timeline: DialogInput,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let (project, duration, range, playhead, draft_cover) = timeline;
        let custom = cx.new(|cx| InputState::new(window, cx).default_value("12000"));
        Self {
            path,
            project,
            duration,
            range,
            playhead,
            choices: ExportChoices {
                // 有入出点区域时默认只导出区域（剪映的入出点就是为这个）
                only_range: range.is_some(),
                // 设了草稿封面就默认一起导出
                cover: draft_cover.as_ref().map(|_| CoverAt::Draft),
                ..ExportChoices::default()
            },
            draft_cover,
            custom,
            advanced: false,
        }
    }

    fn choices(&self, cx: &gpui_kit::App) -> ExportChoices {
        let mut c = self.choices;
        if let Ok(v) = self.custom.read(cx).value().trim().parse() {
            c.custom_kbps = v;
        }
        c
    }

    fn settings(&self, cx: &gpui_kit::App) -> ExportSettings {
        self.choices(cx).settings(self.path.clone(), &self.project)
    }

    fn plan(&self, cx: &gpui_kit::App) -> Result<ExportPlan, &'static str> {
        self.choices(cx).plan_with_cover(
            &self.path,
            &self.project,
            self.range,
            self.playhead,
            self.draft_cover.as_ref(),
        )
    }

    /// 实际导出的长度（只导出区域时是区域长度）。
    fn out_duration(&self) -> Time {
        match self.range.filter(|_| self.choices.only_range) {
            Some((a, b)) => b - a,
            None => self.duration,
        }
    }

    /// 一个选项小块：点了就改 `choices`。
    fn pick(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        on: bool,
        change: impl Fn(&mut ExportChoices) + 'static,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        let id: SharedString = id.into();
        chrome::chip(id, label.into(), on, cx)
            .on_click(cx.listener(move |f, _, _, cx| {
                change(&mut f.choices);
                cx.notify();
            }))
            .into_any_element()
    }
}

/// 一行：左边名字，右边一排选项。
fn row(label: &'static str, items: Vec<AnyElement>, cx: &gpui_kit::App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_start()
        .child(
            div()
                .w(px(72.))
                .flex_none()
                .pt_1()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(h_flex().flex_1().flex_wrap().gap_1().children(items))
}

impl ExportForm {
    /// 各行的选项：（分辨率，帧率，码率，编码，编码器）。
    #[expect(clippy::type_complexity, reason = "五组选项一起算，拆开反而难读")]
    fn option_items(
        &self,
        cx: &Context<'_, Self>,
    ) -> (
        Vec<AnyElement>,
        Vec<AnyElement>,
        Vec<AnyElement>,
        Vec<AnyElement>,
        Vec<AnyElement>,
    ) {
        let c = self.choices;
        let res_items = [
            ("res-project", "原始", Resolution::Project),
            ("res-720", "720P", Resolution::P720),
            ("res-1080", "1080P", Resolution::P1080),
            ("res-1440", "2K", Resolution::P1440),
            ("res-2160", "4K", Resolution::P2160),
        ]
        .map(|(id, label, r)| {
            Self::pick(
                id,
                label,
                c.resolution == r,
                move |ch| ch.resolution = r,
                cx,
            )
        })
        .into();
        let fps_items = std::iter::once(Self::pick(
            "fps-project",
            "同工程",
            c.frame_rate.is_none(),
            |ch| ch.frame_rate = None,
            cx,
        ))
        .chain([24, 25, 30, 50, 60].map(|n| {
            let r = FrameRate::new(n, 1);
            Self::pick(
                format!("fps-{n}"),
                n.to_string(),
                c.frame_rate == Some(r),
                move |ch| {
                    ch.frame_rate = Some(r);
                },
                cx,
            )
        }))
        .collect();
        let br_items = [
            ("br-low", "更低", Bitrate::Lower),
            ("br-rec", "推荐", Bitrate::Recommended),
            ("br-high", "更高", Bitrate::Higher),
            ("br-custom", "自定义", Bitrate::Custom),
        ]
        .map(|(id, label, b)| Self::pick(id, label, c.bitrate == b, move |ch| ch.bitrate = b, cx))
        .into();
        let codec_items = [
            ("codec-h264", "H.264（最通用）", Codec::H264),
            ("codec-hevc", "HEVC（更省空间）", Codec::Hevc),
        ]
        .map(|(id, label, k)| Self::pick(id, label, c.codec == k, move |ch| ch.codec = k, cx))
        .into();
        let enc_items = [
            ("enc-nvenc", "显卡（NVENC）", Encoder::Nvenc),
            ("enc-x264", "CPU", Encoder::X264),
        ]
        .map(|(id, label, e)| Self::pick(id, label, c.encoder == e, move |ch| ch.encoder = e, cx))
        .into();
        (res_items, fps_items, br_items, codec_items, enc_items)
    }

    /// 估计大小：视频（含 192k 声音）加上单独导出的音频。
    fn size_estimate(&self, preview: &ExportSettings) -> String {
        let d = self.out_duration();
        let c = self.choices;
        let mut parts = Vec::new();
        if c.video {
            parts.push(format!(
                "视频 {}（码率 {} kbps）",
                size_text(estimated_bytes(preview.video_bitrate_kbps, d)),
                preview.video_bitrate_kbps
            ));
        }
        if let Some(f) = c.audio {
            // estimated_bytes 会加 192k 的声音，这里减掉
            let kbps = f.kbps().saturating_sub(192);
            parts.push(format!("音频 {}", size_text(estimated_bytes(kbps, d))));
        }
        if parts.is_empty() {
            "视频导出和音频导出至少选一个".to_owned()
        } else {
            format!("大小 {}（估计）", parts.join(" + "))
        }
    }

    /// 视频导出 / 音频导出 / 封面 / 响度 / 范围这几行。
    fn render_outputs(&self, cx: &Context<'_, Self>) -> Vec<AnyElement> {
        let c = self.choices;
        let check =
            |id: &'static str, label: &'static str, on: bool, change: fn(&mut ExportChoices)| {
                Self::pick(
                    id,
                    format!("{} {label}", if on { "☑" } else { "☐" }),
                    on,
                    change,
                    cx,
                )
            };
        let mut rows = vec![
            row(
                "导出",
                vec![
                    check("export-video", "视频导出", c.video, |ch| {
                        ch.video = !ch.video;
                    }),
                    check("export-audio", "音频导出", c.audio.is_some(), |ch| {
                        ch.audio = match ch.audio {
                            Some(_) => None,
                            None => Some(AudioFormat::Mp3),
                        };
                    }),
                ],
                cx,
            )
            .into_any_element(),
        ];
        if c.audio.is_some() {
            let items = [
                ("afmt-mp3", "MP3", AudioFormat::Mp3),
                ("afmt-wav", "WAV", AudioFormat::Wav),
                ("afmt-aac", "AAC", AudioFormat::Aac),
            ]
            .map(|(id, label, f)| {
                Self::pick(
                    id,
                    label,
                    c.audio == Some(f),
                    move |ch| ch.audio = Some(f),
                    cx,
                )
            })
            .into();
            rows.push(row("音频格式", items, cx).into_any_element());
        }
        if c.video {
            let mut options = vec![("cover-none", "不导出", None)];
            if self.draft_cover.is_some() {
                options.push(("cover-draft", "草稿封面", Some(CoverAt::Draft)));
            }
            options.extend([
                ("cover-first", "第一帧", Some(CoverAt::First)),
                ("cover-playhead", "播放头这一帧", Some(CoverAt::Playhead)),
            ]);
            let items = options
                .into_iter()
                .map(|(id, label, at)| {
                    Self::pick(id, label, c.cover == at, move |ch| ch.cover = at, cx)
                })
                .collect();
            rows.push(row("封面", items, cx).into_any_element());
        }
        let loud = [
            ("loud-off", "关", None),
            ("loud-14", "-14 LUFS（短视频平台）", Some(-14)),
            ("loud-16", "-16 LUFS", Some(-16)),
            ("loud-23", "-23 LUFS（广播）", Some(-23)),
        ]
        .map(|(id, label, l)| Self::pick(id, label, c.loudness == l, move |ch| ch.loudness = l, cx))
        .into();
        rows.push(row("目标响度", loud, cx).into_any_element());
        if let Some((a, b)) = self.range {
            let rate = self.project.frame_rate;
            let items = vec![
                Self::pick(
                    "range-all",
                    "全部",
                    !c.only_range,
                    |ch| ch.only_range = false,
                    cx,
                ),
                Self::pick(
                    "range-only",
                    format!(
                        "选定区域 {}–{}",
                        format_timecode(a, rate),
                        format_timecode(b, rate)
                    ),
                    c.only_range,
                    |ch| ch.only_range = true,
                    cx,
                ),
            ];
            rows.push(row("导出范围", items, cx).into_any_element());
        }
        rows
    }

    /// 底部：时长和估计大小。
    fn render_estimate(
        &self,
        preview: &ExportSettings,
        cx: &Context<'_, Self>,
    ) -> impl IntoElement {
        h_flex()
            .id("export-estimate")
            .test_support()
            .pt_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .gap_2()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(format!(
                "时长 {}",
                format_timecode(self.out_duration(), preview.frame_rate)
            )))
            .child("｜")
            .child(SharedString::from(self.size_estimate(preview)))
    }
}

impl Render for ExportForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let c = self.choices;
        let preview = self.settings(cx);
        let (res_items, fps_items, br_items, codec_items, enc_items) = self.option_items(cx);
        let muted = cx.theme().muted_foreground;
        let file_name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(div().text_sm().child(SharedString::from(file_name)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .truncate()
                            .child(SharedString::from(self.path.display().to_string())),
                    ),
            )
            .children(self.render_outputs(cx))
            .children(c.video.then(|| {
                v_flex()
                    .gap_3()
                    .child(row("分辨率", res_items, cx))
                    .child(
                        div()
                            .pl(px(80.))
                            .text_xs()
                            .text_color(muted)
                            .child(SharedString::from(format!(
                                "输出 {}×{}（按工程比例换算）",
                                preview.width, preview.height
                            ))),
                    )
                    .child(row("帧率", fps_items, cx))
                    .child(row("码率", br_items, cx))
                    .children((c.bitrate == Bitrate::Custom).then(|| {
                        h_flex()
                            .pl(px(80.))
                            .gap_2()
                            .child(div().w(px(120.)).child(Input::new(&self.custom)))
                            .child(div().text_sm().text_color(muted).child("kbps"))
                    }))
                    .child(row("视频编码", codec_items, cx))
            }))
            .child(
                div()
                    .id("export-advanced")
                    .test_support()
                    .text_xs()
                    .text_color(muted)
                    .cursor_pointer()
                    .child(if self.advanced {
                        "高级 ▴"
                    } else {
                        "高级 ▾"
                    })
                    .on_click(cx.listener(|f, _, _, cx| {
                        f.advanced = !f.advanced;
                        cx.notify();
                    })),
            )
            .children(self.advanced.then(|| {
                v_flex()
                    .gap_2()
                    .child(row("编码器", enc_items, cx))
                    .child(row(
                        "音频",
                        vec![
                            div()
                                .pt_1()
                                .text_sm()
                                .child("AAC 192 kbps")
                                .into_any_element(),
                        ],
                        cx,
                    ))
            }))
            .child(self.render_estimate(&preview, cx))
    }
}

enum Msg {
    Progress(f32),
    Done(Result<(), String>),
}

/// 依次导出计划里的文件（进度按文件个数平分），再从视频里取封面。
fn run_plan(
    project: &ve_core::Project,
    plan: &ExportPlan,
    tx: &smol::channel::Sender<Msg>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    #[expect(clippy::cast_precision_loss, reason = "文件个数很少")]
    let n = plan.jobs.len().max(1) as f32;
    for (i, job) in plan.jobs.iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "文件个数很少")]
        let base = i as f32;
        ve_export::export(
            project,
            job,
            |p| drop(tx.try_send(Msg::Progress((base + p) / n))),
            cancel,
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some((video, at, out)) = &plan.cover {
        ve_export::grab_frame(video, *at, out).map_err(|e| format!("取封面失败：{e}"))?;
    }
    Ok(())
}

impl Workspace {
    pub(crate) fn export(
        &mut self,
        _: &act::Export,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.export.is_some() {
            return;
        }
        if self.doc.project.timeline.duration() <= Time::ZERO {
            crate::workspace::notify(Notification::warning("时间轴是空的"), window, cx);
            return;
        }
        let dir = self
            .doc
            .path()
            .and_then(|p| p.parent().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let rx = cx.prompt_for_new_path(&dir, Some("导出.mp4"));
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let _ignored = this.update_in(cx, |this, window, cx| {
                    let timeline = (
                        this.doc.project.settings,
                        this.doc.project.timeline.duration(),
                        this.doc.project.timeline.range,
                        this.playhead,
                        this.doc.project.timeline.cover.clone(),
                    );
                    Self::open_export_dialog(path, timeline, window, cx);
                });
            }
        }));
    }

    fn open_export_dialog(
        path: PathBuf,
        timeline: DialogInput,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let form = cx.new(|cx| ExportForm::new(path, timeline, window, cx));
        let ws = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let form = form.clone();
            let ws = ws.clone();
            dialog
                .title("导出")
                .width(px(560.))
                .child(form.clone())
                // 普通 Dialog 不会自己画按钮：on_ok 只登记回调，按钮要放进 footer
                .footer(
                    DialogFooter::new()
                        .child(
                            div()
                                .id("export-cancel-dialog")
                                .test_support()
                                .child(DialogClose::new().trigger(|b| b.ghost().label("取消"))),
                        )
                        .child(DialogAction::new().child(
                            Button::new("export-confirm").primary().label("开始导出"),
                        )),
                )
                .on_ok(move |_, window, cx| match form.read(cx).plan(cx) {
                    Ok(plan) => {
                        ws.update(cx, |this, cx| this.start_export_plan(plan, window, cx));
                        true
                    }
                    Err(e) => {
                        crate::workspace::notify(Notification::warning(e), window, cx);
                        false
                    }
                })
        });
    }

    /// 后台导出一个文件；进度显示在工具栏。
    pub fn start_export(
        &mut self,
        settings: ExportSettings,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let plan = ExportPlan {
            jobs: vec![settings],
            cover: None,
        };
        self.start_export_plan(plan, window, cx);
    }

    /// 后台依次导出计划里的文件，最后取封面；进度显示在工具栏。
    pub fn start_export_plan(
        &mut self,
        plan: ExportPlan,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(first) = plan.jobs.first() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.export = Some(ExportProgress {
            progress: 0.0,
            cancel: Arc::clone(&cancel),
            path: first.path.clone(),
        });
        let project = self.doc.project.clone();
        let (tx, rx) = smol::channel::unbounded::<Msg>();
        let spawned = std::thread::Builder::new()
            .name("ve-export".into())
            .spawn(move || {
                // 导出里任何意外的 panic 也要变成失败消息，否则界面会一直停在某个进度
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_plan(&project, &plan, &tx, &cancel)
                }))
                .unwrap_or_else(|_| Err("导出时出现内部错误".to_owned()));
                drop(tx.try_send(Msg::Done(result)));
            });
        if let Err(e) = spawned {
            self.export = None;
            crate::workspace::notify(
                Notification::error(format!("无法开始导出：{e}")),
                window,
                cx,
            );
            return;
        }
        // 定时取进度（不 await 跨线程通道，原因同引擎事件）
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let mut done = false;
                while let Ok(msg) = rx.try_recv() {
                    done |= matches!(msg, Msg::Done(_));
                    let alive = this
                        .update_in(cx, |this, window, cx| this.on_export_msg(msg, window, cx))
                        .is_ok();
                    done |= !alive;
                }
                if done {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn on_export_msg(&mut self, msg: Msg, window: &mut Window, cx: &mut Context<'_, Self>) {
        match msg {
            Msg::Progress(p) => {
                if let Some(e) = self.export.as_mut() {
                    e.progress = p;
                }
            }
            Msg::Done(result) => {
                let path = self.export.take().map(|e| e.path);
                match result {
                    Ok(()) => {
                        let shown = path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        crate::workspace::notify(
                            Notification::success(format!("导出完成：{shown}")),
                            window,
                            cx,
                        );
                        self.last_export = path;
                    }
                    Err(e) => {
                        crate::workspace::notify(
                            Notification::error(e).autohide(false),
                            window,
                            cx,
                        );
                    }
                }
            }
        }
        cx.notify();
    }

    /// 是否在导出。
    #[must_use]
    pub const fn is_exporting(&self) -> bool {
        self.export.is_some()
    }

    pub(crate) fn render_export_status(&self, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let Some(e) = self.export.as_ref() else {
            let done = self.last_export.clone()?;
            return Some(
                Button::new("export-reveal")
                    .ghost()
                    .compact()
                    .label("打开所在文件夹")
                    .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&done)))
                    .into_any_element(),
            );
        };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "0–100"
        )]
        let pct = (e.progress * 100.0).round() as u32;
        Some(
            h_flex()
                .gap_2()
                .text_sm()
                .child(SharedString::from(format!("导出中 {pct}%")))
                .child(
                    Button::new("export-cancel")
                        .ghost()
                        .compact()
                        .label("取消")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(e) = &this.export {
                                e.cancel.store(true, Ordering::Release);
                            }
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_size_keeps_aspect_and_even_width() {
        assert_eq!(scaled_size(1920, 1080, Resolution::Project), (1920, 1080));
        assert_eq!(scaled_size(1920, 1080, Resolution::P720), (1280, 720));
        // 按短边：竖屏的 720P 是 720×1280（剪映的叫法）
        assert_eq!(scaled_size(1080, 1920, Resolution::P720), (720, 1280));
        assert_eq!(scaled_size(3840, 2160, Resolution::P1080), (1920, 1080));
        assert_eq!(scaled_size(1920, 1080, Resolution::P2160), (3840, 2160));
        assert_eq!(scaled_size(1920, 1080, Resolution::P1440), (2560, 1440));
    }

    #[test]
    fn choices_turn_into_export_settings() {
        let project = ProjectSettings::default();
        let path = PathBuf::from("out.mp4");
        let rec = ExportChoices::default().settings(path.clone(), &project);
        assert_eq!((rec.width, rec.height), (1920, 1080));
        assert_eq!(rec.frame_rate, project.frame_rate);
        assert_eq!(rec.video_bitrate_kbps, 9300);
        assert_eq!(rec.codec, Codec::H264);
        let small = ExportChoices {
            resolution: Resolution::P720,
            frame_rate: Some(FrameRate::new(60, 1)),
            bitrate: Bitrate::Lower,
            codec: Codec::Hevc,
            ..ExportChoices::default()
        }
        .settings(path.clone(), &project);
        assert_eq!((small.width, small.height), (1280, 720));
        assert_eq!(small.frame_rate, FrameRate::new(60, 1));
        assert_eq!(
            small.video_bitrate_kbps, 2900,
            "720p60 HEVC 推荐 5800 的一半"
        );
        let custom = ExportChoices {
            bitrate: Bitrate::Custom,
            custom_kbps: 3,
            ..ExportChoices::default()
        }
        .settings(path.clone(), &project);
        assert_eq!(custom.video_bitrate_kbps, 100, "夹到最小值");
        assert_eq!(size_text(12_345_678), "约 12.3 MB");
        // 音频导出、封面、区域、响度
        let range = Some((Time::from_secs(2), Time::from_secs(5)));
        let all = ExportChoices {
            audio: Some(AudioFormat::Wav),
            cover: Some(CoverAt::Playhead),
            loudness: Some(-14),
            only_range: true,
            ..ExportChoices::default()
        }
        .plan(
            &PathBuf::from("d/片子.mp4"),
            &project,
            range,
            Time::from_secs(3),
        )
        .unwrap();
        assert_eq!(all.jobs.len(), 2);
        assert_eq!(all.jobs[0].output, Output::Video);
        assert_eq!(all.jobs[1].output, Output::Audio(AudioFormat::Wav));
        assert_eq!(all.jobs[1].path, PathBuf::from("d/片子.wav"));
        assert!(
            all.jobs
                .iter()
                .all(|j| j.range == range && j.loudness_lufs == Some(-14))
        );
        assert_eq!(
            all.cover,
            Some((
                PathBuf::from("d/片子.mp4"),
                Time::from_secs(1),
                PathBuf::from("d/片子_封面.png")
            )),
            "封面取成片里播放头那一刻（区域从 2 秒开始）"
        );
        let none = ExportChoices {
            video: false,
            ..ExportChoices::default()
        };
        assert!(none.plan(&path, &project, None, Time::ZERO).is_err());
        let audio_only = ExportChoices {
            video: false,
            audio: Some(AudioFormat::Mp3),
            cover: Some(CoverAt::First),
            ..ExportChoices::default()
        }
        .plan(&path, &project, range, Time::ZERO)
        .unwrap();
        assert_eq!(audio_only.cover, None, "没导出视频就没有封面");
        assert_eq!(audio_only.jobs[0].range, None, "没勾「只导出区域」");
        // 草稿封面：帧按区域换算；图片直接用
        let draft = ExportChoices {
            cover: Some(CoverAt::Draft),
            only_range: true,
            ..ExportChoices::default()
        };
        let frame = ve_core::Cover::Frame(Time::from_secs(4));
        let p = draft
            .plan_with_cover(&path, &project, range, Time::ZERO, Some(&frame))
            .unwrap();
        assert_eq!(p.cover.unwrap().1, Time::from_secs(2));
        let img = ve_core::Cover::Image(PathBuf::from("c.jpg"));
        let p = draft
            .plan_with_cover(&path, &project, range, Time::ZERO, Some(&img))
            .unwrap();
        assert_eq!(p.cover.unwrap().0, PathBuf::from("c.jpg"));
        assert_eq!(
            draft
                .plan(&path, &project, range, Time::ZERO)
                .unwrap()
                .cover,
            None
        );
        assert_eq!(size_text(2_500_000_000), "约 2.50 GB");
    }
}
