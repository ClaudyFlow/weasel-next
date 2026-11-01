//! Windows 可执行组件共用的进程路径与生命周期原语。

use std::{
    io,
    path::{Path, PathBuf},
};

use crate::bindings::{
    CloseHandle, CreateMutexW, ERROR_ALREADY_EXISTS, ERROR_INVALID_PARAMETER, GetLastError, HANDLE,
    SW_SHOWNORMAL, ShellExecuteW,
};
use crate::windows_security::{LocalSecurityDescriptor, RuntimeIdentity};
use windows_strings::{HSTRING, PCWSTR, w};

/// 使用 Windows Shell 的默认关联程序打开绝对文件或目录路径。
pub fn open_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Shell target must be an absolute path",
        ));
    }
    let target = HSTRING::from(path.as_os_str());
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(target.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as isize;
    if code <= 32 {
        return Err(io::Error::other(format!(
            "ShellExecuteW failed with shell error {code}"
        )));
    }
    Ok(())
}

/// 返回**本代码所在模块**（EXE 或 DLL）的文件路径。
///
/// ⚠️ 不能用 `std::env::current_exe()` 代替：TIP 是**进程内 DLL**，跑在宿主进程里
/// （记事本、explorer、WPS…），`current_exe()` 给的是**宿主**的 exe。那样
/// `<安装目录>\userdir.txt` 永远读不到，用户自选的用户文件夹在 TIP 侧会静默回退到
/// `%APPDATA%\Rime-Next`（打字统计就写到别的地方去了）。
///
/// 这里用 `GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS)` 取**本函数所在模块**：
/// 静态链进 exe 时得到那个 exe，链进 DLL 时得到那个 DLL —— 两种形态都落在安装目录。
fn current_module_path() -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;

        const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x0000_0004;
        const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x0000_0002;
        /// 长路径上限（GetModuleFileNameW 的缓冲区上限）。
        const PATH_CEILING: usize = 32_768;

        unsafe extern "system" {
            fn GetModuleHandleExW(
                flags: u32,
                module_name: *const u16,
                module: *mut *mut core::ffi::c_void,
            ) -> i32;
            fn GetModuleFileNameW(
                module: *mut core::ffi::c_void,
                filename: *mut u16,
                size: u32,
            ) -> u32;
        }

        // 取一个本模块内的真实地址作为 "模块标识"。函数指针转地址是安全的：
        // 只用于查询，不会被解引用为字符串。
        let address = current_module_path as *const () as *const u16;
        let mut module: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: 地址来自本进程内的真实函数；UNCHANGED_REFCOUNT 表示不增加引用计数，
        // 查询完也不释放。
        let ok = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                address,
                &mut module,
            )
        };
        if ok != 0 && !module.is_null() {
            let mut buffer = vec![0u16; 260];
            loop {
                // SAFETY: buffer 是长度为 buffer.len() 的可写缓冲区。
                let length =
                    unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) }
                        as usize;
                if length == 0 {
                    break;
                }
                if length < buffer.len() {
                    buffer.truncate(length);
                    return Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer)));
                }
                if buffer.len() >= PATH_CEILING {
                    break;
                }
                // 返回值等于容量 ⇒ 路径被截断，扩大缓冲区重试。
                buffer.resize((buffer.len() * 2).min(PATH_CEILING), 0);
            }
        }
    }
    std::env::current_exe()
}

/// 返回当前程序（本组件）所在目录：EXE 所在目录，或调用方 DLL 所在目录。
///
/// 结果不受进程当前工作目录影响；DLL 形态（TIP/主题）解析到的是**安装目录**，
/// 而不是宿主进程的目录。
///
/// # Errors
///
/// 无法取得模块路径，或该路径没有父目录时返回错误。
pub fn executable_directory() -> io::Result<PathBuf> {
    current_module_path()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("executable has no parent directory"))
}

/// 用户文件夹指针文件名：放在**程序目录**里，内容是用户选的绝对路径（一行）。
///
/// 安装器（`installer/weasel-next.nsi` 的「用户文件夹」页）在用户改了默认位置时写这个文件；
/// 选默认位置时不写 —— 默认行为仍然是 `%APPDATA%\Rime-Next`，老机器上没有这个文件也照旧。
pub const USER_DIR_POINTER_FILE: &str = "userdir.txt";

