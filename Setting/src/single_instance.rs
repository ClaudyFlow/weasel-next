//! 设置界面唯一化（用户 2026-10-28 要求：不许同时存在多个设置界面）。
//!
//! 第二个实例拿到 `AlreadyRunning` 后不创建窗口，而是把已在运行的那个窗口
//! 拉到前台，然后直接退出。
//!
//! 窗口查找用 `EnumWindows` 按「进程可执行文件路径」匹配，而不是
//! `FindWindowW` 按标题/类名：Slint 的窗口类名不是我们能钉死的常量，
//! 标题还会随语言/方案变化；按 exe 路径匹配在沙箱/不同完整性级别下也稳
//! （参照 `Setting/src/switcher_dialog.rs` 里找 broker 窗口的同一结论）。

use core::ffi::c_void;

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const SW_RESTORE: i32 = 9;

#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(
        callback: Option<unsafe extern "system" fn(*mut c_void, isize) -> i32>,
        param: isize,
    ) -> i32;
    fn IsWindowVisible(window: *mut c_void) -> i32;
    fn GetWindowThreadProcessId(window: *mut c_void, pid: *mut u32) -> u32;
    fn ShowWindow(window: *mut c_void, command: i32) -> i32;
    fn BringWindowToTop(window: *mut c_void) -> i32;
    fn SetForegroundWindow(window: *mut c_void) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    fn QueryFullProcessImageNameW(
        process: *mut c_void,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

struct Search {
    /// 小写化的当前 exe 全路径。
    target: String,
    found: *mut c_void,
}

/// 把已在运行的设置窗口拉到前台；找不到就返回 false（调用方照常退出）。
pub fn focus_existing() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut search = Search {
        target: exe.to_string_lossy().to_lowercase(),
        found: core::ptr::null_mut(),
    };
    unsafe {
        EnumWindows(
            Some(find_window),
            &mut search as *mut Search as isize,
        );
    }
    if search.found.is_null() {
        return false;
    }
    unsafe {
        ShowWindow(search.found, SW_RESTORE);
        BringWindowToTop(search.found);
        SetForegroundWindow(search.found);
    }
    true
}

unsafe extern "system" fn find_window(window: *mut c_void, param: isize) -> i32 {
    // SAFETY: `param` 是 `focus_existing` 里 `&mut Search` 的地址，调用期间一直有效。
    let search = unsafe { &mut *(param as *mut Search) };
    if unsafe { IsWindowVisible(window) } == 0 {
        return 1;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    if pid == 0 {
        return 1;
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return 1;
    }
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut size)
    } != 0;
    unsafe {
        let _ = CloseHandle(process);
    }
    if !ok || size as usize > buffer.len() {
        return 1;
    }
    let path = String::from_utf16_lossy(&buffer[..size as usize]).to_lowercase();
    if path == search.target {
        search.found = window;
        return 0;
    }
    1
}
