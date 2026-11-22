//! 草稿首页（剪映）：启动时先进这里——「开始创作」新建，下面是草稿卡片（名字、时长、多久以前改的），
//! 点一张打开它。卡片来自草稿文件夹和「最近打开」，按修改时间从新到旧排。

use crate::theme::palette;
use crate::workspace::Workspace;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use ve_core::Time;

/// 首页最多列多少个草稿。
const MAX_CARDS: usize = 40;
/// 封面高（像素）。
const COVER_HEIGHT: u32 = 110;

/// 一张草稿卡片。
#[derive(Debug, Clone)]
pub(crate) struct DraftCard {
    /// 工程文件。
    pub(crate) path: PathBuf,
    /// 名字（文件名去掉扩展名）。
    pub(crate) name: String,
    /// 最后修改的时间。
    pub(crate) modified: Option<SystemTime>,
    /// 时间轴总长（读不出来时 `None`）。
    pub(crate) duration: Option<Time>,
    /// 封面取自哪个视频文件（时间轴最早的那段有画面的素材）。
    pub(crate) cover: Option<PathBuf>,
}

/// 首页。
#[derive(Debug, Clone, Default)]
pub(crate) struct Home {
    pub(crate) cards: Vec<DraftCard>,
    /// 后台取好的封面（按卡片下标）。
    pub(crate) covers: std::collections::HashMap<usize, std::sync::Arc<gpui_kit::RenderImage>>,
}

/// 工程里时间轴最早的那段有画面的素材文件。
fn cover_of(project: &ve_core::Project) -> Option<PathBuf> {
    project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter_map(|c| {
            let a = project.assets.get(&c.asset()?)?;
            a.video.map(|_| (c.start, a.path.clone()))
        })
        .min_by_key(|(start, _)| *start)
        .map(|(_, path)| path)
}

/// 草稿文件夹里的 `.vep` 加上最近打开的，去重，按修改时间从新到旧，最多 [`MAX_CARDS`] 个。
pub(crate) fn collect(drafts: &Path, recent: &[PathBuf]) -> Vec<DraftCard> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(drafts)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("vep")))
        .collect();
    paths.extend(recent.iter().filter(|p| p.is_file()).cloned());
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| seen.insert(p.canonicalize().unwrap_or_else(|_| p.clone())));
    let mut cards: Vec<DraftCard> = paths
        .into_iter()
        .map(|path| {
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            let loaded = ve_core::project_file::load_project(&path).ok();
            let duration = loaded.as_ref().map(|l| l.project.timeline.duration());
            let cover = loaded.as_ref().and_then(|l| cover_of(&l.project));
            let name = path
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
            DraftCard {
                path,
                name,
                modified,
                duration,
                cover,
            }
        })
        .collect();
    cards.sort_by_key(|c| std::cmp::Reverse(c.modified));
    cards.truncate(MAX_CARDS);
    cards
}

/// 「3 分钟前」「2 小时前」「5 天前」。
pub(crate) fn ago(modified: Option<SystemTime>, now: SystemTime) -> String {
    let Some(secs) = modified
        .and_then(|m| now.duration_since(m).ok())
        .map(|d| d.as_secs())
    else {
        return String::new();
    };
    match secs {
        0..60 => "刚刚".to_owned(),
        60..3600 => format!("{} 分钟前", secs / 60),
        3600..86_400 => format!("{} 小时前", secs / 3600),
        _ => format!("{} 天前", secs / 86_400),
    }
}

/// 时长写成「01:05」或「1:02:03」。
fn short_duration(t: Time) -> String {
    let s = t.as_secs_f64().round();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "秒数"
    )]
    let s = s.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

