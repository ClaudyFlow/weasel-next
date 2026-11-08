//! 符号面板的数据层：解析 Rime 的 `symbols.yaml`（`punctuator/symbols` 段）。
//!
//! 2026-11-06：编码栏「符」按钮的面板之前用一张硬编码 emoji 表，跟输入法实际能打出的
//! 符号完全对不上。这里直接读同一份 `symbols.yaml`（用户目录优先、程序 `data/` 回退），
//! 于是面板与 `/fh`、`/jt` 这些输入码看到的是同一批字符。
//!
//! 之所以手写行扫描而不用 YAML 库：`symbols.yaml` 的 `symbols:` 段是 200 多行
//! 「注释当分类名 + 单行 flow 序列」，注释本身是**数据**（分类名），YAML 解析后
//! 会丢掉；而且里面有大量未加引号的 `©`、`〔`、`←`，通用解析器容易把 `]`、
//! `,` 判成语法。这里只认这个固定形状，稳且零依赖。
//!
//! 数据形状（真实文件）：
//! ```yaml
//!   symbols:
//! #符號、電腦
//!     '/fh': [ ©, ®, ℗, ... ]
//!     '/dn': [ ❖, ⌘, ... ]
//! #象棋、麻將、色子、撲克
//!     '/xq': [ ♔, ♕, ... ]
//! ```
//! 一条注释覆盖**其后连续若干个**分组，直到下一条注释出现，所以 label 要向后继承。

use std::path::{Path, PathBuf};

/// 一个输入码对应的符号分组（`/fh` → 「符號、電腦」→ 158 个字符）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolGroup {
    /// 输入码，如 `/fh`。
    pub code: String,
    /// 分类名（来自上方注释），向后继承；没有注释时回退成输入码。
    pub label: String,
    /// 该组里的候选字符，保持文件里的顺序。
    pub items: Vec<String>,
}

impl SymbolGroup {
    /// 面板标签上显示的短名。
    pub fn title(&self) -> String {
        let name = if self.label.is_empty() {
            self.code.trim_start_matches('/')
        } else {
            // 「象棋、麻將、色子、撲克」这类长标签只取第一截，标签行放得下。
            self.label.split(['、', ',', '，']).next().unwrap_or(&self.label)
        };
        name.chars().take(4).collect()
    }
}

/// 解析 `symbols.yaml` 全文，返回 `punctuator/symbols` 下的所有分组。
/// 找不到该段或格式不符时返回空 `Vec`（调用方据此回退到内置表）。
pub fn parse(text: &str) -> Vec<SymbolGroup> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim_end() == "  symbols:") else {
        return Vec::new();
    };

    let mut groups: Vec<SymbolGroup> = Vec::new();
    let mut label = String::new();
    let mut pending_key: Option<String> = None;
    let mut pending_buf = String::new();

    for line in lines.iter().skip(start + 1) {
        let trimmed = line.trim();

        // 离开 symbols 段：分组行统一缩进 4 空格，遇到缩进 ≤2 的实体行
        // （如 `  half_shape:` 或顶格 `other_key:`）就收工。注释行固定在第 0 列，
        // 不受此限。
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
            if indent <= 2 {
                break;
            }
        }
        if trimmed.starts_with('#') {
            label = trimmed.trim_start_matches('#').trim().to_string();
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }

        // 续行：上一行的 `[` 还没闭合。
        if let Some(_key) = pending_key.as_ref() {
            pending_buf.push(' ');
            pending_buf.push_str(trimmed);
            if trimmed.contains(']') {
                if let Some(key) = pending_key.take() {
                    push_group(&mut groups, key, &label, &pending_buf);
                }
                pending_buf.clear();
            }
            continue;
        }

        // 新分组：`'/xx': [ ... ]`
        if !trimmed.starts_with('\'') || !trimmed.contains(':') {
            continue;
        }
        let Some(quote_end) = trimmed[1..].find('\'') else {
            continue;
        };
        let key = trimmed[1..1 + quote_end].to_string();
        let rest = trimmed[1 + quote_end + 1..].trim_start_matches(':').trim();
        if rest.contains('[') && !rest.contains(']') {
            pending_key = Some(key);
            pending_buf = rest.to_string();
            continue;
        }
        push_group(&mut groups, key, &label, rest);
        // 注释只标注这一组分类的**起点**，其后的分组沿用同一 label，
        // 所以这里不清空 label；下一条注释会覆盖它。
    }
    groups.retain(|g| !g.items.is_empty());
    groups
}

fn push_group(groups: &mut Vec<SymbolGroup>, key: String, label: &str, rest: &str) {
    let Some(open) = rest.find('[') else {
        return;
    };
    let Some(close) = rest.rfind(']') else {
        return;
    };
    if close <= open {
        return;
    }
    let body = &rest[open + 1..close];
    let items: Vec<String> = body
        .split(',')
        .map(|s| s.trim())
        .map(|s| s.trim_matches(|c| c == '\'' || c == '"' || c == '`').trim().to_string())
        .filter(|s| !s.is_empty() && !s.contains('{') && !s.contains('}'))
        .collect();
    if items.is_empty() {
        return;
    }
    groups.push(SymbolGroup {
        code: key,
        label: label.to_string(),
        items,
    });
}

