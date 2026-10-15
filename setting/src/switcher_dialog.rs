//! 「方案选单设定」对话框的数据接线（官方 `SwitcherSettingsDialog`）。
//!
//! 界面来自 `ui/deployer.rc` 的 `IDD_SWITCHER_SETTING`：左侧是 `SysListView32`
//! 报表视图（带勾选框），右侧是简介静态框，下面是说明文字、快捷键输入框、
//! 「获取更多输入方案...」按钮和「中」确认键。
//!
//! 行为与官方一致：
//! * 进入时把可用方案按「已选的在前、未选的在后」填进列表，已选的打勾；
//! * 选中某一行时，右侧显示该方案的显示名、作者、简介；
//! * 按确认时把勾选结果写回 `default.custom.yaml` 的 `patch: schema_list:`。
//!
//! `LVITEMW` 与一串 `LVM_*`/`LVS_*` 在窗口绑定里没有（它们是宏或未纳入过滤清单的
//! 结构），这里按 Win32 的稳定布局与固定值自己定义，避免再扩一轮绑定生成器。

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use weasel_common::bindings::*;
use weasel_style::switcher::{self, SchemaInfo};
use windows_core::PCWSTR;

const IDD_SWITCHER_SETTING: u16 = 129;
/// `ICC_LISTVIEW_CLASSES`：初始化 SysListView32 控件类（commctrl.h 的宏）。
const ICC_LISTVIEW_CLASSES: u32 = 0x0000_0001;
const IDC_SCHEMA_LIST: i32 = 1000;
const IDC_SCHEMA_DESCRIPTION: i32 = 1209;
const IDC_HOTKEYS: i32 = 1003;
const IDC_GET_SCHEMATA: i32 = 1211;

// ---- ListView 的消息与常量（commctrl.h，值固定）--------------------------
const LVM_FIRST: u32 = 0x1000;
const LVM_INSERTITEMW: u32 = LVM_FIRST + 77; // 0x104D
const LVM_SETITEMTEXTW: u32 = LVM_FIRST + 116; // 0x1074
const LVM_GETITEMSTATE: u32 = LVM_FIRST + 44; // 0x102C
const LVM_SETITEMSTATE: u32 = LVM_FIRST + 43; // 0x102B
const LVM_SETEXTENDEDLISTVIEWSTYLE: u32 = LVM_FIRST + 54; // 0x1036
/// 报表视图**必须先有列**才能插入行，否则 LVM_INSERTITEM 会失败（行数一直是 0）。
const LVM_GETITEMCOUNT: u32 = LVM_FIRST + 4; // 0x1004
const LVM_SETCOLUMNWIDTH: u32 = LVM_FIRST + 30; // 0x101E
const LVM_GETITEMDATA: u32 = LVM_FIRST + 41; // 0x1029
const LVM_SETITEMDATA: u32 = LVM_FIRST + 42; // 0x102A
const LVM_INSERTCOLUMNW: u32 = LVM_FIRST + 97; // 0x1061
const LVCF_FMT: u32 = 0x0001;
const LVCF_WIDTH: u32 = 0x0002;
const LVCF_TEXT: u32 = 0x0004;
const LVCFMT_LEFT: i32 = 0x0000;
const LVS_EX_CHECKBOXES: u32 = 0x0000_0004;
const LVIF_TEXT: u32 = 0x0001;
const LVIF_STATE: u32 = 0x0008;
const LVIS_STATEIMAGEMASK: u32 = 0xF000;
const LVN_FIRST: i32 = -100;
const LVN_ITEMCHANGED: i32 = LVN_FIRST - 1; // -101
const WM_NOTIFY_: u32 = 0x004E;
/// 窗口消息（少了它，match 分支会被当成绑定变量、匹配所有消息）。
const WM_CLOSE_: u32 = 0x0010;
/// 窗口消息（少了它，match 分支会被当成绑定变量、匹配所有消息）。
const WM_INITDIALOG_: u32 = 0x0110;
/// 窗口消息（少了它，match 分支会被当成绑定变量、匹配所有消息）。
const WM_COMMAND_: u32 = 0x0111;

/// `INDEXTOSTATEIMAGEMASK(i)` = `i << 12`；未勾选是 1，勾选是 2。
fn state_image(checked: bool) -> u32 {
    (if checked { 2u32 } else { 1u32 }) << 12
}

