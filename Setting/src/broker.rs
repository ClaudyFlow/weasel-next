//! 复用语言栏菜单的 broker 命令入口；只投递消息，不等待服务重启阻塞 UI。
use crate::bindings::*;
use weasel_common::command_menu;

/// 设置界面保存宿主级配置（`weasel.yaml`）后通知 broker 重读，毫秒级、不重启服务。
///
/// 与 D2D 外壳 `d2d_shell::request_settings_reload` 同款窗口消息路径；Slint 正式前端
/// 也走这条路，两前端热更新行为一致。按类名枚举而非 `FindWindowW`：后者在沙箱/
/// 不同完整性级别下实测会漏掉 broker 窗口（见 `switcher_dialog::request_deploy`）。
pub fn request_settings_reload() -> Result<(), String> {
    const WM_COMMAND: u32 = 0x0111;

    unsafe extern "system" {
        fn EnumWindows(
            callback: Option<unsafe extern "system" fn(*mut core::ffi::c_void, isize) -> i32>,
            param: isize,
        ) -> i32;
        fn GetClassNameW(window: *mut core::ffi::c_void, buffer: *mut u16, count: i32) -> i32;
        fn PostMessageW(
            window: *mut core::ffi::c_void,
            msg: u32,
            wparam: usize,
            lparam: isize,
        ) -> i32;
    }

    unsafe extern "system" fn look_for_broker(window: *mut core::ffi::c_void, param: isize) -> i32 {
        let mut buffer = [0u16; 64];
        let length = unsafe { GetClassNameW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
        if length > 0 {
            let class = String::from_utf16_lossy(&buffer[..length as usize]);
            if class == command_menu::BROKER_WINDOW_CLASS {
                unsafe { *(param as *mut *mut core::ffi::c_void) = window };
                return 0;
            }
        }
        1
    }

    let mut broker: *mut core::ffi::c_void = std::ptr::null_mut();
    unsafe {
        let _ = EnumWindows(Some(look_for_broker), &mut broker as *mut _ as isize);
        if broker.is_null() {
            return Err("算法服务管理程序未运行，请先启动算法服务。".into());
        }
        if PostMessageW(
            broker,
            WM_COMMAND,
            command_menu::RELOAD_SETTINGS as usize,
            0,
        ) == 0
        {
            return Err(windows_core::Error::from_thread().to_string());
        }
    }
    Ok(())
}

/// 向已运行的 broker 投递重启服务命令，不等待重启完成。
///
/// 找不到 broker 窗口或 Windows 拒绝投递时返回错误；成功仅表示消息已入队，
/// 不代表配置已应用或服务已重启。
pub fn request_restart() -> Result<(), String> {
    let class = windows_strings::HSTRING::from(command_menu::BROKER_WINDOW_CLASS);
    unsafe {
        let broker = FindWindowW(windows_core::PCWSTR(class.as_ptr()), None);
        if broker.0.is_null() {
            return Err("算法服务管理程序未运行，请先启动算法服务。".into());
        }
        if !PostMessageW(
            Some(broker),
            WM_COMMAND as u32,
            WPARAM(command_menu::RESTART as usize),
            LPARAM(0),
        )
        .as_bool()
        {
            return Err(windows_core::Error::from_thread().to_string());
        }
    }
    Ok(())
}
