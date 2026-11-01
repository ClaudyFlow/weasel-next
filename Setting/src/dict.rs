//! 词库读写（对应蓝宝石 `DictStore` 的最小可用子集）。
//!
//! 只做四件事，够设置页的「词库」页用：
//! * [`list_dicts`] 列出用户目录根部的 `*.dict.yaml`
//! * [`find_entries`] 按编码查候选（行序即名次）
//! * [`add_entry`] 加词（插到同编码末尾；重复直接拒绝）
//! * [`remove_entry`] 删词（从后往前删，避免行号错位）
//!
//! 设计约束（学自它的 `extraLineCount` 思路）：**只改目标词条行**。
//! 文件头的 `--- ... ...`、注释行、其它条目一律原样写回。
//!
//! Rime 词典格式：可选文件头 `---` … `...`，之后每行 `词<TAB>编码[<TAB>权重]`。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 一个用户词典文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictFile {
    /// 文件名（含 `.dict.yaml` 后缀）。
    pub name: String,
    /// 完整路径。
    pub path: PathBuf,
    /// 词条数量（解析后统计，用于界面显示）。
    pub entries: usize,
}

/// 一个词条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub word: String,
    pub code: String,
    /// 权重列（可能为空）。
    pub weight: String,
    /// 0 基行号（写回时用）。
    pub line: usize,
}