/// `LVITEMW`：Win32 的布局，x64 下 `#[repr(C)]` 即为正确排列。
#[repr(C)]
struct LvItemW {
    mask: u32,
    i_item: i32,
    i_sub_item: i32,
    state: u32,
    state_mask: u32,
    text: *mut u16,
    text_max: i32,
    i_image: i32,
    l_param: isize,
    i_indent: i32,
    i_group_id: i32,
    columns: u32,
    pu_columns: *mut u32,
    pi_col_fmt: *mut i32,
    i_group: i32,
}

/// `LVCOLUMNW`：报表视图的列定义（布局同 Win32）。
#[repr(C)]
struct LvColumnW {
    mask: u32,
    fmt: i32,
    cx: i32,
    text: *mut u16,
    text_max: i32,
    i_sub_item: i32,
    i_image: i32,
    i_order: i32,
}

/// `NMLISTVIEW`：`WM_NOTIFY` 里 `LVN_ITEMCHANGED` 的载荷（`NMHDR` 之后的前几个字段）。
#[repr(C)]
struct NmListView {
    hwnd_from: HWND,
    id_from: usize,
    code: u32,
    i_item: i32,
    i_sub_item: i32,
    u_new_state: u32,
    u_old_state: u32,
    u_changed: u32,
    pt: [i32; 2],
    l_param: isize,
}

/// 自定义消息：把列表填充推迟到对话框完全就绪之后。
/// 实测在 WM_INITDIALOG 里插入会被控件拒绝（返回 -1），列却能插成功。
const WM_APP_FILL: u32 = 0x8001;

thread_local! {
    /// 是否已经把填充推迟过一次（防止无限推迟）。
    static DEFERRED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct SwitcherDialog {
    shared: PathBuf,
    user: PathBuf,
    rows: Vec<SchemaInfo>,
    /// 列表是否已填充（幂等标志，见 WM_INITDIALOG 处的说明）。
    initialized: bool,
}

thread_local! {
    /// 对话框运行期的状态。用线程局部而不是往 DWLP_USER 塞裸指针：
    /// 对话框过程与 show() 在同一个线程，指针对不上时的故障是访问违例
    /// （0xC000041D），线程局部从根上避开这类问题。
    static STATE: RefCell<Option<SwitcherDialog>> = const { RefCell::new(None) };
}

/// 在对话框过程里访问状态；不在对话框里时为 None。
/// 在对话框过程里访问状态；不在对话框里、或状态正被占用时为 None。
///
/// **必须用 `try_borrow_mut`**：往 ListView 插行时它会同步发 `WM_NOTIFY` 回来，
/// 形成重入。用 `borrow_mut` 会 panic，而这个 panic 发生在不能展开的 FFI 边界上，
/// 进程会直接中止（0xC000041D），对话框根本不显示——这正是 /switcher 打不开的原因。
fn with_state<T>(f: impl FnOnce(&mut SwitcherDialog) -> T) -> Option<T> {
    STATE.with(|slot| match slot.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => None,
    })
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 打开「方案选单设定」。
pub fn show(shared: &Path, user: &Path) -> Result<(), String> {
    let rows = switcher::list_schemas(shared, &user.to_path_buf())
        .map_err(|error| format!("读取方案列表失败：{error}"))?;
    STATE.with(|slot| {
        *slot.borrow_mut() = Some(SwitcherDialog {
            shared: shared.to_path_buf(),
            user: user.to_path_buf(),
            rows,
            initialized: false,
        });
    });
    // SysListView32 属于通用控件库：不先初始化，它在对话框模板里创建失败，
    // 整个对话框就不会出现（标准控件的 /style、/dict 不受影响）。
    unsafe {
        let classes = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES,
        };
        InitCommonControlsEx(&classes);
    }
    let instance = unsafe { GetModuleHandleW(None) };
    let result = unsafe {
        DialogBoxParamW(
            Some(HINSTANCE(instance.0)),
            PCWSTR(IDD_SWITCHER_SETTING as usize as *const u16),
            None,
            Some(proc),
            LPARAM(0),
        )
    };
    STATE.with(|slot| *slot.borrow_mut() = None);
    if result == -1 {
        Err("对话框创建失败".into())
    } else {
        // 按「中」确认方案之后，接着打开官方界面风格设定 —— 先选方案，再选皮肤，
        // 一个流程走完（同进程顺序调用）。
        if result == IDOK as isize {
            crate::dialogs::show_style(shared, user)?;
        }
        Ok(())
    }
}



