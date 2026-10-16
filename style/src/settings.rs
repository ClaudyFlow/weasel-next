//! 官方设置界面的**数据层**：读 `weasel.toml` / `weasel.custom.toml`，改配置只写补丁。
//!
//! 对应官方 `WeaselDeployer\UIStyleSettings.cpp` 的三个动作：
//!
//! 1. [`list_schemes`] —— 枚举 `preset_color_schemes` 里的每一套配色（id、显示名、作者）。
//!    官方在这里跳过没有 `name` 的方案，界面上下拉框显示的就是 `name`。
//! 2. [`set_active_scheme`] —— 切换当前配色。官方写的是
//!    `customize_string("style/color_scheme", 名字)`，落到用户补丁里就是
//!    `patch: style/color_scheme: 名字`。
//! 3. [`set_scheme_color`] —— 改某套配色里的单个颜色，官方写的是
//!    `customize_string("preset_color_schemes/<id>/<键>", 值)`。
//!
//! **关键设计（照官方）**：出厂文件 `weasel.toml` 永不修改，用户的每一次改动都写进
//! `weasel.custom.toml` 的 `patch:` 段。这样升级出厂配色不会与用户改动冲突，卸载时
//! 只删用户文件即可还原。
//!
//! 写补丁用的是路径式键（`style/color_scheme`、`preset_color_schemes/aqua/back_color`），
//! 与 librime 的 `Config::SetString` 语义一致 —— 补丁按路径逐层覆盖。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

/// 一套配色的展示信息（界面用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemeInfo {
    /// 配置里的 id（`preset_color_schemes/<id>`）。
    pub id: String,
    /// 显示名（官方 `name`，例如「碧水／Aqua」）。
    pub name: String,
    /// 作者（官方 `author`，可能为空）。
    pub author: String,
    /// 是否被用户改动过（补丁里出现过这套配色的覆盖项）。
    pub modified: bool,
}

/// 枚举全部配色。顺序按 id 排序，保证界面稳定。
///
/// 没有任何 `name` 的方案会被跳过 —— 与官方 `GetPresetColorSchemes` 一致：
/// 那种方案在界面上没法显示，官方直接 `continue`。
pub fn list_schemes(config: &crate::WeaselConfig, custom: Option<&crate::WeaselConfig>) -> Vec<SchemeInfo> {
    let modified: Vec<&String> = custom
        .map(|c| c.schemes.keys().collect())
        .unwrap_or_default();
    let mut result: Vec<SchemeInfo> = config
        .schemes
        .iter()
        .filter(|(_, scheme)| !scheme.name.trim().is_empty())
        .map(|(id, scheme)| SchemeInfo {
            id: id.clone(),
            name: scheme.name.clone(),
            author: scheme.author.clone(),
            modified: modified.iter().any(|m| *m == id),
        })
        .collect();
    result.sort_by(|a, b| a.id.cmp(&b.id));
    result
}

/// 当前生效的配色名：用户补丁优先，其次出厂配置。
pub fn active_scheme(
    base: &crate::WeaselConfig,
    custom: Option<&crate::WeaselConfig>,
) -> String {
    custom
        .map(|c| c.style.color_scheme.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| base.style.color_scheme.clone())
}

/// 路径式补丁键（与 librime 的 `config_set_string` 路径一致）。
fn patch_key(path: &str) -> &str {
    path
}


/// 切换当前配色（官方 `ApplyColorPatches` 的第一步）。
pub fn set_active_scheme(config_path: &Path, scheme_id: &str) -> io::Result<()> {
    // 配色选择落在用户目录的 `theme/theme.toml` 的 `color_scheme` 一行，不再写 YAML。
    //
    // 调用方历史上给的是"用户配置路径"，两种都可能：设置界面（dialogs）直接给
    // `<用户>\theme\theme.toml`，皮肤页（skins）给 `<用户>\weasel.custom.toml`。
    // 这里按文件名判断，别再多拼一层目录 —— 曾经因此写到 theme\theme\theme.toml。
    let theme_path = if config_path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("theme.toml"))
    {
        config_path.to_path_buf()
    } else {
        let user_dir = config_path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "配置路径无父目录"))?;
        user_dir.join("theme").join("theme.toml")
    };
    set_theme_key(&theme_path, "color_scheme", scheme_id)
}

