//! 候选窗材质三档（2026-11-02）：`%APPDATA%\Rime-Next\theme\material.txt`。
//!
//! 分工：**主题**（`theme/src/paint.rs::tier`）在画候选窗时运行时读这个文件；
//! **设置界面**只负责写一行字。所以三档的字面值与顺序必须和主题侧
//! `Material::parse` 对齐 —— 改一边等于两边都要改。
//!
//! 历史：这三档原来是编译期 `--features acrylic/original` 出的三个 DLL
//! （`theme-glass.dll` / `theme-acrylic.dll` / `theme-original.dll`）。2026-11-02
//! 用户要求"一个 DLL、在设置里切"，于是统一成这个文件。

use std::path::{Path, PathBuf};

/// 三档写进文件里的字面值；下标 = 设置界面按钮的顺序（0/1/2）。
///
/// 第三档的文件值仍是 `opaque`（主题侧一直认它），只是界面上改叫「默认」——
/// 用户 2026-11-04 10:50：「默认是不透明，然后不要写成不透明，写成默认」。
pub const TIERS: [&str; 3] = ["glass", "acrylic", "opaque"];

/// 三档的中文名（按钮文字与状态行共用）。
///
/// 第三档叫 **默认**：它就是主题本来的样子（yaml 的 `back_color` 铺满、不抓屏不模糊），
/// 而不是"特意关掉效果"的档位。
pub const LABELS: [&str; 3] = ["透明", "亚克力", "默认"];

/// `material.txt` 的完整路径（`<用户目录>/theme/material.txt`）。
pub fn path(user: &Path) -> PathBuf {
    user.join("theme").join("material.txt")
}

/// 读当前档位下标。
///
/// 文件缺失、空、未知值、以及 11-01 遗留的 `plain` 一律当 **0（透明）**——
/// 与主题侧 `Material::parse` 的默认分支一致。
pub fn current(user: &Path) -> usize {
    let text = std::fs::read_to_string(path(user)).unwrap_or_default();
    match text.trim().to_ascii_lowercase().as_str() {
        "acrylic" => 1,
        // `default` 是 2026-11-04 之后界面上的叫法；`opaque` / `original` / `solid`
        // 是同一档的旧值（写回时仍用 `opaque`）。
        "default" | "opaque" | "original" | "solid" => 2,
        _ => 0,
    }
}

/// 把档位写进 `material.txt`；返回写入的字面值，越界返回 `InvalidInput`。
///
/// 目录不存在时自动创建（用户可能刚装好、还没打过字）。
pub fn set(user: &Path, index: usize) -> std::io::Result<&'static str> {
    let Some(name) = TIERS.get(index) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "材质档位越界（只有 0/1/2）",
        ));
    };
    let file = path(user);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&file, name)?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例一个独立临时目录，绝不碰真实用户目录。
    fn temp_user(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "weasel-material-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_and_reads_each_tier() {
        let user = temp_user("roundtrip");
        for (index, name) in TIERS.iter().enumerate() {
            assert_eq!(set(&user, index).unwrap(), *name);
            assert_eq!(std::fs::read_to_string(path(&user)).unwrap(), *name);
            assert_eq!(current(&user), index);
        }
        let _ = std::fs::remove_dir_all(&user);
    }

    #[test]
    fn missing_and_legacy_values_fall_back_to_glass() {
        let user = temp_user("legacy");
        // 文件不存在
        assert_eq!(current(&user), 0);
        // 11-01 的 plain
        std::fs::create_dir_all(path(&user).parent().unwrap()).unwrap();
        std::fs::write(path(&user), "plain").unwrap();
        assert_eq!(current(&user), 0);
        // 老编译期档名
        std::fs::write(path(&user), "original").unwrap();
        assert_eq!(current(&user), 2);
        // 垃圾值
        std::fs::write(path(&user), "  垃圾 ").unwrap();
        assert_eq!(current(&user), 0);
        let _ = std::fs::remove_dir_all(&user);
    }

    #[test]
    fn out_of_range_index_is_rejected() {
        let user = temp_user("range");
        assert!(set(&user, 3).is_err());
        assert!(!path(&user).exists());
        let _ = std::fs::remove_dir_all(&user);
    }
}
