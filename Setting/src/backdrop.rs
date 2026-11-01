//! Slint 可能在事件循环开始后才创建 HWND，必须等待原生窗口就绪。
//! 不将 show() 返回后暂时没有 HWND 误判为材质失败；无需轮询或常驻 Hook。
use crate::{SettingsWindow, bindings::*};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, winit_030::WinitWindowAccessor};

/// 把玻璃化每一步的结果写进日志文件。
///
/// 2026-10-31：`eprintln!` 在 GUI 里**没有去处**，DWM 调用失败时界面上也完全看不出来
/// （根背景会退回纯白），于是「设置界面到底做没做玻璃」只能靠猜。这里落盘到
/// `%LOCALAPPDATA%\Rime-Next\Logs\setting-backdrop.log`，失败原因可查。
fn note(message: &str) {
    let Ok(dir) = std::env::var("LOCALAPPDATA") else {
        return;
    };
    let logs = std::path::Path::new(&dir).join("Rime-Next").join("Logs");
    let _ = std::fs::create_dir_all(&logs);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs.join("setting-backdrop.log"))
    {
        use std::io::Write;
        let _ = writeln!(file, "{stamp} {message}");
    }
}

/// 当前系统是否支持主窗口系统材质所需的 Windows 版本。
pub fn supported() -> bool {
    let version = windows_version::OsVersion::current();
    let ok = version >= windows_version::OsVersion::new(10, 0, 0, 22621);
    note(&format!(
        "supported() 版本={version:?} build={} -> {ok}",
        version.build
    ));
    ok
}
/// 在 Slint 事件循环中等待原生窗口句柄并异步应用 Mica 材质。
///
/// 任务仅持有弱窗口句柄；窗口销毁或调度、窗口句柄、DWM 初始化失败时关闭
/// Slint 的材质标志并记录错误，不阻塞调用线程。
pub fn apply(ui: &SettingsWindow) {
    if !supported() {
        return;
    }
    let weak = ui.as_weak();
    if let Err(error) = slint::spawn_local(async move {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let result = match ui.window().winit_window().await {
            Ok(window) => configure(&window),
            Err(error) => {
                note(&format!("apply: 拿不到 winit 窗口: {error}"));
                Err(error.to_string())
            }
        };
        match result {
            Ok(()) => {
                note("apply: 材质已应用，mica-enabled=true");
                ui.set_mica_enabled(true)
            }
            Err(error) => {
                note(&format!("apply: 材质失败，退回纯白: {error}"));
                ui.set_mica_enabled(false);
            }
        }
    }) {
        note(&format!("apply: 任务调度失败: {error}"));
        ui.set_mica_enabled(false);
    }
}

/// 将 DWM 扩展帧和系统背景材质应用到 Win32 窗口，再安装自定义框架处理。
///
/// 非 Win32 句柄或 DWM 调用失败会返回错误；自定义框架安装失败只记录日志，材质
/// 配置仍视为成功，因为它不影响已完成的 DWM 设置。
fn configure(window: &slint::winit_030::winit::window::Window) -> Result<(), String> {
    // 界面底色全部写死浅色（#ffffff 卡片、#f4f5f7 页面），所以调色板必须固定在浅色。
    // 之前是「跟随系统」：系统切到深色后 Palette.foreground 变白，
    // 设置页文字变成白底白字、整页看着是空的（2026-10-19 修）。
    window.set_theme(Some(slint::winit_030::winit::window::Theme::Light));
    let handle = window.window_handle().map_err(|e| e.to_string())?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        note("configure: 非 Win32 窗口句柄");
        return Err("非 Win32 窗口".into());
    };
    let hwnd = HWND(handle.hwnd.get() as *mut core::ffi::c_void);
    note(&format!("configure: hwnd={:?}", handle.hwnd.get()));
    unsafe {
        // 深浅色由 Winit 跟随系统更新，Slint 使用默认系统调色板。
        // 原生 DWM 标题栏按钮需要整个客户区属于扩展帧。
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins)
            .ok()
            .map_err(|e| {
                note(&format!("configure: DwmExtendFrameIntoClientArea 失败: {e}"));
                e.to_string()
            })?;
        note("configure: 扩展帧 ok");
        // 本机系统是深色，DWM 默认给深色亚克力/Mica；设置界面是浅色 UI，
        // 必须显式把窗口标成浅色，否则玻璃透出来是黑的（2026-10-28 实测）。
        // 先设浅色再设材质，DWM 才会用浅色那一套。
        let light: i32 = 0;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &light as *const _ as _,
            core::mem::size_of_val(&light) as u32,
        )
        .ok();
        // 亚克力（Acrylic）：浅色下是毛玻璃；失败回退 Mica。片头（标题/标签栏）半透明，内容区不透明。
        let backdrop = DWMSBT_TRANSIENTWINDOW as i32;
        if DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            &backdrop as *const _ as _,
            core::mem::size_of_val(&backdrop) as u32,
        )
        .ok()
        .is_err()
        {
            note("configure: 亚克力(TRANSIENTWINDOW) 设置失败，回退 Mica");
            let mica = DWMSBT_MAINWINDOW as i32;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE as u32,
                &mica as *const _ as _,
                core::mem::size_of_val(&mica) as u32,
            )
            .ok()
            .map_err(|e| {
                note(&format!("configure: Mica(MAINWINDOW) 也失败: {e}"));
                e.to_string()
            })?;
            note("configure: Mica 回退成功");
        } else {
            note("configure: 亚克力(TRANSIENTWINDOW) ok");
        }
    }
    // 扩展帧和材质就绪后再触发 SWP_FRAMECHANGED，让 DWM 重算按钮布局。
    if let Err(error) = crate::frame::install(hwnd) {
        note(&format!("configure: 自定义框架失败（不影响材质）: {error}"));
    }
    Ok(())
}
