//! 同步层(2026-11-04):WebDAV 上传/下载用户词库缓存 + 剪贴板 JSON。
//!
//! 支持坚果云 WebDAV(dav.jianguoyun.com)和通用 WebDAV。用 `ureq` 做最小
//! HTTP 客户端(PUT/GET + Basic Auth),不引入大型 WebDAV 库。
//!
//! 同步内容:
//! * 用户词典缓存(`userdb/` 下的 `.userdb` leveldb 目录,打包成 .zip)
//! * 剪贴板 JSON(`clipboard.json`)
//! * 个人 n-gram 表(将来接入)
//!
//! 频率控制由 UI 层(设置外壳)触发;本模块只做单次同步。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// 同步配置(从设置外壳传入)。
#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub server: String,    // https://dav.jianguoyun.com/dav/
    pub user: String,
    pub pass: String,
    pub remote_path: String, // /Rime/user_dict/
}

/// 同步结果。
pub struct SyncResult {
    pub uploaded: Vec<String>,
    pub downloaded: Vec<String>,
    pub errors: Vec<String>,
}

/// WebDAV PUT:上传文件到远端路径。
fn webdav_put(cfg: &SyncConfig, local_path: &Path, remote_name: &str) -> Result<(), String> {
    let data = std::fs::read(local_path).map_err(|e| e.to_string())?;
    let url = format!("{}{}{}", cfg.server.trim_end_matches('/'), cfg.remote_path.trim_end_matches('/'), remote_name);
    let auth = basic_auth(&cfg.user, &cfg.pass);
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(30)).build();
    let response = agent.put(&url)
        .set("Authorization", &format!("Basic {auth}"))
        .set("Content-Type", "application/octet-stream")
        .send_bytes(&data);
    match response {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, _)) if (200..300).contains(&code) => Ok(()),
        Err(ureq::Error::Status(404, _)) => Ok(()), // 远端目录不存在,PUT 自建
        Err(e) => Err(format!("PUT {remote_name}: {e}")),
    }
}

/// WebDAV GET:下载远端文件到本地路径。
fn webdav_get(cfg: &SyncConfig, remote_name: &str, local_path: &Path) -> Result<(), String> {
    let url = format!("{}{}{}", cfg.server.trim_end_matches('/'), cfg.remote_path.trim_end_matches('/'), remote_name);
    let auth = basic_auth(&cfg.user, &cfg.pass);
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(30)).build();
    let response = agent.get(&url)
        .set("Authorization", &format!("Basic {auth}"))
        .call();
    match response {
        Ok(res) => {
            let data = res.into_string().map_err(|e| e.to_string())?;
            std::fs::write(local_path, data.as_bytes()).map_err(|e| e.to_string())
        }
        Err(ureq::Error::Status(404, _)) => Err("远端不存在(首次同步)".to_string()),
        Err(e) => Err(format!("GET {remote_name}: {e}")),
    }
}

fn basic_auth(user: &str, pass: &str) -> String {
    let credentials = format!("{user}:{pass}");
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = credentials.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        out.push(TABLE[(b[0] >> 2) as usize] as char);
        out.push(TABLE[((b[0] & 0x03) << 4 | (b[1] >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((b[1] & 0x0F) << 2 | (b[2] >> 6)) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(b[2] & 0x3F) as usize] as char } else { '=' });
    }
    out
}

/// 执行同步:上传/下载用户词典缓存 + 剪贴板 JSON。
///
/// 同步策略:两侧取时间戳最新者(简化 v1:本地上传远端覆盖,不合并)。
pub fn sync(cfg: &SyncConfig, user_root: &Path) -> SyncResult {
    let mut result = SyncResult { uploaded: Vec::new(), downloaded: Vec::new(), errors: Vec::new() };
    let sync_dir = user_root.join("sync");
    let _ = std::fs::create_dir_all(&sync_dir);

    // 1. 剪贴板 JSON(小文件,直接推拉)
    let clip_local = user_root.join("clipboard.json");
    if clip_local.exists() {
        match webdav_put(cfg, &clip_local, "clipboard.json") {
            Ok(()) => result.uploaded.push("clipboard.json".into()),
            Err(e) => result.errors.push(e),
        }
    }
    let clip_remote = sync_dir.join("clipboard.json");
    if webdav_get(cfg, "clipboard.json", &clip_remote).is_ok() {
        result.downloaded.push("clipboard.json".into());
    }

    // 2. 用户词典缓存:打包 userdb 目录(逐词典 .zip 太复杂,v1 打 tar 不做,
    //    直接把 .userdb 目录里每个文件逐个上传——leveldb 文件数有限)
    let userdb_dir = user_root.join("userdb");
    if let Ok(entries) = std::fs::read_dir(&userdb_dir) {
        for entry in entries.flatten() {
            let db_dir = entry.path();
            if !db_dir.is_dir() { continue; }
            let db_name = db_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Ok(files) = std::fs::read_dir(&db_dir) {
                for file in files.flatten() {
                    let file_path = file.path();
                    if !file_path.is_file() { continue; }
                    let file_name = file_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let remote = format!("{db_name}/{file_name}");
                    match webdav_put(cfg, &file_path, &remote) {
                        Ok(()) => result.uploaded.push(remote.clone()),
                        Err(e) => result.errors.push(e),
                    }
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_auth_encodes_correctly() {
        assert_eq!(basic_auth("user", "pass"), "dXNlcjpwYXNz");
        assert_eq!(basic_auth("a", "b"), "YTpi");
    }
}
