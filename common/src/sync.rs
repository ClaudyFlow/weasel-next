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

/// 坚果云 WebDAV 的**可写根**目录名。
///
/// 实测（2026-11-07 22:1x）：坚果云 `/dav/` 根下 MKCOL 能建出目录（201），但
/// `DELETE` 一律返回 **403 删不掉**，而且这些目录不在用户的「我的坚果云」同步目录里。
/// 用户的文件都在 `/dav/我的坚果云/` 下面，所以对坚果云要把远端路径自动挂到它下面。
/// 旧行为直接把 `remote_path`（默认 `/Rime/user_dict`）拼在 `/dav/` 后面 ⇒
/// 目录不存在 ⇒ `PROPFIND` 返回 409 ⇒ 面板「测试连接」报「连接失败（HTTP 409）」。
const JIANGUOYUN_MOUNT: &str = "我的坚果云";

/// 实际使用的远端目录（无首尾斜杠）。坚果云自动补「我的坚果云」前缀。
pub fn effective_remote_path(cfg: &SyncConfig) -> String {
    let raw = cfg.remote_path.trim().trim_matches('/');
    let path = if raw.is_empty() { "Rime/user_dict" } else { raw };
    if !cfg.server.contains("jianguoyun.com") {
        return path.to_string();
    }
    let already = path == JIANGUOYUN_MOUNT
        || path.starts_with(&format!("{JIANGUOYUN_MOUNT}/"));
    if already {
        path.to_string()
    } else {
        format!("{JIANGUOYUN_MOUNT}/{path}")
    }
}

/// 剪贴板在远端的**独立目录**（与用户词库目录并列）。
///
/// 2026-11-07 22:5x 用户问「为什么剪贴板不是单独的 rime/clipboard 英文路径」：
/// 旧实现把 `clipboard.json` 直接丢在用户词库目录里（`Rime/user_dict/clipboard.json`），
/// 词库文件和剪贴板混在一层。现在剪贴板单独挂到 `remote_path` 的**父级**下：
/// * 坚果云：`我的坚果云/Rime/clipboard`
/// * 通用 WebDAV：`<remote_path 的父级>/clipboard`
pub fn effective_clipboard_dir(cfg: &SyncConfig) -> String {
    let base = effective_remote_path(cfg);
    match base.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => format!("{parent}/clipboard"),
        _ => "clipboard".to_string(),
    }
}

/// 把相对目录串切成逐级绝对路径（`["/我的坚果云", "/我的坚果云/Rime", ...]`），供 MKCOL 用。
fn dir_chain(dir_rel: &str, extra: &[&str]) -> Vec<String> {
    let mut chain = Vec::new();
    let mut acc = String::new();
    for segment in dir_rel.split('/').filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(segment);
        chain.push(acc.clone());
    }
    let mut acc = chain.last().cloned().unwrap_or_default();
    for segment in extra.iter().filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(segment);
        chain.push(acc.clone());
    }
    chain
}

fn dir_url_of(cfg: &SyncConfig, dir_rel: &str) -> String {
    format!(
        "{}/{}",
        cfg.server.trim_end_matches('/'),
        dir_rel.trim_matches('/')
    )
}

fn file_url_in(cfg: &SyncConfig, dir_rel: &str, remote_name: &str) -> String {
    format!(
        "{}/{}",
        dir_url_of(cfg, dir_rel),
        remote_name.trim_start_matches('/')
    )
}

fn auth_header(cfg: &SyncConfig) -> String {
    format!("Basic {}", basic_auth(&cfg.user, &cfg.pass))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build()
}

/// 单级 MKCOL。目录已存在（405）/ 并发创建（409）都算成功；真失败返回原因。
fn mkcol(cfg: &SyncConfig, url: &str) -> Result<(), String> {
    match agent()
        .request("MKCOL", url)
        .set("Authorization", &auth_header(cfg))
        .call()
    {
        Ok(_) => Ok(()),
        // 405 = 目录已存在；409 = 父级/同名冲突（并发创建），都继续往下建。
        Err(ureq::Error::Status(405 | 409, _)) => Ok(()),
        Err(ureq::Error::Status(code @ (401 | 403), _)) => {
            Err(format!("建远端目录 {url}：账号或应用密码无写权限（HTTP {code}）"))
        }
        Err(error) => Err(format!("建远端目录 {url}：{error}")),
    }
}

