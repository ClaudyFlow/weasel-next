//! 从 `Rime-Next\parser\pinyin\pinyin.parser.yaml`（**数据**）**计算**出
//! `speller/algebra` 与 `translator/preedit_format` 的正则，内联写回
//! `Rime-Next\schema\pinyin\luna\pinyin.luna.schema.yaml` 的 `>>> generated <<<` 标记块。
//!
//! 用法：
//!   cargo run -p weasel-style --bin pinyin-schema
//!
//! 为什么要有这一步（2026-10-28）：
//!   parser 目录**只放数据**，正则一律由代码算、内联进方案 —— 和双拼
//!   （`shuangpin-schema` 从键位表算 algebra / preedit）同一套路子。
//!   以前全拼是"parser 里存成品正则 + 方案 `import_preset` 抄走"，
//!   两套架构并存；现在统一成一条：数据进、正则出。
//!
//! 方案文件里未被标记块包住的部分（switches / engine / 各组件…）**原样保留**，
//! 所以手写内容和注释不会因为重新生成而丢失。

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// 生成块起止标记。方案里形如：
///
/// ```yaml
///   # >>> generated: pinyin-schema from Rime-Next\parser\pinyin\pinyin.parser.yaml — 不要手改 <<<
///   algebra:
///     - abbrev/^([a-z]).+$/$1/  # 简拼（首字母）
///   # >>> end generated <<<
/// ```
const START_MARK: &str = ">>> generated";
const END_MARK: &str = ">>> end generated";

/// 拼写运算数据。
#[derive(Debug, Deserialize)]
struct Spec {
    #[serde(default)]
    initials: Vec<String>,
    #[serde(default)]
    abbrev: Vec<Rule>,
    #[serde(default)]
    derive: Vec<Rule>,
    #[serde(default)]
    preedit: Vec<Rule>,
}

/// 一条拼写运算：`pattern` → `replacement`，可选行尾注释。
#[derive(Debug, Deserialize)]
struct Rule {
    pattern: String,
    replacement: String,
    #[serde(default)]
    comment: Option<String>,
}

/// 拼成 rime 的一条运算式：`derive/^([nl])ve$/$1ue/  # nve = nue`。
fn rule_line(kind: &str, rule: &Rule) -> String {
    let body = format!("{kind}/{}/{}/", rule.pattern, rule.replacement);
    match rule.comment.as_deref().map(str::trim) {
        Some(comment) if !comment.is_empty() => format!("{body}  # {comment}"),
        _ => body,
    }
}

/// 从声母表**算**出「多字母声母简拼」：`zh/ch/sh` → `abbrev/^([zcs]h).+$/$1/`。
///
/// 做法：把多字母声母按「去掉首字母后的尾串」分组，同组的首字合成字符类。
/// 顺序跟着数据文件走（zh, ch, sh），不重排 —— 历史产出的 `[zcs]` 就是这个顺序。
fn multi_initial_abbrev(initials: &[String]) -> Vec<String> {
    let mut groups: Vec<(String, Vec<char>)> = Vec::new();
    for initial in initials {
        let mut chars = initial.chars();
        let Some(first) = chars.next() else {
            continue;
        };
        let tail: String = chars.collect();
        if tail.is_empty() {
            continue; // 单字母声母不参与（它已被首字母简拼覆盖）
        }
        match groups.iter_mut().find(|(t, _)| *t == tail) {
            Some(group) => group.1.push(first),
            None => groups.push((tail, vec![first])),
        }
    }
    groups
        .into_iter()
        .map(|(tail, firsts)| {
            let class: String = firsts.iter().collect();
            let names: Vec<String> = firsts.iter().map(|c| format!("{c}{tail}")).collect();
            format!(
                "abbrev/^([{class}]{tail}).+$/$1/  # 简拼（{}）",
                names.join(", ")
            )
        })
        .collect()
}

/// `speller/algebra`：简拼 → 多字母声母简拼 → 拼写变体 / 容错。
fn build_algebra(spec: &Spec) -> Vec<String> {
    let mut lines: Vec<String> = spec.abbrev.iter().map(|r| rule_line("abbrev", r)).collect();
    lines.extend(multi_initial_abbrev(&spec.initials));
    lines.extend(spec.derive.iter().map(|r| rule_line("derive", r)));
    lines
}

/// `translator/preedit_format`：按数据顺序逐条 `xform`。
fn build_preedit(spec: &Spec) -> Vec<String> {
    spec.preedit.iter().map(|r| rule_line("xform", r)).collect()
}

/// 把规则列表渲染成块内容（`key:` 一行 + `    - …` 若干行），缩进 2 空格
/// （方案里 `speller:` / `translator:` 的子节点就是这个层级）。
fn block(key: &str, rules: &[String]) -> String {
    let mut out = format!("  {key}:\n");
    for rule in rules {
        out.push_str("    - ");
        out.push_str(rule);
        out.push('\n');
    }
    out
}

