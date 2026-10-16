//! 极简 YAML 子集：解析 / 序列化，供仓库里"我们自己的配置"使用。
//!
//! 为什么不用第三方库：本工作区没有 YAML crate（Cargo.lock 里没有），而 rime 那一侧
//! 本来就用 YAML，我们只需要能读写**自己写的那几种结构**：缩进映射、块/流式列表、
//! 标量（字符串/布尔/整数/浮点）、行尾注释。
//!
//! 与 `serde_json::Value` 互转是关键：配置在代码里的内部表示一直是 JSON Value
//! （`.get("a.b")` 那套路径查询），换了文件格式也不用动消费方。
//!
//! 支持的语法：
//!
//! ```yaml
//! # 注释
//! key: 值                # 标量
//! nested:
//!   child: 1             # 缩进映射
//! list:                  # 块式列表
//!   - a
//!   - b
//! flow: [a, "b", 3]      # 流式列表
//! quoted: "含 # 与 : 的串"
//! ```
//!
//! 不支持（也用不到）：多行标量、锚点/别名、多文档、复杂键。

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;

/// 解析出来的 YAML 子集。
#[derive(Debug, Clone, PartialEq)]
pub enum Yaml {
    Scalar(Scalar),
    Seq(Vec<Yaml>),
    Map(BTreeMap<String, Yaml>),
}

impl Yaml {
    pub fn as_map(&self) -> Option<&BTreeMap<String, Yaml>> {
        match self {
            Yaml::Map(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_seq(&self) -> Option<&[Yaml]> {
        match self {
            Yaml::Seq(items) => Some(items),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Yaml> {
        self.as_map().and_then(|map| map.get(key))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Yaml::Scalar(scalar) => Some(scalar.text.as_str()),
            _ => None,
        }
    }
}

/// 解析错误（带行号）。
#[derive(Debug, Clone, PartialEq)]
pub struct YamlError {
    pub line: usize,
    pub message: String,
}

impl YamlError {
    pub fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for YamlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "第 {} 行：{}", self.line, self.message)
    }
}

/// 去掉行尾注释：`#` 之前必须是行首或空白，且不在引号内。
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(mark) => {
                if byte == mark {
                    quote = None;
                }
            }
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'#' if index == 0 || bytes[index - 1].is_ascii_whitespace() => {
                    return &line[..index];
                }
                _ => {}
            },
        }
        index += 1;
    }
    line
}

/// 标量：文本 + 是否**带引号**。
///
/// 引号信息必须保留到类型推断那一步：`"0.23"` 是字符串、`0.23` 是数字；
/// 早先去掉引号后才推断，导致 `config_version: "0.23"` 变成浮点数 ✗。
#[derive(Debug, Clone, PartialEq)]
pub struct Scalar {
    pub text: String,
    pub quoted: bool,
}

/// 解析标量：去掉包裹的引号（`""` 表示空串），并记住它原本有没有引号。
fn parse_scalar(value: &str) -> Scalar {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.as_bytes()[0];
        let last = trimmed.as_bytes()[trimmed.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return Scalar {
                text: trimmed[1..trimmed.len() - 1].to_string(),
                quoted: true,
            };
        }
    }
    Scalar {
        text: trimmed.to_string(),
        quoted: false,
    }
}

/// 只要文本（键名等场景）。
fn unquote(value: &str) -> String {
    parse_scalar(value).text
}

/// 把一个标量文本变成节点：流式列表 `[a, b]` → Seq，其余 → Scalar。
fn parse_scalar_or_flow(value: &str) -> Yaml {
    let trimmed = value.trim();
    // 空流式集合：`{}` 空映射、`[]` 空列表
    if trimmed == "{}" {
        return Yaml::Map(BTreeMap::new());
    }
    if trimmed == "[]" {
        return Yaml::Seq(Vec::new());
    }
    if trimmed.len() >= 2 && trimmed.starts_with('[') && trimmed.ends_with(']') {
        let body = &trimmed[1..trimmed.len() - 1];
        let items = split_flow(body)
            .into_iter()
            .map(|item| Yaml::Scalar(parse_scalar(&item)))
            .collect();
        return Yaml::Seq(items);
    }
    Yaml::Scalar(parse_scalar(trimmed))
}

/// 按逗号切分流式列表（忽略引号里的逗号）。
fn split_flow(body: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for character in body.chars() {
        match quote {
            Some(mark) => {
                current.push(character);
                if character == mark {
                    quote = None;
                }
            }
            None => match character {
                '"' | '\'' => {
                    quote = Some(character);
                    current.push(character);
                }
                ',' => {
                    items.push(current.trim().to_string());
                    current.clear();
                }
                _ => current.push(character),
            },
        }
    }
    if !current.trim().is_empty() {
        items.push(current.trim().to_string());
    }
    items.retain(|item| !item.is_empty());
    items
}

