//! 持久化：与 Lua 版统计文件**同一语法**（`input_stat = { ... }` 的 Lua 表字面量）。
//!
//! 规范依据 `docs/lua-parity-spec.md` §4：
//! * 路径 `<user_data_dir>/lua/input_stat_<schema_id>.lua`（[`file_name`]）；
//! * 语法 = `input_stat = ` + 匿名表字面量 + 结尾换行，制表符缩进、`["键"]`/`[数字]` 键、
//!   `,` 结尾；键顺序由 Lua 的 `pairs` 决定，**不要求逐字节复现**；
//! * 解析器读回时兼容 Lua 版输出（任意键顺序、`--` 注释、`;` 分隔符、`return` 前缀）；
//! * 读回后由 [`crate::state::InputStats::migrate_format`] 处理 `logVer` 版本迁移。
//!
//! 与 Lua 版的有意偏离（登记于规范附录 B）：本模块对字符串做 `\\`/`"`/换行转义。Lua 版
//! `table.serialize` 不转义，一旦上屏文本含 `"` 或 `\` 其文件就无法再被 `loadfile` 读回；
//! 转义后本模块写出的文件既能被自己读回，也能被 Lua 正确读回。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::state::{Bucket, Fastest, InputStats, NewWords, DEFAULT_SKIN_INDEX};

/// 统计文件名前缀（Lua 版：`input_stat_<schema_id>.lua`）。
pub const STATS_FILE_PREFIX: &str = "input_stat_";
/// 统计文件名后缀。
pub const STATS_FILE_SUFFIX: &str = ".lua";
/// 解析器最多接受的嵌套层数，避免损坏文件造成的深递归。
const MAX_DEPTH: usize = 32;

/// 解析统计文件时的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// 语法错误，`position` 是出错处的字节偏移。
    Syntax { position: usize, message: String },
    /// 嵌套层数超过 [`MAX_DEPTH`]。
    TooDeep,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax { position, message } => {
                write!(f, "统计文件语法错误（偏移 {position}）：{message}")
            }
            Self::TooDeep => f.write_str("统计文件嵌套过深"),
        }
    }
}

impl std::error::Error for ParseError {}

/// 返回某个方案对应的统计文件名（不含目录）。
pub fn file_name(schema_id: &str) -> String {
    format!("{STATS_FILE_PREFIX}{schema_id}{STATS_FILE_SUFFIX}")
}

// ---------------------------------------------------------------- 序列化

/// 写数字：整数值写成整数（Lua 的整数子类型就是这么输出的），其余用最短往返表示。
fn write_number(out: &mut String, value: f64) {
    if value.fract() == 0.0 && value.is_finite() && value.abs() < 9.0e15 {
        let _ = write!(out, "{}", value as i64);
    } else {
        let _ = write!(out, "{value}");
    }
}

/// 把字符串写成 Lua 字符串字面量（转义 `\`、`"`、换行、回车、制表符）。
fn lua_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// 按 `table.serialize` 的风格写出一个表体。
///
/// 与 Lua 版的有意偏离（附录 B 登记）：Lua 每层键都只缩进一个制表符，这里按嵌套深度缩进，
/// 更易读；键顺序也不同（Lua 用 `pairs` 哈希序）。两者都不影响可读回性。
fn write_map(out: &mut String, prefix: &str, entries: &[(String, String)]) {
    let _ = write!(out, "{{\n");
    for (key, value) in entries {
        let _ = writeln!(out, "\t{prefix}[\"{key}\"] = {value},");
    }
    let _ = write!(out, "}}");
}

/// 写出 `[整数] = 整数` 的计数表。
fn write_count_map(values: &BTreeMap<i64, i64>) -> String {
    let mut text = String::from("{\n");
    for (key, value) in values {
        let _ = writeln!(text, "\t\t\t[{key}] = {value},");
    }
    text.push_str("\t\t}");
    text
}