/// 列出用户目录根部的 `*.dict.yaml`（按文件名排序）。
pub fn list_dicts(user: &Path) -> io::Result<Vec<DictFile>> {
    let mut out = Vec::new();
    let read = match fs::read_dir(user) {
        Ok(read) => read,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(error) => return Err(error),
    };
    for entry in read {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        if !name.ends_with(".dict.yaml") {
            continue;
        }
        let entries = fs::read_to_string(&path)
            .map(|text| parse_entries(&text).len())
            .unwrap_or(0);
        out.push(DictFile {
            name,
            path,
            entries,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// 解析词典正文（跳过文件头、注释、空行）。
pub fn parse_entries(text: &str) -> Vec<Entry> {
    let lines: Vec<&str> = text.lines().collect();
    let has_header = lines.iter().any(|line| line.trim() == "---");
    let mut entries = Vec::new();
    let mut in_header = false;
    let mut header_done = !has_header;
    for (index, line) in lines.iter().enumerate() {
        if !header_done {
            let trimmed = line.trim();
            if trimmed == "---" {
                if in_header {
                    header_done = true;
                } else {
                    in_header = true;
                }
                continue;
            }
            if in_header && trimmed == "..." {
                header_done = true;
            }
            continue;
        }
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let mut parts = line.split('\t');
        let Some(word) = parts.next() else { continue };
        let Some(code) = parts.next() else { continue };
        if word.is_empty() || code.is_empty() {
            continue;
        }
        entries.push(Entry {
            word: word.to_string(),
            code: code.to_string(),
            weight: parts.next().unwrap_or_default().to_string(),
            line: index,
        });
    }
    entries
}

/// 按编码查候选；`prefix` 为真时按前缀匹配。返回的 `(名次, 词条)` 名次从 1 开始。
pub fn find_entries(text: &str, code: &str, prefix: bool) -> Vec<(usize, Entry)> {
    let needle = code.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut rank = 0usize;
    let mut out = Vec::new();
    for entry in parse_entries(text) {
        if !entry.code.to_lowercase().starts_with(&needle) {
            continue;
        }
        if !prefix && entry.code.to_lowercase() != needle {
            continue;
        }
        rank += 1;
        out.push((rank, entry));
    }
    out
}

/// 加词：插到同编码最后一条之后；该编码不存在时追加到文件末尾。
///
/// 返回给人看的结果说明。重复（同词同码）直接拒绝、不改文件。
pub fn add_entry(text: &str, word: &str, code: &str) -> Result<(String, String), String> {
    let word = word.trim();
    let code_raw = code.trim();
    if !code_raw.chars().all(|c| c.is_ascii_lowercase()) {
        return Err("编码只能是小写字母".into());
    }
    let code = code_raw.to_string();
    if word.is_empty() {
        return Err("词不能为空".into());
    }
    if code.is_empty() {
        return Err("编码不能为空".into());
    }
    if !code.chars().all(|c| c.is_ascii_lowercase()) {
        return Err("编码只能是小写字母".into());
    }
    let lines: Vec<String> = text.lines().map(|line| line.to_string()).collect();
    let entries = parse_entries(text);
    if let Some(same) = entries
        .iter()
        .find(|entry| entry.word == word && entry.code.eq_ignore_ascii_case(&code))
    {
        return Err(format!(
            "已存在：{word} → {code}（第 {} 行），未改动",
            same.line + 1
        ));
    }
    let same_code: Vec<&Entry> = entries.iter().filter(|e| e.code.eq_ignore_ascii_case(&code)).collect();
    let note = if same_code.is_empty() {
        format!("该编码此前没有词，{word} 成为首选")
    } else {
        format!(
            "该编码已有 {} 个词（首个是「{}」），{word} 追加到末尾",
            same_code.len(),
            same_code[0].word
        )
    };
    let insert_at = same_code
        .iter()
        .map(|entry| entry.line)
        .max()
        .map(|line| line + 1)
        .unwrap_or(lines.len());
    let mut out = lines;
    out.insert(insert_at, format!("{word}\t{code}"));
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    Ok((joined, format!("{note}（写入第 {} 行）", insert_at + 1)))
}

/// 删词：删掉所有同名词条，返回 (新内容, 删除条数)。
pub fn remove_entry(text: &str, word: &str) -> (String, usize) {
    let word = word.trim();
    let lines: Vec<String> = text.lines().map(|line| line.to_string()).collect();
    let hits: Vec<usize> = parse_entries(text)
        .into_iter()
        .filter(|entry| entry.word == word)
        .map(|entry| entry.line)
        .collect();
    if hits.is_empty() {
        return (text.to_string(), 0);
    }
    let mut out = lines;
    // 从后往前删，行号才不会错位。
    for line in hits.iter().rev() {
        if *line < out.len() {
            out.remove(*line);
        }
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    (joined, hits.len())
}

/// 把词典备份到 `<user>/backup/dicts/<时间戳>-<文件名>`，返回备份路径。
pub fn backup_dict(user: &Path, path: &Path) -> io::Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "dict.yaml".to_string());
    let dir = user.join("backup").join("dicts");
    fs::create_dir_all(&dir)?;
    let target = dir.join(format!("{stamp}-{name}"));
    fs::copy(path, &target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\nname: test\n...\n今天\ttj\n明天\ttm\n测试\tcs\n";

    #[test]
    fn parses_body_after_header() {
        let entries = parse_entries(SAMPLE);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].word, "今天");
        assert_eq!(entries[0].code, "tj");
        assert_eq!(entries[2].code, "cs");
    }

    #[test]
    fn parses_file_without_header() {
        let entries = parse_entries("你好\tni\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].word, "你好");
    }

    #[test]
    fn find_respects_prefix_flag() {
        let exact = find_entries(SAMPLE, "cs", false);
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].1.word, "测试");
        let prefix = find_entries(SAMPLE, "t", true);
        assert_eq!(prefix.len(), 2, "tj 与 tm 都以前缀 t 开头");
        assert_eq!(prefix[0].0, 1, "名次从 1 开始");
    }

    #[test]
    fn add_appends_after_same_code() {
        let (text, note) = add_entry(SAMPLE, "词库工具", "cigj").unwrap();
        assert!(note.contains("此前没有词"));
        assert!(text.ends_with("测试\tcs\n词库工具\tcigj\n"), "应追加到末尾：{text}");
    }

    #[test]
    fn add_inserts_after_existing_code_group() {
        let (text, note) = add_entry(SAMPLE, "天津", "tj").unwrap();
        assert!(note.contains("已有 1 个词"));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[3], "今天\ttj");
        assert_eq!(lines[4], "天津\ttj", "应插在同编码之后");
    }

    #[test]
    fn add_rejects_duplicate() {
        let error = add_entry(SAMPLE, "今天", "tj").unwrap_err();
        assert!(error.contains("已存在"));
    }

    #[test]
    fn add_rejects_bad_code() {
        assert!(add_entry(SAMPLE, "词", "Ci").unwrap_err().contains("小写"));
        assert!(add_entry(SAMPLE, "", "ci").unwrap_err().contains("不能为空"));
    }

    #[test]
    fn remove_deletes_all_matching_words() {
        let (text, removed) = remove_entry(SAMPLE, "明天");
        assert_eq!(removed, 1);
        assert!(!text.contains("明天"));
        assert!(text.contains("今天") && text.contains("测试"));
    }

    #[test]
    fn remove_missing_is_noop() {
        let (text, removed) = remove_entry(SAMPLE, "不存在");
        assert_eq!(removed, 0);
        assert_eq!(text, SAMPLE);
    }

    #[test]
    fn header_and_comments_are_preserved() {
        let input = "---\nname: t\n# 注释\n...\n今天\ttj\n";
        let (text, _) = add_entry(input, "明天", "tm").unwrap();
        assert!(text.starts_with("---\nname: t\n# 注释\n...\n"), "头部必须原样保留");
    }
}
