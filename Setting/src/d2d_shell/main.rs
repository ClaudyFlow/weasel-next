//! Direct2D 设置界面外壳 v1（实验）：Win32 顶层窗口 + DComp 交换链 + D2D/DWrite 自绘，
//! DWM 亚克力做背景。**不动现有 Slint 设置界面**——本程序是独立 exe
//! （`Weasel-Next-Setting-D2D.exe`），先从托盘/命令行单独启动，页面内容后续逐页迁移。
//!
//! 为什么用 DComp 而不是 `ID2D1HwndRenderTarget`：真玻璃要求窗口逐像素透明
//! （圆角之外透出桌面模糊），HwndRenderTarget 拿不到逐像素 alpha；候选窗
//! （`theme/src/presenter.rs`）已经用同一条路走通，这里照搬。
//!
//! v2 范围（2026-10-31）：页签改成与 Slint 版一致的 **7 页**（主页/界面风格/方案选单/
//! 常规/更新/词库/下载）；「界面风格」已接真数据（`weasel_settings::themes`，与 Slint
//! 版同一份：读出厂主题链 + 用户 `theme/theme.yaml`），支持滚轮滚动、悬停、点选换配色。
//! 其余六页仍是占位，逐页迁移。

#![windows_subsystem = "windows"]
#![allow(non_snake_case, non_upper_case_globals, clippy::all)]

mod acrylic;
mod present;

use present::Presenter;
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
const WINDOW_W: f32 = 1000.0;
const WINDOW_H: f32 = 660.0;
/// 主页(个人记录)的内容总高(DIP):生涯 + 9 卡 + 词云 + 时段 + 三张图各一行。
/// 2026-10-31 改版后各功能独占一行,总高超出可视区,主页从此**可滚动**。
const HOME_CONTENT_DIP: f32 = 1090.0;

/// 页签与 Slint 版对齐（`Setting/ui/main.slint` 的 `nav-items`，共 7 页）。
const PAGES: [&str; 7] = ["主页", "外观", "方案", "常规", "更新", "词库", "下载"];
const PAGE_HINTS: [&str; 7] = [
    "个人记录：击键、速度、常用字词（待迁移）",
    "配色方案：37 套出厂配色，点一下即时生效",
    "可用输入方案与切换",
    "常规设置字段（待迁移）",
    "版本与更新检查（待迁移）",
    "用户词典、词频与导入导出（待迁移）",
    "方案仓库下载（待迁移）",
];

/// 「界面风格」页的行布局（DIP）：行高、行间距。
const ROW_H: f32 = 64.0;
const ROW_GAP: f32 = 8.0;
/// 内容区顶部（DIP）：标题 74、副标题 106，列表从 138 开始。
const LIST_TOP: f32 = 138.0;
/// 「界面风格」页的页号（`PAGES` 下标）。
const PAGE_THEMES: usize = 1;
/// 「主页」（个人记录）的页号。
const PAGE_HOME: usize = 0;
/// 「常规」（表单字段）的页号。
const PAGE_SETTINGS: usize = 3;
/// 「方案选单」的页号。
const PAGE_SCHEMAS: usize = 2;
/// 「更新」的页号。
const PAGE_UPDATE: usize = 4;

/// 常规页的一行（只读预览版）。
struct FieldRow {
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

/// 把 ARGB 的 RGB 分量按比例提亮（往白里混）。
fn lighten(argb: u32, amount: f32) -> u32 {
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
fn darken(argb: u32, factor: f32) -> u32 {
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
            log("方案选单：已请 broker 重新部署");
        } else {
            log("方案选单：没找到 broker 窗口，部署请求没发出去");
        }
    }
}

/// 全局状态：UI 线程单线程使用，WndProc 直接取用（和候选窗 `Window` 同一做法）。
static mut SHELL: Option<Shell> = None;

#[allow(static_mut_refs)]
fn shell() -> Option<&'static mut Shell> {
    unsafe { (*core::ptr::addr_of_mut!(SHELL)).as_mut() }
}

