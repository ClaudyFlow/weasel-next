//! 部署记录（`user.toml`）：记录**各文件夹**上次部署的时间与哈希。
//!
//! 两份，格式不同、职责不同：
//!
//! * `build\user.toml` —— 部署时写入。每个文件夹一段，含 `time` 与 `hash`；
//!   **没有新变动就不动时间**（把上次那份原样带过来）。
//! * `<用户文件夹>\user.toml` —— 由**每十分钟**一次的检测维护。每个文件夹一行，
//!   记录成 `旧哈希 -> 时间 新哈希`：检测到变动就把新时间和新哈希**追加**在箭头后面，
//!   保留上次的哈希，便于回溯。
//!
//! 部署本身**不再做校验**：判断"要不要部署"由这份检测记录承担，部署只管执行。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 一个文件夹在某次部署时的记录。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderStamp {
    /// 上次实质变动的时间（没有变动时保持不变）。
    pub time: String,
    /// 该文件夹内容的哈希。
    pub hash: String,
}

/// `build\user.toml` 的位置。
pub fn build_stamp_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join("build").join("user.toml")
}

/// 用户文件夹根目录那份 `user.toml` 的位置。
pub fn watch_stamp_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join("user.toml")
}

/// 解析 `build\user.toml`：`[文件夹名]` 段，含 `time` / `hash`。
pub fn parse_stamps(text: &str) -> BTreeMap<String, FolderStamp> {
    let mut out: BTreeMap<String, FolderStamp> = BTreeMap::new();
    let Ok(value) = toml::from_str::<toml::Value>(text) else {
        return out;
    };
    let Some(table) = value.as_table() else {
        return out;
    };
    for (name, entry) in table {
        // last_deploy 之类的顶层标量跳过，只认"段"。
        let Some(section) = entry.as_table() else {
            continue;
        };
        let mut stamp = FolderStamp::default();
        if let Some(time) = section.get("time").and_then(|v| v.as_str()) {
            stamp.time = time.to_string();
        }
        if let Some(hash) = section.get("hash").and_then(|v| v.as_str()) {
            stamp.hash = hash.to_string();
        }
        out.insert(name.clone(), stamp);
    }
    out
}

/// 写出 `build\user.toml`（按名字 a–z 排序，稳定可 diff）。
///
/// `previous` 是上一次的记录：**哈希没变就不动时间**（沿用上次的 `time`），
/// 变了才用 `now`。
pub fn write_stamps(
    path: &Path,
    current: &BTreeMap<String, String>,
    previous: &BTreeMap<String, FolderStamp>,
    now: &str,
) -> std::io::Result<BTreeMap<String, FolderStamp>> {
    let mut merged: BTreeMap<String, FolderStamp> = BTreeMap::new();
    let mut out = String::from(
        "# user.toml —— 各文件夹上次部署的时间与哈希（自动生成，勿手改）。\n\
         # 哈希没有变化时时间保持不变；部署不再做校验，判断由根目录那份 user.toml 的定期检测承担。\n\n",
    );
    out.push_str(&format!("last_deploy = \"{now}\"\n"));
    for (name, hash) in current {
        let unchanged = previous.get(name).is_some_and(|old| &old.hash == hash);
        let time = match (unchanged, previous.get(name)) {
            (true, Some(old)) => old.time.clone(),
            _ => now.to_string(),
        };
        out.push_str(&format!("\n[{name}]\ntime = \"{time}\"\nhash = \"{hash}\"\n"));
        merged.insert(name.clone(), FolderStamp { time, hash: hash.clone() });
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, out)?;
    Ok(merged)
}

/// 一次检测得到的变动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub name: String,
    /// 上一次的哈希（没有记录时为空）。
    pub old_hash: String,
    pub new_hash: String,
    pub time: String,
}

/// 比较两次记录，挑出**哈希变了**的文件夹。
pub fn detect_changes(
    previous: &BTreeMap<String, FolderStamp>,
    current: &BTreeMap<String, String>,
    now: &str,
) -> Vec<Detected> {
    let mut out = Vec::new();
    for (name, hash) in current {
        let old = previous.get(name).map(|s| s.hash.clone()).unwrap_or_default();
        if &old != hash {
            out.push(Detected {
                name: name.clone(),
                old_hash: old,
                new_hash: hash.clone(),
                time: now.to_string(),
            });
        }
    }
    out
}

