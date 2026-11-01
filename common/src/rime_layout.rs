//! Rime-Next 布局的**读写/规范化**与**校验**。
//!
//! 目录约定（嵌套分类，避免扁平文件混淆）：
//!
//! ```text
//! <根>/
//!   schema/<方案>/<方案>.schema.yaml    方案
//!   schema/<方案>/<方案>.dict.yaml      词典
//!   eassy/<名字>/<名字>.eassy.yaml      细胞词库（YAML 源）
//!   eassy/essay.txt                     部署产物（由 YAML 归并生成）
//!   model/<名字>.gram                   语言模型
//!   theme/<名字>/<名字>.theme.yaml      主题
//!   （其余扁平文件如 default.yaml / punctuation.yaml 属于全局配置，保持扁平）
//! ```
//!
//! 本模块提供两件事：
//! - [`normalize`]：把散落在根目录的 `*.schema.yaml` / `*.dict.yaml` / `*.eassy.yaml` / `*.gram`
//!   **搬进**上面的嵌套位置（只移动，不删除内容），并返回每一步动作供日志使用。
//! - [`validate`]：校验 `schema_list` 中每个方案的源文件是否就位，缺什么就报什么。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// **程序文件夹布局**（2026-10-28 定）：
///
/// ```text
/// PROGRAM_DIR/Rime-Next/            ← 程序文件夹（安装目录，exe 在这层）
///     Weasel-Next-Server.exe …      ← 可执行文件
///     data/                         ← 内置数据目录 = librime 的 shared_data_dir
///         default.yaml  schema/  opencc/  parser/  eassy/  theme/ ...
/// ```
///
/// - `PROGRAM_DIR`：安装根（`Program Files`）。
/// - `Rime-Next`：产品目录 —— **程序文件夹就到这一层**，安装路径 + 产品名即可。
/// - `data`：数据目录 —— 本模块里就叫 [`DATA_FOLDER`]，由 [`data_dir`] 解析。
///
/// ★ **版本不进目录层级**。`Weasel-Next-<版本>` 是**安装包名 / 可执行文件名**
/// （`Weasel-Next-0.18.4-Setup.exe`、`Weasel-Next-Engine.exe`），不是目录名；
/// 多版本并存之类的排布属于**外壳（Weasel）安装器**的决定，Rime 不参与。
///
/// ★ **分层职责**：拼路径是 Weasel 的事，Rime 侧只有四个**被注入**的目录
/// （`shared_data_dir` / `user_data_dir` / `prebuilt_data_dir` / `staging_dir`），
/// 它们拿到的是这里的**绝对路径**，不知道也不需要知道 `Program Files`、
/// `Rime-Next`、`data` 这些名字是怎么来的。
///
/// 用户文件夹（`%APPDATA%\Rime-Next`）**不受影响**，见 [`USER_DATA_FOLDER`]。

/// 安装根 `PROGRAM_DIR` 的环境变量（`Program Files`）。
pub const PROGRAM_DIR_ENV: &str = "ProgramFiles";

/// 产品目录名：`PROGRAM_DIR` 下的 `Rime-Next`。
pub const PRODUCT_FOLDER: &str = "Rime-Next";

/// 版本号（编译期取自 crate 版本，全仓统一）。
///
/// 只用于**安装包/可执行文件名**（`Weasel-Next-<版本>`），不参与目录拼装。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// **数据目录名**（2026-10-28 由 `shared` 改名而来）：程序文件夹里的 `data`。
pub const DATA_FOLDER: &str = "data";

/// 改名前的共享数据目录名（`Rime-Next`），**仅用于旧装机回退兼容**。
pub const LEGACY_DATA_FOLDER: &str = "Rime-Next";

/// 用户数据目录名（`%APPDATA%\Rime-Next`）。
///
/// 2026-10-28 定：用户文件夹布局**不动**，只改程序文件夹。
pub const USER_DATA_FOLDER: &str = "Rime-Next";

/// 程序文件夹：`PROGRAM_DIR/Rime-Next`（安装路径 + 产品名，到此为止）。
///
/// 拿不到 `ProgramFiles` 时返回 `None`（此时调用方应改用 exe 所在目录，见 [`data_dir`]）。
pub fn program_dir() -> Option<PathBuf> {
    std::env::var_os(PROGRAM_DIR_ENV).map(|base| PathBuf::from(base).join(PRODUCT_FOLDER))
}

