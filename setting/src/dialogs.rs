//! 官方设置界面（Win32 对话框）的 Rust 实现。
//!
//! 界面本身来自 `ui/deployer.rc` 里的 DIALOGEX 模板——与官方 `WeaselDeployer.rc`
//! 同一套控件、同一套坐标。本模块只负责把数据填进去、把用户的操作写回配置：
//!
//! * `WM_INITDIALOG`：把 `preset_color_schemes` 的显示名填进下拉框，把当前配色的
//!   颜色键填进左侧列表，并按字段类型决定显示下拉框还是编辑框（官方也是二选一）。
//! * `WM_COMMAND`：`IDC_COLOR_SCHEME` 的下拉框换项 → 立刻写
//!   `patch: style/color_scheme: <id>`（等价于官方 `customize_string`）。
//! * `WM_MEASUREITEM` / `WM_DRAWITEM`：列表是 `LBS_OWNERDRAWFIXED`，自绘每一行：
//!   左边一个小色块，右边是颜色键名与当前值。
//!
//! 入口与官方一致：`weasel-settings.exe /style` 打开界面风格设定，
//! `/switcher` 与 `/dict` 分别对应方案选单设定与用户词典管理。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use weasel_style::settings as style_settings;

use crate::bindings::*;

/// 对话框 ID，与 `ui/deployer.rc` 一致。
const IDD_STYLE_SETTING: u16 = 102;
const IDD_SWITCHER_SETTING: u16 = 101;
const IDD_DICT_MANAGEMENT: u16 = 103;

const IDC_COLOR_SCHEME: i32 = 1001;
const IDC_COLOR_LIST: i32 = 1002;
const IDC_COLOR_VALUE: i32 = 1003;
const IDC_SET_COLOR: i32 = 1004;
const IDC_SELECT_FONT: i32 = 1005;
const IDC_STYLE_LIST: i32 = 1006;
const IDC_STYLE_VALUE_COMBO: i32 = 1007;
const IDC_STYLE_VALUE_EDIT: i32 = 1008;
const IDC_SET_STYLE: i32 = 1009;

/// 颜色键清单与官方 `WeaselStyleKeys` 的颜色部分一一对应。
const COLOR_KEYS: &[&str] = &[
    "back_color",
    "border_color",
    "shadow_color",
    "text_color",
    "candidate_text_color",
    "candidate_back_color",
    "candidate_border_color",
    "candidate_shadow_color",
    "label_color",
    "comment_text_color",
    "hilited_text_color",
    "hilited_back_color",
    "hilited_border_color",
    "hilited_shadow_color",
    "hilited_candidate_text_color",
    "hilited_candidate_back_color",
    "hilited_candidate_border_color",
    "hilited_candidate_shadow_color",
    "hilited_label_color",
    "hilited_comment_text_color",
    "nextpage_color",
    "prevpage_color",
];

/// 对话框运行期的状态；只在对话框所在线程内访问。
struct StyleDialog {
    shared: PathBuf,
    user: PathBuf,
    /// 出厂 + 用户补丁合并后的配置。
    config: Option<weasel_style::WeaselConfig>,
    /// 配色（id、显示名）列表，顺序与下拉框一致。
    schemes: Vec<(String, String)>,
    /// 当前选中的配色下标。
    selected: usize,
    /// 左侧列表当前显示的颜色键与取值。
    colors: Vec<(String, u32)>,
}

impl StyleDialog {
    /// 从磁盘读配置并算出界面要显示的内容。
    fn reload(&mut self) {
        let base_text = std::fs::read_to_string(self.shared.join("weasel.yaml")).ok();
        let custom_text = std::fs::read_to_string(self.user.join("weasel.custom.yaml")).ok();
        self.config = base_text.and_then(|base| {
            weasel_style::parse_with_patch(&base, custom_text.as_deref()).ok()
        });
        self.schemes = self
            .config
            .as_ref()
            .map(|config| {
                style_settings::list_schemes(config, None)
                    .into_iter()
                    .map(|info| (info.id, info.name))
                    .collect()
            })
            .unwrap_or_default();
        let active = self
            .config
            .as_ref()
            .map(|config| config.style.color_scheme.clone())
            .unwrap_or_default();
        self.selected = self
            .schemes
            .iter()
            .position(|(id, _)| *id == active)
            .unwrap_or(0);
        self.collect_colors();
    }

