//! 执行打开设置、帮助和目录等 Shell 菜单操作。
//!
//! 这些动作在独立线程中处理，避免阻塞托盘消息循环；调用边界也确保它们不在文本输入处理器（TIP）中运行。
use crate::bindings::*;
use weasel_common::comrt::ComApartment;
use weasel_common::{
    command_menu::*,
    process::RuntimePaths,
    task_dialog::{TaskDialog, TaskDialogIcon},
};
use windows_strings::{HSTRING, PCWSTR, w};

/// 将菜单命令解析为可交给 Windows Shell 打开的目标。
///
/// 设置程序缺失时回退到用户目录；用户数据和日志目录会按需创建，程序目录则必须已经存在。
/// 路径发现、目录创建或不支持的命令以错误返回，调用方负责向用户展示。
fn target(command: u32) -> Result<HSTRING, String> {
    match command {
        SETTINGS | STATISTICS | SWITCHER | STYLE => {
            let paths = RuntimePaths::discover().map_err(|e| e.to_string())?;
            let executable = paths.executable_directory.join("Weasel-Next-Setting.exe");
            if !executable.is_file() {
                return target(USER_DIRECTORY);
            }
            Ok(HSTRING::from(executable.as_os_str()))
        }
        HELP => Ok(HSTRING::from("https://rime.im/docs/")),
        FORUM => Ok(HSTRING::from("https://rime.im/discuss/")),
        USER_DIRECTORY | PROGRAM_DIRECTORY | LOG_DIRECTORY => {
            let paths = RuntimePaths::discover().map_err(|e| e.to_string())?;
            let path = match command {
                USER_DIRECTORY => paths.user_data,
                LOG_DIRECTORY => paths.logs,
                _ => paths.executable_directory,
            };
            if command != PROGRAM_DIRECTORY {
                std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            }
            Ok(HSTRING::from(path.as_os_str()))
        }
        _ => Err("不支持的菜单命令".to_owned()),
    }
}

/// 设置程序的命令行参数：官方那几个对话框各自需要一个开关。
/// 不带参数会打开旧的设置界面，这正是"右键出来不是官方设置"的原因。
fn parameters(command: u32) -> Option<HSTRING> {
    match command {
        // 设置 = 主界面（**无参数**），不能传 /style —— 那是"界面风格"对话框。
        STYLE => Some(HSTRING::from("/style")),
        STATISTICS => Some(HSTRING::from("/stat")),
        SWITCHER => Some(HSTRING::from("/switcher")),
        _ => None,
    }
}

/// 在专用线程中打开菜单目标，并将启动线程或 Shell 失败显示为错误对话框。
///
/// 工作线程初始化单线程 COM apartment，完成 Shell 调用后再反初始化；本函数不等待打开操作结束。
pub fn open(command: u32) {
    let result = std::thread::Builder::new()
        .name("weasel-shell".into())
        .spawn(move || {
            let result = (|| -> Result<(), String> {
                let target = target(command)?;
                let _apartment = ComApartment::initialize_sta().map_err(|e| e.to_string())?;
                // ⚠️ 这里必须**真的把 `parameters()` 传下去**。原来 lpParameters 写的是 `None`，
                // 于是"设置"打开的是无参数界面（旧界面），而不是 `/style`；
                // "输入统计"更是压根进不去。这个 bug 一直藏着，因为参数函数本身写对了。
                let params = parameters(command);
                let params_ptr = match &params {
                    Some(value) => PCWSTR(value.as_ptr()),
                    None => PCWSTR::null(),
                };
                let result = unsafe {
                    ShellExecuteW(
                        None,
                        w!("open"),
                        PCWSTR(target.as_ptr()),
                        params_ptr,
                        None,
                        SW_SHOWNORMAL,
                    )
                };
                if result.0 as isize <= 32 {
                    return Err(format!("无法打开目标，Shell 错误码：{}", result.0 as isize));
                }
                Ok(())
            })();
            if let Err(error) = result {
                show_error(&error);
            }
        });
    if let Err(error) = result {
        show_error(&format!("无法启动菜单操作：{error}"));
    }
}

/// 使用前台错误对话框呈现菜单操作失败原因。
fn show_error(error: &str) {
    if let Err(dialog_error) = TaskDialog::new("小狼毫Next", "无法打开目标")
        .icon(TaskDialogIcon::Error)
        .content(error)
        .show()
    {
        eprintln!("Weasel-Next-Server: could not show menu error dialog: {dialog_error}");
    }
}
