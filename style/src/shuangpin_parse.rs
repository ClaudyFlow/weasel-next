//! 自定义双拼解析器（用户 2026-10-19 定案）
//!
//! **目标**：给一份「音节 YAML」就能把按键串切成音节，不再依赖 rime 的
//! `algebra` / `preedit_format` 正则。
//!
//! 一个方案 = `initial`（双字母声母键位）+ `final`（韵母键位）+
//! `syllables`（合法音节表，消除同键多韵母歧义，来自
//! `Rime-Next\parser\pinyin\syllables.parser.yaml`，424 条，2026-10-27 从 .txt 转 YAML）。
//!
//! 键位源文件（`parser/shuangpin/<方案>.parser.yaml`）是 YAML（2026-10-26 起不再认 TOML）：
//! 顶层 `schema:` 块放元信息，`initial:` / `final:` 放键位映射。
//!
//! 规则：每音节 2 键；`zh/ch/sh` 各占一键；零声母用双写（`an`→`aan`）或原样；
//! 同键多韵母（`s`=ong/iong、`d`=iang/uang）用音节表挑真实存在的组合。

use std::collections::{BTreeMap, HashSet};

#[derive(Debug, serde::Deserialize)]
struct Raw {
    /// 键位文件里的实际键名是**复数**（`initials:` / `finals:`）——
    /// 单数 `final` 是 Rust 2024 保留字，字段名这才用了复数。
    #[serde(default)]
    initials: BTreeMap<String, String>,
    #[serde(default)]
    finals: BTreeMap<String, String>,
    #[serde(default)]
    syllables: Vec<String>,
}

/// syllables.parser.yaml 的结构（2026-10-27 从 syllables.txt 转换）。
#[derive(Debug, serde::Deserialize)]
struct SyllablesRaw {
    #[serde(default)]
    syllables: Vec<String>,
}

/// 解析出来的一个音节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub keys: String,
    pub pinyin: String,
    pub incomplete: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Scheme {
    pub initial: BTreeMap<String, String>,
    pub r#final: BTreeMap<String, String>,
    pub syllables: HashSet<String>,
    by_key: BTreeMap<char, Vec<String>>,
    zero: Vec<(String, String)>,
}

impl Scheme {
    /// 从键位 YAML 文本解析（2026-10-26：不再提供 TOML 识别）。
    pub fn from_yaml(text: &str) -> Result<Self, String> {
        let raw: Raw = weasel_common::yaml::from_str(text)?;
        Ok(Self::new(raw.initials, raw.finals, raw.syllables))
    }

    /// 方案 YAML + 音节表 YAML 分开给（引擎里就是这么用的）。
    /// 2026-10-27：音节表从 syllables.txt 转为 syllables.parser.yaml，直接用 YAML 解析。
    pub fn from_yaml_parts(scheme_text: &str, syllables_yaml: &str) -> Result<Self, String> {
        let raw: Raw = weasel_common::yaml::from_str(scheme_text)?;
        let syllables_raw: SyllablesRaw = weasel_common::yaml::from_str(syllables_yaml)?;
        Ok(Self::new(raw.initials, raw.finals, syllables_raw.syllables))
    }

    /// ★ 按 **parser 引用** 定位键位文件（**反查等模块复用这个入口**）。
    ///
    /// 引用就是相对 `parser\` 的路径（含 `\` 前的子文件夹=大类、主干=键位名），
    /// 如 `shuangpin/xiaohe` → `parser\shuangpin\xiaohe.parser.yaml`。路径即唯一 id，
    /// 与任何方案无关——多个方案（含反查方案）可以共用同一份键位，
    /// 也天然容纳以后加的 `zhuyin/` 等其他大类。搜索顺序：程序文件夹
    /// `Rime-Next\parser\`（出厂数据，优先）→ 用户目录 `parser\`（兼容兜底）。
    /// 传裸键位名（`xiaohe`，不带大类）时扫各大类子文件夹按主干匹配（兼容兜底）。
    pub fn locate_parser_yaml(
        install_data: &std::path::Path,
        user_data: &std::path::Path,
        parser_ref: &str,
    ) -> Option<std::path::PathBuf> {
        let rel = parser_ref.replace('\\', "/").trim_matches('/').to_string();
        if !rel.is_empty() && !rel.contains("..") {
            for root in [install_data.join("parser"), user_data.join("parser")] {
                let path = root.join(format!("{rel}.parser.yaml"));
                if path.is_file() {
                    return Some(path);
                }
            }
        }
        // 兜底：裸键位名（`xiaohe`）——扫 parser\<大类>\ 各子文件夹按文件主干匹配。
        if !rel.contains('/') {
            for root in [install_data.join("parser"), user_data.join("parser")] {
                let Ok(cats) = std::fs::read_dir(&root) else { continue };
                for cat in cats.flatten() {
                    if !cat.path().is_dir() {
                        continue;
                    }
                    let path = cat.path().join(format!("{rel}.parser.yaml"));
                    if path.is_file() {
                        return Some(path);
                    }
                }
            }
        }
        None
    }

