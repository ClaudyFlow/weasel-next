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

const IDD_SWITCHER_SETTING: u16 = 101;
/// `ICC_LISTVIEW_CLASSES`：初始化 SysListView32 控件类（commctrl.h 的宏）。
const ICC_LISTVIEW_CLASSES: u32 = 0x0000_0001;
const IDC_SCHEMA_LIST: i32 = 1208;
const IDC_SCHEMA_DESCRIPTION: i32 = 1209;
const IDC_HOTKEYS: i32 = 1210;
const IDC_GET_SCHEMATA: i32 = 1211;

// ---- ListView 的消息与常量（commctrl.h，值固定）--------------------------
const LVM_FIRST: u32 = 0x1000;
const LVM_INSERTITEMW: u32 = LVM_FIRST + 77; // 0x104D
const LVM_SETITEMTEXTW: u32 = LVM_FIRST + 116; // 0x1074
const LVM_GETITEMSTATE: u32 = LVM_FIRST + 44; // 0x102C
const LVM_SETITEMSTATE: u32 = LVM_FIRST + 43; // 0x102B
const LVM_SETEXTENDEDLISTVIEWSTYLE: u32 = LVM_FIRST + 54; // 0x1036
const LVS_EX_CHECKBOXES: u32 = 0x0000_0004;
const LVIF_TEXT: u32 = 0x0001;
const LVIF_STATE: u32 = 0x0008;
const LVIS_STATEIMAGEMASK: u32 = 0xF000;
const LVN_FIRST: i32 = -100;
const LVN_ITEMCHANGED: i32 = LVN_FIRST - 1; // -101
const WM_NOTIFY_: u32 = 0x004E;

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

struct SwitcherDialog {
    shared: PathBuf,
    user: PathBuf,
    rows: Vec<SchemaInfo>,
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
        Ok(())
    }
}



/// 把一行插进 ListView，并按需要打勾。
unsafe fn insert_row(list: HWND, index: i32, name: &str, checked: bool) {
    let mut text = wide(name);
    let item = LvItemW {
        mask: LVIF_TEXT | LVIF_STATE,
        i_item: index,
        i_sub_item: 0,
        state: state_image(checked),
        state_mask: LVIS_STATEIMAGEMASK,
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
    unsafe {
        SendMessageW(
            list,
            LVM_INSERTITEMW,
            WPARAM(0),
            LPARAM(&item as *const LvItemW as isize),
        );
    }
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

unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match message {
        WM_INITDIALOG_ => {
            let list = unsafe { GetDlgItem(Some(hwnd), IDC_SCHEMA_LIST) };
            unsafe {
                SendMessageW(
                    list,
                    LVM_SETEXTENDEDLISTVIEWSTYLE,
                    WPARAM(0),
                    LPARAM(LVS_EX_CHECKBOXES as isize),
                );
            }
            let mut first_selected: Option<usize> = None;
            with_state(|state| {
                for (index, row) in state.rows.iter().enumerate() {
                    unsafe { insert_row(list, index as i32, &row.name, row.selected) };
                    if row.selected && first_selected.is_none() {
                        first_selected = Some(index);
                    }
                }
            });
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
                // 先按界面顺序收集勾选结果
                let mut checked: Vec<bool> = Vec::new();
                with_state(|state| {
                    for index in 0..state.rows.len() {
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
                });
                let outcome = with_state(|state| {
                    let selected: Vec<String> = state
                        .rows
                        .iter()
                        .zip(checked.iter())
                        .filter(|(_, on)| **on)
                        .map(|(row, _)| row.id.clone())
                        .collect();
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
