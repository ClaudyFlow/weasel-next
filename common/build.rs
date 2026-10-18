fn main() {
    build_identity();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    windows_bindgen::builder()
        .output(out.join("bindings.rs"))
        .input_default()
        .filters([
            // 官方设置界面用的对话框与 GDI 绘制（DialogBoxParam / 自绘列表行）
            "Windows.Win32.DialogBoxParamW",
            "Windows.Win32.EndDialog",
            "Windows.Win32.GetDlgItem",
            "Windows.Win32.SendDlgItemMessageW",
            "Windows.Win32.InvalidateRect",
            "Windows.Win32.MessageBoxW",
            "Windows.Win32.SetWindowLongPtrW",
            "Windows.Win32.GetWindowLongPtrW",
            "Windows.Win32.ShowWindow",
            "Windows.Win32.CreateSolidBrush",
            "Windows.Win32.DeleteObject",
            "Windows.Win32.FillRect",
            "Windows.Win32.FrameRect",
            "Windows.Win32.GetSysColor",
            "Windows.Win32.GetSysColorBrush",
            "Windows.Win32.SetBkMode",
            "Windows.Win32.SetTextColor",
            "Windows.Win32.DrawTextW",
            "Windows.Win32.RECT",
            "Windows.Win32.MEASUREITEMSTRUCT",
            "Windows.Win32.DRAWITEMSTRUCT",
            "Windows.Win32.WM_INITDIALOG",
            "Windows.Win32.WM_MEASUREITEM",
            "Windows.Win32.WM_DRAWITEM",
            "Windows.Win32.WM_COMMAND",
            "Windows.Win32.WM_CLOSE",
            "Windows.Win32.CB_ADDSTRING",
            "Windows.Win32.CB_SETCURSEL",
            "Windows.Win32.CB_GETCURSEL",
            "Windows.Win32.CBN_SELCHANGE",
            "Windows.Win32.LB_ADDSTRING",
            "Windows.Win32.LB_RESETCONTENT",
            "Windows.Win32.SetWindowTextW",
            "Windows.Win32.PostMessageW",
            "Windows.Win32.InitCommonControlsEx",
            "Windows.Win32.INITCOMMONCONTROLSEX",
            "Windows.Win32.GetModuleHandleW",
            "Windows.Win32.GetKeyState",
            "Windows.Win32.GetNamedPipeClientProcessId",
            "Windows.Win32.GetNamedPipeServerProcessId",
            "Windows.Win32.CoCreateGuid",
            "Windows.Win32.CoInitializeEx",
            "Windows.Win32.CoUninitialize",
            "Windows.Win32.COINIT_APARTMENTTHREADED",
            "Windows.Win32.RoInitialize",
            "Windows.Win32.RoUninitialize",
            "Windows.Win32.RO_INIT_SINGLETHREADED",
            "Windows.Win32.GetProcessTimes",
            "Windows.Win32.CreateEventW",
            "Windows.Win32.OpenEventW",
            "Windows.Win32.SetEvent",
            "Windows.Win32.WaitForMultipleObjects",
            "Windows.Win32.WaitForSingleObject",
            "Windows.Win32.SYNCHRONIZE",
            "Windows.Win32.OpenProcess",
            "Windows.Win32.PROCESS_QUERY_LIMITED_INFORMATION",
            "Windows.Win32.QueryFullProcessImageNameW",
            "Windows.Win32.VK_SHIFT",
            "Windows.Win32.MessageBoxW",
            "Windows.Win32.MB_OK",
            "Windows.Win32.MB_OKCANCEL",
            "Windows.Win32.MB_SETFOREGROUND",
            "Windows.Win32.TaskDialogIndirect",
            "Windows.Win32.LoadLibraryExW",
            "Windows.Win32.GetProcAddress",
            "Windows.Win32.LOAD_LIBRARY_SEARCH_SYSTEM32",
            "Windows.Win32.TASKDIALOGCONFIG",
            "Windows.Win32.TDF_EXPANDED_BY_DEFAULT",
            "Windows.Win32.TDF_ENABLE_HYPERLINKS",
            "Windows.Win32.TDF_POSITION_RELATIVE_TO_WINDOW",
            "Windows.Win32.TDF_SHOW_MARQUEE_PROGRESS_BAR",
            "Windows.Win32.TDF_SIZE_TO_CONTENT",
            "Windows.Win32.TDCBF_CANCEL_BUTTON",
            "Windows.Win32.TDCBF_OK_BUTTON",
            "Windows.Win32.TDM_ENABLE_BUTTON",
            "Windows.Win32.TDM_SET_ELEMENT_TEXT",
            "Windows.Win32.TDM_SET_MARQUEE_PROGRESS_BAR",
            "Windows.Win32.TDM_SET_PROGRESS_BAR_POS",
            "Windows.Win32.TDM_SET_PROGRESS_BAR_MARQUEE",
            "Windows.Win32.TDN_CREATED",
            "Windows.Win32.TDN_DESTROYED",
            "Windows.Win32.TDN_HYPERLINK_CLICKED",
            "Windows.Win32.TDE_MAIN_INSTRUCTION",
            "Windows.Win32.TDE_CONTENT",
            "Windows.Win32.TDE_EXPANDED_INFORMATION",
            "Windows.Win32.TDE_FOOTER",
            "Windows.Win32.TD_ERROR_ICON",
            "Windows.Win32.TD_INFORMATION_ICON",
            "Windows.Win32.TD_WARNING_ICON",
            "Windows.Win32.SendMessageW",
            "Windows.Win32.ShellExecuteW",
            "Windows.Win32.SW_SHOWNORMAL",
            "Windows.Win32.IDOK",
            "Windows.Win32.IDCANCEL",
            "Windows.Win32.GetCurrentProcess",
            "Windows.Win32.OpenProcessToken",
            "Windows.Win32.GetTokenInformation",
            "Windows.Win32.ConvertSidToStringSidW",
            "Windows.Win32.ConvertStringSecurityDescriptorToSecurityDescriptorW",
            "Windows.Win32.ConvertSecurityDescriptorToStringSecurityDescriptorW",
            "Windows.Win32.GetSecurityInfo",
            "Windows.Win32.CloseHandle",
            "Windows.Win32.LocalFree",
            "Windows.Win32.CreateMutexW",
            "Windows.Win32.GetLastError",
            "Windows.Win32.ERROR_PIPE_BUSY",
            "Windows.Win32.TOKEN_USER",
            "Windows.Win32.TOKEN_GROUPS",
            "Windows.Win32.TOKEN_QUERY",
            "Windows.Win32.TokenUser",
            "Windows.Win32.TokenLogonSid",
            "Windows.Win32.SE_GROUP_LOGON_ID",
            "Windows.Win32.SDDL_REVISION_1",
            "Windows.Win32.SECURITY_ATTRIBUTES",
            "Windows.Win32.ERROR_ALREADY_EXISTS",
            "Windows.Win32.ERROR_INVALID_PARAMETER",
            "Windows.Win32.DACL_SECURITY_INFORMATION",
            "Windows.Win32.SACL_SECURITY_INFORMATION",
            "Windows.Win32.LABEL_SECURITY_INFORMATION",
            "Windows.Win32.SE_KERNEL_OBJECT",
        ])
        .flat()
        .write();
    println!("cargo:rerun-if-changed=build.rs");
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("protoc is available");
    unsafe { std::env::set_var("PROTOC", protoc) };

    println!("cargo:rerun-if-changed=proto/broker.proto");
    println!("cargo:rerun-if-changed=proto/common.proto");
    println!("cargo:rerun-if-changed=proto/deploy.proto");
    println!("cargo:rerun-if-changed=proto/renderer.proto");
    println!("cargo:rerun-if-changed=proto/rpc.proto");
    println!("cargo:rerun-if-changed=proto/tip.proto");
    prost_build::compile_protos(
        &[
            "proto/broker.proto",
            "proto/common.proto",
            "proto/deploy.proto",
            "proto/renderer.proto",
            "proto/rpc.proto",
            "proto/tip.proto",
        ],
        &["proto"],
    )
    .expect("protobuf definitions compile");
}

