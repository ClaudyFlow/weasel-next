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
use std::path::Path;

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
    /// 所属系列（父目录名，例如 `yuhao`）；平铺布局的方案为 None。
    pub series: Option<String>,
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
    let mut available: BTreeMap<String, (String, String, String, Option<String>)> = BTreeMap::new();
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
                let Some(name_of_dir) = dir.file_name().map(|n| n.to_string_lossy().to_string()) else {
                    continue;
                };
                // ① 平铺布局：schema\<方案>\<方案>.schema.yaml
                let flat = dir.join(format!("{name_of_dir}.schema.yaml"));
                if let Ok(text) = fs::read_to_string(&flat) {
                    let (name, author, description) = read_schema_fields(&text);
                    available.insert(
                        name_of_dir.clone(),
                        (
                            name.unwrap_or_else(|| name_of_dir.clone()),
                            author.unwrap_or_default(),
                            description.unwrap_or_default(),
                            None,
                        ),
                    );
                    continue;
                }
                // ② 系列布局（2026-10-28 起「目录 = id 全部分段」）：
                //    schema\<系列>\<叶>\<系列>.<叶>.schema.yaml
                //    例：id `double_pinyin.flypy` → schema\double_pinyin\flypy\double_pinyin.flypy.schema.yaml
                //    旧布局仍兼容：schema\<系列>\<方案>\<方案>.schema.yaml（那时 id 不带系列前缀）。
                //    ⚠️ 2026-10-29 修的 bug：原来只找 `<叶>.schema.yaml`，于是新布局一个都读不到，
                //    设置里的方案列表看不到 double_pinyin.* / pinyin.* / cangjie.* / wubi.*。
                let series = name_of_dir.clone();
                let Ok(children) = fs::read_dir(&dir) else {
                    continue;
                };
                for child in children {
                    let scheme_dir = child?.path();
                    if !scheme_dir.is_dir() {
                        continue;
                    }
                    let Some(leaf) = scheme_dir.file_name().map(|n| n.to_string_lossy().to_string()) else {
                        continue;
                    };
                    let dotted = format!("{series}.{leaf}");
                    // 先新布局（id 带系列前缀），再旧布局（id = 叶名）。
                    let mut found = None;
                    for (id, stem) in [(&dotted, &dotted), (&leaf, &leaf)] {
                        let file = scheme_dir.join(format!("{stem}.schema.yaml"));
                        if let Ok(text) = fs::read_to_string(&file) {
                            found = Some((id.clone(), text));
                            break;
                        }
                    }
                    let Some((id, text)) = found else {
                        continue;
                    };
                    let (scheme_name, author, description) = read_schema_fields(&text);
                    available.insert(
                        id.clone(),
                        (
                            scheme_name.unwrap_or_else(|| id.clone()),
                            author.unwrap_or_default(),
                            description.unwrap_or_default(),
                            Some(series.clone()),
                        ),
                    );
                }
            }
        }

    // 已选清单：读 schema.yaml（用户目录那份优先，回退出厂那份）。不再读写任何 TOML。
    let selected = read_schema_ids(shared, user);
    let mut rows = Vec::with_capacity(available.len());
    for id in &selected {
        if let Some((name, author, description, series)) = available.remove(id) {
            rows.push(SchemaInfo {
                id: id.clone(),
                name,
                author,
                description,
                selected: true,
                series,
            });
        }
    }
    for (id, (name, author, description, series)) in available {
        rows.push(SchemaInfo {
            id,
            name,
            author,
            description,
            selected: false,
            series,
        });
    }
    Ok(rows)
}



