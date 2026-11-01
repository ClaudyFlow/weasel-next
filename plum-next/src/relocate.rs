//! 仓库 A / 方案 B、C 的「拉取 + 搬运」流程（用户总结的 plum 核心流程）。
//!
//! 完整步骤（与用户给的总结一一对应）：
//! 1. 从 URL 解析仓库名 B，拼出本地路径 `用户根目录/B`；要求用户根存在、B 不存在或为空。
//! 2. libgit2 clone（`fetch::clone_only`，返回值判断状态；失败用 code/class/message 上报）。
//! 3. 推导仓库名（workdir 的 basename，回退 origin URL 末段；libgit2 无直接 API）。
//! 4. std::fs 校验 `B/part1`、`B/part2` 根层存在且为真实目录
//!    （非符号链接、非文件、大小写严格一致、拒绝对穿越）。
//! 5. 防覆盖检查：用户根下的 part1/part2 已存在且非空 → 报错（`--force` 可强制先删）。
//! 6. fs_extra::dir::move_dir 只把这两个文件夹搬到用户根（同盘 rename，跨盘自动 copy+delete）。
//! 7. 校验搬运结果。
//! 8. remove_dir_all(B)（用 remove_dir_all crate，处理 Windows .git 只读文件）。
//! 9. 最终用户根下直接是 part1、part2，程序即可识别。

use std::path::{Path, PathBuf};

use crate::fetch::{self, FetchOpts};

/// 一次搬运的结果。
pub struct RelocateReport {
    pub repo_name: String,
    pub parts: Vec<String>,
}

/// 从仓库 URL 解析仓库名：取最后一段，去掉 `.git`。
///
/// libgit2 没有直接的「仓库名」API，必须推导。
pub fn repo_name_from_url(url: &str) -> String {
    let url = url.trim_end_matches('/');
    let last = url.rsplit('/').next().unwrap_or(url);
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

/// 校验 part 名字合法：拒绝 `.` / `..` / 绝对路径 / 含路径分隔符（防路径穿越）。
fn validate_part_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(format!("非法的 part 名: {name:?}"));
    }
    if Path::new(name).is_absolute() {
        return Err(format!("part 名不能是绝对路径: {name:?}"));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(format!("part 名不能含路径分隔符: {name:?}"));
    }
    Ok(())
}

/// 在 `parent` 下找名字与 `name` **大小写严格相等**的条目
/// （在大小写不敏感的文件系统上也能识别被改过大小写的目录）。
fn read_dir_exact_name(parent: &Path, name: &str) -> Option<String> {
    let entries = std::fs::read_dir(parent).ok()?;
    for e in entries.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n == name {
            return Some(n);
        }
    }
    None
}

/// 检查 `parent/name` 存在且为**真实目录**（非符号链接、非文件），且目录名大小写严格一致。
fn check_real_dir(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let path = parent.join(name);
    let meta =
        std::fs::symlink_metadata(&path).map_err(|e| format!("无法访问 {path:?}: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err(format!("{path:?} 是符号链接，已拒绝（只允许真实目录）"));
    }
    if !meta.is_dir() {
        return Err(format!("{path:?} 不是目录"));
    }
    match read_dir_exact_name(parent, name) {
        Some(real) if real == name => Ok(path),
        Some(real) => Err(format!(
            "大小写不匹配：期望 {name:?}，实际 {real:?}（拒绝识别）"
        )),
        None => Err(format!("{path:?} 不存在或名字大小写不匹配")),
    }
}

/// 目录是否非空。
fn dir_non_empty(path: &Path) -> bool {
    std::fs::read_dir(path)
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
}

/// 拉取并搬运一个仓库：clone `url` → `用户根目录/repo_name`，
/// 校验根层 part，搬到用户根，最后删除临时仓库目录（含 .git）。
pub fn pull_and_relocate(
    user_root: &Path,
    url: &str,
    branch: Option<&str>,
    parts: &[String],
    force: bool,
    opts: &FetchOpts,
    emit: impl Fn(&str, f64, &str),
) -> Result<RelocateReport, String> {
    let repo_name = clone_to_temp(user_root, url, branch, opts, &emit)?;
    let local = user_root.join(&repo_name);
    for p in parts {
        validate_part_name(p)?;
    }
    relocate_move(user_root, &local, &repo_name, parts, force, &emit)
}

/// 市场 / 插件式安装：clone 后把仓库根层的**所有真实目录**整体搬进用户根目录
/// （等价于 `pull_and_relocate`，但 parts 自动取「仓库顶层目录」，无需手动指定）。
pub fn pull_and_relocate_all(
    user_root: &Path,
    url: &str,
    branch: Option<&str>,
    force: bool,
    opts: &FetchOpts,
    emit: impl Fn(&str, f64, &str),
) -> Result<RelocateReport, String> {
    let repo_name = clone_to_temp(user_root, url, branch, opts, &emit)?;
    let local = user_root.join(&repo_name);

    // 枚举根层真实目录作为 parts
    let mut parts: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(&local)
        .map_err(|e| format!("无法读取仓库根目录 {local:?}: {e}"))?;
    for e in entries.flatten() {
        let p = e.path();
        let meta = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() && !meta.file_type().is_symlink() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                parts.push(name.to_string());
            }
        }
    }
    if parts.is_empty() {
        // 没有可搬运目录：清理临时克隆，避免残留
        let _ = remove_dir_all::remove_dir_all(&local);
        return Err(format!("仓库 {repo_name} 根层没有可搬运的目录"));
    }

    relocate_move(user_root, &local, &repo_name, &parts, force, &emit)
}

