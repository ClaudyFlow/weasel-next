//! 目录树的哈希清单（Merkle 风格），用于**部署时逐层比对**。
//!
//! 目标（按需求）：根目录下除 `build/` 之外的**每个文件夹**与**每个文件**都有哈希，
//! 存放在根目录的 `.hash-tree.toml` 里，结构是树形的：
//!
//! ```toml
//! version = 1
//!
//! # 文件夹哈希：相对路径 → 哈希（"" 表示根本身）
//! # 算法：把该文件夹下每个直接子项的名字与哈希（子文件夹用它的文件夹哈希）排序后，
//! #       以 "名字\0哈希\n" 相连，再取 SHA-256。因此任一后代变化都会向上传播。
//! [folders]
//! "" = "9f2c…"
//! "schema" = "1a3b…"
//! "schema/sancang" = "77e0…"
//!
//! # 文件哈希：相对路径 → { sha256, size }
//! [files]
//! "schema/sancang/sancang.dict.yaml" = { sha256 = "…", size = 1898902 }
//! ```
//!
//! 部署时比对两份清单：
//! 1. 先看**文件夹哈希**，从根往下找第一处不同的分支（`changed_branches`）；
//! 2. 再在该分支下逐层看，直到定位到**具体文件**（`diff`）。
//! 只有差异分支需要重新部署/重新编译。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 清单文件名（放在被扫描的根目录下）。
pub const TREE_FILE_NAME: &str = ".hash-tree.toml";

/// 默认排除的目录名：编译产物目录不参与哈希（每次部署都会变，且可由源重建）。
pub const DEFAULT_SKIP: [&str; 1] = ["build"];

/// 单个文件条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// 内容的 SHA-256（十六进制小写）。
    pub sha256: String,
    /// 字节数。
    pub size: u64,
}

/// 一棵目录树的哈希清单。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HashTree {
    /// 版本号，便于将来升级格式。
    pub version: u32,
    /// 文件夹相对路径（`/` 分隔，空串表示根）→ 文件夹哈希。
    pub folders: BTreeMap<String, String>,
    /// 文件相对路径（`/` 分隔）→ 文件哈希。
    pub files: BTreeMap<String, FileEntry>,
}

impl HashTree {
    /// 扫描 `root` 生成清单，跳过 `skip` 中的顶层目录名（默认跳过 `build`）。
    ///
    /// 清单文件自身不参与哈希。
    pub fn build(root: &Path) -> io::Result<Self> {
        Self::build_skipping(root, &DEFAULT_SKIP)
    }

    /// 同 [`HashTree::build`]，但可指定要跳过的顶层目录名。
    pub fn build_skipping(root: &Path, skip: &[&str]) -> io::Result<Self> {
        let mut tree = HashTree {
            version: 1,
            folders: BTreeMap::new(),
            files: BTreeMap::new(),
        };
        // 递归自底向上：每个文件夹算完自己的哈希后登记，并把结果交给父级。
        let root_hash = walk(root, "", "", skip, &mut tree)?;
        tree.folders.insert(String::new(), root_hash);
        Ok(tree)
    }