/// 写出 `[序号] = 数字` 的数值序列（整数值按整数写出）。
fn write_number_seq(values: &[f64]) -> String {
    let mut text = String::from("{\n");
    for (index, value) in values.iter().enumerate() {
        let mut number = String::new();
        write_number(&mut number, *value);
        let _ = writeln!(text, "\t\t\t[{}] = {number},", index + 1);
    }
    text.push_str("\t\t}");
    text
}

/// 写出生字/词集合（`["词"] = 1`）。
fn write_word_set(words: &BTreeSet<String>) -> String {
    let mut text = String::from("{\n");
    for word in words {
        let _ = writeln!(text, "\t\t\t[{}] = 1,", lua_string(word));
    }
    text.push_str("\t\t}");
    text
}

/// 写出极速记录（键名与 Lua 版一致：`spd` / `keyTouchSpd` / `avgCodeLen`）。
fn fastest_literal(fastest: &Fastest) -> String {
    let mut spd = String::new();
    write_number(&mut spd, fastest.spd);
    let mut key_touch = String::new();
    write_number(&mut key_touch, fastest.key_touch_spd);
    let mut avg_code = String::new();
    write_number(&mut avg_code, fastest.avg_code_len);
    format!(
        "{{\n\t\t\t[\"spd\"] = {spd},\n\t\t\t[\"keyTouchSpd\"] = {key_touch},\n\t\t\t[\"avgCodeLen\"] = {avg_code},\n\t\t}}"
    )
}

/// 写出一个周期桶，键名与 Lua 版字段名一一对应。
fn bucket_literal(bucket: &Bucket) -> String {
    let mut out = String::new();
    let entries: Vec<(String, String)> = vec![
        ("count".into(), bucket.count.to_string()),
        ("length".into(), bucket.length.to_string()),
        ("fastest".into(), fastest_literal(&bucket.fastest)),
        ("ts".into(), bucket.ts.to_string()),
        ("lengths".into(), write_count_map(&bucket.lengths)),
        ("codeLengths".into(), write_count_map(&bucket.code_lengths)),
        ("avgGaps".into(), write_number_seq(&bucket.avg_gaps)),
        ("avgCnts".into(), write_number_seq(&bucket.avg_cnts)),
        ("avgCodeLen".into(), write_number_seq(&bucket.avg_code_len)),
        ("keyTouchCnt".into(), bucket.key_touch_cnt.to_string()),
        (
            "totalCodeLenWithoutSpace".into(),
            bucket.total_code_len_without_space.to_string(),
        ),
    ];
    write_map(&mut out, "\t\t", &entries);
    out
}

/// 把统计状态序列化成 Lua 版可读回的文本（含结尾换行）。
pub fn serialize(stats: &InputStats) -> String {
    let new_words = {
        let mut out = String::new();
        let entries: Vec<(String, String)> = vec![
            ("startTime".into(), stats.new_words.start_time.to_string()),
            ("words".into(), write_word_set(&stats.new_words.words)),
        ];
        write_map(&mut out, "\t\t", &entries);
        out
    };
    let entries: Vec<(String, String)> = vec![
        ("daily".into(), bucket_literal(&stats.daily)),
        ("weekly".into(), bucket_literal(&stats.weekly)),
        ("monthly".into(), bucket_literal(&stats.monthly)),
        ("yearly".into(), bucket_literal(&stats.yearly)),
        ("logVer".into(), stats.log_ver.to_string()),
        ("daily_max".into(), stats.daily_max.to_string()),
        ("newWords".into(), new_words),
        ("deviceType".into(), lua_string(&stats.device_type)),
        (
            "progressBarSkinIdx_word".into(),
            stats.skin_idx_word.to_string(),
        ),
        (
            "progressBarSkinIdx_code".into(),
            stats.skin_idx_code.to_string(),
        ),
    ];
    let mut out = String::from("input_stat = ");
    write_map(&mut out, "\t", &entries);
    out.push('\n');
    out
}