    /// 取出当前配色里每个颜色键的取值（缺项按 0 显示）。
    fn collect_colors(&mut self) {
        self.colors.clear();
        let Some(config) = &self.config else { return };
        let Some((id, _)) = self.schemes.get(self.selected) else {
            return;
        };
        let Some(scheme) = config.schemes.get(id) else {
            return;
        };
        for key in COLOR_KEYS {
            let value = match *key {
                "back_color" => scheme.back_color,
                "border_color" => scheme.border_color,
                "shadow_color" => scheme.shadow_color,
                "text_color" => scheme.text_color,
                "candidate_text_color" => scheme.candidate_text_color,
                "candidate_back_color" => scheme.candidate_back_color,
                "candidate_border_color" => scheme.candidate_border_color,
                "candidate_shadow_color" => scheme.candidate_shadow_color,
                "label_color" => scheme.label_color,
                "comment_text_color" => scheme.comment_text_color,
                "hilited_text_color" => scheme.hilited_text_color,
                "hilited_back_color" => scheme.hilited_back_color,
                "hilited_border_color" => scheme.hilited_border_color,
                "hilited_shadow_color" => scheme.hilited_shadow_color,
                "hilited_candidate_text_color" => scheme.hilited_candidate_text_color,
                "hilited_candidate_back_color" => scheme.hilited_candidate_back_color,
                "hilited_candidate_border_color" => scheme.hilited_candidate_border_color,
                "hilited_candidate_shadow_color" => scheme.hilited_candidate_shadow_color,
                "hilited_label_color" => scheme.hilited_label_color,
                "hilited_comment_text_color" => scheme.hilited_comment_text_color,
                "nextpage_color" => scheme.nextpage_color,
                "prevpage_color" => scheme.prevpage_color,
                _ => 0,
            };
            self.colors.push(((*key).to_string(), value));
        }
    }
}

/// 打开「界面风格设定」对话框。`/style` 走这里。
pub fn show_style(shared: &Path, user: &Path) -> Result<(), String> {
    let mut state = StyleDialog {
        shared: shared.to_path_buf(),
        user: user.to_path_buf(),
        config: None,
        schemes: Vec::new(),
        selected: 0,
        colors: Vec::new(),
    };
    state.reload();
    let state = Rc::new(RefCell::new(state));
    let pointer = Rc::into_raw(state.clone());
    let instance = unsafe { GetModuleHandleW(None) };
    let result = unsafe {
        DialogBoxParamW(
            instance,
            MAKEINTRESOURCEW(IDD_STYLE_SETTING),
            None,
            Some(style_proc),
            pointer as isize,
        )
    };
    // 无论正常关闭还是出错，都要把引用计数还回去。
    unsafe { drop(Rc::from_raw(pointer)) };
    if result == -1 {
        Err("对话框创建失败".into())
    } else {
        Ok(())
    }
}

/// 取回对话框状态（`DWLP_USER` 里存的是 `Rc<RefCell<StyleDialog>>` 的裸指针）。
unsafe fn state_of(hwnd: HWND) -> Option<Rc<RefCell<StyleDialog>>> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, DWLP_USER) } as *const RefCell<StyleDialog>;
    if raw.is_null() {
        return None;
    }
    // 借用一份 Rc 但**不**接管所有权：指针由调用方在对话框结束时回收。
    let rc = unsafe { Rc::from_raw(raw) };
    let clone = rc.clone();
    std::mem::forget(rc);
    Some(clone)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 界面风格设定的对话框过程。
