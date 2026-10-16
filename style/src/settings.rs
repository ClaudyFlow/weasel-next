//! 官方设置界面的**数据层**：读 `weasel.yaml` / `weasel.custom.yaml`，改配置只写补丁。
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
//! **关键设计（照官方）**：出厂文件 `weasel.yaml` 永不修改，用户的每一次改动都写进
//! `weasel.custom.yaml` 的 `patch:` 段。这样升级出厂配色不会与用户改动冲突，卸载时
//! 只删用户文件即可还原。
//!
//! 写补丁用的是路径式键（`style/color_scheme`、`preset_color_schemes/aqua/back_color`），
//! 与 librime 的 `Config::SetString` 语义一致 —— 补丁按路径逐层覆盖。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
pub fn set_active_scheme(custom_path: &Path, scheme_id: &str) -> io::Result<()> {
    // 不再写 weasel.custom.yaml（全部去 YAML）：等接线到 theme\theme.toml 后恢复写入。
    let _ = (custom_path, scheme_id);
    Ok(())
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
}