impl Workspace {
    /// 打开首页，草稿从 `drafts` 文件夹（和最近打开）里找。
    pub fn show_home_in(&mut self, drafts: &Path, cx: &mut Context<'_, Self>) {
        let cards = collect(drafts, self.recent.paths());
        // 封面在后台取（每张卡一帧）
        for (i, cover) in cards
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.cover.clone().map(|p| (i, p)))
        {
            let work = cx.background_spawn(async move {
                ve_media::thumbnails(&cover, 1, COVER_HEIGHT)
                    .ok()
                    .and_then(|f| f.into_iter().next())
            });
            self.tasks.push(cx.spawn(async move |this, cx| {
                let frame = work.await;
                let _ignored = this.update(cx, |this, cx| {
                    if let (Some(home), Some(image)) = (
                        this.home.as_mut(),
                        frame.as_ref().and_then(crate::preview::to_render_image),
                    ) {
                        home.covers.insert(i, image);
                        cx.notify();
                    }
                });
            }));
        }
        self.home = Some(Home {
            cards,
            covers: std::collections::HashMap::new(),
        });
        cx.notify();
    }

    /// 首页有几张卡片已经取到封面（测试用）。
    #[must_use]
    pub fn home_covers(&self) -> usize {
        self.home.as_ref().map_or(0, |h| h.covers.len())
    }

    /// 打开首页（草稿在全局设置的草稿位置）。
    pub fn show_home(&mut self, cx: &mut Context<'_, Self>) {
        let drafts = self.settings.drafts();
        self.show_home_in(&drafts, cx);
    }

    /// 首页上的卡片：（工程文件，时长）（测试用）。
    #[must_use]
    pub fn home_cards(&self) -> Vec<(PathBuf, Option<Time>)> {
        self.home
            .as_ref()
            .map(|h| {
                h.cards
                    .iter()
                    .map(|c| (c.path.clone(), c.duration))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn show_home_action(
        &mut self,
        _: &crate::actions::ShowHome,
        _: &mut gpui_kit::Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.show_home(cx);
    }

    fn render_draft_card(&self, i: usize, card: &DraftCard, cx: &Context<'_, Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let cover = self.home.as_ref().and_then(|h| h.covers.get(&i)).cloned();
        let path = card.path.clone();
        let initial: String = card.name.chars().take(2).collect();
        v_flex()
            .id(SharedString::from(format!("home-draft-{i}")))
            .test_support()
            .w(px(180.))
            .gap_1()
            .cursor_pointer()
            .child(
                div()
                    .relative()
                    .h(px(110.))
                    .rounded(px(6.))
                    .bg(palette::timeline_bg())
                    .border_1()
                    .border_color(cx.theme().border)
                    .hover(|s| s.border_color(palette::accent()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .text_xl()
                    .text_color(muted)
                    .map(|tile| match cover {
                        Some(image) => tile.child(gpui_kit::img(image).size_full()),
                        None => tile.child(SharedString::from(initial)),
                    })
                    .children(card.duration.map(|d| {
                        div()
                            .absolute()
                            .right(px(6.))
                            .bottom(px(6.))
                            .px_1()
                            .rounded(px(3.))
                            .bg(gpui_kit::black().opacity(0.6))
                            .text_xs()
                            .text_color(gpui_kit::white())
                            .child(SharedString::from(short_duration(d)))
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .truncate()
                    .child(SharedString::from(card.name.clone())),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(ago(card.modified, SystemTime::now()))),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.home = None;
                this.open_path(&path, window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// 首页：「开始创作」和草稿卡片。
    pub(crate) fn render_home(&self, cx: &Context<'_, Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let cards = self.home.as_ref().map_or(&[][..], |h| &h.cards[..]);
        let start = v_flex()
            .id("home-new")
            .test_support()
            .w(px(380.))
            .h(px(150.))
            .rounded(px(10.))
            .bg(palette::accent().opacity(0.18))
            .border_1()
            .border_color(palette::accent())
            .hover(|s| s.bg(palette::accent().opacity(0.28)))
            .cursor_pointer()
            .items_center()
            .justify_center()
            .gap_2()
            .child(div().text_3xl().text_color(palette::accent()).child("+"))
            .child(div().text_lg().child("开始创作"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.home = None;
                cx.notify();
            }));
        let grid = if cards.is_empty() {
            div()
                .text_sm()
                .text_color(muted)
                .child("还没有草稿：点「开始创作」，导入素材后会自动保存成草稿")
                .into_any_element()
        } else {
            h_flex()
                .flex_wrap()
                .gap_4()
                .children(
                    cards
                        .iter()
                        .enumerate()
                        .map(|(i, c)| self.render_draft_card(i, c, cx)),
                )
                .into_any_element()
        };
        let body = v_flex()
            .id("home-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(48.))
            .py(px(24.))
            .gap_6()
            .child(start)
            .child(
                h_flex()
                    .gap_2()
                    .items_baseline()
                    .child(div().text_lg().child("草稿"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(SharedString::from(format!("{} 个", cards.len()))),
                    ),
            )
            .child(grid);
        // 没有系统标题栏：首页顶上也要有窗口按钮和能拖动窗口的地方
        let bar = gpui_kit::component::TitleBar::new()
            .h(px(crate::topbar::TOPBAR_HEIGHT))
            .bg(palette::window_bg())
            .border_color(palette::window_bg())
            .child(
                div()
                    .px_2()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child("剪辑"),
            );
        v_flex()
            .id("home")
            .test_support()
            .size_full()
            .child(bar)
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn ago_and_duration_read_naturally() {
        let now = SystemTime::now();
        assert_eq!(ago(Some(now - Duration::from_secs(5)), now), "刚刚");
        assert_eq!(ago(Some(now - Duration::from_secs(180)), now), "3 分钟前");
        assert_eq!(ago(Some(now - Duration::from_secs(7200)), now), "2 小时前");
        assert_eq!(ago(Some(now - Duration::from_hours(72)), now), "3 天前");
        assert_eq!(ago(None, now), "");
        assert_eq!(short_duration(Time::from_secs(65)), "01:05");
        assert_eq!(short_duration(Time::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn missing_folder_gives_no_cards() {
        let dir = tempfile::tempdir().unwrap();
        assert!(collect(&dir.path().join("没有"), &[]).is_empty());
    }
}