unsafe extern "system" fn style_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match message {
        WM_INITDIALOG => {
            unsafe { SetWindowLongPtrW(hwnd, DWLP_USER, lparam) };
            let Some(state) = (unsafe { state_of(hwnd) }) else {
                return 0;
            };
            let state = state.borrow();
            // 下拉框：配色显示名
            for (_, name) in &state.schemes {
                let text = wide(name);
                unsafe { SendDlgItemMessageW(hwnd, IDC_COLOR_SCHEME, CB_ADDSTRING, 0, text.as_ptr() as LPARAM) };
            }
            unsafe {
                SendDlgItemMessageW(hwnd, IDC_COLOR_SCHEME, CB_SETCURSEL, state.selected, 0);
                // 左侧颜色列表：先清空再逐条加入（自绘，文字内容仅用于行数）
                SendDlgItemMessageW(hwnd, IDC_COLOR_LIST, LB_RESETCONTENT, 0, 0);
            }
            for (key, value) in &state.colors {
                let text = wide(&format!("{key}   0x{value:08x}"));
                unsafe { SendDlgItemMessageW(hwnd, IDC_COLOR_LIST, LB_ADDSTRING, 0, text.as_ptr() as LPARAM) };
            }
            // 按官方做法：样式值的下拉框与编辑框二选一显示（这里默认显示下拉框）
            unsafe {
                ShowWindow(GetDlgItem(hwnd, IDC_STYLE_VALUE_EDIT), SW_HIDE);
            }
            1
        }
        WM_COMMAND => {
            let id = (wparam & 0xffff) as i32;
            let notification = ((wparam >> 16) & 0xffff) as u32;
            if id == IDC_COLOR_SCHEME && notification == CBN_SELCHANGE {
                let Some(state) = (unsafe { state_of(hwnd) }) else {
                    return 0;
                };
                let index = unsafe {
                    SendDlgItemMessageW(hwnd, IDC_COLOR_SCHEME, CB_GETCURSEL, 0, 0)
                } as usize;
                let mut state = state.borrow_mut();
                if index < state.schemes.len() && index != state.selected {
                    state.selected = index;
                    let id = state.schemes[index].0.clone();
                    // 写用户补丁：等价于官方 customize_string("style/color_scheme", id)
                    let path = state.user.join("weasel.custom.yaml");
                    match style_settings::set_active_scheme(&path, &id) {
                        Ok(()) => {
                            state.reload();
                            // 列表内容随配色变化，重建一遍
                            unsafe {
                                SendDlgItemMessageW(hwnd, IDC_COLOR_LIST, LB_RESETCONTENT, 0, 0);
                            }
                            for (key, value) in &state.colors {
                                let text = wide(&format!("{key}   0x{value:08x}"));
                                unsafe { SendDlgItemMessageW(hwnd, IDC_COLOR_LIST, LB_ADDSTRING, 0, text.as_ptr() as LPARAM) };
                            }
                            unsafe { InvalidateRect(hwnd, None, 1) };
                        }
                        Err(error) => {
                            let text = wide(&format!("写入补丁失败：{error}"));
                            unsafe {
                                MessageBoxW(hwnd, PCWSTR(text.as_ptr()), PCWSTR(wide("提示").as_ptr()), MB_OK | MB_ICONWARNING);
                            }
                        }
                    }
                }
                return 1;
            }
            if id == IDOK || id == IDCANCEL {
                unsafe { EndDialog(hwnd, id as isize) };
                return 1;
            }
            0
        }
        WM_MEASUREITEM => {
            let item = lparam as *mut MEASUREITEMSTRUCT;
            if !item.is_null() && unsafe { (*item).CtlID } == IDC_COLOR_LIST as u32 {
                unsafe { (*item).itemHeight = 18 };
                return 1;
            }
            0
        }
        WM_DRAWITEM => {
            let item = lparam as *mut DRAWITEMSTRUCT;
            if item.is_null() || unsafe { (*item).CtlID } != IDC_COLOR_LIST as u32 {
                return 0;
            }
            let item = unsafe { &*item };
            let Some(state) = (unsafe { state_of(hwnd) }) else {
                return 0;
            };
            let state = state.borrow();
            let index = item.itemID as usize;
            let Some((key, value)) = state.colors.get(index) else {
                return 1;
            };
            unsafe {
                // 选中行底色，其余用窗口底色
                let selected = item.itemState & ODS_SELECTED != 0;
                let background = if selected {
                    GetSysColor(COLOR_HIGHLIGHT)
                } else {
                    GetSysColor(COLOR_WINDOW)
                };
                let foreground = if selected {
                    GetSysColor(COLOR_HIGHLIGHTTEXT)
                } else {
                    GetSysColor(COLOR_WINDOWTEXT)
                };
                let mut brush = CreateSolidBrush(background);
                FillRect(item.hDC, &item.rcItem, brush);
                DeleteObject(brush);

                // 色块：按颜色键的取值画一个小方块（圆整到 RGB）
                let r = ((value >> 16) & 0xff) as u8;
                let g = ((value >> 8) & 0xff) as u8;
                let b = (value & 0xff) as u8;
                let mut swatch = item.rcItem;
                swatch.left += 2;
                swatch.right = swatch.left + 24;
                swatch.top += 2;
                swatch.bottom -= 2;
                let color = RGB(r, g, b);
                brush = CreateSolidBrush(color);
                FillRect(item.hDC, &swatch, brush);
                DeleteObject(brush);
                // 色块描边，浅色配色也能看清边界
                let mut frame = swatch;
                FrameRect(item.hDC, &frame, GetSysColorBrush(COLOR_WINDOWTEXT));

                // 文字：颜色键名 + 取值
                let mut text = wide(&format!("{key}   0x{value:08x}"));
                let mut text_area = item.rcItem;
                text_area.left = swatch.right + 6;
                SetBkMode(item.hDC, TRANSPARENT as i32);
                SetTextColor(item.hDC, foreground);
                DrawTextW(item.hDC, &mut text, -1, &mut text_area, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            }
            1
        }
        WM_CLOSE => {
            unsafe { EndDialog(hwnd, IDCANCEL as isize) };
            1
        }
        _ => 0,
    }
}

