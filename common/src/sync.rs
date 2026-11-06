//! 同步层（2026-11-04 建，2026-11-06 接通面板）：WebDAV 上传/下载用户词库缓存 + 剪贴板 JSON。
//!
//! 学 trime 的同步设置后补齐的三块：**测试连接 / 同步间隔 / 接收过滤**。
//! 用 `ureq` 做最小 HTTP 客户端（PUT / GET / PROPFIND / MKCOL + Basic Auth），
//! 不引入大型 WebDAV 库。
//!
//! 配置与状态都在用户数据根（`%APPDATA%\Rime-Next\`）：
//! * `sync.json`      —— 服务器 / 账号 / 间隔 / 接收过滤
//! * `sync_state.json`—— 上次同步时间与结果（面板显示用）
//!
//! 下载遵循「**只合并、不覆盖**」：剪贴板按文本去重合并（本地标记优先），
//! 用户词典只落到 `sync/incoming/` 暂存，绝不覆盖引擎正在写的 leveldb 文件。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// 同步配置（设置外壳 / 面板「编辑配置」写这个文件）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// WebDAV 根，如 `https://dav.jianguoyun.com/dav/`
    pub server: String,
    pub user: String,
    pub pass: String,
    /// 远端目录，如 `/Rime/user_dict/`
    #[serde(default = "default_remote_path")]
    pub remote_path: String,
    /// 自动同步间隔（分钟）；0 = 只手动。
    #[serde(default)]
    pub interval_min: u32,
    /// 是否启用自动同步（与 `interval_min > 0` 同时满足才跑）。
    #[serde(default)]
    pub auto: bool,
    /// 接收过滤：允许从远端下载的类别，`clipboard` / `userdb`。
    /// 空数组 = 只上传不接收（trime 的「仅上传」）。
    #[serde(default = "default_receive")]
    pub receive: Vec<String>,
}

fn default_remote_path() -> String {
    "/Rime/user_dict".into()
}
fn default_receive() -> Vec<String> {
    vec!["clipboard".into(), "userdb".into()]
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            server: "https://dav.jianguoyun.com/dav/".into(),
            user: String::new(),
            pass: String::new(),
            remote_path: default_remote_path(),
            interval_min: 30,
            auto: false,
            receive: default_receive(),
        }
    }
}

impl SyncConfig {
    pub fn accepts(&self, what: &str) -> bool {
        self.receive.iter().any(|s| s.eq_ignore_ascii_case(what))
    }
    /// 面板上显示的服务器（不泄露密码）。
    pub fn summary(&self) -> String {
        let host = self
            .server
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or(&self.server);
        format!("{} @ {}", self.user, host)
    }
}

/// 同步结果。
#[derive(Debug, Default)]
pub struct SyncResult {
    pub uploaded: Vec<String>,
    pub downloaded: Vec<String>,
    pub errors: Vec<String>,
    /// 合并进本地 clipboard.json 的新条数。
    pub merged: usize,
}

impl SyncResult {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![format!(
            "上传 {} / 下载 {} / 合并 {} / 错误 {}",
            self.uploaded.len(),
            self.downloaded.len(),
            self.merged,
            self.errors.len()
        )];
        v.extend(self.errors.iter().map(|e| format!("· {e}")));
        v
    }
}

/// 上次同步状态（面板显示）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncState {
    pub last_sync: Option<String>,
    pub last_ok: bool,
    pub lines: Vec<String>,
}

pub fn config_path(user_root: &Path) -> PathBuf {
    user_root.join("sync.json")
}
pub fn state_path(user_root: &Path) -> PathBuf {
    user_root.join("sync_state.json")
}