/// 内置兜底表：`symbols.yaml` 一个都读不到时面板也不能空白。
/// 第一组始终是「常用」，其余按 Rime 的习惯分类。
pub fn fallback() -> Vec<SymbolGroup> {
    fn group(code: &str, label: &str, list: &str) -> SymbolGroup {
        SymbolGroup {
            code: code.into(),
            label: label.into(),
            items: list.split_whitespace().map(str::to_string).collect(),
        }
    }
    vec![
        group(
            "/常用",
            "常用",
            "😀 😁 😂 🤣 😊 😍 😘 😎 🤔 😅 🙂 🙃 😉 😢 😭 😡 🥺 😴 🥰 😇 🙄 😋 🤩 🥳 😱 🤗 🤝 👍 👎 👏 🙏 💪 👀 💬 🔗 📷 🎵 💤",
        ),
        group("/标点", "中文标点", "， 。 ： ； ！ ？ … — · 「」 『』 （） 【】 《》 〈〉 “” ‘’ 、 々 〃"),
        group("/箭头", "箭头", "↑ ↓ ← → ↖ ↗ ↙ ↘ ↔ ↕ ⇄ ⇅ ⇒ ⇔ ↵ ➜ ➤ ➡"),
        group("/星号", "星号", "★ ☆ ✦ ✧ ✪ ✫ ✬ ✭ ✮ ✯ ✰ ✱ ✲ ✳ ✴ ✵ ✶ ✷ ✸ ✹ ✺ ✻ ✼ ✽ ✾ ✿ ❀ ❁ ❂ ❃ ❄ ❅ ❆ ❇ ❈ ❉ ❊ ❋"),
        group("/几何", "几何", "■ □ ▲ △ ▼ ▽ ◆ ◇ ○ ● ◎ ◐ ◑ ◒ ◓ ◢ ◣ ◤ ◥ ◧ ◨ ◩ ◪ ◫ ★ ☆"),
        group("/数学", "数学", "＋ － × ÷ ＝ ≠ ≈ ≡ ≤ ≥ ± ∓ ∞ ∑ ∏ √ ∫ ∂ ∇ ∈ ∉ ⊆ ⊇ ∪ ∩ ∠ ⊥ ∥ ° ℃ ％ ‰"),
        group("/货币", "货币", "￥ ¥ ＄ $ € £ ¢ ₩ ￠ ￡ ﹩ ￡ ㊎ ㊍ ㊌ ㊋ ㊏"),
    ]
}

/// 按「用户目录 → 程序 data 目录 → 内置表」的顺序加载，并在最前面补一组常用 emoji。
pub fn load(user_root: &Path, shared_data: Option<&Path>) -> Vec<SymbolGroup> {
    let mut candidates: Vec<PathBuf> = vec![user_root.join("symbols.yaml")];
    if let Some(dir) = shared_data {
        candidates.push(dir.join("symbols.yaml"));
    }
    let mut groups: Vec<SymbolGroup> = candidates
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok().and_then(|t| {
            let g = parse(&t);
            (!g.is_empty()).then_some(g)
        }))
        .unwrap_or_default();

    let mut all = vec![fallback().remove(0)];
    if groups.is_empty() {
        all.extend(fallback().into_iter().skip(1));
    } else {
        all.append(&mut groups);
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
config_version: '1.6'
punctuator:
  symbols:
#符號、電腦
    '/fh': [ ©, ®, '℗', × ]
    '/dn': [ ⌘, ⇧ ]
#象棋、麻將
    '/xq': [ ♔, ♕ ]
    '/mj': [ 🀀, 🀁 ]
#表情
    '/bq': [ ☻, ☺ ]
  half_shape:
    ',' : { commit: ， }
"#;

    #[test]
    fn parses_groups_with_inherited_comment_labels() {
        let g = parse(SAMPLE);
        assert_eq!(g.len(), 5, "实际分组：{g:#?}");
        assert_eq!(g[0].code, "/fh");
        assert_eq!(g[0].label, "符號、電腦");
        assert_eq!(g[0].items, vec!["©", "®", "℗", "×"]);
        assert_eq!(g[1].code, "/dn");
        assert_eq!(g[1].label, "符號、電腦", "注释向后继承到下一组");
        assert_eq!(g[2].label, "象棋、麻將");
        assert_eq!(g[3].label, "象棋、麻將");
        assert_eq!(g[4].label, "表情");
    }

    #[test]
    fn stops_at_next_top_level_key() {
        let g = parse(&format!("{SAMPLE}other_key: 1\n"));
        assert_eq!(g.len(), 5);
    }

    #[test]
    fn missing_section_yields_empty() {
        assert!(parse("punctuator:\n  full_shape: {}\n").is_empty());
    }

    #[test]
    fn title_is_short_and_never_empty() {
        let g = parse(SAMPLE);
        assert_eq!(g[0].title(), "符號", "长标签只取顿号前那一截");
        assert!(!SymbolGroup { code: "/zz".into(), label: String::new(), items: vec![] }.title().is_empty());
    }

    #[test]
    fn parses_the_real_shipped_symbols_yaml() {
        // 与出厂数据对账：这一份是输入法实际用的 `Rime-Next/symbols.yaml`。
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../Rime-Next/symbols.yaml");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));
        let g = parse(&text);
        assert!(g.len() > 150, "实际分组数 {}", g.len());
        let fh = g.iter().find(|x| x.code == "/fh").expect("应有 /fh 组");
        assert_eq!(fh.label, "符號、電腦");
        assert!(fh.items.len() > 100, "/fh 实际 {} 项", fh.items.len());
        assert_eq!(fh.items[0], "©");
        let jt = g.iter().find(|x| x.code == "/jt").expect("应有 /jt 箭头组");
        assert_eq!(jt.items[0], "↑");
        assert!(g.iter().all(|x| x.items.iter().all(|s| !s.contains('[') && !s.contains(']'))));
    }

    #[test]
    fn fallback_always_has_usable_items() {
        let f = fallback();
        assert_eq!(f[0].code, "/常用");
        assert!(f.iter().all(|g| !g.items.is_empty()));
    }
}
