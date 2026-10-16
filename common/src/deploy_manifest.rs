//! 部署记录：`<用户目录>/deploy.yaml`。
//!
//! 文件是 `<用户目录>/deploy.yaml`（rime 自己的状态另用 `installation.yaml` / `user.yaml`）。
//!
//! 职责是"这次要不要部署"以及"哪些文件夹变了"，为**增量部署**提供依据：
//!
//! * 只统计**勾选中的**方案（清单来自 `schema/schema.yaml`）——用户上次勾选、这次取消的
//!   方案完全不参与检查，改它也不会触发部署，单次部署量因此小一截；
//! * 一行一个文件夹：`"<文件夹>" = "<部署时间> <sha256>"`，首行是整体的部署时间；
//! * **只有部署成功才写入**（失败一律不动），所以文件里的时间就是"最后一次成功部署"；
//! * 只存在于用户文件夹根目录；`build/` 里不再放清单（那是 rime 的产物目录）。
//!
//! 为什么需要它：librime 只按**源文件的 mtime** 决定重建，源文件被 touch（内容一个字节
//! 没变）也会触发分钟级全量重建。先在这里比对内容哈希，才能把"没意义的部署"整段跳掉。
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// 文件名固定放在用户目录根下。
///
/// 叫 `deploy.yaml` 而不是 `user.yaml`：`user.*` 是 **rime 自己**的运行时状态
/// （上次选的方案、方案访问时间），重名会互相覆盖 ✗ —— rime 的归 rime，这份归我们。
pub const STATE_FILE: &str = "deploy.yaml";

/// 参与部署的源文件后缀（方案、词典、补丁、主题、方案清单）。
const SOURCE_EXTENSIONS: [&str; 3] = ["yaml", "yml", "toml"];

/// 历史遗留的清单位置（`build/.deploy-manifest.sha256`），只为清理旧文件而保留。
const LEGACY_MANIFEST: &str = "build/.deploy-manifest.sha256";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// 部署记录：整体时间 + 每个文件夹的"时间 哈希"。
pub struct Manifest {
    /// 最近一次成功部署的时间。
    pub deployed_at: Option<String>,
    /// 文件夹键 → `"<时间> <sha256>"`。
    pub folders: BTreeMap<String, String>,
}

impl Manifest {
    /// 只取哈希部分用于比较（时间不参与）。
    fn hashes(&self) -> BTreeMap<&str, &str> {
        self.folders
            .iter()
            .map(|(key, value)| (key.as_str(), hash_of(value)))
            .collect()
    }

    /// 条目数（日志用）。
    pub fn len(&self) -> usize {
        self.folders.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty()
    }

    /// 记录一个文件夹：内容没变就沿用原来的时间，变了才盖新时间。
    fn record(&mut self, key: String, hash: String, stamp: &str) {
        let previous = self.folders.get(&key);
        let stamp = match previous {
            Some(value) if hash_of(value) == hash => value
                .split_whitespace()
                .next()
                .unwrap_or(stamp)
                .to_string(),
            _ => stamp.to_string(),
        };
        self.folders.insert(key, format!("{stamp} {hash}"));
    }
}

/// 从 `"<时间> <哈希>"` 里取哈希部分。
///
/// 取**最后一个**空白分隔的片段：收集阶段还没盖章，值形如 `" <哈希>"`，
/// 用 `nth(1)` 会得到 `None`（曾经因此把"内容没变"误判成"变了"）。
fn hash_of(value: &str) -> &str {
    value.split_whitespace().last().unwrap_or("")
}

/// `<用户目录>/deploy.yaml`。
pub fn state_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join(STATE_FILE)
}