/// 把变动写进根目录那份 `user.toml`：`旧哈希 -> 时间 新哈希`（保留历史）。
///
/// 已存在的行**追加**在箭头后面，不覆盖；箭头已经指向的哈希与本次新哈希相同时不动。
pub fn write_watch(path: &Path, previous_text: &str, changes: &[Detected]) -> std::io::Result<String> {
    let mut lines: Vec<String> = previous_text
        .lines()
        .map(|line| line.to_string())
        .collect();
    for change in changes {
        let key = format!("{} = ", change.name);
        let arrow = format!(
            "{} -> {} {}",
            if change.old_hash.is_empty() { "-" } else { &change.old_hash },
            change.time,
            change.new_hash
        );
        if let Some(slot) = lines.iter_mut().find(|line| line.starts_with(&key)) {
            let tail = slot[key.len()..].trim().trim_matches('"').to_string();
            if tail.ends_with(&change.new_hash) {
                continue; // 已经是这个哈希，不重复追加
            }
            *slot = format!("{key}\"{tail} -> {} {}\"", change.time, change.new_hash);
        } else {
            lines.push(format!("{key}\"{arrow}\""));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = lines.join("\n") + "\n";
    std::fs::write(path, &text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamps(pairs: &[(&str, &str, &str)]) -> BTreeMap<String, FolderStamp> {
        pairs
            .iter()
            .map(|(n, t, h)| (n.to_string(), FolderStamp { time: t.to_string(), hash: h.to_string() }))
            .collect()
    }

    fn hashes(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(n, h)| (n.to_string(), h.to_string())).collect()
    }

    #[test]
    fn unchanged_hash_keeps_time() {
        let dir = std::env::temp_dir().join("weasel-next-user-toml-keep");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("build").join("user.toml");
        let prev = stamps(&[("schema", "2026-10-16 10:00:00", "aaa"), ("theme", "2026-10-16 10:00:00", "bbb")]);
        // schema 变了、theme 没变
        let cur = hashes(&[("schema", "ccc"), ("theme", "bbb")]);
        let merged = write_stamps(&path, &cur, &prev, "2026-10-16 11:00:00").expect("写入应当成功");
        assert_eq!(merged["schema"].time, "2026-10-16 11:00:00", "变了的用新时间");
        assert_eq!(merged["theme"].time, "2026-10-16 10:00:00", "没变的保持旧时间");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[schema]") && text.contains("[theme]"), "一段一个文件夹");
        let back = parse_stamps(&text);
        assert_eq!(back["schema"].hash, "ccc");
        assert_eq!(back["theme"].time, "2026-10-16 10:00:00");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_appends_arrow_history() {
        let dir = std::env::temp_dir().join("weasel-next-user-toml-arrow");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("user.toml");
        let prev = stamps(&[("schema", "2026-10-16 10:00:00", "aaa")]);
        let cur = hashes(&[("schema", "ccc"), ("theme", "ddd")]);
        let changes = detect_changes(&prev, &cur, "2026-10-16 11:00:00");
        assert_eq!(changes.len(), 2, "两个文件夹都变了");
        let text = write_watch(&path, "", &changes).expect("写入应当成功");
        assert!(text.contains("schema = \"aaa -> 2026-10-16 11:00:00 ccc\""), "旧哈希 -> 时间 新哈希");
        assert!(text.contains("theme = \"- -> 2026-10-16 11:00:00 ddd\""), "原先没有记录时用 - 占位");
        // 再检测一次同样的内容：不应重复追加
        let again = detect_changes(&prev, &cur, "2026-10-16 12:00:00");
        let text2 = write_watch(&path, &text, &again).expect("写入应当成功");
        assert_eq!(text2.matches("->").count(), text.matches("->").count(), "哈希没变不重复追加");
        let _ = std::fs::remove_dir_all(&dir);
    }
}