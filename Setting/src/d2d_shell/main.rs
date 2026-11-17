//! Direct2D 设置界面外壳 v1（实验）：Win32 顶层窗口 + DComp 交换链 + D2D/DWrite 自绘，
//! DWM 亚克力做背景。**不动现有 Slint 设置界面**——本程序是独立 exe
//! （`Weasel-Next-Setting-D2D.exe`），先从托盘/命令行单独启动，页面内容后续逐页迁移。
//!
//! 为什么用 DComp 而不是 `ID2D1HwndRenderTarget`：真玻璃要求窗口逐像素透明
//! （圆角之外透出桌面模糊），HwndRenderTarget 拿不到逐像素 alpha；候选窗
//! （`theme/src/presenter.rs`）已经用同一条路走通，这里照搬。
//!
//! v2 范围（2026-10-31 起，2026-11-15 全部落地）：页签与 Slint 版一致的 **7 页**
//! （主页/外观/方案/常规/更新/词库/下载）。七页均已接真数据、可交互：外观读
//! `weasel_settings::themes`（与 Slint 同一份），方案页是父子树 + 输入类型子行，
//! 常规页字段可编辑（行内编辑 + 下拉），词库页接 `weasel_settings::dict`。
//! 有意保留的只读项：材质三档（用户要求隐藏，只用默认不透明）、自动更新。
//! 共享逻辑通过 `#[path]` 复用 Slint 那边的 dialogs/switcher_dialog/style_keys。

#![windows_subsystem = "windows"]
#![allow(non_snake_case, non_upper_case_globals, clippy::all)]

use std::cell::RefCell;
mod acrylic;
mod extras;
mod palette;
use palette::Palette;
mod present;
// 官方 Win32 对话框（/style、/switcher、/dict）与样式键表是框架无关的 Rust 模块，
// 直接从 Slint 二进制那份源码复用；D2D 外壳也能独立弹同一套模板。
#[path = "../style_keys.rs"]
mod style_keys;
#[path = "../dialogs.rs"]
mod dialogs;
#[path = "../dict_dialog.rs"]
mod dict_dialog;
#[path = "../switcher_dialog.rs"]
mod switcher_dialog;

use present::Presenter;
use weasel_settings::signals::{self, Signal};
use weasel_settings::themes::{ThemeRow, Themes};
use weasel_theme_support::d2d_bindings::*;
use windows_strings::w;

// ── 消息与样式常量（绑定里只生成了少数几个，其余按 Win32 ABI 常量自行定义）──────────
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_CLOSE: u32 = 0x0010;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_DPICHANGED: u32 = 0x02E0;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_NCCALCSIZE: u32 = 0x0083;
const WM_CHAR: u32 = 0x0102;
/// 输入法上屏字符（2026-11-16）：中文经 IME 上屏后走 `WM_IME_CHAR`（或宽字符 `WM_CHAR`）。
/// 本窗口是纯自绘 Win32 窗口、且原来的字符处理只认 ASCII（`0x20..=0x7E`）
/// ⇒ 「设置窗里打不了字」（中文一律被丢）。这里补上 Unicode 通路。
const WM_IME_CHAR: u32 = 0x0286;
const WM_TIMER: u32 = 0x0113;
/// 自定义窗口消息＝外部信号入口：谁都可以 `PostMessageW(hwnd, WM_APP_PAGE_SELECTED, 页号, 0)`
/// 让设置窗口进入某页（比如「方案」页 = 2），不必自己碰窗口内部状态。
const WM_APP_PAGE_SELECTED: u32 = 0x8001;

// ── 记住窗口尺寸(2026-11-01):%APPDATA%\Rime-Next\setting-window.json ──
fn window_size_file() -> Option<std::path::PathBuf> {
    weasel_common::process::user_data_dir().map(|dir| dir.join("setting-window.json"))
}

fn load_window_size() -> Option<(f32, f32)> {
    let text = std::fs::read_to_string(window_size_file()?).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let w = value.get("w")?.as_f64()? as f32;
    let h = value.get("h")?.as_f64()? as f32;
    // 合理范围护栏
    (w >= 640.0 && h >= 400.0 && w <= 3840.0 && h <= 2160.0).then_some((w, h))
}

fn save_window_size(w: f32, h: f32) {
    if let Some(path) = window_size_file()
        && let Ok(text) = serde_json::to_string(&serde_json::json!({ "w": w, "h": h }))
    {
        let _ = std::fs::write(path, text);
    }
}
const WM_NCHITTEST: u32 = 0x0084;

const WS_POPUP_: u32 = 0x8000_0000;
const WS_THICKFRAME_: u32 = 0x0004_0000;
const WS_SYSMENU_: u32 = 0x0008_0000;
const WS_MINIMIZEBOX_: u32 = 0x0002_0000;
const WS_MAXIMIZEBOX_: u32 = 0x0001_0000;
const WS_MAXIMIZE_: u32 = 0x0100_0000;
const WS_EX_APPWINDOW_: u32 = 0x0004_0000;
const WS_EX_NOREDIRECTIONBITMAP_: u32 = 0x0020_0000;

const SW_SHOW_: i32 = 5;
const SW_MINIMIZE_: i32 = 6;
const SW_MAXIMIZE_: i32 = 3;
const SW_RESTORE_: i32 = 9;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;

const HTCAPTION_: isize = 2;
const HTCLIENT_: isize = 1;
const HTLEFT_: isize = 10;
const HTRIGHT_: isize = 11;
const HTTOP_: isize = 12;
const HTTOPLEFT_: isize = 13;
const HTTOPRIGHT_: isize = 14;
const HTBOTTOM_: isize = 15;
const HTBOTTOMLEFT_: isize = 16;
const HTBOTTOMRIGHT_: isize = 17;

const GWLP_USERDATA_: i32 = -21;
const GWL_STYLE_: i32 = -16;
const DWMWA_USE_IMMERSIVE_DARK_MODE_: u32 = 20;
const DWMWA_WINDOW_CORNER_PREFERENCE_: u32 = 33;
const DWMWCP_ROUND_: i32 = 2;
const CS_VREDRAW_HREDRAW: u32 = 0x0003;
const CW_USEDEFAULT_: i32 = 0x8000_0000u32 as i32;
const IDC_ARROW_: usize = 32512;

/// 窗口尺寸（DIP）。外壳按 DPI 缩放，D2D 里统一用像素坐标。
const WINDOW_W: f32 = 1280.0;
const WINDOW_H: f32 = 860.0;
/// 主页(个人记录)的内容总高(DIP):生涯 + 9 卡 + 词云 + 时段 + 三张图各一行。
/// 2026-10-31 改版后各功能独占一行,总高超出可视区,主页从此**可滚动**。
const HOME_CONTENT_DIP: f32 = 1090.0;

/// 页签与 Slint 版对齐（`Setting/ui/main.slint` 的 `nav-items`，共 7 页）。
const PAGES: [&str; 7] = ["主页", "外观", "方案", "常规", "更新", "词库", "下载"];
const PAGE_HINTS: [&str; 7] = [
    "个人记录：击键、速度、词云（待迁移）",
    "配色方案：37 套出厂配色，点一下即时生效",
    "可用输入方案与切换",
    "常规设置字段（待迁移）",
    "版本与更新检查（待迁移）",
    "用户词典、词频与导入导出",
    "方案仓库浏览与一键安装（Rime-Next 组织）",
];

/// 「外观」页的行布局（DIP）：行高、行间距。
const ROW_H: f32 = 64.0;
const ROW_GAP: f32 = 8.0;
/// 内容区顶部（DIP）：标题 74、副标题 106，列表从 138 开始。
const LIST_TOP: f32 = 138.0;
// 页号以 `weasel_settings::signals` 为唯一真相源：两个前端、命令行、外部消息
// 都认同一套编号，避免哪天加页签时漏改一处。
/// 「外观」页的页号（`PAGES` 下标）。
const PAGE_THEMES: usize = signals::PAGE_THEMES;
/// 「主页」（个人记录）的页号。
const PAGE_HOME: usize = signals::PAGE_HOME;
/// 「常规」（表单字段）的页号。
const PAGE_SETTINGS: usize = signals::PAGE_SETTINGS;
/// 「方案」的页号。
const PAGE_SCHEMAS: usize = signals::PAGE_SCHEMAS;
/// 「词库」的页号(PAGES[5])。
const PAGE_DICTS: usize = signals::PAGE_DICTS;
/// 「更新」的页号。
const PAGE_UPDATE: usize = signals::PAGE_UPDATE;
/// 「下载」的页号(PAGES[6])。
const PAGE_DOWNLOAD: usize = signals::PAGE_DOWNLOAD;
/// 「下载」页仓库卡片行高与间距(DIP)。
const REPO_ROW_H: f32 = 92.0;
const REPO_GAP: f32 = 10.0;

/// 常规页的一行（可编辑：行内编辑 / 下拉由 `extras` 绘制）。
struct FieldRow {
    #[allow(dead_code)]
    field: weasel_settings::form::Field,
    title: String,
    value: String,
    kind: String,
    /// 是否被用户覆盖过（`false` = 继承出厂/安装配置）。
    overridden: bool,
    description: String,
}

impl FieldRow {
    /// 由 lib 的 `form::Field` 组装：值取"当前覆盖"或"继承值"。
    fn from_field(field: &weasel_settings::form::Field) -> Self {
        let field = field.clone();
        let overridden = field.selected != 0;
        let value = if !overridden {
            format!("继承 {}", field.inherited_text)
        } else if field.kind == "choice" || field.kind == "enum_number" {
            field
                .choices
                .get(field.selected as usize)
                .cloned()
                .unwrap_or_else(|| field.text.clone())
        } else {
            field.text.clone()
        };
        // 值/说明都可能很长：各截一段，别把行撑爆（完整值去配置文件看）。
        let value = match value.as_str() {
            "true" => "开".to_string(),
            "false" => "关".to_string(),
            _ if value.chars().count() > 42 => {
                format!("{}…", value.chars().take(42).collect::<String>())
            }
            _ => value,
        };
        let description = field.description.lines().next().unwrap_or("").trim().to_string();
        let description = if description.chars().count() > 34 {
            format!("{}…", description.chars().take(34).collect::<String>())
        } else {
            description
        };
        Self {
            title: field.title.clone(),
            value,
            kind: field.kind.clone(),
            overridden,
            description,
            field,
        }
    }
}

/// 窗口材质（用户 15:1x 要三版）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Material {
    /// 透明玻璃：低不透明度，靠背后的模糊透出来。
    Glass,
    /// 哑光金属：不透明深灰 + 顶边高光 + 细横纹。
    Metal,
    /// 亚克力：半透乳白，不像玻璃那样透。
    Acrylic,
}

/// 候选窗三档材质的读写都走 `weasel_settings::material`（与 Slint 版共用一份，
/// 字面值/顺序/兜底规则只在一个地方维护）。

/// 把 ARGB 的 RGB 分量按比例提亮（往白里混）。
pub(crate) fn lighten(argb: u32, amount: f32) -> u32 {
    let mix = |channel: u32| {
        let value = channel as f32 + (255.0 - channel as f32) * amount;
        (value as u32).min(255)
    };
    let r = mix((argb >> 16) & 0xFF);
    let g = mix((argb >> 8) & 0xFF);
    let b = mix(argb & 0xFF);
    (r << 16) | (g << 8) | b
}

/// 把 ARGB 的 RGB 分量按比例压暗（`factor` 越小越黑）。
pub(crate) fn darken(argb: u32, factor: f32) -> u32 {
    let r = (((argb >> 16) & 0xFF) as f32 * factor) as u32 & 0xFF;
    let g = (((argb >> 8) & 0xFF) as f32 * factor) as u32 & 0xFF;
    let b = ((argb & 0xFF) as f32 * factor) as u32 & 0xFF;
    (r << 16) | (g << 8) | b
}

/// 千分位（累计击键这类大数用）。
fn thou(value: u64) -> String {
    let text = value.to_string();
    let mut out = String::with_capacity(text.len() + text.len() / 3);
    for (index, ch) in text.chars().enumerate() {
        if index > 0 && (text.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `0.1 键/秒` 的整数 → `5.2 键/秒`。
fn key_speed_text(value: u32) -> String {
    format!("{}.{} 键/秒", value / 10, value % 10)
}

/// `0.01` 定点的整数 → `1.23`。
fn two_decimals(value: u32) -> String {
    format!("{}.{:02}", value / 100, value % 100)
}

/// 请 broker 重新部署（与官方对话框 OnOK 之后同一个动作）。
///
/// 引擎是单实例，设置程序自己拉第二个只会拿到 `AlreadyRunning`，所以必须由掌握引擎生死的
/// broker 做。挂接方式：按窗口类名 `weasel-next-broker` 找到它，`WM_COMMAND` + 命令号 1001。
/// 这里**手写 extern** 而不是用生成的绑定 —— 外壳只用这三个函数，省得为它们再生成一份。
/// 设置界面保存宿主级配置（weasel.yaml）后通知 broker：重读设置存储。
/// 与 request_deploy 同款窗口消息路径，但 broker 只重读配置（毫秒级），不部署。
fn request_settings_reload() {
    const RELOAD_SETTINGS: usize = 1016; // weasel_common::command_menu::RELOAD_SETTINGS
    const WM_COMMAND: u32 = 0x0111;
    const BROKER_WINDOW_CLASS: &str = "weasel-next-broker";

    unsafe extern "system" {
        fn EnumWindows(
            callback: Option<unsafe extern "system" fn(*mut core::ffi::c_void, isize) -> i32>,
            param: isize,
        ) -> i32;
        fn GetClassNameW(window: *mut core::ffi::c_void, buffer: *mut u16, count: u32) -> i32;
        fn PostMessageW(
            window: *mut core::ffi::c_void,
            msg: u32,
            wparam: usize,
            lparam: isize,
        ) -> i32;
    }

    unsafe extern "system" fn look_for_broker(window: *mut core::ffi::c_void, param: isize) -> i32 {
        let mut buffer = [0u16; 64];
        let length = unsafe { GetClassNameW(window, buffer.as_mut_ptr(), buffer.len() as u32) };
        if length > 0 {
            let class = String::from_utf16_lossy(&buffer[..length as usize]);
            if class == BROKER_WINDOW_CLASS {
                unsafe { *(param as *mut *mut core::ffi::c_void) = window };
                return 0;
            }
        }
        1
    }

    let mut broker: *mut core::ffi::c_void = core::ptr::null_mut();
    unsafe {
        let _ = EnumWindows(Some(look_for_broker), &mut broker as *mut _ as isize);
        if !broker.is_null() {
            let _ = PostMessageW(broker, WM_COMMAND, RELOAD_SETTINGS, 0);
        }
    }
}

fn request_deploy() {
    const DEPLOY_COMMAND: usize = 1001; // weasel_common::command_menu::DEPLOY
    const WM_COMMAND: u32 = 0x0111;
    const BROKER_WINDOW_CLASS: &str = "weasel-next-broker";

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
            if class == BROKER_WINDOW_CLASS {
                unsafe { *(param as *mut *mut core::ffi::c_void) = window };
                return 0;
            }
        }
        1
    }

    let mut found: *mut core::ffi::c_void = core::ptr::null_mut();
    unsafe {
        // 用 EnumWindows 而不是 FindWindowW：后者在部分环境/完整性级别下返回 0，
        // 表现就是"改了方案清单却没人重新部署"（Slint 版踩过）。
        EnumWindows(
            Some(look_for_broker),
            &mut found as *mut *mut core::ffi::c_void as isize,
        );
        if !found.is_null() {
            PostMessageW(found, WM_COMMAND, DEPLOY_COMMAND, 0);
            log("方案页：已请 broker 重新部署");
        } else {
            log("方案页：没找到 broker 窗口，部署请求没发出去");
        }
    }
}

/// 全局状态：UI 线程单线程使用，WndProc 直接取用（和候选窗 `Window` 同一做法）。
static mut SHELL: Option<Shell> = None;

/// 从 plum 静态索引读仓库列表（与 Slint 版 `rows_from_index` 同源，D2D 字段裁剪）。
fn repo_rows_from_index(
    index_dir: &std::path::Path,
    user_root: &std::path::Path,
) -> Option<(Vec<RepoRowData>, Vec<String>)> {
    let (_, groups) = plum_next::index::load(index_dir).ok()?;
    if groups.iter().all(|(_, recipes)| recipes.is_empty()) {
        return None;
    }
    let installed = weasel_settings::download::load_installed(user_root);
    let mut rows: Vec<RepoRowData> = Vec::new();
    let mut cats: Vec<String> = vec!["全部".to_owned()];
    for (category, recipes) in groups {
        let cat = if category.name.trim().is_empty() {
            plum_next::index::category_name(&category.key).to_owned()
        } else {
            category.name.clone()
        };
        if !cats.contains(&cat) {
            cats.push(cat.clone());
        }
        for recipe in recipes {
            let short = recipe.repo.rsplit('/').next().unwrap_or(&recipe.repo).to_owned();
            let initial = short
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_owned());
            let description = if recipe.schemas.is_empty() {
                recipe.name.clone()
            } else {
                format!("{} · {}", recipe.name, recipe.schemas.join(", "))
            };
            rows.push(RepoRowData {
                badge: weasel_settings::download::badge_for(installed.get(&recipe.repo), None),
                name: short,
                description,
                clone_url: recipe.clone_url(),
                branch: recipe.branch.clone().unwrap_or_default(),
                pushed_at: String::new(),
                category: cat.clone(),
                initial,
            });
        }
    }
    Some((rows, cats))
}

/// GitHub API 实时列举的仓库 → 行（与 Slint 版同源）。
fn repo_rows_from_org(
    repos: &[plum_next::org::Repo],
    user_root: &std::path::Path,
) -> (Vec<RepoRowData>, Vec<String>) {
    let installed = weasel_settings::download::load_installed(user_root);
    let mut rows: Vec<RepoRowData> = Vec::new();
    let mut cats: Vec<String> = vec!["全部".to_owned()];
    for r in repos {
        let cat = weasel_settings::download::category_of(&r.name, &r.topics);
        if !cats.contains(&cat) {
            cats.push(cat.clone());
        }
        let initial = r
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_owned());
        let badge = weasel_settings::download::badge_for(
            installed.get(&format!("{}/{}", plum_next::org::ORG, r.name)),
            r.pushed_at.as_deref(),
        );
        rows.push(RepoRowData {
            badge,
            name: r.name.clone(),
            description: r.description.clone().unwrap_or_default(),
            clone_url: r.clone_url.clone(),
            branch: r.default_branch.clone().unwrap_or_default(),
            pushed_at: r.pushed_at.clone().unwrap_or_default(),
            category: cat,
            initial,
        });
    }
    (rows, cats)
}

/// 用户目录 user.yaml 里的 `last_build_time`（部署完成信号，Slint 版同源）。
fn read_last_build_time() -> Option<i64> {
    let path = weasel_common::process::user_data_dir()?.join("user.yaml");
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("last_build_time:") {
            return value.trim().parse::<i64>().ok();
        }
    }
    None
}

fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── 每页候选数(与 Slint 版 dialogs.rs 同源,bin 内部模块拿不到,这里本地复刻) ──────

const PAGE_SIZE_MIN: u32 = 3;
const PAGE_SIZE_MAX: u32 = 9;

fn read_menu_page_size(path: &std::path::Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_menu = false;
    for line in text.lines() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        if trimmed == "menu:" && indent == 0 {
            in_menu = true;
            continue;
        }
        if !in_menu {
            continue;
        }
        if !trimmed.is_empty() && indent == 0 {
            break;
        }
        if indent == 2
            && let Some((key, value)) = trimmed.split_once(':')
            && key.trim() == "page_size"
            && let Ok(parsed) = value.trim().trim_matches('"').parse::<u32>()
        {
            return Some(parsed);
        }
    }
    None
}

/// 读生效的 `menu/page_size`：用户目录优先，回退出厂，再回退 5。
fn current_page_size(user: &std::path::Path, shared: &std::path::Path) -> u32 {
    read_menu_page_size(&user.join("default.yaml"))
        .or_else(|| read_menu_page_size(&shared.join("default.yaml")))
        .unwrap_or(5)
}

