//! 自定义快捷键（剪映「快捷键」里点一个键、再按新的键）。
//!
//! 每一条默认绑定是一个「键位」，用（动作名，默认键）认出来；改过的键位存在
//! `%APPDATA%/video-editor/shortcuts.json`（键位 → 新键，空字符串表示去掉这个键）。
//! 启动时和每次改键后，按「启动时的整张键位表 + 改动」重建（组件库自己的绑定原样保留）。

use gpui_kit::{App, Global, KeyBinding};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 本程序动作名的前缀（`actions!(editor, …)`）；别的前缀是组件库的，不让改。
const OURS: &str = "editor::";

/// 改过的键位：键位编号（`动作名|默认键`）→ 新键（GPUI 写法，如 `ctrl-k`；空 = 去掉）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Overrides(BTreeMap<String, String>);

impl Overrides {
    /// 读文件；没有或读不懂时当作没改过。
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// 写文件（目录不在时建好）。
    ///
    /// # Errors
    /// 写不进去。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// 键位现在用的键（没改过就是默认键）。
    #[must_use]
    pub fn current<'a>(&'a self, slot: &'a str) -> &'a str {
        self.0
            .get(slot)
            .map_or_else(|| default_key(slot), String::as_str)
    }

    /// 有没有改过。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// 默认存放位置：`%APPDATA%\video-editor\shortcuts.json`（取不到 APPDATA 时放当前目录）。
#[must_use]
pub fn default_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map_or_else(PathBuf::new, PathBuf::from)
        .join("video-editor")
        .join("shortcuts.json")
}

/// 键位编号：`动作名|默认键`。
#[must_use]
pub fn slot_id(action: &str, key: &str) -> String {
    format!("{action}|{key}")
}

/// 键位编号里的默认键。
fn default_key(slot: &str) -> &str {
    slot.rsplit_once('|').map_or("", |(_, k)| k)
}

/// 一条绑定的键（GPUI 写法，多段的用空格隔开）。
fn keys_of(b: &KeyBinding, cx: &App) -> String {
    let os = cx.platform_info().os;
    b.keystrokes()
        .iter()
        .map(|k| k.inner().unparse(os))
        .collect::<Vec<_>>()
        .join(" ")
}

/// App 里的快捷键状态：启动时的整张键位表、改动、存到哪。
struct Keymap {
    base: Vec<KeyBinding>,
    overrides: Overrides,
    path: Option<PathBuf>,
}

impl Global for Keymap {}

/// 记下启动时的整张键位表（组件库和本程序的都在），读入保存过的改动并生效。
/// `path` 为 `None` 时不读也不写文件（测试）。
pub fn install(cx: &mut App, path: Option<PathBuf>) {
    let base: Vec<KeyBinding> = cx.key_bindings().borrow().bindings().cloned().collect();
    let overrides = path.as_deref().map(Overrides::load).unwrap_or_default();
    cx.set_global(Keymap {
        base,
        overrides,
        path,
    });
    rebuild(cx);
}

/// 正式启动时：读 `path` 里保存过的改动并生效，以后改键也存到那里。
pub fn use_file(cx: &mut App, path: PathBuf) {
    if cx.try_global::<Keymap>().is_none() {
        install(cx, None);
    }
    let overrides = Overrides::load(&path);
    let map = cx.global_mut::<Keymap>();
    map.overrides = overrides;
    map.path = Some(path);
    rebuild(cx);
}

/// 按「启动时的键位表 + 改动」重建。
fn rebuild(cx: &mut App) {
    let Some(map) = cx.try_global::<Keymap>() else {
        return;
    };
    let mut out = Vec::with_capacity(map.base.len());
    for b in &map.base {
        let name = b.action().name();
        if !name.starts_with(OURS) {
            out.push(b.clone());
            continue;
        }
        let slot = slot_id(name, &keys_of(b, cx));
        match map.overrides.0.get(&slot) {
            None => out.push(b.clone()),
            Some(k) if k.is_empty() => {}
            Some(k) => {
                match KeyBinding::load(k, b.action().boxed_clone(), b.predicate(), false, None, cx)
                {
                    Ok(nb) => out.push(nb),
                    Err(e) => {
                        log::warn!("快捷键 {k} 读不懂（{e}），{name} 用回默认键");
                        out.push(b.clone());
                    }
                }
            }
        }
    }
    cx.clear_key_bindings();
    cx.bind_keys(out);
}

/// 默认键 `key`（GPUI 写法）对应的本程序键位编号。
#[must_use]
pub fn slot_for_default(cx: &App, key: &str) -> Option<String> {
    let map = cx.try_global::<Keymap>()?;
    let want = gpui_kit::Keystroke::parse_with_os(key, cx.platform_info().os).ok()?;
    map.base.iter().find_map(|b| {
        let name = b.action().name();
        let [k] = b.keystrokes() else {
            return None;
        };
        let k = k.inner();
        (name.starts_with(OURS) && k.modifiers == want.modifiers && k.key == want.key)
            .then(|| slot_id(name, &keys_of(b, cx)))
    })
}

/// 键位现在的键。
#[must_use]
pub fn current(cx: &App, slot: &str) -> String {
    cx.try_global::<Keymap>().map_or_else(
        || default_key(slot).to_owned(),
        |m| m.overrides.current(slot).to_owned(),
    )
}

/// 有没有改过的键位。
#[must_use]
pub fn customized(cx: &App) -> bool {
    cx.try_global::<Keymap>()
        .is_some_and(|m| !m.overrides.is_empty())
}

