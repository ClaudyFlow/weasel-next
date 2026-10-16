//! Rime-Next 布局的**读写/规范化**与**校验**。
//!
//! 目录约定（嵌套分类，避免扁平文件混淆）：
//!
//! ```text
//! <根>/
//!   schema/<方案>/<方案>.schema.yaml    方案
//!   schema/<方案>/<方案>.dict.yaml      词典
//!   eassy/<名字>/<名字>.eassy.toml      细胞词库（TOML 源）
//!   eassy/essay.txt                     部署产物（由 TOML 归并生成）
//!   model/<名字>.gram                   语言模型
//!   theme/<名字>/<名字>.theme.toml      主题
//!   （其余扁平文件如 default.yaml / punctuation.yaml 属于全局配置，保持扁平）
//! ```
//!
//! 本模块提供两件事：
//! - [`normalize`]：把散落在根目录的 `*.schema.yaml` / `*.dict.yaml` / `*.eassy.toml` / `*.gram`
//!   **搬进**上面的嵌套位置（只移动，不删除内容），并返回每一步动作供日志使用。
//! - [`validate`]：校验 `schema_list` 中每个方案的源文件是否就位，缺什么就报什么。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 一次文件搬迁（供日志/汇报）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    /// 原路径。
    pub from: PathBuf,
    /// 新路径。
    pub to: PathBuf,
}

/// 一个校验问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// 严重程度。
    pub level: Level,
    /// 说明。
    pub message: String,
}

/// 问题级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// 缺失但会降级运行（例如语言模型）。
    Warning,
    /// 缺失会导致方案不可用（必须修）。
    Error,
}

/// 把 `<name>.schema.yaml` / `<name>.dict.yaml` / `<name>.eassy.toml` / `<name>.gram`
/// 从根目录搬进嵌套位置。返回已执行的搬迁清单。
///
/// 只做移动：目标已存在时不覆盖（视为已整理），源为符号链接/目录时跳过。
pub fn normalize(root: &Path) -> io::Result<Vec<Move>> {
    let mut moves = Vec::new();
    if !root.is_dir() {
        return Ok(moves);
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let (dir, stem) = if let Some(stem) = name.strip_suffix(".schema.yaml") {
            ("schema", stem.to_string())
        } else if let Some(stem) = name.strip_suffix(".dict.yaml") {
            ("schema", stem.to_string())
        } else if let Some(stem) = name.strip_suffix(".eassy.toml") {
            ("eassy", stem.to_string())
        } else if let Some(stem) = name.strip_suffix(".gram") {
            ("model", stem.to_string())
        } else if let Some(stem) = name.strip_suffix(".theme.toml") {
            ("theme", stem.to_string())
        } else if let Some(stem) = name.strip_suffix(".lua") {
            ("plugin", stem.to_string())
        } else {
            continue;
        };
        // 统一成树：<类型>/<名字>/<名字>.<后缀>（model / theme / plugin 也带名字这一层）
        let target_dir = root.join(dir).join(&stem);
        fs::create_dir_all(&target_dir)?;
        let target = target_dir.join(&name);
        if target.exists() {
            continue;
        }
        fs::rename(&path, &target)?;
        moves.push(Move { from: path, to: target });
    }
    Ok(moves)
}

/// 校验一份根下的方案源文件是否齐全。
///
/// `schemas` 是本次要校验的方案名（通常来自 `default.custom.yaml` 的 `schema_list`）。
pub fn validate(root: &Path, schemas: &[String]) -> Vec<Issue> {
    let mut issues = Vec::new();
    for name in schemas {
        let dir = root.join("schema").join(name);
        for suffix in [".schema.yaml", ".dict.yaml"] {
            let nested = dir.join(format!("{name}{suffix}"));
            let flat = root.join(format!("{name}{suffix}"));
            if !nested.exists() && !flat.exists() {
                issues.push(Issue {
                    level: Level::Error,
                    message: format!(
                        "缺少方案文件 {suffix}：应为 {}（或兼容位置 {}）",
                        nested.display(),
                        flat.display()
                    ),
                });
            } else if !nested.exists() {
                issues.push(Issue {
                    level: Level::Warning,
                    message: format!(
                        "方案文件 {name}{suffix} 仍在扁平位置 {}，建议搬进 {}",
                        flat.display(),
                        dir.display()
                    ),
                });
            }
        }
    }
    issues
}

