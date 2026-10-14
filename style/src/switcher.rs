//! 方案选单的数据层：对应官方 `SwitcherSettingsDialog` 读写的两样东西。
//!
//! 官方走 levers API（`get_available_schema_list` / `get_selected_schema_list` /
//! `get_hotkeys`）。我们没有 levers，但同样的信息就在数据目录里，于是直接读文件：
//!
//! * **可用方案** = `<共享目录>\schema\<方案>\<方案>.schema.yaml` 的集合。
//!   每个方案文件里的 `schema/name`（显示名）、`schema/author`、`schema/description`
//!   就是官方简介框显示的三行。
//! * **已选方案** = `default.yaml` 的 `schema_list`（叠加用户补丁后的结果）。
//!   官方的列表里，已选中的排前面并打勾，其余可用方案接在后面不打勾。
//! * **写回** = `patch: schema_list:` 一段 YAML 列表，写进 `default.custom.yaml`，
//!   与官方 `customize_string("schema_list", ...)` 落到同一个位置。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 列表里的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaInfo {
    /// 方案 id（`<方案>.schema.yaml` 的文件名主干）。
    pub id: String,
    /// 显示名（`schema/name`，缺失时退回 id）。
    pub name: String,
    /// 作者（`schema/author`）。
    pub author: String,
    /// 简介（`schema/description`）。
    pub description: String,
    /// 是否已选入方案选单。
    pub selected: bool,
}

/// 从方案文件里抠出 `schema:` 段下的三个字段。
///
/// 只做**行级**解析：这三个字段在官方方案文件里都是 `schema:` 段下的单行标量，
/// 值可能带引号。这样不必引入完整 YAML 解析器，也不怕方案文件里的其它结构。
fn read_schema_fields(text: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut name = None;
    let mut author = None;
    let mut description = None;
    let lines: Vec<&str> = text.lines().collect();
    let mut in_schema = false;
    let mut k = 0;
    while k < lines.len() {
        let trimmed = lines[k].trim_end();
        if trimmed.starts_with("schema:") && !trimmed.starts_with("schema/") {
            in_schema = true;
            k += 1;
            continue;
        }
        if in_schema {
            // 缩进结束说明 schema 段结束了
            if !trimmed.is_empty() && !trimmed.starts_with(' ') && !trimmed.starts_with('\t') {
                in_schema = false;
                k += 1;
                continue;
            }
            let stripped = trimmed.trim_start();
            let base_indent = trimmed.len() - stripped.len();
            for (key, slot) in [
                ("name:", &mut name),
                ("author:", &mut author),
                ("description:", &mut description),
            ] {
                if let Some(rest) = stripped.strip_prefix(key) {
                    let inline = rest.trim();
                    if inline == "|" || inline == ">" || inline == "|-" || inline == ">-" {
                        // 块状标量：把后面比键更深的缩进行整段取回来
                        let mut block: Vec<&str> = Vec::new();
                        let mut j = k + 1;
                        while j < lines.len() {
                            let next = lines[j].trim_end();
                            let next_stripped = next.trim_start();
                            let next_indent = next.len() - next_stripped.len();
                            if next.trim().is_empty() {
                                block.push("");
                                j += 1;
                                continue;
                            }
                            if next_indent <= base_indent {
                                break;
                            }
                            block.push(next_stripped);
                            j += 1;
                        }
                        while block.last().is_some_and(|line| line.is_empty()) {
                            block.pop();
                        }
                        if !block.is_empty() {
                            *slot = Some(block.join("\n"));
                        }
                        k = j;
                        break;
                    }
                    let value = inline.trim_matches(['"', '\''].as_ref()).trim();
                    if !value.is_empty() {
                        *slot = Some(value.to_string());
                    }
                }
            }
        }
        k += 1;
    }
    (name, author, description)
}

/// 读 `schema_list`（`- schema: <id>` 形式）。
fn read_schema_list(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut in_list = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with("schema_list:") {
            in_list = true;
            continue;
        }
        if in_list {
            if !trimmed.is_empty() && !trimmed.starts_with(' ') && !trimmed.starts_with('\t') {
                break;
            }
            let stripped = trimmed.trim_start();
            if let Some(rest) = stripped.strip_prefix("- ") {
                let value = rest.trim();
                let id = value
                    .strip_prefix("schema:")
                    .map(str::trim)
                    .unwrap_or(value)
                    .trim_matches(['"', '\''].as_ref());
                if !id.is_empty() {
                    result.push(id.to_string());
                }
            }
        }
    }
    result
}

/// 列出全部方案：已选中的在前（按 `schema_list` 顺序），其余按 id 排在后面。
pub fn list_schemas(shared: &Path, custom_path: &Path) -> io::Result<Vec<SchemaInfo>> {
    let schema_root = shared.join("schema");
    let mut available: BTreeMap<String, (String, String, String)> = BTreeMap::new();
    if schema_root.is_dir() {
        for entry in fs::read_dir(&schema_root)? {
            let dir = entry?.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(id) = dir.file_name().map(|n| n.to_string_lossy().to_string()) else {
                continue;
            };
            let file = dir.join(format!("{id}.schema.yaml"));
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            let (name, author, description) = read_schema_fields(&text);
            available.insert(
                id.clone(),
                (
                    name.unwrap_or_else(|| id.clone()),
                    author.unwrap_or_default(),
                    description.unwrap_or_default(),
                ),
            );
        }
    }

    // 已选列表：出厂 default.yaml 叠加用户补丁（补丁用文本覆盖，见 parse_with_patch 的语义）
    let base_text = fs::read_to_string(shared.join("default.yaml")).unwrap_or_default();
    let mut selected = read_schema_list(&base_text);
    if let Ok(custom_text) = fs::read_to_string(custom_path) {
        let patched = read_schema_list(&custom_text);
        if !patched.is_empty() {
            selected = patched;
        }
    }

    let mut rows = Vec::with_capacity(available.len());
    for id in &selected {
        if let Some((name, author, description)) = available.remove(id) {
            rows.push(SchemaInfo {
                id: id.clone(),
                name,
                author,
                description,
                selected: true,
            });
        }
    }
    for (id, (name, author, description)) in available {
        rows.push(SchemaInfo {
            id,
            name,
            author,
            description,
            selected: false,
        });
    }
    Ok(rows)
}