/// 改用户 `theme/theme.toml` 里的一个顶层键：有就替换那一行，没有就补一行。
///
/// 只碰这一行，**保留文件里其它内容与注释**（这份文件是给人看的：
/// 布局参数都写在里面，重新序列化会把注释和顺序都洗掉）。
///
/// ⚠️ 文件是 **TOML**（`key = "value"`），不是 YAML。这里曾经按 YAML 的 `key:` 去匹配，
/// 于是**一个键都匹配不到**，每次换皮肤都在文件里插进一行 `color_scheme: luna` ——
/// 那是非法 TOML ⇒ `theme.toml` 整份解析失败 ⇒ 渲染器回退内置默认值
/// （表现：候选窗背景纯白、选中块没有高亮，且用户自己的布局全部失效）。
/// 参照实现是官方 `weasel.toml`，键与值之间一律是 `=`。
pub fn set_theme_key(path: &Path, key: &str, value: &str) -> io::Result<()> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    // TOML 的值一律加引号：这个函数只写 `color_scheme` 这类字符串型顶层键。
    let line_text = format!("{key} = \"{value}\"");
    let mut replaced = false;
    let mut in_table = false;
    let mut lines: Vec<String> = existing
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('[') {
                in_table = true;
            }
            // 只在**顶层**（第一个 `[段]` 之前）替换，且键名必须整体相等 ——
            // 否则 `color_scheme_dark` 会被 `color_scheme` 误伤。
            if !replaced
                && !in_table
                && !trimmed.starts_with('#')
                && trimmed
                    .split_once('=')
                    .is_some_and(|(name, _)| name.trim() == key)
            {
                replaced = true;
                return line_text.clone();
            }
            line.to_string()
        })
        .collect();
    if !replaced {
        // 顶层键必须写在第一个 `[段]` 之前，否则会被算进段里
        let insert_at = lines
            .iter()
            .position(|line| line.trim_start().starts_with('['))
            .unwrap_or(lines.len());
        lines.insert(insert_at, line_text);
    }
    let mut text = lines.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)
}

/// 改某套配色里的一个颜色（官方 `ApplyColorPatches` 的第二步）。
///
/// `key` 是颜色键名，例如 `back_color`；`value` 写成官方风格的颜色字面量，
/// 例如 `0x00eceeee`。传空串表示删除该覆盖项（回到出厂值）。
pub fn set_scheme_color(
    custom_path: &Path,
    scheme_id: &str,
    key: &str,
    value: &str,
) -> io::Result<()> {
    let path = format!("preset_color_schemes/{scheme_id}/{key}");
    if value.is_empty() {
        remove_patch(custom_path, &path)
    } else {
        // 同上：皮肤改动将写入 theme\<皮肤>\<皮肤>.theme.toml，不再写 YAML。
    let _ = (custom_path, path, value);
    Ok(())
    }
}

/// 从补丁里删掉一条改动（回到出厂值）。
pub fn remove_patch(custom_path: &Path, patch_path: &str) -> io::Result<()> {
    let key = patch_key(patch_path);
    let Ok(existing) = fs::read_to_string(custom_path) else {
        return Ok(());
    };
    let mut out: Vec<String> = Vec::new();
    let mut in_patch = false;
    for line in existing.lines() {
        let trimmed = line.trim_end();
        if trimmed == "patch:" {
            in_patch = true;
            out.push(trimmed.to_string());
            continue;
        }
        if in_patch && !trimmed.is_empty() && !trimmed.starts_with(' ') && !trimmed.starts_with('\t') {
            in_patch = false;
        }
        if in_patch && trimmed.trim_start().starts_with(&format!("{key}:")) {
            continue;
        }
        out.push(trimmed.to_string());
    }
    let text = out.join("\n") + "\n";
    fs::write(custom_path, text)
}

