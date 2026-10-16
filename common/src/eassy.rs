//! 细胞词库（`*.eassy.toml`）的读取与归并。
//!
//! 约定（与用户目录分类一致）：
//! - `eassy/<名字>/<名字>.eassy.toml`（首选）或 `dict/<名字>/<名字>.eassy.toml`（兼容旧位置）。
//! - 也接受直接放在根目录的 `*.eassy.toml`。
//! - 文件格式为 TOML 的 `[words]` 表：键是词，值是频次。
//!
//! ```toml
//! [words]
//! "〇" = 981
//! "〇一" = 488
//! ```
//!
//! 归并规则：**同词频次求和**。输出 librime 需要的 `essay.txt`（每行 `词<TAB>频次`）。
//!
//! librime 只认 `essay.txt`（`vocabulary` 默认名就是 `essay`），所以 TOML 是**源**、
//! `essay.txt` 是**部署产物**：仓库里只放 `.eassy.toml`，部署时生成 txt。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 词 → 频次（归并后的结果）。
pub type Vocabulary = BTreeMap<String, u64>;

/// 从一份 `*.eassy.toml` 文本解析出词表。
///
/// 只读取 `words:` 表，其余键忽略；值必须为正整数，非正整数条目跳过（不视为错误）。
///
/// 注意：YAML 允许重复键（后者覆盖），而旧的 TOML 形态禁止 —— 所以下面那条"宽容解析"
/// 分支基本不会再走到（只在 YAML 本身语法错误时兜底）。词库里原本有 5 处重复词
/// （軟麪筋 / 鋪 / 𪡏 / 𪢮 / 𪻐），改成 YAML 后这些词的频次取**最后一条**而不是求和，
/// 对 37 万条词库的整体排序没有实际影响。
pub fn parse(text: &str) -> Result<Vocabulary, toml::de::Error> {
    match toml::from_str::<toml::Value>(text) {
        Ok(value) => {
            let mut out = Vocabulary::new();
            if let Some(words) = value.get("words").and_then(|v| v.as_table()) {
                for (word, entry) in words {
                    if let Some(freq) = entry.as_integer() {
                        if freq > 0 {
                            *out.entry(word.clone()).or_insert(0) += freq as u64;
                        }
                    }
                }
            }
            Ok(out)
        }
        // YAML 语法错误时退回逐行宽容解析（同词频次相加 ✓），保证一份坏文件不会让词库作废
        Err(cause) => Ok(parse_lenient(text, cause)),
    }
}

/// 宽容解析：逐行认 `"词": 频次`（也认旧的 `"词" = 频次`），重复词频次**相加**；
/// 注释与其它键忽略。
fn parse_lenient(text: &str, _cause: toml::de::Error) -> Vocabulary {
    let mut out = Vocabulary::new();
    let mut in_words = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_words = line.contains("words");
            continue;
        }
        if !in_words {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let word = key.trim().trim_matches('"').trim_matches('\'').to_string();
        if word.is_empty() {
            continue;
        }
        let digits: String = value
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            continue;
        }
        if let Ok(freq) = digits.parse::<u64>() {
            if freq > 0 {
                *out.entry(word).or_insert(0) += freq;
            }
        }
    }
    out
}

/// 读取一份 `*.eassy.toml` 文件。
pub fn read_file(path: &Path) -> io::Result<Vocabulary> {
    let text = fs::read_to_string(path)?;
    parse(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

/// 把 `dir` 本身与 `dir/*/` 下的 `*.eassy.toml` 收进 `files`。
fn collect_into(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            for inner in fs::read_dir(&path)? {
                let candidate = inner?.path();
                if is_eassy(&candidate) {
                    files.push(candidate);
                }
            }
        } else if is_eassy(&path) {
            files.push(path);
        }
    }
    Ok(())
}

/// 扫描一份根下的全部 `*.eassy.toml` 并按词归并求和。
///
/// 扫描位置：`<root>/eassy/*/`、`<root>/eassy/`、`<root>/dict/*/`、`<root>/dict/`、`<root>/`。
/// 目录不存在时跳过（不报错）。
pub fn collect(root: &Path) -> io::Result<Vocabulary> {
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in [root.join("eassy"), root.join("dict"), root.to_path_buf()] {
        collect_into(&dir, &mut files)?;
    }
    files.sort();
    files.dedup();

    let mut merged = Vocabulary::new();
    for file in files {
        let words = read_file(&file)?;
        for (word, freq) in words {
            *merged.entry(word).or_insert(0) += freq;
        }
    }
    Ok(merged)
}

fn is_eassy(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .map(|n| n.to_string_lossy().ends_with(".eassy.toml"))
            .unwrap_or(false)
}

