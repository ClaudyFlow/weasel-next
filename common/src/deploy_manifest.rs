//! 部署清单：用 SHA-256 判断"这次的源文件是否和上次部署时一致"。
//!
//! librime 自己按**文件修改时间**决定要不要重建方案：只要源 yaml 的 mtime 变新，哪怕内容
//! 一个字节都没变，也会触发一次完整重建——而 `pinyin_simp.dict.yaml` 有 30 MB、重建出的
//! `*.table.bin` 有 88 MB，这就是部署慢的主因。这里改为比较**内容哈希**：
//!
//! * 一致 → 调用方可以直接跳过整个部署（零停机、零耗时）；
//! * 不一致 → 正常部署，部署成功后把新清单落盘。
//!
//! 清单是纯文本、按行 `sha256<两个空格>相对路径`，与 `sha256sum` 的输出习惯一致，便于人眼
//! 核对和 `diff` 比较；比较时按路径排序，因此清单的顺序不影响结果。

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// 参与部署的源文件后缀（字典、方案、补丁、安装信息）。
const SOURCE_EXTENSIONS: [&str; 2] = ["yaml", "yml"];

/// 清单默认存放位置（相对用户数据目录）：放在 `build/` 下，与部署产物同处一地。
pub const MANIFEST_RELATIVE_PATH: &str = "build/.deploy-manifest.sha256";

/// 相对路径 → 内容的 SHA-256（十六进制小写）。
///
/// 用 [`BTreeMap`] 是为了让序列化结果稳定：同一批文件无论遍历顺序如何，清单文本都一样，
/// 因此可以直接用字符串比较判断"是否变化"。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    entries: BTreeMap<String, String>,
}

impl Manifest {
    /// 清单里的文件数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 清单是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 某个相对路径在清单里的哈希。
    pub fn get(&self, relative: &str) -> Option<&str> {
        self.entries.get(relative).map(String::as_str)
    }

    /// 序列化为清单文本（行尾统一为 `\n`，最后一行也带换行）。
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (path, hash) in &self.entries {
            out.push_str(hash);
            out.push_str("  ");
            out.push_str(path);
            out.push('\n');
        }
        out
    }

    /// 解析清单文本；忽略空行与 `#` 注释，无法解析的行直接忽略（清单是缓存，坏了就重算）。
    pub fn from_text(text: &str) -> Self {
        let mut entries = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((hash, path)) = line.split_once("  ") else {
                continue;
            };
            let hash = hash.trim();
            let path = path.trim();
            if hash.len() == 64 && !path.is_empty() {
                entries.insert(path.to_string(), hash.to_ascii_lowercase());
            }
        }
        Self { entries }
    }
}

/// 扫描 `user_data_dir` 下参与部署的源文件，逐个算 SHA-256。
///
/// * 递归遍历，但跳过 `build/`（那是产物目录）与点开头的隐藏目录；
/// * 只收 [`SOURCE_EXTENSIONS`] 里的后缀；
/// * 相对路径统一用 `/` 分隔，保证跨平台一致。
pub fn collect(user_data_dir: &Path) -> io::Result<Manifest> {
    let mut files = Vec::new();
    walk(user_data_dir, user_data_dir, &mut files)?;
    files.sort();
    let mut entries = BTreeMap::new();
    for relative in files {
        let hash = hash_file(&user_data_dir.join(&relative))?;
        entries.insert(relative, hash);
    }
    Ok(Manifest { entries })
}

/// 读取清单文件；文件不存在或内容不可用时返回 `None`（调用方按"需要部署"处理）。
pub fn load(path: &Path) -> Option<Manifest> {
    let text = fs::read_to_string(path).ok()?;
    let manifest = Manifest::from_text(&text);
    (!manifest.is_empty()).then_some(manifest)
}

/// 写出清单文件（必要时创建父目录）。
pub fn store(path: &Path, manifest: &Manifest) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, manifest.to_text())
}

/// 用户数据目录下的清单文件路径。
pub fn manifest_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join(MANIFEST_RELATIVE_PATH)
}

/// 判断"源文件与上次部署时完全一致"。
///
/// `stored` 为 `None`（从没部署过、或清单损坏）时一律返回 `false`，也就是照常部署——
/// 宁可多部署一次，也不能因为缓存问题漏掉改动。
pub fn is_unchanged(stored: Option<&Manifest>, current: &Manifest) -> bool {
    match stored {
        Some(stored) => !current.is_empty() && stored == current,
        None => false,
    }
}

/// 单个文件的 SHA-256（分块读取，30 MB 的字典也不会一次性进内存）。
fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(to_hex(&hasher.finalize()))
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 递归收集相对路径（`/` 分隔），跳过 `build/` 与隐藏目录。
fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if name == "build" {
                continue;
            }
            walk(root, &path, out)?;
        } else if file_type.is_file() && has_source_extension(&name) {
            if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(())
}

fn has_source_extension(name: &str) -> bool {
    let Some((_, extension)) = name.rsplit_once('.') else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    SOURCE_EXTENSIONS.contains(&extension.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip_is_stable_under_ordering() {
        let mut manifest = Manifest::default();
        manifest.entries.insert("b.yaml".into(), "b".repeat(64));
        manifest.entries.insert("a/nested.yaml".into(), "a".repeat(64));
        // 插入顺序不影响输出：BTreeMap 保证按路径排序。
        let text = manifest.to_text();
        let parsed = Manifest::from_text(&text);
        assert_eq!(parsed, manifest);
        assert!(text.starts_with(&"a".repeat(64)));
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn comments_and_broken_lines_are_ignored() {
        let text = format!(
            "# 注释\n\n{}  good.yaml\n不是哈希  bad.yaml\n{} no-two-spaces.yaml\n",
            "c".repeat(64),
            "d".repeat(64)
        );
        let parsed = Manifest::from_text(&text);
        assert_eq!(parsed.len(), 1);
        assert!(parsed.get("good.yaml").is_some());
    }

    #[test]
    fn unchanged_requires_a_stored_matching_manifest() {
        let mut current = Manifest::default();
        current.entries.insert("x.yaml".into(), "e".repeat(64));
        assert!(!is_unchanged(None, &current));
        assert!(is_unchanged(Some(&current.clone()), &current));

        let mut other = current.clone();
        other.entries.insert("x.yaml".into(), "f".repeat(64));
        assert!(!is_unchanged(Some(&other), &current));
    }

    #[test]
    fn empty_manifest_is_never_treated_as_unchanged() {
        let empty = Manifest::default();
        assert!(!is_unchanged(Some(&empty), &empty));
    }

    #[test]
    fn collect_skips_build_directory_and_non_source_files() {
        let root = std::env::temp_dir().join(format!(
            "weasel-deploy-manifest-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("build")).unwrap();
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("one.schema.yaml"), b"a").unwrap();
        fs::write(root.join("nested/two.dict.yaml"), b"b").unwrap();
        fs::write(root.join("build/should-be-ignored.yaml"), b"c").unwrap();
        fs::write(root.join("readme.txt"), b"d").unwrap();

        let manifest = collect(&root).unwrap();
        assert_eq!(manifest.len(), 2);
        assert!(manifest.get("one.schema.yaml").is_some());
        assert!(manifest.get("nested/two.dict.yaml").is_some());
        assert!(manifest.get("build/should-be-ignored.yaml").is_none());

        // 内容变化必然改变哈希。
        fs::write(root.join("one.schema.yaml"), b"changed").unwrap();
        let changed = collect(&root).unwrap();
        assert!(!is_unchanged(Some(&manifest), &changed));

        let _ = fs::remove_dir_all(&root);
    }
}
