//! `schema/description` 的**强制标准**（2026-10-28 定）。
//!
//! 方案文件的 `schema/description` **一律用 YAML 块标量 `|`** 写：
//!
//! ```yaml
//! schema:
//!   description: |
//!     小鹤双拼·简体（明月拼音 + 双拼键位）。
//! ```
//!
//! 为什么强制：
//! * **所见即所得** —— `|` 保留换行、不做折叠，方案里的排版就是界面上看到的排版；
//!   普通标量 / 折叠标量 `>` 会把换行 silently 变成空格，多行说明写进去就没了。
//! * **不用转义** —— 中文冒号、括号、`#`、`：` 这些在普通标量里会撞 YAML 语法，
//!   在块里全是普通字符，写什么是什么。
//! * **可追加** —— 以后想在描述里加键位表、词库来源，直接加缩进行即可，不用改语法。
//!
//! 三种入口：
//! * [`render_description`] —— 生成器写文件用；
//! * [`normalize`] —— 把既有的非块写法（`>` / 引号 / 普通多行）改成 `|`；
//! * [`check`] —— 测试用，不合标准就报错（`style/tests/schema_syntax.rs`）。

/// `schema:` 映射里 description 键的缩进（方案顶层映射下的标准层级）。
const KEY_INDENT: &str = "  ";
/// 块内容的缩进（比键多两级）。
const BODY_INDENT: &str = "    ";

/// 渲染标准形态的 `description`（含结尾换行）。
///
/// 多行文本按行输出，逐行 `trim_end`（行尾空格无意义且易触发 diff 噪音）。
pub fn render_description(text: &str) -> String {
    let mut out = format!("{KEY_INDENT}description: |\n");
    for line in text.trim_end().lines() {
        out.push_str(BODY_INDENT);
        out.push_str(line.trim_end());
        out.push('\n');
    }
    if text.trim_end().is_empty() {
        // 空描述也留一个占位行，避免 `description: |` 后面直接接下一个键
        out.push_str(BODY_INDENT);
        out.push('\n');
    }
    out
}

/// 定位 `schema:` 映射里的 `description:` 行，返回 `(行号, 原始行)`。
fn find_description(lines: &[&str]) -> Option<(usize, String)> {
    // 先找到顶层 `schema:`（列 0），再在它下面（缩进 > 0）找 `description:`。
    let schema_at = lines.iter().position(|line| line.trim_end() == "schema:")?;
    for (offset, line) in lines.iter().enumerate().skip(schema_at + 1) {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // 回到列 0（下一个顶层键，或 `###` 分节符）→ 已离开 schema 映射
        if !line.starts_with(' ') {
            break;
        }
        // 只认 `schema:` 的**直接**子键（缩进正好 2 空格）：更深处的
        // `description`（如 switch 里每条开关的）不参与。
        if line.starts_with("  description:") && !line.starts_with("   ") {
            let value = trimmed["  description:".len()..].trim_start().to_string();
            return Some((offset, value));
        }
    }
    None
}

/// 块标量指示符（`|` / `>` + 可选的 `-` / `+`）。
fn block_indicator(value: &str) -> Option<&str> {
    let head = value.trim_end();
    if head.starts_with('|') || head.starts_with('>') {
        Some(head)
    } else {
        None
    }
}