// ---------------------------------------------------------------- 解析

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Number(f64),
    Str(String),
    Bool(bool),
    Table(Vec<(Key, Value)>),
}

#[derive(Debug, Clone, PartialEq)]
enum Key {
    Int(i64),
    Str(String),
}

impl Value {
    fn field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Table(entries) => entries.iter().find_map(|(key, value)| match key {
                Key::Str(text) if text == name => Some(value),
                _ => None,
            }),
            _ => None,
        }
    }

    fn int(&self) -> Option<i64> {
        match self {
            Value::Number(number) => Some(*number as i64),
            Value::Bool(flag) => Some(i64::from(*flag)),
            _ => None,
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            Value::Number(number) => Some(*number),
            _ => None,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }

    /// 读成 `[序号] = 值` 形式的有序序列（按下标升序；缺号跳过）。
    fn sequence(&self) -> Vec<&Value> {
        let Value::Table(entries) = self else {
            return Vec::new();
        };
        let mut indexed: Vec<(i64, &Value)> = entries
            .iter()
            .filter_map(|(key, value)| match key {
                Key::Int(index) => Some((*index, value)),
                _ => None,
            })
            .collect();
        indexed.sort_by_key(|(index, _)| *index);
        indexed.into_iter().map(|(_, value)| value).collect()
    }

    /// 读成 `[整数] = 整数` 的计数表。
    fn count_map(&self) -> BTreeMap<i64, i64> {
        let Value::Table(entries) = self else {
            return BTreeMap::new();
        };
        entries
            .iter()
            .filter_map(|(key, value)| {
                let index = match key {
                    Key::Int(index) => *index,
                    Key::Str(text) => text.parse::<i64>().ok()?,
                };
                value.int().map(|count| (index, count))
            })
            .collect()
    }

    /// 读成 `["字符串"] = 任意` 的集合（生字/词表）。
    fn word_set(&self) -> BTreeSet<String> {
        let Value::Table(entries) = self else {
            return BTreeSet::new();
        };
        entries
            .iter()
            .filter_map(|(key, _)| match key {
                Key::Str(text) => Some(text.clone()),
                Key::Int(_) => None,
            })
            .collect()
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            pos: 0,
        }
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError::Syntax {
            position: self.pos,
            message: message.into(),
        })
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// 跳过空白、`--` 行注释与 Lua 的分隔符 `,`/`;`。
    fn skip_trivia(&mut self, skip_separators: bool) {
        loop {
            match self.peek() {
                Some(byte) if byte.is_ascii_whitespace() => self.pos += 1,
                Some(b'-') if self.bytes.get(self.pos + 1) == Some(&b'-') => {
                    while let Some(byte) = self.peek() {
                        self.pos += 1;
                        if byte == b'\n' {
                            break;
                        }
                    }
                }
                Some(b',') | Some(b';') if skip_separators => self.pos += 1,
                _ => return,
            }
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), ParseError> {
        self.skip_trivia(true);
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            self.error(format!("需要 '{}'", byte as char))
        }
    }

    fn parse_ident(&mut self) -> Option<String> {
        let start = self.pos;
        while let Some(byte) = self.peek() {
            if byte.is_ascii_alphanumeric() || byte == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            None
        } else {
            Some(String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned())
        }
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut text = String::new();
        loop {
            match self.peek() {
                None => return self.error("字符串未闭合"),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(text);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'n') => text.push('\n'),
                        Some(b't') => text.push('\t'),
                        Some(b'r') => text.push('\r'),
                        Some(b'"') => text.push('"'),
                        Some(b'\\') => text.push('\\'),
                        Some(other) => text.push(other as char),
                        None => return self.error("转义序列未结束"),
                    }
                    self.pos += 1;
                }
                Some(_) => {
                    let rest = std::str::from_utf8(&self.bytes[self.pos..]).map_err(|_| {
                        ParseError::Syntax {
                            position: self.pos,
                            message: "非法 UTF-8".into(),
                        }
                    })?;
                    let ch = rest.chars().next().unwrap_or('\u{fffd}');
                    text.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn parse_number(&mut self) -> Result<f64, ParseError> {
        let start = self.pos;
        if matches!(self.peek(), Some(b'+') | Some(b'-')) {
            self.pos += 1;
        }
        while let Some(byte) = self.peek() {
            if byte.is_ascii_digit()
                || byte == b'.'
                || byte == b'e'
                || byte == b'E'
                || ((byte == b'+' || byte == b'-')
                    && matches!(self.bytes.get(self.pos - 1), Some(b'e') | Some(b'E')))
            {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
        text.parse::<f64>().map_err(|_| ParseError::Syntax {
            position: start,
            message: format!("非法数字 '{text}'"),
        })
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, ParseError> {
        if depth > MAX_DEPTH {
            return Err(ParseError::TooDeep);
        }
        self.skip_trivia(true);
        match self.peek() {
            Some(b'{') => self.parse_table(depth),
            Some(b'"') => self.parse_string().map(Value::Str),
            Some(byte) if byte.is_ascii_digit() || byte == b'-' || byte == b'+' => {
                self.parse_number().map(Value::Number)
            }
            Some(_) => {
                let ident = self.parse_ident().unwrap_or_default();
                match ident.as_str() {
                    "true" => Ok(Value::Bool(true)),
                    "false" => Ok(Value::Bool(false)),
                    "nil" => Ok(Value::Bool(false)),
                    other => self.error(format!("无法识别的值 '{other}'")),
                }
            }
            None => self.error("值缺失"),
        }
    }

    fn parse_key(&mut self) -> Result<Key, ParseError> {
        self.skip_trivia(true);
        if self.peek() == Some(b'[') {
            self.pos += 1;
            self.skip_trivia(true);
            let key = match self.peek() {
                Some(b'"') => Key::Str(self.parse_string()?),
                Some(byte) if byte.is_ascii_digit() || byte == b'-' => {
                    Key::Int(self.parse_number()? as i64)
                }
                _ => Key::Str(self.parse_ident().unwrap_or_default()),
            };
            self.expect(b']')?;
            Ok(key)
        } else {
            match self.parse_ident() {
                Some(ident) => Ok(Key::Str(ident)),
                None => self.error("键缺失"),
            }
        }
    }

    fn parse_table(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.expect(b'{')?;
        let mut entries = Vec::new();
        loop {
            self.skip_trivia(true);
            match self.peek() {
                None => return self.error("表未闭合"),
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Table(entries));
                }
                _ => {}
            }
            let key = self.parse_key()?;
            self.expect(b'=')?;
            let value = self.parse_value(depth + 1)?;
            entries.push((key, value));
        }
    }

    /// 解析整个文件：允许 `input_stat = <表>`、`return <表>` 或直接的 `<表>`。
    fn parse_document(&mut self) -> Result<Value, ParseError> {
        self.skip_trivia(true);
        if let Some(ident) = self.parse_ident() {
            self.skip_trivia(true);
            if self.peek() == Some(b'=') {
                self.pos += 1; // `input_stat = …`
            } else if ident != "return" {
                return self.error(format!("无法识别的文件前缀 '{ident}'"));
            }
        }
        let value = self.parse_value(0)?;
        self.skip_trivia(true);
        if self.pos != self.bytes.len() {
            return self.error("文件尾部有多余内容");
        }
        Ok(value)
    }
}

/// 从解析出的表里读取一个周期桶；缺失字段按默认值处理。
fn bucket_from(value: Option<&Value>) -> Bucket {
    let Some(value) = value else {
        return Bucket::default();
    };
    let fastest = value.field("fastest");
    let numbers = |name: &str| -> Vec<f64> {
        value
            .field(name)
            .map(|seq| seq.sequence().iter().filter_map(|v| v.number()).collect())
            .unwrap_or_default()
    };
    Bucket {
        count: value.field("count").and_then(Value::int).unwrap_or(0),
        length: value.field("length").and_then(Value::int).unwrap_or(0),
        fastest: Fastest {
            spd: fastest
                .and_then(|f| f.field("spd"))
                .and_then(Value::number)
                .unwrap_or(0.0),
            key_touch_spd: fastest
                .and_then(|f| f.field("keyTouchSpd"))
                .and_then(Value::number)
                .unwrap_or(0.0),
            avg_code_len: fastest
                .and_then(|f| f.field("avgCodeLen"))
                .and_then(Value::number)
                .unwrap_or(0.0),
        },
        ts: value.field("ts").and_then(Value::int).unwrap_or(0),
        lengths: value
            .field("lengths")
            .map(Value::count_map)
            .unwrap_or_default(),
        code_lengths: value
            .field("codeLengths")
            .map(Value::count_map)
            .unwrap_or_default(),
        avg_gaps: numbers("avgGaps"),
        avg_cnts: numbers("avgCnts"),
        avg_code_len: numbers("avgCodeLen"),
        key_touch_cnt: value.field("keyTouchCnt").and_then(Value::int).unwrap_or(0),
        total_code_len_without_space: value
            .field("totalCodeLenWithoutSpace")
            .and_then(Value::int)
            .unwrap_or(0),
    }
}

/// 读取 Lua 版或本模块写出的统计文件。
///
/// 兼容：`input_stat = {...}`、`return {...}`、裸表字面量、`--` 行注释、`,`/`;` 分隔符、
/// 任意键顺序、Lua 写出的 `true`/`false`。未知字段忽略；缺失的 `logVer` 读作 `-1`，
/// 以便上层按 §4.4 触发版本迁移。
pub fn parse(text: &str) -> Result<InputStats, ParseError> {
    let mut parser = Parser::new(text);
    let root = parser.parse_document()?;
    let new_words = root.field("newWords");
    Ok(InputStats {
        daily: bucket_from(root.field("daily")),
        weekly: bucket_from(root.field("weekly")),
        monthly: bucket_from(root.field("monthly")),
        yearly: bucket_from(root.field("yearly")),
        log_ver: root.field("logVer").and_then(Value::int).unwrap_or(-1),
        daily_max: root.field("daily_max").and_then(Value::int).unwrap_or(0),
        new_words: NewWords {
            start_time: new_words
                .and_then(|n| n.field("startTime"))
                .and_then(Value::int)
                .unwrap_or(0),
            words: new_words
                .and_then(|n| n.field("words"))
                .map(Value::word_set)
                .unwrap_or_default(),
        },
        device_type: root
            .field("deviceType")
            .and_then(Value::text)
            .unwrap_or_default()
            .to_owned(),
        skin_idx_word: root
            .field("progressBarSkinIdx_word")
            .and_then(Value::int)
            .unwrap_or(DEFAULT_SKIN_INDEX),
        skin_idx_code: root
            .field("progressBarSkinIdx_code")
            .and_then(Value::int)
            .unwrap_or(DEFAULT_SKIN_INDEX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Bucket, LOG_VERSION};

    fn sample_bucket() -> Bucket {
        let mut bucket = Bucket::at(1_700_000_000);
        bucket.count = 2;
        bucket.length = 5;
        bucket.key_touch_cnt = 12;
        bucket.total_code_len_without_space = 9;
        bucket.fastest = Fastest {
            spd: 83.076,
            key_touch_spd: 4.461,
            avg_code_len: 3.222,
        };
        bucket.lengths = BTreeMap::from([(2_i64, 1_i64), (3_i64, 1_i64)]);
        bucket.code_lengths = BTreeMap::from([(4_i64, 2_i64)]);
        bucket.avg_gaps = vec![4.0, 3.5];
        bucket.avg_cnts = vec![6.0, 5.0];
        bucket.avg_code_len = vec![18.0, 17.0];
        bucket
    }

    fn sample_stats() -> InputStats {
        let bucket = sample_bucket();
        InputStats {
            daily: bucket.clone(),
            weekly: bucket.clone(),
            monthly: bucket.clone(),
            yearly: bucket,
            log_ver: LOG_VERSION,
            daily_max: 3,
            new_words: NewWords {
                start_time: 1_700_000_000,
                words: BTreeSet::from(["你好".to_owned()]),
            },
            device_type: "💻".into(),
            skin_idx_word: 5,
            skin_idx_code: 7,
        }
    }

    #[test]
    fn file_name_matches_lua_layout() {
        assert_eq!(file_name("luna_pinyin"), "input_stat_luna_pinyin.lua");
    }

    #[test]
    fn serialized_text_is_lua_table_literal_with_lua_key_names() {
        let text = serialize(&sample_stats());
        assert!(text.starts_with("input_stat = {"), "以 input_stat 变量开头");
        assert!(text.ends_with("}\n"), "以换行结束");
        for key in [
            "[\"daily\"]",
            "[\"weekly\"]",
            "[\"monthly\"]",
            "[\"yearly\"]",
            "[\"logVer\"]",
            "[\"daily_max\"]",
            "[\"newWords\"]",
            "[\"startTime\"]",
            "[\"words\"]",
            "[\"deviceType\"]",
            "[\"progressBarSkinIdx_word\"]",
            "[\"progressBarSkinIdx_code\"]",
            "[\"count\"]",
            "[\"length\"]",
            "[\"fastest\"]",
            "[\"spd\"]",
            "[\"keyTouchSpd\"]",
            "[\"avgCodeLen\"]",
            "[\"ts\"]",
            "[\"lengths\"]",
            "[\"codeLengths\"]",
            "[\"avgGaps\"]",
            "[\"avgCnts\"]",
            "[\"keyTouchCnt\"]",
            "[\"totalCodeLenWithoutSpace\"]",
        ] {
            assert!(text.contains(key), "缺少 Lua 版字段名 {key}");
        }
        assert!(text.contains("[\"avgGaps\"] = {\n\t\t\t[1] = 4,"), "整数值写成整数");
        assert!(text.contains("[\"avgGaps\"] = {\n\t\t\t[1] = 4,\n\t\t\t[2] = 3.5,"), "小数保留");
        assert!(text.contains("[\"words\"] = {\n\t\t\t[\"你好\"] = 1,"), "生字词集合");
        assert!(text.contains("[\"deviceType\"] = \"💻\""), "设备类型字符串");
    }

    #[test]
    fn serialize_then_parse_round_trips_exactly() {
        let stats = sample_stats();
        let parsed = parse(&serialize(&stats)).expect("解析自己写出的文件");
        assert_eq!(parsed, stats);
    }

    #[test]
    fn strings_with_quotes_survive_round_trip() {
        let mut stats = InputStats::default();
        stats.device_type = "a\"b\\c".into();
        stats.new_words.words.insert("say \"hi\"".into());
        let text = serialize(&stats);
        assert!(text.contains("\\\""), "引号被转义");
        assert_eq!(parse(&text).unwrap(), stats, "转义后可读回");
    }

    #[test]
    fn parses_lua_flavoured_input_with_comments_and_any_order() {
        let text = r#"
-- input_statistics.lua 写出的统计
input_stat = {
  ["daily_max"] = 4,
  ["logVer"] = 4,
  ["deviceType"] = "📱",
  ["newWords"] = { ["startTime"] = 123, ["words"] = { ["世界"] = 1, ["你好"] = 1 } },
  ["progressBarSkinIdx_word"] = 3,
  ["weekly"] = {
    ["count"] = 1, ["length"] = 4;
    ["fastest"] = { ["avgCodeLen"] = 2.5, ["spd"] = 96.0, ["keyTouchSpd"] = 4.0 },
    ["lengths"] = { [4] = 1 },
    ["codeLengths"] = { [5] = 1 },
    ["avgGaps"] = { [1] = 2 },
    ["avgCnts"] = { [1] = 4 },
    ["avgCodeLen"] = { [1] = 5 },
    ["keyTouchCnt"] = 5,
    ["totalCodeLenWithoutSpace"] = 4,
    ["ts"] = 1700000000,
  },
  ["daily"] = { ["count"] = 3, ["length"] = 9, ["ts"] = 1700000000 },
}
"#;
        let stats = parse(text).expect("解析 Lua 风格输入");
        assert_eq!(stats.log_ver, 4);
        assert_eq!(stats.daily_max, 4);
        assert_eq!(stats.device_type, "📱");
        assert_eq!(stats.new_words.start_time, 123);
        assert_eq!(stats.new_words.words.len(), 2);
        assert!(stats.new_words.words.contains("世界"));
        assert_eq!(stats.skin_idx_word, 3);
        assert_eq!(stats.skin_idx_code, DEFAULT_SKIN_INDEX, "缺失的索引回退默认值");
        assert_eq!(stats.daily.count, 3);
        assert_eq!(stats.weekly.count, 1);
        assert_eq!(stats.weekly.lengths.get(&4), Some(&1));
        assert_eq!(stats.weekly.code_lengths.get(&5), Some(&1));
        assert_eq!(stats.weekly.avg_gaps, vec![2.0]);
        assert_eq!(stats.weekly.avg_cnts, vec![4.0]);
        assert_eq!(stats.weekly.avg_code_len, vec![5.0]);
        assert_eq!(stats.weekly.key_touch_cnt, 5);
        assert_eq!(stats.weekly.total_code_len_without_space, 4);
        assert!((stats.weekly.fastest.spd - 96.0).abs() < 1e-12);
        assert!((stats.weekly.fastest.avg_code_len - 2.5).abs() < 1e-12);
        assert_eq!(stats.monthly, Bucket::default());
        assert_eq!(stats.yearly, Bucket::default());
    }

    #[test]
    fn missing_log_version_reads_as_minus_one() {
        let mut stats = parse("{ [\"daily\"] = { [\"count\"] = 1 } }").unwrap();
        assert_eq!(stats.log_ver, -1, "缺失 logVer 必须触发迁移");
        assert!(stats.migrate_format(LOG_VERSION), "缺失 logVer 时迁移会重置历史");
        assert_eq!(stats.log_ver, LOG_VERSION);
        assert_eq!(stats.daily.count, 0, "迁移是破坏性的");
    }

    #[test]
    fn accepts_return_statement_and_bare_table() {
        let stats = parse("return { [\"daily\"] = { [\"count\"] = 2 } }").unwrap();
        assert_eq!(stats.daily.count, 2);
        let stats = parse("{ [\"daily_max\"] = 7 }").unwrap();
        assert_eq!(stats.daily_max, 7);
    }

    #[test]
    fn reports_syntax_errors_with_position() {
        match parse("input_stat = { [\"daily\"] = { [\"count\"] = } }").unwrap_err() {
            ParseError::Syntax { position, .. } => assert!(position > 0),
            other => panic!("期望语法错误，得到 {other:?}"),
        }
        assert!(parse("input_stat = { ").is_err(), "未闭合的表应报错");
        assert!(parse("{ [\"daily\"] = 1 } trailing").is_err(), "尾部多余内容应报错");
        assert!(parse("nonsense { }").is_err(), "未知前缀应报错");
    }

    #[test]
    fn rejects_excessive_nesting() {
        let mut text = String::from("input_stat = ");
        for _ in 0..(MAX_DEPTH + 5) {
            text.push_str("{ [\"a\"] = ");
        }
        assert_eq!(parse(&text), Err(ParseError::TooDeep));
    }
}