    /// 按固定路径定位**音节表**（双拼/全拼共用，住 `parser\pinyin\`）。
    pub fn locate_syllables_yaml(
        install_data: &std::path::Path,
        user_data: &std::path::Path,
    ) -> Option<std::path::PathBuf> {
        for dir in [
            install_data.join("parser").join("pinyin"),
            user_data.join("parser").join("pinyin"),
        ] {
            let path = dir.join("syllables.parser.yaml");
            if path.is_file() {
                return Some(path);
            }
        }
        None
    }

    /// **parser 引用**（如 `shuangpin/xiaohe`）→ 可用的 [`Scheme`]（定位 + 读盘 + 解析一步到位）。
    ///
    /// # Errors
    /// 文件缺失或 YAML 解析失败时返回错误文本。
    pub fn load_by_parser_id(
        install_data: &std::path::Path,
        user_data: &std::path::Path,
        parser_ref: &str,
    ) -> Result<Self, String> {
        let scheme_path = Self::locate_parser_yaml(install_data, user_data, parser_ref)
            .ok_or_else(|| format!("找不到 parser 引用为 {parser_ref} 的键位文件"))?;
        let syllables_path = Self::locate_syllables_yaml(install_data, user_data)
            .ok_or_else(|| "找不到音节表 syllables.parser.yaml".to_string())?;
        let scheme_text = std::fs::read_to_string(&scheme_path)
            .map_err(|e| format!("读 {} 失败：{e}", scheme_path.display()))?;
        let syllables_yaml = std::fs::read_to_string(&syllables_path)
            .map_err(|e| format!("读 {} 失败：{e}", syllables_path.display()))?;
        Self::from_yaml_parts(&scheme_text, &syllables_yaml)
    }