/// 环境变量覆盖：优先级最高，给排障/测试用（`WEASEL_NEXT_USER_DIR=D:\Rime`）。
pub const USER_DIR_ENV: &str = "WEASEL_NEXT_USER_DIR";

/// 解析 Rime 用户文件夹，优先级：
///
/// 1. 环境变量 [`USER_DIR_ENV`]（绝对路径才认）；
/// 2. 程序目录里的指针文件 [`USER_DIR_POINTER_FILE`]（安装时选的）；
/// 3. `%APPDATA%\Rime-Next`（历史默认）。
///
/// 只做路径计算，不创建目录。返回 `None` 表示连 `APPDATA` 都没有（异常环境）。
pub fn user_data_dir() -> Option<PathBuf> {
    let executable = executable_directory().ok();
    resolve_user_data_dir(
        executable.as_deref(),
        std::env::var_os(USER_DIR_ENV).map(PathBuf::from),
        std::env::var_os("APPDATA").map(PathBuf::from),
    )
}

/// [`user_data_dir`] 的纯函数版本：输入全部显式给出，便于测试。
pub fn resolve_user_data_dir(
    executable_directory: Option<&Path>,
    environment_override: Option<PathBuf>,
    appdata: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(dir) = environment_override.filter(|path| path.is_absolute()) {
        return Some(dir);
    }
    if let Some(dir) = executable_directory.and_then(read_user_dir_pointer) {
        return Some(dir);
    }
    appdata
        .filter(|path| path.is_absolute())
        .map(|path| path.join("Rime-Next"))
}

/// 读程序目录里的 `userdir.txt`：第一行、去空白、必须是绝对路径，否则忽略。
///
/// 安装目录是管理员写的，但用户在设置里也可能手改这个文件；解析失败一律回退默认，
/// 不让一个坏文件把输入法变成"找不到用户数据"。
fn read_user_dir_pointer(executable_directory: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(executable_directory.join(USER_DIR_POINTER_FILE)).ok()?;
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let path = PathBuf::from(line);
    path.is_absolute().then_some(path)
}

/// 当前进程使用的程序、用户数据和日志目录。
///
/// 用户数据位于漫游配置目录，日志位于本地应用数据目录。此结构只保存路径，
/// 不代表目录已经存在；需要目录时应调用 [`RuntimePaths::ensure`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    /// 当前组件可执行文件所在目录。
    pub executable_directory: PathBuf,
    /// Rime 用户数据目录。
    pub user_data: PathBuf,
    /// 组件日志目录。
    pub logs: PathBuf,
}

impl RuntimePaths {
    /// 根据当前可执行文件位置和进程环境解析运行时目录。
    ///
    /// 本方法只计算目录，不创建目录，并要求 `APPDATA` 和 `LOCALAPPDATA`
    /// 环境变量提供绝对路径。
    ///
    /// # Errors
    ///
    /// 无法定位可执行文件，或缺少有效的用户目录环境变量时返回错误。
    pub fn discover() -> io::Result<Self> {
        Self::select(
            executable_directory()?,
            std::env::var_os("APPDATA").map(PathBuf::from),
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        )
    }

    /// 根据明确提供的输入纯计算目录布局。
    ///
    /// 用户文件夹走 [`resolve_user_data_dir`]：环境变量 > 程序目录 `userdir.txt` >
    /// `%APPDATA%\Rime-Next`。安装器让用户自选的位置就是通过 `userdir.txt` 传进来的。
    fn select(
        executable_directory: PathBuf,
        appdata: Option<PathBuf>,
        local_appdata: Option<PathBuf>,
    ) -> io::Result<Self> {
        let user_data = resolve_user_data_dir(
            Some(&executable_directory),
            std::env::var_os(USER_DIR_ENV).map(PathBuf::from),
            appdata,
        )
        .ok_or_else(|| io::Error::other("APPDATA must be set to an absolute directory"))?;
        let logs = local_appdata
            .filter(|path| path.is_absolute())
            .ok_or_else(|| io::Error::other("LOCALAPPDATA must be set to an absolute directory"))?
            .join("Rime-Next")
            .join("Logs");
        Ok(Self {
            executable_directory,
            user_data,
            logs,
        })
    }

