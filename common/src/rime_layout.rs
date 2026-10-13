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
        assert!(root.join("model/zh-hans.gram").exists());
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

    #[test]
    fn validate_reports_missing_and_flat_placement() {
        let root = temp("validate");
        fs::create_dir_all(root.join("schema/ok")).unwrap();
        fs::write(root.join("schema/ok/ok.schema.yaml"), "x").unwrap();
        fs::write(root.join("schema/ok/ok.dict.yaml"), "x").unwrap();
        fs::write(root.join("missing.dict.yaml"), "x").unwrap();

        let issues = validate(&root, &["ok".into(), "missing".into(), "absent".into()]);
        // ok 无问题；missing 缺 schema.yaml（且在扁平位置有 dict.yaml → 两条）；absent 两个都缺
        assert!(issues.iter().any(|i| i.level == Level::Error && i.message.contains("absent")));
        assert!(issues.iter().any(|i| i.message.contains("missing.schema.yaml")));
        assert!(issues.iter().any(|i| i.level == Level::Warning && i.message.contains("扁平位置")));
        let _ = fs::remove_dir_all(&root);
    }
}