fn build_identity() {
    use std::{
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let revision = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=normal"])
        .map(|status| if status.is_empty() { "clean" } else { "dirty" })
        .unwrap_or("unknown");
    let epoch = std::env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string()
    });
    println!("cargo:rustc-env=WEASEL_BUILD_REVISION={revision}");
    println!("cargo:rustc-env=WEASEL_BUILD_STATE={dirty}");
    println!("cargo:rustc-env=WEASEL_BUILD_EPOCH={epoch}");
    let date = epoch.parse::<i64>().ok().and_then(|seconds| {
        Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command",
            &format!("[DateTimeOffset]::FromUnixTimeSeconds({seconds}).UtcDateTime.ToString('yyyy-MM-dd HH:mm:ss')")])
            .output().ok().filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }).unwrap_or_else(|| format!("Unix seconds {epoch}"));
    println!("cargo:rustc-env=WEASEL_BUILD_DATE={date}");
    println!(
        "cargo:rustc-env=WEASEL_BUILD_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    // ⚠️⚠️ 这个列表直接决定**增量编译能不能用**。`common` 是所有 crate 的依赖，
    // 它一重编，Engine/Server/Renderer/Tip/Setting 全部跟着重编。
    //
    // 三条硬教训（2026-10-17 实测，每次都是这个文件）：
    //
    // 1) **绝不监视 `.git/index`**。本 build 脚本自己就调 `git status --porcelain`（见上），
    //    而 `git status` 会刷新 index 的 stat 缓存 ⇒ 注册它 = **自己触发自己**：
    //    每次构建都重跑 build.rs ⇒ 全量重编。分支/标签变化看 HEAD 与 refs 就够。
    //
    // 2) **绝不监视"其他 crate 的源码目录"**（`Tip`/`Server`/`Engine`/`Renderer`/`Setting`）。
    //    那些目录里的任何改动都跟本 crate 的输出无关，却会让 common 重编 ⇒ 全量重编。
    //    想知道"仓库脏不脏"，下面的 `git status` 已经给出 `WEASEL_BUILD_STATE` 了，
    //    不需要靠目录监视来实现。（上一轮我把 `Setting` 也加进去过，等于让增量编译彻底失效。）
    //
    // 3) 目录名要与当前实际目录一致；不存在的路径不要注册
    //    （原先写的 `tip`/`broker`/`renderer`/`server` 是改名前的旧名字）。
    //
    // 结论：**只保留真正影响本 crate 编译产物的东西** —— git 的 revision 来源，
    // 加上 common 自己的源码目录（cargo 本来就会监视它，这里显式写出便于理解）。
    for path in [
        ".git/HEAD",
        ".git/refs",
        ".git/packed-refs",
        "common",
        "build_support",
        "Cargo.lock",
    ] {
        let full = root.join(path);
        if full.exists() {
            println!("cargo:rerun-if-changed={}", full.display());
        }
    }
}