    /// 方案 YAML + 音节表纯文本分开给（旧版，保留兼容）。
    pub fn from_parts(scheme_text: &str, syllables_text: &str) -> Result<Self, String> {
        let raw: Raw = weasel_common::yaml::from_str(scheme_text)?;
        // 音节表是纯文本：一行一个音节（不用 YAML，免得被扫描当成方案配置）
        let syllables: Vec<String> = syllables_text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_string)
            .collect();
        Ok(Self::new(raw.initials, raw.finals, syllables))
    }

    pub fn new(
        initial: BTreeMap<String, String>,
        r#final: BTreeMap<String, String>,
        syllables: Vec<String>,
    ) -> Self {
        let mut by_key: BTreeMap<char, Vec<String>> = BTreeMap::new();
        for (final_, key) in &r#final {
            if let Some(ch) = key.chars().next() {
                by_key.entry(ch).or_default().push(final_.clone());
            }
        }
        for list in by_key.values_mut() {
            list.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()));
        }
        let zeros = ["a", "ai", "an", "ang", "ao", "e", "ei", "en", "eng", "er", "o", "ou"];
        let mut zero: Vec<(String, String)> = Vec::new();
        for final_ in zeros {
            let first = &final_[..1];
            if "aoe".contains(first) {
                zero.push((format!("{first}{final_}"), final_.to_string()));
            }
        }
        for final_ in zeros {
            zero.push((final_.to_string(), final_.to_string()));
        }
        zero.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count()));
        Self {
            initial,
            r#final,
            syllables: syllables.into_iter().collect(),
            by_key,
            zero,
        }
    }

    pub fn parse(&self, keys: &str) -> Vec<Token> {
        let chars: Vec<char> = keys.chars().filter(|c| c.is_ascii_alphabetic()).collect();
        let mut out = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            if let Some((key, pinyin, len)) = self.match_zero(&chars, index) {
                out.push(Token { keys: key, pinyin, incomplete: false });
                index += len;
                continue;
            }
            if index + 1 < chars.len() {
                if let Some(pinyin) = self.combine(chars[index], chars[index + 1]) {
                    out.push(Token {
                        keys: format!("{}{}", chars[index], chars[index + 1]),
                        pinyin,
                        incomplete: false,
                    });
                    index += 2;
                    continue;
                }
            }
            out.push(Token {
                keys: chars[index].to_string(),
                pinyin: String::new(),
                incomplete: true,
            });
            index += 1;
        }
        out
    }

    /// 按「全拼」贪心切分（最长优先）：整串都能切成合法音节时返回空格分隔的拼音。
    ///
    /// 用途：用户在双拼方案里直接打全拼（`hang` / `qingkong`）时，
    /// 不应该被当成双拼键位去解码（那会得到 `haneng` 这种乱码）。
    fn segment_pinyin(&self, keys: &str) -> Option<String> {
        if self.syllables.is_empty() {
            return None;
        }
        let chars: Vec<char> = keys.chars().filter(|c| c.is_ascii_alphabetic()).collect();
        if chars.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            let mut matched = false;
            for len in (1..=6).rev() {
                if index + len > chars.len() {
                    continue;
                }
                let candidate: String = chars[index..index + len].iter().collect();
                if self.syllables.contains(&candidate) {
                    out.push(candidate);
                    index += len;
                    matched = true;
                    break;
                }
            }
            if !matched {
                return None;
            }
        }
        Some(out.join(" "))
    }

    pub fn full_pinyin(&self, keys: &str) -> String {
        // 先按「全拼」切：能整串切成合法音节就原样输出（用户直接打全拼时）。
        // 例外：`aa`/`ee`/`oo` 开头是双拼的零声母双写，不能当全拼切。
        let head: Vec<char> = keys.chars().filter(|c| c.is_ascii_alphabetic()).take(2).collect();
        let doubled_zero = head.len() == 2 && head[0] == head[1] && "aoe".contains(head[0]);
        if !doubled_zero {
            if let Some(text) = self.segment_pinyin(keys) {
                return text;
            }
        }
        self.parse(keys)
            .iter()
            .map(|token| {
                if token.incomplete {
                    token.keys.clone()
                } else {
                    token.pinyin.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn match_zero(&self, chars: &[char], index: usize) -> Option<(String, String, usize)> {
        for (key, pinyin) in &self.zero {
            let len = key.chars().count();
            if index + len > chars.len() {
                continue;
            }
            let candidate: String = chars[index..index + len].iter().collect();
            if candidate != *key {
                continue;
            }
            if !self.syllables.is_empty() && !self.syllables.contains(pinyin) {
                continue;
            }
            let first = &key[..1];
            let doubled = key.chars().count() >= 2 && key.starts_with(&format!("{first}{first}"));
            if !doubled {
                if index + len + 1 < chars.len()
                    && self.combine(chars[index + len], chars[index + len + 1]).is_some()
                {
                    continue;
                }
            }
            return Some((candidate, pinyin.clone(), len));
        }
        None
    }

    fn combine(&self, first: char, second: char) -> Option<String> {
        let initials = self.initials_for(first);
        let finals = self.by_key.get(&second)?;
        for initial in &initials {
            for final_ in finals {
                let syllable = format!("{initial}{final_}");
                if self.syllables.is_empty() || self.syllables.contains(&syllable) {
                    return Some(syllable);
                }
            }
        }
        match (initials.first(), finals.first()) {
            (Some(initial), Some(final_)) => Some(format!("{initial}{final_}")),
            _ => None,
        }
    }

    fn initials_for(&self, key: char) -> Vec<String> {
        let mut result = Vec::new();
        for (initial, mapped) in &self.initial {
            if mapped.chars().next() == Some(key) {
                result.push(initial.clone());
            }
        }
        if result.is_empty() && "bpmfdtnlgkhjqxrzcsyw".contains(key) {
            result.push(key.to_string());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syllables() -> Vec<String> {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../Rime-Next/parser/pinyin/syllables.parser.yaml"
        ))
        .expect("syllables.parser.yaml");
        let raw: SyllablesRaw = weasel_common::yaml::from_str(&text).expect("音节表 YAML 解析");
        raw.syllables
    }

    fn ziranma() -> Scheme {
        let mut initials = BTreeMap::new();
        initials.insert("zh".into(), "v".into());
        initials.insert("ch".into(), "i".into());
        initials.insert("sh".into(), "u".into());
        let mut finals = BTreeMap::new();
        for (final_, key) in [
            ("a", "a"), ("o", "o"), ("e", "e"), ("i", "i"), ("u", "u"), ("v", "v"),
            ("ai", "l"), ("ei", "z"), ("ui", "v"), ("ao", "k"), ("ou", "b"), ("iu", "q"),
            ("ie", "x"), ("ue", "t"), ("ve", "t"), ("an", "j"), ("en", "f"), ("in", "n"),
            ("un", "p"), ("ang", "h"), ("eng", "g"), ("ing", "y"), ("ong", "s"),
            ("ia", "w"), ("iao", "c"), ("ian", "m"), ("iang", "d"), ("iong", "s"),
            ("ua", "w"), ("uo", "o"), ("uai", "y"), ("uan", "r"), ("uang", "d"),
        ] {
            finals.insert(final_.to_string(), key.to_string());
        }
        Scheme::new(initials, finals, syllables())
    }

    #[test]
    fn parse_ni_hao() {
        assert_eq!(ziranma().full_pinyin("nihk"), "ni hao");
    }

    #[test]
    fn parse_qing_kong() {
        assert_eq!(ziranma().full_pinyin("qyks"), "qing kong");
    }

    #[test]
    fn parse_zhuang() {
        assert_eq!(ziranma().full_pinyin("vd"), "zhuang");
    }

    #[test]
    fn parse_zero_initial() {
        assert_eq!(ziranma().full_pinyin("aan"), "an");
    }

    #[test]
    fn parse_incomplete() {
        let tokens = ziranma().parse("q");
        assert_eq!(tokens.len(), 1);
        assert!(tokens[0].incomplete);
    }

    /// ★ 反查要用的契约：按 **parser 引用**（相对路径 id）从真实目录定位并加载键位。
    /// 用仓库源数据当"程序文件夹"，用户目录指一个空路径（验证兜底顺序）。
    #[test]
    fn load_by_parser_id_from_repo_data() {
        let install_data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../Rime-Next");
        let user_data = std::path::Path::new("/nonexistent-rime-user");

        // 路径式引用：shuangpin/xiaohe、shuangpin/ziranma 都必须唯一定位并解析成功
        // （nihc = 小鹤的 ni hao：h=声母 h，c=ao；nihk = 自然码的 ni hao：k=ao）
        let xiaohe =
            Scheme::load_by_parser_id(&install_data, user_data, "shuangpin/xiaohe").expect("xiaohe");
        assert_eq!(xiaohe.full_pinyin("nihc"), "ni hao");
        let ziranma_loaded =
            Scheme::load_by_parser_id(&install_data, user_data, "shuangpin/ziranma")
                .expect("ziranma");
        assert_eq!(ziranma_loaded.full_pinyin("nihk"), "ni hao");

        // 兼容兜底：裸键位名（不带大类）也要能定位到同一个文件
        assert_eq!(
            Scheme::locate_parser_yaml(&install_data, user_data, "xiaohe"),
            Scheme::locate_parser_yaml(&install_data, user_data, "shuangpin/xiaohe")
        );

        // 引用与文件路径一致：shuangpin/xiaohe 必须落在 parser\shuangpin\xiaohe.parser.yaml
        for (refer, expected) in [
            ("shuangpin/xiaohe", "shuangpin"),
            ("shuangpin/ziranma", "shuangpin"),
        ] {
            let path = Scheme::locate_parser_yaml(&install_data, user_data, refer)
                .unwrap_or_else(|| panic!("定位 {refer} 失败"));
            assert!(
                path.parent().is_some_and(|p| p.ends_with(expected)),
                "parser 引用 {refer} 定位到的文件不在 {expected} 子文件夹：{}",
                path.display()
            );
        }

        // 不存在的引用必须返回 None，不能误配
        assert!(Scheme::locate_parser_yaml(&install_data, user_data, "nonexistent").is_none());
        assert!(Scheme::locate_parser_yaml(&install_data, user_data, "shuangpin/nonexistent").is_none());
    }

    /// 从真实形态的键位 YAML 解析（键位映射 + 行尾注释 + schema 元信息块）。
    #[test]
    fn from_yaml_reads_key_mapping() {
        let text = "\
schema:
  id: double_pinyin_ziranma
  name: 自然码双拼·简体
  version: \"0.18\"

initials:
  zh: \"v\"   # 声母双字母
  ch: \"i\"
  sh: \"u\"

finals:
  a: \"a\"
  o: \"o\"
  e: \"e\"
  i: \"i\"
  u: \"u\"
  v: \"v\"
  ai: \"l\"
  ei: \"z\"
  ao: \"k\"
  ou: \"b\"
  iu: \"q\"
  ie: \"x\"
  an: \"j\"
  en: \"f\"
  in: \"n\"
  un: \"p\"
  ang: \"h\"
  eng: \"g\"
  ing: \"y\"
  ong: \"s\"
  iao: \"c\"
  ian: \"m\"
  iang: \"d\"
  uo: \"o\"
  uai: \"y\"
  uan: \"r\"
";
        let scheme = Scheme::from_yaml(text).expect("YAML 解析");
        assert_eq!(scheme.initial.get("zh").map(String::as_str), Some("v"));
        assert_eq!(scheme.r#final.get("ai").map(String::as_str), Some("l"));
        assert_eq!(scheme.r#final.get("ing").map(String::as_str), Some("y"));
        // 键位要真的能工作：自然码 nihk = 你(ni) + 好(hao: h+ao=k)
        assert_eq!(scheme.full_pinyin("nihk"), "ni hao");
    }

    /// TOML 形态（`[finals]` 表头 + `key = value`）必须**解析失败** ——
    /// 钉死 2026-10-26 起不再认 TOML（自研 YAML 解析器只认 `key: value`）。
    #[test]
    fn toml_is_not_recognized() {
        let toml_text = "\
[initials]
zh = \"v\"

[finals]
ai = \"l\"
an = \"j\"
";
        assert!(Scheme::from_yaml(toml_text).is_err());
    }
}