struct Formats {
    title: IDWriteTextFormat,
    heading: IDWriteTextFormat,
    body: IDWriteTextFormat,
    label: IDWriteTextFormat,
    glyph: IDWriteTextFormat,
    right: IDWriteTextFormat,
    /// 居中的 16px 正文字（常用字块、柱状图标签用）。
    center: IDWriteTextFormat,
    /// 粗体小字（卡片右侧注脚，用户 15:4x 要求"加粗"）。
    note: IDWriteTextFormat,
}

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
    /// 时段统计（今天 24 小时）：`(每小时击键, 每小时有效毫秒)`；老 stat.bin 是全 0。
    hours: ([u32; 24], [u64; 24]),
    /// 主页副标题：天数 / 读文件告警。
    stats_note: String,
    /// 常规页字段（只读预览）。
    fields: Vec<FieldRow>,
    /// 常规页副标题：字段数 / 读配置告警。
    settings_note: String,
    /// 方案选单页：行（顺序 = 界面顺序，`selected` 是未落盘的内存态）。
    schemas: Vec<weasel_style::switcher::SchemaInfo>,
    /// 已折叠的系列（树结构的折叠状态）。
    collapsed: std::collections::BTreeSet<String>,
    /// 方案选单页状态行（保存/部署结果）。
    schema_status: String,
    /// 出厂数据目录与用户目录（读写 `schema/schema.yaml` 用）。
    /// 面板材质。
    material: Material,
    /// 当前皮肤（steam）的主题色：面板底、正文。
    scheme_back: u32,
    scheme_text: u32,
    scheme_hilite: u32,
    shared_dir: std::path::PathBuf,
    user_dir: std::path::PathBuf,
    /// 更新页：安装目录（自动更新功能按用户 12:5x 指示**先不做**，页面上只留一句说明）。
    update_dir: std::path::PathBuf,
    /// 列表滚动量（像素，已按 DPI 缩放）。
    scroll: f32,
    /// 主页内容实测底边(像素,paint_home 每帧回写;滚动上限用它适配任何布局改版)。
    home_content_h: std::cell::Cell<f32>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hover {
    None,
    Minimize,
    Maximize,
    Close,
    /// 左侧页签。
    Page(usize),
    /// 「界面风格」页里的配色行。
    Row(usize),
    /// 「方案选单」页的「保存并部署」按钮。
    Apply,
    /// 「外观」页的三档材质按钮（下标 0=glass 1=acrylic 2=metal）。
    Material(usize),
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
        let (w, h) = self.client_size();
        let x = 176.0 * s;
        let width = (w - x - 24.0 * s).max(80.0 * s);
        (x, 74.0 * s, width, (h - 74.0 * s - 24.0 * s).max(40.0 * s))
    }

    /// 列表可视区顶边与高度（像素）。方案选单页底部要给「保存并部署」留一条。
    fn list_viewport(&self) -> (f32, f32) {
        let s = self.scale();
        let (_, h) = self.client_size();
        let top = LIST_TOP * s;
        let bottom_reserve = if self.page == PAGE_SCHEMAS { 128.0 * s } else { 40.0 * s };
        (top, (h - top - bottom_reserve).max(40.0 * s))
    }

    /// 当前页可滚动的行数（0 = 该页没有列表）。
    fn list_len(&self) -> usize {
        match self.page {
            PAGE_THEMES => self.rows.len(),
            PAGE_SETTINGS => self.fields.len(),
            PAGE_SCHEMAS => self.schema_items().len(),
            _ => 0,
        }
    }

    /// 「外观」页顶部三档材质按钮的矩形（像素）。
    fn material_buttons(&self) -> Vec<(f32, f32, f32, f32)> {
        let s = self.scale();
        let (x, top, _, _) = self.content_area();
        let (w, h) = (96.0 * s, 30.0 * s);
        (0..3)
            .map(|index| (x + index as f32 * (w + 8.0 * s), top + 56.0 * s, w, h))
            .collect()
    }

    /// 把材质写进 `theme/material.txt`（渲染器下次画候选窗时读取）。
    fn set_material(&mut self, index: usize) {
        let names = ["glass", "acrylic", "metal"];
        let Some(name) = names.get(index) else { return };
        let path = self.user_dir.join("theme").join("material.txt");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&path, name) {
            Ok(()) => self.status = format!("候选窗材质 → {name}（打一次字生效）"),
            Err(error) => self.status = format!("写材质失败：{error}"),
        }
        self.repaint();
    }

    /// 「保存并部署」按钮矩形（像素）；只有方案选单页有。
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
        if x >= 20.0 * s && x <= 132.0 * s && y >= 76.0 * s {
            let index = ((y - 76.0 * s) / (44.0 * s)) as usize;
            if index < PAGES.len() {
                return Hover::Page(index);
            }
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
        match form::fields(section, &document) {
            Ok(fields) => {
                self.settings_note = format!(
                    "{} · {} 个字段 · 只读预览（编辑控件下一轮）{}",
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

    /// 方案页的显示项（树结构）：`(是否系列标题, 行下标, 标题文字)`。
    /// 系列按名字升序；折叠起来的系列只留标题行。
    fn schema_items(&self) -> Vec<(bool, usize, String)> {
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
    fn save_schemas(&mut self) {
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
            Err(error) => self.schema_status = format!("写入方案选单失败：{error}"),
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
        self.rows = themes.rows();
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

        // 面板本体：半透明深色玻璃（DWM 亚克力的染色在 acrylic.rs 里设成同一个色相）。
        let base = darken(self.scheme_back, 0.55);
        let panel = match self.material {
            Material::Glass => (0x34 << 24) | base,
            Material::Metal => (0xFF << 24) | darken(self.scheme_back, 0.75),
            Material::Acrylic => (0xB4 << 24) | lighten(base, 0.25),
        };
        fill_round(0.0, 0.0, w, h, 14.0 * s, panel);
        if self.material == Material::Metal {
            // 哑光金属：顶部一条高光 + 底部一条暗边 + 两道极淡横纹（拉丝感）
            fill_rect(0.0, 0.0, w, 2.0 * s, (0x66 << 24) | 0xFFFFFF);
            fill_rect(0.0, h - 2.0 * s, w, 2.0 * s, 0x66000000);
            for index in 0..2 {
                let y = h * (0.34 + index as f32 * 0.22);
                fill_rect(0.0, y, w, 1.0 * s, 0x0AFFFFFF);
            }
        }
        // 极细描边：玻璃边缘的亮线。
        // （用 1px 圆角矩形叠画的做法留给 v2 的 SDF；v1 先用内描边矩形近似。）
        fill_round(1.0 * s, 1.0 * s, w - 2.0 * s, h - 2.0 * s, 13.0 * s, 0x26FFFFFF);

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
            0xFFF2F5FA,
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
                let bg = if i == 2 { 0xD9C42B1C } else { 0x1FFFFFFF };
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
                0xFFE9EEF7,
            );
        }

        // 左侧页签。
        for (index, name) in PAGES.iter().enumerate() {
            let y = 76.0 * s + index as f32 * 44.0 * s;
            let selected = index == self.page;
            let hovered = matches!(self.hover, Hover::Page(i) if i == index);
            if selected {
                fill_round(20.0 * s, y, 112.0 * s, 40.0 * s, 9.0 * s, 0x26FFFFFF);
                fill_round(20.0 * s, y + 11.0 * s, 3.0 * s, 18.0 * s, 1.5 * s, 0xFF4C8DFF);
            } else if hovered {
                fill_round(20.0 * s, y, 112.0 * s, 40.0 * s, 9.0 * s, 0x12FFFFFF);
            }
            self.text(
                &rt,
                brush,
                name,
                &f.body,
                36.0 * s,
                y + 10.0 * s,
                180.0 * s,
                20.0 * s,
                if selected { 0xFFF2F5FA } else { 0xFFB8C1CF },
            );
        }

        // 右侧内容区：标题 + 副标题（界面风格页显示真实状态），再按页分派。
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
            0xFFF2F5FA,
        );
        let subtitle = match self.page {
            PAGE_THEMES => self.status.clone(),
            PAGE_HOME => self.stats_note.clone(),
            PAGE_SETTINGS => self.settings_note.clone(),
            PAGE_SCHEMAS => self.schema_status.clone(),
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
            if self.page == PAGE_THEMES { 0xFF9AB0D0 } else { 0xFF8A94A3 },
        );

        if self.page == PAGE_THEMES {
            self.paint_themes(&rt, brush, f, content_x, content_w, s);
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
            // 页脚固定在窗口底(不随内容滚)。
            self.text(
                &rt,
                brush,
                "灵感来源:羊毛雪花 · input_statistic 统计插件",
                &f.note,
                content_x,
                h - 34.0 * s,
                content_w,
                16.0 * s,
                0xFF9AA4B2,
            );
        } else if self.page == PAGE_SETTINGS {
            self.paint_settings(&rt, brush, f, content_x, content_w, s);
        } else if self.page == PAGE_SCHEMAS {
            self.paint_schemas(&rt, brush, f, content_x, content_w, s);
        } else if self.page == PAGE_UPDATE {
            self.paint_update(&rt, brush, f, content_x, content_w, s);
        } else {
            self.paint_placeholder(&rt, brush, f, content_x, content_top + 62.0 * s, content_w, content_h, s);
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
    /// 个人生涯（Lv/称号/境界/经验条/名著）→ 年度热力图 → 键盘热力图 → 指法分布 → 常用字 → 页脚。
    /// 我原来自己加的「9 张数字卡」「近 30 天柱状图」「码长分布」Slint 没有，已删除。
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
        let top = self.list_viewport().0 - self.scroll;
        let (_, h) = self.client_size();
        let window_h = h; // 页脚钉在真实窗口底
        let h = h + self.scroll;
        let Some(snap) = &self.stats else {
            self.text(rt, brush, "读不到统计数据", &f.body, x, top, width, 22.0 * s, 0xFFB8C1CF);
            return;
        };
        if !snap.has_data {
            self.text(rt, brush, "还没有统计数据：先打几个字，这里会自动出现", &f.body, x, top, width, 22.0 * s, 0xFFB8C1CF);
            return;
        }

        // ① 个人等级第一行(用户 2026-10-31):段位卡放最顶上,9 卡挪到下面。
        let (title, level, exp_cur, exp_span) = weasel_settings::career::career_level(snap.total_exp);
        let realm = weasel_settings::career::career_realm(snap.total_chars);
        let books = weasel_settings::career::book_level(snap.total_chars);
        let career_top = top;
        let career_h = 96.0 * s;
        draw_fill_round(rt, brush, x, career_top, width, career_h, 10.0 * s, 0x47FFFFFF);
        self.text(
            rt,
            brush,
            &format!("{title}  Lv.{level} / {}  ·  {realm}", weasel_settings::career::CAREER_MAX_LEVEL),
            &f.heading,
            x + 16.0 * s,
            career_top + 10.0 * s,
            width - 32.0 * s,
            30.0 * s,
            0xFFF7FAFF,
        );
        let bar_y = career_top + 46.0 * s;
        let bar_w = width - 32.0 * s;
        draw_fill_round(rt, brush, x + 16.0 * s, bar_y, bar_w, 8.0 * s, 4.0 * s, 0x33FFFFFF);
        let ratio = (exp_cur as f32 / exp_span.max(1) as f32).clamp(0.0, 1.0);
        draw_fill_round(rt, brush, x + 16.0 * s, bar_y, (bar_w * ratio).max(8.0 * s), 8.0 * s, 4.0 * s, 0xFF4C8DFF);
        self.text(
            rt,
            brush,
            &format!("{} / {} 经验 · 还差 {} 升下一级 · {books}", exp_cur, exp_span, exp_span.saturating_sub(exp_cur)),
            &f.note,
            x + 16.0 * s,
            bar_y + 12.0 * s,
            width - 32.0 * s,
            18.0 * s,
            0xFFE4EAF5,
        );

        // ② 个人生涯 = 标题 + 9 张统计卡
        let cards_top = career_top + career_h + 14.0 * s;
        self.text(rt, brush, "个人生涯", &f.heading, x, cards_top, 220.0 * s, 28.0 * s, 0xFFF7FAFF);
        self.text(rt, brush, "本机记录，不上传", &f.label, x + 110.0 * s, cards_top + 6.0 * s, 260.0 * s, 16.0 * s, 0xFF9AA4B2);
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
        let gap = 10.0 * s;
        let cw = (width - 2.0 * gap) / 3.0;
        let ch = 74.0 * s;
        for (index, (label, value, note)) in cards.iter().enumerate() {
            let cx = x + (index % 3) as f32 * (cw + gap);
            let cy = cards_top + 34.0 * s + (index / 3) as f32 * (ch + gap);
            self.stat_card(rt, brush, f, cx, cy, cw, ch, label, value, note, s);
        }

        // ③ 年度热力图（Slint 的「2026」格子）
        let heat_top = cards_top + 34.0 * s + 3.0 * (ch + gap) + 8.0 * s;
        let heat_h = 96.0 * s;
        draw_fill_round(rt, brush, x, heat_top, width, heat_h, 10.0 * s, 0x47FFFFFF);
        self.text(rt, brush, &format!("{} 年", snap.quarter_label), &f.label, x + 14.0 * s, heat_top + 8.0 * s, 200.0 * s, 16.0 * s, 0xFFC9D2E0);
        let cell = 7.0 * s;
        let gap = 2.0 * s;
        let cols = ((width - 28.0 * s) / (cell + gap)).floor().max(1.0) as usize;
        let peak = snap.days.iter().map(|day| day.count).max().unwrap_or(0).max(1);
        for (index, day) in snap.days.iter().enumerate() {
            if index >= 371 || day.count == 0 && !day.today {
                continue;
            }
            let col = index % cols;
            let row = index / cols;
            if row >= 7 {
                break;
            }
            let cx = x + 14.0 * s + col as f32 * (cell + gap);
            let cy = heat_top + 26.0 * s + row as f32 * (cell + gap);
            let ratio = day.count as f32 / peak as f32;
            let alpha = (0x30 as f32 + ratio * 0xCF as f32) as u32;
            draw_fill_round(rt, brush, cx, cy, cell, cell, 1.5 * s, (alpha << 24) | 0x34D399);
        }

        // ③ 键盘热力图 + 指法分布（数据来自 lib）
        let key_top = heat_top + heat_h + 14.0 * s;
        // 键高固定:之前从 (h - key_top - 150) 推导,h 含滚动偏移后键块被撑到
        // 一整屏高(用户截图"怎么这么长")。
        let key_h = 210.0 * s;
        draw_fill_round(rt, brush, x, key_top, width, key_h, 10.0 * s, 0x47FFFFFF);
        self.text(rt, brush, "键盘热力图", &f.heading, x + 14.0 * s, key_top + 8.0 * s, 200.0 * s, 26.0 * s, 0xFFF7FAFF);
        let (key_counts, _code_lens) = weasel_settings::stats::load_hist();
        let rows: [&[u32]; 4] = [
            &[0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30],
            &[0x51, 0x57, 0x45, 0x52, 0x54, 0x59, 0x55, 0x49, 0x4F, 0x50],
            &[0x41, 0x53, 0x44, 0x46, 0x47, 0x48, 0x4A, 0x4B, 0x4C],
            &[0x5A, 0x58, 0x43, 0x56, 0x42, 0x4E, 0x4D, 0x20],
        ];
        let key_peak = key_counts.iter().copied().max().unwrap_or(0).max(1) as f32;
        let area_x = x + 14.0 * s;
        let area_w = width - 28.0 * s;
        let mut ky = key_top + 40.0 * s;
        let kh = 26.0 * s;
        for row in rows.iter() {
            let count = row.len() as f32;
            let kw = ((area_w - (count - 1.0) * 2.0 * s) / count).max(6.0 * s);
            let mut kx = area_x;
            for vk in row.iter() {
                let value = key_counts.get(*vk as usize).copied().unwrap_or(0) as f32;
                let ratio = (value / key_peak).min(1.0);
                let alpha = (0x22 as f32 + ratio * 0xD8 as f32) as u32;
                draw_fill_round(rt, brush, kx, ky, kw, kh, 3.0 * s, (alpha << 24) | 0x34D399);
                kx += kw + 2.0 * s;
            }
            ky += kh + 3.0 * s;
        }
        let (hands, rows_f, names) = weasel_settings::stats::finger_breakdown(&key_counts);
        self.text(rt, brush, &format!("指法分布 · 左右手 {hands} · 三行 {rows_f}"), &f.note, area_x, ky + 4.0 * s, area_w, 18.0 * s, 0xFFE4EAF5);
        self.text(rt, brush, &names, &f.label, area_x, ky + 22.0 * s, area_w, 16.0 * s, 0xFFC9D2E0);

        // ④ 常用字（字号 = 出现次数）
        let words_top = ky + 44.0 * s;
        self.text(rt, brush, "常用字", &f.heading, x, words_top, 200.0 * s, 26.0 * s, 0xFFF7FAFF);
        let words_peak = snap.words.first().map(|(_, c)| *c).unwrap_or(1).max(1) as f32;
        let mut wx = x;
        let mut wy = words_top + 30.0 * s;
        let right = x + width;
        let mut row_h = 26.0 * s;
        for (word, count) in snap.words.iter().take(48) {
            let ratio = *count as f32 / words_peak;
            let (format, cell_w, cell_h) = if ratio > 0.6 {
                (&f.heading, 44.0 * s, 34.0 * s)
            } else if ratio > 0.3 {
                (&f.center, 30.0 * s, 28.0 * s)
            } else {
                (&f.body, 22.0 * s, 22.0 * s)
            };
            if wx + cell_w > right {
                wx = x;
                wy += row_h;
                row_h = cell_h;
            }
            if wy + cell_h > h - 40.0 * s {
                break;
            }
            self.text(rt, brush, word, format, wx, wy, cell_w, cell_h, 0xFFF7FAFF);
            wx += cell_w;
        }
        // 实测内容底边回写(滚动上限的数据源;含滚动偏移,转回内容坐标)。
        self.home_content_h.set(wy + row_h + self.scroll);

        // ⑤ 页脚
        self.text(
            rt,
            brush,
            "灵感来源：羊毛雪花 · input_statistic 统计插件",
            &f.note,
            x,
            window_h - 34.0 * s,
            width,
            16.0 * s,
            0xFF9AA4B2,
        );
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
        draw_fill_round(rt, brush, x, y, w, h, 10.0 * s, 0x47FFFFFF);
        self.text(rt, brush, label, &f.label, x + 16.0 * s, y + 10.0 * s, w - 32.0 * s, 16.0 * s, 0xFF8A94A3);
        // 大数字（白） + **灰色小字在它右边同一行**（用户 15:0x 明确要求，别放下面）
        self.text(rt, brush, value, &f.heading, x + 16.0 * s, y + 30.0 * s, w - 32.0 * s, 30.0 * s, 0xFFF2F5FA);
        let value_width = (value.chars().count() as f32 * 14.0 * s).min(w - 40.0 * s);
        self.text(
            rt,
            brush,
            note,
            &f.note,
            x + 16.0 * s + value_width + 8.0 * s,
            y + 38.0 * s,
            (w - 24.0 * s - value_width).max(40.0 * s),
            18.0 * s,
            0xFFE4EAF5,
        );
    }

    /// 「界面风格」页：真实配色列表（滚轮滚动 + 悬停 + 点选即生效）。
    fn paint_themes(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        f: &Formats,
        x: f32,
        width: f32,
        s: f32,
    ) {
        let (top, viewport_h) = self.list_viewport();
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
                0xFFB8C1CF,
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
                0x244C8DFF
            } else if hovered {
                0x1AFFFFFF
            } else {
                0x0FFFFFFF
            };
            draw_fill_round(rt, brush, rx, ry, rw, rh, 10.0 * s, bg);
            if row.active {
                draw_fill_round(rt, brush, rx, ry, 3.0 * s, rh, 1.5 * s, 0xFF4C8DFF);
            }
            // 色块 = 迷你候选窗：底色 + 选中条（高亮背）+ 文字条 + 注释条。
            let sw = 40.0 * s;
            let sx = rx + 14.0 * s;
            let sy = ry + (rh - sw) / 2.0;
            draw_fill_round(rt, brush, sx, sy, sw, sw, 8.0 * s, row.back);
            draw_fill_round(
                rt,
                brush,
                sx + 6.0 * s,
                sy + sw * 0.58,
                sw - 12.0 * s,
                sw * 0.24,
                2.0 * s,
                row.hilited_back,
            );
            draw_fill_round(
                rt,
                brush,
                sx + 6.0 * s,
                sy + sw * 0.22,
                (sw - 12.0 * s) * 0.55,
                3.0 * s,
                1.5 * s,
                row.text,
            );
            draw_fill_round(
                rt,
                brush,
                sx + 6.0 * s,
                sy + sw * 0.38,
                (sw - 12.0 * s) * 0.35,
                3.0 * s,
                1.5 * s,
                row.comment,
            );
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
                0xFFE9EEF7,
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
                0xFF8A94A3,
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
                    0xFF8A94A3,
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
        draw_fill_round(rt, brush, track_x, top, 4.0 * s, viewport_h, 2.0 * s, 0x1AFFFFFF);
        let thumb_h = (viewport_h * viewport_h / (viewport_h + max)).max(24.0 * s);
        let thumb_y = top + (viewport_h - thumb_h) * (self.scroll / max);
        draw_fill_round(rt, brush, track_x, thumb_y, 4.0 * s, thumb_h, 2.0 * s, 0x88FFFFFF);
    }

    /// 方案选单页：可勾选的方案列表 + 右下「保存并部署」。
    fn paint_schemas(
        &self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
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
                0xFFB8C1CF,
            );
        }
        for (index, (rx, ry, rw, rh)) in self.row_rects().into_iter().enumerate() {
            if ry + rh < top || ry > top + viewport_h {
                continue;
            }
            let items = self.schema_items();
            let Some((is_header, row_index, label)) = items.get(index).cloned() else { continue };
            let hovered = matches!(self.hover, Hover::Row(i) if i == index);
            if is_header {
                let arrow = if self.collapsed.contains(&self.schemas[row_index].series.clone().unwrap_or_default()) { "▸" } else { "▾" };
                draw_fill_round(rt, brush, rx, ry, rw, rh, 8.0 * s,
                    if hovered { 0x22FFFFFF } else { 0x14FFFFFF });
                self.text(rt, brush, &format!("{arrow} {label}"), &f.body,
                    rx + 16.0 * s, ry + (rh - 22.0 * s) / 2.0, rw - 32.0 * s, 22.0 * s, 0xFFF2F5FA);
                continue;
            }
            let row = &self.schemas[row_index];
            draw_fill_round(
                rt,
                brush,
                rx,
                ry,
                rw,
                rh,
                10.0 * s,
                if hovered { 0x1AFFFFFF } else { 0x0FFFFFFF },
            );
            // 勾选块（选中 = 蓝底 + ✓）。
            let (bx, by, bs) = (rx + 16.0 * s, ry + (rh - 22.0 * s) / 2.0, 22.0 * s);
            if row.selected {
                draw_fill_round(rt, brush, bx, by, bs, bs, 6.0 * s, 0xFF4C8DFF);
                self.text(rt, brush, "✓", &f.glyph, bx, by + 3.0 * s, bs, 16.0 * s, 0xFFF2F5FA);
            } else {
                draw_fill_round(rt, brush, bx, by, bs, bs, 6.0 * s, 0x22FFFFFF);
            }
            // 组内第一行：在副标题里标出系列（= 文件夹分类），不再画会重叠的顶部小字
            let first_of_group = index == 0 || self.schemas[index - 1].series != row.series;
            let series_tag = if first_of_group {
                match row.series.as_deref() {
                    Some(name) => format!("▸{name} · "),
                    None => "".to_string(),
                }
            } else {
                String::new()
            };
            self.text(
                rt,
                brush,
                &row.name,
                &f.body,
                bx + bs + 14.0 * s,
                ry + 10.0 * s,
                rw - 120.0 * s,
                20.0 * s,
                0xFFE9EEF7,
            );
            let series = row
                .series
                .as_deref()
                .map(|name| format!(" · 系列 {name}"))
                .unwrap_or_default();
            let sub = format!("{series_tag}{}", row.id);
            self.text(
                rt,
                brush,
                &sub,
                &f.label,
                bx + bs + 14.0 * s,
                ry + 34.0 * s,
                rw - 120.0 * s,
                18.0 * s,
                0xFF8A94A3,
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
                if hovered { 0xFF5D9BFF } else { 0xFF4C8DFF },
            );
            self.text(rt, brush, "保存并部署", &f.center, bx, by + 8.0 * s, bw, 18.0 * s, 0xFFF7FAFF);
        }
        // 状态行只画在按钮左侧（原来横跨整行，会和按钮/最后一行叠在一起）
        let (_, h) = self.client_size();
        self.text(
            rt,
            brush,
            &self.schema_status,
            &f.label,
            x,
            h - 78.0 * s,
            (width - 160.0 * s).max(80.0 * s),
            18.0 * s,
            0xFFE4EAF5,
        );
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
            draw_fill_round(rt, brush, x, ry, width, row_h, 10.0 * s, 0x0FFFFFFF);
            self.text(rt, brush, label, &f.body, x + 16.0 * s, ry + 18.0 * s, 120.0 * s, 20.0 * s, 0xFFE9EEF7);
            self.text(
                rt,
                brush,
                value,
                &f.right,
                x + 130.0 * s,
                ry + 18.0 * s,
                width - 150.0 * s,
                20.0 * s,
                0xFF8A94A3,
            );
        }
    }

    /// 常规页：字段只读预览（编辑控件下一轮）。值来自 lib 的 `form::fields`。
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
        if self.fields.is_empty() {
            self.text(
                rt,
                brush,
                "没有读到设置字段（原因见上方状态行）",
                &f.body,
                x,
                top + 8.0 * s,
                width,
                22.0 * s,
                0xFFB8C1CF,
            );
            return;
        }
        for (index, (rx, ry, rw, rh)) in self.row_rects().into_iter().enumerate() {
            if ry + rh < top || ry > top + viewport_h {
                continue;
            }
            let row = &self.fields[index];
            let hovered = matches!(self.hover, Hover::Row(i) if i == index);
            draw_fill_round(
                rt,
                brush,
                rx,
                ry,
                rw,
                rh,
                10.0 * s,
                if hovered { 0x1AFFFFFF } else { 0x0FFFFFFF },
            );
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
                0xFFE9EEF7,
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
                    0xFF7D8899,
                );
            }
            self.text(
                rt,
                brush,
                &row.value,
                &f.right,
                rx + rw * 0.56,
                ry + 22.0 * s,
                rw * 0.42 - 16.0 * s,
                20.0 * s,
                if row.overridden { 0xFF9AB0D0 } else { 0xFF8A94A3 },
            );
            let _ = &row.kind;
        }
        self.paint_scrollbar(rt, brush, x, width, s);
    }

    /// 未迁移页的占位卡片。
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
        draw_fill_round(rt, brush, x, y, width, card_h, 10.0 * s, 0x0FFFFFFF);
        draw_fill_round(rt, brush, x, y, 3.0 * s, card_h, 1.5 * s, 0x804C8DFF);
        self.text(
            rt,
            brush,
            "本页尚未迁移到 Direct2D 外壳",
            &f.body,
            x + 20.0 * s,
            y + 16.0 * s,
            width - 40.0 * s,
            22.0 * s,
            0xFFE9EEF7,
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
            0xFF8A94A3,
        );
        self.text(
            rt,
            brush,
            "已迁移：界面风格（配色）· 其余六页逐页来",
            &f.label,
            x + 20.0 * s,
            y + 64.0 * s,
            width - 40.0 * s,
            18.0 * s,
            0xFF6E7887,
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
            WM_MOUSEWHEEL => {
                if let Some(s) = shell() {
                    // 高 16 位是有符号的滚轮增量（120 = 一格）。
                    let delta = ((wp.0 >> 16) & 0xffff) as u16 as i16 as f32;
                    let row = (ROW_H + ROW_GAP) * s.scale();
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
                    match s.hit(x, y) {
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
                            s.page = index;
                            s.scroll = 0.0;
                            s.hover = Hover::None;
                            if index == PAGE_HOME {
                                s.refresh_stats();
                            }
                            s.repaint();
                        }
                        Hover::Row(index) => {
                            if s.page == PAGE_THEMES {
                                s.select_scheme(index);
                            } else if s.page == PAGE_SCHEMAS {
                                let items = s.schema_items();
                                match items.get(index) {
                                    Some((true, row_index, _)) => {
                                        let series = s.schemas[*row_index].series.clone().unwrap_or_default();
                                        if !s.collapsed.remove(&series) {
                                            s.collapsed.insert(series);
                                        }
                                    }
                                    Some((false, row_index, _)) => {
                                        if let Some(row) = s.schemas.get_mut(*row_index) {
                                            row.selected = !row.selected;
                                        }
                                    }
                                    None => {}
                                }
                                let count = s.schemas.iter().filter(|row| row.selected).count();
                                s.schema_status =
                                    format!("{} 个方案 · 已选 {} 个 · 点右下「保存并部署」落盘", s.schemas.len(), count);
                                s.repaint();
                            }
                        }
                        Hover::Apply => s.save_schemas(),
                        Hover::Material(index) => s.set_material(index),
                        Hover::None => {}
                    }
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

        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP_ | WS_EX_APPWINDOW_,
            class,
            w!("小狼毫Next 设置"),
            WS_POPUP_ | WS_THICKFRAME_ | WS_MINIMIZEBOX_ | WS_MAXIMIZEBOX_ | WS_SYSMENU_,
            CW_USEDEFAULT_,
            CW_USEDEFAULT_,
            npixels(WINDOW_W),
            npixels(WINDOW_H),
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        );
        if hwnd.0.is_null() {
            return Err("CreateWindowExW 失败".into());
        }
        // 材质：--material=glass|metal|acrylic（默认 glass）
        let material = std::env::args()
            .find_map(|arg| arg.strip_prefix("--material=").map(str::to_owned))
            .map(|value| match value.as_str() {
                "metal" => Material::Metal,
                "acrylic" => Material::Acrylic,
                _ => Material::Glass,
            })
            .unwrap_or(Material::Glass);
        // 截图/验收用：`--page=1` 直接开在「界面风格」页（不必拿鼠标去点）。
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
        // 系统级亚克力：与面板同色相（#1B1F27，alpha 控制染色强度）。
        acrylic::enable_acrylic(hwnd, 0x991B1F27);

        // 配色页数据：与 Slint 版同一份（`weasel_settings::themes`）。
        let shared = weasel_settings::themes::shared_dir_for_exe();
        let user = weasel_settings::themes::user_dir();
        let mut shell_state = Shell {
            hwnd,
            dpi: GetDpiForWindow(hwnd).max(96),
            presenter: None,
            formats: None,
            page: start_page,
            hover: Hover::None,
            themes: None,
            rows: Vec::new(),
            status: String::new(),
            stats: None,
            home_content_h: std::cell::Cell::new(0.0),
            hours: ([0; 24], [0; 24]),
            stats_note: String::new(),
            fields: Vec::new(),
            settings_note: String::new(),
            schemas: Vec::new(),
            collapsed: std::collections::BTreeSet::new(),
            schema_status: String::new(),
            material,
            scheme_back: 0x001B_1F27,
            scheme_text: 0x00E9_EEF7,
            scheme_hilite: 0x004C_8DFF,
            shared_dir: shared.clone(),
            user_dir: user.clone(),
            update_dir: std::path::PathBuf::new(),
            scroll: 0.0,
        };
        let themes = Themes::load(&shared, &user);
        if let Some(active) = themes.rows().iter().find(|row| row.active) {
            shell_state.scheme_back = active.back & 0x00FF_FFFF;
            shell_state.scheme_text = active.text & 0x00FF_FFFF;
            shell_state.scheme_hilite = active.hilited_back & 0x00FF_FFFF;
        }
        shell_state.status = themes.status();
        shell_state.rows = themes.rows();
        shell_state.themes = Some(themes);
        // 主页数据：与 Slint 版同一份 `weasel_settings::stats` 聚合。
        shell_state.refresh_stats();
        // 常规页字段：同样走 lib（`config::Document` + `catalog::load` + `form::fields`）。
        shell_state.refresh_settings();
        // 方案选单：与 Slint 版同一份 `weasel_style::switcher`。
        shell_state.refresh_schemas();
        // 更新页：WinSparkle 偏好（只读写偏好，不初始化检查器）。
        shell_state.refresh_update();
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
    if let Err(error) = run() {
        log(&format!("外壳启动失败：{error}"));
    }
}