/// 写 `menu/page_size` 到用户目录 `default.yaml`；用户目录没有这份文件时
/// **以共享目录的出厂文件为底**（部署时用户那份整份替换共享那份，凭空造一份会
/// 丢 schema_list/主题等全部默认项）。与 Slint 版 write_page_size 同一语义。
fn write_page_size(user: &std::path::Path, shared: &std::path::Path, page_size: u32) -> std::io::Result<()> {
    let page_size = page_size.clamp(PAGE_SIZE_MIN, PAGE_SIZE_MAX);
    let path = user.join("default.yaml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => std::fs::read_to_string(shared.join("default.yaml"))?,
    };
    let mut lines: Vec<String> = raw.lines().map(str::to_string).collect();
    let indent_of = |line: &str| line.len() - line.trim_start().len();
    let entry = format!("  page_size: {page_size}");
    let menu_start = lines
        .iter()
        .position(|line| line.trim() == "menu:" && indent_of(line) == 0);
    match menu_start {
        None => {
            lines.push("menu:".to_string());
            lines.push(entry);
        }
        Some(start) => {
            let end = (start + 1..lines.len())
                .find(|&index| {
                    let trimmed = lines[index].trim();
                    !trimmed.is_empty() && !trimmed.starts_with('#') && indent_of(&lines[index]) == 0
                })
                .unwrap_or(lines.len());
            let existing = (start + 1..end).find(|&index| {
                indent_of(&lines[index]) == 2 && lines[index].trim_start().starts_with("page_size:")
            });
            match existing {
                Some(index) => lines[index] = entry,
                None => lines.insert(start + 1, entry),
            }
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

#[allow(static_mut_refs)]
fn shell() -> Option<&'static mut Shell> {
    unsafe { (*core::ptr::addr_of_mut!(SHELL)).as_mut() }
}

struct Formats {
    title: IDWriteTextFormat,
    heading: IDWriteTextFormat,
    /// 左侧导航文字(正文 1.5em,2026-11-14 用户)。
    nav: IDWriteTextFormat,
    body: IDWriteTextFormat,
    label: IDWriteTextFormat,
    glyph: IDWriteTextFormat,
    right: IDWriteTextFormat,
    /// 居中的 16px 正文字（常用字块、柱状图标签用）。
    center: IDWriteTextFormat,
    /// 粗体小字（卡片右侧注脚，用户 15:4x 要求"加粗"）。
    note: IDWriteTextFormat,
}

/// 「下载」页的一行仓库（与 Slint 版 `RepoRow` 同源，字段裁到 D2D 用得到的）。
struct RepoRowData {
    name: String,
    description: String,
    clone_url: String,
    branch: String,
    pushed_at: String,
    category: String,
    initial: String,
    /// 「已安装」/「可更新」/空（`weasel_settings::download::badge_for`）。
    badge: &'static str,
}

/// 「下载」页工作线程与 UI 线程的共享信箱：线程写，`WM_TIMER` 采样回 UI。
///
/// 为什么不用事件回调：D2D 外壳是单线程窗口过程，跨线程只有 PostMessage / 定时器
/// 两条路；定时器 100ms 采样对进度条足够顺滑，代码也最短。
#[derive(Default)]
struct RepoJob {
    /// 列表加载完成：(行, 分类, 状态行文字)。
    loaded: Option<(Vec<RepoRowData>, Vec<String>, String)>,
    /// 安装进度 0..=1。
    frac: f32,
    /// 安装结束：Ok(仓库名) / Err(错误消息)。
    done: Option<Result<String, String>>,
}

/// 词云数据：`weasel_settings::wordcloud::build()` 的返回值（词项 + 摘要）。
type CloudData = (
    Vec<weasel_settings::wordcloud::CloudItem>,
    weasel_settings::wordcloud::CloudSummary,
);

struct Shell {
    hwnd: HWND,
    dpi: u32,
    presenter: Option<Presenter>,
    formats: Option<Formats>,
    page: usize,
    hover: Hover,
    /// 配色页的数据（与 Slint 版共用 `weasel_settings::themes`）。
    themes: Option<Themes>,
    /// 配色行缓存：`paint` 每帧都要用，别每次都重读文件。
    rows: Vec<ThemeRow>,
    /// 状态行文字（读文件失败时会说明原因）。
    status: String,
    /// 主页数据（`weasel_settings::stats`，与 Slint 版同一份聚合）。
    stats: Option<weasel_settings::stats::Snapshot>,
    /// 常规页可编辑的配置文档(点击字段即改即存)。
    document: Option<weasel_settings::config::Document>,
    paths: Option<weasel_common::process::RuntimePaths>,
    /// 常规页行内编辑:正在编辑的字段行下标 + 编辑缓冲。
    editing: Option<usize>,
    edit_buf: String,
    /// 时段统计（今天 24 小时）：`(每小时击键, 每小时有效毫秒)`；老 stat.bin 是全 0。
    hours: ([u32; 24], [u64; 24]),
    /// 主页副标题：天数 / 读文件告警。
    stats_note: String,
    /// 常规页字段（可编辑）。
    fields: Vec<FieldRow>,
    /// 常规页副标题：字段数 / 读配置告警。
    settings_note: String,
    /// 方案页：行（顺序 = 界面顺序，`selected` 是未落盘的内存态）。
    schemas: Vec<weasel_style::switcher::SchemaInfo>,
    /// 已折叠的系列（树结构的折叠状态）。
    collapsed: std::collections::BTreeSet<String>,
    /// 方案页状态行（保存/部署结果）。
    schema_status: String,
    /// 出厂数据目录与用户目录（读写 `schema/schema.yaml` 用）。
    /// 面板材质。
    material: Material,
    /// 候选窗材质档（0=透明 1=亚克力 2=默认）：「外观」页三个按钮的选中态。
    ///
    /// 与 `%APPDATA%\Rime-Next\theme\material.txt` 同步；渲染器自己会重读那个文件
    /// （`theme/src/paint.rs::tier`），这里只管显示与落盘。
    candidate_material: usize,
    /// 当前皮肤（steam）的主题色：面板底、正文。
    shared_dir: std::path::PathBuf,
    user_dir: std::path::PathBuf,
    /// 「词库」页状态(2026-11-08 D2D 迁移)。
    dict_rows: Vec<(i32, String, String, String)>,
    dict_status: String,
    /// 词库扫描信箱(2026-11-16 用户「数据做成信号,点击后直接热更新」):
    /// `dict::list_dicts` 扫盘+解析全部词典 YAML 是秒级 IO,放后台线程,
    /// 结果写这里,`WM_TIMER` 采样回 UI —— 点词库页不再卡顿。
    dict_mail: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// 词库查询信箱(`dict_find` 扫全部词典找词条,同样秒级)。
    dict_find_mail: std::sync::Arc<std::sync::Mutex<Option<Vec<(i32, String, String, String)>>>>,
    dict_code: String,
    dict_word: String,
    dict_edit: usize, // 0=无 1=编码框 2=词框
    dict_scroll: f32,
    /// 「下载」页状态(2026-11-13 D2D 迁移)：仓库行、分类页签、安装进度。
    repo_rows: Vec<RepoRowData>,
    repo_cats: Vec<String>,
    /// 激活分类下标（0 = 全部）。
    repo_tab: usize,
    /// 列表状态行（加载来源 / 安装结果）。
    repo_status: String,
    /// 正在安装的仓库名（None = 空闲）。
    repo_installing: Option<String>,
    /// 正在安装行的进度 0..=1（定时器从信箱采样）。
    repo_frac: f32,
    /// 工作线程信箱（列表加载 + 安装进度/结果）。
    repo_progress: std::sync::Arc<std::sync::Mutex<RepoJob>>,
    /// 装完后的部署等待：请求部署那一刻的 `last_build_time` + 放弃期限（epoch 秒）。
    deploy_before: Option<i64>,
    deploy_deadline: i64,
    /// 每页候选数（常规页 spinner，3..9，落 `menu/page_size`）。
    page_size: u32,
    /// 更新页：安装目录（自动更新功能按用户 12:5x 指示**先不做**，页面上只留一句说明）。
    update_dir: std::path::PathBuf,
    /// 列表滚动量（像素，已按 DPI 缩放）。
    scroll: f32,
    /// WM_TIMER 计数:太阳深浅 10 秒一查。
    tick: u32,
    /// 深浅模式(0=深 1=浅 2=自动,跟随外观页三钮)。
    color_mode: usize,
    /// 主页「时段统计」:24 小时柱 + 前 7 天柱(百分比,透明度)。
    week_bars: Vec<(i32, i32)>,
    /// 时段摘要(「最活跃 9 时 · 共 120 分」)。
    hour_summary: String,
    /// 时段峰值小时。
    hour_peak: i32,
    /// 词云字号 → 文本格式缓存(词云每词字号不同,格式按 px 缓存)。
    cloud_formats: RefCell<std::collections::HashMap<i32, IDWriteTextFormat>>,
    /// 词云数据缓存（2026-11-16 修「首页/切页慢」）。
    ///
    /// `wordcloud::build()` 是重活：复制 96 个 userdb 文件到临时目录、开 15 个 LevelDB
    /// 全表扫、再排序 + 1040×400 网格排布。**实测 686 ms/次**（`wordcloud.rs` 的
    /// `bench_build_cost`）。原来它写在 `paint_home` 里，于是首页上每一次重绘
    /// （hover / 滚轮 / 点页签）都要白扣 0.69 秒，界面直接卡死。
    /// 现在只在 `refresh_stats()`（启动 / 进主页 / 重选主页）里**发一次后台重建请求**。
    cloud_cache: RefCell<Option<CloudData>>,
    /// 后台重建词云的收件箱：工作线程写，`WM_TIMER` 采样回 UI（照 `repo_progress` 的写法）。
    cloud_pending: std::sync::Arc<std::sync::Mutex<Option<CloudData>>>,
    /// 已有一个重建线程在跑 ⇒ 不再派生第二个。
    cloud_building: bool,
    /// 全部皮肤的调色板(id → 语义色),外观页色块按模式取用。
    palettes: std::collections::BTreeMap<String, Palette>,
    /// 时段柱百分比(24 小时 + 前 7 天,0..100)。
    hour_pct: Vec<i32>,
    /// 主页内容实测底边(像素,paint_home 每帧回写;滚动上限用它适配任何布局改版)。
    home_content_h: std::cell::Cell<f32>,
    /// 常规页下拉选项浮层（None = 关闭）；两个 −/+ spinner 已改为选项控件。
    option_popup: Option<extras::OptionPopup>,
    /// 下拉浮层滚动偏移（物理像素，按整行吸附）。
    option_scroll: f32,
    // ── 2026-11-15 全量对齐 Slint：以下状态由 `extras` 模块驱动 ──────────
    /// 方案页：拼音 15 项输入类型（shared 出厂清单 + user custom 覆盖）。
    pinyin_types: Option<weasel_style::switcher::PinyinInputTypes>,
    /// 方案页：禁用态快照（只读，用于父组闸门）。
    schema_disabled: std::collections::BTreeSet<String>,
    /// 方案页：搜索框文本与焦点。
    schema_filter: String,
    schema_filter_focus: bool,
    /// 常规页：候选字号（与 Slint `font_point` 同一份 user 配置）。
    font_point: u32,
    /// 常规页：脏状态 / 无效字段；只有点「保存」才落盘（与 Slint 一致）。
    dirty: bool,
    invalid: std::collections::BTreeSet<usize>,
    /// 常规页：app_options / wasm modules 子编辑器（None = 普通字段列表）。
    extra: Option<extras::ExtraEditor>,
    /// 部署进度浮层（None = 未显示）。
    deploy_overlay: Option<extras::DeployOverlay>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hover {
    None,
    Minimize,
    Maximize,
    Close,
    /// 左侧页签。
    Page(usize),
    /// 「外观」页里的配色行。
    Row(usize),
    /// 「方案」页的「保存并部署」按钮。
    Apply,
    /// 「词库」页的四个动作按钮(0=查候选 1=加入 2=删除 3=重新读取)。
    DictBtn(usize),
    /// 「下载」页的分类页签（`repo_cats` 下标）。
    RepoTab(usize),
    /// 「下载」页的仓库行「安装」按钮（过滤后列表的下标）。
    RepoBtn(usize),
    /// 「下载」页右上「刷新列表」按钮。
    RepoRefresh,
    /// 「常规」页两个下拉选项框（0 = 候选字号 1 = 每页候选数）。
    OptionBox(usize),
    /// 下拉浮层里的选项行（下标 = 档位数组下标）。
    OptionItem(usize),
    /// 点下拉浮层外：只关闭浮层。
    OptionDismiss,
    /// 「外观」页的三档材质按钮（下标 0=glass 1=acrylic 2=metal）。
    Material(usize),
    /// 深浅三钮(0=深色 1=浅色 2=自动)。
    ColorMode(usize),
    /// 布局三钮(0=横排 1=竖排 2=竹简)。
    LayoutBtn(usize),
    /// 方案页搜索框。
    SchemaFilter,
    /// 方案页「拼音」主方案行左侧的折叠三角。
    SchemaChevron(usize),
    /// 方案页官方对话框按钮(0=菜单设置 1=外观设置 2=用户词典)。
    Official(usize),
    /// 常规页底部按钮(0=保存 1=保存并应用 2=恢复默认)。
    SaveBtn(usize),
    /// 常规页字段行右侧「恢复继承」。
    RowReset(usize),
    /// 常规页 app_options / modules 字段右侧「展开」。
    ExtraOpen(usize),
    /// 子编辑器：返回、行、动作。
    ExtraBack,
    ExtraRow(usize),
    ExtraAction(usize),
    /// 主页「独立统计窗口」按钮。
    StatsWindow,
    /// 方案页「带进度部署」按钮。
    DeployProgress,
}

/// `0xAARRGGBB` → D2D 颜色（直通 alpha；D2D 内部自己预乘）。
fn color(argb: u32) -> D2D_COLOR_F {
    D2D_COLOR_F {
        r: ((argb >> 16) & 0xff) as f32 / 255.0,
        g: ((argb >> 8) & 0xff) as f32 / 255.0,
        b: (argb & 0xff) as f32 / 255.0,
        a: ((argb >> 24) & 0xff) as f32 / 255.0,
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn log(message: &str) {
    use std::io::Write;
    let Some(dir) = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .map(|base| base.join("Rime-Next").join("Logs"))
    else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("settings-shell.log"))
    {
        let _ = writeln!(file, "{message}");
    }
}

/// 把一个字符送进「当前处于编辑态」的文本框；没有任何编辑目标时返回 false。
///
/// 顺序与 `WM_CHAR` 的分派一致：搜索框/子编辑器 → 词库编码框/词框 → 常规页行内编辑。
/// 编码框只收 ASCII（码表编码），其余框收任意可打印字符（含中文，2026-11-16 用户
/// 「设置窗里打不了字」）。
fn insert_typed_char(s: &mut Shell, ch: char) -> bool {
    let code = ch as u32;
    if extras::on_char(s, code) {
        return true;
    }
    if s.page == PAGE_DICTS && s.dict_edit > 0 {
        if s.dict_edit == 2 || ch.is_ascii() {
            let buf = if s.dict_edit == 1 {
                &mut s.dict_code
            } else {
                &mut s.dict_word
            };
            buf.push(ch);
        }
        return true;
    }
    if s.editing.is_some() {
        s.edit_push(ch);
        return true;
    }
    false
}

/// Liang-Barsky：把线段裁到矩形内（裁掉的部分在窗口外，画不出来，纯省算力）。
fn clip_segment(
    from: (f32, f32),
    to: (f32, f32),
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
) -> Option<(windows_numerics::Vector2, windows_numerics::Vector2)> {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    for (p, q) in [
        (-dx, from.0 - min_x),
        (dx, max_x - from.0),
        (-dy, from.1 - min_y),
        (dy, max_y - from.1),
    ] {
        if p.abs() < f32::EPSILON {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return None;
            }
            if r > t0 {
                t0 = r;
            }
        } else {
            if r < t0 {
                return None;
            }
            if r < t1 {
                t1 = r;
            }
        }
    }
    Some((
        windows_numerics::Vector2 { x: from.0 + dx * t0, y: from.1 + dy * t0 },
        windows_numerics::Vector2 { x: from.0 + dx * t1, y: from.1 + dy * t1 },
    ))
}

/// 把所有线段塞进**一条** path geometry（多个 figure），之后每帧只提交一次 DrawGeometry。
/// 2026-11-16：AC2 网格/放射线从「每根线一条几何」改成这个，90 次 DrawGeometry → 2 次。
fn lines_geometry(
    factory: &ID2D1Factory1,
    segments: &[(windows_numerics::Vector2, windows_numerics::Vector2)],
) -> Option<ID2D1PathGeometry> {
    unsafe {
        let path = factory.CreatePathGeometry().ok()?;
        let sink = path.Open().ok()?;
        for (from, to) in segments {
            sink.BeginFigure(*from, D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddLine(*to);
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
        }
        let _ = sink.Close();
        Some(path)
    }
}

/// AC2 网格两族几何的跨帧缓存。
/// 键含 D2D 工厂指针（同源要求，换了 Presenter 设备就重建）+ 窗口宽高 + 间距。
fn grid_geometries(
    factory: &ID2D1Factory1,
    w: f32,
    h: f32,
    spacing: f32,
    count: i32,
    half: f32,
) -> Option<std::rc::Rc<[ID2D1PathGeometry; 2]>> {
    type Cache = Option<(usize, u32, u32, u32, std::rc::Rc<[ID2D1PathGeometry; 2]>)>;
    thread_local! {
        static CACHE: std::cell::RefCell<Cache> = const { std::cell::RefCell::new(None) };
    }
    let factory_key = windows_core::Interface::as_raw(factory) as usize;
    let (wb, hb, sb) = (w.to_bits(), h.to_bits(), spacing.to_bits());
    let hit = CACHE.with(|cache| {
        cache.borrow().as_ref().and_then(|(f, cw, ch, cs, geos)| {
            (*f == factory_key && (*cw, *ch, *cs) == (wb, hb, sb)).then(|| geos.clone())
        })
    });
    if hit.is_some() {
        return hit;
    }
    let cx = w * 0.5;
    let cy = h * 0.5;
    // 窗口外一律看不见；只留 2 个间距给平移（phase 最大 2*spacing），线长直接减半。
    let margin = spacing * 2.0 + 2.0;
    let (min_x, min_y, max_x, max_y) = (-margin, -margin, w + margin, h + margin);
    let mut built = Vec::with_capacity(2);
    for angle_deg in [45.0f32, 135.0] {
        let (nx, ny) = (angle_deg.to_radians().cos(), angle_deg.to_radians().sin());
        let base = (cx * nx + cy * ny) % spacing;
        // 多给 2 条：平移最多 2*spacing，靠多出来的线补位，窗口边缘不会露空。
        let mut segments = Vec::with_capacity((count as usize) + 5);
        for k in -(count / 2) - 2..=(count / 2) + 2 {
            let d = base + k as f32 * spacing;
            let px = cx + nx * d;
            let py = cy + ny * d;
            let from = (px - ny * half, py + nx * half);
            let to = (px + ny * half, py - nx * half);
            if let Some(segment) = clip_segment(from, to, min_x, min_y, max_x, max_y) {
                segments.push(segment);
            }
        }
        built.push(lines_geometry(factory, &segments)?);
    }
    let geos: [ID2D1PathGeometry; 2] = built.try_into().ok()?;
    let rc = std::rc::Rc::new(geos);
    CACHE.with(|cache| *cache.borrow_mut() = Some((factory_key, wb, hb, sb, rc.clone())));
    Some(rc)
}

/// AC2 放射线几何的跨帧缓存：24 根按 0° 基准建一条几何，每帧用旋转矩阵转到 `rot_deg`。
fn ray_geometry(
    factory: &ID2D1Factory1,
    w: f32,
    h: f32,
    r0: f32,
    r1: f32,
) -> Option<std::rc::Rc<ID2D1PathGeometry>> {
    type Cache = Option<(usize, u32, u32, u32, u32, std::rc::Rc<ID2D1PathGeometry>)>;
    thread_local! {
        static CACHE: std::cell::RefCell<Cache> = const { std::cell::RefCell::new(None) };
    }
    let factory_key = windows_core::Interface::as_raw(factory) as usize;
    let (wb, hb, r0b, r1b) = (w.to_bits(), h.to_bits(), r0.to_bits(), r1.to_bits());
    let hit = CACHE.with(|cache| {
        cache.borrow().as_ref().and_then(|(f, cw, ch, c0, c1, geo)| {
            (*f == factory_key && (*cw, *ch, *c0, *c1) == (wb, hb, r0b, r1b)).then(|| geo.clone())
        })
    });
    if hit.is_some() {
        return hit;
    }
    let cx = w * 0.5;
    let cy = h * 0.5;
    let mut segments = Vec::with_capacity(24);
    for i in 0..24 {
        let a = (i as f32 * (360.0 / 24.0)).to_radians();
        let (ca, sa) = (a.cos(), a.sin());
        segments.push((
            windows_numerics::Vector2 { x: cx + ca * r0, y: cy + sa * r0 },
            windows_numerics::Vector2 { x: cx + ca * r1, y: cy + sa * r1 },
        ));
    }
    let rc = std::rc::Rc::new(lines_geometry(factory, &segments)?);
    CACHE.with(|cache| *cache.borrow_mut() = Some((factory_key, wb, hb, r0b, r1b, rc.clone())));
    Some(rc)
}

/// 折线描边(AC2 层专用):一条 path,N 个点,HOLLOW 开始 + OPEN 结束。
fn ac2_stroke(
    rt: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    factory: &ID2D1Factory1,
    pts: &[windows_numerics::Vector2],
    ink: u32,
    width: f32,
) {
    if pts.len() < 2 {
        return;
    }
    unsafe {
        let Ok(path) = factory.CreatePathGeometry() else {
            return;
        };
        let Ok(sink) = path.Open() else {
            return;
        };
        sink.BeginFigure(pts[0], D2D1_FIGURE_BEGIN_HOLLOW);
        for p in &pts[1..] {
            sink.AddLine(*p);
        }
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        let _ = sink.Close();
        brush.SetColor(&color(ink));
        rt.DrawGeometry(&path, brush, width, None::<&ID2D1StrokeStyle>);
    }
}

/// 斜切高亮条(AC2 菜单选中语言):平行四边形,左右两侧各切 `slant` px,直角。
/// 用于导航选中 pill / 配色行当前项 —— 底色仍走语义 token(形状 AC2 化,配色不变)。
fn draw_fill_slanted(
    rt: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    factory: &ID2D1Factory1,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    slant: f32,
    argb: u32,
) {
    if w <= slant * 2.0 || h <= 1.0 {
        return;
    }
    unsafe {
        let Ok(path) = factory.CreatePathGeometry() else {
            return;
        };
        let Ok(sink) = path.Open() else {
            return;
        };
        let p = |px: f32, py: f32| windows_numerics::Vector2 { x: px, y: py };
        sink.BeginFigure(p(x + slant, y), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(p(x + w, y));
        sink.AddLine(p(x + w - slant, y + h));
        sink.AddLine(p(x, y + h));
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        let _ = sink.Close();
        brush.SetColor(&color(argb));
        rt.FillGeometry(&path, brush, None::<&ID2D1Brush>);
    }
}

/// 方案页分组标题的折叠箭头：矢量实心三角（与 Slint 版 `schemas.slint` 的 Path 同款，
/// 2026-11-14 用户「方案界面那个箭头做成同款大号箭头，现在的有些小」）。
/// `down=true` 展开朝下、`false` 收起朝右。
/// 不用字符：字体里 `▸/▾` 易缺字、`▶` 会被彩色 emoji 字体接管（Slint 版同结论）。
fn draw_fill_triangle(
    rt: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    factory: &ID2D1Factory1,
    x: f32,
    y: f32,
    size: f32,
    down: bool,
    argb: u32,
) {
    unsafe {
        let Ok(path) = factory.CreatePathGeometry() else {
            return;
        };
        let Ok(sink) = path.Open() else {
            return;
        };
        let p = |px: f32, py: f32| windows_numerics::Vector2 { x: px, y: py };
        sink.BeginFigure(p(x, y), D2D1_FIGURE_BEGIN_FILLED);
        if down {
            sink.AddLine(p(x + size, y));
            sink.AddLine(p(x + size * 0.5, y + size));
        } else {
            sink.AddLine(p(x + size, y + size * 0.5));
            sink.AddLine(p(x, y + size));
        }
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        let _ = sink.Close();
        brush.SetColor(&color(argb));
        rt.FillGeometry(&path, brush, None::<&ID2D1Brush>);
    }
}

/// AC2 背景:45° 菱形网格 + 中央放射线 + 同心菱形徽记 + 四周晕影。
/// 与候选窗 `theme/src/paint.rs::paint_ac2` 同一套语言(2026-11-14 用户「做成
/// 刺客信条2的背景,不要图片,只要它的背景配色的布局,按它的动画做一下」):
/// 网格沿对角漂移、放射线缓转、徽记呼吸 —— 相位全部取系统毫秒,重绘间隔粗
/// 也不跳变。深色 = 白线;浅色 = 墨线(文艺复兴手稿感)。
/// 全层 PushLayer 裁进面板圆角,不会戳到圆角外。
/// 白光斜扫(HTML `.candidate::before` 同款,与候选窗 `diagonal_sheen` 一致):
/// 左上 25% 白 → 中段全透 → 右下 12% 白。叠在 AC2 几何层上补「高光」
/// (2026-11-14 用户:「这个不够亮,不是很好看,高光一下」)。
/// HTML 皮肤展示页同款彩色多层渐变（11-15 23:0x「颜色的渐变呢，纯棕褐色啊」）：
///   ① 135° panel→card 彩色底；② 左上 accent2 光晕；③ 右下 accent 光晕。
/// 用线性渐变近似 CSS 的 radial-gradient，D2D 的 CLAMP 让轴外保持透明端点，
/// 三层都铺全窗也互不串色。取代原来的白色斜扫（那层被用户说成「雾气」）。
fn paint_skin_gradient(rt: &ID2D1RenderTarget, w: f32, h: f32) {
    unsafe {
        let pal = palette::get();
        let full = D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h };
        let tint = |rgb: u32, a: f32| D2D_COLOR_F {
            r: ((rgb >> 16) & 0xff) as f32 / 255.0,
            g: ((rgb >> 8) & 0xff) as f32 / 255.0,
            b: (rgb & 0xff) as f32 / 255.0,
            a,
        };
        let layer = |from: (f32, f32), to: (f32, f32), stops: &[D2D1_GRADIENT_STOP]| {
            if let Ok(collection) = rt.CreateGradientStopCollection(
                stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
            {
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: windows_numerics::Vector2 { x: from.0, y: from.1 },
                    endPoint: windows_numerics::Vector2 { x: to.0, y: to.1 },
                };
                if let Ok(gb) = rt.CreateLinearGradientBrush(&props, None, &collection) {
                    rt.FillRectangle(&full, &gb);
                }
            }
        };
        // ① 135° 底色：panel → 提亮一档的 card，两端真的有色差。
        layer((0.0, 0.0), (w, h), &[
            D2D1_GRADIENT_STOP { position: 0.0, color: color(pal.panel | 0xFF00_0000) },
            D2D1_GRADIENT_STOP { position: 1.0, color: color(crate::lighten(pal.card, 0.22) | 0xFF00_0000) },
        ]);
        // ② 左上 accent2（浅金 / 浅色强调字）。
        layer((0.0, 0.0), (w * 0.78, h * 0.68), &[
            D2D1_GRADIENT_STOP { position: 0.0, color: tint(pal.sel_text, 0.55) },
            D2D1_GRADIENT_STOP { position: 0.55, color: tint(pal.sel_text, 0.18) },
            D2D1_GRADIENT_STOP { position: 1.0, color: tint(pal.sel_text, 0.0) },
        ]);
        // ③ 右下 accent（强调色）。
        layer((w, h), (w * 0.22, h * 0.32), &[
            D2D1_GRADIENT_STOP { position: 0.0, color: tint(pal.accent, 0.44) },
            D2D1_GRADIENT_STOP { position: 0.60, color: tint(pal.accent, 0.12) },
            D2D1_GRADIENT_STOP { position: 1.0, color: tint(pal.accent, 0.0) },
        ]);
    }
}

/// 四通道求导光场(2026-11-17 用户「做四通道求导吧」):
/// 每团光 = 一个**径向**渐变 —— 每个通道(R,G,B,A)是「中心色 → 边缘透明」的
/// 解析导数场,一个 brush 一次 draw call;不再用两条对角线性 brush 近似光球。
/// 底色场保留线性(panel → card 提亮,两端真有色差)。
fn paint_color_field(rt: &ID2D1RenderTarget, w: f32, h: f32) {
    unsafe {
        let pal = palette::get();
        let full = D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h };
        // ① 底色场:panel → card 提亮(线性,135°)。
        if let Ok(collection) = rt.CreateGradientStopCollection(
            &[
                D2D1_GRADIENT_STOP { position: 0.0, color: color(pal.panel | 0xFF00_0000) },
                D2D1_GRADIENT_STOP { position: 1.0, color: color(crate::lighten(pal.card, 0.22) | 0xFF00_0000) },
            ],
            D2D1_GAMMA_2_2,
            D2D1_EXTEND_MODE_CLAMP,
        ) {
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: windows_numerics::Vector2 { x: 0.0, y: 0.0 },
                endPoint: windows_numerics::Vector2 { x: w, y: h },
            };
            if let Ok(gb) = rt.CreateLinearGradientBrush(&props, None, &collection) {
                rt.FillRectangle(&full, &gb);
            }
        }
        // ② 四通道求导光球:中心色 → 边缘 alpha=0,四通道同步衰减。
        let orb = |cx: f32, cy: f32, radius: f32, argb: u32, peak: f32| {
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: D2D_COLOR_F {
                        r: ((argb >> 16) & 0xFF) as f32 / 255.0,
                        g: ((argb >> 8) & 0xFF) as f32 / 255.0,
                        b: (argb & 0xFF) as f32 / 255.0,
                        a: peak,
                    },
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: D2D_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
                },
            ];
            if let Ok(collection) = rt.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) {
                let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: windows_numerics::Vector2 { x: cx, y: cy },
                    gradientOriginOffset: windows_numerics::Vector2 { x: 0.0, y: 0.0 },
                    radiusX: radius,
                    radiusY: radius,
                };
                if let Ok(rb) = rt.CreateRadialGradientBrush(&props, None, &collection) {
                    rt.FillRectangle(&full, &rb);
                }
            }
        };
        // 左上浅光球(sel_text)+ 右下强调光球(accent),中心都压在窗角外一点,
        // 让可见部分是球面的中段 —— 渐变读起来更"场"。
        orb(w * 0.08, h * 0.02, w * 0.62, pal.sel_text, 0.50);
        orb(w * 0.94, h * 0.98, w * 0.55, pal.accent, 0.40);
    }
}