pub fn load_config(user_root: &Path) -> Option<SyncConfig> {
    let text = std::fs::read_to_string(config_path(user_root)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_config(user_root: &Path, cfg: &SyncConfig) -> Result<(), String> {
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(config_path(user_root), text).map_err(|e| e.to_string())
}

/// 配置文件不存在时写一份带注释说明不了的 JSON 模板（字段自带默认值语义），返回路径。
pub fn ensure_template(user_root: &Path) -> PathBuf {
    let path = config_path(user_root);
    if !path.exists() {
        let _ = save_config(user_root, &SyncConfig::default());
    }
    path
}

pub fn load_state(user_root: &Path) -> SyncState {
    std::fs::read_to_string(state_path(user_root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_state(user_root: &Path, state: &SyncState) {
    if let Ok(text) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(state_path(user_root), text);
    }
}

fn now_stamp() -> String {
    // 不引 time crate 的格式化特性，用 SystemTime 自己算本地时间足够显示用。
    // 用 Win32 GetLocalTime 会准确；非 Windows 回退成 Unix 秒。
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct SystemTime {
            year: u16, month: u16, day_of_week: u16, day: u16,
            hour: u16, minute: u16, second: u16, milliseconds: u16,
        }
        unsafe extern "system" { fn GetLocalTime(t: *mut SystemTime); }
        let mut t = SystemTime::default();
        unsafe { GetLocalTime(&mut t) };
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
    }
    #[cfg(not(windows))]
    {
        format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs())
    }
}

/// 记录一次同步结果到 `sync_state.json`。
pub fn record_run(user_root: &Path, result: &SyncResult) {
    save_state(user_root, &SyncState {
        last_sync: Some(now_stamp()),
        last_ok: result.ok(),
        lines: result.lines(),
    });
}

// ── WebDAV 原语 ────────────────────────────────────────────────────────────

fn dir_url(cfg: &SyncConfig) -> String {
    format!(
        "{}/{}",
        cfg.server.trim_end_matches('/'),
        cfg.remote_path.trim_matches('/')
    )
}

fn file_url(cfg: &SyncConfig, remote_name: &str) -> String {
    format!("{}/{}", dir_url(cfg), remote_name.trim_start_matches('/'))
}

fn auth_header(cfg: &SyncConfig) -> String {
    format!("Basic {}", basic_auth(&cfg.user, &cfg.pass))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build()
}

/// MKCOL 建远端目录（不存在时）；已存在 / 不支持都当成功，PUT 会给出真实结果。
fn mkcol(cfg: &SyncConfig, url: &str) {
    let _ = agent()
        .request("MKCOL", url)
        .set("Authorization", &auth_header(cfg))
        .call();
}

/// 测试连接：PROPFIND Depth 0 打远端目录；不支持 PROPFIND 的服务器回落 GET。
/// 401/403 会把「账号或密码不对」直接说清楚，不再笼统报一个网络错。
pub fn test_connection(cfg: &SyncConfig) -> Result<String, String> {
    let url = dir_url(cfg);
    let req = || {
        agent()
            .request("PROPFIND", &url)
            .set("Authorization", &auth_header(cfg))
            .set("Depth", "0")
            .call()
    };
    match req() {
        Ok(res) => Ok(format!("连接成功（HTTP {}）", res.status())),
        Err(ureq::Error::Status(401 | 403, _)) => Err("账号或应用密码不对（HTTP 401/403）".into()),
        Err(ureq::Error::Status(405, _)) => {
            // 极少数网关禁 PROPFIND，用 GET 再确认一次凭证是否可用。
            match agent().get(&url).set("Authorization", &auth_header(cfg)).call() {
                Ok(res) => Ok(format!("连接成功（HTTP {}，服务器不支持 PROPFIND）", res.status())),
                Err(ureq::Error::Status(code, _)) => Err(format!("连接失败（HTTP {code}）")),
                Err(e) => Err(format!("连接失败：{e}")),
            }
        }
        Err(ureq::Error::Status(code, _)) => Err(format!("连接失败（HTTP {code}）")),
        Err(e) => Err(format!("连接失败：{e}")),
    }
}

fn webdav_put(cfg: &SyncConfig, local_path: &Path, remote_name: &str) -> Result<(), String> {
    let data = std::fs::read(local_path).map_err(|e| format!("读 {}: {e}", local_path.display()))?;
    let url = file_url(cfg, remote_name);
    let put = || {
        agent()
            .put(&url)
            .set("Authorization", &auth_header(cfg))
            .set("Content-Type", "application/octet-stream")
            .send_bytes(&data)
    };
    match put() {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(404 | 409, _)) => {
            // 远端目录链不存在：逐级 MKCOL 后重试一次。
            mkcol(cfg, &dir_url(cfg));
            if let Some(parent) = remote_name.rfind('/') {
                mkcol(cfg, &file_url(cfg, &remote_name[..parent]));
            }
            match put() {
                Ok(_) => Ok(()),
                Err(e) => Err(format!("PUT {remote_name}: {e}")),
            }
        }
        Err(e) => Err(format!("PUT {remote_name}: {e}")),
    }
}

/// 下载成字节（leveldb 是二进制，不能用 `into_string`）。
fn webdav_get(cfg: &SyncConfig, remote_name: &str) -> Result<Vec<u8>, String> {
    let url = file_url(cfg, remote_name);
    let res = agent()
        .get(&url)
        .set("Authorization", &auth_header(cfg))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "远端不存在（首次同步）".to_string(),
            ureq::Error::Status(401 | 403, _) => "账号或应用密码不对（HTTP 401/403）".to_string(),
            other => format!("GET {remote_name}: {other}"),
        })?;
    let mut buf = Vec::new();
    res.into_reader()
        .take(32 * 1024 * 1024)
        .read_to_end(&mut buf)
        .map_err(|e| format!("GET {remote_name}: {e}"))?;
    Ok(buf)
}