/// 一层缩进：`indent` 是**其子项**应当位于的缩进，`key` 是这一层在父层里的键名。
struct Level {
    indent: usize,
    key: String,
    map: BTreeMap<String, Yaml>,
    /// 该层是列表（`key:` 下面全是 `- ` 项）时，按顺序收集。
    seq: Option<Vec<Yaml>>,
}

/// 解析极简 YAML，返回顶层节点。
pub fn parse_yaml(text: &str) -> Result<Yaml, YamlError> {
    let mut stack: Vec<Level> = vec![Level {
        indent: 0,
        key: String::new(),
        map: BTreeMap::new(),
        seq: None,
    }];
    for (number, raw) in text.lines().enumerate() {
        let line_number = number + 1;
        let line = strip_comment(raw).trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with("---") {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let content = line.trim_start();

        // 先收掉缩进已结束的层
        while stack.len() > 1 && indent < stack[stack.len() - 1].indent {
            close_level(&mut stack);
        }

        // 列表项：`- 值` 或 `-`（子结构）
        if let Some(rest) = content.strip_prefix("- ") {
            let item = parse_scalar_or_flow(rest);
            push_seq_item(&mut stack, item);
            continue;
        }
        if content == "-" {
            push_seq_item(&mut stack, Yaml::Map(BTreeMap::new()));
            continue;
        }

        let Some(colon) = content.find(':') else {
            return Err(YamlError::new(line_number, "缺少 ':'"));
        };
        let key = unquote(&content[..colon]);
        let value = content[colon + 1..].trim();

        if value.is_empty() {
            // 子结构：压入一层，缩进要求比本行更深
            stack.push(Level {
                indent: indent + 1,
                key,
                map: BTreeMap::new(),
                seq: Some(Vec::new()),
            });
        } else {
            let node = parse_scalar_or_flow(value);
            stack
                .last_mut()
                .expect("current level exists")
                .map
                .insert(key, node);
        }
    }
    while stack.len() > 1 {
        close_level(&mut stack);
    }
    let root = stack.pop().expect("root exists");
    // 顶层有键 → 映射；只有列表项 → 列表；**空文档 → 空映射**（空配置应当是空对象，
    // 不能变成空数组，否则上层"配置必须是对象"的校验会失败）。
    if !root.map.is_empty() {
        return Ok(Yaml::Map(root.map));
    }
    match root.seq {
        Some(items) if !items.is_empty() => Ok(Yaml::Seq(items)),
        _ => Ok(Yaml::Map(std::collections::BTreeMap::new())),
    }
}

/// 给当前层追加一个列表项（若当前层不是列表，则把它转成列表语义）。
fn push_seq_item(stack: &mut [Level], item: Yaml) {
    if let Some(level) = stack.last_mut() {
        level.seq.get_or_insert_with(Vec::new).push(item);
    }
}

/// 弹出一层并合回父层：有列表项就取列表，否则取映射。
fn close_level(stack: &mut Vec<Level>) {
    let done = stack.pop().expect("stack len > 1");
    let node = match done.seq {
        Some(items) if !items.is_empty() => Yaml::Seq(items),
        _ => Yaml::Map(done.map),
    };
    stack
        .last_mut()
        .expect("parent exists")
        .map
        .insert(done.key, node);
}

// ---------------------------------------------------------------- 与 JSON 互转

/// 标量 → JSON：**带引号的一律是字符串**；不带引号才按内容推断类型。
fn scalar_to_json(scalar: &Scalar) -> serde_json::Value {
    if scalar.quoted {
        return serde_json::Value::String(scalar.text.clone());
    }
    let text = scalar.text.as_str();
    match text {
        "true" | "True" | "yes" | "on" => return serde_json::Value::Bool(true),
        "false" | "False" | "no" | "off" => return serde_json::Value::Bool(false),
        "null" | "~" => return serde_json::Value::Null,
        _ => {}
    }
    if let Ok(number) = text.parse::<i64>() {
        return serde_json::Value::Number(number.into());
    }
    if let Ok(number) = text.parse::<f64>() {
        if let Some(number) = serde_json::Number::from_f64(number) {
            return serde_json::Value::Number(number);
        }
    }
    serde_json::Value::String(text.to_string())
}

/// YAML 节点 → `serde_json::Value`（配置的内部表示）。
pub fn to_json(node: &Yaml) -> serde_json::Value {
    match node {
        Yaml::Scalar(scalar) => scalar_to_json(scalar),
        Yaml::Seq(items) => serde_json::Value::Array(items.iter().map(to_json).collect()),
        Yaml::Map(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), to_json(value)))
                .collect(),
        ),
    }
}