    /// 序列化为 TOML 文本。
    pub fn to_text(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// 从 TOML 文本解析。
    pub fn from_text(text: &str) -> Option<Self> {
        toml::from_str(text).ok()
    }

    /// 读取根目录下的清单（不存在或损坏时返回 `None`）。
    pub fn load(root: &Path) -> Option<Self> {
        let text = fs::read_to_string(root.join(TREE_FILE_NAME)).ok()?;
        Self::from_text(&text)
    }

    /// 写出到根目录下的 `.hash-tree.toml`。
    pub fn store(&self, root: &Path) -> io::Result<()> {
        fs::write(root.join(TREE_FILE_NAME), self.to_text())
    }

    /// 与另一份清单比对，返回**变化的文件路径**（新增/修改/删除）。
    pub fn diff(&self, other: &Self) -> Vec<String> {
        let mut out = Vec::new();
        for (path, entry) in &other.files {
            match self.files.get(path) {
                Some(old) if old == entry => {}
                _ => out.push(path.clone()),
            }
        }
        for path in self.files.keys() {
            if !other.files.contains_key(path) {
                out.push(path.clone());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// 只比较**文件夹哈希**，返回需要重新部署的顶层分支名（相对路径）。
    ///
    /// 这是"先看 a 文件夹，再看 b 文件夹"的第一步：如果某个分支的文件夹哈希相同，
    /// 整棵子树都可以跳过。
    pub fn changed_branches(&self, other: &Self) -> Vec<String> {
        let mut names: Vec<String> = other
            .folders
            .keys()
            .filter(|k| !k.is_empty() && !k.contains('/'))
            .cloned()
            .collect();
        for key in self.folders.keys() {
            if !key.is_empty() && !key.contains('/') && !names.contains(key) {
                names.push(key.clone());
            }
        }
        names.sort();
        names.dedup();
        names
            .into_iter()
            .filter(|name| self.folders.get(name) != other.folders.get(name))
            .collect()
    }
}

/// 递归扫描一个目录，返回该目录的**文件夹哈希**，并把自身登记进 `tree.folders`。
///
/// 文件夹哈希 = 对"直接子项（名字, 哈希）"排序后按 `名字\0哈希\n` 拼接再取 SHA-256；
/// 子文件夹用它的文件夹哈希参与，因此任一后代变化都会向上传播到根。
fn walk(root: &Path, relative: &str, dir: &Path, skip: &[&str], tree: &mut HashTree) -> io::Result<String> {
    let _ = root;
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    let mut children: Vec<(String, String)> = Vec::new();
    for path in entries {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        if name == TREE_FILE_NAME {
            continue;
        }
        let child_relative = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        if path.is_dir() {
            if relative.is_empty() && skip.contains(&name.as_str()) {
                continue;
            }
            let hash = walk(root, &child_relative, &path, skip, tree)?;
            children.push((name, hash));
        } else if path.is_file() {
            let bytes = fs::read(&path)?;
            let hash = hex(Sha256::digest(&bytes).as_slice());
            tree.files.insert(
                child_relative,
                FileEntry {
                    sha256: hash.clone(),
                    size: bytes.len() as u64,
                },
            );
            children.push((name, hash));
        }
    }
    children.sort();
    let mut hasher = Sha256::new();
    for (name, hash) in &children {
        hasher.update(name.as_bytes());
        hasher.update([0u8]);
        hasher.update(hash.as_bytes());
        hasher.update(b"\n");
    }
    let folder_hash = hex(hasher.finalize().as_slice());
    tree.folders.insert(relative.to_string(), folder_hash.clone());
    Ok(folder_hash)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weasel-hashtree-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hashes_every_file_and_folder_except_build() {
        let root = temp("build");
        fs::create_dir_all(root.join("schema/sancang")).unwrap();
        fs::create_dir_all(root.join("build/staging")).unwrap();
        fs::write(root.join("schema/sancang/sancang.dict.yaml"), "name: sancang\n").unwrap();
        fs::write(root.join("default.yaml"), "config_version: 1\n").unwrap();
        fs::write(root.join("build/staging/junk.bin"), b"junk").unwrap();

        let tree = HashTree::build(&root).unwrap();
        assert!(tree.files.contains_key("default.yaml"));
        assert!(tree.files.contains_key("schema/sancang/sancang.dict.yaml"));
        assert!(!tree.files.keys().any(|k| k.starts_with("build/")), "build 应被排除");
        assert!(tree.folders.contains_key(""));
        assert!(tree.folders.contains_key("schema"));
        assert!(tree.folders.contains_key("schema/sancang"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_hash_changes_only_in_affected_branch() {
        let root = temp("branch");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(root.join("a/one.txt"), "1").unwrap();
        fs::write(root.join("b/two.txt"), "2").unwrap();
        let before = HashTree::build(&root).unwrap();

        fs::write(root.join("a/one.txt"), "changed").unwrap();
        let after = HashTree::build(&root).unwrap();

        assert_ne!(before.folders.get("a"), after.folders.get("a"), "a 分支应变");
        assert_eq!(before.folders.get("b"), after.folders.get("b"), "b 分支不应变");
        assert_eq!(before.folders.get(""), after.folders.get(""), "根因 a 变化而变化");
        assert_eq!(after.diff(&before), vec!["a/one.txt".to_string()]);
        assert_eq!(after.changed_branches(&before), vec!["a".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn round_trips_through_toml() {
        let root = temp("toml");
        fs::write(root.join("x.yaml"), "k: v\n").unwrap();
        let tree = HashTree::build(&root).unwrap();
        tree.store(&root).unwrap();
        let loaded = HashTree::load(&root).expect("应能读回");
        assert_eq!(tree, loaded);
        // 清单文件自身不参与哈希
        let again = HashTree::build(&root).unwrap();
        assert!(!again.files.contains_key(TREE_FILE_NAME));
        let _ = fs::remove_dir_all(&root);
    }
}