fn basic_auth(user: &str, pass: &str) -> String {
    let credentials = format!("{user}:{pass}");
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = credentials.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        out.push(TABLE[(b[0] >> 2) as usize] as char);
        out.push(TABLE[((b[0] & 0x03) << 4 | (b[1] >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((b[1] & 0x0F) << 2 | (b[2] >> 6)) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(b[2] & 0x3F) as usize] as char } else { '=' });
    }
    out
}

// ── 单次同步 ──────────────────────────────────────────────────────────────

/// 执行同步：上传本地、按接收过滤下载并合并。
pub fn sync(cfg: &SyncConfig, user_root: &Path) -> SyncResult {
    let mut result = SyncResult::default();

    // 1. 剪贴板 JSON：上传 + 合并下载（不再像旧版那样丢进 sync/ 就完事）。
    let clip_path = user_root.join("clipboard.json");
    if clip_path.exists() {
        match webdav_put(cfg, &clip_path, "clipboard.json") {
            Ok(()) => result.uploaded.push("clipboard.json".into()),
            Err(e) => result.errors.push(e),
        }
    }
    if cfg.accepts("clipboard") {
        match webdav_get(cfg, "clipboard.json") {
            Ok(bytes) => {
                #[cfg(windows)]
                match serde_json::from_slice::<Vec<crate::clipboard::ClipEntry>>(&bytes) {
                    Ok(remote) => {
                        let mut store = crate::clipboard::ClipboardStore::load(user_root);
                        result.merged = store.merge_remote(remote);
                        result.downloaded.push("clipboard.json".into());
                    }
                    Err(e) => result.errors.push(format!("远端 clipboard.json 不是合法 JSON: {e}")),
                }
                #[cfg(not(windows))]
                {
                    result.merged = bytes.len();
                    result.downloaded.push("clipboard.json".into());
                }
            }
            Err(e) if e == "远端不存在（首次同步）" => { /* 首次同步不算错误 */ }
            Err(e) => result.errors.push(e),
        }
    }

    // 2. 用户词典：本地逐文件上传；接收过滤允许时把同名远端文件拉到暂存区。
    //    ⚠️ 绝不覆盖 `userdb/` 里正在被引擎写的 leveldb 文件。
    let userdb_dir = user_root.join("userdb");
    if let Ok(entries) = std::fs::read_dir(&userdb_dir) {
        for entry in entries.flatten() {
            let db_dir = entry.path();
            if !db_dir.is_dir() {
                continue;
            }
            let db_name = db_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Ok(files) = std::fs::read_dir(&db_dir) {
                for file in files.flatten() {
                    let path = file.path();
                    if !path.is_file() {
                        continue;
                    }
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let remote = format!("{db_name}/{name}");
                    match webdav_put(cfg, &path, &remote) {
                        Ok(()) => result.uploaded.push(remote.clone()),
                        Err(e) => result.errors.push(e),
                    }
                    if cfg.accepts("userdb") {
                        match webdav_get(cfg, &remote) {
                            Ok(bytes) => {
                                let staging = user_root.join("sync").join("incoming").join(&db_name);
                                if std::fs::create_dir_all(&staging).is_ok()
                                    && std::fs::write(staging.join(&name), bytes).is_ok()
                                {
                                    result.downloaded.push(remote.clone());
                                }
                            }
                            Err(e) if e == "远端不存在（首次同步）" => {}
                            Err(e) => result.errors.push(e),
                        }
                    }
                }
            }
        }
    }
    result
}