fn paint_sheen(rt: &ID2D1RenderTarget, w: f32, h: f32) {
    unsafe {
        let start_point = windows_numerics::Vector2 { x: 0.0, y: 0.0 };
        let end_point = windows_numerics::Vector2 { x: w * 0.72, y: h };
        let stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: D2D_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.25 },
            },
            D2D1_GRADIENT_STOP {
                position: 0.38,
                color: D2D_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.0 },
            },
            D2D1_GRADIENT_STOP {
                position: 0.62,
                color: D2D_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.0 },
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: D2D_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.12 },
            },
        ];
        if let Ok(stop_collection) =
            rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
        {
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: start_point,
                endPoint: end_point,
            };
            if let Ok(gb) = rt.CreateLinearGradientBrush(&props, None, &stop_collection) {
                rt.FillRectangle(&D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h }, &gb);
            }
        }
    }
}

fn paint_ac2(
    rt: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    factory: &ID2D1Factory1,
    w: f32,
    h: f32,
    panel_radius: f32,
    dark: bool,
) {
    unsafe {
        // 2026-11-16：原来把 epoch 毫秒直接 `as f32`（约 1.7e12）；f32 只有 24 位尾数，
        // 精度约 131072 ms ⇒ drift/rot/breath 每 ~131 秒才变一次，动画看着像冻住。
        // 动画已永久废弃(2026-11-17 用户):相位固定,几何层纯静态,不再取系统毫秒。
        let now_ms = 0.0_f32;
        let cx = w * 0.5;
        let cy = h * 0.5;
        let (grid_ink, ray_ink, mark_ink, veil_ink) = if dark {
            (0x14FF_FFFFu32, 0x24FF_FFFF, 0x3CFF_FFFF, 0x8C00_0000)
        } else {
            (0x1000_0000, 0x1C00_0000, 0x2E00_0000, 0x4600_0000)
        };
        let radius = (w.max(h) * 0.5).max(1.0);

        // 2026-11-16：外层 `paint()` 已经用「面板同半径圆角图层」把整帧裁进圆角，
        // 这里不再自己 PushLayer（少一层全窗离屏，圆角只裁一次）。

        // ① 45° 菱形网格(两向线族,沿法向漂移)。
        // 2026-11-16 性能修复：原来 `family()` 每帧循环**新建 90 条 2 点 path geometry**
        // 再各 `DrawGeometry` 一次，实测这一项吃 ~150 ms/帧（≈整进程 CPU 的九成）。
        // 漂移只是沿法向平移 ⇒ 每族几何**只建一次并缓存**，每帧用 SetTransform 平移，
        // 2 次 DrawGeometry 取代 90 次；线长也从 2*diag 收到 diag/2+余量，画面逐像素一致。
        let spacing = 46.0_f32.max(panel_radius * 2.2);
        let diag = (w * w + h * h).sqrt();
        let count = ((diag / spacing).ceil() as i32 + 2).max(2);
        let half = diag * 0.5 + spacing * 2.0;
        let drift = (now_ms * 0.006) % (spacing * 2.0);
        if let Some(geos) = grid_geometries(&factory, w, h, spacing, count, half) {
            for (index, phase) in [drift, drift * 0.7 + 23.0].iter().enumerate() {
                let angle_deg = if index == 0 { 45.0f32 } else { 135.0 };
                let (nx, ny) = (angle_deg.to_radians().cos(), angle_deg.to_radians().sin());
                let transform =
                    windows_numerics::Matrix3x2::translation(nx * phase, ny * phase);
                rt.SetTransform(&transform);
                brush.SetColor(&color(grid_ink));
                rt.DrawGeometry(&geos[index], brush, 1.0, None::<&ID2D1StrokeStyle>);
            }
            rt.SetTransform(&windows_numerics::Matrix3x2::identity());
        }

        // ② 放射线 24 根(外段),整组 2.5°/s 缓转(144 秒一圈)。
        // 同一套缓存思路：24 根建成一条几何，每帧只换旋转矩阵。
        let rot_deg = (now_ms * 0.0025) % 360.0;
        let r0 = radius * 0.30;
        let r1 = radius * 1.45;
        if let Some(geo) = ray_geometry(&factory, w, h, r0, r1) {
            let transform = windows_numerics::Matrix3x2::rotation_around(
                rot_deg,
                windows_numerics::Vector2 { x: cx, y: cy },
            );
            rt.SetTransform(&transform);
            brush.SetColor(&color(ray_ink));
            rt.DrawGeometry(&*geo, brush, 1.0, None::<&ID2D1StrokeStyle>);
            rt.SetTransform(&windows_numerics::Matrix3x2::identity());
        }

        // ③ 同心菱形徽记(3.2 秒呼吸 ±30%)+ 超出四角的大菱形。
        let breath = 0.72 + 0.28 * (now_ms * std::f32::consts::TAU / 3200.0).sin();
        let breathe = |argb: u32, scale: f32| {
            let a = (((argb >> 24) as f32 / 255.0) * breath * scale * 255.0).round() as u32;
            (a << 24) | (argb & 0x00FF_FFFF)
        };
        let diamond = |half: f32, ink: u32| {
            ac2_stroke(
                rt,
                brush,
                &factory,
                &[
                    windows_numerics::Vector2 { x: cx, y: cy - half },
                    windows_numerics::Vector2 { x: cx + half, y: cy },
                    windows_numerics::Vector2 { x: cx, y: cy + half },
                    windows_numerics::Vector2 { x: cx - half, y: cy },
                    windows_numerics::Vector2 { x: cx, y: cy - half },
                ],
                ink,
                1.0,
            );
        };
        diamond(radius * 0.34, breathe(mark_ink, 1.0));
        diamond(radius * 0.52, breathe(mark_ink, 0.6));
        diamond(radius * 1.55, breathe(mark_ink, 0.45));

        // ④ 四周晕影(AC2 vignette):四条向内渐变,边缘 → 透明。
        let edge = (w.min(h) * 0.22).max(28.0);
        // 2026-11-14 实测(采样 #A89997):veil 铺全窗时,轴外区域并不会 clamp 成
        // 透明端点色,8 条黑渐变整窗蒙暗 —— FillRectangle **只铺各自的边缘条带**,
        // 渐变轴与条带重合,所见即所得。
        let veil = |rect: (f32, f32, f32, f32), from: (f32, f32), to: (f32, f32)| {
            let stops = [
                D2D1_GRADIENT_STOP { position: 0.0, color: color(veil_ink) },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: D2D_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
                },
            ];
            if let Ok(collection) = rt.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) {
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: windows_numerics::Vector2 { x: from.0, y: from.1 },
                    endPoint: windows_numerics::Vector2 { x: to.0, y: to.1 },
                };
                if let Ok(vb) = rt.CreateLinearGradientBrush(&props, None, &collection) {
                    rt.FillRectangle(
                        &D2D_RECT_F { left: rect.0, top: rect.1, right: rect.2, bottom: rect.3 },
                        &vb,
                    );
                }
            }
        };
        veil((0.0, 0.0, w, edge), (0.0, 0.0), (0.0, edge));                 // 顶
        veil((0.0, h - edge, w, h), (0.0, h), (0.0, h - edge));             // 底
        veil((0.0, 0.0, edge, h), (0.0, 0.0), (edge, 0.0));                 // 左
        veil((w - edge, 0.0, w, h), (w, 0.0), (w - edge, 0.0));             // 右
    }
}

/// 画填充圆角矩形（`0xAARRGGBB`）。外层 `paint` 与各页绘制函数共用。
fn draw_fill_round(
    rt: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    argb: u32,
) {
    unsafe {
        brush.SetColor(&color(argb));
        let rounded = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            radiusX: radius,
            radiusY: radius,
        };
        rt.FillRoundedRectangle(&rounded, brush);
    }
}

/// 把长说明裁成单行（CJK 按两个西文宽计），避免换行后压到下一块内容上。
/// `unit` 是当前字号下单个 ASCII 字符的大致宽度（DIP×scale）。
fn fit_text(text: &str, max_w: f32, unit: f32) -> String {
    if max_w <= unit * 2.0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0.0;
    for ch in text.chars() {
        let cw = if (ch as u32) >= 0x2E80 { unit * 2.0 } else { unit };
        if used + cw > max_w - unit {
            out.push('…');
            break;
        }
        out.push(ch);
        used += cw;
    }
    out
}

/// 主页分区卡片：只有主题卡片底（半透明，保留亚克力纹理），不再画外圈描边。
/// 2026-11-16 用户要求「去掉首页的白色边框」：边框是主题 `border_color`（白色），
/// 去掉描边属于结构改动，颜色值一个不动。
fn draw_home_card(
    rt: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    s: f32,
) {
    let pal = palette::get();
    draw_fill_round(
        rt,
        brush,
        x,
        y,
        w,
        h,
        10.0 * s,
        (pal.card & 0x00FF_FFFF) | 0xD000_0000,
    );
}

/// 热力图 / 键帽的五档色：由当前配色 `accent` 与 `card` 混合派生（浅→深），
/// 第 0 档是「没数据」用中性覆盖层。取代此前写死的四档绿。
fn heat_tiers() -> [u32; 5] {
    let pal = palette::get();
    let base = pal.card | 0xFF00_0000;
    let accent = pal.accent | 0xFF00_0000;
    let blend = |t: f32| -> u32 {
        let ch = |shift: u32| -> u32 {
            let a = ((base >> shift) & 0xFF) as f32;
            let b = ((accent >> shift) & 0xFF) as f32;
            ((a + (b - a) * t) as u32) & 0xFF
        };
        0xFF00_0000 | (ch(16) << 16) | (ch(8) << 8) | ch(0)
    };
    [
        pal.overlay_mid(),
        blend(0.30),
        blend(0.55),
        blend(0.80),
        crate::darken(accent, 0.75) | 0xFF00_0000,
    ]
}

/// 画填充矩形。
fn draw_fill_rect(
    rt: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    argb: u32,
) {
    unsafe {
        brush.SetColor(&color(argb));
        let rect = D2D_RECT_F {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        };
        rt.FillRectangle(&rect, brush);
    }
}

fn create_text_format(    factory: &IDWriteFactory,
    family: &str,
    weight: DWRITE_FONT_WEIGHT,
    size: f32,
    align: DWRITE_TEXT_ALIGNMENT,
) -> Result<IDWriteTextFormat, String> {
    // PCWSTR 必须带结尾 0：`wide()` 不带，这里单独补。
    let family: Vec<u16> = family.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let format = factory
            .CreateTextFormat(
                windows_core::PCWSTR::from_raw(family.as_ptr()),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("zh-CN"),
            )
            .map_err(|error| format!("CreateTextFormat 失败: {error}"))?;
        format
            .SetTextAlignment(align)
            .ok()
            .map_err(|error| format!("SetTextAlignment 失败: {error}"))?;
        format
            .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
            .ok()
            .map_err(|error| format!("SetParagraphAlignment 失败: {error}"))?;
        Ok(format)
    }
}

impl Shell {
    fn scale(&self) -> f32 {
        // 用户 2026-10-31:默认字号太小看不清 ⇒ 缩放不到 125% 的显示也按 125% 起。
        (self.dpi as f32 / 96.0).max(1.25)
    }

    fn client_size(&self) -> (f32, f32) {
        unsafe {
            let mut rect = RECT::default();
            if !GetClientRect(self.hwnd, &mut rect).as_bool() {
                return (0.0, 0.0);
            }
            ((rect.right - rect.left) as f32, (rect.bottom - rect.top) as f32)
        }
    }

    /// 标题栏右侧三个按钮的命中区（像素坐标）。
    fn button_rects(&self) -> [(f32, f32, f32, f32); 3] {
        let s = self.scale();
        let (w, _) = self.client_size();
        let bw = 44.0 * s;
        let bh = 30.0 * s;
        let y = 11.0 * s;
        let close_x = w - bw - 8.0 * s;
        [
            (close_x - bw * 2.0, y, bw, bh), // 最小化
            (close_x - bw, y, bw, bh),       // 最大化
            (close_x, y, bw, bh),            // 关闭
        ]
    }

    /// 右侧内容区：左上角与可用尺寸（像素）。
    fn content_area(&self) -> (f32, f32, f32, f32) {
        let s = self.scale();
        // client_size 是物理像素,布局坐标全部按 DIP×s 绘制 ⇒ 宽高先除回 s
        // 换成 DIP,否则 125% 缩放下内容按 1.25 倍画、右缘必裁(双重缩放)。
        let (w, h) = self.client_size();
        let (w, h) = (w / s, h / s);
        let x = 144.0 * s;
        let width = (w - 176.0 - 24.0).max(80.0) * s;
        (x, 74.0 * s, width, (h - 74.0 - 24.0).max(40.0) * s)
    }

    /// 列表可视区顶边与高度（像素）。方案页底部要给「保存并部署」留一条。
    /// 词库页上方有两行控件（按钮行 + 输入行）、下载页有一行（页签行），列表都要往下让。
    fn list_viewport(&self) -> (f32, f32) {
        let s = self.scale();
        let (_, h) = self.client_size();
        let top = if self.page == PAGE_DICTS {
            (LIST_TOP + 88.0) * s
        } else if self.page == PAGE_DOWNLOAD {
            (LIST_TOP + 48.0) * s
        } else if self.page == PAGE_THEMES {
            (LIST_TOP + 72.0) * s
        } else if self.page == PAGE_SETTINGS {
            // 2026-11-16 20:5x 用户要求：「候选窗字号」「每页候选数」两个下拉改成
            // 字段列表最前两行，顶部不再留独立横条（见 `list_len`/`paint_settings`）。
            LIST_TOP * s
        } else if self.page == PAGE_SCHEMAS {
            // 方案页顶部留一条搜索框 + 批量操作按钮。
            (LIST_TOP + 48.0) * s
        } else {
            LIST_TOP * s
        };
        let bottom_reserve = if self.page == PAGE_SCHEMAS || self.page == PAGE_SETTINGS {
            // 底部一排按钮 + 状态行；留足一整行，避免半行压到按钮上。
            170.0 * s
        } else {
            40.0 * s
        };
        (top, (h - top - bottom_reserve).max(40.0 * s))
    }

    /// 当前页可滚动的行数（0 = 该页没有列表）。
    fn list_len(&self) -> usize {
        match self.page {
            PAGE_THEMES => self.rows.len(),
            // 常规页：字段列表最前面固定两行是「候选窗字号」「每页候选数」下拉行。
            PAGE_SETTINGS => self.extra.as_ref().map_or(self.fields.len() + 2, |e| e.rows.len()),
            PAGE_SCHEMAS => self.schema_items().len(),
            PAGE_DOWNLOAD => self.repo_filtered().len(),
            _ => 0,
        }
    }

    /// 「外观」页顶部三档材质按钮的矩形（像素）。
    /// 深浅三钮:材质行右对齐(0=深色 1=浅色 2=自动)。
    fn color_mode_buttons(&self) -> Vec<(f32, f32, f32, f32)> {
        let s = self.scale();
        let (x, top, width, _) = self.content_area();
        let (w, h) = (72.0 * s, 30.0 * s);
        (0..3)
            .map(|index| {
                (
                    x + width - (3 - index) as f32 * (w + 8.0 * s) + 8.0 * s,
                    top + 56.0 * s,
                    w,
                    h,
                )
            })
            .collect()
    }

    fn paint_color_mode_buttons(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        f: &Formats,
        s: f32,
    ) {
        for (index, (bx, by, bw, bh)) in self.color_mode_buttons().into_iter().enumerate() {
            let active = self.color_mode == index;
            let hovered = matches!(self.hover, Hover::ColorMode(i) if i == index);
            let bg = if active {
                0x24000000 | (palette::get().accent & 0xFFFFFF)
            } else if hovered {
                palette::get().overlay_mid()
            } else {
                palette::get().overlay_weak()
            };
            draw_fill_round(rt, brush, bx, by, bw, bh, 8.0 * s, bg);
            if active {
                // 选中条贴满圆角切完后的左侧直边：起点 +r、长度 h-2r（r = 底色圆角）
                let r = 8.0 * s;
                let bar_h = bh - 2.0 * r;
                let bar_y = by + r;
                draw_fill_round(rt, brush, bx, bar_y, 3.0 * s, bar_h, 1.5 * s, palette::get().accent);
            }
            self.text(
                rt,
                brush,
                palette::MODE_LABELS[index],
                &f.label,
                bx + 12.0 * s,
                by + 5.0 * s,
                bw - 16.0 * s,
                bh - 8.0 * s,
                if active { palette::get().text } else { palette::get().text_dim },
            );
        }
    }