/// 把勾选结果写进 `default.custom.yaml` 的 `patch: schema_list:`。
///
/// 这一段是 **YAML 列表**（不是路径式键），所以单独写：
/// 找到已有的 `schema_list:` 段就地替换，没有就追加到 `patch:` 段里。
pub fn set_schema_list(custom_path: &Path, ids: &[String]) -> io::Result<()> {
    let existing = fs::read_to_string(custom_path).unwrap_or_default();
    let lines: Vec<&str> = existing.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + ids.len() + 2);
    let mut replaced = false;
    let mut has_patch = false;
    let mut in_patch = false;
    let mut skipping = false;

    for line in &lines {
        let trimmed = line.trim_end();
        if skipping {
            // 跳过旧的 schema_list 内容（缩进的 `- schema:` 行）
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with(' ') || trimmed.starts_with('\t') {
                continue;
            }
            skipping = false;
        }
        if trimmed == "patch:" {
            has_patch = true;
            in_patch = true;
            out.push(trimmed.to_string());
            continue;
        }
        if in_patch && !trimmed.is_empty() && !trimmed.starts_with(' ') && !trimmed.starts_with('\t') {
            in_patch = false;
        }
        if in_patch && trimmed.trim_start().starts_with("schema_list:") {
            let indent = &trimmed[..trimmed.len() - trimmed.trim_start().len()];
            out.push(format!("{indent}schema_list:"));
            for id in ids {
                out.push(format!("{indent}  - schema: {id}"));
            }
            replaced = true;
            skipping = true;
            continue;
        }
        out.push(trimmed.to_string());
    }

    if !replaced {
        if !has_patch {
            if !out.is_empty() {
                out.push(String::new());
            }
            out.push("patch:".to_string());
        }
        out.push("  schema_list:".to_string());
        for id in ids {
            out.push(format!("  - schema: {id}"));
        }
    }

    let text = out.join("\n") + "\n";
    if let Some(parent) = custom_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(custom_path, text)
}

/// `default.custom.yaml` 的位置（与 `weasel.custom.yaml` 同目录）。
pub fn user_default_custom_path(user_dir: &Path) -> PathBuf {
    user_dir.join("default.custom.yaml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_shared() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Rime-Next")
    }

    #[test]
    fn reads_factory_fields() {
        let (name, author, description) =
            read_schema_fields("schema:\n  name: \"朙月拼音\"\n  author: 佛振\n  description: |\n    注音\n");
        assert_eq!(name.as_deref(), Some("朙月拼音"));
        assert_eq!(author.as_deref(), Some("佛振"));
        assert_eq!(description.as_deref(), Some("注音"), "块状标量应整段读出");
    }

    #[test]
    fn reads_schema_list() {
        let ids = read_schema_list("schema_list:\n  - schema: luna_pinyin\n  - schema: stroke\n");
        assert_eq!(ids, vec!["luna_pinyin", "stroke"]);
    }

    #[test]
    fn lists_available_with_selected_first() {
        let shared = repo_shared();
        let custom = std::env::temp_dir().join("weasel-style-switcher-none.yaml");
        let _ = std::fs::remove_file(&custom);
        let rows = list_schemas(&shared, &custom).expect("应能列出方案");
        println!("方案 {} 个：", rows.len());
        for row in rows.iter().take(12) {
            println!(
                "  {} [{}] {} — {}",
                row.id,
                if row.selected { "已选" } else { "未选" },
                row.name,
                row.author
            );
        }
        assert!(rows.len() >= 8, "至少 8 个方案：实际 {}", rows.len());
        assert!(rows[0].selected, "已选方案应排在前面");
        assert!(rows.iter().any(|r| r.id == "luna_pinyin" && r.selected));
        assert!(rows.iter().any(|r| !r.selected), "应存在未选方案");
        assert!(rows.iter().all(|r| !r.name.is_empty()), "每个方案都要有显示名");
    }

    #[test]
    fn writes_schema_list_patch() {
        let path = std::env::temp_dir().join("weasel-style-switcher-write.yaml");
        let _ = std::fs::remove_file(&path);
        set_schema_list(&path, &["luna_pinyin".into(), "stroke".into()]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        println!("写入结果：\n{text}");
        assert!(text.contains("patch:"), "应有 patch 段");
        assert!(text.contains("schema_list:"), "应有 schema_list");
        assert!(text.contains("- schema: luna_pinyin"), "应含第一个方案");
        assert!(text.contains("- schema: stroke"), "应含第二个方案");
        // 再写一次应就地替换而不是叠加
        set_schema_list(&path, &["cangjie5".into()]).unwrap();
        let text2 = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text2.matches("schema_list:").count(), 1, "不应重复：{text2}");
        assert!(text2.contains("- schema: cangjie5"));
        assert!(!text2.contains("luna_pinyin"), "旧列表应被替换：{text2}");
        let _ = std::fs::remove_file(&path);
    }
}