/// 逐级建目录（`dir_rel` 的每一级 + `extra` 子目录）。
///
/// 父目录不存在时只建叶子会被服务器判 **409**（2026-11-07 实测：`userdb`
/// 下的 `<方案>.userdb/` 子目录没建，导致「立即同步」每条用户词库都 409）。
fn ensure_dirs_of(cfg: &SyncConfig, dir_rel: &str, extra: &[&str]) -> Result<(), String> {
    for path in dir_chain(dir_rel, extra) {
        let url = format!("{}{}", cfg.server.trim_end_matches('/'), path);
        mkcol(cfg, &url)?;
    }
    Ok(())
}

/// 建出某个远端目录本身（含全部父级）。
fn ensure_dir_of(cfg: &SyncConfig, dir_rel: &str) -> Result<(), String> {
    ensure_dirs_of(cfg, dir_rel, &[])
}

/// 远端文件需要的目录链（目录本身 + 文件所在子目录，如 `<方案>.userdb/`）。
fn remote_parent_chain(dir_rel: &str, remote_name: &str) -> Vec<String> {
    let segments: Vec<&str> = remote_name.split('/').filter(|s| !s.is_empty()).collect();
    let parents = &segments[..segments.len().saturating_sub(1)];
    dir_chain(dir_rel, parents)
}

/// 为某个远端文件逐级建出父目录（PUT 409 时用）。
fn ensure_parent_dirs(cfg: &SyncConfig, dir_rel: &str, remote_name: &str) -> Result<(), String> {
    for path in remote_parent_chain(dir_rel, remote_name) {
        let url = format!("{}{}", cfg.server.trim_end_matches('/'), path);
        mkcol(cfg, &url)?;
    }
    Ok(())
}

/// leveldb 的 `LOCK` 是引擎持有的锁文件，既读不出来（os error 33）也没必要同步。
fn skip_upload(name: &str) -> bool {
    name.eq_ignore_ascii_case("LOCK")
}

/// 测试连接：PROPFIND Depth 0 打远端目录；目录不存在（404/409）就逐级建出来再探测。
/// 401/403 会把「账号或密码不对」直接说清楚，不再笼统报一个网络错。
pub fn test_connection(cfg: &SyncConfig) -> Result<String, String> {
    let userdb_rel = effective_remote_path(cfg);
    let url = dir_url_of(cfg, &userdb_rel);
    let request = || {
        agent()
            .request("PROPFIND", &url)
            .set("Authorization", &auth_header(cfg))
            .set("Depth", "0")
            .call()
    };
    match request() {
        Ok(res) => Ok(format!("连接成功（HTTP {}）· {}", res.status(), url)),
        Err(ureq::Error::Status(401 | 403, _)) => Err("账号或应用密码不对（HTTP 401/403）".into()),
        // 404/409 = 远端目录还不存在（首次同步）。
        Err(ureq::Error::Status(404 | 409, _)) => {
            ensure_dir_of(cfg, &userdb_rel)?;
            match request() {
                Ok(res) => Ok(format!(
                    "连接成功（HTTP {}，已创建远端目录）· {}",
                    res.status(),
                    url
                )),
                Err(ureq::Error::Status(code, _)) => {
                    Err(format!("已尝试建目录，但探测仍失败（HTTP {code}）· {url}"))
                }
                Err(error) => Err(format!("已尝试建目录，但探测仍失败：{error} · {url}")),
            }
        }
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

fn webdav_put(cfg: &SyncConfig, dir_rel: &str, local_path: &Path, remote_name: &str) -> Result<(), String> {
    let data = std::fs::read(local_path).map_err(|e| format!("读 {}: {e}", local_path.display()))?;
    let url = file_url_in(cfg, dir_rel, remote_name);
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
            // 远端目录链（含文件所在子目录）不存在：逐级 MKCOL 后重试一次。
            ensure_parent_dirs(cfg, dir_rel, remote_name)?;
            match put() {
                Ok(_) => Ok(()),
                Err(e) => Err(format!("PUT {remote_name}: {e}")),
            }
        }
        Err(e) => Err(format!("PUT {remote_name}: {e}")),
    }
}