/// 按官方的命令行习惯分发：`/style`、`/switcher`、`/dict`。
///
/// 返回 `true` 表示已由对话框处理（调用方不要再启动 Slint 界面）。
pub fn run_from_args(args: &[String], shared: &Path, user: &Path) -> bool {
    for arg in args {
        let arg = arg.to_ascii_lowercase();
        if arg == "/style" || arg == "--style" || arg == "-style" {
            if let Err(error) = show_style(shared, user) {
                eprintln!("weasel-settings: {error}");
            }
            return true;
        }
        if arg == "/switcher" || arg == "--switcher" || arg == "-switcher" {
            // 方案选单设定与词典管理先占位：模板已在 deployer.rc 里，过程待补。
            if let Err(error) = show_placeholder(IDD_SWITCHER_SETTING) {
                eprintln!("weasel-settings: {error}");
            }
            return true;
        }
        if arg == "/dict" || arg == "--dict" || arg == "-dict" {
            if let Err(error) = show_placeholder(IDD_DICT_MANAGEMENT) {
                eprintln!("weasel-settings: {error}");
            }
            return true;
        }
    }
    false
}

/// 仅显示模板、不接数据的对话框（方案选单设定与词典管理先用它占位，
/// 保证模板本身可编译可用；数据接线随后补）。
fn show_placeholder(dialog: u16) -> Result<(), String> {
    unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, _: LPARAM) -> isize {
        match message {
            WM_COMMAND => {
                let id = (wparam & 0xffff) as i32;
                if id == IDOK || id == IDCANCEL {
                    unsafe { EndDialog(hwnd, id as isize) };
                    return 1;
                }
                0
            }
            WM_CLOSE => {
                unsafe { EndDialog(hwnd, IDCANCEL as isize) };
                1
            }
            _ => 0,
        }
    }
    let instance = unsafe { GetModuleHandleW(None) };
    let result = unsafe {
        DialogBoxParamW(instance, MAKEINTRESOURCEW(dialog), None, Some(proc), 0)
    };
    if result == -1 {
        Err(format!("对话框 {dialog} 创建失败"))
    } else {
        Ok(())
    }
}