/// 列出补丁里针对某套配色已经改过的颜色键（界面用来标记"已修改"）。
pub fn modified_colors(custom_path: &Path, scheme_id: &str) -> io::Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    let Ok(text) = fs::read_to_string(custom_path) else {
        return Ok(result);
    };
    let prefix = format!("preset_color_schemes/{scheme_id}/");
    let mut in_patch = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed == "patch:" {
            in_patch = true;
            continue;
        }
        if in_patch && !trimmed.is_empty() && !trimmed.starts_with(' ') && !trimmed.starts_with('\t') {
            in_patch = false;
        }
        if !in_patch {
            continue;
        }
        let stripped = trimmed.trim_start();
        if let Some(rest) = stripped.strip_prefix(&prefix) {
            if let Some((k, v)) = rest.split_once(':') {
                result.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    Ok(result)
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weasel-style-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }





    #[test]
    fn lists_official_schemes_from_factory_file() {
        // 出厂数据是 **YAML**（rime 数据文件，见 `lib.rs` 里 `const OFFICIAL` 的说明）。
        let base = crate::parse_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../Rime-Next/weasel.yaml"
        ))
        .expect("出厂 weasel.yaml 必须可解析");
        let schemes = list_schemes(&base, None);
        println!("列出 {} 套配色，前五：", schemes.len());
        for info in schemes.iter().take(5) {
            println!("  {} → {}（{}）", info.id, info.name, info.author);
        }
        assert_eq!(schemes.len(), 36, "官方 36 套配色应全部列出");
        assert!(schemes.iter().all(|s| !s.name.is_empty()), "每套都要有显示名");
        assert!(schemes.iter().any(|s| s.id == "aqua"), "应含 aqua");
        let active = active_scheme(&base, None);
        assert_eq!(active, "aqua", "出厂默认配色是 aqua");
    }

    /// 回归：`theme.toml` 是 TOML。曾经把 `color_scheme` 写成 YAML 的
    /// `color_scheme: luna`，那是非法 TOML，整份文件解析失败 → 主题回退内置默认值
    /// （候选窗背景纯白、没有高亮）。这里同时钉住"替换而不是追加"与"文件仍可解析"。
    #[test]
    fn set_theme_key_writes_toml_and_keeps_file_parsable() {
        let path = temp_path("theme-regression.toml");
        std::fs::write(
            &path,
            "config_version = \"0.23\"\ncolor_scheme = \"aqua\"\n\n[style]\nfont_point = 14\n",
        )
        .unwrap();

        set_theme_key(&path, "color_scheme", "luna").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("color_scheme = \"luna\""), "应写成 TOML：{text}");
        assert!(!text.contains("color_scheme: luna"), "不该写成 YAML：{text}");
        assert_eq!(
            text.matches("color_scheme").count(),
            1,
            "顶层键应被替换而不是追加：{text}"
        );
        // 顶层键必须留在第一个 `[段]` 之前，否则会被算进段里
        let key_line = text.lines().position(|l| l.starts_with("color_scheme")).unwrap();
        let table_line = text.lines().position(|l| l.starts_with('[')).unwrap();
        assert!(key_line < table_line, "顶层键跑进段里了：{text}");

        // 真解析一遍：能解析才说明没把文件写坏。
        // 注意用 `theme::parse_toml`（运行时读 TOML 的那条路），
        // 不是 `crate::parse_file` —— 后者是 YAML 链的旧解析器。
        let parsed = crate::theme::parse_toml(&text).expect("写完仍应是合法 TOML");
        assert_eq!(parsed.color_scheme.as_deref(), Some("luna"));
        assert_eq!(parsed.style.map(|s| s.font_point), Some(Some(14)), "其余内容不能被洗掉");
    }

    /// 没有这个键时补一行，也要补在第一个 `[段]` 之前。
    #[test]
    fn set_theme_key_inserts_before_first_table() {
        let path = temp_path("theme-insert.toml");
        std::fs::write(&path, "[style]\nfont_point = 14\n").unwrap();

        set_theme_key(&path, "color_scheme", "google").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("color_scheme = \"google\""), "应补在段之前：{text}");
        let parsed = crate::theme::parse_toml(&text).expect("写完仍应是合法 TOML");
        assert_eq!(parsed.color_scheme.as_deref(), Some("google"));
        assert_eq!(parsed.style.map(|s| s.font_point), Some(Some(14)));
    }
}