/// 本地时间 `YYYY-MM-DD HH:MM:SS`（rime 的 `installation.yaml` 用 UTC，这里给人看，
/// 用本地时间更直观；取不到系统时间时退化成空串，不影响比对逻辑）。
fn now_string() -> String {
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct SystemTimeFields {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    unsafe extern "system" {
        fn GetLocalTime(out: *mut SystemTimeFields);
    }
    let mut fields = SystemTimeFields::default();
    unsafe { GetLocalTime(&mut fields) };
    if fields.year == 0 {
        return String::new();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        fields.year, fields.month, fields.day, fields.hour, fields.minute, fields.second
    )
}

/// 读取记录；文件不存在或不可用时返回 `None`（调用方按"需要部署"处理）。
pub fn load(path: &Path) -> Option<Manifest> {
    let text = fs::read_to_string(path).ok()?;
    let mut manifest = Manifest::default();
    let mut in_folders = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "folders:" {
            in_folders = true;
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches('"').to_string();
        let value = value.trim().trim_matches('"').to_string();
        if in_folders {
            manifest.folders.insert(key, value);
        } else if key == "deployed_at" {
            manifest.deployed_at = Some(value);
        }
    }
    (!manifest.folders.is_empty()).then_some(manifest)
}

/// 写出记录（必要时创建父目录）。**只在部署成功后调用。**
pub fn store(path: &Path, current: &Manifest) -> io::Result<()> {
    let stamp = now_string();
    let mut merged = Manifest {
        deployed_at: Some(stamp.clone()),
        folders: BTreeMap::new(),
    };
    for (key, value) in &current.folders {
        merged.record(key.clone(), hash_of(value).to_string(), &stamp);
    }
    let mut text = String::from(
        "# Weasel-Next 部署记录 —— 只在用户文件夹根目录存在这一份，别处不会生成。\n\
         # 首行是最近一次**成功**部署的时间；下面是各文件夹的「时间 内容哈希」，一行一个。\n\
         # 内容没变就不重新部署；部署失败不会改写这个文件。\n",
    );
    text.push_str(&format!(
        "deployed_at: {}\n\nfolders:\n",
        merged.deployed_at.clone().unwrap_or_default()
    ));
    for (key, value) in &merged.folders {
        text.push_str(&format!("  \"{key}\": \"{value}\"\n"));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)
}

/// 判断"源文件与上次部署时完全一致"。
///
/// `stored` 为 `None`（从没部署过、或记录损坏）时返回 `false`，也就是照常部署 ——
/// 宁可多部署一次，也不能因为缓存问题漏掉改动。
pub fn is_unchanged(stored: Option<&Manifest>, current: &Manifest) -> bool {
    match stored {
        Some(stored) => !current.is_empty() && stored.hashes() == current.hashes(),
        None => false,
    }
}

/// 清理历史遗留的 `build/.deploy-manifest.sha256`（记录已搬到用户根目录）。
pub fn remove_legacy(user_data_dir: &Path) {
    let path = user_data_dir.join(LEGACY_MANIFEST);
    if path.is_file() {
        let _ = fs::remove_file(path);
    }
}

/// 收集**需要检查**的文件夹与内容哈希。
///
/// 只统计：勾选中的方案、主题、细胞词库、宿主配置。没勾选的方案完全不看 ——
/// 用户取消勾选之后再改它的文件，也不该触发部署。
pub fn collect(shared_dir: &Path, user_data_dir: &Path) -> io::Result<Manifest> {
    let mut manifest = Manifest::default();
    let stamp = String::new(); // 收集阶段不盖章，store 时统一写

    // 勾选中的方案：共享目录与用户目录各按**目录**记一条（没勾选的完全不看）
    for id in checked_schemas(shared_dir, user_data_dir) {
        for (origin, root) in [("shared", shared_dir), ("user", user_data_dir)] {
            if let Some(hash) = hash_folder(&root.join("schema").join(&id))? {
                manifest.record(format!("{origin}/schema/{id}"), hash, &stamp);
            }
        }
    }

    // 主题与细胞词库：按目录记（目录里的文件不再单独记，避免同一个东西两行）
    for (origin, root) in [("shared", shared_dir), ("user", user_data_dir)] {
        for kind in ["theme", "eassy"] {
            if let Some(hash) = hash_folder(&root.join(kind))? {
                manifest.record(format!("{origin}/{kind}"), hash, &stamp);
            }
        }
    }

    // 用户目录根下的两个散文件（不在上面任何目录里，各记一条）
    for name in ["schema/schema.yaml", "weasel.custom.yaml"] {
        let path = user_data_dir.join(name.replace('/', std::path::MAIN_SEPARATOR_STR));
        if path.is_file() {
            manifest.record(
                format!("user/{name}"),
                hash_files(std::iter::once(path))?,
                &stamp,
            );
        }
    }

    Ok(manifest)
}

/// 读勾选清单（`schema/schema.yaml` 的 `schemas` 列表，用户目录优先）。
fn checked_schemas(shared_dir: &Path, user_data_dir: &Path) -> Vec<String> {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Table {
        schemas: Vec<String>,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Legacy {
        schemas: BTreeMap<String, bool>,
    }
    for root in [user_data_dir, shared_dir] {
        let path = root.join("schema").join("schema.yaml");
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(table) = crate::yaml::from_str::<Table>(&text) {
            if !table.schemas.is_empty() {
                return table.schemas;
            }
        }
        // 兼容最早的写法：`[schemas]` 表（键 = 方案，值 = 是否勾选）
        if let Ok(legacy) = crate::yaml::from_str::<Legacy>(&text) {
            let mut ids: Vec<String> = legacy
                .schemas
                .iter()
                .filter(|(_, selected)| **selected)
                .map(|(id, _)| id.clone())
                .collect();
            ids.sort();
            if !ids.is_empty() {
                return ids;
            }
        }
    }
    Vec::new()
}

/// 一个文件夹里所有源文件的内容哈希（按相对路径排序，结果稳定）。
///
/// 目录不存在或里面**没有**参与部署的源文件时返回 `None` —— 空目录不该在记录里
/// 占一行（空内容的哈希是固定的 `e3b0c442…`，看着像有效数据，其实什么都没记）。
fn hash_folder(folder: &Path) -> io::Result<Option<String>> {
    let mut files = Vec::new();
    walk(folder, folder, &mut files)?;
    if files.is_empty() {
        return Ok(None);
    }
    files.sort();
    hash_files(files.into_iter().map(|relative| folder.join(relative))).map(Some)
}

/// 把一组文件的内容按顺序喂给同一个哈希器（相对路径也参与，改名同样算变化）。
fn hash_files(paths: impl Iterator<Item = PathBuf>) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    for path in paths {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        let mut file = fs::File::open(&path)?;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        hasher.update(b"\0");
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 递归收集参与部署的源文件（跳过 `build/` 与隐藏文件）。
fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
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
                out.push(relative.to_path_buf());
            }
        }
    }
    Ok(())
}

