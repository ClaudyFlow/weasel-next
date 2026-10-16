//! 为原生主题提供共享的系统绑定、外观查询和屏幕几何工具。
//!
//! 本 crate 重导出主题 API，并集中维护多个原生主题共用的支持代码；它不负责主题注册、
//! RPC 路由或主题后端生命周期管理。
pub use weasel_theme_api as theme_api;
pub mod appearance;
pub mod bindings;
pub mod d2d_bindings;
pub mod presentation;
pub mod diagnostics {
    pub fn record(message: std::fmt::Arguments<'_>) {
        // 渲染器是 windows_subsystem = "windows"，没有 stderr —— eprintln 等于把原因扔掉。
        // 写到与渲染器日志同目录的 theme.log，主题侧的具体失败原因才看得见。
        use std::io::Write;
        let Some(dir) = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .map(|base| base.join("Rime-Next").join("Logs"))
        else {
            return;
        };
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("theme.log"))
        {
            let _ = writeln!(file, "Weasel-Next-Renderer: {message}");
        }
    }
}