/// 删掉用户根目录里遗留的扁平方案/词典 yaml（分类副本存在时才算重复品）。
///
/// 旧的扁平布局：`<user>\luna_pinyin.schema.yaml`。现在读端按 `schema/<名字>/`
/// 与 `build/<方案>/` 分类，根下这些成了重复品 —— 留着既碍眼，又容易被误当源文件，
/// 还会让"源文件哈希"跟着抖。只有确认分类副本在 `schema/` 或 `build/` 里存在时才删。
///
/// 返回删掉的文件清单（日志用）。
pub fn cleanup_flat_sources(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    if !root.is_dir() {
        return Ok(removed);
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let stem = match name.find('.') {
            Some(index) if index > 0 => name[..index].to_string(),
            _ => continue,
        };
        let classified = match () {
            _ if name.ends_with(".schema.yaml") || name.ends_with(".dict.yaml") => {
                root.join("schema").join(&stem).join(&name)
            }
            _ if name.ends_with(".reverse.bin")
                || name.ends_with(".table.bin")
                || name.ends_with(".prism.bin") =>
            {
                root.join("build").join(&stem).join(&name)
            }
            _ => continue,
        };
        if classified.is_file() {
            fs::remove_file(&path)?;
            removed.push(path);
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weasel-layout-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn moves_flat_files_into_nested_dirs() {
        let root = temp("normalize");
        fs::write(root.join("sancang.schema.yaml"), "schema_version: 1\n").unwrap();
        fs::write(root.join("sancang.dict.yaml"), "name: sancang\n").unwrap();
        fs::write(root.join("default.eassy.toml"), "[words]\n\"甲\" = 1\n").unwrap();
        fs::write(root.join("zh-hans.gram"), b"x").unwrap();
        fs::write(root.join("default.yaml"), "config_version: 1\n").unwrap();

        let moves = normalize(&root).unwrap();
        assert_eq!(moves.len(), 4, "四个分类文件应被搬走");
        assert!(root.join("schema/sancang/sancang.schema.yaml").exists());
        assert!(root.join("schema/sancang/sancang.dict.yaml").exists());
        assert!(root.join("eassy/default/default.eassy.toml").exists());
        assert!(root.join("model/zh-hans/zh-hans.gram").exists());
        // 全局配置保持扁平
        assert!(root.join("default.yaml").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn normalize_is_idempotent() {
        let root = temp("idempotent");
        fs::write(root.join("a.schema.yaml"), "x").unwrap();
        let first = normalize(&root).unwrap();
        assert_eq!(first.len(), 1);
        let second = normalize(&root).unwrap();
        assert!(second.is_empty(), "第二次应无事可做");
        let _ = fs::remove_dir_all(&root);
    }

    /// 重复品要被清掉，而不是留在 build\\ 根下越积越多。
    #[test]
    fn classify_keeps_only_schema_config_flat() {
        let root = temp("classify-layout");
        fs::create_dir_all(root.join("luna_pinyin")).unwrap();
        // 词典表已在正确位置：不许动
        fs::write(root.join("luna_pinyin").join("luna_pinyin.table.bin"), b"table").unwrap();
        // 配置被错放进子目录：必须摊回根下
        fs::write(root.join("luna_pinyin").join("luna_pinyin.schema.yaml"), b"schema").unwrap();
        // 根下的 prism：必须归到自己的目录（rime: error opening prism file）
        fs::write(root.join("luna_pinyin_simp.prism.bin"), b"prism").unwrap();
        // 根下的词典表：必须归到自己的目录（rime: Error opening table file）
        fs::write(root.join("stroke.table.bin"), b"stroke").unwrap();
        fs::write(root.join("default.yaml"), b"global").unwrap();

        let moves = classify_staging(&root).unwrap();
        assert_eq!(moves.len(), 3, "schema.toml 摊平 + prism/table 归目录");
        assert!(root.join("luna_pinyin.schema.yaml").exists(), "配置是唯一扁平的那个");
        assert!(
            root.join("luna_pinyin_simp").join("luna_pinyin_simp.prism.bin").exists(),
            "prism 必须分目录"
        );
        assert!(
            root.join("stroke").join("stroke.table.bin").exists(),
            "词典表必须分目录"
        );
        assert!(
            root.join("luna_pinyin").join("luna_pinyin.table.bin").exists(),
            "已就位的词典表不许动"
        );
        assert!(root.join("default.yaml").exists(), "全局配置留在根下");

        let again = classify_staging(&root).unwrap();
        assert!(again.is_empty(), "第二次应无事可做（幂等）");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn validate_reports_missing_and_flat_placement() {
        let root = temp("validate");
        fs::create_dir_all(root.join("schema/ok")).unwrap();
        fs::write(root.join("schema/ok/ok.schema.yaml"), "x").unwrap();
        fs::write(root.join("schema/ok/ok.dict.yaml"), "x").unwrap();
        fs::write(root.join("missing.dict.yaml"), "x").unwrap();

        let issues = validate(&root, &["ok".into(), "missing".into(), "absent".into()]);
        // ok 无问题；missing 缺 schema.toml（且在扁平位置有 dict.yaml → 两条）；absent 两个都缺
        assert!(issues.iter().any(|i| i.level == Level::Error && i.message.contains("absent")));
        assert!(issues.iter().any(|i| i.message.contains("missing.schema.yaml")));
        assert!(issues.iter().any(|i| i.level == Level::Warning && i.message.contains("扁平位置")));
        let _ = fs::remove_dir_all(&root);
    }
}
/// 把部署产物收进 `build/<方案>/`（同名分类，目录内部不再分层）。
///
/// 规格：`build/<方案>/<方案>.schema.yaml`、`<方案>.table.bin`、`<方案>.prism.bin`、
/// `<方案>.reverse.bin` 放在同一层。`default.yaml` 等全局配置留在 `build/` 根下。
///
/// 为什么需要这一步：librime 里给这些产物定路径的是 `Dictionary` 组件自己的解析器，
/// 与 `DictCompiler` 的目标解析器不是同一个实例，一边算出分类路径、一边拼扁平路径。
/// 与其在两处同时改引擎，不如在部署收尾时统一移位 —— 读端走的是同一套分类规则，
/// 移完之后两边自然一致。
pub fn classify_staging(staging: &Path) -> io::Result<Vec<Move>> {
    // `build/` 的布局规则是照 rime 的报错实证出来的，**只有配置是扁平的**：
    //
    //   build/<方案>.schema.yaml                      ← 配置层按扁平路径找，嵌套就"找不到方案"
    //   build/<名字>/<名字>.prism.bin                 ← prism 要分目录
    //   build/<名字>/<名字>.table.bin / .reverse.bin   ← 词典表要分目录
    //
    // 两句报错分别对应踩错的两边：
    //   W nonexistent config file '...build\luna_pinyin_simp.schema.yaml'
    //   E error opening prism file '...build\luna_pinyin_simp\luna_pinyin_simp.prism.bin'
    //   E Error opening table file '...build\luna_pinyin\luna_pinyin.table.bin'
    //
    // 返回搬动的清单（调用方用于日志）。
    let mut moves = Vec::new();
    if !staging.is_dir() {
        return Ok(moves);
    }

    // 只给这三类产物分目录；其余（尤其 `default.yaml`、`<方案>.schema.yaml`）必须留在根下。
    // `default.yaml` 是 rime 的基础配置，搬走就等于把整个 workspace 弄丢 ✗
    let wanted_dir = |name: &str| -> Option<String> {
        let is_artifact = name.ends_with(".prism.bin")
            || name.ends_with(".table.bin")
            || name.ends_with(".reverse.bin");
        if !is_artifact {
            return None;
        }
        let stem = name.split('.').next()?;
        if stem.is_empty() {
            return None;
        }
        Some(stem.to_string())
    };

    // ① 子目录里位置不对的（配置）摊回根下
    for entry in fs::read_dir(staging)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let folder = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        for nested in fs::read_dir(&path)? {
            let nested = nested?.path();
            if !nested.is_file() {
                continue;
            }
            let Some(name) = nested.file_name().map(|name| name.to_string_lossy().to_string()) else {
                continue;
            };
            if wanted_dir(&name).is_some_and(|dir| dir == folder) {
                continue;  // 位置正确
            }
            let target = staging.join(&name);
            replace_if_newer(&nested, &target)?;
            moves.push(Move { from: nested, to: target });
        }
        if fs::read_dir(&path)?.next().is_none() {
            let _ = fs::remove_dir(&path);
        }
    }

    // ② 根下该分目录的产物归位
    for entry in fs::read_dir(staging)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().map(|name| name.to_string_lossy().to_string()) else {
            continue;
        };
        let Some(dir) = wanted_dir(&name) else {
            continue;
        };
        let target_dir = staging.join(&dir);
        fs::create_dir_all(&target_dir)?;
        let target = target_dir.join(&name);
        replace_if_newer(&path, &target)?;
        moves.push(Move { from: path, to: target });
    }

    Ok(moves)
}

/// 把 `source` 搬到 `target`：目标已存在时保留**更新**的那份。
fn replace_if_newer(source: &Path, target: &Path) -> io::Result<()> {
    if target.exists() {
        let source_is_newer = match (
            fs::metadata(source).and_then(|meta| meta.modified()),
            fs::metadata(target).and_then(|meta| meta.modified()),
        ) {
            (Ok(newer), Ok(existing)) => newer > existing,
            _ => true,
        };
        if source_is_newer {
            fs::rename(source, target)
        } else {
            fs::remove_file(source)
        }
    } else {
        fs::rename(source, target)
    }
}

