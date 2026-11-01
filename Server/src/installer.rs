//! 安装后的部署和当前用户自启动项维护在交互用户上下文中执行，避免提升权限的
//! 安装进程直接改写用户服务状态。

use std::{path::PathBuf, process::Command};

use windows_registry::CURRENT_USER;

/// 当前用户登录时运行 broker 的注册表键。
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// broker 自启动命令使用的注册表值名称。
const RUN_VALUE: &str = "小狼毫Next算法服务";

/// 获取当前 broker 可执行文件所在目录。
///
/// 若当前可执行文件没有父目录则返回错误；该目录也是安装后服务命令的工作目录。
fn executable_directory() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(std::env::current_exe()?
        .parent()
        .ok_or("broker executable has no parent directory")?
        .to_owned())
}

/// 构造带引号的 broker 自启动命令，确保含空格的安装路径作为单一路径解析。
fn startup_command(directory: &std::path::Path) -> String {
    format!("\"{}\"", directory.join("Weasel-Next-Server.exe").display())
}

/// 判断自启动命令是不是「本产品自己的 broker」。
///
/// 用于区分用户手改的值与**旧版本目录**里的值：版本化安装后，从旧版本升级时
/// Run 键仍指着 `...\Weasel-Next-<旧版本>\Weasel-Next-Server.exe`，必须改指新版本，
/// 否则重启后又把旧版本拉起来（2026-10-28 用户报的“重复安装”问题之一）。
fn is_own_broker_command(command: &str) -> bool {
    let command = command.trim();
    let path = command
        .strip_prefix('"')
        .and_then(|rest| rest.split('"').next())
        .unwrap_or(command);
    std::path::Path::new(path)
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("Weasel-Next-Server.exe"))
}

/// 执行安装后部署，并登记/更新 broker 自启动项。
///
/// 部署失败会中止并返回错误。自启动项不存在时写入；已指向**其他版本目录**的
/// Weasel-Next-Server.exe 时更新到当前版本；用户手改的其他命令保持不动。
pub(crate) fn post_install() -> Result<(), Box<dyn std::error::Error>> {
    let directory = executable_directory()?;
    let status = Command::new(directory.join("Weasel-Next-Engine.exe"))
        .args(["--deploy", "--silent"])
        .current_dir(&directory)
        .status()?;
    if !status.success() {
        return Err(format!("Rime deployment failed with {status}").into());
    }

    let run = CURRENT_USER.create(RUN_KEY)?;
    let expected = startup_command(&directory);
    match run.get_string(RUN_VALUE) {
        Ok(existing) if existing == expected => {}
        Ok(existing) if is_own_broker_command(&existing) => {
            // 旧版本目录（或旧的非版本化目录）→ 切到当前版本。
            run.set_string(RUN_VALUE, expected)?;
        }
        // 用户自定义的启动命令：保留，不覆盖。
        Ok(_) => {}
        Err(_) => {
            run.set_string(RUN_VALUE, expected)?;
        }
    }
    Ok(())
}

/// 移除指向当前安装目录的自启动项。
///
/// 注册表键不存在时视为已清理；若同名值已被用户改成其他命令则保留该值。
pub(crate) fn post_uninstall() -> Result<(), Box<dyn std::error::Error>> {
    let directory = executable_directory()?;
    let Ok(run) = CURRENT_USER.open(RUN_KEY) else {
        return Ok(());
    };
    let expected = startup_command(&directory);
    if run.get_string(RUN_VALUE).ok().as_deref() == Some(expected.as_str()) {
        run.remove_value(RUN_VALUE)?;
    }
    Ok(())
}
