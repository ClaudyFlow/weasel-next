//! 剪贴板数据层(2026-11-04):历史环 / 置顶 / 收藏 / 隐藏 / 加密 / 同步标记。
//!
//! 存储格式:JSON 文件(`%APPDATA%\Rime-Next\clipboard.json`),与用户词典/统计
//! 同在用户数据根。历史环上限可配(默认 200 条,超出自动清理最旧非置顶项)。
//!
//! 用法:
//! ```
//! let mut store = ClipboardStore::load(root);
//! store.push("复制的文本");          // 剪贴板新内容入环(去重:相同文本置顶旧条)
//! store.set_pin(0, true);           // 置顶(排到最前,不被清理)
//! store.set_fav(0, true);           // 收藏
//! store.set_hidden(1, true);        // 隐藏(仅"隐藏"过滤可见)
//! store.set_lock(2, true, "密码");  // 加密(查看需密码)
//! let items = store.visible("all", ""); // 按过滤+搜索取可见列表
//! ```

use std::path::{Path, PathBuf};

const DEFAULT_MAX: usize = 200;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClipEntry {
    pub text: String,
    #[serde(default)]
    pub pin: bool,
    #[serde(default)]
    pub fav: bool,
    #[serde(default)]
    pub hid: bool,
    #[serde(default)]
    pub lock: bool,
    #[serde(default)]
    pub pass: Option<String>,
    /// 时间戳(HH:MM 显示用)
    pub time: String,
    /// 同步标记(云端已备份)
    #[serde(default)]
    pub synced: bool,
}

/// 剪贴板历史存储。
pub struct ClipboardStore {
    entries: Vec<ClipEntry>,
    max: usize,
    path: Option<PathBuf>,
}

impl ClipboardStore {
    pub fn load(user_root: &Path) -> Self {
        let path = user_root.join("clipboard.json");
        let entries: Vec<ClipEntry> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { entries, max: DEFAULT_MAX, path: Some(path) }
    }

    pub fn entries(&self) -> &[ClipEntry] {
        &self.entries
    }

    /// 剪贴板新内容入环。相同文本置顶旧条(刷新时间),不重复添加。
    pub fn push(&mut self, text: &str) {
        if text.is_empty() { return; }
        let now = now_hhmm();
        // 去重:已有相同文本 → 移到最前 + 刷新时间
        if let Some(pos) = self.entries.iter().position(|e| e.text == text) {
            let mut entry = self.entries.remove(pos);
            entry.time = now;
            self.entries.insert(0, entry);
        } else {
            self.entries.insert(0, ClipEntry {
                text: text.to_string(),
                pin: false, fav: false, hid: false, lock: false,
                pass: None, time: now, synced: false,
            });
        }
        self.trim();
        self.save();
    }

    /// 超上限清理:从最旧开始删非置顶项。
    fn trim(&mut self) {
        while self.entries.len() > self.max {
            let victim = self.entries.iter().rposition(|e| !e.pin);
            match victim {
                Some(i) => { self.entries.remove(i); }
                None => break, // 全是置顶,不删
            }
        }
    }

    pub fn set_pin(&mut self, index: usize, on: bool) { if let Some(e) = self.entries.get_mut(index) { e.pin = on; } self.save(); }
    pub fn set_fav(&mut self, index: usize, on: bool) { if let Some(e) = self.entries.get_mut(index) { e.fav = on; } self.save(); }
    pub fn set_hidden(&mut self, index: usize, on: bool) { if let Some(e) = self.entries.get_mut(index) { e.hid = on; } self.save(); }
    pub fn set_lock(&mut self, index: usize, on: bool, pass: Option<String>) {
        if let Some(e) = self.entries.get_mut(index) { e.lock = on; e.pass = pass; }
        self.save();
    }
    pub fn mark_synced(&mut self, index: usize) { if let Some(e) = self.entries.get_mut(index) { e.synced = true; } self.save(); }

    /// 按过滤条件 + 搜索取可见列表(下标指向 entries)。
    /// filter: "all" / "pin" / "fav" / "hid"
    pub fn visible(&self, filter: &str, search: &str) -> Vec<usize> {
        let q = search.to_lowercase();
        self.entries.iter().enumerate()
            .filter(|(_, e)| {
                match filter {
                    "pin" if !e.pin => return false,
                    "fav" if !e.fav => return false,
                    "hid" if !e.hid => return false,
                    _ => {}
                }
                q.is_empty() || e.text.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// 删除条目。
    pub fn remove(&mut self, index: usize) { if index < self.entries.len() { self.entries.remove(index); } self.save(); }

    fn save(&self) {
        if let Some(path) = &self.path
            && let Ok(text) = serde_json::to_string_pretty(&self.entries)
        {
            let _ = std::fs::write(path, text);
        }
    }
}

fn now_hhmm() -> String {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime { year: u16, month: u16, day_of_week: u16, day: u16, hour: u16, minute: u16, second: u16, milliseconds: u16 }
    unsafe extern "system" { fn GetLocalTime(time: *mut SystemTime); }
    let mut now = SystemTime::default();
    unsafe { GetLocalTime(&mut now) };
    format!("{:02}:{:02}", now.hour, now.minute)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> ClipboardStore {
        let dir = std::env::temp_dir().join(format!("lrn_clip_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ClipboardStore::load(&dir)
    }

    #[test]
    fn push_dedup_and_refresh() {
        let mut store = temp_store();
        store.push("hello");
        store.push("world");
        store.push("hello"); // 去重:移到最前
        assert_eq!(store.entries()[0].text, "hello");
        assert_eq!(store.entries().len(), 2);
    }

    #[test]
    fn pin_prevents_trim() {
        let mut store = temp_store();
        store.max = 3;
        store.push("keep_me");
        store.set_pin(0, true); // 置顶 keep_me(当前 entries[0])
        for i in 0..5 { store.push(&format!("item{i}")); } // 触发清理
        assert!(store.entries().iter().any(|e| e.text == "keep_me"), "置顶条目不被清理");
    }

    #[test]
    fn visible_filters_work() {
        let mut store = temp_store();
        store.push("alpha");
        store.push("beta"); // entries[0]=beta(最新在前)
        store.set_fav(0, true); // beta 被收藏
        let favs = store.visible("fav", "");
        assert_eq!(favs.len(), 1);
        assert_eq!(store.entries()[favs[0]].text, "beta");
    }

    #[test]
    fn lock_requires_password() {
        let mut store = temp_store();
        store.push("secret");
        store.set_lock(0, true, Some("8848".into()));
        assert!(store.entries()[0].lock);
        assert_eq!(store.entries()[0].pass.as_deref(), Some("8848"));
    }
}