/// 剥掉一对匹配的引号（YAML 单/双引号标量）。
fn unquote(value: &str) -> String {
    let value = value.trim();
    for (open, close) in [('"', '"'), ('\'', '\'')] {
        if value.len() >= 2 && value.starts_with(open) && value.ends_with(close) {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

/// 收集「键之后、缩进更深的那些行」——块内容或普通标量的续行。
fn continuation_lines(lines: &[&str], from: usize, key_indent: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate().skip(from) {
        if line.trim().is_empty() {
            // 块内的空行算内容（保留段落间距），但末尾的连续空行不算
            out.push(index);
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent > key_indent {
            out.push(index);
        } else {
            break;
        }
    }
    while out.last().is_some_and(|i| lines[*i].trim().is_empty()) {
        out.pop();
    }
    out
}

/// 检查是否符合标准：`schema:` 下**必须有** `description`，且**必须是** `|` 块。
pub fn check(text: &str) -> Result<(), String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some((_, value)) = find_description(&lines) else {
        return Err("`schema:` 下缺少 `description`".to_string());
    };
    match block_indicator(&value) {
        Some(indicator) if indicator == "|" => Ok(()),
        Some(other) => Err(format!(
            "`description` 用了 `{other}`，标准只认块标量 `|`（保留换行、不折叠）"
        )),
        None => Err(format!(
            "`description` 不是块标量：写了 `{value}`，应为 `|` + 缩进的内容行"
        )),
    }
}

/// 把 `description` 规范化成标准形态；已经是 `|` 就原样返回。
///
/// 只认 `schema:` 映射下的那一个，其它地方的 `description` 不动。
pub fn normalize(text: &str) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some((index, value)) = find_description(&lines) else {
        return Err("`schema:` 下缺少 `description`，无处下手".to_string());
    };
    let key_indent = KEY_INDENT.len();

    // 已是标准形态 → 不动（保持手写排版，生成器不折腾文件）
    if block_indicator(&value) == Some("|") {
        return Ok(text.to_string());
    }

    // 取出原值：块内容 / 续行 / 同行标量。
    let body: Vec<&str> = if block_indicator(&value).is_some() {
        continuation_lines(&lines, index + 1, key_indent)
            .iter()
            .map(|i| lines[*i])
            .collect()
    } else if value.is_empty() {
        continuation_lines(&lines, index + 1, key_indent)
            .iter()
            .map(|i| lines[*i])
            .collect()
    } else {
        vec![value.as_str()]
    };

    // 折叠语义：`>`/普通多行标量把换行变空格；`|` 保留换行。
    let folded = block_indicator(&value).is_none_or(|i| i.starts_with('>'));
    let text_value = if folded {
        body.iter()
            .map(|line| unquote(line.trim()))
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        body.iter()
            .map(|line| unquote(line.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // 用标准块替换掉 `description:` 那一行 + 它吃掉的内容行。
    let consumed = if value.is_empty() || block_indicator(&value).is_some() {
        1 + body.len()
    } else {
        1
    };
    let mut out: Vec<String> = lines[..index].iter().map(|l| l.to_string()).collect();
    out.push("  description: |".to_string());
    if text_value.is_empty() {
        out.push("    ".to_string());
    } else {
        for line in text_value.lines() {
            out.push(format!("    {line}"));
        }
    }
    out.extend(lines[index + consumed..].iter().map(|l| l.to_string()));

    let newline = if text.contains('\r') { "\r\n" } else { "\n" };
    let mut joined = out.join(newline);
    if text.ends_with('\n') {
        joined.push_str(newline);
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_is_block_scalar() {
        assert_eq!(render_description("明月拼音。"), "  description: |\n    明月拼音。\n");
        assert_eq!(
            render_description("第一行。\n第二行。"),
            "  description: |\n    第一行。\n    第二行。\n"
        );
    }

    #[test]
    fn check_accepts_block_literal() {
        let text = "---\nschema:\n  id: x\n  description: |\n    明月拼音。\n###\n";
        assert!(check(text).is_ok());
    }

    #[test]
    fn check_rejects_folded_and_plain() {
        assert!(check("schema:\n  description: >\n    a\n").is_err());
        assert!(check("schema:\n  description:\n    明月拼音。\n").is_err());
        assert!(check("schema:\n  description: \"明月拼音。\"\n").is_err());
        assert!(check("schema:\n  id: x\n").is_err());
    }

    #[test]
    fn normalize_rewrites_plain_multiline() {
        let text = "---\nschema:\n  id: pinyin\n  description:\n    Rime-Next 的全拼方案。\n  dependency:\n    - stroke\n";
        let out = normalize(text).unwrap();
        assert_eq!(
            out,
            "---\nschema:\n  id: pinyin\n  description: |\n    Rime-Next 的全拼方案。\n  dependency:\n    - stroke\n"
        );
        assert!(check(&out).is_ok());
    }

    #[test]
    fn normalize_folds_folded_scalar_into_one_line() {
        let text = "schema:\n  description: >\n    第一行。\n    第二行。\n  id: x\n";
        let out = normalize(text).unwrap();
        assert_eq!(out, "schema:\n  description: |\n    第一行。 第二行。\n  id: x\n");
    }

    #[test]
    fn normalize_is_idempotent() {
        let text = "schema:\n  description: |\n    明月拼音。\n";
        assert_eq!(normalize(text).unwrap(), text);
    }

    #[test]
    fn normalize_strips_quotes() {
        let text = "schema:\n  description: '明月拼音'\n";
        assert_eq!(normalize(text).unwrap(), "schema:\n  description: |\n    明月拼音\n");
    }

    #[test]
    fn only_touches_schema_description() {
        // 别处的 `description`（比如 switch 的）不该被动
        let text = "schema:\n  description: |\n    明月拼音。\nswitch:\n  - name: x\n    description: |\n      开关说明\n";
        assert_eq!(normalize(text).unwrap(), text);
    }
}