/// 按顺序把第 n 个标记块替换成第 n 个 `blocks` 内容。
fn replace_blocks(text: &str, blocks: &[String]) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.contains(END_MARK) {
            ends.push(index);
        } else if line.contains(START_MARK) {
            starts.push(index);
        }
    }
    if starts.is_empty() {
        return Err(format!(
            "方案里没有 `{START_MARK}` 标记块，无从下手。先在要生成的位置加上标记。"
        ));
    }
    if starts.len() != ends.len() {
        return Err(format!(
            "标记块不成对：{START_MARK} × {}，{END_MARK} × {}",
            starts.len(),
            ends.len()
        ));
    }
    if starts.len() != blocks.len() {
        return Err(format!(
            "标记块 {} 个，但生成了 {} 段内容 —— 方案与生成器不同步",
            starts.len(),
            blocks.len()
        ));
    }

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut cursor = 0usize;
    for (index, start) in starts.iter().enumerate() {
        let end = ends[index];
        if end <= *start {
            return Err(format!("第 {} 个标记块的起止顺序不对", index + 1));
        }
        out.extend(lines[cursor..=*start].iter().map(|l| l.to_string()));
        // 块内容自身以 `\n` 结尾，去掉最后一个空行
        let mut body = blocks[index].clone();
        while body.ends_with('\n') {
            body.pop();
        }
        out.extend(body.lines().map(|l| l.to_string()));
        out.push(lines[end].to_string());
        cursor = end + 1;
    }
    out.extend(lines[cursor..].iter().map(|l| l.to_string()));

    let newline = if text.contains('\r') { "\r\n" } else { "\n" };
    let mut joined = out.join(newline);
    if text.ends_with('\n') {
        joined.push_str(newline);
    }
    Ok(joined)
}

/// 找到"数据根目录"（含 `Rime-Next\` 的那个）。
///
/// ⚠️ **不能用 `env!("CARGO_MANIFEST_DIR")`**：那是编译期常量，装到别的机器上会指向
/// 开发机的路径（不存在），程序直接失效。所以用运行时路径：从 exe 往上找。
fn install_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    for dir in exe.ancestors().skip(1) {
        if dir.join("data").is_dir() || dir.join("Rime-Next").is_dir() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// 开发期兜底：`<repo>`（style 的上一级）。
fn source_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn main() {
    let root = install_root().unwrap_or_else(source_root);
    let data = weasel_common::rime_layout::data_dir(&root);
    let source = data.join("parser").join("pinyin").join("pinyin.parser.yaml");
    // 2026-10-28 布局：全拼方案 id 是 `pinyin.luna`，文件在
    // `schema/pinyin/luna/pinyin.luna.schema.yaml`（旧路径 `schema/pinyin/pinyin.schema.yaml` 已不存在）。
    let target = data
        .join("schema")
        .join("pinyin")
        .join("luna")
        .join("pinyin.luna.schema.yaml");

    let text = match std::fs::read_to_string(&source) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("✗ 读不到拼写数据 {}: {e}", source.display());
            std::process::exit(1);
        }
    };
    // 用完整 YAML 解析：`- pattern: …` 这种"序列套映射"极简解析器认不了。
    let spec: Spec = match serde_norway::from_str(&text) {
        Ok(spec) => spec,
        Err(e) => {
            eprintln!("✗ 解析 {} 失败: {e}", source.display());
            std::process::exit(1);
        }
    };

    let schema_text = match std::fs::read_to_string(&target) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("✗ 读不到方案 {}: {e}", target.display());
            std::process::exit(1);
        }
    };
    // 块顺序 = 方案里标记块的出现顺序：先 algebra，后 preedit_format。
    let blocks = [
        block("algebra", &build_algebra(&spec)),
        block("preedit_format", &build_preedit(&spec)),
    ];
    let updated = match replace_blocks(&schema_text, &blocks) {
        Ok(updated) => updated,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    // 标准（2026-10-28）：`schema/description` 一律块标量 `|`。手写的方案可能还是
    // 老写法（普通多行标量），顺手规范化 —— 幂等，已是 `|` 就原样返回。
    let updated = match weasel_style::schema_standard::normalize(&updated) {
        Ok(updated) => updated,
        Err(e) => {
            eprintln!("✗ {} 的 description 不合标准: {e}", target.display());
            std::process::exit(1);
        }
    };
    if updated == schema_text {
        println!("✓ {} 已是最新，无改动", target.display());
        return;
    }
    match std::fs::write(&target, &updated) {
        Ok(()) => println!("✓ 已按 {} 重新生成 {}", source.display(), target.display()),
        Err(e) => {
            eprintln!("✗ 写回 {} 失败: {e}", target.display());
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, replacement: &str) -> Rule {
        Rule {
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            comment: None,
        }
    }

    #[test]
    fn multi_initial_merges_into_one_char_class() {
        let initials = ["b", "p", "zh", "ch", "sh"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let out = multi_initial_abbrev(&initials);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], "abbrev/^([zcs]h).+$/$1/  # 简拼（zh, ch, sh）");
    }

    #[test]
    fn single_letter_initials_yield_nothing() {
        let initials = ["b", "p", "m"].iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(multi_initial_abbrev(&initials).is_empty());
    }

    #[test]
    fn block_renders_two_space_indent() {
        let rendered = block("algebra", &["abbrev/^([a-z]).+$/$1/".to_string()]);
        assert_eq!(rendered, "  algebra:\n    - abbrev/^([a-z]).+$/$1/\n");
    }

    #[test]
    fn replace_blocks_keeps_surrounding_text() {
        let text = "---\nspeller:\n  alphabet: abc\n  # >>> generated <<<\n  algebra:\n    - OLD\n  # >>> end generated <<<\n###\n";
        let out = replace_blocks(text, &[block("algebra", &["NEW".to_string()])]).unwrap();
        assert!(out.contains("alphabet: abc"));
        assert!(out.contains("    - NEW"));
        assert!(!out.contains("OLD"));
        assert!(out.ends_with("###\n"));
    }

    #[test]
    fn replace_blocks_errors_when_marks_unpaired() {
        let text = "# >>> generated <<<\n  algebra:\n";
        assert!(replace_blocks(text, &["x".to_string()]).is_err());
    }

    #[test]
    fn replace_blocks_errors_when_no_mark() {
        assert!(replace_blocks("speller:\n  alphabet: abc\n", &["x".to_string()]).is_err());
    }
}
