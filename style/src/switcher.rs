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
//! * **写回** = 用户目录 `schema/schema.toml`（勾选清单 + 当前方案）；引擎读它再落到 librime，
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

/// `schema.toml` 的内容：**勾选了哪些方案** + **当前用的是哪个**。
///
/// 这是"方案信息"的唯一落点（用户目录 `%APPDATA%\Rime-Next\schema\schema.toml`）：
/// 选项清单给引擎拿去写 rime 的 `default.custom.yaml`，`active` 给引擎在新会话上
/// `select_schema` —— 两者都从这一份文件读，不再散落在 YAML 里。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SchemaTable {
    /// 当前使用的方案 id；空表示"还没定过"，交给 rime 自己的记忆。
    pub active: Option<String>,
    /// 勾选清单，**按方案菜单顺序**排列（第一条是 rime 没有记忆时的默认方案）。
    ///
    /// 顺序必须保真：早先版本用 map 存，键会自动按 a–z 排序，`bopomofo` 跑到第一个，
    /// 一旦 rime 丢了"上次用的方案"就会拿注音当默认，打字出怪字。
    pub schemas: Vec<String>,
}

impl SchemaTable {
    /// 已勾选的方案 id（保持清单顺序）。
    pub fn selected_ids(&self) -> Vec<String> {
        self.schemas.clone()
    }
    /// 记下当前方案。
    pub fn set_active(&mut self, id: impl Into<String>) {
        let id = id.into();
        self.active = if id.is_empty() { None } else { Some(id) };
    }
}

/// 读 `schema.toml`：用户目录优先，其次程序文件夹的出厂那份。
///
/// 解析失败或文件缺失都返回空表（= 没有偏好），由调用方决定回退行为。
pub fn read_schema_table(shared: &Path, user: &Path) -> SchemaTable {
    let candidates = [
        user.join("schema").join("schema.toml"),
        shared.join("schema").join("schema.toml"),
    ];
    for path in candidates {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        // 新格式（数组）优先
        if let Ok(table) = toml::from_str::<SchemaTable>(&text) {
            if table.active.is_some() || !table.schemas.is_empty() {
                return table;
            }
        }
        // 兼容早期两种写法：`[schemas]` 表或纯键值表（都丢了顺序 →
        // 用出厂 default.yaml 的顺序把勾选过的挑出来，保证默认方案不出错）
        let flat: Option<std::collections::BTreeMap<String, bool>> = {
            #[derive(serde::Deserialize, Default)]
            #[serde(default)]
            struct LegacyTable {
                active: Option<String>,
                schemas: std::collections::BTreeMap<String, bool>,
            }
            match toml::from_str::<LegacyTable>(&text) {
                Ok(legacy) if !legacy.schemas.is_empty() => {
                    let mut table = SchemaTable {
                        active: legacy.active,
                        schemas: Vec::new(),
                    };
                    let factory = read_schema_list(
                        &fs::read_to_string(shared.join("default.yaml")).unwrap_or_default(),
                    );
                    for id in factory {
                        if legacy.schemas.get(&id).copied().unwrap_or(false) {
                            table.schemas.push(id);
                        }
                    }
                    for (id, selected) in &legacy.schemas {
                        if *selected && !table.schemas.contains(id) {
                            table.schemas.push(id.clone());
                        }
                    }
                    if !table.schemas.is_empty() {
                        return table;
                    }
                    None
                }
                _ => toml::from_str::<std::collections::BTreeMap<String, bool>>(&text).ok(),
            }
        };
        if let Some(flat) = flat {
            let factory = read_schema_list(
                &fs::read_to_string(shared.join("default.yaml")).unwrap_or_default(),
            );
            let mut table = SchemaTable::default();
            for id in factory {
                if flat.get(&id).copied().unwrap_or(false) {
                    table.schemas.push(id);
                }
            }
            for (id, selected) in &flat {
                if *selected && !table.schemas.contains(id) {
                    table.schemas.push(id.clone());
                }
            }
            if !table.schemas.is_empty() {
                return table;
            }
        }
    }
    SchemaTable::default()
}