/// `serde_json::Value` → YAML 文本（我们写出去的那几种文件用）。
pub fn from_json(value: &serde_json::Value, indent: usize, out: &mut String) {
    use serde_json::Value;
    let pad = " ".repeat(indent);
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                match child {
                    Value::Object(_) if !child.as_object().is_some_and(|m| m.is_empty()) => {
                        out.push_str(&format!("{pad}{key}:\n"));
                        from_json(child, indent + 2, out);
                    }
                    Value::Array(items) if !items.is_empty() => {
                        out.push_str(&format!("{pad}{key}:\n"));
                        for item in items {
                            out.push_str(&format!("{pad}  - {}\n", inline_scalar(item)));
                        }
                    }
                    _ => out.push_str(&format!("{pad}{key}: {}\n", inline_scalar(child))),
                }
            }
        }
        _ => out.push_str(&format!("{pad}{}\n", inline_scalar(value))),
    }
}

/// 标量/空容器的行内写法。
fn inline_scalar(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::Null => "~".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => {
            if text.is_empty() {
                "\"\"".to_string()
            } else if text.contains([':', '#', '"', '\'', '\n'])
                || text.starts_with([' ', '-', '[', '{', '*', '&', '!'])
            {
                format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                text.clone()
            }
        }
        Value::Array(_) => "[]".to_string(),
        Value::Object(_) => "{}".to_string(),
    }
}

/// 解析文本为 `serde_json::Value`。
pub fn to_value(text: &str) -> Result<serde_json::Value, String> {
    parse_yaml(text)
        .map(|node| to_json(&node))
        .map_err(|error| error.to_string())
}

/// 解析文本为任意可反序列化类型。
pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let value = to_value(text)?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

/// 把 `serde_json::Value` 写成 YAML 文本（顶层为映射）。
pub fn to_string_pretty(value: &serde_json::Value) -> String {
    let mut out = String::new();
    from_json(value, 0, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_maps_lists_and_scalars() {
        let node = parse_yaml(
            "# 注释\n\
             theme: official\n\
             enabled: true\n\
             count: 3\n\
             nested:\n  \
               child: \"含 # 的串\"\n\
             schemas:\n  \
               - luna_pinyin\n  \
               - stroke\n\
             flow: [a, \"b\", 3]\n",
        )
        .unwrap();
        let json = to_json(&node);
        assert_eq!(json["theme"], "official");
        assert_eq!(json["enabled"], true);
        assert_eq!(json["count"], 3);
        assert_eq!(json["nested"]["child"], "含 # 的串");
        assert_eq!(json["schemas"], json!(["luna_pinyin", "stroke"]));
        assert_eq!(json["flow"], json!(["a", "b", 3]));
    }

    #[test]
    fn round_trips_through_text() {
        let value = json!({
            "theme": "official",
            "schemas": ["luna_pinyin", "stroke"],
            "sound": {"enabled": true, "key": "sound/k.wav"},
        });
        let text = to_string_pretty(&value);
        let back = to_value(&text).unwrap();
        assert_eq!(back["theme"], "official");
        assert_eq!(back["schemas"], json!(["luna_pinyin", "stroke"]));
        assert_eq!(back["sound"]["enabled"], true);
    }

    #[test]
    fn typed_deserialization_works() {
        #[derive(serde::Deserialize, PartialEq, Debug)]
        struct Table {
            #[serde(default)]
            schemas: Vec<String>,
        }
        let table: Table = from_str("schemas:\n  - a\n  - b\n").unwrap();
        assert_eq!(table, Table { schemas: vec!["a".into(), "b".into()] });
    }

    #[test]
    fn quoted_scalars_stay_strings() {
        // 引号就是"这是字符串"的显式声明，不做类型推断
        let json = to_value("config_version: \"0.23\"\nflag: \"true\"\nnum: \"3\"\n").unwrap();
        assert_eq!(json["config_version"], "0.23");
        assert_eq!(json["flag"], "true");
        assert_eq!(json["num"], "3");
        // 不加引号才推断
        let plain = to_value("num: 3\nflag: true\nratio: 0.5\n").unwrap();
        assert_eq!(plain["num"], 3);
        assert_eq!(plain["flag"], true);
        assert_eq!(plain["ratio"], 0.5);
    }

    #[test]
    fn empty_flow_collections_are_supported() {
        let json = to_value("settings:\n  abc: {}\n  empty: []\n").unwrap();
        assert_eq!(json["settings"]["abc"], serde_json::json!({}));
        assert_eq!(json["settings"]["empty"], serde_json::json!([]));
    }

    #[test]
    fn empty_document_is_an_empty_mapping() {
        assert_eq!(to_value("").unwrap(), serde_json::json!({}));
        assert_eq!(to_value("# 只有注释\n").unwrap(), serde_json::json!({}));
        // 顶层列表仍然认得出来
        assert_eq!(to_value("- a\n- b\n").unwrap(), serde_json::json!(["a", "b"]));
    }

    #[test]
    fn quoted_scalars_keep_special_characters() {
        let json = to_value("punctuator: \"，。\"\n").unwrap();
        assert_eq!(json["punctuator"], "，。");
    }
}