    /// 创建用户数据目录和日志目录（若它们尚不存在）。
    ///
    /// # Errors
    ///
    /// 创建任一目录失败时返回底层 I/O 错误。
    pub fn ensure(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.user_data)?;
        std::fs::create_dir_all(&self.logs)
    }
}

/// 表示当前组件持有单实例互斥体的 RAII 守卫。
///
/// 该类型拥有底层 Win32 句柄，并在丢弃时关闭句柄。互斥体只用于检测对象是否
/// 已存在，不由任何线程取得所有权，因此释放时无需调用 `ReleaseMutex`。
pub struct SingleInstance {
    handle: HANDLE,
}

/// 获取单实例互斥体时可能发生的错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleInstanceError {
    /// 同一用户登录会话中已有该组件实例。
    AlreadyRunning,
    /// 创建互斥体或准备其安全属性失败，包含对应的 Win32 错误码。
    CreateFailed(u32),
}

impl std::fmt::Display for SingleInstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("another instance is already running"),
            Self::CreateFailed(error) => write!(f, "CreateMutexW failed with Win32 error {error}"),
        }
    }
}

impl std::error::Error for SingleInstanceError {}

impl SingleInstance {
    /// 根据当前进程的 Windows 用户和登录会话身份获取组件互斥体。
    ///
    /// `component` 会成为内核对象名称的一部分，必须符合
    /// [`RuntimeIdentity::mutex_name`] 的命名约束。
    ///
    /// # Errors
    ///
    /// 若无法读取当前进程身份、构造安全描述符或创建互斥体，则返回
    /// [`SingleInstanceError::CreateFailed`]；若对象已存在，则返回
    /// [`SingleInstanceError::AlreadyRunning`]。
    pub fn acquire(component: &str) -> Result<Self, SingleInstanceError> {
        let identity = RuntimeIdentity::current().map_err(platform_error)?;
        Self::acquire_for(&identity, component)
    }