    /// 当前布局(读 theme.yaml 的 style/layout/type;缺省 horizontal_squeeze)。
    fn read_layout_mode(&self) -> String {
        let path = self.user_dir.join("theme").join("theme.yaml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return "horizontal_squeeze".to_owned();
        };
        let mut in_layout = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed == "layout:" {
                in_layout = true;
                continue;
            }
            if in_layout {
                let indent = line.len() - line.trim_start().len();
                if !trimmed.is_empty() && indent < 4 {
                    break;
                }
                if let Some((key, value)) = trimmed.split_once(':') {
                    if key.trim() == "type" {
                        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
                        if !value.is_empty() {
                            return value.to_owned();
                        }
                    }
                }
            }
        }
        "horizontal_squeeze".to_owned()
    }

    /// 写布局(行编辑:替换 layout 块里的 type 行,不动其他键;渲染器按 mtime 自动重载)。
    fn write_layout_mode(&self, value: &str) {
        let path = self.user_dir.join("theme").join("theme.yaml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        let mut in_layout = false;
        let mut replaced = false;
        for line in lines.iter_mut() {
            let trimmed = line.trim();
            if trimmed == "layout:" {
                in_layout = true;
                continue;
            }
            if in_layout {
                let indent = line.len() - line.trim_start().len();
                if !trimmed.is_empty() && indent < 4 {
                    break;
                }
                if let Some((key, _)) = trimmed.split_once(':') {
                    if key.trim() == "type" {
                        let indent_str = " ".repeat(line.len() - line.trim_start().len());
                        *line = format!("{indent_str}type: \"{value}\"");
                        replaced = true;
                        break;
                    }
                }
            }
        }
        if !replaced {
            lines.push("  layout:".to_string());
            lines.push(format!("    type: \"{}\"", value));
        }
        let _ = std::fs::write(&path, lines.join("\n"));
    }

    /// 布局三钮:材质行下一行左侧(0=横排 1=竖排 2=竹简)。
    fn layout_buttons(&self) -> Vec<(f32, f32, f32, f32)> {
        let s = self.scale();
        let (x, top, _, _) = self.content_area();
        let (w, h) = (76.0 * s, 30.0 * s);
        (0..3)
            .map(|index| (x + index as f32 * (w + 8.0 * s), top + 94.0 * s, w, h))
            .collect()
    }

    fn paint_layout_buttons(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        f: &Formats,
        s: f32,
    ) {
        let labels = ["横排", "竖排", "竹简"];
        let current = self.read_layout_mode();
        for (index, (bx, by, bw, bh)) in self.layout_buttons().into_iter().enumerate() {
            let is_current =
                current == ["horizontal_squeeze", "vertical_squeeze", "cells"][index];
            let hovered = matches!(self.hover, Hover::LayoutBtn(i) if i == index);
            let bg = if is_current {
                0x24000000 | (palette::get().accent & 0xFFFFFF)
            } else if hovered {
                palette::get().overlay_mid()
            } else {
                palette::get().overlay_weak()
            };
            draw_fill_round(rt, brush, bx, by, bw, bh, 8.0 * s, bg);
            if is_current {
                // 选中条贴满圆角切完后的左侧直边：起点 +r、长度 h-2r（r = 底色圆角）
                let r = 8.0 * s;
                let bar_h = bh - 2.0 * r;
                let bar_y = by + r;
                draw_fill_round(rt, brush, bx, bar_y, 3.0 * s, bar_h, 1.5 * s, palette::get().accent);
            }
            self.text(
                rt,
                brush,
                labels[index],
                &f.label,
                bx + 12.0 * s,
                by + 5.0 * s,
                bw - 16.0 * s,
                bh - 8.0 * s,
                if is_current { palette::get().text } else { palette::get().text_dim },
            );
        }
    }

    /// 21:4x 用户「隐藏玻璃和亚克力默认选项，只使用默认效果」：整排三档按钮
    /// 不再绘制，也不再有命中热区。返回空向量后 `Hover::Material` 永远不会产生。
    fn material_buttons(&self) -> Vec<(f32, f32, f32, f32)> {
        Vec::new()
    }

    /// 「外观」页顶部左侧：21:4x 起三档材质按钮全部隐藏，只留一行只读状态。
    /// 候选窗恒定用「默认（不透明）」档；cyime 液态玻璃光泽是纯绘制层，
    /// 不需要窗口透明、抓屏或 DWM backdrop，所以藏掉选项也不损失玻璃观感。
    fn paint_material_buttons(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        f: &Formats,
        s: f32,
    ) {
        let (x, top, _, _) = self.content_area();
        self.text(
            rt,
            brush,
            "材质：默认（不透明）· 液态玻璃光泽",
            &f.label,
            x,
            top + 56.0 * s,
            320.0 * s,
            30.0 * s,
            palette::get().text_dim,
        );
    }

    /// 把材质写进 `theme/material.txt`（渲染器下次画候选窗时读取）。
    ///
    /// 用户 2026-11-02：三档从"编译期三个 DLL"改成"一个 DLL 运行时切"，所以这里只管
    /// 写文件；渲染器把 material.txt 也算进配置时间戳，下一次按键就重载度量与配色。
    fn set_material(&mut self, _index: usize) {
        // 21:4x：只剩默认档，无论谁调用都强制写 2（opaque）。
        let index = weasel_settings::material::DEFAULT_INDEX;
        match weasel_settings::material::set(&self.user_dir, index) {
            // 成功不写状态栏（用户 2026-11-04 10:25：「这个提示删掉」）。
            Ok(_) => {
                self.candidate_material = index;
                self.material = match index {
                    0 => Material::Glass,
                    1 => Material::Acrylic,
                    _ => Material::Metal,
                };
                // DWM 亚克力:透明/亚克力档开(亚克力 80% 透=染色 alpha 0x33),默认档关。
                palette::sync_material(self.hwnd, index);
                self.status.clear();
            }
            Err(error) => self.status = format!("写材质失败：{error}"),
        }
        self.repaint();
    }

    /// 「保存并部署」按钮矩形（像素）；只有方案页有。
    fn apply_button(&self) -> Option<(f32, f32, f32, f32)> {
        if self.page != PAGE_SCHEMAS {
            return None;
        }
        let s = self.scale();
        let (x, _, width, _) = self.content_area();
        let (_, h) = self.client_size();
        let bw = 132.0 * s;
        let bh = 34.0 * s;
        Some((x + width - bw, h - 90.0 * s, bw, bh))
    }

    fn max_scroll(&self) -> f32 {
        let s = self.scale();
        if self.page == PAGE_HOME {
            // 主页:按 paint_home 上一帧实测的内容底边算滚动上限——
            // 布局怎么改都不会把底部内容关在可视区外(2026-10-31 用户报"不能翻页")。
            let content = self.home_content_h.get().max(HOME_CONTENT_DIP * s);
            return (content - self.list_viewport().1).max(0.0);
        }
        if self.page == PAGE_DOWNLOAD {
            // 「下载」页卡片更高，按自己的行高算。
            let content = self.repo_filtered().len() as f32 * (REPO_ROW_H + REPO_GAP) * s;
            return (content - self.list_viewport().1).max(0.0);
        }
        let content = self.list_len() as f32 * (ROW_H + ROW_GAP) * s;
        (content - self.list_viewport().1).max(0.0)
    }

    /// 配色行矩形（像素，已含滚动量）。
    fn row_rects(&self) -> Vec<(f32, f32, f32, f32)> {
        let s = self.scale();
        let (x, _, width, _) = self.content_area();
        let top = self.list_viewport().0 - self.scroll;
        (0..self.list_len())
            .map(|index| {
                (
                    x,
                    top + index as f32 * (ROW_H + ROW_GAP) * s,
                    width,
                    ROW_H * s,
                )
            })
            .collect()
    }

    fn hit(&self, x: f32, y: f32) -> Hover {
        let [min, max, close] = self.button_rects();
        for (slot, rect) in [
            (Hover::Minimize, min),
            (Hover::Maximize, max),
            (Hover::Close, close),
        ] {
            if x >= rect.0 && x <= rect.0 + rect.2 && y >= rect.1 && y <= rect.1 + rect.3 {
                return slot;
            }
        }
        let s = self.scale();
        if x >= 20.0 * s && x <= 80.0 * s && y >= 76.0 * s {
            let index = ((y - 76.0 * s) / (44.0 * s)) as usize;
            if index < PAGES.len() {
                return Hover::Page(index);
            }
        }
        if self.page == PAGE_THEMES {
            for (i, (bx, by, bw, bh)) in self.color_mode_buttons().into_iter().enumerate() {
                if x >= bx && x <= bx + bw && y >= by && y <= by + bh {
                    return Hover::ColorMode(i);
                }
            }
            for (i, (bx, by, bw, bh)) in self.layout_buttons().into_iter().enumerate() {
                if x >= bx && x <= bx + bw && y >= by && y <= by + bh {
                    return Hover::LayoutBtn(i);
                }
            }
            // 「外观」页顶部的三档材质按钮（先于列表命中，按钮在列表上方）。
            for (index, rect) in self.material_buttons().into_iter().enumerate() {
                if x >= rect.0 && x <= rect.0 + rect.2 && y >= rect.1 && y <= rect.1 + rect.3 {
                    return Hover::Material(index);
                }
            }
        }
        if self.page == PAGE_DICTS {
            for (i, rect) in self.dict_button_rects().into_iter().enumerate() {
                if x >= rect.0 && x <= rect.0 + rect.2 && y >= rect.1 && y <= rect.1 + rect.3 {
                    return Hover::DictBtn(i);
                }
            }
        }
        if self.page == PAGE_DOWNLOAD {
            let (rx, ry, rw, rh) = self.repo_refresh_rect();
            if x >= rx && x <= rx + rw && y >= ry && y <= ry + rh {
                return Hover::RepoRefresh;
            }
            for (i, rect) in self.repo_tab_rects().into_iter().enumerate() {
                if x >= rect.0 && x <= rect.0 + rect.2 && y >= rect.1 && y <= rect.1 + rect.3 {
                    return Hover::RepoTab(i);
                }
            }
            for (i, rect) in self.repo_btn_rects().into_iter().enumerate() {
                if x >= rect.0 && x <= rect.0 + rect.2 && y >= rect.1 && y <= rect.1 + rect.3 {
                    return Hover::RepoBtn(i);
                }
            }
        }
        // 全量对齐 Slint 后新增的控件（搜索框、字号、保存/官方按钮、行内辅助按钮）。
        if let Some(h) = extras::hit(self, x, y) {
            return h;
        }
        if self.page == PAGE_THEMES || self.page == PAGE_SETTINGS || self.page == PAGE_SCHEMAS {
            let (top, height) = self.list_viewport();
            if y >= top && y <= top + height {
                for (index, rect) in self.row_rects().iter().enumerate() {
                    if x >= rect.0 && x <= rect.0 + rect.2 && y >= rect.1 && y <= rect.1 + rect.3 {
                        return Hover::Row(index);
                    }
                }
            }
        }
        if let Some((bx, by, bw, bh)) = self.apply_button() {
            if x >= bx && x <= bx + bw && y >= by && y <= by + bh {
                return Hover::Apply;
            }
        }
        Hover::None
    }

    /// 重新读常规页字段（启动时一次；编辑控件落地后再做保存后的刷新）。
    fn refresh_settings(&mut self) {
        use weasel_settings::{catalog, config, form};
        let paths = match weasel_common::process::RuntimePaths::discover() {
            Ok(paths) => paths,
            Err(error) => {
                self.settings_note = format!("找不到运行目录：{error}");
                self.fields.clear();
                return;
            }
        };
        let document = match config::Document::load(&paths) {
            Ok(document) => document,
            Err(error) => {
                self.settings_note = format!("读配置失败：{error}");
                self.fields.clear();
                return;
            }
        };
        let catalog = match catalog::load(&paths, &document) {
            Ok(catalog) => catalog,
            Err(error) => {
                self.settings_note = format!("读配置描述失败：{error}");
                self.fields.clear();
                return;
            }
        };
        // 与 Slint 版「常规」页同一个口径：取第 0 个分区（`page_shows(3)` = `category == 0`）。
        let Some(section) = catalog.sections.first() else {
            self.settings_note = "没有可编辑的配置分区".to_string();
            self.fields.clear();
            return;
        };
        self.paths = Some(paths.clone());
        let fields_result = form::fields(section, &document);
        match fields_result {
            Ok(fields) => {
                self.document = Some(document);
                self.settings_note = format!(
                    "{} · {} 个字段 · 点击行循环/编辑,改后点下方「仅保存 / 保存并应用」{}",
                    section.title,
                    fields.len(),
                    if catalog.notices.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", catalog.notices.join("；"))
                    }
                );
                self.fields = fields.iter().map(FieldRow::from_field).collect();
            }
            Err(error) => {
                self.settings_note = format!("生成表单失败：{error}");
                self.fields.clear();
            }
        }
    }

    /// 常规页字段点击:选项循环到下一档(继承→选项1→选项2→…),即改即存。
    /// 文本/JSON 字段本轮不处理(要文本输入控件)。
    fn cycle_field(&mut self, index: usize) {
        extras::cycle_field(self, index);
    }

    #[allow(dead_code)]
    fn cycle_field_raw(&mut self, index: usize) {
        use weasel_settings::form;
        let Some(document) = &mut self.document else { return };
        let Some(mut row) = self.fields.get_mut(index) else { return };
        let field = &mut row.field;
        // 文本/JSON 字段走行内编辑(2026-11-01 第二增量)
        if field.kind == "string" || field.kind == "json" {
            self.editing = Some(index);
            self.edit_buf = if field.text.is_empty() {
                field.inherited_text.clone()
            } else {
                field.text.clone()
            };
            self.settings_note = format!("编辑 {} :回车保存 · Esc 取消", field.title);
            return;
        }
        if field.choices.len() < 2 {
            self.settings_note = format!("{}:该字段暂不支持点击修改", field.title);
            return;
        }
        let next = ((field.selected.max(0) as usize) + 1) % field.choices.len();
        let selected = next as i32;
        let text = field.choices.get(next).cloned().unwrap_or_default();
        field.selected = selected;
        field.text = text.clone();
        match field.value(selected, &text) {
            Ok(value) => {
                if let Err(error) = form::assign(&mut document.patch, &field.path, value) {
                    self.settings_note = format!("{}:写入失败 {error}", field.title);
                    return;
                }
            }
            Err(error) => {
                self.settings_note = format!("{}:取值失败 {error}", field.title);
                return;
            }
        }
        let saved_title = field.title.clone();
        drop(row);
        if let Err(error) = document.save() {
            self.settings_note = format!("保存失败:{error}");
            return;
        }
        // 宿主级配置热更新(2026-11-17):保存后通知 broker 重读 weasel.yaml。
        request_settings_reload();
        // 重读,行显示回到真实状态(继承/覆盖、裁剪规则统一)
        self.refresh_settings();
        self.settings_note = format!("已保存:{} = {}", saved_title, text);
    }

    /// 行内编辑:输入可打印字符(退格由 WM_KEYDOWN 单独处理)。
    fn edit_push(&mut self, ch: char) {
        if self.editing.is_some() {
            self.edit_buf.push(ch);
            self.repaint();
        }
    }

    /// 行内编辑:退格。
    fn edit_backspace(&mut self) {
        if self.editing.is_some() {
            self.edit_buf.pop();
            self.repaint();
        }
    }

    /// 行内编辑:回车保存到内存草稿(点「保存」才落盘),Esc 取消。
    fn edit_commit(&mut self) {
        extras::edit_commit(self);
    }

    #[allow(dead_code)]
    fn edit_commit_raw(&mut self) {
        use weasel_settings::form;
        let Some(index) = self.editing.take() else { return };
        let buf = std::mem::take(&mut self.edit_buf);
        let Some(document) = &mut self.document else { return };
        let Some(mut row) = self.fields.get_mut(index) else { return };
        let field = &mut row.field;
        field.selected = 1;
        field.text = buf.clone();
        match field.value(1, &buf) {
            Ok(value) => {
                if let Err(error) = form::assign(&mut document.patch, &field.path, value) {
                    self.settings_note = format!("{}:写入失败 {error}", field.title);
                    return;
                }
            }
            Err(error) => {
                self.settings_note = format!("{}:取值失败 {error}", field.title);
                return;
            }
        }
        let saved_title = field.title.clone();
        drop(row);
        if let Err(error) = document.save() {
            self.settings_note = format!("保存失败:{error}");
            return;
        }
        // 宿主级配置热更新(2026-11-17):保存后通知 broker 重读 weasel.yaml。
        request_settings_reload();
        self.refresh_settings();
        self.settings_note = format!("已保存:{} = {}", saved_title, buf);
    }

    /// 行内编辑:Esc 取消,不落盘。
    fn edit_cancel(&mut self) {
        self.editing = None;
        self.edit_buf.clear();
        self.settings_note = "已取消编辑".to_string();
        self.repaint();
    }

    /// 信号入口（2026-11-14）：点击「方案」等标签只发 `PageSelected`，
    /// **不直接改 `page`**。切页、滚动复位、进入页面后的数据加载全由这里统一分发，
    /// `--page=` 和外部 `WM_APP_PAGE_SELECTED` 也走同一条路。
    fn emit(&mut self, signal: Signal) {
        let (target, out) = signals::dispatch(signal, self.page, signals::PAGE_COUNT);
        let navigating = matches!(signal, Signal::PageSelected(_));
        if navigating {
            self.page = target;
            self.scroll = 0.0;
            self.hover = Hover::None;
            self.dict_edit = 0;
            // 离开常规页时收起下拉选项，切回来不会残留一个悬空浮层。
            self.option_popup = None;
            self.option_scroll = 0.0;
        }
        for next in out {
            self.handle_signal(next);
        }
        if navigating {
            self.repaint();
        }
    }

    /// 信号槽：页面模块在这里订阅自己关心的信号。
    /// 进入「方案」界面 ⇒ `ReloadSchemas` ⇒ 重读 `schema/schema.yaml`，
    /// 外部改过方案清单也不会看到旧列表。
    fn handle_signal(&mut self, signal: Signal) {
        match signal {
            Signal::PageEntered(page) => {
                log(&format!("[signal] 进入页面 {page} ({})", PAGES[page.min(PAGES.len() - 1)]));
            }
            Signal::PageReselected(page) => {
                log(&format!("[signal] 重选页面 {page} ({})", PAGES[page.min(PAGES.len() - 1)]));
            }
            Signal::ReloadStats => self.refresh_stats(),
            Signal::ReloadSchemas => self.refresh_schemas(),
            Signal::ReloadDicts => self.dict_refresh(),
            Signal::PageSelected(_) => {}
        }
    }

    /// 方案页可见行：`(行类型, 方案下标, 分组标题)`。
    /// 行类型见 `extras::KIND_*`：主方案 / 缩进子行 / 文件夹标题 / 平铺方案。
    fn schema_items(&self) -> Vec<(u8, usize, String)> {
        extras::schema_items(self)
    }

    /// 旧的「按文件夹平铺」视图，树形改造后只保留作兼容参考。
    #[allow(dead_code)]
    fn schema_items_base(&self) -> Vec<(bool, usize, String)> {
        let mut items = Vec::new();
        let mut index = 0;
        while index < self.schemas.len() {
            let series = self.schemas[index].series.clone().unwrap_or_default();
            let start = index;
            while index < self.schemas.len()
                && self.schemas[index].series.clone().unwrap_or_default() == series
            {
                index += 1;
            }
            let count = index - start;
            let label = if series.is_empty() { "其他（无文件夹）".to_string() } else { series.clone() };
            items.push((true, start, format!("{label}  ({count})")));
            if !self.collapsed.contains(&series) {
                for row in start..index {
                    items.push((false, row, String::new()));
                }
            }
        }
        items
    }

    /// 重新读方案列表（启动时、保存后各一次）。
    ///
    /// 首次运行要把出厂 `default.yaml` 的 `schema_list` 抄进用户 `schema/schema.yaml`
    /// （官方对话框也这样起手）—— 少了这步界面会显示"一个都没勾"。
    fn refresh_schemas(&mut self) {
        self.refresh_schemas_raw();
        extras::inject_input_types(self);
    }

    #[allow(dead_code)]
    fn refresh_schemas_raw(&mut self) {
        let _ = weasel_style::switcher::seed_schema_table(&self.shared_dir, &self.user_dir);
        match weasel_style::switcher::list_schemas(&self.shared_dir, &self.user_dir) {
            Ok(list) => {
                let mut list = list;
                // 按系列（= 方案文件夹）分组，组内按名字排；平铺的（series=None）排最后。
                list.sort_by(|a, b| {
                    let ka = (a.series.is_none(), a.series.clone().unwrap_or_default(), a.name.clone());
                    let kb = (b.series.is_none(), b.series.clone().unwrap_or_default(), b.name.clone());
                    ka.cmp(&kb)
                });
                self.schema_status = format!(
                    "{} 个方案 · 已选 {} 个 · 勾选只改内存，点右下「保存并部署」才落盘",
                    list.len(),
                    list.iter().filter(|row| row.selected).count()
                );
                self.schemas = list;
            }
            Err(error) => {
                self.schema_status = format!("读取方案列表失败：{error}");
                self.schemas.clear();
            }
        }
    }

    /// 把勾选写进 `schema/schema.yaml`，再请 broker 重新部署。
    /// 全量版：拼音输入类型覆盖、父组闸门、官方部署进度都在 `extras` 里做。
    fn save_schemas(&mut self) {
        extras::save_schemas(self);
    }

    #[allow(dead_code)]
    fn save_schemas_raw(&mut self) {
        let ids: Vec<String> = self
            .schemas
            .iter()
            .filter(|row| row.selected)
            .map(|row| row.id.clone())
            .collect();
        // 官方 OnOK：一项都没选就报错，不写文件。
        if ids.is_empty() {
            self.schema_status = "至少要选用一项。".to_string();
            return;
        }
        let path = self
            .user_dir
            .join("schema")
            .join(weasel_style::switcher::SCHEMA_TABLE_FILE);
        match weasel_style::switcher::write_schema_ids(&path, &ids) {
            Ok(()) => {
                self.schema_status =
                    format!("已保存 {} 个方案，正在请 broker 重新部署…", ids.len());
                request_deploy();
            }
            Err(error) => self.schema_status = format!("写入菜单失败：{error}"),
        }
    }

    /// 更新页只做「位置与版本」展示。
    ///
    /// **自动检查更新按用户指示（2026-10-31 12:5x）先不做** —— 而且本产品本来就没随包带
    /// `WinSparkle.dll`（安装目录、`installer/weasel-next.nsi`、`script/build-installer.ps1` 里都没有），
    /// Slint 版的开关一直是禁用态。所以这里不再画开关，只写一句"暂不做"。
    fn refresh_update(&mut self) {
        self.update_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_default();
    }

    /// 重新读统计数据（进主页时、启动时各刷一次）。
    // ── 「词库」页(2026-11-08 D2D 迁移,后端=weasel_settings::dict) ──

    /// 查候选:按编码(前缀)扫全部用户词库。
    fn dict_find(&mut self) {
        // 同「信号 + 热更新」:查询扫全部词典文件,放后台线程,结果热更新。
        let user = self.user_dir.clone();
        let code = self.dict_code.trim().to_string();
        self.dict_status = format!("正在查询 {code}…");
        let mail = self.dict_find_mail.clone();
        std::thread::spawn(move || {
            let mut rows: Vec<(i32, String, String, String)> = Vec::new();
            let dicts = weasel_settings::dict::list_dicts(&user).unwrap_or_default();
            if !code.is_empty() {
                for dict in &dicts {
                    let Ok(text) = std::fs::read_to_string(&dict.path) else { continue };
                    for (rank, entry) in weasel_settings::dict::find_entries(&text, &code, true) {
                        rows.push((rank as i32, entry.word.clone(), entry.code.clone(), dict.name.clone()));
                    }
                }
            }
            let total: usize = dicts.iter().map(|d| d.entries).sum();
            let _ = total;
            if let Ok(mut guard) = mail.lock() {
                *guard = Some(rows);
            }
        });
        self.repaint();
    }

    /// 加入词条:词用编码框里的编码(或词本身可拼)。
    fn dict_add(&mut self) {
        let word = self.dict_word.trim().to_string();
        let code = self.dict_code.trim().to_string();
        if word.is_empty() || code.is_empty() {
            self.dict_status = "编码和词都要填".into();
            self.repaint();
            return;
        }
        let user = self.user_dir.clone();
        match weasel_settings::dict::add_entry(&code, &word, &code) {
            Ok(_) => {
                let _ = weasel_settings::dict::backup_dict(&user, &user.join("dict").join("qiaoma").join(format!("qiaoma.{code}.dict.yaml")));
                self.dict_status = format!("已加入 {word}({code})");
                self.dict_find();
            }
            Err(e) => self.dict_status = format!("加入失败:{e}"),
        }
        self.repaint();
    }

    /// 删除词条:按词删。
    fn dict_remove(&mut self) {
        let word = self.dict_word.trim().to_string();
        if word.is_empty() {
            self.dict_status = "要删的词填在词框里".into();
            self.repaint();
            return;
        }
        let user = self.user_dir.clone();
        let (content, count) = weasel_settings::dict::remove_entry(&word, &word);
        let _ = content;
        self.dict_status = format!("删除 {word}:{count} 处");
        let _ = weasel_settings::dict::backup_dict(&user, &user.join("dict").join("qiaoma").join("qiaoma.dict.yaml"));
        self.dict_find();
        self.repaint();
    }

    fn dict_refresh(&mut self) {
        // 2026-11-16「信号 + 热更新」:扫盘放后台,UI 立即显示"正在扫描",
        // 结果经 dict_mail 由 WM_TIMER 采样热更新 —— 点击词库页零卡顿。
        self.dict_status = "正在扫描词典…".into();
        self.dict_rows.clear();
        let mail = self.dict_mail.clone();
        let user = self.user_dir.clone();
        std::thread::spawn(move || {
            let dicts = weasel_settings::dict::list_dicts(&user).unwrap_or_default();
            let total: usize = dicts.iter().map(|d| d.entries).sum();
            let status = format!("{} 个词典 · 共 {} 条", dicts.len(), total);
            if let Ok(mut guard) = mail.lock() {
                *guard = Some(status);
            }
        });
        self.repaint();
    }

    /// WM_TIMER 采样词库扫描结果(照 poll_repo_job)。
    fn poll_dict_job(&mut self) {
        let status = self
            .dict_mail
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(status) = status {
            self.dict_status = status;
            self.repaint();
        }
    }

    /// WM_TIMER 采样词库查询结果。
    fn poll_dict_find(&mut self) {
        let rows = self
            .dict_find_mail
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(rows) = rows {
            self.dict_rows = rows;
            self.dict_scroll = 0.0;
            self.repaint();
        }
    }

    fn refresh_stats(&mut self) {
        let (entries, warnings) = weasel_settings::stats::load();
        let snapshot =
            weasel_settings::stats::snapshot(&entries, weasel_settings::stats::today_local());
        self.stats_note = if !warnings.is_empty() {
            warnings.join("；")
        } else if snapshot.has_data {
            format!("{} 天记录 · {}", entries.len(), snapshot.quarter_label)
        } else {
            "还没有统计数据（stat.bin 为空）".to_string()
        };
        self.stats = Some(snapshot);
        self.hours = weasel_settings::stats::load_hours();
        // 时段统计:全零(stat.bin 无小时段)给一条演示曲线,照 Slint 版口径。
        let (hour_keys, hour_ms) = if self.hours.0.iter().all(|v| *v == 0) {
            let curve = [0u32, 0, 0, 0, 0, 0, 0, 12, 340, 620, 480, 390, 410, 300, 260, 330, 470, 610, 720, 810, 640, 380, 120, 30];
            let mut ms = [0u64; 24];
            for (index, value) in curve.iter().enumerate() {
                ms[index] = u64::from(*value) * 120;
            }
            (curve.to_vec(), ms.to_vec())
        } else {
            (self.hours.0.to_vec(), self.hours.1.to_vec())
        };
        self.hour_peak = hour_keys
            .iter()
            .enumerate()
            .max_by_key(|(_, v)| **v)
            .map(|(i, _)| i as i32)
            .unwrap_or(0);
        let hour_total_min = hour_ms.iter().sum::<u64>() / 60_000;
        self.hour_summary = format!("最活跃 {} 时 · 共 {} 分（按有效输入时长）", self.hour_peak, hour_total_min);
        let peak = hour_keys.iter().copied().max().unwrap_or(0).max(1) as f32;
        let mut pct: Vec<i32> = hour_keys
            .iter()
            .map(|value| ((*value as f32 / peak) * 100.0).round() as i32)
            .collect();
        // 前 7 天日总量柱接在 24 小时柱后(2026-11-07):越久越淡。
        self.week_bars = weasel_settings::stats::load_week_bars();
        for (p, alpha) in &self.week_bars {
            pct.push(*p);
        }
        self.hour_pct = pct;
        // 词云：交给后台线程重建（686 ms/次，放 UI 线程就是一次 0.7 秒冻屏）。
        self.request_cloud();
    }

    /// 请求后台重建词云（启动 / 进主页 / 重选主页各一次）。
    ///
    /// 建一次实测 686 ms，**绝不能跑在 UI 线程上**：首页每一次重绘都会走到
    /// `paint_home`，以前那里直接调 `build()`，等于每帧冻 0.7 秒。
    /// 现在 UI 只读 `cloud_cache`；线程把结果放进 `cloud_pending`，`WM_TIMER` 取回。
    fn request_cloud(&mut self) {
        if self.cloud_building {
            return; // 已有一个在跑，不必排队
        }
        self.cloud_building = true;
        let slot = std::sync::Arc::clone(&self.cloud_pending);
        std::thread::spawn(move || {
            let data = weasel_settings::wordcloud::build();
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(data);
            }
        });
    }

    /// `WM_TIMER`：把后台建好的词云取回缓存并重绘。
    fn poll_cloud(&mut self) {
        let ready = self
            .cloud_pending
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        if let Some(data) = ready {
            *self.cloud_cache.borrow_mut() = Some(data);
            self.cloud_building = false;
            self.repaint();
        }
    }

    /// 点选一套配色：写 `theme/theme.yaml` → 重读 → 刷新行与状态。
    fn select_scheme(&mut self, index: usize) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let id = row.id.clone();
        let Some(themes) = self.themes.as_mut() else {
            return;
        };
        match themes.select(&id) {
            Ok(message) => self.status = message,
            Err(error) => self.status = format!("切换失败：{error}"),
        }
        // 16 套一行一套:切换后重载行同样剥掉 .dark 重复项(与 run() 过滤一致)。
        self.rows = themes
            .rows()
            .into_iter()
            .filter(|row| !row.id.ends_with(".dark"))
            .collect();
        // 配色管线:设置界面整套颜色跟候选窗活动配色走,亚克力染色同步。
        palette::init(palette::load());
        palette::sync_material(self.hwnd, self.candidate_material);
        self.repaint();
    }

    fn paint(&self) {
        let Some(present) = &self.presenter else {
            return;
        };
        let Some(f) = &self.formats else {
            return;
        };
        let (w, h) = (present.size().0 as f32, present.size().1 as f32);
        let s = self.scale();
        let rt = present.render_target();
        let brush = present.brush();

        let mut fill_round = |x: f32, y: f32, ww: f32, hh: f32, r: f32, argb: u32| {
            draw_fill_round(&rt, brush, x, y, ww, hh, r, argb);
        };
        let mut fill_rect = |x: f32, y: f32, ww: f32, hh: f32, argb: u32| {
            draw_fill_rect(&rt, brush, x, y, ww, hh, argb);
        };

        present.begin_draw();
        present.clear_transparent();

        // 2026-11-16 用户「做抗锯齿，圆角全是锯齿」：
        // 之前只有面板底色是圆角填充，渐变/AC2/描边仍画到**方形**窗口角上，再由 DWM 在
        // 约 8px 处做圆角裁剪；面板半径是 14*scale（≈17.5px），两条不同半径的弧叠在
        // 四个窗口角 ⇒ 月牙状的双弧锯齿边。现在先把整帧内容统一裁进面板同半径的圆角
        // 图层（D2D 几何蒙版，抗锯齿），四角只剩一条 D2D 抗锯齿弧，DWM 那圈裁在透明
        // 像素上（不产生可见边）。
        let panel_radius = 14.0 * s;
        let mut panel_layer: Option<ID2D1Layer> = None;
        {
            let factory = present.d2d_factory();
            let rounded = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h },
                radiusX: panel_radius.max(0.0),
                radiusY: panel_radius.max(0.0),
            };
            if let Ok(mask) = unsafe { factory.CreateRoundedRectangleGeometry(&rounded) } {
                if let Ok(mask_geom) = windows_core::Interface::cast::<ID2D1Geometry>(&mask) {
                    if let Ok(layer) = unsafe { rt.CreateLayer(None) } {
                        let params = D2D1_LAYER_PARAMETERS {
                            contentBounds: D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h },
                            geometricMask: core::mem::ManuallyDrop::new(Some(mask_geom)),
                            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                            maskTransform: windows_numerics::Matrix3x2::identity(),
                            opacity: 1.0,
                            opacityBrush: core::mem::ManuallyDrop::new(None),
                            layerOptions: D2D1_LAYER_OPTIONS_NONE,
                        };
                        unsafe { rt.PushLayer(&params, &layer) };
                        panel_layer = Some(layer);
                    }
                }
            }
        }

        // 面板本体：半透明深色玻璃（DWM 亚克力的染色在 acrylic.rs 里设成同一个色相）。
        // 深浅自适应:深色盘压暗做导航轨,浅色盘直接用面板色(不再无条件 darken)。
        let base = if palette::get().is_dark {
            darken(palette::get().panel, 0.55)
        } else {
            palette::get().panel
        };
        // 2026-11-14 用户:透明/亚克力要**带皮肤的色相**——染色用 panel2(色纸色),
        // 不再是灰白;「默认」完全不透明。亚克力 = 80% 透明(20% 蒙版)。
        let panel = match self.material {
            Material::Glass => (0x33 << 24) | palette::get().panel, // 80% 透明
            Material::Metal => {
                (0xFF << 24)
                    | if palette::get().is_dark {
                        darken(palette::get().panel, 0.75)
                    } else {
                        palette::get().panel
                    }
            }
            Material::Acrylic => (0x80 << 24) | palette::get().panel, // 50% 透明
        };
        // 方形填充 + 外层圆角图层蒙版 ⇒ 圆角只被裁一次（抗锯齿一次，不会出现双弧）。
        fill_rect(0.0, 0.0, w, h, panel);
        // AC2(刺客信条 2)几何背景层(2026-11-14 用户):网格漂移 + 放射线缓转 +
        // 菱形徽记呼吸 + 晕影,取代此前的光斑 wash + 白光斜扫。几何全在面板圆角内。
        // 动画相位已固定(now_ms=0)，纯静态背景，只是保留昨天的视觉。
        paint_ac2(&rt, brush, present.d2d_factory(), w, h, 14.0 * s, palette::get().is_dark);
        // 11-15 23:0x「像雾气一样，颜色的渐变呢」：不透明档改铺 HTML 同款
        // 彩色多层渐变；透明档保留薄白斜光（那两档靠 DWM 合成，白光才像玻璃）。
        if self.material == Material::Metal {
            // 2026-11-17 用户「做四通道求导吧」:底色场 + 径向求导光球,替代三层线性。
            paint_color_field(&rt, w, h);
        } else {
            paint_sheen(&rt, w, h);
        }
        if self.material == Material::Metal {
            // 哑光金属：顶部一条高光 + 底部一条暗边 + 两道极淡横纹（拉丝感）
            fill_rect(0.0, 0.0, w, 2.0 * s, (0x66 << 24) | 0xFFFFFF);
            fill_rect(0.0, h - 2.0 * s, w, 2.0 * s, 0x66000000);
            for index in 0..2 {
                let y = h * (0.34 + index as f32 * 0.22);
                fill_rect(0.0, y, w, 1.0 * s, palette::get().overlay_weak());
            }
        }
        // 极细描边：玻璃边缘的亮线。
        // （用 1px 圆角矩形叠画的做法留给 v2 的 SDF；v1 先用内描边矩形近似。）
        fill_round(1.0 * s, 1.0 * s, w - 2.0 * s, h - 2.0 * s, 7.0 * s, palette::get().overlay_strong());

        // 标题栏。
        fill_rect(0.0, 0.0, w, 52.0 * s, 0x00000000);
        self.text(
            &rt,
            brush,
            "小狼毫Next 设置",
            &f.title,
            20.0 * s,
            14.0 * s,
            360.0 * s,
            24.0 * s,
            palette::get().text,
        );

        // 窗口按钮。
        let [min, max, close] = self.button_rects();
        let glyphs = [("–", min), ("□", max), ("✕", close)];
        for (i, (glyph, rect)) in glyphs.iter().enumerate() {
            let hovered = match (i, self.hover) {
                (0, Hover::Minimize) | (1, Hover::Maximize) | (2, Hover::Close) => true,
                _ => false,
            };
            if hovered {
                let bg = if i == 2 { 0xD9C42B1C } else { palette::get().overlay_weak() };
                fill_round(rect.0, rect.1, rect.2, rect.3, 7.0 * s, bg);
            }
            self.text(
                &rt,
                brush,
                glyph,
                &f.glyph,
                rect.0,
                rect.1 + 1.0 * s,
                rect.2,
                rect.3,
                palette::get().text,
            );
        }

        // 左侧页签。
        for (index, name) in PAGES.iter().enumerate() {
            let y = 76.0 * s + index as f32 * 40.0 * s;
            let selected = index == self.page;
            let hovered = matches!(self.hover, Hover::Page(i) if i == index);
            if selected {
                // 导航选中条做回圆角(2026-11-14 用户「做回圆角」——斜切仅候选窗弃用,
                // 设置窗同样回归 8px 圆角 pill);左侧竖 accent 标记保留。
                let r = 8.0 * s;
                let h = 34.0 * s;
                fill_round(20.0 * s, y, 60.0 * s, h, r, palette::get().overlay_strong());
                fill_round(20.0 * s, y + r, 3.0 * s, h - 2.0 * r, 1.5 * s, palette::get().accent);
            } else if hovered {
                fill_round(20.0 * s, y, 60.0 * s, 34.0 * s, 8.0 * s, palette::get().overlay_weak());
            }
            self.text(
                &rt,
                brush,
                name,
                &f.nav,
                30.0 * s,
                y + 7.0 * s,
                44.0 * s,
                18.0 * s,
                if selected { palette::get().text } else { palette::get().text_dim },
            );
        }

        // 右侧内容区：标题 + 副标题（外观页显示真实状态），再按页分派。
        let (content_x, content_top, content_w, content_h) = self.content_area();
        self.text(
            &rt,
            brush,
            PAGES[self.page],
            &f.heading,
            content_x,
            content_top,
            content_w,
            30.0 * s,
            palette::get().text,
        );
        let subtitle = match self.page {
            PAGE_THEMES => {
                let active = self
                    .rows
                    .iter()
                    .find(|row| row.active)
                    .map(|row| row.name.clone())
                    .unwrap_or_else(|| "—".to_string());
                format!(
                    "16 套皮肤 · 当前 {active} · 深浅=右上三钮 · 材质=默认（液态玻璃光泽）"
                )
            }
            PAGE_HOME => self.stats_note.clone(),
            PAGE_SETTINGS => self.settings_note.clone(),
            PAGE_SCHEMAS => self.schema_status.clone(),
            PAGE_DICTS => self.dict_status.clone(),
            PAGE_DOWNLOAD => self.repo_status.clone(),
            PAGE_UPDATE => "版本与位置（自动更新暂不做）".to_string(),
            _ => PAGE_HINTS[self.page].to_string(),
        };
        self.text(
            &rt,
            brush,
            &subtitle,
            &f.label,
            content_x,
            content_top + 32.0 * s,
            content_w,
            18.0 * s,
            if self.page == PAGE_THEMES { palette::get().text_dim } else { palette::get().text_dim },
        );

        if self.page == PAGE_THEMES {
            self.paint_themes(&rt, brush, present.d2d_factory(), f, content_x, content_w, s);
        } else if self.page == PAGE_HOME {
            // 主页可滚动:裁剪到列表视口,滚出去的内容不许盖标题带(检查点遗留的
            // "真裁剪 PushAxisAlignedClip" 在主页先落地)。
            let (vp_top, vp_h) = self.list_viewport();
            unsafe {
                rt.PushAxisAlignedClip(
                    &D2D_RECT_F {
                        left: content_x - 8.0 * s,
                        top: vp_top,
                        right: content_x + content_w + 8.0 * s,
                        bottom: vp_top + vp_h,
                    },
                    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                );
            }
            self.paint_home(&rt, brush, f, content_x, content_w, s);
            unsafe {
                rt.PopAxisAlignedClip();
            }
        } else if self.page == PAGE_SETTINGS {
            if self.extra.is_some() {
                extras::paint_extra(self, &rt, brush, f, content_x, content_w, s);
            } else {
                self.paint_settings(&rt, brush, f, content_x, content_w, s);
            }
        } else if self.page == PAGE_SCHEMAS {
            self.paint_schemas(&rt, brush, present.d2d_factory(), f, content_x, content_w, s);
        } else if self.page == PAGE_UPDATE {
            self.paint_update(&rt, brush, f, content_x, content_w, s);
        } else if self.page == PAGE_DICTS {
            self.paint_dicts(&rt, brush, f, s);
        } else if self.page == PAGE_DOWNLOAD {
            self.paint_download(&rt, brush, f, s);
        } else {
            self.paint_placeholder(&rt, brush, f, content_x, content_top + 62.0 * s, content_w, content_h, s);
        }


        // 新增控件与部署进度浮层画在页面内容之上。
        extras::paint(self, &rt, brush, f, content_x, content_w, s);
        // 整帧内容画完：撤掉面板圆角蒙版（见本函数开头）。
        if panel_layer.is_some() {
            unsafe { rt.PopLayer() };
        }
        if let Err(error) = present.end_draw() {
            log(&format!("呈现失败：{error}"));
        }
    }

    fn text(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        text: &str,
        format: &IDWriteTextFormat,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        argb: u32,
    ) {
        let utf16 = wide(text);
        unsafe {
            brush.SetColor(&color(argb));
            let rect = D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            };
            rt.DrawText(
                &utf16,
                format,
                &rect,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    /// 主页：**照 Slint 的顺序**（用户 21:0x：多出来的功能删掉，只按 Slint 来）。
    ///
    /// 顺序与 Slint `ui/stats.slint` 一致：
    /// 个人生涯（Lv/称号/境界/经验条/名著）→ 年度热力图 → 键盘热力图 → 指法分布 → 时段统计 → 词云 → 页脚。
    /// 我原来自己加的「9 张数字卡」「近 30 天柱状图」「码长分布」Slint 没有，已删除。
    /// 词云字号 → 文本格式(按 px 缓存;D2D 目标固定 96 DPI,px = DIP)。
    fn cloud_format(&self, px: i32) -> IDWriteTextFormat {
        if let Some(fmt) = self.cloud_formats.borrow().get(&px) {
            return fmt.clone();
        }
        let factory: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
            .expect("DWriteCreateFactory 失败");
        let family = self.font_family();
        let family: Vec<u16> = family.encode_utf16().chain(Some(0)).collect();
        let format = unsafe {
            factory
                .CreateTextFormat(
                    windows_core::PCWSTR::from_raw(family.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    px as f32,
                    w!("zh-CN"),
                )
                .expect("词云格式创建失败")
        };
        unsafe {
            let _ = format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        }
        self.cloud_formats.borrow_mut().insert(px, format.clone());
        format
    }

    fn paint_home(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        f: &Formats,
        x: f32,
        width: f32,
        s: f32,
    ) {
        // 可滚动主页:整页随 scroll 上移(2026-10-31 用户报"不能翻页")。
        // 2026-11-15 21:2x：每个分区一张带描边的卡片，卡片之间固定留 gap，
        // 页脚也进入内容流（不再钉窗口底），彻底杜绝「叠在一起」。
        let top = self.list_viewport().0 - self.scroll;
        let (window_w, _) = self.client_size();
        let gap = 14.0 * s;
        let pad = 14.0 * s;
        let Some(snap) = &self.stats else {
            self.text(rt, brush, "读不到统计数据", &f.body, x, top, width, 22.0 * s, palette::get().text_dim);
            return;
        };
        if !snap.has_data {
            self.text(rt, brush, "还没有统计数据：先打几个字，这里会自动出现", &f.body, x, top, width, 22.0 * s, palette::get().text_dim);
            return;
        }

        // ① 个人等级第一行(用户 2026-10-31):段位卡放最顶上,9 卡挪到下面。
        let (title, level, exp_cur, exp_span) = weasel_settings::career::career_level(snap.total_exp);
        let realm = weasel_settings::career::career_realm(snap.total_chars);
        let books = weasel_settings::career::book_level(snap.total_chars);
        let career_top = top;
        // 经验条下面给两行说明，长名著信息裁成单行，不再换行压到卡外。
        let career_h = 108.0 * s;
        draw_home_card(rt, brush, x, career_top, width, career_h, s);
        self.text(
            rt,
            brush,
            &format!("{title}  Lv.{level} / {}  ·  {realm}", weasel_settings::career::CAREER_MAX_LEVEL),
            &f.heading,
            x + 16.0 * s,
            career_top + 10.0 * s,
            width - 32.0 * s,
            30.0 * s,
            palette::get().text,
        );
        let bar_y = career_top + 46.0 * s;
        let bar_w = width - 32.0 * s;
        draw_fill_round(rt, brush, x + 16.0 * s, bar_y, bar_w, 8.0 * s, 4.0 * s, palette::get().overlay_strong());
        let ratio = (exp_cur as f32 / exp_span.max(1) as f32).clamp(0.0, 1.0);
        draw_fill_round(rt, brush, x + 16.0 * s, bar_y, (bar_w * ratio).max(8.0 * s), 8.0 * s, 4.0 * s, palette::get().accent);
        let exp_line = fit_text(
            &format!("{} / {} 经验 · 还差 {} 升下一级", exp_cur, exp_span, exp_span.saturating_sub(exp_cur)),
            width - 32.0 * s,
            7.0 * s,
        );
        let book_line = fit_text(&books, width - 32.0 * s, 6.0 * s);
        self.text(rt, brush, &exp_line, &f.note, x + 16.0 * s, bar_y + 12.0 * s, width - 32.0 * s, 18.0 * s, palette::get().text_dim);
        self.text(rt, brush, &book_line, &f.label, x + 16.0 * s, bar_y + 32.0 * s, width - 32.0 * s, 16.0 * s, palette::get().text_dim);

        // ② 个人生涯 = 一张组框，里面放标题 + 9 张数字卡
        let group_top = career_top + career_h + gap;
        let cgap = 10.0 * s;
        let cw = (width - 2.0 * pad - 2.0 * cgap) / 3.0;
        let ch = 74.0 * s;
        let group_h = 38.0 * s + 3.0 * ch + 2.0 * cgap + pad;
        draw_home_card(rt, brush, x, group_top, width, group_h, s);
        self.text(rt, brush, "个人生涯", &f.heading, x + pad, group_top + 8.0 * s, 220.0 * s, 28.0 * s, palette::get().text);
        self.text(rt, brush, "本机记录，不上传", &f.label, x + 110.0 * s, group_top + 14.0 * s, 260.0 * s, 16.0 * s, palette::get().text_dim);
        let today_min = snap.today_active_ms / 60_000;
        let total_min = snap.total_active_ms / 60_000;
        let recent_avg = snap.recent_chars / u64::from(snap.recent_days.max(1));
        let cards: [(&str, String, String); 9] = [
            ("今日击键", thou(snap.today_keys), format!("累计 {}", thou(snap.total_keys))),
            ("今日上屏", format!("{} 字", thou(snap.today_chars)), format!("累计 {} 字", thou(snap.total_chars))),
            ("今日速度", format!("{} 字/分", snap.today_speed), format!("近 7 天 {}", snap.recent_speed)),
            ("连续使用", format!("{} 天", snap.streak_days), format!("最长 {} 天", snap.best_streak_days)),
            ("今日键准", format!("{}.{}%", snap.today_key_accuracy_x10 / 10, snap.today_key_accuracy_x10 % 10),
                format!("近 7 天 {}.{}%", snap.recent_key_accuracy_x10 / 10, snap.recent_key_accuracy_x10 % 10)),
            ("击键速度", key_speed_text(snap.today_key_speed_x10), format!("历史最快 {}", key_speed_text(snap.best_key_speed_x10))),
            ("平均码长", format!("{} 键/字", two_decimals(snap.today_code_len_x100)), format!("近 7 天 {}", two_decimals(snap.recent_code_len_x100))),
            ("打字时间", format!("{} 分", today_min), format!("累计 {} 分", total_min)),
            ("平均每日", format!("{} 字", snap.avg_daily_chars), format!("近 7 天 {} 字", recent_avg)),
        ];
        for (index, (label, value, note)) in cards.iter().enumerate() {
            let cx = x + pad + (index % 3) as f32 * (cw + cgap);
            let cy = group_top + 38.0 * s + (index / 3) as f32 * (ch + cgap);
            self.stat_card(rt, brush, f, cx, cy, cw, ch, label, value, note, s);
        }

        // ③ 年度热力图（独立卡片；四档色阶由当前 accent 派生，跟着配色走）
        let heat_top = group_top + group_h + gap;
        let ygap = 4.0 * s;
        let inner_w = width - 2.0 * pad;
        let ycell = ((inner_w - 52.0 * ygap) / 53.0).floor().max(6.0 * s);
        let heat_h = 28.0 * s + 7.0 * (ycell + ygap) + 34.0 * s;
        draw_home_card(rt, brush, x, heat_top, width, heat_h, s);
        self.text(rt, brush, &format!("{} 年", snap.quarter_label), &f.heading, x + pad, heat_top + 8.0 * s, 240.0 * s, 22.0 * s, palette::get().text);
        let tiers = heat_tiers();
        let tier_color = |count: u32| -> u32 {
            if count == 0 { tiers[0] }
            else if count < 100 { tiers[1] }
            else if count < 500 { tiers[2] }
            else if count < 1500 { tiers[3] }
            else { tiers[4] }
        };
        for (index, day) in snap.days.iter().enumerate().take(371) {
            let col = index / 7;
            let row = index % 7;
            let cx = x + pad + col as f32 * (ycell + ygap);
            let cy = heat_top + 28.0 * s + row as f32 * (ycell + ygap);
            draw_fill_round(rt, brush, cx, cy, ycell, ycell, 3.0 * s, tier_color(day.count));
            // 今天：外圈一圈亮色描边 + 一格高亮（用户 09:5x：看不出哪格是今天）
            if day.today {
                // 描边：外圈亮色 + 内圈退回格子颜色（没有 stroke API，用两圈填色模拟）
                draw_fill_round(rt, brush, cx - 1.5 * s, cy - 1.5 * s, ycell + 3.0 * s, ycell + 3.0 * s, 4.0 * s, palette::get().text);
                draw_fill_round(rt, brush, cx, cy, ycell, ycell, 3.0 * s, tier_color(day.count));
            }
        }
        // 图例:少 □□□□ 多 (<100 / <500 / <1500 / ≥1500 键)
        let legend_y = heat_top + 28.0 * s + 7.0 * (ycell + ygap) + 4.0 * s;
        let mut lx = x + pad;
        self.text(rt, brush, "少", &f.note, lx, legend_y, 20.0 * s, 14.0 * s, palette::get().text_dim);
        lx += 22.0 * s;
        for shade in tiers {
            draw_fill_round(rt, brush, lx, legend_y, 10.0 * s, 10.0 * s, 2.0 * s, shade);
            lx += 14.0 * s;
        }
        let legend_w = (x + width - pad - lx - 4.0 * s).max(20.0 * s);
        let legend_text = fit_text("多 (<100 / <500 / <1500 / ≥1500 键)", legend_w, 7.0 * s);
        self.text(rt, brush, &legend_text, &f.note, lx + 4.0 * s, legend_y, legend_w, 14.0 * s, palette::get().text_dim);

        // ④ 键盘热力图(Slint 同款完整键帽:五排、键名、四档色阶；独立卡片)
        let key_top = heat_top + heat_h + gap;
        type KeyRow = &'static [(u32, &'static str, f32)];
        let key_rows: [KeyRow; 5] = [
            &[(0xC0, "`", 1.0), (0x31, "1", 1.0), (0x32, "2", 1.0), (0x33, "3", 1.0), (0x34, "4", 1.0), (0x35, "5", 1.0), (0x36, "6", 1.0), (0x37, "7", 1.0), (0x38, "8", 1.0), (0x39, "9", 1.0), (0x30, "0", 1.0), (0xBD, "-", 1.0), (0xBB, "=", 1.0), (0x08, "退格", 2.0)],
            &[(0x09, "Tab", 1.5), (0x51, "Q", 1.0), (0x57, "W", 1.0), (0x45, "E", 1.0), (0x52, "R", 1.0), (0x54, "T", 1.0), (0x59, "Y", 1.0), (0x55, "U", 1.0), (0x49, "I", 1.0), (0x4F, "O", 1.0), (0x50, "P", 1.0), (0xDB, "[", 1.0), (0xDD, "]", 1.0), (0xDC, "\\", 1.5)],
            &[(0x14, "Caps", 1.8), (0x41, "A", 1.0), (0x53, "S", 1.0), (0x44, "D", 1.0), (0x46, "F", 1.0), (0x47, "G", 1.0), (0x48, "H", 1.0), (0x4A, "J", 1.0), (0x4B, "K", 1.0), (0x4C, "L", 1.0), (0xBA, ";", 1.0), (0xDE, "'", 1.0), (0x0D, "回车", 2.2)],
            &[(0x10, "Shift", 2.3), (0x5A, "Z", 1.0), (0x58, "X", 1.0), (0x43, "C", 1.0), (0x56, "V", 1.0), (0x42, "B", 1.0), (0x4E, "N", 1.0), (0x4D, "M", 1.0), (0xBC, ",", 1.0), (0xBE, ".", 1.0), (0xBF, "/", 1.0), (0xA1, "Shift", 2.3)],
            &[(0x11, "Ctrl", 1.5), (0x5B, "Win", 1.25), (0x12, "Alt", 1.25), (0x20, "空格", 6.0), (0xA4, "Alt", 1.25), (0x5C, "Win", 1.25), (0xA3, "Ctrl", 1.5)],
        ];
        let (key_counts, _cl) = weasel_settings::stats::load_hist();
        let key_peak = key_counts.iter().copied().max().unwrap_or(0).max(1) as f64;
        let key_tiers = heat_tiers();
        let key_color = |vk: u32| -> u32 {
            let count = key_counts.get(vk as usize).copied().unwrap_or(0) as f64;
            if count == 0.0 { key_tiers[0] }
            else {
                let ratio = count / key_peak;
                if ratio > 0.55 { key_tiers[4] }
                else if ratio > 0.3 { key_tiers[3] }
                else if ratio > 0.12 { key_tiers[2] }
                else { key_tiers[1] }
            }
        };
        let key_ink = |vk: u32| -> u32 {
            let count = key_counts.get(vk as usize).copied().unwrap_or(0) as f64;
            let ratio = count / key_peak;
            if count == 0.0 || ratio <= 0.55 { palette::get().text } else { palette::get().text_dim }
        };
        let kh = 34.0 * s;
        let kgap = 4.0 * s;
        // 卡高 = 标题 30 + 5 行键 + 指法两行
        let key_h = 30.0 * s + 5.0 * (kh + kgap) + 56.0 * s;
        draw_home_card(rt, brush, x, key_top, width, key_h, s);
        self.text(rt, brush, "键盘热力图", &f.heading, x + pad, key_top + 8.0 * s, 240.0 * s, 22.0 * s, palette::get().text);
        let area_x = x + pad;
        let area_w = width - 2.0 * pad;
        let mut ky = key_top + 30.0 * s;
        for row in key_rows.iter() {
            let units: f32 = row.iter().map(|(_, _, u)| u).sum();
            let unit_w = (area_w - (row.len() as f32 - 1.0) * kgap) / units.max(1.0);
            let mut kx = area_x;
            for (vk, label, unit) in row.iter() {
                let kw = unit * unit_w;
                draw_fill_round(rt, brush, kx, ky, kw, kh, 4.0 * s, key_color(*vk));
                self.text(rt, brush, label, &f.center, kx, ky + (kh - 16.0 * s) / 2.0, kw, 16.0 * s, key_ink(*vk));
                kx += kw + kgap;
            }
            ky += kh + kgap;
        }
        let (hands, rows_f, names) = weasel_settings::stats::finger_breakdown(&key_counts);
        // 两行说明都裁成单行，小窗口下不再自动换行叠到下一行/卡片外。
        let hand_line = fit_text(&format!("指法分布 · 左右手 {hands} · 三行 {rows_f}"), area_w, 7.0 * s);
        let finger_line = fit_text(&names, area_w, 6.0 * s);
        self.text(rt, brush, &hand_line, &f.note, area_x, ky + 2.0 * s, area_w, 16.0 * s, palette::get().text_dim);
        self.text(rt, brush, &finger_line, &f.label, area_x, ky + 22.0 * s, area_w, 16.0 * s, palette::get().text_dim);

        // ⑤ 时段统计（独立卡片；标题不再压在键盘卡底）:24 小时柱 + 前 7 天日总量柱。
        let hour_top = key_top + key_h + gap;
        let hour_h = 150.0 * s;
        draw_home_card(rt, brush, x, hour_top, width, hour_h, s);
        self.text(rt, brush, "时段统计", &f.heading, x + pad, hour_top + 12.0 * s, 220.0 * s, 26.0 * s, palette::get().text);
        self.text(
            rt,
            brush,
            &self.hour_summary,
            &f.label,
            x + 240.0 * s,
            hour_top + 18.0 * s,
            (width - 240.0 * s - pad).max(40.0),
            18.0 * s,
            palette::get().text_dim,
        );
        let bars = &self.hour_pct;
        let bar_n = bars.len() as f32;
        if bar_n > 0.0 {
            let bar_gap = 3.0 * s;
            let bar_w = ((width - 2.0 * pad - bar_gap * bar_n) / bar_n).max(2.0);
            let bar_max_h = 64.0 * s;
            let base_y = hour_top + 46.0 * s;
            for (index, pct) in bars.iter().enumerate() {
                let bh = (*pct as f32 / 100.0).max(0.02) * bar_max_h;
                let bx = x + pad + index as f32 * (bar_w + bar_gap);
                let is_week = index >= 24;
                let color = if index == self.hour_peak as usize && !is_week {
                    palette::get().accent
                } else if is_week {
                    // 前 7 天柱:压暗一档区分时段/周柱
                    darken(palette::get().text_dim, 0.8)
                } else {
                    palette::get().text_dim
                };
                draw_fill_round(rt, brush, bx, base_y + (bar_max_h - bh), bar_w, bh, 2.0 * s, color);
            }
            // 小时标签 0/6/12/18/23：固定宽度框居中，避免两位数被柱宽裁成单个数字。
            let label_w = 26.0 * s;
            for hour in [0usize, 6, 12, 18, 23] {
                let center = x + pad + hour as f32 * (bar_w + bar_gap) + bar_w / 2.0;
                self.text(
                    rt,
                    brush,
                    &hour.to_string(),
                    &f.center,
                    center - label_w / 2.0,
                    base_y + bar_max_h + 4.0 * s,
                    label_w,
                    14.0 * s,
                    palette::get().text_dim,
                );
            }
        }
        // ⑥ 我的词云（独立卡片；常用字区块已按用户要求移除 2026-11-15）。
        // 画布 1040×400 按内容宽等比缩放，词语不会再溢出卡片或压到页脚。
        // 2026-11-16：词云由后台线程建（实测 686 ms/次），这里**只读缓存、绝不构建**；
        // 还没建好就画空卡片 + 一行提示——界面绝不为它等。
        let cloud_ref = self.cloud_cache.borrow();
        let (cloud_items, cloud_note): (&[weasel_settings::wordcloud::CloudItem], String) =
            match cloud_ref.as_ref() {
                Some((items, summary)) => (items.as_slice(), summary.note()),
                None => (&[], "词云统计中…".to_string()),
            };
        let cloud_top = hour_top + hour_h + gap;
        let canvas_w = width - 2.0 * pad;
        let k = canvas_w / 1040.0;
        let canvas_h = 400.0 * k;
        let cloud_h = 44.0 * s + canvas_h + pad;
        draw_home_card(rt, brush, x, cloud_top, width, cloud_h, s);
        self.text(
            rt,
            brush,
            "我的词云",
            &f.heading,
            x + pad,
            cloud_top + 12.0 * s,
            200.0 * s,
            26.0 * s,
            palette::get().text,
        );
        let note_w = (width - 220.0 * s - pad).max(40.0);
        let note_text = fit_text(&cloud_note, note_w, 6.5 * s);
        self.text(
            rt,
            brush,
            &note_text,
            &f.label,
            x + 220.0 * s,
            cloud_top + 20.0 * s,
            note_w,
            18.0 * s,
            palette::get().text_dim,
        );
        let tints = [
            palette::get().accent,
            palette::get().text,
            palette::get().text_dim,
            crate::lighten(palette::get().text_dim, 0.25),
            crate::lighten(palette::get().text_dim, 0.45),
        ];
        for item in cloud_items {
            let size = (item.size as f32 * k).max(9.0).round() as i32;
            let cx = x + pad + item.x as f32 * k;
            let cy = cloud_top + 44.0 * s + item.y as f32 * k;
            let color = tints[(item.tint.max(0) as usize).min(4)];
            let fmt = self.cloud_format(size);
            self.text(rt, brush, &item.word, &fmt, cx, cy, item.w as f32 * k + 30.0, size as f32 + 8.0, color);
        }

        // 页脚也进入内容流（不再钉窗口底，避免与词云卡片叠在一起）。
        let footer_y = cloud_top + cloud_h + gap;
        self.text(
            rt,
            brush,
            "灵感来源：🐐🪶❄🌸 羊毛雪花 · input_statistic 统计插件",
            &f.center,
            0.0,
            footer_y,
            window_w,
            16.0 * s,
            palette::get().text_dim,
        );
        // 实测内容底边回写(滚动上限的数据源;含滚动偏移,转回内容坐标)。
        self.home_content_h.set(footer_y + 24.0 * s + self.scroll);
    }


    /// 数字卡片：小标签 + 大数字 + 灰色注脚。
    #[allow(clippy::too_many_arguments)]
    fn stat_card(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        f: &Formats,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        label: &str,
        value: &str,
        note: &str,
        s: f32,
    ) {
        // 卡片底色略提（面板已经 24% 透明，卡片太透字就糊了）
        draw_fill_round(rt, brush, x, y, w, h, 8.0 * s, palette::get().overlay_heavy());
        self.text(rt, brush, label, &f.label, x + 16.0 * s, y + 10.0 * s, w - 32.0 * s, 16.0 * s, palette::get().text_dim);
        // 大数字（白） + **灰色小字在它右边同一行**（用户 15:0x 明确要求，别放下面）。
        // 窄窗口下注脚按可用宽度裁成单行，否则换行会溢出小卡片（21:2x 修）。
        self.text(rt, brush, value, &f.heading, x + 16.0 * s, y + 30.0 * s, w - 32.0 * s, 30.0 * s, palette::get().text);
        let value_units: f32 = value
            .chars()
            .map(|c| if (c as u32) >= 0x2E80 { 2.0 } else { 1.0 })
            .sum();
        // 估算偏小会把注脚压在大数字上；这里取偏大的字宽并留 12px 间隙。
        let value_width = (value_units * 11.0 * s).min(w - 46.0 * s);
        let note_x = x + 16.0 * s + value_width + 12.0 * s;
        let note_w = (x + w - 12.0 * s - note_x).max(20.0 * s);
        let short = fit_text(note, note_w, 9.0 * s);
        self.text(
            rt,
            brush,
            &short,
            &f.note,
            note_x,
            y + 38.0 * s,
            note_w,
            18.0 * s,
            palette::get().text_dim,
        );
    }

    /// 「外观」页：真实配色列表（滚轮滚动 + 悬停 + 点选即生效）。
    fn paint_themes(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        factory: &ID2D1Factory1,
        f: &Formats,
        x: f32,
        width: f32,
        s: f32,
    ) {
        let (top, viewport_h) = self.list_viewport();
        // 材质按钮先画（列表已为它们让出顶部这一条：见 `list_viewport`）。
        self.paint_material_buttons(rt, brush, f, s);
        self.paint_color_mode_buttons(rt, brush, f, s);
        self.paint_layout_buttons(rt, brush, f, s);
        if self.rows.is_empty() {
            self.text(
                rt,
                brush,
                "没有读到配色（原因见上方状态行）",
                &f.body,
                x,
                top + 8.0 * s,
                width,
                22.0 * s,
                palette::get().text_dim,
            );
            return;
        }
        for (index, (rx, ry, rw, rh)) in self.row_rects().into_iter().enumerate() {
            // 起点在视口上方（滚动出界）或完全在下方的一律不画——
            // 否则会压住页标题/副标题（用户 16:0x 报的重叠）。
            if ry < top || ry > top + viewport_h - 4.0 * s {
                continue;
            }
            let row = &self.rows[index];
            let hovered = matches!(self.hover, Hover::Row(i) if i == index);
            let bg = if row.active {
                0x24000000 | (palette::get().accent & 0xFFFFFF)
            } else if hovered {
                palette::get().overlay_mid()
            } else {
                palette::get().overlay_weak()
            };
            let r = 10.0 * s;
            // 当前配色行做回圆角(2026-11-14 用户);左侧竖 accent 标记照旧。
            draw_fill_round(rt, brush, rx, ry, rw, rh, r, bg);
            if row.active {
                draw_fill_round(rt, brush, rx, ry + r, 3.0 * s, rh - 2.0 * r, 1.5 * s, palette::get().accent);
            }
            // 色块 = 迷你候选窗(按模式取色):
            //   深色钮 → .dark 色块;浅色钮 → .light 色块;
            //   自动 → 上半浅色/下半深色一刀两断,所见即所得。
            let sw = 40.0 * s;
            let sx = rx + 14.0 * s;
            let sy = ry + (rh - sw) / 2.0;
            let base = row
                .id
                .trim_end_matches(".light")
                .trim_end_matches(".dark")
                .to_string();
            let variant = |suffix: &str| {
                self.palettes.get(&format!("{base}{suffix}")).map(|p| {
                    (p.panel, p.sel_bg, p.text, p.text_dim)
                })
            };
            let light_c = variant(".light");
            let dark_c = variant(".dark");
            let fallback = (row.back, row.hilited_back, row.text, row.comment);
            match self.color_mode {
                1 => {
                    let (b, sel, t, c) = light_c.unwrap_or(fallback);
                    draw_fill_round(rt, brush, sx, sy, sw, sw, 8.0 * s, b);
                    draw_fill_round(rt, brush, sx + 6.0 * s, sy + sw * 0.58, sw - 12.0 * s, sw * 0.24, 2.0 * s, sel);
                    draw_fill_round(rt, brush, sx + 6.0 * s, sy + sw * 0.22, (sw - 12.0 * s) * 0.55, 3.0 * s, 1.5 * s, t);
                    draw_fill_round(rt, brush, sx + 6.0 * s, sy + sw * 0.38, (sw - 12.0 * s) * 0.35, 3.0 * s, 1.5 * s, c);
                }
                0 => {
                    let (b, sel, t, c) = dark_c.unwrap_or(fallback);
                    draw_fill_round(rt, brush, sx, sy, sw, sw, 8.0 * s, b);
                    draw_fill_round(rt, brush, sx + 6.0 * s, sy + sw * 0.58, sw - 12.0 * s, sw * 0.24, 2.0 * s, sel);
                    draw_fill_round(rt, brush, sx + 6.0 * s, sy + sw * 0.22, (sw - 12.0 * s) * 0.55, 3.0 * s, 1.5 * s, t);
                    draw_fill_round(rt, brush, sx + 6.0 * s, sy + sw * 0.38, (sw - 12.0 * s) * 0.35, 3.0 * s, 1.5 * s, c);
                }
                _ => {
                    // 自动:上半浅色块,下半深色块——中间一刀,所见即所得
                    if let Some((lb, lsel, lt, lc)) = light_c {
                        if let Some((db, dsel, dt, dc)) = dark_c {
                            let half = sw * 0.5;
                            draw_fill_round(rt, brush, sx, sy, sw, half, 8.0 * s, lb);
                            draw_fill_round(
                                rt,
                                brush,
                                sx,
                                sy + half,
                                sw,
                                half,
                                8.0 * s,
                                db,
                            );
                            draw_fill_round(rt, brush, sx + 6.0 * s, sy + sw * 0.30, sw - 12.0 * s, sw * 0.20, 2.0 * s, lsel);
                            draw_fill_round(rt, brush, sx + 6.0 * s, sy + sw * 0.60, sw - 12.0 * s, sw * 0.20, 2.0 * s, dsel);
                            let _ = (lt, lc, dt, dc);
                        }
                    }
                }
            }
            let text_x = sx + sw + 14.0 * s;
            let text_x = sx + sw + 14.0 * s;
            self.text(
                rt,
                brush,
                &row.name,
                &f.body,
                text_x,
                ry + 10.0 * s,
                rw - 200.0 * s,
                20.0 * s,
                palette::get().text,
            );
            let sub = if row.author.is_empty() {
                row.id.clone()
            } else {
                format!("{} · {}", row.id, row.author)
            };
            self.text(
                rt,
                brush,
                &sub,
                &f.label,
                text_x,
                ry + 34.0 * s,
                rw - 200.0 * s,
                18.0 * s,
                palette::get().text_dim,
            );
            if row.active {
                let (bw, bh) = (56.0 * s, 24.0 * s);
                let bx = rx + rw - bw - 14.0 * s;
                let by = ry + (rh - bh) / 2.0;
                draw_fill_round(
                    rt,
                    brush,
                    bx,
                    by,
                    bw,
                    bh,
                    bh / 2.0,
                    0xFF00_0000 | (row.hilited_back & 0x00FF_FFFF),
                );
                self.text(
                    rt,
                    brush,
                    "当前",
                    &f.glyph,
                    bx,
                    by + 4.0 * s,
                    bw,
                    16.0 * s,
                    row.hilited_text,
                );
            } else if row.modified {
                self.text(
                    rt,
                    brush,
                    "已改",
                    &f.label,
                    rx + rw - 60.0 * s,
                    ry + 22.0 * s,
                    46.0 * s,
                    18.0 * s,
                    palette::get().text_dim,
                );
            }
        }
        // 滚动条（只有装不下时才画）。
        self.paint_scrollbar(rt, brush, x, width, s);
    }

    /// 列表滚动条（配色页与常规页共用）。
    fn paint_scrollbar(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        x: f32,
        width: f32,
        s: f32,
    ) {
        let (top, viewport_h) = self.list_viewport();
        let max = self.max_scroll();
        if max <= 0.5 {
            return;
        }
        let track_x = x + width + 6.0 * s;
        draw_fill_round(rt, brush, track_x, top, 4.0 * s, viewport_h, 2.0 * s, palette::get().overlay_mid());
        let thumb_h = (viewport_h * viewport_h / (viewport_h + max)).max(24.0 * s);
        let thumb_y = top + (viewport_h - thumb_h) * (self.scroll / max);
        draw_fill_round(rt, brush, track_x, thumb_y, 4.0 * s, thumb_h, 2.0 * s, palette::get().overlay_heavy());
    }

    /// 方案页：可勾选的方案列表 + 右下「保存并部署」。
    fn paint_schemas(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        factory: &ID2D1Factory1,
        f: &Formats,
        x: f32,
        width: f32,
        s: f32,
    ) {
        let (top, viewport_h) = self.list_viewport();
        if self.schemas.is_empty() {
            self.text(
                rt,
                brush,
                "没有读到任何方案（原因见上方状态行）",
                &f.body,
                x,
                top + 8.0 * s,
                width,
                22.0 * s,
                palette::get().text_dim,
            );
        }
        let items = self.schema_items();
        for (index, (rx, ry, rw, rh)) in self.row_rects().into_iter().enumerate() {
            if ry + rh < top || ry > top + viewport_h {
                continue;
            }
            let Some((kind, row_index, label)) = items.get(index).cloned() else { continue };
            let hovered = matches!(self.hover, Hover::Row(i) if i == index)
                || matches!(self.hover, Hover::SchemaChevron(i) if i == index);
            if kind == extras::KIND_GROUP {
                let series = self.schemas[row_index].series.clone().unwrap_or_default();
                let collapsed = self.collapsed.contains(&series);
                draw_fill_round(rt, brush, rx, ry, rw, rh, 8.0 * s,
                    if hovered { palette::get().overlay_mid() } else { palette::get().overlay_weak() });
                // 大号矢量三角（18px）独立占一列，不再跟标题文字混排成小字符。
                let arrow = 18.0 * s;
                let ax = rx + 16.0 * s;
                let ay = ry + (rh - arrow) / 2.0;
                draw_fill_triangle(rt, brush, factory, ax, ay, arrow, !collapsed,
                    palette::get().text);
                let text_x = ax + arrow + 10.0 * s;
                self.text(rt, brush, &label, &f.body,
                    text_x, ry + (rh - 22.0 * s) / 2.0,
                    rw - (text_x - rx) - 16.0 * s, 22.0 * s, palette::get().text);
                continue;
            }
            let row = &self.schemas[row_index];
            let is_parent = kind == extras::KIND_PARENT;
            let is_child = kind == extras::KIND_CHILD;
            // 拼音主方案未勾选 ⇒ 输入类型子行「父未选压住」灰掉（记忆保留、可恢复）。
            let muted = is_child && !extras::pinyin_selected(self);
            draw_fill_round(
                rt,
                brush,
                rx,
                ry,
                rw,
                rh,
                10.0 * s,
                if hovered { palette::get().overlay_mid() } else { palette::get().overlay_weak() },
            );
            // 主方案行：左侧矢量三角（展开向下 / 折叠向右），点它只折叠、不改勾选。
            let mut bx = rx + 16.0 * s;
            if is_parent {
                let arrow = 16.0 * s;
                let ax = rx + 14.0 * s;
                let ay = ry + (rh - arrow) / 2.0;
                draw_fill_triangle(rt, brush, factory, ax, ay, arrow,
                    !self.collapsed.contains(extras::PINYIN_CHEVRON), palette::get().text);
                bx = ax + arrow + 10.0 * s;
            } else if is_child {
                // 子行缩进一层，左侧画一条竖连接线表示层级；父未选时连它也一起变灰。
                draw_fill_round(rt, brush, rx + 30.0 * s, ry + 4.0 * s,
                    2.0 * s, rh - 8.0 * s, 1.0 * s,
                    if muted { palette::get().overlay_mid() } else { palette::get().accent });
                bx = rx + 46.0 * s;
            }
            let (by, bs) = (ry + (rh - 22.0 * s) / 2.0, 22.0 * s);
            if muted {
                // 灰框 + 灰色 ✓ 只表示「这行原本勾过」，不写盘、重勾父方案即恢复蓝色。
                draw_fill_round(rt, brush, bx, by, bs, bs, 6.0 * s, palette::get().overlay_weak());
                if row.selected {
                    self.text(rt, brush, "✓", &f.glyph, bx, by + 3.0 * s, bs, 16.0 * s,
                        palette::get().text_dim);
                }
            } else if row.selected {
                draw_fill_round(rt, brush, bx, by, bs, bs, 6.0 * s, palette::get().accent);
                self.text(rt, brush, "✓", &f.glyph, bx, by + 3.0 * s, bs, 16.0 * s, palette::get().text);
            } else {
                draw_fill_round(rt, brush, bx, by, bs, bs, 6.0 * s, palette::get().overlay_mid());
            }
            let text_x = bx + bs + 14.0 * s;
            let text_w = (rw - (text_x - rx) - 16.0 * s).max(40.0 * s);
            self.text(
                rt,
                brush,
                &row.name,
                &f.body,
                text_x,
                ry + 10.0 * s,
                text_w,
                20.0 * s,
                if muted { palette::get().text_dim } else { palette::get().text },
            );
            let sub = if is_parent {
                let n = self.pinyin_types.as_ref().map(|p| p.options.len()).unwrap_or(0);
                format!("pinyin · 主方案 · {n} 项输入类型")
            } else if is_child {
                format!(
                    "{} · 拼音输入类型{}",
                    row.id.trim_start_matches("__it__"),
                    if muted { " · 父未选" } else { "" }
                )
            } else {
                match row.series.as_deref() {
                    Some(name) => format!("{name} · {}", row.id),
                    None => row.id.clone(),
                }
            };
            self.text(
                rt,
                brush,
                &sub,
                &f.label,
                text_x,
                ry + 34.0 * s,
                text_w,
                18.0 * s,
                palette::get().text_dim,
            );
        }
        self.paint_scrollbar(rt, brush, x, width, s);

        // 底部：状态文字 + 按钮。
        if let Some((bx, by, bw, bh)) = self.apply_button() {
            let hovered = matches!(self.hover, Hover::Apply);
            draw_fill_round(
                rt,
                brush,
                bx,
                by,
                bw,
                bh,
                9.0 * s,
                if hovered { palette::get().accent } else { palette::get().accent },
            );
            self.text(rt, brush, "保存并部署", &f.center, bx, by + 8.0 * s, bw, 18.0 * s, palette::get().text);
        }
        // 状态文字已在页面副标题显示；这里不再重复画一行，避免压住底部按钮。
    }

    /// 更新页：版本与位置（自动更新按用户 10-31 指示先不做）。
    fn paint_update(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        f: &Formats,
        x: f32,
        width: f32,
        s: f32,
    ) {
        let top = self.list_viewport().0;
        let logs = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .map(|base| base.join("Rime-Next").join("Logs"))
            .unwrap_or_default();
        let rows: [(&str, String); 5] = [
            ("版本", format!("{}（Direct2D 外壳）", env!("CARGO_PKG_VERSION"))),
            ("安装目录", self.update_dir.display().to_string()),
            ("用户数据", self.user_dir.display().to_string()),
            ("日志目录", logs.display().to_string()),
            (
                "自动更新",
                "暂不做（本产品没随包带 WinSparkle；用户 10-31 定）".to_string(),
            ),
        ];
        let row_h = 56.0 * s;
        for (index, (label, value)) in rows.iter().enumerate() {
            let ry = top + index as f32 * (row_h + 8.0 * s);
            draw_fill_round(rt, brush, x, ry, width, row_h, 8.0 * s, palette::get().overlay_weak());
            self.text(rt, brush, label, &f.body, x + 16.0 * s, ry + 18.0 * s, 120.0 * s, 20.0 * s, palette::get().text);
            self.text(
                rt,
                brush,
                value,
                &f.right,
                x + 130.0 * s,
                ry + 18.0 * s,
                width - 150.0 * s,
                20.0 * s,
                palette::get().text_dim,
            );
        }
    }

    /// 常规页：字段可编辑（string/json 行内编辑、choice 点击循环，取色器见
    /// `extras::paint_field_extras`）。值来自 lib 的 `form::fields`。
    ///
    /// 2026-11-16 20:5x 用户要求：「候选窗字号」「每页候选数」两个下拉不再挂在
    /// 列表上方一条，而是成为字段列表最前面的两行（标题/说明在左、下拉框在右、
    /// 随列表滚动、行 hover 高亮；下拉框与浮层由 `extras` 画）。
    fn paint_settings(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        f: &Formats,
        x: f32,
        width: f32,
        s: f32,
    ) {
        let (top, viewport_h) = self.list_viewport();
        for (index, (rx, ry, rw, rh)) in self.row_rects().into_iter().enumerate() {
            if ry + rh < top || ry > top + viewport_h {
                continue;
            }
            if index < 2 {
                // 两个下拉选项行：标题/说明在左，右侧下拉框由 `extras::paint` 画。
                let hovered = matches!(self.hover, Hover::Row(i) if i == index)
                    || matches!(self.hover, Hover::OptionBox(i) if i == index);
                draw_fill_round(
                    rt,
                    brush,
                    rx,
                    ry,
                    rw,
                    rh,
                    10.0 * s,
                    if hovered {
                        palette::get().overlay_mid()
                    } else {
                        palette::get().overlay_weak()
                    },
                );
                self.text(
                    rt,
                    brush,
                    extras::option_row_title(index),
                    &f.body,
                    rx + 16.0 * s,
                    ry + 9.0 * s,
                    rw - 210.0 * s,
                    20.0 * s,
                    palette::get().text,
                );
                self.text(
                    rt,
                    brush,
                    extras::option_row_desc(index),
                    &f.label,
                    rx + 16.0 * s,
                    ry + 33.0 * s,
                    rw - 210.0 * s,
                    16.0 * s,
                    palette::get().text_dim,
                );
                extras::paint_option_box(self, rt, brush, f, index, s);
                continue;
            }
            let field_index = index - 2;
            let row = &self.fields[field_index];
            let hovered = matches!(self.hover, Hover::Row(i) if i == index);
            let editing_this = self.editing == Some(field_index);
            draw_fill_round(
                rt,
                brush,
                rx,
                ry,
                rw,
                rh,
                10.0 * s,
                if editing_this {
                    0x33000000 | (palette::get().accent & 0xFFFFFF)
                } else if hovered {
                    palette::get().overlay_mid()
                } else {
                    palette::get().overlay_weak()
                },
            );
            if editing_this {
                // 行内编辑框:蓝框 + 缓冲 + 光标 + 操作提示
                let bx = rx + rw - 330.0 * s;
                let by = ry + 10.0 * s;
                let bw = 310.0 * s;
                let bh = rh - 20.0 * s;
                draw_fill_round(rt, brush, bx, by, bw, bh, 6.0 * s, darken(palette::get().panel, 0.6));
                draw_fill_round(rt, brush, bx, by, bw, bh, 6.0 * s, 0x14000000 | (palette::get().accent & 0xFFFFFF));
                let shown: String = {
                    // 太长显示尾部(光标恒在末尾)
                    let max_chars = (bw / (9.0 * s)) as usize;
                    let chars: Vec<char> = self.edit_buf.chars().collect();
                    if chars.len() > max_chars {
                        chars[chars.len() - max_chars..].iter().collect()
                    } else {
                        self.edit_buf.clone()
                    }
                };
                self.text(rt, brush, &shown, &f.body, bx + 10.0 * s, by + (bh - 20.0 * s) / 2.0, bw - 20.0 * s, 20.0 * s, palette::get().text);
                // 光标(实心竖线,贴文本尾;简化:恒画在框内右侧固定偏移前)
                let text_w = shown.chars().count() as f32 * 9.0 * s;
                draw_fill_rect(rt, brush, bx + 10.0 * s + text_w, by + (bh - 20.0 * s) / 2.0, 2.0 * s, 20.0 * s, palette::get().accent);
                self.text(rt, brush, "回车保存 · Esc 取消", &f.note, bx, ry + rh + 2.0 * s, bw, 14.0 * s, palette::get().text_dim);
            }
            // 左：字段名 + 说明；右：当前值（右对齐，覆盖过用蓝色）。
            self.text(
                rt,
                brush,
                &row.title,
                &f.body,
                rx + 16.0 * s,
                ry + 9.0 * s,
                rw * 0.55,
                20.0 * s,
                palette::get().text,
            );
            if !row.description.is_empty() {
                self.text(
                    rt,
                    brush,
                    &row.description,
                    &f.label,
                    rx + 16.0 * s,
                    ry + 33.0 * s,
                    rw * 0.55,
                    16.0 * s,
                    palette::get().text_dim,
                );
            }
            self.text(
                rt,
                brush,
                &row.value,
                &f.right,
                rx + rw * 0.56,
                ry + 22.0 * s,
                (rw * 0.42 - 78.0 * s).max(60.0 * s),
                20.0 * s,
                if row.overridden { palette::get().text_dim } else { palette::get().text_dim },
            );
            extras::paint_field_extras(self, rt, brush, field_index, rx, ry, rw, rh, s);
            let _ = &row.kind;
        }
        if self.fields.is_empty() {
            self.text(
                rt,
                brush,
                "没有读到设置字段（原因见上方状态行）",
                &f.body,
                x,
                top + 2.0 * (ROW_H + ROW_GAP) * s + 8.0 * s,
                width,
                22.0 * s,
                palette::get().text_dim,
            );
        }
        self.paint_scrollbar(rt, brush, x, width, s);
    }

    /// 「词库」页四个按钮的矩形(像素):0=查候选 1=加入 2=删除 3=重新读取。
    fn dict_button_rects(&self) -> [(f32, f32, f32, f32); 4] {
        let s = self.scale();
        let (cx, _, cw, _) = self.content_area();
        let right = cx + cw;
        let y = LIST_TOP * s;
        [
            (right - 260.0 * s, y, 80.0 * s, 32.0 * s),
            (right - 170.0 * s, y, 80.0 * s, 32.0 * s),
            (right - 80.0 * s, y, 80.0 * s, 32.0 * s),
            (cx, y, 110.0 * s, 32.0 * s),
        ]
    }

    /// 词库页两个输入框（编码 / 词）的矩形（像素）。paint_dicts 与点击聚焦共用。
    fn dict_input_rects(&self) -> ((f32, f32, f32, f32), (f32, f32, f32, f32)) {
        let s = self.scale();
        let (cx, _, cw, _) = self.content_area();
        let iy = (LIST_TOP + 44.0) * s;
        let ih = 32.0 * s;
        let iw = (cw - 12.0 * s) * 0.5;
        ((cx, iy, iw, ih), (cx + iw + 12.0 * s, iy, iw, ih))
    }

    // ── 「下载」页(2026-11-13 D2D 迁移,Slint download.slint 同源) ─────────────

    /// 当前分类页签下的仓库行下标（`repo_rows` 的子集，顺序保持）。
    fn repo_filtered(&self) -> Vec<usize> {
        if self.repo_tab == 0 || self.repo_tab >= self.repo_cats.len() {
            return (0..self.repo_rows.len()).collect();
        }
        let cat = &self.repo_cats[self.repo_tab];
        self.repo_rows
            .iter()
            .enumerate()
            .filter(|(_, row)| &row.category == cat)
            .map(|(i, _)| i)
            .collect()
    }

    /// 分类页签条矩形（像素，相对内容区，y = LIST_TOP 一行）。宽度按文字粗略估（中文 18/字）。
    fn repo_tab_rects(&self) -> Vec<(f32, f32, f32, f32)> {
        let s = self.scale();
        let (cx, _, _, _) = self.content_area();
        let mut x = cx;
        let y = LIST_TOP * s;
        self.repo_cats
            .iter()
            .map(|cat| {
                let w = (cat.chars().count() as f32 * 18.0 + 28.0) * s;
                let rect = (x, y, w, 32.0 * s);
                x += w + 8.0 * s;
                rect
            })
            .collect()
    }

    /// 仓库行「安装」按钮矩形（像素，过滤后下标序）。与 paint_download 的卡片布局一致。
    fn repo_btn_rects(&self) -> Vec<(f32, f32, f32, f32)> {
        let s = self.scale();
        let (cx, _, cw, _) = self.content_area();
        let right = cx + cw;
        let top = self.list_viewport().0 - self.scroll;
        self.repo_filtered()
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let y = top + i as f32 * (REPO_ROW_H + REPO_GAP) * s;
                (right - 116.0 * s, y + (REPO_ROW_H - 32.0) / 2.0 * s, 96.0 * s, 32.0 * s)
            })
            .collect()
    }

    /// 「下载」页「刷新列表」按钮（页签行最右）。
    fn repo_refresh_rect(&self) -> (f32, f32, f32, f32) {
        let s = self.scale();
        let (cx, _, cw, _) = self.content_area();
        (cx + cw - 104.0 * s, LIST_TOP * s, 104.0 * s, 32.0 * s)
    }

    /// 后台加载仓库列表：静态索引优先（离线/限流都能列），回退 GitHub API。
    /// 结果写信箱，`WM_TIMER` 采样回 UI。
    fn repo_refresh(&mut self) {
        self.repo_status = "正在加载方案列表…".into();
        let job = self.repo_progress.clone();
        let user = self.user_dir.clone();
        std::thread::spawn(move || {
            // 2026-11-16 20:1x 修复：**网络/磁盘加载期间绝不持锁**。
            // 旧写法在 spawn 进来第一行就 `job.lock()` 然后跑 `list_repos` 网络请求；
            // 本机代理链坏（系统代理 7890/client 没跑）时会卡 20+ 秒，而 UI 线程的
            // `WM_TIMER → poll_repo_job` 每 100 ms 也要拿这把锁 ⇒ UI 线程整段卡死，
            // Windows 直接判「未响应」（19:09:56 Application Hang 实测）。锁只包写结果。
            let loaded = if let Some((rows, cats)) =
                repo_rows_from_index(&plum_next::index::default_dir(), &user)
            {
                let total = rows.len();
                Some((rows, cats, format!("共 {total} 个仓库（来源：静态索引）")))
            } else {
                match plum_next::org::list_repos(plum_next::org::ORG) {
                    Ok(repos) => {
                        let (rows, cats) = repo_rows_from_org(&repos, &user);
                        let total = rows.len();
                        Some((rows, cats, format!("共 {total} 个仓库")))
                    }
                    Err((msg, net)) => {
                        let head = if net { "网络错误" } else { "加载失败" };
                        Some((Vec::new(), vec!["全部".to_owned()], format!("{head}：{msg}")))
                    }
                }
            };
            if let Ok(mut guard) = job.lock() {
                guard.loaded = loaded;
            }
        });
    }

    /// 一键安装：拉取 → 搬运 → 记清单 → 请 broker 部署（Slint 版同流程）。
    fn repo_install(&mut self, name: &str) {
        if self.repo_installing.is_some() {
            return;
        }
        let Some(row) = self.repo_rows.iter().find(|r| r.name == name) else {
            return;
        };
        {
            let mut guard = self.repo_progress.lock().unwrap_or_else(|e| e.into_inner());
            guard.frac = 0.0;
            guard.done = None;
        }
        self.repo_installing = Some(name.to_owned());
        self.repo_frac = 0.0;
        self.repo_status = format!("正在安装 {name}…");
        let job = self.repo_progress.clone();
        let (clone_url, branch, pushed_at) =
            (row.clone_url.clone(), row.branch.clone(), row.pushed_at.clone());
        let root = self.user_dir.clone();
        std::thread::spawn(move || {
            let opts = plum_next::fetch::FetchOpts { proxy: None };
            let branch_opt = if branch.is_empty() { None } else { Some(branch.as_str()) };
            let result = plum_next::relocate::pull_and_relocate_all(
                &root, &clone_url, branch_opt, false, &opts, |_, frac, _| {
                    let mut guard = job.lock().unwrap_or_else(|e| e.into_inner());
                    guard.frac = frac as f32;
                },
            );
            let mut guard = job.lock().unwrap_or_else(|e| e.into_inner());
            match result {
                Ok(r) => {
                    // 记清单：装了哪一版（远端 pushed_at），下次列表据此算「可更新」。
                    let full = format!("{}/{}", plum_next::org::ORG, r.repo_name);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let _ = weasel_settings::download::record_installed(
                        &root,
                        weasel_settings::download::InstalledRepo {
                            repo: full,
                            pushed_at: pushed_at.clone(),
                            installed_at: format!("epoch:{now}"),
                            ..Default::default()
                        },
                    );
                    guard.done = Some(Ok(r.repo_name));
                }
                Err(e) => guard.done = Some(Err(e)),
            }
        });
    }

    /// 定时器采样（100ms）：信箱 → Shell 字段，装完自动走「请求部署 → 等完成」。
    fn poll_repo_job(&mut self) {
        // 短临界区：一次拿走全部信箱内容，锁立刻放掉（后面要调 &mut self 方法）。
        let (loaded, done, frac) = {
            let mut guard = self.repo_progress.lock().unwrap_or_else(|e| e.into_inner());
            (guard.loaded.take(), guard.done.take(), guard.frac)
        };
        if let Some((rows, cats, status)) = loaded {
            self.repo_rows = rows;
            self.repo_cats = cats;
            self.repo_tab = self.repo_tab.min(self.repo_cats.len().saturating_sub(1));
            self.repo_status = status;
            self.scroll = self.scroll.min(self.max_scroll());
            self.repaint();
        }
        if self.repo_installing.is_some() {
            self.repo_frac = frac;
            if let Some(result) = done {
                self.repo_installing = None;
                self.repo_frac = 0.0;
                match result {
                    Ok(name) => {
                        self.repo_status = format!("已拉取：{name}，正在部署…");
                        self.deploy_before = read_last_build_time();
                        self.deploy_deadline = now_epoch_secs() + 120;
                        request_deploy();
                        self.repo_refresh();
                    }
                    Err(msg) => self.repo_status = format!("安装失败：{msg}"),
                }
            }
            self.repaint();
        } else if self.deploy_deadline > 0 {
            // 等部署完成：last_build_time 变了就是干完了，最多等 120 秒。
            if read_last_build_time() != self.deploy_before {
                self.repo_status = "已完成拉取并部署，可以直接用了".into();
                self.deploy_deadline = 0;
            } else if now_epoch_secs() > self.deploy_deadline {
                self.repo_status = "已请求部署，但没等到完成信号（可稍后手动部署）".into();
                self.deploy_deadline = 0;
            }
            self.repaint();
        }
    }

    fn paint_download(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        f: &Formats,
        s: f32,
    ) {
        // 布局相对内容区(标题/状态副标题由外层分派画):
        // LIST_TOP 行 = 页签(左) + 刷新列表(右);之下 = 仓库卡片列表。
        let (cx, _, cw, _) = self.content_area();
        let right = cx + cw;
        // 刷新列表按钮
        let (rx, ry, rw, rh) = self.repo_refresh_rect();
        let hovered = matches!(self.hover, Hover::RepoRefresh);
        draw_fill_round(rt, brush, rx, ry, rw, rh, 6.0, if hovered { palette::get().accent_hover() } else { palette::get().accent });
        self.text(rt, brush, "刷新列表", &f.label, rx, ry + 8.0 * s, rw, 16.0 * s, palette::get().accent_text);
        // 分类页签
        for (i, (cat, (x, y, w, h))) in self.repo_cats.iter().zip(self.repo_tab_rects()).enumerate() {
            let active = i == self.repo_tab;
            draw_fill_round(rt, brush, x, y, w, h, 16.0 * s, if active { palette::get().accent } else { palette::get().overlay_weak() });
            self.text(
                rt,
                brush,
                cat,
                &f.label,
                x + 14.0 * s,
                y + 8.0 * s,
                w - 14.0 * s,
                16.0 * s,
                if active { palette::get().accent_text } else { palette::get().text_dim },
            );
        }
        // 仓库卡片列表（滚动，skip 掉滚出视口顶部的行）
        let (top, vh) = self.list_viewport();
        let row_h = REPO_ROW_H * s;
        let btn_rects = self.repo_btn_rects();
        let start_y = top - self.scroll;
        let filtered = self.repo_filtered();
        for (fi, &ri) in filtered.iter().enumerate() {
            let y = start_y + fi as f32 * (row_h + REPO_GAP * s);
            if y + row_h < top || y > top + vh {
                continue;
            }
            let row = &self.repo_rows[ri];
            // 卡片
            draw_fill_round(rt, brush, cx, y, cw, row_h, 8.0 * s, palette::get().card);
            // 首字母徽标(文字用整块矩形,吃内置的垂直居中)
            draw_fill_round(rt, brush, cx + 16.0 * s, y + 22.0 * s, 48.0 * s, 48.0 * s, 8.0 * s, palette::get().accent);
            self.text(rt, brush, &row.initial, &f.title, cx + 16.0 * s, y + 22.0 * s, 48.0 * s, 48.0 * s, palette::get().accent_text);
            // 名称 + 描述
            self.text(rt, brush, &row.name, &f.body, cx + 80.0 * s, y + 16.0 * s, 420.0 * s, 22.0 * s, palette::get().text);
            self.text(rt, brush, &row.description, &f.label, cx + 80.0 * s, y + 46.0 * s, cw - 260.0 * s, 18.0 * s, palette::get().text_dim);
            // 徽标（右上）：已安装=绿 / 可更新=橙
            if !row.badge.is_empty() {
                let color = if row.badge == "已安装" { 0xFF16A34A } else { 0xFFD97706 };
                draw_fill_round(rt, brush, right - 116.0 * s, y + 14.0 * s, 96.0 * s, 24.0 * s, 12.0 * s, color);
                self.text(rt, brush, row.badge, &f.label, right - 116.0 * s, y + 18.0 * s, 96.0 * s, 16.0 * s, palette::get().accent_text);
            }
            // 安装按钮（正在安装时按钮变进度条）
            let (bx, by, bw, bh) = btn_rects[fi];
            if self.repo_installing.as_deref() == Some(row.name.as_str()) {
                draw_fill_round(rt, brush, bx, by, bw, bh, 6.0, palette::get().btn_bg());
                draw_fill_round(rt, brush, bx, by, bw * self.repo_frac.clamp(0.0, 1.0), bh, 6.0, palette::get().accent);
                self.text(
                    rt,
                    brush,
                    &format!("{}%", (self.repo_frac * 100.0) as i32),
                    &f.label,
                    bx,
                    by + 8.0 * s,
                    bw,
                    16.0 * s,
                    palette::get().text,
                );
            } else {
                let hovered = matches!(self.hover, Hover::RepoBtn(b) if b == fi);
                draw_fill_round(rt, brush, bx, by, bw, bh, 6.0, if hovered { palette::get().accent_hover() } else { palette::get().accent });
                self.text(rt, brush, "安装", &f.label, bx, by + 8.0 * s, bw, 16.0 * s, palette::get().accent_text);
            }
        }
        if filtered.is_empty() {
            self.text(rt, brush, "还没有仓库列表——点「刷新列表」加载（离线时检查网络）", &f.label, cx, top + 20.0 * s, cw, 18.0 * s, palette::get().text_dim);
        }
    }

    /// 「词库」页:查候选 → 看名次 → 加词/删词(Slint dicts.slint 的 D2D 版)。
    fn paint_dicts(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        f: &Formats,
        s: f32,
    ) {
        // 布局全部相对内容区(标题/副标题由外层分派画,这里从 LIST_TOP 开始):
        // 第一行 = 四个动作按钮;第二行 = 编码/词两个输入框;之下 = 汇总列表。
        let (cx, _, cw, _) = self.content_area();
        let right = cx + cw;
        // 动作按钮(矩形与 dict_button_rects 同源):重新读取在左,其余三个靠右。
        let names = ["查候选", "加入词库", "删除", "重新读取"];
        for (i, (bx, by, bw, bh)) in self.dict_button_rects().into_iter().enumerate() {
            let hovered = matches!(self.hover, Hover::DictBtn(b) if b == i);
            draw_fill_round(rt, brush, bx, by, bw, bh, 6.0, if hovered { palette::get().accent_hover() } else { palette::get().accent });
            self.text(rt, brush, names[i], &f.label, bx, by + 8.0 * s, bw, 16.0 * s, palette::get().accent_text);
        }
        // 输入框两个:编码 / 词
        let ((ix, iy, iw, ih), (wx, wy, ww, wh)) = self.dict_input_rects();
        let focus_code = self.dict_edit == 1;
        let focus_word = self.dict_edit == 2;
        draw_fill_round(rt, brush, ix, iy, iw, ih, 6.0, if focus_code { palette::get().accent } else { palette::get().btn_bg() });
        self.text(
            rt,
            brush,
            &format!("编码(如 cs,可只打前缀){}{}", if self.dict_code.is_empty() { " " } else { ":" }, self.dict_code),
            &f.label,
            ix + 8.0 * s,
            iy + 8.0 * s,
            iw - 16.0 * s,
            18.0 * s,
            if self.dict_code.is_empty() { palette::get().text_dim } else { palette::get().text },
        );
        draw_fill_round(rt, brush, wx, wy, ww, wh, 6.0, if focus_word { palette::get().accent } else { palette::get().btn_bg() });
        self.text(
            rt,
            brush,
            &format!("要加的词{}", if self.dict_word.is_empty() { String::new() } else { format!(":{}", self.dict_word) }),
            &f.label,
            wx + 8.0 * s,
            wy + 8.0 * s,
            ww - 16.0 * s,
            18.0 * s,
            if self.dict_word.is_empty() { palette::get().text_dim } else { palette::get().text },
        );
        // 结果列表(滚动区)
        let (top, vh) = self.list_viewport();
        let row_h = 32.0 * s;
        draw_fill_round(rt, brush, cx, top, cw, vh, 6.0, palette::get().card);
        if self.dict_rows.is_empty() {
            self.text(
                rt,
                brush,
                "点「查候选」汇总用户词典，点行看名次；改动在「加入 / 删除」后回方案页部署生效",
                &f.label,
                cx + 12.0 * s,
                top + 14.0 * s,
                cw - 24.0 * s,
                18.0 * s,
                palette::get().text_dim,
            );
        }
        let skip = (self.dict_scroll / row_h) as usize;
        let mut y = top;
        for (i, (rank, word, code, source)) in self.dict_rows.iter().skip(skip).enumerate() {
            if y + row_h > top + vh {
                break;
            }
            let bg = if i % 2 == 0 { palette::get().card } else { palette::get().card };
            draw_fill_rect(rt, brush, cx, y, right, y + row_h, bg);
            self.text(rt, brush, &format!("{}.", rank), &f.label, cx + 8.0 * s, y + 7.0 * s, 40.0 * s, 18.0 * s, palette::get().text_dim);
            self.text(rt, brush, word, &f.body, cx + 56.0 * s, y + 6.0 * s, 300.0 * s, 20.0 * s, palette::get().text);
            self.text(rt, brush, code, &f.label, cx + 380.0 * s, y + 7.0 * s, 200.0 * s, 18.0 * s, palette::get().text_dim);
            self.text(rt, brush, source, &f.label, cx + 600.0 * s, y + 7.0 * s, 200.0 * s, 18.0 * s, palette::get().text_dim);
            y += row_h;
        }
    }

    fn paint_placeholder(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        f: &Formats,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        s: f32,
    ) {
        let card_h = (height - 20.0 * s).clamp(70.0 * s, 120.0 * s);
        draw_fill_round(rt, brush, x, y, width, card_h, 8.0 * s, palette::get().overlay_weak());
        draw_fill_round(rt, brush, x, y, 3.0 * s, card_h, 1.5 * s, 0x80000000 | (palette::get().accent & 0xFFFFFF));
        self.text(
            rt,
            brush,
            "这一页暂时没有可显示的内容",
            &f.body,
            x + 20.0 * s,
            y + 16.0 * s,
            width - 40.0 * s,
            22.0 * s,
            palette::get().text,
        );
        self.text(
            rt,
            brush,
            PAGE_HINTS[self.page],
            &f.label,
            x + 20.0 * s,
            y + 42.0 * s,
            width - 40.0 * s,
            18.0 * s,
            palette::get().text_dim,
        );
        self.text(
            rt,
            brush,
            "七页均已迁移到 Direct2D 外壳（本卡片仅为兜底）",
            &f.label,
            x + 20.0 * s,
            y + 64.0 * s,
            width - 40.0 * s,
            18.0 * s,
            palette::get().text_dim,
        );
    }

    fn on_size(&mut self) {
        let (w, h) = self.client_size();
        if w < 1.0 || h < 1.0 {
            return;
        }
        if let Some(present) = &mut self.presenter {
            if let Err(error) = unsafe { present.resize(w as u32, h as u32) } {
                log(&format!("resize 失败：{error}"));
            }
        } else if let Ok(present) = unsafe { Presenter::new(self.hwnd, w as u32, h as u32) } {
            self.presenter = Some(present);
        }
        self.repaint();
    }

    fn repaint(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn rebuild_formats(&mut self) {
        let s = self.scale();
        self.formats = None;
        // 文本格式只跟字号有关；D2D 目标的 DPI 固定 96，字号按缩放倍数算。
        let factory: IDWriteFactory = match unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
        {
            Ok(factory) => factory,
            Err(error) => {
                log(&format!("DWriteCreateFactory 失败：{error}"));
                return;
            }
        };
        let family = self.font_family();
        let make = |weight: DWRITE_FONT_WEIGHT, size: f32, align: DWRITE_TEXT_ALIGNMENT| {
            create_text_format(&factory, &family, weight, size, align).ok()
        };
        match (
            make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 15.0 * s, DWRITE_TEXT_ALIGNMENT_LEADING),
            make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 22.0 * s, DWRITE_TEXT_ALIGNMENT_LEADING),
            make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.5 * 1.5 * s, DWRITE_TEXT_ALIGNMENT_LEADING),
            make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.5 * s, DWRITE_TEXT_ALIGNMENT_LEADING),
            make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.0 * s, DWRITE_TEXT_ALIGNMENT_LEADING),
            make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.0 * s, DWRITE_TEXT_ALIGNMENT_CENTER),
            make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 11.5 * s, DWRITE_TEXT_ALIGNMENT_TRAILING),
            make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 16.0 * s, DWRITE_TEXT_ALIGNMENT_CENTER),
            make(DWRITE_FONT_WEIGHT_BOLD, 13.5 * s, DWRITE_TEXT_ALIGNMENT_LEADING),
        ) {
            (
                Some(title),
                Some(heading),
                Some(nav),
                Some(body),
                Some(label),
                Some(glyph),
                Some(right),
                Some(center),
                Some(note),
            ) => {
                self.formats = Some(Formats {
                    title,
                    heading,
                    nav,
                    body,
                    label,
                    glyph,
                    right,
                    center,
                    note,
                });
            }
            _ => log("文本格式创建失败（字体缺失？）"),
        }
    }

    fn font_family(&self) -> String {
        // 中文界面优先微软雅黑，缺字时 DWrite 会自动回退到系统字体。
        "Microsoft YaHei UI".to_string()
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // 客户区铺满整个窗口（自绘标题栏），系统框架不占位；缩放仍由 HT* 命中负责。
            WM_NCCALCSIZE if wp.0 != 0 => LRESULT(0),
            WM_NCHITTEST => {
                let mut rect = RECT::default();
                if !GetWindowRect(hwnd, &mut rect).as_bool() {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                let x = (lp.0 as u16 as i16) as i32 - rect.left;
                let y = ((lp.0 >> 16) as u16 as i16) as i32 - rect.top;
                let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
                let s = shell().map(|s| s.scale()).unwrap_or(1.0);
                let border = (6.0 * s) as i32;
                let maximized = GetWindowLongPtrW(hwnd, GWL_STYLE_) & WS_MAXIMIZE_ as isize != 0;
                if !maximized {
                    let left = x < border;
                    let right = x >= w - border;
                    let top = y < border;
                    let bottom = y >= h - border;
                    if top && left {
                        return LRESULT(HTTOPLEFT_);
                    }
                    if top && right {
                        return LRESULT(HTTOPRIGHT_);
                    }
                    if bottom && left {
                        return LRESULT(HTBOTTOMLEFT_);
                    }
                    if bottom && right {
                        return LRESULT(HTBOTTOMRIGHT_);
                    }
                    if left {
                        return LRESULT(HTLEFT_);
                    }
                    if right {
                        return LRESULT(HTRIGHT_);
                    }
                    if top {
                        return LRESULT(HTTOP_);
                    }
                    if bottom {
                        return LRESULT(HTBOTTOM_);
                    }
                }
                // 标题栏空白处交给系统拖动；三个按钮与页签要拿到客户区消息。
                let (fx, fy) = (x as f32, y as f32);
                let over_control = shell()
                    .map(|s| !matches!(s.hit(fx, fy), Hover::None))
                    .unwrap_or(false);
                if !over_control && fy < 52.0 * s {
                    return LRESULT(HTCAPTION_);
                }
                LRESULT(HTCLIENT_)
            }
            WM_SIZE => {
                if let Some(s) = shell() {
                    s.on_size();
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let new_dpi = (wp.0 & 0xffff) as u32;
                if let Some(s) = shell() {
                    s.dpi = new_dpi.max(96);
                    let suggested = lp.0 as *const RECT;
                    if !suggested.is_null() {
                        let rect = *suggested;
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            rect.left,
                            rect.top,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                    s.rebuild_formats();
                    s.repaint();
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if let Some(s) = shell() {
                    let x = (lp.0 as u16 as i16) as f32;
                    let y = ((lp.0 >> 16) as u16 as i16) as f32;
                    let hover = s.hit(x, y);
                    if hover != s.hover {
                        s.hover = hover;
                        s.repaint();
                    }
                    let mut tracking = TRACKMOUSEEVENT {
                        cbSize: core::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE as u32,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tracking);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                if let Some(s) = shell() {
                    if s.hover != Hover::None {
                        s.hover = Hover::None;
                        s.repaint();
                    }
                }
                LRESULT(0)
            }
            WM_CHAR => {
                // 新增的搜索框 / 子编辑器输入框优先消费按键。
                if let Some(s) = shell() {
                    if extras::on_char(s, wp.0 as u32) {
                        s.repaint();
                        return LRESULT(0);
                    }
                }
                // 词库页的两个输入框(2026-11-08 D2D 迁移)
                if let Some(s) = shell() {
                    if s.page == PAGE_DICTS && s.dict_edit > 0 {
                        let code = wp.0 as u32;
                        match code {
                            0x0D => {
                                if s.dict_edit == 1 { s.dict_find(); } else { s.dict_add(); }
                            }
                            0x1B => s.dict_edit = 0,
                            0x08 => {
                                let buf = if s.dict_edit == 1 { &mut s.dict_code } else { &mut s.dict_word };
                                buf.pop();
                            }
                            _ => {
                                // 编码框只收 ASCII（码表编码）；词框收任意可打印字符（含中文）。
                                if let Some(ch) = char::from_u32(code)
                                    && !ch.is_control()
                                    && (s.dict_edit == 2 || ch.is_ascii())
                                {
                                    log(&format!("[char] WM_CHAR={code:#X} → 词库框{}", s.dict_edit));
                                    let buf = if s.dict_edit == 1 {
                                        &mut s.dict_code
                                    } else {
                                        &mut s.dict_word
                                    };
                                    buf.push(ch);
                                }
                            }
                        }
                        s.repaint();
                        return LRESULT(0);
                    }
                    if s.editing.is_some() {
                        let code = wp.0 as u32;
                        match code {
                            0x0D => s.edit_commit(),          // 回车
                            0x1B => s.edit_cancel(),          // Esc
                            0x08 => s.edit_backspace(),       // 退格
                            _ => {
                                // 任意可打印字符都收（含中文；输入法上屏可能走 WM_CHAR）。
                                if let Some(ch) = char::from_u32(code)
                                    && !ch.is_control()
                                {
                                    log(&format!("[char] WM_CHAR={code:#X} → 字段行"));
                                    s.edit_push(ch);
                                }
                            }
                        }
                        return LRESULT(0);
                    }
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_TIMER => {
                // 部署进度浮层 / 太阳深浅之外的定时更新。
                if let Some(s) = shell() {
                    extras::tick(s);
                }
                // 「下载」页后台任务信箱采样（100ms）：进度/结果 → UI。
                if let Some(s) = shell() {
                    s.poll_repo_job();
                    // 词库扫描/查询信箱采样:后台 IO 完成 → 热更新(信号模式)。
                    s.poll_dict_job();
                    s.poll_dict_find();
                }
                // 词云后台重建采样（100ms）：建好就换缓存并重绘。
                if let Some(s) = shell() {
                    s.poll_cloud();
                }
                // AC2 背景动画（2026-11-14）已确认从 11-15 起被不透明档渐变完全盖住
                // （live 两帧 changed_pixels=0）⇒ 2026-11-16 variant B 起不再 5fps 重绘
                // （纯烧 CPU，无视觉产出）。若恢复 A 方案（渐变挪到 AC2 之前）需同时把
                // `if s.tick % 2 == 0 { s.repaint(); }` 加回来。
                if let Some(s) = shell() {
                    s.tick = s.tick.wrapping_add(1);
                }
                // 太阳深浅:每 100 个 tick(10 秒)重算一次,过日出/日落线就换调色板。
                if let Some(s) = shell() {
                    if s.tick % 100 == 0 {
                        let pal = palette::load();
                        let changed = pal.is_dark != palette::get().is_dark;
                        if changed {
                            palette::init(pal);
                            palette::sync_material(s.hwnd, s.candidate_material);
                            s.repaint();
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                if let Some(s) = shell() {
                    // 高 16 位是有符号的滚轮增量（120 = 一格）。
                    let delta = ((wp.0 >> 16) & 0xffff) as u16 as i16 as f32;
                    // 下拉选项浮层打开时，滚轮只滚列表、不动页面。
                    if let Some(pop) = s.option_popup {
                        let row_h = extras::OPTION_ROW_DIP * s.scale();
                        let max = extras::option_max_scroll(s, pop);
                        let target = (s.option_scroll - delta / 120.0 * 3.0 * row_h).clamp(0.0, max);
                        let snapped = (target / row_h).round() * row_h;
                        let snapped = snapped.clamp(0.0, max);
                        if (snapped - s.option_scroll).abs() > 0.01 {
                            s.option_scroll = snapped;
                            s.repaint();
                        }
                        return LRESULT(0);
                    }
                    if s.page == PAGE_DICTS {
                        // 词库列表滚动走自己的 dict_scroll(行高 32)。
                        let row = 32.0 * s.scale();
                        let (_, vh) = s.list_viewport();
                        let max = (s.dict_rows.len() as f32 * row - vh).max(0.0);
                        let target = (s.dict_scroll - delta / 120.0 * 3.0 * row).clamp(0.0, max);
                        if (target - s.dict_scroll).abs() > 0.01 {
                            s.dict_scroll = target;
                            s.repaint();
                        }
                        return LRESULT(0);
                    }
                    let row = if s.page == PAGE_DOWNLOAD {
                        (REPO_ROW_H + REPO_GAP) * s.scale()
                    } else {
                        (ROW_H + ROW_GAP) * s.scale()
                    };
                    let step = 3.0 * row;
                    let target = (s.scroll - delta / 120.0 * step).clamp(0.0, s.max_scroll());
                    // 用户 16:0x：滚动**按整行吸附**，永远不出现"半行"（否则行会压住标题）。
                    let snapped = (target / row).round() * row;
                    let snapped = snapped.clamp(0.0, s.max_scroll());
                    if (snapped - s.scroll).abs() > 0.01 {
                        s.scroll = snapped;
                        s.repaint();
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                if let Some(s) = shell() {
                    let x = (lp.0 as u16 as i16) as f32;
                    let y = ((lp.0 >> 16) as u16 as i16) as f32;
                    // 词库页输入框聚焦:点编码框/词框置 dict_edit,点空白清除。
                    if s.page == PAGE_DICTS {
                        let ((cx1, cy1, cw1, ch1), (cx2, cy2, cw2, ch2)) = s.dict_input_rects();
                        let in1 = x >= cx1 && x <= cx1 + cw1 && y >= cy1 && y <= cy1 + ch1;
                        let in2 = x >= cx2 && x <= cx2 + cw2 && y >= cy2 && y <= cy2 + ch2;
                        let new_edit = if in1 {
                            1
                        } else if in2 {
                            2
                        } else {
                            0
                        };
                        if new_edit != s.dict_edit {
                            s.dict_edit = new_edit;
                            s.repaint();
                        }
                        if in1 || in2 {
                            return LRESULT(0);
                        }
                    }
                    let hovered_now = s.hit(x, y);
                    s.hover = hovered_now;
                    if extras::on_click(s, hovered_now) {
                        s.repaint();
                        return LRESULT(0);
                    }
                    match hovered_now {
                        Hover::DictBtn(kind) => {
                            match kind {
                                0 => s.dict_find(),
                                1 => s.dict_add(),
                                2 => s.dict_remove(),
                                3 => s.dict_refresh(),
                                _ => {}
                            }
                            s.repaint();
                        }
                        Hover::Close => {
                            let _ = DestroyWindow(hwnd);
                        }
                        Hover::Minimize => {
                            let _ = ShowWindow(hwnd, SW_MINIMIZE_);
                        }
                        Hover::Maximize => {
                            let maximized =
                                GetWindowLongPtrW(hwnd, GWL_STYLE_) & WS_MAXIMIZE_ as isize != 0;
                            let _ = ShowWindow(
                                hwnd,
                                if maximized { SW_RESTORE_ } else { SW_MAXIMIZE_ },
                            );
                        }
                        Hover::Page(index) => {
                            // 点击只发信号：页面状态与「方案页重载清单」由 emit 统一处理。
                            s.emit(Signal::PageSelected(index));
                        }
                        Hover::Row(index) => {
                            if s.page == PAGE_SETTINGS {
                                // 前两行是「候选窗字号」「每页候选数」选项行：点行（含标题区）
                                // 也打开下拉浮层；其余行才是字段行 ⇒ 字段下标 = 行下标 - 2。
                                if index < 2 {
                                    extras::toggle_option_box(s, index);
                                } else {
                                    s.cycle_field(index - 2);
                                }
                            } else if s.page == PAGE_THEMES {
                                s.select_scheme(index);
                            } else if s.page == PAGE_SCHEMAS {
                                // 分组折叠 / 输入类型启用集同步 / 普通方案勾选统一在 extras 处理。
                                extras::toggle_schema_row(s, index);
                            }
                        }
                        Hover::Apply => s.save_schemas(),
                        Hover::ColorMode(index) => {
                            s.color_mode = index;
                            palette::write_mode(&s.user_dir, index);
                            palette::init(palette::load());
                            palette::sync_material(s.hwnd, s.candidate_material);
                            s.repaint();
                        }
                        Hover::LayoutBtn(index) => {
                            let values = ["horizontal_squeeze", "vertical_squeeze", "cells"];
                            s.write_layout_mode(values[index.min(2)]);
                            s.repaint();
                        }
                        Hover::Material(index) => s.set_material(index),
                        Hover::RepoTab(index) => {
                            s.repo_tab = index;
                            s.scroll = 0.0;
                            s.repaint();
                        }
                        Hover::RepoRefresh => s.repo_refresh(),
                        Hover::RepoBtn(index) => {
                            let filtered = s.repo_filtered();
                            if let Some(&ri) = filtered.get(index) {
                                let name = s.repo_rows[ri].name.clone();
                                s.repo_install(&name);
                            }
                            s.repaint();
                        }
                        Hover::None => {}
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            // 外部信号入口：PostMessageW(hwnd, WM_APP_PAGE_SELECTED, 页号, 0)。
            WM_APP_PAGE_SELECTED => {
                if let Some(s) = shell() {
                    s.emit(Signal::PageSelected(wp.0 as usize));
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let _ = BeginPaint(hwnd, &mut ps);
                if let Some(s) = shell() {
                    s.paint();
                }
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                // 记住窗口尺寸:客户区物理像素即 DIP(npixels 是 1:1 的?否——
                // 这里统一换算回 DIP 存,下次按当前 DPI 重算)
                if let Some(s) = shell() {
                    let (w, h) = s.client_size();
                    let scale = s.scale();
                    save_window_size(w / scale, h / scale);
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn run() -> Result<(), String> {
    unsafe {
        // 本线程按 Per-Monitor V2 处理 DPI（不写清单，直接运行时设置）。
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT(-4isize as *mut _));

        let instance = GetModuleHandleW(None);
        if instance.0.is_null() {
            return Err("GetModuleHandleW 失败".into());
        }
        let class = w!("Weasel-Next-Settings-Shell-D2D");
        let cursor = LoadCursorW(None, windows_core::PCWSTR::from_raw(IDC_ARROW_ as *const u16));
        let wc = WNDCLASSW {
            style: CS_VREDRAW_HREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: HINSTANCE(instance.0),
            hIcon: HICON::default(),
            hCursor: cursor,
            hbrBackground: HBRUSH::default(),
            lpszMenuName: windows_core::PCWSTR::null(),
            lpszClassName: class,
        };
        if RegisterClassW(&wc).0 == 0 {
            return Err("RegisterClassW 失败".into());
        }

        // 记住窗口尺寸(2026-11-01):上次退出的大小优先,出厂常量兜底
        let (stored_w, stored_h) = load_window_size().unwrap_or((WINDOW_W, WINDOW_H));
        let window_w = npixels(stored_w);
        let window_h = npixels(stored_h);
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP_ | WS_EX_APPWINDOW_,
            class,
            w!("小狼毫Next 设置"),
            WS_POPUP_ | WS_THICKFRAME_ | WS_MINIMIZEBOX_ | WS_MAXIMIZEBOX_ | WS_SYSMENU_,
            CW_USEDEFAULT_,
            CW_USEDEFAULT_,
            window_w,
            window_h,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        );
        if hwnd.0.is_null() {
            return Err("CreateWindowExW 失败".into());
        }
        // 材质:--material=glass|metal|acrylic 覆盖;candidate_material 在后段赋值,
        // 这里先占位 Glass,set_material 与启动后段会统一同步。
        let material = std::env::args()
            .find_map(|arg| arg.strip_prefix("--material=").map(str::to_owned))
            .map(|value| match value.as_str() {
                "metal" => Material::Metal,
                "acrylic" => Material::Acrylic,
                _ => Material::Glass,
            })
            .unwrap_or(Material::Glass);
        // 截图/验收用：`--page=1` 直接开在「外观」页（不必拿鼠标去点）。
        let start_page = std::env::args()
            .skip(1)
            .find_map(|arg| {
                arg.strip_prefix("--page=")
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap_or(0)
            .min(PAGES.len() - 1);

        // 深色标题/边框（本机是深色主题，与外壳配色一致）。
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE_,
            &dark as *const _ as *const core::ffi::c_void,
            core::mem::size_of::<i32>() as u32,
        );
        // Win11：让 DWM 自己把窗口裁成圆角（亚克力模糊也随之被裁）。
        let corner: i32 = DWMWCP_ROUND_;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE_,
            &corner as *const _ as *const core::ffi::c_void,
            core::mem::size_of::<i32>() as u32,
        );
        // 系统级亚克力（2026-11-04 用户要求"识别背景颜色"）:
        // 染色 alpha 0x99→0x66(40%)——之前 60% 不透明深色把壁纸颜色全盖住,
        // 只剩"黑板子"。降染色后 DWM 模糊的背景颜色能透出来。
        // 亚克力染色跟候选窗活动配色(palette.rs):面板色 + 40% alpha。
        palette::init(palette::load());
        acrylic::enable_acrylic(hwnd, 0x40000000 | (palette::get().panel & 0x00FFFFFF));

        // 配色页数据：与 Slint 版同一份（`weasel_settings::themes`）。
        let shared = weasel_settings::themes::shared_dir_for_exe();
        let user = weasel_settings::themes::user_dir();
        let mut shell_state = Shell {
            hwnd,
            dpi: GetDpiForWindow(hwnd).max(96),
            presenter: None,
            formats: None,
            // 初始页由信号决定（见下方 `emit(PageSelected(start_page))`）。
            page: 0,
            hover: Hover::None,
            themes: None,
            rows: Vec::new(),
            status: String::new(),
            stats: None,
            document: None,
            paths: None,
            editing: None,
            dict_rows: Vec::new(),
            dict_status: String::new(),
            dict_mail: std::sync::Arc::new(std::sync::Mutex::new(None)),
            dict_find_mail: std::sync::Arc::new(std::sync::Mutex::new(None)),
            dict_code: String::new(),
            dict_word: String::new(),
            dict_edit: 0,
            dict_scroll: 0.0,
            repo_rows: Vec::new(),
            repo_cats: vec!["全部".to_owned()],
            repo_tab: 0,
            repo_status: String::new(),
            repo_installing: None,
            repo_frac: 0.0,
            repo_progress: std::sync::Arc::new(std::sync::Mutex::new(RepoJob::default())),
            deploy_before: None,
            deploy_deadline: 0,
            page_size: 5,
            edit_buf: String::new(),
            home_content_h: std::cell::Cell::new(0.0),
            option_popup: None,
            option_scroll: 0.0,
            pinyin_types: None,
            schema_disabled: std::collections::BTreeSet::new(),
            schema_filter: String::new(),
            schema_filter_focus: false,
            font_point: 0,
            dirty: false,
            invalid: std::collections::BTreeSet::new(),
            extra: None,
            deploy_overlay: None,
            hours: ([0; 24], [0; 24]),
            stats_note: String::new(),
            fields: Vec::new(),
            settings_note: String::new(),
            schemas: Vec::new(),
            collapsed: std::collections::BTreeSet::new(),
            schema_status: String::new(),
            material,
            candidate_material: weasel_settings::material::current(&user),
            shared_dir: shared.clone(),
            user_dir: user.clone(),
            update_dir: std::path::PathBuf::new(),
            scroll: 0.0,
            tick: 0u32,
            color_mode: 2,
            week_bars: Vec::new(),
            hour_summary: String::new(),
            hour_peak: 0,
            cloud_formats: RefCell::new(std::collections::HashMap::new()),
            cloud_cache: RefCell::new(None),
            cloud_pending: std::sync::Arc::new(std::sync::Mutex::new(None)),
            cloud_building: false,
            hour_pct: Vec::new(),
            palettes: std::collections::BTreeMap::new(),
        };
        shell_state.color_mode = palette::current_mode(&user);
        shell_state.palettes = palette::load_all();
        // 材质同步:Shell.material 跟 material.txt,DWM 亚克力按档开/关。
        shell_state.material = match shell_state.candidate_material {
            0 => Material::Glass,
            1 => Material::Acrylic,
            _ => Material::Metal,
        };
        palette::sync_material(hwnd, shell_state.candidate_material);
        let themes = Themes::load(&shared, &user);
        shell_state.status = themes.status();
        // 16 套皮肤一行一套:深浅由右上三钮(color_mode)决定,行里不再放 .dark 重复项。
        shell_state.rows = themes
            .rows()
            .into_iter()
            .filter(|row| !row.id.ends_with(".dark"))
            .collect();
        shell_state.themes = Some(themes);
        // 主页数据：与 Slint 版同一份 `weasel_settings::stats` 聚合。
        shell_state.refresh_stats();
        // 常规页字段：同样走 lib（`config::Document` + `catalog::load` + `form::fields`）。
        shell_state.refresh_settings();
        // 方案页：与 Slint 版同一份 `weasel_style::switcher`。
        shell_state.refresh_schemas();
        // 更新页：WinSparkle 偏好（只读写偏好，不初始化检查器）。
        shell_state.refresh_update();
        // 每页候选数：读生效值（用户 default.yaml → 出厂 → 5）。
        shell_state.page_size = current_page_size(&user, &shared);
        // 下载页：后台拉仓库列表（索引优先），WM_TIMER 定时采样进度。
        shell_state.repo_refresh();
        shell_state.rebuild_formats();
        if shell_state.formats.is_none() {
            return Err("文本格式创建失败".into());
        }
        let (w, h) = shell_state.client_size();
        if w < 1.0 || h < 1.0 {
            return Err("客户区尺寸为 0".into());
        }
        shell_state.presenter = Some(Presenter::new(hwnd, w as u32, h as u32)?);
        // 存进全局：WndProc 直接取（单线程 UI，生命周期 = 进程）。
        *core::ptr::addr_of_mut!(SHELL) = Some(shell_state);
        // 初始尺寸必须按 **实际 DPI** 折算：逻辑 1000x660 DIP，125% 缩放下是 1250x825 像素。
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        center_in_work_area(
            hwnd,
            npixels(WINDOW_W * scale),
            npixels(WINDOW_H * scale),
        );
        let _ = ShowWindow(hwnd, SW_SHOW_);
        // `--page=2` 与鼠标点「方案」标签一样，只发信号；由分发器切页并重载方案清单。
        if let Some(s) = shell() {
            s.emit(Signal::PageSelected(start_page));
        }
        // 下载页进度采样定时器：100ms 一次，无任务时空转（几次整数比较）。
        unsafe {
            SetTimer(
                Some(hwnd),
                1,
                100,
                None,
            );
        }
        if let Some(s) = shell() {
            s.repaint();
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // 窗口已销毁：先放掉图形资源，再退出（Presenter 的 Drop 会摘掉合成目标）。
        *core::ptr::addr_of_mut!(SHELL) = None;
    }
    Ok(())
}

/// 按最近显示器的工作区居中（CW_USEDEFAULT 对 WS_POPUP 会落在 0,0，压住任务栏）。
unsafe fn center_in_work_area(hwnd: HWND, w: i32, h: i32) {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST as u32);
        let mut info = MONITORINFO {
            cbSize: core::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
        let x = info.rcWork.left + ((info.rcWork.right - info.rcWork.left) - w) / 2;
        let y = info.rcWork.top + ((info.rcWork.bottom - info.rcWork.top) - h) / 2;
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

fn npixels(dip: f32) -> i32 {
    // 初始窗口按 96 DPI 的 DIP 折算；创建后 DWM 会按实际 DPI 让我们收到 WM_DPICHANGED。
    dip as i32
}

fn main() {
    // 与 Slint 版相同的命令行开关：/style /switcher /dict /stat /deploy。
    // 前三个复用官方 Win32 对话框模板；后两个转交同包 Slint 窗口。
    if extras::run_cli() {
        return;
    }
    if let Err(error) = run() {
        log(&format!("外壳启动失败：{error}"));
    }
}

/// 同包 Slint 设置程序（独立统计窗 / 部署进度窗）。
/// `dialogs::run_from_args` 的 `/stat` 与 `/deploy` 会落到这两个函数上。
pub fn show_stats() -> Result<(), String> {
    extras::open_companion("/stat")
}
pub fn deploy_with_progress() -> Result<(), String> {
    extras::open_companion("/deploy")
}
