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
    /// `theme.log` 的大小上限；超过就把旧的挪成 `theme.1.log`，重新开一份。
    ///
    /// 2026-10-30 实测：这个文件长到 **72 MB / 51.7 万行**（不轮转）。
    /// 排查要看的是"最近发生了什么"，不是"从装上那天起每一帧"，所以留 8 MB 足够。
    const MAX_BYTES: u64 = 8 * 1024 * 1024;

    /// 超过上限就轮转一次；失败（被别的进程占着、权限问题）一律忽略 ——
    /// 日志轮转绝不能影响写日志本身，更不能影响输入。
    fn rotate_if_huge(path: &std::path::Path) {
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        if meta.len() < MAX_BYTES {
            return;
        }
        let backup = path.with_file_name("theme.1.log");
        // 先删再改名：Windows 上 `rename` 覆盖已存在的文件会失败。
        let _ = std::fs::remove_file(&backup);
        let _ = std::fs::rename(path, &backup);
    }

    /// 无条件写一条诊断（真正的故障原因必须看得见）。
    pub fn record(message: std::fmt::Arguments<'_>) {
        // 渲染器是 windows_subsystem = "windows"，没有 stderr —— eprintln 等于把原因扔掉。
        // 写到与渲染器日志同目录的 theme.log，主题侧的具体失败原因才看得见。
        use std::io::Write;
        // 2026-11-10：`create_dir_all` 原来每条日志都跑一次（内部至少一次目录 stat +
        // 一次创建尝试）。实测每条 `record()` 的 open+append+close 就要 0.386ms，
        // 目录检查再叠上去是白给。改成进程内只做一次；每 4096 条顺手自愈一次，
        // 用户中途删掉 Logs 目录也还能重新写回来。
        static LOG_DIR: std::sync::OnceLock<Option<std::path::PathBuf>> =
            std::sync::OnceLock::new();
        let Some(dir) = LOG_DIR.get_or_init(|| -> Option<std::path::PathBuf> {
            let dir = std::env::var_os("LOCALAPPDATA")
                .map(std::path::PathBuf::from)
                .map(|base| base.join("Rime-Next").join("Logs"))?;
            let _ = std::fs::create_dir_all(&dir);
            Some(dir)
        }) else {
            return;
        };
        let path = dir.join("theme.log");
        // 每 4096 条才 stat 一次文件大小：每次写入都 stat 会把"省下来的每键开销"
        // 又还回去。计数器溢出回 0 时多 stat 一次，无所谓。
        static WRITES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if WRITES
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .is_multiple_of(4096)
        {
            let _ = std::fs::create_dir_all(dir);
            rotate_if_huge(&path);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(file, "Weasel-Next-Renderer: {message}");
        }
    }

    /// 逐帧布局明细（默认关闭；`WEASEL_THEME_LOG=1` 打开）。
    ///
    /// ⚠️ 2026-10-29 实测：每次布局记 1 行面板 + N 行候选矩形，每次 `record()` 都要
    /// `open+append+close`（实测 0.386ms），一屏 5 行候选就是 6~8 次落盘、约 3ms/键，
    /// 而且 `theme.log` 已经长到 **72 MB / 517,778 行**（不轮转）。这是"打字卡"里
    /// 可去掉的一笔固定开销，也顺手止住了日志无限增长。
    ///
    /// 2026-11-10 扩到动画/悬停：live `theme.log` 里 `动画帧 t=` 17731 行、
    /// `悬停:` 13095 行，都是 8ms 一帧或鼠标抖动时**无条件**落盘的逐帧明细。
    /// 现在一律走这里，默认零写盘；排查时 `WEASEL_THEME_LOG=1` 全量恢复。
    pub fn record_verbose(message: std::fmt::Arguments<'_>) {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ON
            .get_or_init(|| verbose_switch_from(std::env::var("WEASEL_THEME_LOG").ok().as_deref()))
        {
            return;
        }
        record(message);
    }

    /// 逐帧开关的纯判定，便于单测（进程级 `OnceLock` 无法在测试里重置）。
    /// 未设置或显式 `0/off/false/""` 关闭，其余任何值打开。
    pub fn verbose_switch_from(raw: Option<&str>) -> bool {
        !matches!(
            raw.map(str::trim),
            None | Some("") | Some("0") | Some("off") | Some("OFF") | Some("false") | Some("FALSE")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::diagnostics::verbose_switch_from;

    #[test]
    fn verbose_log_defaults_off_and_only_explicit_off_values_disable() {
        for off in [
            None,
            Some(""),
            Some("0"),
            Some("off"),
            Some("OFF"),
            Some("false"),
        ] {
            assert!(!verbose_switch_from(off), "{off:?} 必须关闭逐帧日志");
        }
        for on in [Some("1"), Some("true"), Some("yes"), Some(" 1 ")] {
            assert!(verbose_switch_from(on), "{on:?} 必须打开逐帧日志");
        }
    }
}