    /// 使用给定 Windows 身份获取组件互斥体。
    ///
    /// 调用方负责确保 `identity` 对应当前进程的预期用户和登录会话。返回的
    /// 守卫拥有互斥体句柄；守卫存活期间，同名实例无法再次成功获取。
    ///
    /// # Errors
    ///
    /// 对象名称无效、安全描述符创建失败或 Win32 互斥体创建失败时返回
    /// [`SingleInstanceError::CreateFailed`]；同名对象已存在时返回
    /// [`SingleInstanceError::AlreadyRunning`]。
    pub fn acquire_for(
        identity: &RuntimeIdentity,
        component: &str,
    ) -> Result<Self, SingleInstanceError> {
        let name = identity.mutex_name(component).map_err(platform_error)?;
        let security = LocalSecurityDescriptor::for_logon(identity).map_err(platform_error)?;
        let attributes = security.security_attributes();
        let wide = HSTRING::from(name);
        // Presence only: no thread owns this mutex and no ReleaseMutex is needed.
        let handle = unsafe {
            CreateMutexW(
                Some(&attributes),
                false,
                windows_core::PCWSTR(wide.as_ptr()),
            )
        };
        if handle.0.is_null() {
            return Err(SingleInstanceError::CreateFailed(unsafe { GetLastError() }));
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS as u32 {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(SingleInstanceError::AlreadyRunning);
        }
        Ok(Self { handle })
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

unsafe impl Send for SingleInstance {}
unsafe impl Sync for SingleInstance {}

fn platform_error(error: std::io::Error) -> SingleInstanceError {
    SingleInstanceError::CreateFailed(error.raw_os_error().unwrap_or(ERROR_INVALID_PARAMETER) as u32)
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn selects_installed_paths_without_touching_directories() {
        let base = std::env::temp_dir().join("weasel-path-selection-only");
        let installed = RuntimePaths::select(
            base.clone(),
            Some(base.join("roaming")),
            Some(base.join("local")),
        )
        .unwrap();
        assert_eq!(installed.user_data, base.join("roaming/Rime-Next"));
        assert_eq!(installed.logs, base.join("local/Rime-Next/Logs"));
        assert!(RuntimePaths::select(base.clone(), Some(base.clone()), None).is_err());
        assert!(RuntimePaths::select(base.clone(), Some(base), Some("relative".into())).is_err());
    }

    #[test]
    fn installed_data_uses_roaming_directory() {
        let roaming = PathBuf::from(r"C:\Users\测试\AppData\Roaming");
        let local = PathBuf::from(r"C:\Users\测试\AppData\Local");
        assert_eq!(
            RuntimePaths::select(
                PathBuf::from(r"C:\Program Files\Weasel-Next"),
                Some(roaming.clone()),
                Some(local),
            )
            .unwrap()
            .user_data,
            roaming.join("Rime-Next")
        );
    }

    #[test]
    fn user_data_does_not_fall_back_to_install_directory() {
        let directory = PathBuf::from(r"C:\Program Files\Weasel-Next");
        let local = Some(PathBuf::from(r"C:\Users\测试\AppData\Local"));
        assert!(RuntimePaths::select(directory.clone(), None, local.clone()).is_err());
        assert!(RuntimePaths::select(directory, Some("relative".into()), local).is_err());
    }

    /// 安装器让用户自选的用户文件夹：程序目录里的 `userdir.txt` 必须压过 `%APPDATA%`。
    ///
    /// 回归自 2026-10-29 的需求「自己选择程序路径和用户文件夹路径」——以前所有组件都把
    /// `%APPDATA%\Rime-Next` 写死，安装器就算给了选择页也无处落地。
    #[test]
    fn user_dir_pointer_file_overrides_appdata() {
        let root = std::env::temp_dir().join(format!("weasel-userdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let chosen = root.join("自定义 用户数据");
        std::fs::write(
            root.join(USER_DIR_POINTER_FILE),
            format!("{}\r\n", chosen.display()),
        )
        .unwrap();

        assert_eq!(
            resolve_user_data_dir(
                Some(&root),
                None,
                Some(PathBuf::from(r"C:\Users\测试\AppData\Roaming")),
            ),
            Some(chosen.clone()),
            "指针文件里的绝对路径应当生效（含中文/空格）"
        );

        // 环境变量优先级更高（排障用）。
        assert_eq!(
            resolve_user_data_dir(
                Some(&root),
                Some(PathBuf::from(r"D:\临时用户目录")),
                Some(PathBuf::from(r"C:\Users\测试\AppData\Roaming")),
            ),
            Some(PathBuf::from(r"D:\临时用户目录"))
        );

        // 指针文件写坏了（相对路径/空）⇒ 回退 %APPDATA%\Rime-Next，而不是报错。
        std::fs::write(root.join(USER_DIR_POINTER_FILE), "relative\\dir\n").unwrap();
        assert_eq!(
            resolve_user_data_dir(
                Some(&root),
                None,
                Some(PathBuf::from(r"C:\Users\测试\AppData\Roaming")),
            ),
            Some(PathBuf::from(r"C:\Users\测试\AppData\Roaming").join("Rime-Next"))
        );

        // 没有指针文件时行为与历史一致。
        std::fs::remove_file(root.join(USER_DIR_POINTER_FILE)).unwrap();
        assert_eq!(
            resolve_user_data_dir(
                Some(&root),
                None,
                Some(PathBuf::from(r"C:\Users\测试\AppData\Roaming")),
            ),
            Some(PathBuf::from(r"C:\Users\测试\AppData\Roaming").join("Rime-Next"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 模块路径：静态链进 exe 时必须等于 `current_exe()`。
    ///
    /// DLL 形态（TIP/主题）走的是同一段 `GetModuleHandleExW` 代码，只是命中的模块变成
    /// 调用方 DLL —— 这里只能验证 exe 形态；DLL 形态靠 live 安装验证。
    #[test]
    fn module_path_matches_test_executable() {
        let module = current_module_path().unwrap();
        let exe = std::env::current_exe().unwrap();
        assert_eq!(module, exe);
        assert_eq!(executable_directory().unwrap(), exe.parent().unwrap());
    }
}