//! 下载页数据层：仓库分类、网络错误判定、已安装清单与「有没有新版」。
//!
//! 组织名写死为 `Rime-Next`，镜像自动探测由 plum 的 `fetch` 负责，这里不涉及。
//! 列表拉取与安装逻辑在 `Weasel-Next-Setting` 的 `wire_downloads` 里直接调 plum-next。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 安装清单：`<用户根>\.plum-next\installed.json`。
///
/// 为什么要有它：GitHub 的列表只给「最新 pushed_at」，不知道本机装的是哪一版；
/// 没有清单就只能显示「✓ 已安装」，永远发现不了更新（2026-10-30 用户要求「更新拉取」）。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct InstalledRepo {
    /// 完整名 `Rime-Next/schema.092`。
    #[serde(default)]
    pub repo: String,
    /// 上次拉取时远端仓库的 pushed_at（ISO8601）。
    #[serde(default)]
    pub pushed_at: String,
    /// 本机安装时间（人读）。
    #[serde(default)]
    pub installed_at: String,
}

pub fn manifest_path(user_root: &Path) -> PathBuf {
    user_root.join(".plum-next").join("installed.json")
}

/// 读清单；坏文件当空清单处理（宁可显示「未安装」，也不要让下载页整页打不开）。
pub fn load_installed(user_root: &Path) -> BTreeMap<String, InstalledRepo> {
    let path = manifest_path(user_root);
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn record_installed(user_root: &Path, entry: InstalledRepo) -> std::io::Result<()> {
    let path = manifest_path(user_root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut all = load_installed(user_root);
    all.insert(entry.repo.clone(), entry);
    let text = serde_json::to_string_pretty(&all).unwrap_or_else(|_| "{}".to_owned());
    std::fs::write(path, text)
}

/// 卡片徽标：`已安装` / `可更新` / 空（没装过）。
///
/// `pushed_at` 是 ISO8601（`2026-10-29T12:34:56Z`），字典序即时间序，直接比字符串。
pub fn badge_for(installed: Option<&InstalledRepo>, remote_pushed_at: Option<&str>) -> &'static str {
    let Some(local) = installed else { return "" };
    let remote = remote_pushed_at.unwrap_or_default();
    if remote.is_empty() || local.pushed_at.is_empty() {
        return "已安装";
    }
    if remote > local.pushed_at.as_str() {
        "可更新"
    } else {
        "已安装"
    }
}

/// 根据仓库名 + topics 推导分类。
pub fn category_of(name: &str, topics: &[String]) -> String {
    let mut hay = name.to_lowercase();
    for t in topics {
        hay.push(' ');
        hay.push_str(&t.to_lowercase());
    }
    if hay.contains("theme") || hay.contains("皮肤") || hay.contains("skin") {
        "主题".to_owned()
    } else if hay.contains("dict") || hay.contains("词典") {
        "词典".to_owned()
    } else if hay.contains("schema") || hay.contains("方案") {
        "方案".to_owned()
    } else if hay.contains("tool") || hay.contains("util") || hay.contains("plugin") {
        "工具".to_owned()
    } else {
        "其它".to_owned()
    }
}

/// 判断错误文案是否来自网络问题（用于安装阶段弹通知横幅）。
pub fn is_network_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("connect")
        || m.contains("timed out")
        || m.contains("timeout")
        || m.contains("resolve")
        || m.contains("ssl")
        || m.contains("network")
        || m.contains("connection")
        || m.contains("econn")
        || m.contains("reset")
        || m.contains("中断")
        || m.contains("断连")
}

#[cfg(test)]
mod tests {
    use super::{badge_for, load_installed, record_installed, InstalledRepo};
    use std::path::PathBuf;

    fn entry(repo: &str, pushed: &str) -> InstalledRepo {
        InstalledRepo {
            repo: repo.to_owned(),
            pushed_at: pushed.to_owned(),
            installed_at: "epoch:0".to_owned(),
        }
    }

    #[test]
    fn never_installed_has_no_badge() {
        assert_eq!(badge_for(None, Some("2026-10-30T00:00:00Z")), "");
    }

    #[test]
    fn same_pushed_at_is_installed() {
        let local = entry("Rime-Next/schema.092", "2026-10-30T00:00:00Z");
        assert_eq!(badge_for(Some(&local), Some("2026-10-30T00:00:00Z")), "已安装");
    }

    #[test]
    fn newer_remote_pushed_at_means_update() {
        let local = entry("Rime-Next/schema.092", "2026-10-01T00:00:00Z");
        assert_eq!(badge_for(Some(&local), Some("2026-10-30T12:00:00Z")), "可更新");
    }

    #[test]
    fn unknown_remote_timestamp_falls_back_to_installed() {
        let local = entry("Rime-Next/schema.092", "2026-10-01T00:00:00Z");
        assert_eq!(badge_for(Some(&local), None), "已安装");
    }

    #[test]
    fn manifest_round_trip() {
        let root: PathBuf = std::env::temp_dir().join(format!("wn-market-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(load_installed(&root).is_empty());
        record_installed(&root, entry("Rime-Next/a", "2026-10-01T00:00:00Z")).unwrap();
        record_installed(&root, entry("Rime-Next/b", "2026-10-02T00:00:00Z")).unwrap();
        let all = load_installed(&root);
        assert_eq!(all.len(), 2);
        assert_eq!(all["Rime-Next/a"].pushed_at, "2026-10-01T00:00:00Z");
        let _ = std::fs::remove_dir_all(&root);
    }
}