/// 写出 librime 需要的 `essay.txt`：每行 `词<TAB>频次`，按频次降序（同频次按词升序，保证确定性）。
///
/// 返回写出的条目数。
/// 把词表渲染成 ssay.txt 的完整字节（按词频降序，同频按词序）。
///
/// 单独抽出来是为了**先比对再落盘**：librime 只按 mtime 决定重编词典，
/// 内容没变却重写文件会让所有词典白重建一遍（分钟级）。
pub fn render_essay_txt(vocabulary: &Vocabulary) -> Vec<u8> {
    let mut entries: Vec<(&String, &u64)> = vocabulary.iter().collect();
    entries.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let mut out = Vec::new();
    for (word, freq) in &entries {
        out.extend_from_slice(word.as_bytes());
        out.push(b'\t');
        out.extend_from_slice(freq.to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

/// 把已经渲染好的内容写到 \path\（必要时建父目录）。
pub fn write_essay_bytes(path: &Path, rendered: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, rendered)
}

/// 写 \ssay.txt\；返回写入的词条数。
pub fn write_essay_txt(path: &Path, vocabulary: &Vocabulary) -> io::Result<usize> {
    write_essay_bytes(path, &render_essay_txt(vocabulary))?;
    Ok(vocabulary.len())
}

/// 一步到位：从 `source_root` 收集并归并，写出 `essay.txt` 到 `target_root/essay.txt`。
///
/// 词表为空时不写文件（避免把已有的 txt 清空）。
pub fn deploy(source_root: &Path, target_root: &Path) -> io::Result<Option<(PathBuf, usize)>> {
    let vocabulary = collect(source_root)?;
    if vocabulary.is_empty() {
        return Ok(None);
    }
    let target = target_root.join("essay.txt");
    let count = write_essay_txt(&target, &vocabulary)?;
    Ok(Some((target, count)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weasel-eassy-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }


    /// 重复键不再让整份词库作废：宽容解析按同词求和。
    #[test]
    fn tolerates_duplicate_keys() {
        let text = "[words]\n\"好\" = 3\n\"好\" = 4\n\"你\" = 1\n";
        let v = parse(text).expect("重复键不应报错");
        assert_eq!(v.get("好"), Some(&7), "重复键应求和");
        assert_eq!(v.get("你"), Some(&1));
        assert_eq!(v.len(), 2);
    }
    #[test]
    fn merges_same_word_by_summing_frequencies() {
        let a = parse("[words]\n\"好\" = 3\n\"你好\" = 5\n").unwrap();
        let b = parse("[words]\n\"好\" = 4\n\"世界\" = 1\n").unwrap();
        let mut merged = a;
        for (w, f) in b {
            *merged.entry(w).or_insert(0) += f;
        }
        assert_eq!(merged.get("好"), Some(&7));
        assert_eq!(merged.get("你好"), Some(&5));
        assert_eq!(merged.get("世界"), Some(&1));
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn ignores_other_tables_and_non_positive_values() {
        let v = parse("[meta]\nname = \"x\"\n\n[words]\n\"甲\" = 2\n\"乙\" = \"x\"\n\"丙\" = -1\n").unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v.get("甲"), Some(&2));
    }

    #[test]
    fn writes_tab_separated_sorted_by_frequency_desc() {
        let dir = temp_dir("write");
        fs::create_dir_all(dir.join("eassy/default")).unwrap();
        fs::write(
            dir.join("eassy/default/default.eassy.toml"),
            "[words]\n\"少\" = 1\n\"多\" = 9\n",
        )
        .unwrap();
        fs::create_dir_all(dir.join("dict/extra")).unwrap();
        fs::write(
            dir.join("dict/extra/extra.eassy.toml"),
            "[words]\n\"多\" = 1\n",
        )
        .unwrap();

        let (_, count) = deploy(&dir, &dir).unwrap().expect("应生成 essay.txt");
        assert_eq!(count, 2);
        let text = fs::read_to_string(dir.join("essay.txt")).unwrap();
        assert_eq!(text, "多\t10\n少\t1\n");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 两种布局（eassy/ 与 dict/）都要被扫到。
    #[test]
    fn collects_from_both_eassy_and_dict_dirs() {
        let dir = temp_dir("both");
        fs::create_dir_all(dir.join("eassy/one")).unwrap();
        fs::create_dir_all(dir.join("dict/two")).unwrap();
        fs::write(dir.join("eassy/one/one.eassy.toml"), "[words]\n\"甲\" = 1\n").unwrap();
        fs::write(dir.join("dict/two/two.eassy.toml"), "[words]\n\"乙\" = 2\n").unwrap();
        let v = collect(&dir).unwrap();
        assert_eq!(v.get("甲"), Some(&1));
        assert_eq!(v.get("乙"), Some(&2));
        let _ = fs::remove_dir_all(&dir);
    }
}