fn has_source_extension(name: &str) -> bool {
    let Some((_, extension)) = name.rsplit_once('.') else {
        return false;
    };
    SOURCE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("weasel-next-deploy-state-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// 记录写在用户根目录，格式是"首行时间 + 一行一个文件夹"。
    #[test]
    fn state_file_lives_in_the_user_root_with_one_line_per_folder() {
        let user = temp("layout");
        let shared = temp("layout-shared");
        fs::create_dir_all(user.join("schema").join("luna_pinyin")).unwrap();
        fs::write(
            user.join("schema").join("luna_pinyin").join("luna_pinyin.dict.yaml"),
            b"name: luna_pinyin\n",
        )
        .unwrap();
        fs::write(
            user.join("schema").join("schema.yaml"),
            "schemas:\n  - luna_pinyin\n  - stroke\n",
        )
        .unwrap();
        // stroke 勾选着但没有文件 → 不产生条目；没勾选的方案不检查
        fs::create_dir_all(user.join("schema").join("cangjie5")).unwrap();
        fs::write(user.join("schema").join("cangjie5").join("x.yaml"), b"skip: 1\n").unwrap();

        let current = collect(&shared, &user).unwrap();
        let path = state_path(&user);
        store(&path, &current).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with('#'), "首行是注释说明");
        assert!(text.contains("\ndeployed_at: "), "要有部署时间");
        assert!(text.contains("\"user/schema/luna_pinyin\":"), "勾选的方案要有一行");
        assert!(!text.contains("cangjie5"), "没勾选的方案不参与");
        assert!(path.starts_with(&user), "记录只放在用户目录");
        assert_eq!(path, user.join("deploy.yaml"));
        let _ = fs::remove_dir_all(&user);
        let _ = fs::remove_dir_all(&shared);
    }

    /// 条目命名规则：一律带 `shared/` 或 `user/` 前缀；同一个东西只记一行；空目录不记。
    #[test]
    fn entries_are_prefixed_deduplicated_and_skip_empty_folders() {
        let user = temp("naming");
        let shared = temp("naming-shared");
        // 用户主题目录里只有一个 theme.yaml：应当只出现 "user/theme" 一行
        fs::create_dir_all(user.join("theme")).unwrap();
        fs::write(user.join("theme").join("theme.yaml"), b"color_scheme: aqua\n").unwrap();
        // 空目录：不该出现任何行
        fs::create_dir_all(user.join("eassy")).unwrap();
        // 勾选的方案 + 清单 + 宿主配置
        fs::create_dir_all(user.join("schema").join("luna_pinyin")).unwrap();
        fs::write(
            user.join("schema").join("luna_pinyin").join("luna_pinyin.dict.yaml"),
            b"name: luna_pinyin\n",
        )
        .unwrap();
        fs::write(
            user.join("schema").join("schema.yaml"),
            "schemas:\n  - luna_pinyin\n",
        )
        .unwrap();
        fs::write(user.join("weasel.custom.yaml"), b"theme: official\n").unwrap();

        let manifest = collect(&shared, &user).unwrap();
        let keys: Vec<&str> = manifest.folders.keys().map(String::as_str).collect();
        assert!(keys.iter().all(|k| k.starts_with("shared/") || k.starts_with("user/")),
                "所有条目都必须带 shared/ 或 user/ 前缀：{keys:?}");
        assert!(keys.contains(&"user/theme"), "主题按目录记一行");
        assert!(!keys.iter().any(|k| k.ends_with("theme/theme.yaml")), "目录里的文件不再单独记");
        assert!(!keys.iter().any(|k| k.contains("eassy")), "空目录不记：{keys:?}");
        assert!(keys.contains(&"user/schema/luna_pinyin"), "勾选的方案记一行");
        assert!(keys.contains(&"user/schema/schema.yaml"), "清单文件带前缀");
        assert!(keys.contains(&"user/weasel.custom.yaml"), "宿主配置带前缀");
        // 哈希都不是空内容哈希
        assert!(
            manifest.folders.values().all(|v| !v.contains("e3b0c442")),
            "空内容哈希不该出现在记录里：{:?}",
            manifest.folders
        );
        let _ = fs::remove_dir_all(&user);
        let _ = fs::remove_dir_all(&shared);
    }

    /// 内容没变 → 判定"无需部署"；内容变了 → 判定"要部署"。
    #[test]
    fn unchanged_content_skips_and_changed_content_deploys() {
        let user = temp("compare");
        let shared = temp("compare-shared");
        fs::create_dir_all(user.join("schema").join("luna_pinyin")).unwrap();
        let dict = user.join("schema").join("luna_pinyin").join("luna_pinyin.dict.yaml");
        fs::write(&dict, b"a: 1\n").unwrap();
        fs::write(
            user.join("schema").join("schema.yaml"),
            "schemas:\n  - luna_pinyin\n",
        )
        .unwrap();

        let first = collect(&shared, &user).unwrap();
        let path = state_path(&user);
        store(&path, &first).unwrap();
        let stored = load(&path);
        assert!(is_unchanged(stored.as_ref(), &first), "内容没变应判定为无需部署");

        // 只改内容：哈希必须变
        fs::write(&dict, b"a: 2\n").unwrap();
        let changed = collect(&shared, &user).unwrap();
        assert!(!is_unchanged(stored.as_ref(), &changed), "内容变了应判定为要部署");
        let _ = fs::remove_dir_all(&user);
        let _ = fs::remove_dir_all(&shared);
    }

    /// 上次记录里没有的文件夹 → 判定为要部署；记录为空 → 一律部署。
    #[test]
    fn missing_or_empty_state_always_deploys() {
        let user = temp("missing");
        let shared = temp("missing-shared");
        fs::create_dir_all(user.join("schema")).unwrap();
        fs::write(
            user.join("schema").join("schema.yaml"),
            "schemas:\n  - luna_pinyin\n",
        )
        .unwrap();
        fs::create_dir_all(user.join("schema").join("luna_pinyin")).unwrap();
        fs::write(
            user.join("schema").join("luna_pinyin").join("luna_pinyin.dict.yaml"),
            b"a: 1\n",
        )
        .unwrap();
        let current = collect(&shared, &user).unwrap();
        assert!(!is_unchanged(None, &current), "没有记录时要部署");
        assert!(!is_unchanged(Some(&Manifest::default()), &current), "空记录要部署");
        let _ = fs::remove_dir_all(&user);
        let _ = fs::remove_dir_all(&shared);
    }

    /// 时间戳只在内容变化时更新：同一个文件夹内容没变就沿用旧时间。
    #[test]
    fn folder_time_is_kept_when_content_is_unchanged() {
        let user = temp("stamp");
        let shared = temp("stamp-shared");
        fs::create_dir_all(user.join("schema")).unwrap();
        fs::write(user.join("weasel.custom.yaml"), b"theme: official\n").unwrap();
        let current = collect(&shared, &user).unwrap();
        let path = state_path(&user);
        store(&path, &current).unwrap();
        let first = load(&path).unwrap();

        // 再存一次（内容没变）→ 时间保持
        store(&path, &current).unwrap();
        let second = load(&path).unwrap();
        assert_eq!(first.folders, second.folders, "内容没变时不该改时间");
        let _ = fs::remove_dir_all(&user);
        let _ = fs::remove_dir_all(&shared);
    }
}