/// 把一行插进 ListView，并按需要打勾。
unsafe fn insert_row(hwnd: HWND, index: i32, name: &str, checked: bool) -> (isize, isize, isize) {
    // 分三步，每步都记返回值，便于定位是哪一步被控件拒绝：
    //   1) 先插一个空行（mask=0，不带文本也不带状态）
    //   2) 再用 LVM_SETITEMTEXTW 设文本
    //   3) 最后用 LVM_SETITEMSTATE 设勾选
    let mut empty = LvItemW {
        mask: 0,
        i_item: index,
        i_sub_item: 0,
        state: 0,
        state_mask: 0,
        text: std::ptr::null_mut(),
        text_max: 0,
        i_image: 0,
        l_param: 0,
        i_indent: 0,
        i_group_id: 0,
        columns: 0,
        pu_columns: std::ptr::null_mut(),
        pi_col_fmt: std::ptr::null_mut(),
        i_group: 0,
    };
    let inserted = SendDlgItemMessageW(hwnd, IDC_SCHEMA_LIST, LVM_INSERTITEMW, WPARAM(0), LPARAM(&mut empty as *mut LvItemW as isize)).0;
    if inserted < 0 {
        return (inserted, -999, -999);
    }
    let mut text = wide(name);
    let mut text_item = LvItemW {
        mask: LVIF_TEXT,
        i_item: index,
        i_sub_item: 0,
        state: 0,
        state_mask: 0,
        text: text.as_mut_ptr(),
        text_max: text.len() as i32,
        i_image: 0,
        l_param: 0,
        i_indent: 0,
        i_group_id: 0,
        columns: 0,
        pu_columns: std::ptr::null_mut(),
        pi_col_fmt: std::ptr::null_mut(),
        i_group: 0,
    };
    let text_result = SendDlgItemMessageW(hwnd, IDC_SCHEMA_LIST, LVM_SETITEMTEXTW, WPARAM(index as usize), LPARAM(&mut text_item as *mut LvItemW as isize)).0;
    let state_result = if checked {
        let mut state_item = LvItemW {
            mask: LVIF_STATE,
            i_item: index,
            i_sub_item: 0,
            state: state_image(true),
            state_mask: LVIS_STATEIMAGEMASK,
            text: std::ptr::null_mut(),
            text_max: 0,
            i_image: 0,
            l_param: 0,
            i_indent: 0,
            i_group_id: 0,
            columns: 0,
            pu_columns: std::ptr::null_mut(),
            pi_col_fmt: std::ptr::null_mut(),
            i_group: 0,
        };
        SendDlgItemMessageW(hwnd, IDC_SCHEMA_LIST, LVM_SETITEMSTATE, WPARAM(index as usize), LPARAM(&mut state_item as *mut LvItemW as isize)).0
    } else {
        0
    };
    (inserted, text_result, state_result)
}

/// 右侧简介：显示名、作者、简介，与官方 `ShowDetails` 的三段一致。
unsafe fn show_details(hwnd: HWND, row: &SchemaInfo) {
    let mut text = format!("{}\n\n{}", row.name, row.author);
    if !row.description.is_empty() {
        text.push_str("\n\n");
        text.push_str(&row.description);
    }
    // 静态框用 SetWindowTextW 更新（官方也是这么做的）
    let wide_text = wide(&text);
    unsafe {
        SetWindowTextW(
            GetDlgItem(Some(hwnd), IDC_SCHEMA_DESCRIPTION),
            PCWSTR(wide_text.as_ptr()),
        );
    }
}