/// 下载成字节（leveldb 是二进制，不能用 `into_string`）。
fn webdav_get(cfg: &SyncConfig, dir_rel: &str, remote_name: &str) -> Result<Vec<u8>, String> {
    let url = file_url_in(cfg, dir_rel, remote_name);
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
    // 剪贴板单独一个远端目录（`Rime/clipboard`）；用户词库仍在 `remote_path`（`Rime/user_dict`）。
    let clip_dir = effective_clipboard_dir(cfg);
    let userdb_rel = effective_remote_path(cfg);

    // 1. 剪贴板 JSON：上传 + 合并下载（不再像旧版那样丢进 sync/ 就完事）。
    let clip_path = user_root.join("clipboard.json");
    if clip_path.exists() {
        match webdav_put(cfg, &clip_dir, &clip_path, "clipboard.json") {
            Ok(()) => result.uploaded.push("clipboard.json".into()),
            Err(e) => result.errors.push(e),
        }
    }
    if cfg.accepts("clipboard") {
        match webdav_get(cfg, &clip_dir, "clipboard.json") {
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
                    if skip_upload(&name) {
                        continue;
                    }
                    let remote = format!("{db_name}/{name}");
                    match webdav_put(cfg, &userdb_rel, &path, &remote) {
                        Ok(()) => result.uploaded.push(remote.clone()),
                        Err(e) => result.errors.push(e),
                    }
                    if cfg.accepts("userdb") {
                        match webdav_get(cfg, &userdb_rel, &remote) {
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
        let base = effective_remote_path(&cfg);
        assert_eq!(dir_url_of(&cfg, &base), "https://dav.example.com/dav/Rime/user_dict");
        assert_eq!(
            file_url_in(&cfg, &base, "clipboard.json"),
            "https://dav.example.com/dav/Rime/user_dict/clipboard.json"
        );
        assert_eq!(
            file_url_in(&cfg, &base, "luna/LOG"),
            "https://dav.example.com/dav/Rime/user_dict/luna/LOG"
        );
        // 剪贴板有自己的目录：`Rime/clipboard`（与 user_dict 并列，2026-11-07 22:5x 用户口径）。
        assert_eq!(
            file_url_in(&cfg, &effective_clipboard_dir(&cfg), "clipboard.json"),
            "https://dav.example.com/dav/Rime/clipboard/clipboard.json"
        );
    }

    #[test]
    fn clipboard_gets_its_own_dir_beside_user_dict() {
        // 2026-11-07 22:5x 用户问「为什么剪贴板不是单独的 rime/clipboard 英文路径」：
        // 剪贴板必须和用户词库目录分开，且名字用英文 `clipboard`。
        let jgy = SyncConfig {
            server: "https://dav.jianguoyun.com/dav/".into(),
            remote_path: "/Rime/user_dict".into(),
            ..Default::default()
        };
        assert_eq!(effective_remote_path(&jgy), "我的坚果云/Rime/user_dict");
        assert_eq!(effective_clipboard_dir(&jgy), "我的坚果云/Rime/clipboard");
        assert_eq!(
            dir_url_of(&jgy, &effective_clipboard_dir(&jgy)),
            "https://dav.jianguoyun.com/dav/我的坚果云/Rime/clipboard"
        );
        // MKCOL 链：父级逐级建（「我的坚果云」→ Rime → clipboard）。
        assert_eq!(
            dir_chain(&effective_clipboard_dir(&jgy), &[]),
            vec![
                "/我的坚果云".to_string(),
                "/我的坚果云/Rime".to_string(),
                "/我的坚果云/Rime/clipboard".to_string(),
            ]
        );
        // 通用 WebDAV 同理：`/Rime/user_dict` → `/Rime/clipboard`。
        let generic = SyncConfig {
            server: "https://dav.example.com/dav/".into(),
            remote_path: "/Rime/user_dict/".into(),
            ..Default::default()
        };
        assert_eq!(effective_clipboard_dir(&generic), "Rime/clipboard");
        // 单段 remote_path（没有父级）⇒ 直接挂服务器根下的 clipboard。
        let flat = SyncConfig {
            server: "https://dav.example.com/dav/".into(),
            remote_path: "/sync".into(),
            ..Default::default()
        };
        assert_eq!(effective_clipboard_dir(&flat), "clipboard");
        // 坚果云已手写「我的坚果云」前缀时同样只补 clipboard。
        let manual = SyncConfig { remote_path: "/我的坚果云/Rime/user_dict".into(), ..jgy };
        assert_eq!(effective_clipboard_dir(&manual), "我的坚果云/Rime/clipboard");
    }

    #[test]
    fn jianguoyun_paths_are_mounted_under_my_jianguoyun() {
        // 2026-11-07 22:1x 实测：`/dav/Rime/user_dict` 在坚果云不存在（PROPFIND 返回 409，
        // 面板「测试连接」就报「连接失败（HTTP 409）」）；而 `/dav/` 根下 MKCOL 建出来的
        // 目录 DELETE 一律 403 删不掉 ⇒ 远端路径必须挂到「我的坚果云」下面。
        let cfg = SyncConfig {
            server: "https://dav.jianguoyun.com/dav/".into(),
            remote_path: "/Rime/user_dict".into(),
            ..Default::default()
        };
        let base = effective_remote_path(&cfg);
        assert_eq!(base, "我的坚果云/Rime/user_dict");
        assert_eq!(
            dir_url_of(&cfg, &base),
            "https://dav.jianguoyun.com/dav/我的坚果云/Rime/user_dict"
        );
        assert_eq!(
            dir_chain(&base, &[]),
            vec![
                "/我的坚果云".to_string(),
                "/我的坚果云/Rime".to_string(),
                "/我的坚果云/Rime/user_dict".to_string(),
            ]
        );
        // 已经手写成带「我的坚果云」的路径：不要重复加前缀。
        let cfg = SyncConfig { remote_path: "/我的坚果云/Rime".into(), ..cfg };
        assert_eq!(effective_remote_path(&cfg), "我的坚果云/Rime");
        assert_eq!(effective_clipboard_dir(&cfg), "我的坚果云/clipboard");
    }

    #[test]
    fn generic_webdav_remote_path_is_kept_as_is() {
        let cfg = SyncConfig {
            server: "https://dav.example.com/dav/".into(),
            remote_path: "/Rime/user_dict/".into(),
            ..Default::default()
        };
        let base = effective_remote_path(&cfg);
        assert_eq!(base, "Rime/user_dict");
        assert_eq!(dir_url_of(&cfg, &base), "https://dav.example.com/dav/Rime/user_dict");
        assert_eq!(
            dir_chain(&base, &[]),
            vec!["/Rime".to_string(), "/Rime/user_dict".to_string()]
        );
    }

    #[test]
    fn userdb_subdir_is_in_the_mkcol_chain() {
        // 2026-11-07 22:29 实测：`<方案>.userdb/` 子目录没建好 ⇒ 每个用户词库文件
        // PUT 都 409（「立即同步」报「错误 73」）。父目录必须一起逐级建出来。
        let jgy = SyncConfig {
            server: "https://dav.jianguoyun.com/dav/".into(),
            remote_path: "/Rime/user_dict".into(),
            ..Default::default()
        };
        let base = effective_remote_path(&jgy);
        assert_eq!(
            remote_parent_chain(&base, "luna_pinyin.userdb/000003.log"),
            vec![
                "/我的坚果云".to_string(),
                "/我的坚果云/Rime".to_string(),
                "/我的坚果云/Rime/user_dict".to_string(),
                "/我的坚果云/Rime/user_dict/luna_pinyin.userdb".to_string(),
            ]
        );
        // 落在目录根的文件（clipboard.json 旧位置）不该多建子目录。
        assert_eq!(remote_parent_chain(&base, "clipboard.json"), dir_chain(&base, &[]));
        // 通用 WebDAV 同理，且带中文的方案目录不能丢。
        let generic = SyncConfig {
            server: "https://dav.example.com/dav/".into(),
            remote_path: "/Rime/user_dict/".into(),
            ..Default::default()
        };
        let base = effective_remote_path(&generic);
        assert_eq!(
            remote_parent_chain(&base, "自学习锁-自学习-tiger_sentence.userdb/LOG").last().unwrap(),
            "/Rime/user_dict/自学习锁-自学习-tiger_sentence.userdb"
        );
    }

    #[test]
    fn leveldb_lock_file_is_not_uploaded() {
        assert!(skip_upload("LOCK"));
        assert!(skip_upload("lock"));
        assert!(!skip_upload("CURRENT"));
        assert!(!skip_upload("LOG"));
        assert!(!skip_upload("000003.log"));
        assert!(!skip_upload("MANIFEST-000001"));
        assert!(!skip_upload("clipboard.json"));
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