/// 给键位换一个键。别的键位正在用这个键时把它去掉（同一个键只做一件事），返回被去掉的那个键位。
pub fn assign(cx: &mut App, slot: &str, key: &str) -> Option<String> {
    let all_slots = all_slots(cx);
    let map = cx.try_global::<Keymap>()?;
    let taken = all_slots
        .into_iter()
        .find(|s| s != slot && map.overrides.current(s) == key);
    let map = cx.global_mut::<Keymap>();
    if let Some(other) = &taken {
        map.overrides.0.insert(other.clone(), String::new());
    }
    if default_key(slot) == key {
        map.overrides.0.remove(slot);
    } else {
        map.overrides.0.insert(slot.to_owned(), key.to_owned());
    }
    persist(cx);
    rebuild(cx);
    taken
}

/// 全部恢复默认。
pub fn reset(cx: &mut App) {
    if let Some(map) = cx.try_global::<Keymap>()
        && !map.overrides.is_empty()
    {
        cx.global_mut::<Keymap>().overrides = Overrides::default();
        persist(cx);
        rebuild(cx);
    }
}

/// 本程序所有键位的编号。
fn all_slots(cx: &App) -> Vec<String> {
    cx.try_global::<Keymap>()
        .map(|m| {
            m.base
                .iter()
                .filter(|b| b.action().name().starts_with(OURS))
                .map(|b| slot_id(b.action().name(), &keys_of(b, cx)))
                .collect()
        })
        .unwrap_or_default()
}

fn persist(cx: &App) {
    if let Some(map) = cx.try_global::<Keymap>()
        && let Some(path) = &map.path
        && let Err(e) = map.overrides.save(path)
    {
        log::warn!("快捷键没存上：{e}");
    }
}

/// 一览里的写法 → GPUI 写法（`Ctrl+Shift+B` → `ctrl-shift-b`）。
#[must_use]
pub fn to_binding(shown: &str) -> String {
    // `Ctrl+-`、`Ctrl+=`：最后一段是键本身
    let (mods, key) = shown.rsplit_once('+').map_or(("", shown), |(m, k)| {
        if k.is_empty() {
            (m.trim_end_matches('+'), "+")
        } else {
            (m, k)
        }
    });
    let key = match key {
        "空格" => "space".to_owned(),
        "←" => "left".to_owned(),
        "→" => "right".to_owned(),
        "↑" => "up".to_owned(),
        "↓" => "down".to_owned(),
        "Esc" => "escape".to_owned(),
        other => other.to_lowercase(),
    };
    mods.split('+')
        .filter(|m| !m.is_empty())
        .map(str::to_lowercase)
        .chain(std::iter::once(key))
        .collect::<Vec<_>>()
        .join("-")
}

/// 修饰键的 GPUI 写法 → 一览里的写法。
const MODS: [(&str, &str); 6] = [
    ("ctrl-", "Ctrl"),
    ("alt-", "Alt"),
    ("shift-", "Shift"),
    ("win-", "Win"),
    ("cmd-", "Win"),
    ("platform-", "Win"),
];

/// GPUI 写法 → 一览里的写法（`ctrl-shift-b` → `Ctrl+Shift+B`；空 = 「未设置」）。
#[must_use]
pub fn to_shown(binding: &str) -> String {
    if binding.is_empty() {
        return "未设置".to_owned();
    }
    let mut rest = binding;
    let mut parts = Vec::new();
    while let Some((m, tail)) = MODS
        .iter()
        .find_map(|(p, shown)| rest.strip_prefix(p).map(|t| (*shown, t)))
    {
        parts.push(m.to_owned());
        rest = tail;
    }
    let key = match rest {
        "space" => "空格".to_owned(),
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        "up" => "↑".to_owned(),
        "down" => "↓".to_owned(),
        "escape" => "Esc".to_owned(),
        k => {
            let mut c = k.chars();
            c.next().map_or_else(String::new, |f| {
                f.to_uppercase().chain(c).collect::<String>()
            })
        }
    };
    parts.push(key);
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_and_binding_round_trip() {
        for (shown, bind) in [
            ("Ctrl+Shift+B", "ctrl-shift-b"),
            ("Ctrl+-", "ctrl--"),
            ("Ctrl+=", "ctrl-="),
            ("空格", "space"),
            ("Shift+Delete", "shift-delete"),
            ("Alt+Shift+M", "alt-shift-m"),
            ("←", "left"),
            ("Esc", "escape"),
            ("[", "["),
            ("`", "`"),
        ] {
            assert_eq!(to_binding(shown), bind, "{shown}");
            assert_eq!(to_shown(bind), shown, "{bind}");
        }
        assert_eq!(to_shown(""), "未设置");
    }

    #[test]
    fn overrides_save_load_and_fall_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("子目录").join("shortcuts.json");
        let mut o = Overrides::default();
        assert_eq!(o.current("editor::Split|ctrl-b"), "ctrl-b");
        o.0.insert("editor::Split|ctrl-b".into(), "ctrl-k".into());
        o.save(&path).unwrap();
        let back = Overrides::load(&path);
        assert_eq!(back, o);
        assert_eq!(back.current("editor::Split|ctrl-b"), "ctrl-k");
        std::fs::write(&path, "坏了").unwrap();
        assert!(Overrides::load(&path).is_empty(), "读不懂当作没改过");
    }
}