thread_local! {
    /// 对话框过程的进入深度。用来把重入挡在栈溢出之前：
    /// 一旦某个消息链条把自己套进去，深度会迅速增长，这里直接截断并打印消息号，
    /// 既避免崩溃，也把"是谁在递归"记录下来。
    static DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// 对话框过程入口：带深度守卫，超深就记日志并忽略该消息。
unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    let depth = DEPTH.with(|cell| {
        let value = cell.get();
        cell.set(value + 1);
        value
    });
    if depth > 0 {
        // 只在重入时记录，正常消息不刷日志
        eprintln!("Weasel-Next-Setting: 对话框消息重入 depth={depth} message=0x{message:04X}");
    }
    if depth > 6 {
        eprintln!("Weasel-Next-Setting: 重入过深，忽略 message=0x{message:04X}（防止栈溢出）");
        DEPTH.with(|cell| cell.set(cell.get() - 1));
        return 0;
    }
    let result = handle(hwnd, message, wparam, lparam);
    DEPTH.with(|cell| cell.set(cell.get() - 1));
    result
}
fn handle(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match message {
        WM_INITDIALOG_ => {
            // 幂等：填充时列表控件会同步发通知、通知又回到这里重入，
            // 重入再跑一遍填充就会重复插入（实测行数从 12 涨到几千）。
            match with_state(|state| std::mem::replace(&mut state.initialized, true)) {
                Some(false) => {}
                _ => return 1,
            }
            let list = unsafe { GetDlgItem(Some(hwnd), IDC_SCHEMA_LIST) };
            if list.0.is_null() {
                // 控件还没创建好：还原标志，等下一次初始化再填
                with_state(|state| state.initialized = false);
                return 1;
            }
            // 诊断：把控件的窗口样式打出来。LVS_OWNERDATA=0x1000（虚拟列表会拒绝插行）；
            // LVS_REPORT=0x1、LVS_ALIGNLEFT=0x800。
            unsafe {
                let style = GetWindowLongPtrW(list, -16) as u32;   // GWL_STYLE
                let exstyle = GetWindowLongPtrW(list, -20) as u32; // GWL_EXSTYLE
                eprintln!("Weasel-Next-Setting: 列表样式 style=0x{style:08X} (OWNERDATA={}) exstyle=0x{exstyle:08X}",
                    if style & 0x1000 != 0 { "是" } else { "否" });
            }
            if !DEFERRED.with(|c| c.replace(true)) {
                // 第一次进来：先把标志还原，投递一条消息，等对话框就绪后再填。
                with_state(|state| state.initialized = false);
                unsafe { PostMessageW(Some(hwnd), WM_APP_FILL, WPARAM(0), LPARAM(0)) };
                return 1;
            }
            eprintln!(  "Weasel-Next-Setting: 初始化 列表句柄={:?} 行数={:?} 勾选扩展={}",  list.0,  with_state(|state| state.rows.len()),  LVS_EX_CHECKBOXES + LVM_SETEXTENDEDLISTVIEWSTYLE );
            // 报表视图**必须先有列**才能插入行，否则 LVM_INSERTITEM 会失败（行数一直是 0）。
            unsafe {
                let mut header = wide("方案");
                let column = LvColumnW {
                    mask: LVCF_FMT | LVCF_WIDTH | LVCF_TEXT,
                    fmt: LVCFMT_LEFT,
                    cx: 110,
                    text: header.as_mut_ptr(),
                    text_max: header.len() as i32,
                    i_sub_item: 0,
                    i_image: 0,
                    i_order: 0,
                };
                let added = SendDlgItemMessageW(
                    hwnd,
                    IDC_SCHEMA_LIST,
                    LVM_INSERTCOLUMNW,
                    WPARAM(0),
                    LPARAM(&column as *const LvColumnW as isize),
                );
                    // 方案名有长有短（朙月拼音·簡化字 等），官方用 SetColumnWidth(0, 客户区宽-20)；
                    // 这里直接给足 320 像素，避免名字被截断。
                    unsafe { SendDlgItemMessageW(hwnd, IDC_SCHEMA_LIST, LVM_SETCOLUMNWIDTH, WPARAM(0), LPARAM(320)) };
                eprintln!("Weasel-Next-Setting: 插入列 → {}", added.0);
            }
            let mut first_selected: Option<usize> = None;
            with_state(|state| {
                for (index, row) in state.rows.iter().enumerate() {
                    let (ins, txt, st) = unsafe { insert_row(hwnd, index as i32, &row.name, row.selected) };
                    // 照官方 SwitcherSettingsDialog：把方案 id 的指针存进该项的 ItemData，
                    // 保存时用 LVM_GETITEMDATA 从控件取回 —— 不再依赖 state.rows 的长度。
                    // 泄漏一份宽字符串换取指针稳定（对话框生命周期内有效，官方同样持有指针）。
                    let id_wide = wide(&row.id);
                    let id_ptr = id_wide.as_ptr() as isize;
                    std::mem::forget(id_wide);
                    unsafe {
                        SendDlgItemMessageW(
                            hwnd,
                            IDC_SCHEMA_LIST,
                            LVM_SETITEMDATA,
                            WPARAM(index),
                            LPARAM(id_ptr),
                        )
                    };
                    if index < 3 { eprintln!("Weasel-Next-Setting: 插行 {index} → 插入={ins} 文本={txt} 状态={st}"); }
                    if index < 4 {
                        eprintln!("Weasel-Next-Setting: 插入第 {index} 行「{name}」", name = row.name);
                    }
                    if row.selected && first_selected.is_none() {
                        first_selected = Some(index);
                    }
                }
            });
            // 插完之后直接问控件要行数，确认插入是否真的生效
            let rows = unsafe {
                SendDlgItemMessageW(hwnd, IDC_SCHEMA_LIST, 0x1004, WPARAM(0), LPARAM(0))
            };
            eprintln!("Weasel-Next-Setting: 插完 LVM_GETITEMCOUNT={}", rows.0);
            // 勾选框样式放在插入之后设置：实测先设它会导致 LVM_INSERTITEM 返回 -1。
            unsafe {
                SendMessageW(list, LVM_SETEXTENDEDLISTVIEWSTYLE, WPARAM(0), LPARAM(LVS_EX_CHECKBOXES as isize));
            }
            if let Some(index) = first_selected {
                let mut item = LvItemW {
                    mask: LVIF_STATE,
                    i_item: index as i32,
                    i_sub_item: 0,
                    state: LVIS_SELECTED_STATE,
                    state_mask: LVIS_SELECTED_STATE,
                    text: std::ptr::null_mut(),
                    text_max: 0,
                    i_image: 0,
                    l_param: 0,
                    i_indent: 0,
                    i_group_id: 0,
                    columns: 0,
                    pu_columns: std::ptr::null_mut(),
                    pi_col_fmt: std::ptr::null_mut(),
                    i_group: 0,
                };
                unsafe {
                    SendMessageW(
                        list,
                        LVM_SETITEMSTATE,
                        WPARAM(index),
                        LPARAM(&mut item as *mut LvItemW as isize),
                    );
                }
                with_state(|state| {
                    if let Some(row) = state.rows.get(index) {
                        unsafe { show_details(hwnd, row) };
                    }
                    if let Some(hotkeys) = read_hotkeys(&state.shared) {
                        let text = wide(&hotkeys);
                        unsafe {
                            SetWindowTextW(
                                GetDlgItem(Some(hwnd), IDC_HOTKEYS),
                                PCWSTR(text.as_ptr()),
                            );
                        }
                    }
                });
            }
            // 显式显示：这个模板创建出来后一直不可见（/style 同模板却正常），
            // 这里主动 ShowWindow 一次。
            unsafe { ShowWindow(hwnd, 5) }; // SW_SHOW
            1
        }
        WM_APP_FILL => {
            // 推迟到这里的填充：此时对话框与列表控件都已完全就绪。
            let _ = handle(hwnd, WM_INITDIALOG_, WPARAM(0), LPARAM(0));
            1
        }
        WM_NOTIFY_ => {
            let header = lparam.0 as *const NmListView;
            if header.is_null() {
                return 0;
            }
            let header = unsafe { &*header };
            if header.code as i32 == LVN_ITEMCHANGED {
                let index = header.i_item as usize;
                with_state(|state| {
                    if let Some(row) = state.rows.get(index) {
                        unsafe { show_details(hwnd, row) };
                    }
                });
            }
            0
        }
        WM_COMMAND_ => {
            let id = (wparam.0 & 0xffff) as i32;
            if id == IDOK {
                let list = unsafe { GetDlgItem(Some(hwnd), IDC_SCHEMA_LIST) };
                // 照官方 SwitcherSettingsDialog::OnOK 的写法：按**控件的行数**循环，
                // 不用 state.rows 的长度 —— 两者不一致时按下标配对才不会静默丢数据。
                let count = unsafe {
                    SendMessageW(list, LVM_GETITEMCOUNT, WPARAM(0), LPARAM(0)).0 as usize
                };
                let mut checked: Vec<bool> = Vec::with_capacity(count);
                for index in 0..count {
                    let value = unsafe {
                        SendMessageW(
                            list,
                            LVM_GETITEMSTATE,
                            WPARAM(index),
                            LPARAM(LVIS_STATEIMAGEMASK as isize),
                        )
                    };
                    let image = ((value.0 as u32) & LVIS_STATEIMAGEMASK) >> 12;
                    checked.push(image == 2);
                }
                // 官方：一项都没选就报错退出，不写文件。
                if !checked.iter().any(|on| *on) {
                    let text = wide("至少要选用一项。");
                    let caption = wide("提示");
                    unsafe {
                        MessageBoxW(
                            Some(hwnd),
                            PCWSTR(text.as_ptr()),
                            PCWSTR(caption.as_ptr()),
                            0u32 | MB_ICONWARNING_,
                        );
                    }
                    return 1;
                }
                let outcome = with_state(|state| {
                    // 照官方 OnOK：从控件取回每项 ItemData 里的方案 id。
                    let mut selected: Vec<String> = Vec::new();
                    for index in 0..count {
                        if !checked.get(index).copied().unwrap_or(false) {
                            continue;
                        }
                        let raw = unsafe { SendMessageW(list, LVM_GETITEMDATA, WPARAM(index), LPARAM(0)).0 } as *const u16;
                        if raw.is_null() {
                            continue;
                        }
                        let mut len = 0usize;
                        while unsafe { *raw.add(len) } != 0 {
                            len += 1;
                        }
                        let slice = unsafe { std::slice::from_raw_parts(raw, len) };
                        selected.push(String::from_utf16_lossy(slice));
                    }
                    let path = switcher::user_default_custom_path(&state.user);
                    switcher::set_schema_list(&path, &selected)
                });
                match outcome {
                    Some(Ok(())) => {
                        unsafe { EndDialog(hwnd, IDOK as isize) };
                    }
                    Some(Err(error)) => {
                        let text = wide(&format!("写入方案选单失败：{error}"));
                        let caption = wide("提示");
                        unsafe {
                            MessageBoxW(
                                Some(hwnd),
                                PCWSTR(text.as_ptr()),
                                PCWSTR(caption.as_ptr()),
                                0u32 | MB_ICONWARNING_,
                            );
                        }
                    }
                    None => {}
                }
                return 1;
            }
            if id == IDCANCEL {
                unsafe { EndDialog(hwnd, IDCANCEL as isize) };
                return 1;
            }
            0
        }
        WM_CLOSE_ => {
            unsafe { EndDialog(hwnd, IDCANCEL as isize) };
            1
        }
        _ => 0,
    }
}