/// 读勾选清单（兼容旧调用点）。
pub fn read_schema_toml(shared: &Path, user: &Path) -> Vec<String> {
    read_schema_table(shared, user).selected_ids()
}

/// 写 `schema.toml`：勾选清单 + 当前方案，一次写完。
pub fn write_schema_table(path: &Path, table: &SchemaTable) -> io::Result<()> {
    let mut out = String::from(
        "# 方案信息 —— 勾选清单 + 当前使用的方案（YAML）。\n\
         # 由「方案选单设定」与输入法自己维护；引擎启动时读它并把选择落实到 librime。\n",
    );
    if let Some(active) = &table.active {
        out.push_str(&format!("\nactive = {:?}\n", active));
    }
    out.push_str("\nschemas = [\n");
    for id in &table.schemas {
        out.push_str(&format!("    {id:?},\n"));
    }
    out.push_str("]\n");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, out)
}

/// 首次运行时把出厂 `default.yaml` 的 `schema_list` 抄进 `schema.toml`。
///
/// 这样"方案信息"从一开始就只有一处（用户目录那份），而不是"用户没定过就散在 rime 的
/// 出厂配置里"。清单非空时什么都不做（用户或设置程序已经定过），返回是否写入。
pub fn seed_schema_table(shared: &Path, user: &Path) -> std::io::Result<bool> {
    let existing = read_schema_table(shared, user);
    if existing.active.is_some() || !existing.schemas.is_empty() {
        return Ok(false);
    }
    let Ok(text) = fs::read_to_string(shared.join("default.yaml")) else {
        return Ok(false);
    };
    let ids = read_schema_list(&text);
    if ids.is_empty() {
        return Ok(false);
    }
    let table = SchemaTable {
        active: None,
        schemas: ids,
    };
    let path = user.join("schema").join("schema.toml");
    write_schema_table(&path, &table)?;
    Ok(true)
}

/// 方案清单**不再**落成 `default.custom.yaml`。
///
/// librime 侧（`deployment_tasks.cc` 的 Weasel-Next 补丁）现在直接读
/// `<用户目录>/schema/schema.toml` 的 `schemas = [...]`，所以这份中转 YAML
/// 既不需要生成，也不该出现在用户目录里。
///
/// 这条注释保留是为了说明"为什么这里没有生成代码" —— 曾经有过，
/// 用户看到的 `default.custom.yaml` 就是它写的。

/// 写 `schema.toml` 的兼容入口：只给勾选清单，保留原有 `active`。
pub fn write_schema_toml(path: &Path, ids: &[String]) -> io::Result<()> {
    let mut table = match fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str::<SchemaTable>(&t).ok())
    {
        Some(table) => table,
        None => SchemaTable::default(),
    };
    table.schemas = ids.to_vec();
    write_schema_table(path, &table)
}

#[cfg(test)]
mod schema_toml_tests {
    use super::*;

    #[test]
    fn writes_schema_list_in_given_order() {
        let dir = std::env::temp_dir().join("weasel-next-schema-toml-write");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("schema").join("schema.toml");
        let ids = vec![
            "terra_pinyin".to_string(),
            "bopomofo".to_string(),
            "luna_pinyin".to_string(),
        ];
        write_schema_toml(&path, &ids).expect("写入应当成功");
        // 顺序必须原样保留：菜单顺序 = 清单顺序，第一条也是 rime 没记忆时的默认方案
        assert_eq!(
            read_schema_toml(&dir, &dir),
            ids,
            "读回来的顺序应与写入一致"
        );
        let text = fs::read_to_string(&path).expect("应当读回");
        let entries: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with('"'))
            .collect();
        assert_eq!(
            entries,
            vec!["\"terra_pinyin\",", "\"bopomofo\",", "\"luna_pinyin\","],
            "一行一个方案，且不打乱顺序"
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