/// 步骤 1-3：解析仓库名、校验用户根、clone 到临时目录 `用户根/repo_name`。
/// 返回推导后的仓库名。
fn clone_to_temp(
    user_root: &Path,
    url: &str,
    branch: Option<&str>,
    opts: &FetchOpts,
    emit: &impl Fn(&str, f64, &str),
) -> Result<String, String> {
    let repo_name = repo_name_from_url(url);
    if repo_name.is_empty() {
        return Err(format!("无法从 URL 解析仓库名: {url}"));
    }
    if !user_root.exists() {
        return Err(format!("用户根目录不存在: {}", user_root.display()));
    }
    if !user_root.is_dir() {
        return Err(format!("用户根目录不是目录: {}", user_root.display()));
    }
    let local = user_root.join(&repo_name);
    // 用户根非空 + 只认根层，所以本地目录 B 不能已存在且非空
    if local.exists() && dir_non_empty(&local) {
        return Err(format!(
            "本地目录已存在且非空，无法 clone 到: {}（请先清理）",
            local.display()
        ));
    }
    emit("clone", 0.1, &format!("clone {url} → {}", local.display()));
    fetch::clone_only(&local, url, branch, opts).map_err(|e| {
        format!(
            "clone 失败: code={:?}, class={:?}, msg={}",
            e.code(),
            e.class(),
            e.message()
        )
    })?;
    Ok(derive_repo_name(&local).unwrap_or(repo_name))
}

/// 步骤 4-8：校验 parts 结构、防覆盖、搬运、搬运后校验、删除临时目录。
/// 与「显式 parts」和「自动 parts」两种入口共用。
fn relocate_move(
    user_root: &Path,
    local: &Path,
    repo_name: &str,
    parts: &[String],
    force: bool,
    emit: &impl Fn(&str, f64, &str),
) -> Result<RelocateReport, String> {
    emit("validate", 0.4, "校验顶层目录");
    for p in parts {
        validate_part_name(p)?;
        check_real_dir(local, p)
            .map_err(|e| format!("仓库 {repo_name} 结构校验失败: {e}"))?;
    }

    // 防覆盖检查
    if force {
        for p in parts {
            let dest = user_root.join(p);
            if dest.exists() {
                remove_dir_all::remove_dir_all(&dest)
                    .map_err(|e| format!("清理已存在的 {dest:?} 失败: {e}"))?;
            }
        }
    } else {
        for p in parts {
            let dest = user_root.join(p);
            if dest.exists() && dir_non_empty(&dest) {
                return Err(format!(
                    "目标 {dest:?} 已存在且非空，拒绝覆盖（用 --force 强制）"
                ));
            }
        }
    }

    // 搬运顶层目录（同盘 rename；跨盘 fs_extra 自动 fallback 到 copy+delete）
    emit("move", 0.6, "搬运顶层目录");
    for p in parts {
        let src = local.join(p);
        let dest = user_root.join(p);
        let mut options = fs_extra::dir::CopyOptions::new();
        options.overwrite = true;
        fs_extra::dir::move_dir(&src, &dest, &options)
            .map_err(|e| format!("搬运 {src:?} → {dest:?} 失败: {e}"))?;
    }

    // 校验搬运结果
    for p in parts {
        check_real_dir(user_root, p)
            .map_err(|e| format!("搬运后校验失败: {e}（请检查目标目录）"))?;
    }

    // 删除临时仓库目录 B（含 .git，Windows 只读安全）
    emit("cleanup", 0.9, "删除临时仓库目录");
    remove_dir_all::remove_dir_all(local)
        .map_err(|e| format!("删除临时目录 {local:?} 失败: {e}"))?;

    emit("done", 1.0, &format!("完成（仓库 {repo_name}）"));
    Ok(RelocateReport {
        repo_name: repo_name.to_string(),
        parts: parts.to_vec(),
    })
}

/// 推导仓库名：优先 workdir 的 basename，回退 origin URL 末段解析。
fn derive_repo_name(local: &Path) -> Option<String> {
    let repo = git2::Repository::open(local).ok()?;
    if let Some(wd) = repo.workdir() {
        if let Some(name) = wd.file_name() {
            return Some(name.to_string_lossy().to_string());
        }
    }
    if let Ok(remote) = repo.find_remote("origin") {
        if let Ok(url) = remote.url() {
            let n = repo_name_from_url(url);
            if !n.is_empty() {
                return Some(n);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_name_from_url_strips_git_and_trailing_slash() {
        assert_eq!(repo_name_from_url("https://github.com/A/B.git"), "B");
        assert_eq!(repo_name_from_url("https://github.com/A/B"), "B");
        assert_eq!(repo_name_from_url("https://github.com/A/B.git/"), "B");
    }

    #[test]
    fn validate_part_name_rejects_traversal_and_abs() {
        assert!(validate_part_name("part1").is_ok());
        assert!(validate_part_name("part2").is_ok());
        assert!(validate_part_name("..").is_err());
        assert!(validate_part_name(".").is_err());
        assert!(validate_part_name("/etc").is_err());
        assert!(validate_part_name("a/b").is_err());
        assert!(validate_part_name("a\\b").is_err());
    }
}