/// 读 `default.yaml` 里 `switcher/hotkeys` 的值（官方显示在快捷键框里）。
fn read_hotkeys(shared: &Path) -> Option<String> {
    let text = std::fs::read_to_string(shared.join("default.yaml")).ok()?;
    let mut in_switcher = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with("switcher:") {
            in_switcher = true;
            continue;
        }
        if in_switcher {
            if !trimmed.is_empty() && !trimmed.starts_with(' ') && !trimmed.starts_with('\t') {
                break;
            }
            let stripped = trimmed.trim_start();
            if let Some(rest) = stripped.strip_prefix("hotkeys:") {
                let value = rest.trim();
                if value.starts_with('[') {
                    // 行内列表：["Control+grave", "F4"]
                    return Some(
                        value
                            .trim_start_matches('[')
                            .trim_end_matches(']')
                            .split(',')
                            .map(|part| part.trim().trim_matches(['"', '\''].as_ref()))
                            .filter(|part| !part.is_empty())
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                }
                return Some(value.trim_matches(['"', '\''].as_ref()).to_string());
            }
        }
    }
    None
}

/// Win32 的 `LVIS_SELECTED`。
const LVIS_SELECTED_STATE: u32 = 0x0002;
/// `MB_ICONWARNING`（与 dialogs.rs 里的常量同值）。
const MB_ICONWARNING_: u32 = 0x0030;
