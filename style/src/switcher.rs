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
/// 列出全部方案：已选中的在前（按 `schema.toml` 里的顺序，即 a–z），其余按 id 排在后面。
///
/// 方案来自**两处** `schema\` 目录：程序文件夹的出厂方案与用户文件夹的方案，
/// 同名以用户那份为准。已选清单读 `schema.toml`（用户目录优先、回退出厂）。
pub fn list_schemas(shared: &Path, user: &Path) -> io::Result<Vec<SchemaInfo>> {
    let mut available: BTreeMap<String, (String, String, String)> = BTreeMap::new();
    // 先出厂、后用户：同名时用户的覆盖出厂的。
    for root in [shared.join("schema"), user.join("schema")] {
        if !root.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&root)? {
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

    // 已选清单：读 schema.toml（用户目录那份优先，回退出厂那份）。不再读写任何 YAML。
    let selected = read_schema_toml(shared, user);
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


}

// ---------------------------------------------------------------- schema.toml

/// 读 `schema.toml`：一行一个方案，键是方案 id、值表示是否勾选。
///
/// 用户目录那份优先（`%APPDATA%\Rime-Next\schema\schema.toml`），没有就用
/// 程序文件夹的出厂那份。返回**按 a–z 排序**的已选 id；解析失败当作"没有已选"。
pub fn read_schema_toml(shared: &Path, user: &Path) -> Vec<String> {
    let candidates = [user.join("schema").join("schema.toml"), shared.join("schema").join("schema.toml")];
    for path in candidates {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(table) = toml::from_str::<std::collections::BTreeMap<String, bool>>(&text) else {
            continue;
        };
        let mut ids: Vec<String> = table
            .into_iter()
            .filter(|(_, selected)| *selected)
            .map(|(id, _)| id)
            .collect();
        ids.sort();
        return ids;
    }
    Vec::new()
}

/// 写 `schema.toml`：一行一个方案（键 = 方案 id、值 = `true`），**一律按 a–z 排序**。
///
/// 只写已选的（未选的不出现，与"未列出的视为未勾选"一致）。程序文件夹只读，
/// 所以调用方传进来的应当是用户目录那份路径。
pub fn write_schema_toml(path: &Path, ids: &[String]) -> io::Result<()> {
    let mut sorted: Vec<&String> = ids.iter().collect();
    sorted.sort();
    sorted.dedup();
    let mut out = String::from(
        "# 输入方案清单 —— 一行一个方案：键是方案 id，值表示是否勾选。按 a–z 排序。\n\
         # 由「方案选单设定」按「中」后写出；不再写入任何 YAML。\n\n",
    );
    for id in sorted {
        out.push_str(id);
        out.push_str(" = true\n");
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, out)
}

#[cfg(test)]
mod schema_toml_tests {
    use super::*;

    #[test]
    fn writes_one_entry_per_line_sorted() {
        let dir = std::env::temp_dir().join("weasel-next-schema-toml-write");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("schema").join("schema.toml");
        let ids = vec![
            "terra_pinyin".to_string(),
            "bopomofo".to_string(),
            "luna_pinyin".to_string(),
        ];
        write_schema_toml(&path, &ids).expect("写入应当成功");
        let text = fs::read_to_string(&path).expect("应当读回");
        let body: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim_start().starts_with('#') && !line.trim().is_empty())
            .collect();
        assert_eq!(
            body,
            vec!["bopomofo = true", "luna_pinyin = true", "terra_pinyin = true"],
            "一行一个方案且按 a–z 排序"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_user_file_first_and_only_selected() {
        let shared = std::env::temp_dir().join("weasel-next-schema-toml-read-shared");
        let user = std::env::temp_dir().join("weasel-next-schema-toml-read-user");
        let _ = fs::remove_dir_all(&shared);
        let _ = fs::remove_dir_all(&user);
        fs::create_dir_all(shared.join("schema")).unwrap();
        fs::write(
            shared.join("schema").join("schema.toml"),
            "bopomofo = true\nluna_pinyin = true\n",
        )
        .unwrap();
        // 用户那份存在时以它为准，且 false 的不算已选
        fs::create_dir_all(user.join("schema")).unwrap();
        fs::write(
            user.join("schema").join("schema.toml"),
            "# 注释行应当被忽略\nterra_pinyin = true\nstroke = false\ncangjie5 = true\n",
        )
        .unwrap();
        assert_eq!(
            read_schema_toml(&shared, &user),
            vec!["cangjie5".to_string(), "terra_pinyin".to_string()],
            "用户那份优先、只取 true、按 a–z 排序"
        );
        // 删掉用户那份，应当回退到出厂那份
        let _ = fs::remove_dir_all(&user);
        assert_eq!(
            read_schema_toml(&shared, &user),
            vec!["bopomofo".to_string(), "luna_pinyin".to_string()],
            "用户那份不存在时回退出厂那份"
        );
        let _ = fs::remove_dir_all(&shared);
    }
}
