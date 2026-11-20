//! 特效插件（达芬奇的 OFX、Premiere 的效果插件的轻量版）：插件目录里每个 `.wgsl` 文件是一个
//! 画面特效，文件名就是特效名。写法见 `docs/plugins.md` 和 [`ve_render::plugin`]。
//!
//! 启动时载入；目录还不存在时建好并放两个示例。素材面板「特效」里列出来，点一下加到播放头处。

use crate::workspace::{Workspace, notify};
use gpui_kit::component::notification::Notification;
use gpui_kit::{Context, Window};
use std::path::{Path, PathBuf};

/// 示例插件：反色。
const EXAMPLE_INVERT: &str = "// 反色：强度 0 不变，1 完全反过来
fn plugin_effect(c: vec3<f32>, p: vec2<f32>, dims: vec2<f32>, amount: f32, t: f32) -> vec3<f32> {
    return mix(c, vec3(1.0) - c, amount);
}
";

/// 示例插件：老电视（扫描线 + 轻微闪烁）。
const EXAMPLE_TV: &str = "// 老电视：每隔两行压暗一行，整体随时间轻微闪烁
fn plugin_effect(c: vec3<f32>, p: vec2<f32>, dims: vec2<f32>, amount: f32, t: f32) -> vec3<f32> {
    let line = select(1.0, 0.6, (i32(p.y) % 3) == 0);
    let flicker = 1.0 - 0.05 * sin(t * 60.0);
    return mix(c, c * line * flicker, amount);
}
";

/// 默认插件目录：`%APPDATA%\\video-editor\\effects`。
#[must_use]
pub fn default_dir() -> PathBuf {
    crate::recent::default_file()
        .parent()
        .map_or_else(|| PathBuf::from("effects"), |p| p.join("effects"))
}

/// 目录还不存在时建好，放两个示例插件。
///
/// # Errors
/// 建不了目录、写不了文件。
pub fn seed_examples(dir: &Path) -> std::io::Result<()> {
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("反色.wgsl"), EXAMPLE_INVERT)?;
    std::fs::write(dir.join("老电视.wgsl"), EXAMPLE_TV)?;
    Ok(())
}

/// 载入目录里的插件：返回（载入成功的名字，载入失败的（文件名，原因））。
#[must_use]
pub fn load_dir(dir: &Path) -> (Vec<String>, Vec<(String, String)>) {
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (ok, bad);
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("wgsl"))
        })
        .collect();
    files.sort();
    for path in files {
        let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        if ve_core::effect::PluginName::new(&name).is_none() {
            bad.push((name, "名字太长（最多 48 字节）".to_owned()));
            continue;
        }
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|src| ve_render::plugin::register(&name, &src))
        {
            Ok(()) => ok.push(name),
            Err(e) => bad.push((name, e)),
        }
    }
    (ok, bad)
}

impl Workspace {
    /// 换插件目录并载入（测试用、启动时）。
    pub fn use_plugin_dir(
        &mut self,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.plugin_dir = dir;
        self.reload_plugins(window, cx);
    }

    /// 载入了哪些插件。
    #[must_use]
    pub fn plugins(&self) -> &[String] {
        &self.plugins
    }

    /// 重新读插件目录；有写错的插件时提示是哪个、错在哪。
    pub(crate) fn reload_plugins(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let (ok, bad) = load_dir(&self.plugin_dir);
        self.plugins = ok;
        if let Some((name, why)) = bad.first() {
            let first_line = why.lines().find(|l| !l.trim().is_empty()).unwrap_or(why);
            notify(
                Notification::warning(format!(
                    "{} 个插件没载入：{name}（{first_line}）",
                    bad.len()
                )),
                window,
                cx,
            );
        }
        // 插件源码变了：预览重画
        self.engine_revision = None;
        self.sync_engine();
        cx.notify();
    }

    /// 在播放头处加一段插件特效 `name`。
    pub fn add_plugin_effect_at_playhead(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(effect) = ve_core::Effect::plugin(name) else {
            return;
        };
        let at = self.playhead_frame();
        if let Some(clip) = self.apply(window, cx, |p| {
            ve_core::edit::add_effect(p, at, ve_core::edit::ADJUSTMENT_LENGTH, effect)
        }) {
            self.doc.select(clip, false);
            self.inspector.tab = crate::inspector::InspectorTab::Effect;
            self.after_change(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn examples_compile_and_broken_files_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("effects");
        seed_examples(&plugins).unwrap();
        std::fs::write(plugins.join("坏的.wgsl"), "fn 不对 {").unwrap();
        std::fs::write(plugins.join("说明.txt"), "不是插件").unwrap();
        let (ok, bad) = load_dir(&plugins);
        assert_eq!(ok, ["反色", "老电视"]);
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].0, "坏的");
        // 已经有目录：不再放示例
        std::fs::remove_file(plugins.join("反色.wgsl")).unwrap();
        seed_examples(&plugins).unwrap();
        assert!(!plugins.join("反色.wgsl").exists());
        assert_eq!(load_dir(&dir.path().join("没有")), (Vec::new(), Vec::new()));
    }
}