/// **数据目录**：`<程序文件夹>/data`（旧装机回退 `<程序文件夹>/Rime-Next`）。
///
/// 入参是程序文件夹（正常运行时就是 exe 所在目录）。过渡期两套布局都能跑；
/// 新安装器铺成 `…/Rime-Next/data` 后回退自然失效。
///
/// 返回值即交给 librime 的 `shared_data_dir` —— Rime 只认这一个绝对路径。
pub fn data_dir(install_dir: &Path) -> PathBuf {
    let modern = install_dir.join(DATA_FOLDER);
    if modern.is_dir() {
        return modern;
    }
    install_dir.join(LEGACY_DATA_FOLDER)
}

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

/// 把 `<name>.schema.yaml` / `<name>.dict.yaml` / `<name>.eassy.yaml` / `<name>.gram`
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
          // 用户 2026-10-19：**根部的 .dict.yaml 不再归类进 schema\<id>\** ——
          // librime 的 dictionary / import_tables 只在用户目录根部解析，
          // 挪进子目录会让「日月/宇浩」这类外部方案报 source file does not exist。
          // 用户 2026-10-19：根部的 **rime.lua** 不许搬走 ——
          // librime-lua 只在数据目录根部找引导文件，搬进 plugin\ 等于没装。
          if name == "rime.lua" {
              continue;
          }
          if name.ends_with(".dict.yaml") {
              continue;
          }
          let (dir, stem) = if let Some(stem) = name.strip_suffix(".schema.yaml") {
            ("schema", stem.to_string())
        } else if let Some(stem) = name.strip_suffix(".dict.yaml") {
            ("schema", stem.to_string())
        } else if let Some(stem) = name.strip_suffix(".eassy.yaml") {
            ("eassy", stem.to_string())
        } else if let Some(stem) = name.strip_suffix(".gram") {
            ("model", stem.to_string())
        } else if let Some(stem) = name.strip_suffix(".theme.yaml") {
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
          // 用户 2026-10-19：用 import_tables 的方案（日月/宇浩系列）**必须**保留根部扁平 dict ——
          // librime 只在用户目录根部解析 dictionary / import_tables，删掉它们就编不出词典。
          // 用户 2026-10-19：**不删根部 .dict.yaml** —— 日月/宇浩等外部方案
          // 的词典只能从用户目录根部解析（librime 的 dictionary / import_tables），
          // 删了它们就编不出词典；我们自己的方案本来就没有根部 dict，不受影响。
          let removable = !name.ends_with(".dict.yaml");
          if classified.is_file() && removable {
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
        fs::write(root.join("default.eassy.yaml"), "words://n  \"甲\": 1\n").unwrap();
        fs::write(root.join("zh-hans.gram"), b"x").unwrap();
        fs::write(root.join("default.yaml"), "config_version: 1\n").unwrap();

        let moves = normalize(&root).unwrap();
        assert_eq!(moves.len(), 3, "schema/eassy/model 三个搬走；根部 *.dict.yaml 保留");
        assert!(root.join("schema/sancang/sancang.schema.yaml").exists());
        assert!(root.join("sancang.dict.yaml").exists(), "★ 根部 dict 不许搬（日月/宇浩靠它）");
        assert!(root.join("eassy/default/default.eassy.yaml").exists());
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
        assert_eq!(moves.len(), 2, "prism/table 归目录；配置留在子目录（build 布局自 2026-10-20 起如此）");
        assert!(root.join("luna_pinyin").join("luna_pinyin.schema.yaml").exists(), "配置留在方案目录");
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

    // 只给三类**产物**分目录；`default.yaml` 与 `<方案>.schema.yaml` 必须留在根下 ——
    // rime 报过错：配置层是扁平查找，搬进子目录就 "nonexistent config file"，方案会消失。
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

    // ① 兼容旧布局：把子目录里放错的**产物**摊回根下。
    //    ★ 2026-10-20 起 build\<方案>\<方案>.schema.yaml 是**正确**布局（改了 librime 的
    //      resource.cc），因此这里不再搬运 .schema.yaml / .dict.yaml，只处理 .bin。

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


/// 该方案是否用 `import_tables`（日月/宇浩系列那样把词库拆成共享表）。
///
/// 这类方案的词典必须留在用户目录根部：librime 的 `dictionary` / `import_tables`
/// 只按「用户目录根 / 相对路径」解析，挪进 `schema/<id>/` 反而编不出来。
fn scheme_uses_import_tables(root: &Path, stem: &str) -> bool {
    // 注意：`import_tables` 写在**词库**（dict.yaml）里，不是 schema 里
    let dict = root.join(format!("{stem}.dict.yaml"));
    fs::read_to_string(dict)
        .map(|text| text.contains("import_tables"))
        .unwrap_or(false)
}