#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    /// 2026-10-29 回归：系列布局（「目录 = id 全部分段」）必须被列出，
    /// 且 id 是 `系列.叶`（`double_pinyin.flypy`），不是叶名 `flypy`。
    #[test]
    fn lists_series_layout_with_dotted_ids() {
        let root = std::env::temp_dir().join(format!("weasel-switch-series-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let shared = root.join("shared");
        let flypy = shared.join("schema").join("double_pinyin").join("flypy");
        fs::create_dir_all(&flypy).unwrap();
        fs::write(
            flypy.join("double_pinyin.flypy.schema.yaml"),
            "schema:\n  name: 小鹤双拼\n",
        )
        .unwrap();
        let stroke = shared.join("schema").join("stroke");
        fs::create_dir_all(&stroke).unwrap();
        fs::write(stroke.join("stroke.schema.yaml"), "schema:\n  name: 五笔画\n").unwrap();
        let user = root.join("user");
        fs::create_dir_all(user.join("schema")).unwrap();
        fs::write(
            user.join("schema").join("schema.yaml"),
            "active: double_pinyin.flypy\nschemas:\n  - double_pinyin.flypy\n  - stroke\n",
        )
        .unwrap();

        let rows = list_schemas(&shared, &user).unwrap();
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert!(
            ids.contains(&"double_pinyin.flypy"),
            "系列布局应生成 `系列.叶` 的 id，实际 {ids:?}"
        );
        assert!(ids.contains(&"stroke"), "平铺布局仍要列出来，实际 {ids:?}");
        let fly = rows.iter().find(|row| row.id == "double_pinyin.flypy").unwrap();
        assert_eq!(fly.name, "小鹤双拼");
        assert_eq!(fly.series.as_deref(), Some("double_pinyin"));
        assert!(fly.selected, "清单里勾过的方案应标成已选");
        let _ = fs::remove_dir_all(&root);
    }

}

// ---------------------------------------------------------------- schema.yaml

/// 方案清单文件名（用户目录 `schema/` 下），与引擎侧同名同义。
///
/// 引擎侧常量是 `librime-next` 的 `lever::paths::SCHEMA_MANIFEST = "schema.yaml"`；
/// 这里必须与它一致，否则"宿主勾方案、引擎读清单"会各读各的（2026-10-27 前的
/// 双轨 bug：宿主读 `schema.toml`、引擎读 `schema.yaml`，勾了方案不生效）。
///
/// 2026-10-27：**TOML 彻底移除**。`schema.toml` 不再被任何 Rust 代码读取；
/// 磁盘上若仍有，只可能是装机版补丁 C++ 引擎在用（由安装器负责产出）。
pub const SCHEMA_TABLE_FILE: &str = "schema.yaml";

/// 旧清单文件名，**只**用于一次性迁移（读到就转写成 yaml，之后不再碰）。
const LEGACY_SCHEMA_TABLE_FILE: &str = "schema.toml";

/// `schema.yaml` 的内容：**勾选了哪些方案** + **当前用的是哪个**。
///
/// 这是"方案信息"的唯一落点（用户目录 `%APPDATA%\Rime-Next\schema\schema.yaml`）：
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

/// 读 `schema.yaml`：用户目录优先，其次程序文件夹的出厂那份。
///
/// 解析失败或文件缺失都返回空表（= 没有偏好），由调用方决定回退行为。
///
/// 两份 yaml 都没有时**一次性**读旧 `schema.toml`：解析出来就地转写成 yaml
/// （旧机器升级不丢勾选），读不动就当没有。
pub fn read_schema_table(shared: &Path, user: &Path) -> SchemaTable {
    let candidates = [
        user.join("schema").join(SCHEMA_TABLE_FILE),
        shared.join("schema").join(SCHEMA_TABLE_FILE),
    ];
    for path in &candidates {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        if let Ok(table) = weasel_common::yaml::from_str::<SchemaTable>(&text) {
            if table.active.is_some() || !table.schemas.is_empty() {
                return table;
            }
        }
    }
    // 一次性迁移：旧 toml 只在用户目录 / 共享目录各找一次，成功就落成 yaml。
    for root in [user, shared] {
        let legacy = root.join("schema").join(LEGACY_SCHEMA_TABLE_FILE);
        let Ok(text) = fs::read_to_string(&legacy) else {
            continue;
        };
        let Some(table) = parse_legacy_toml(&text) else {
            continue;
        };
        let target = user.join("schema").join(SCHEMA_TABLE_FILE);
        let _ = write_schema_table(&target, &table);
        return table;
    }
    SchemaTable::default()
}

/// 手工解析旧 `schema.toml`（**刻意不引 toml crate**：2026-10-27 起全仓库不要 toml）。
///
/// 只认真实出现过的两种写法，够迁移用即可：
/// ```text
/// active = "luna_pinyin"
/// schemas = [ "a", "b", ]   # 数组形式，保序
/// bopomofo = true           # 键值形式，按出现顺序（比 BTreeMap 的 a–z 更保真）
/// ```
fn parse_legacy_toml(text: &str) -> Option<SchemaTable> {
    let unquote = |s: &str| s.trim().trim_matches('"').trim_matches('\'').to_string();
    let mut active: Option<String> = None;
    let mut schemas: Vec<String> = Vec::new();
    let mut in_array = false;
    for raw in text.lines() {
        // 去行注释（引号里的 # 不处理：迁移文本不会有这种边角情况）
        let line = raw.split_once('#').map(|(a, _)| a).unwrap_or(raw).trim();
        if line.is_empty() {
            continue;
        }
        if in_array {
            if line.starts_with(']') {
                in_array = false;
                continue;
            }
            let id = unquote(line.trim_end_matches(','));
            if !id.is_empty() && !schemas.contains(&id) {
                schemas.push(id);
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "active" => {
                let v = unquote(value);
                if !v.is_empty() {
                    active = Some(v);
                }
            }
            "schemas" => {
                let rest = value.trim_start_matches('[');
                if let Some(inner) = rest.strip_suffix(']') {
                    // 单行写完：schemas = ["a", "b"]
                    for part in inner.split(',') {
                        let id = unquote(part);
                        if !id.is_empty() && !schemas.contains(&id) {
                            schemas.push(id);
                        }
                    }
                } else if rest.starts_with('[') || value.starts_with('[') {
                    // 多行数组：后续行由 in_array 分支收
                    in_array = true;
                }
            }
            _ => {
                if value == "true" {
                    let id = unquote(key);
                    if !id.is_empty() && !schemas.contains(&id) {
                        schemas.push(id);
                    }
                }
            }
        }
    }
    if active.is_none() && schemas.is_empty() {
        None
    } else {
        Some(SchemaTable { active, schemas })
    }
}

/// 读勾选清单（用户目录优先）。
pub fn read_schema_ids(shared: &Path, user: &Path) -> Vec<String> {
    read_schema_table(shared, user).selected_ids()
}

/// 把一个方案 id 写成 YAML 标量；**纯数字开头的必须加引号**。
///
/// 2026-11-02 实测（092 码表方案）：`active: 092` 会被 YAML 解析成数字，
/// `SchemaTable` 的 `Option<String>` 反序列化直接失败 ⇒ 整份 `schema.yaml` 被当成坏文件
/// ⇒ `read_schema_table` 回退去读旧 `schema.toml` ⇒ 用户选了 092，重启又跳回原方案。
/// 所以写的时候就得给这类 id 加引号，读的那一侧不用改。
fn yaml_scalar(value: &str) -> String {
    let plain = !value.is_empty()
        && !value.as_bytes()[0].is_ascii_digit()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~"
        );
    if plain {
        value.to_string()
    } else {
        format!(
            "\"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        )
    }
}

/// 写 `schema.yaml`：勾选清单 + 当前方案，一次写完。
pub fn write_schema_table(path: &Path, table: &SchemaTable) -> io::Result<()> {    let mut out = String::from(
        "# 方案信息 —— 勾选清单 + 当前使用的方案（YAML）。\n\
         # 由「方案选单设定」与输入法自己维护；引擎启动时读它并把选择落实到 librime。\n\
         #\n\
         # 2026-10-27 自 schema.toml 迁移而来：Rust 侧已彻底不认 TOML。\n\
         # 装机版补丁 C++ 引擎仍读同目录的 schema.toml，两者内容需保持一致——\n\
         # 正式发布时这两份文件应由安装器统一产出。\n",
    );
    if let Some(active) = &table.active {
        out.push_str(&format!("active: {}\n", yaml_scalar(active)));
    }
    if table.schemas.is_empty() {
        out.push_str("schemas: []\n");
    } else {
        out.push_str("schemas:\n");
        for id in &table.schemas {
            out.push_str(&format!("  - {}\n", yaml_scalar(id)));
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, out)
}

/// 首次运行时把出厂 `default.yaml` 的 `schema_list` 抄进 `schema.yaml`。
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
    let path = user.join("schema").join(SCHEMA_TABLE_FILE);
    write_schema_table(&path, &table)?;
    Ok(true)
}

/// 方案清单**不再**落成 `default.custom.yaml`。
///
/// librime 侧（`deployment_tasks.cc` 的 Weasel-Next 补丁）现在直接读
/// `<用户目录>/schema/schema.yaml` 的 `schemas:`，所以这份中转 YAML
/// 既不需要生成，也不该出现在用户目录里。
///
/// 这条注释保留是为了说明"为什么这里没有生成代码" —— 曾经有过，
/// 用户看到的 `default.custom.yaml` 就是它写的。

/// 只写勾选清单的入口：**保留原有 `active`**（与 `write_schema_table` 的区别）。
pub fn write_schema_ids(path: &Path, ids: &[String]) -> io::Result<()> {
    let mut table = fs::read_to_string(path)
        .ok()
        .and_then(|t| weasel_common::yaml::from_str::<SchemaTable>(&t).ok())
        .unwrap_or_default();
    table.schemas = ids.to_vec();
    write_schema_table(path, &table)
}

#[cfg(test)]
mod schema_table_tests {
    use super::*;

    #[test]
    fn writes_schema_list_in_given_order_as_yaml() {
        let dir = std::env::temp_dir().join("weasel-next-schema-yaml-write");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("schema").join(SCHEMA_TABLE_FILE);
        let ids = vec![
            "terra_pinyin".to_string(),
            "bopomofo".to_string(),
            "luna_pinyin".to_string(),
        ];
        write_schema_ids(&path, &ids).expect("写入应当成功");
        // 顺序必须原样保留：菜单顺序 = 清单顺序，第一条也是 rime 没记忆时的默认方案
        assert_eq!(read_schema_ids(&dir, &dir), ids, "读回来的顺序应与写入一致");
        let text = fs::read_to_string(&path).expect("应当读回");
        assert!(
            text.contains("schemas:\n  - terra_pinyin\n  - bopomofo\n  - luna_pinyin\n"),
            "写出的应当是 YAML 序列，实际：\n{text}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_active_as_yaml_scalar() {
        let dir = std::env::temp_dir().join("weasel-next-schema-yaml-active");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("schema").join(SCHEMA_TABLE_FILE);
        let table = SchemaTable {
            active: Some("double_pinyin_ziranma".to_string()),
            schemas: vec!["luna_pinyin_simp".to_string()],
        };
        write_schema_table(&path, &table).expect("写入应当成功");
        let text = fs::read_to_string(&path).expect("应当读回");
        assert!(
            text.contains("active: double_pinyin_ziranma\n"),
            "active 应是 YAML 标量而不是 `active = \"...\"`，实际：\n{text}"
        );
        // 引擎侧 `load_schema_manifest` 吃的就是这两个键
        assert_eq!(
            read_schema_table(&dir, &dir),
            table,
            "往返解析应完全相等"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrates_legacy_toml_into_yaml() {
        let dir = std::env::temp_dir().join("weasel-next-schema-migrate");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("schema")).unwrap();
        fs::write(
            dir.join("schema").join(LEGACY_SCHEMA_TABLE_FILE),
            "# 注释行应当被忽略\nactive = \"terra_pinyin\"\nschemas = [\n    \"terra_pinyin\",\n    \"bopomofo\",\n]\n",
        )
        .unwrap();
        // 只有旧 toml：应当迁移出 yaml，且顺序保真
        assert_eq!(
            read_schema_ids(&dir, &dir),
            vec!["terra_pinyin".to_string(), "bopomofo".to_string()],
            "旧 toml 应被迁移并按原顺序读出"
        );
        assert!(
            dir.join("schema").join(SCHEMA_TABLE_FILE).is_file(),
            "迁移后应落成 schema.yaml"
        );
        assert_eq!(
            read_schema_table(&dir, &dir).active.as_deref(),
            Some("terra_pinyin"),
            "active 也应一并迁移"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_key_value_toml_keeps_file_order() {
        let table = parse_legacy_toml("bopomofo = true\nluna_pinyin = true\nstroke = false\n")
            .expect("键值形式应能解析");
        assert_eq!(
            table.schemas,
            vec!["bopomofo".to_string(), "luna_pinyin".to_string()],
            "按文件出现顺序，只取 true"
        );
    }

    #[test]
    fn numeric_schema_id_survives_roundtrip() {
        // 092 是真实方案 id（码表）。不写引号时 YAML 会把它当数字，
        // 反序列化 `Option<String>` 失败 ⇒ 整份 schema.yaml 作废、回退旧 toml。
        let dir = std::env::temp_dir().join("weasel-next-schema-numeric-id");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("schema")).unwrap();
        let path = dir.join("schema").join(SCHEMA_TABLE_FILE);
        let table = SchemaTable {
            active: Some("092".to_string()),
            schemas: vec!["pinyin.luna".to_string(), "092".to_string()],
        };
        write_schema_table(&path, &table).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("active: \"092\""),
            "纯数字 id 必须加引号，否则读不回来：{text}"
        );
        let back = read_schema_table(&dir, &dir);
        assert_eq!(back.active.as_deref(), Some("092"), "写出去再读回来要一致");
        assert_eq!(
            back.schemas,
            vec!["pinyin.luna".to_string(), "092".to_string()]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn yaml_wins_over_legacy_toml() {
        let shared = std::env::temp_dir().join("weasel-next-schema-yaml-shared");
        let user = std::env::temp_dir().join("weasel-next-schema-yaml-user");
        let _ = fs::remove_dir_all(&shared);
        let _ = fs::remove_dir_all(&user);
        fs::create_dir_all(user.join("schema")).unwrap();
        fs::write(
            user.join("schema").join(SCHEMA_TABLE_FILE),
            "active: bopomofo\nschemas:\n  - bopomofo\n",
        )
        .unwrap();
        // 旧 toml 同时存在：yaml 在场时它完全不参与
        fs::write(
            user.join("schema").join(LEGACY_SCHEMA_TABLE_FILE),
            "active = \"luna_pinyin\"\nschemas = [\"luna_pinyin\"]\n",
        )
        .unwrap();
        assert_eq!(read_schema_ids(&shared, &user), vec!["bopomofo".to_string()]);
        let _ = fs::remove_dir_all(&shared);
        let _ = fs::remove_dir_all(&user);
    }
}