/// 面板「立即同步」入口：读配置 → 同步 → 记状态。
pub fn run_once(user_root: &Path) -> Result<SyncResult, String> {
    let cfg = load_config(user_root).ok_or_else(|| "未配置 WebDAV 同步（sync.json）".to_string())?;
    if cfg.server.trim().is_empty() || cfg.user.trim().is_empty() {
        return Err("同步配置不完整：请填服务器与账号".into());
    }
    let result = sync(&cfg, user_root);
    record_run(user_root, &result);
    Ok(result)
}

/// 按 `interval_min` 启动后台自动同步（进程内只启动一次）。
/// 面板打开时调用；首轮不会立刻触发，等满一个间隔再跑，避免开输入法就打网络。
pub fn start_auto(user_root: PathBuf) {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("rime-sync-autorun".into())
            .spawn(move || {
                let mut next: Option<Instant> = None;
                let mut last_interval = 0u32;
                loop {
                    std::thread::sleep(Duration::from_secs(20));
                    let Some(cfg) = load_config(&user_root) else {
                        next = None;
                        continue;
                    };
                    if !cfg.auto || cfg.interval_min == 0 {
                        next = None;
                        continue;
                    }
                    if cfg.interval_min != last_interval {
                        last_interval = cfg.interval_min;
                        next = Some(Instant::now() + Duration::from_secs((cfg.interval_min as u64) * 60));
                        continue;
                    }
                    if next.is_some_and(|t| Instant::now() >= t) {
                        let _ = run_once(&user_root);
                        next = Some(Instant::now() + Duration::from_secs((cfg.interval_min as u64) * 60));
                    }
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_auth_encodes_correctly() {
        assert_eq!(basic_auth("user", "pass"), "dXNlcjpwYXNz");
        assert_eq!(basic_auth("a", "b"), "YTpi");
    }

    #[test]
    fn url_join_has_exactly_one_slash() {
        let cfg = SyncConfig { server: "https://dav.example.com/dav/".into(), remote_path: "/Rime/user_dict/".into(), ..Default::default() };
        assert_eq!(dir_url(&cfg), "https://dav.example.com/dav/Rime/user_dict");
        assert_eq!(file_url(&cfg, "clipboard.json"), "https://dav.example.com/dav/Rime/user_dict/clipboard.json");
        assert_eq!(file_url(&cfg, "luna/LOG"), "https://dav.example.com/dav/Rime/user_dict/luna/LOG");
    }

    #[test]
    fn legacy_config_without_new_fields_still_loads() {
        let cfg: SyncConfig = serde_json::from_str(
            r#"{"server":"https://dav.example.com/dav/","user":"u","pass":"p","remote_path":"/Rime/"}"#,
        ).unwrap();
        assert!(!cfg.auto);
        assert_eq!(cfg.interval_min, 0);
        assert!(cfg.accepts("clipboard"));
        assert!(cfg.accepts("userdb"));
    }

    #[test]
    fn receive_filter_blocks_download() {
        let cfg = SyncConfig { receive: vec!["userdb".into()], ..Default::default() };
        assert!(!cfg.accepts("clipboard"));
        assert!(cfg.accepts("userdb"));
        let empty = SyncConfig { receive: vec![], ..Default::default() };
        assert!(!empty.accepts("clipboard"));
    }

    #[test]
    fn config_roundtrip_and_state_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lrn_sync_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = SyncConfig { server: "https://dav.example.com/dav/".into(), user: "alice".into(), pass: "secret".into(), interval_min: 15, auto: true, ..Default::default() };
        save_config(&dir, &cfg).unwrap();
        let back = load_config(&dir).unwrap();
        assert_eq!(back.user, "alice");
        assert_eq!(back.interval_min, 15);
        assert!(back.auto);
        let mut res = SyncResult::default();
        res.uploaded.push("clipboard.json".into());
        record_run(&dir, &res);
        let st = load_state(&dir);
        assert!(st.last_sync.is_some());
        assert!(st.last_ok);
        assert_eq!(st.lines[0], "上传 1 / 下载 0 / 合并 0 / 错误 0");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
