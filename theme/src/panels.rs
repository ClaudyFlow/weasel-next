//! 编码栏三按钮的弹出面板：剪贴板历史（贴）/ 符号（符）/ 同步（同）。
//!
//! 每个面板是一个自包含的 `WS_POPUP` 窗口，用 `ID2D1HwndRenderTarget` 绘制
//! （不碰候选窗的 D3D11 + DXGI + DirectComposition 管线），挂在渲染器 UI 线程上、
//! 由同一消息循环驱动。2026-11-06 把「有壳无功能」的三块接通：
//!
//! * **剪贴板**：`start_services()` 起后台线程轮询剪贴板序列号，新文本自动进
//!   `ClipboardStore`；面板支持 全部/置顶/收藏/隐藏 过滤、搜索、置顶/收藏/删除。
//! * **符号**：读同一份 `symbols.yaml`（用户目录 → 程序 `data/` → 内置兜底），
//!   201 个 Rime 输入码分组，分类标签切换 + 搜索。
//! * **同步**：`sync.json` 配置、测试连接、立即同步、编辑配置、自动间隔与接收过滤，
//!   结果直接画在面板里（不再弹 MessageBox）；下载只合并、不覆盖本地。
//!
//! 入口：`open_clipboard()` / `open_symbol()` / `open_sync()`，由 `mod.rs` 的
//! `WM_LBUTTONDOWN` 在命中 `Hit::BarButton(idx)` 时调用。
//!
//! **生命周期（2026-11-07 用户口径「面板常驻」）**：面板一旦打开就一直留着 ——
//! 点里面的卡片/符号/分类/表单不关，候选窗收起（组字结束）不关，点面板外也不关；
//! 只有点面板右上角关闭钮、或主题后端销毁（渲染器/服务退出）才消失。

// 本模块大量直接调用 D2D / Win32 的 unsafe COM 接口，且 `panel_proc` 在 `unsafe extern fn`
// 内解引用裸指针。统一允许“在 unsafe 函数体内省略内层 unsafe 块”，避免大量风格性 E0133 噪音。
#![allow(unsafe_op_in_unsafe_fn)]

use crate::d2d_bindings::*;
use windows_core::PCWSTR;
use windows_strings::w;
use weasel_common::clipboard::ClipboardStore;
use weasel_common::symbols::{self, SymbolGroup};
use weasel_common::sync;
use weasel_style::ColorScheme;

use core::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI8, AtomicIsize, Ordering};
use std::sync::{Mutex, Once};

/// 当前打开的面板对应的编码栏按钮下标（右→左 0/1/2）；-1 = 没有面板。
/// 供候选窗把「已选中」的按钮一直高亮（用户 2026-11-06：选中后一直显示）。
/// 面板销毁原因（只用于诊断）。
///
/// 用户 2026-11-07 22:3x 口径：面板常驻，**只有**关闭钮或退出服务才消失；编码栏
/// 同一按钮再点、面板内任何点击、Esc、候选窗收起都不许消失。留这个原因码是为了
/// 下次再有人报「点一下就没了」时，日志能直接说出是谁销毁的。
static DESTROY_CAUSE: AtomicI8 = AtomicI8::new(0);
/// 销毁原因：右上角关闭钮。
const DESTROY_CLOSE_BTN: i8 = 1;
/// 销毁原因：服务退出（主题后端销毁 → `close_all_panels`）。
const DESTROY_SERVICE: i8 = 2;
/// 销毁原因：面板内 Enter（模态确认/上屏）等内部路径。
const DESTROY_OTHER: i8 = 3;

static ACTIVE_BUTTON: AtomicI8 = AtomicI8::new(-1);
/// 当前面板窗口句柄（再次点同一个按钮 = 关闭，也避免叠出多个面板）。
static CURRENT_PANEL: AtomicIsize = AtomicIsize::new(0);
/// 当前面板种类（0 同步 / 1 剪贴板 / 2 符号；-1 无），用来区分「再点=关」和「换按钮=切」。
static CURRENT_KIND: AtomicI8 = AtomicI8::new(-1);
/// 每类面板各自的窗口句柄槽（0 同步 / 1 剪贴板 / 2 符号）：
/// **符号和剪贴板可以同时出现**（用户 2026-11-07），同类再点=收起。
static PANEL_SLOTS: [AtomicIsize; 3] = [
    AtomicIsize::new(0),
    AtomicIsize::new(0),
    AtomicIsize::new(0),
];
/// 处于打开状态的按钮位掩码（bit0=同步 bit1=剪贴板 bit2=符号），供编码栏高亮。
static BUTTON_BITS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// 某个编码栏按钮当前是否打开着面板。
pub fn button_active(idx: usize) -> bool {
    (idx < 3) && (BUTTON_BITS.load(Ordering::Relaxed) >> idx) & 1 == 1
}
/// 后台任务（同步 / 测试连接）回传的结果，由 `WM_SYNC_DONE` 取走。
static SYNC_RESULT: Mutex<Option<Vec<String>>> = Mutex::new(None);
/// 候选窗句柄（mod.rs 打开面板时记下）：符号点击经它送回输入流直拟上屏。
static CANDIDATE_HWND: AtomicIsize = AtomicIsize::new(0);
/// 候选字号 em（物理像素 ×4 定点），符号格子按 1.5em 见方、0.25em 内边距。
static FONT_PX4: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// 面板 → 候选窗 的直拟上屏消息（`lParam` = `Box<String>` 裸指针，接收方释放）。
pub const WM_APP_COMMIT_TEXT: i32 = 0x842C; // WM_APP + 0x2C

/// 打开面板前的前台窗口(模态输入完把焦点还给宿主应用,防止上屏文字
/// 经 IME 提交进面板自己的搜索框——用户 2026-11-07 实测)。
static PREV_FOCUS: AtomicIsize = AtomicIsize::new(0);

/// 面板拖动状态（2026-11-07 17:5x 用户「能拖动了，但是好闪」后重写）：
/// 旧实现用 WM_MOUSEMOVE 的**客户坐标差**累加窗口位置 —— 面板一移动，下一条
/// 鼠标消息的客户坐标基准就变了，再取差值会来回抖（闪的主因之一）。
/// 现在按下时记「屏幕光标 + 窗口左上」，移动时只按屏幕光标位移算位置，没有反馈环。
/// `PDRAG_X` 同时充当「是否正在拖」的哨兵：值 = 按下时的屏幕光标 X，未拖时 = `PDRAG_OFF`。
const PDRAG_OFF: i32 = i32::MIN;
/// 绑定白名单里没有 SWP_NOZORDER（生成物只有 NOACTIVATE/NOREDRAW/NOSIZE），本地补。
const SWP_NOZORDER: i32 = 0x0004;
/// 同理生成物里也没有 SWP_NOMOVE（"同类已在 → 置顶保持" 要用它：只提 z 序，不挪不改尺寸）。
const SWP_NOMOVE: i32 = 0x0002;
static PDRAG_X: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(PDRAG_OFF);
static PDRAG_Y: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// 按下瞬间的窗口左上角（屏幕坐标），拖动时以它为基准 + 光标位移。
static PANEL_L: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static PANEL_T: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// 候选窗侧登记的**当前主题配色**：面板与候选窗同源取色（用户 2026-11-07
/// 「接入 theme 的配色」）。没登记时用 `DEFAULT_PAL`（旧的写死浅色）兜底。
static THEME_SCHEME: Mutex<Option<ColorScheme>> = Mutex::new(None);

/// 打开面板前由候选窗侧登记自己的句柄 + 字号。
pub fn set_candidate_hwnd(v: isize, em: f32) {
    CANDIDATE_HWND.store(v, Ordering::SeqCst);
    FONT_PX4.store((em.max(1.0) * 4.0) as u32, Ordering::Relaxed);
}

/// 打开面板前由候选窗侧登记主题配色（`mod.rs` 三按钮点击时调）。
pub fn set_theme_scheme(scheme: &ColorScheme) {
    if let Ok(mut slot) = THEME_SCHEME.lock() {
        *slot = Some(scheme.clone());
    }
}

/// 当前面板配色：优先用候选窗登记的 theme 配色，没登记时回落旧浅色。
fn current_pal() -> Pal {
    THEME_SCHEME
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(pal_from_scheme))
        .unwrap_or(DEFAULT_PAL)
}

/// 当前 em（物理像素）；候选窗没登记过时按 20px 兜底。
fn font_px() -> f32 {
    FONT_PX4.load(Ordering::Relaxed) as f32 / 4.0
}

/// 公开给 mod.rs 的兜底复制（候选窗已收时，符号点按不丢字）。
pub fn copy_text_to_clipboard(text: &str) {
    copy_to_clipboard(text);
}

/// 当前哪个编码栏按钮处于「选中」（面板打开）状态。
pub fn active_button() -> Option<usize> {
    let v = ACTIVE_BUTTON.load(Ordering::Relaxed);
    (v >= 0).then_some(v as usize)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Sync,
    Clipboard,
    Symbol,
}

// ── 配色：面板接入 theme 的 color_scheme（用户 2026-11-07） ────────────────
// 映射口径与候选窗 `paint::squeeze_palette_for` 保持一致：底色用主题 `back_color`，
// 正文优先 `candidate_text_color`（挤压皮肤候选字色），次要文字用 `comment_text_color`，
// 强调用 `text_color`，悬停/选中底用 `hilited_candidate_back_color`；每项都带
// 亮度差 > 0.30 的可读性回落，深底配深字 / 浅底配浅字时不会隐形。
// 面板是独立 `WS_POPUP` 窗口，没有模糊底图，所以底色**强制不透明**（玻璃档也不会透桌面）。

/// 面板一整套颜色（已换算成 D2D 颜色）。
#[derive(Clone, Copy)]
struct Pal {
    /// 面板底色。
    bg: D2D_COLOR_F,
    /// 标题条 / 搜索框 / 按钮底色（底色向墨色提一档）。
    bar: D2D_COLOR_F,
    /// 正文。
    ink: D2D_COLOR_F,
    /// 次要文字（未选中标签 / 时间 / 占位提示）。
    mute: D2D_COLOR_F,
    /// 分隔线 / 边框。
    line: D2D_COLOR_F,
    /// 强调文字（选中标签 / 动作按钮文字）。
    accent: D2D_COLOR_F,
    /// 悬停行 / 选中标签底。
    hover: D2D_COLOR_F,
    /// 置顶星标。
    pin: D2D_COLOR_F,
    /// 收藏 / 删除等危险动作。
    danger: D2D_COLOR_F,
}

/// 没登记主题配色时的兜底：2026-11-07 17:39 版写死的浅色面板。
const DEFAULT_PAL: Pal = Pal {
    bg: D2D_COLOR_F { r: 0.98, g: 0.98, b: 0.99, a: 1.0 },
    bar: D2D_COLOR_F { r: 0.93, g: 0.95, b: 0.98, a: 1.0 },
    ink: D2D_COLOR_F { r: 0.12, g: 0.13, b: 0.16, a: 1.0 },
    mute: D2D_COLOR_F { r: 0.5, g: 0.52, b: 0.58, a: 1.0 },
    line: D2D_COLOR_F { r: 0.86, g: 0.88, b: 0.91, a: 1.0 },
    accent: D2D_COLOR_F { r: 0.20, g: 0.45, b: 0.85, a: 1.0 },
    hover: D2D_COLOR_F { r: 0.86, g: 0.92, b: 1.0, a: 1.0 },
    pin: D2D_COLOR_F { r: 0.95, g: 0.62, b: 0.12, a: 1.0 },
    danger: D2D_COLOR_F { r: 0.82, g: 0.25, b: 0.25, a: 1.0 },
};

/// 相对亮度（0..1），与 `paint.rs` 的可读性校验同一口径。
fn color_luma(c: u32) -> f32 {
    let r = ((c >> 16) & 0xFF) as f32;
    let g = ((c >> 8) & 0xFF) as f32;
    let b = (c & 0xFF) as f32;
    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
}

/// 两色亮度差 > 0.30 = 读得出来（与 `paint.rs` 同阈值）。
fn color_readable(a: u32, b: u32) -> bool {
    (color_luma(a) - color_luma(b)).abs() > 0.30
}

/// 强制不透明（面板不透明底，见模块注释）。
fn opaque(c: u32) -> u32 {
    0xFF00_0000 | (c & 0x00FF_FFFF)
}

/// 两色线性混色：`t = 0` 取 `a`，`t = 1` 取 `b`。输出恒不透明。
fn blend(a: u32, b: u32, t: f32) -> u32 {
    let ch = |shift: u32| -> u32 {
        let x = ((a >> shift) & 0xFF) as f32;
        let y = ((b >> shift) & 0xFF) as f32;
        ((x + (y - x) * t).round().clamp(0.0, 255.0)) as u32
    };
    0xFF00_0000 | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// 深浅底各自的中性墨色兜底。
fn neutral_on(bg: u32) -> u32 {
    if color_luma(bg) < 0.5 {
        0xFFF2_F3F5 // 深底 → 浅墨
    } else {
        0xFF1B_1D22 // 浅底 → 深墨
    }
}

/// 主题色可用（不透明且压在 `bg` 上读得出）就用它，否则回落 `fallback`。
fn pick_ink(c: u32, bg: u32, fallback: u32) -> u32 {
    if (c >> 24) >= 0x40 {
        let v = opaque(c);
        if color_readable(v, bg) {
            return v;
        }
    }
    fallback
}

/// 底色类（候选底/悬停底）：alpha ≥ 0x40 才认，否则回落 `fallback`，输出恒不透明。
fn pick_bg(c: u32, fallback: u32) -> u32 {
    if (c >> 24) >= 0x40 {
        opaque(c)
    } else {
        opaque(fallback)
    }
}

/// 两色逐通道取中值（与 `paint.rs::muted_mid` 同口径）。
fn muted_mid(fg: u32, bg: u32) -> u32 {
    let mix = |shift: u32| -> u32 { ((((fg >> shift) & 0xFF) + ((bg >> shift) & 0xFF)) / 2) & 0xFF };
    0xFF00_0000 | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// `ColorScheme` → 面板颜色。steam 实测：底 `#141617`、正文白、次要 `#A7A7A9`、
/// 强调 `#CD8C52`、悬停底 `#594231`（与候选窗同源）。
fn pal_from_scheme(scheme: &ColorScheme) -> Pal {
    let bg = opaque(scheme.back_color);
    let neutral = neutral_on(bg);
    // 正文优先用候选字色（挤压皮肤里正文就是它），主题 `text_color` 留作强调色。
    let ink = pick_ink(scheme.candidate_text_color, bg, neutral);
    let theme_ink = pick_ink(scheme.text_color, bg, ink);
    let mute = pick_ink(
        scheme.comment_text_color,
        bg,
        muted_mid(ink, bg),
    );
    let line = pick_ink(scheme.border_color, bg, blend(bg, ink, 0.35));
    // 标题条/输入框/按钮底：底色向正文色提一档（深底更亮、浅底更暗，两边都成立）。
    let bar = blend(bg, ink, 0.08);
    let hover = pick_bg(scheme.hilited_candidate_back_color, blend(bg, ink, 0.16));
    // 强调色要压在 `bar` 上读得出来，读不出来就用正文色。
    let accent = if color_readable(theme_ink, bar) { theme_ink } else { ink };
    let (pin, danger) = if color_luma(bg) < 0.5 {
        (0xFFE8_B44Eu32, 0xFFE1_5B4Bu32) // 深底：亮琥珀 / 亮红
    } else {
        (0xFFB4_740Au32, 0xFFB3_261Eu32) // 浅底：深琥珀 / 深红
    };
    let cf = crate::theme::paint::d2d_color;
    Pal {
        bg: cf(bg),
        bar: cf(bar),
        ink: cf(ink),
        mute: cf(mute),
        line: cf(line),
        accent: cf(accent),
        hover: cf(hover),
        pin: cf(pin),
        danger: cf(danger),
    }
}

// ── 停靠 / 吸附（纯函数，单测覆盖） ─────────────────────────────────────

/// 符号面板贴候选窗右侧是否放得下：**按候选窗整宽判断**（符号条宽度 = 候选窗整宽）。
///
/// `open()` 用它决定符号面板宽度（放不下 ⇒ 退成面板默认宽），`dock_point()` 用它
/// 决定落点 —— 两处必须同一口径，否则会出现「按 470 宽判断放得下」与「按整宽判断
/// 放不下」打架，面板位置和宽度对不上。
fn symbol_fits_beside((al, _, ar, _): (i32, i32, i32, i32), sw: i32) -> bool {
    ar + DOCK_GAP + (ar - al) <= sw
}

/// 面板默认停靠位置：给定候选窗屏幕矩形 `(left, top, right, bottom)`、面板尺寸与屏幕尺寸。
///
/// * **剪贴板**：贴候选窗**左侧**（右沿 = 候选窗左沿 − GAP），左边放不下翻右侧；
///   竖直优先与候选窗顶对齐（往下伸），下方放不下改与底对齐。
/// * **符号**：贴候选窗**右侧**（左沿 = 候选窗右沿 + GAP），竖直同上。
///   **右侧放不下时绝不翻到左侧**（会压到剪贴板的面板位，用户 2026-11-07 21:1x 点名否掉），
///   改为**上下停靠**：优先贴候选窗上方，上方放不下贴下方，都不行夹屏。
///   横流（`horizontal`）时上下停靠的 x 落在**同步面板右侧**（同步默认贴候选窗上方/下方、
///   左沿对齐候选窗左沿）—— 用户 2026-11-07 口述的 ④。
///   非横流时用户还没给规则（检查点未决 #3），先同样走上下，x 仍尽量贴右（夹屏）。
///   **布局沿用现有口径**（用户 2026-11-07：「三个界面的布局还是按现在的」），
///   视觉按原型 `ime-prototype.html` 改成单行横向符号条。
/// * **同步**：贴候选窗**上方**（下沿 = 候选窗上沿，用户 2026-11-07「贴紧」）；
///   上方放不下翻**下方**；上下都放不下就夹在屏幕内空间大的一侧。横向左沿对齐候选窗左沿并夹屏。
///
/// 每类面板只认自己这份矩形（拖动吸附「一一对应，不互换位置」）。
fn dock_point(
    kind: Kind,
    (al, at, ar, ab): (i32, i32, i32, i32),
    (w, h): (i32, i32),
    (sw, sh): (i32, i32),
    horizontal: bool,
) -> (i32, i32) {
    let clamp_x = |x: i32| x.clamp(0, (sw - w).max(0));
    let clamp_y = |y: i32| y.clamp(0, (sh - h).max(0));
    // 竖直：与候选窗顶对齐；下方装不下就与底对齐再夹屏。
    let v_top = |h: i32| if at + h <= sh { at } else { clamp_y(ab - h) };
    // 竖直「上下停靠」：优先贴上方；上方放不下贴下方；都不行就夹屏（空间大的一侧）。
    let v_above_below = |h: i32| {
        if at - h >= 0 {
            at - h
        } else if ab + h <= sh {
            ab
        } else if at > sh - ab {
            0
        } else {
            sh - h
        }
    };
    match kind {
        Kind::Clipboard => {
            let left = al - w - DOCK_GAP;
            let x = if left >= 0 { left } else { clamp_x(ar + DOCK_GAP) };
            (clamp_x(x), clamp_y(v_top(h)))
        }
        Kind::Symbol => {
            if symbol_fits_beside((al, at, ar, ab), sw) {
                // 常规：贴候选窗右侧，竖直与候选窗顶对齐（下方装不下则与底对齐）。
                let x = ar + DOCK_GAP;
                return (clamp_x(x), clamp_y(v_top(h)));
            }
            // 右侧放不下：不翻左。上下停靠（横流时横坐标落在同步面板右侧）。
            let y = clamp_y(v_above_below(h));
            if horizontal {
                let sync_x = clamp_x(al);
                (clamp_x(sync_x + SYNC_W + DOCK_GAP), y)
            } else {
                (clamp_x(ar + DOCK_GAP), y)
            }
        }
        Kind::Sync => (clamp_x(al), clamp_y(v_above_below(h))),
    }
}

/// 拖动吸附：**只看本面板自己的停靠矩形**（其它类的矩形不参与，绝不互换位置）。
/// 判据 = 面板几何中心进入该矩形（含边界）⇒ 返回停靠左上角；否则返回 `None`（自由拖动）。
fn snap_point(
    kind: Kind,
    anchor: (i32, i32, i32, i32),
    (w, h): (i32, i32),
    (sw, sh): (i32, i32),
    (cx, cy): (i32, i32),
    horizontal: bool,
) -> Option<(i32, i32)> {
    let (dx, dy) = dock_point(kind, anchor, (w, h), (sw, sh), horizontal);
    // 2× 比较避免除法，中心落在矩形边界上也算进入。
    let inside = 2 * cx >= 2 * dx
        && 2 * cx <= 2 * (dx + w)
        && 2 * cy >= 2 * dy
        && 2 * cy <= 2 * (dy + h);
    inside.then_some((dx, dy))
}

/// 拖动时实时取「当前候选框」屏幕矩形；候选窗句柄失效才回落开面板时的锚点。
fn live_anchor(fallback: Option<(i32, i32, i32, i32)>) -> Option<(i32, i32, i32, i32)> {
    let h = CANDIDATE_HWND.load(Ordering::SeqCst);
    if h != 0 {
        let cand = h as *mut c_void;
        if unsafe { raw::IsWindow(cand) } != 0 {
            let mut r = [0i32; 4];
            if unsafe { raw::GetWindowRect(cand, r.as_mut_ptr()) } != 0 {
                return Some((r[0], r[1], r[2], r[3]));
            }
        }
    }
    fallback
}

#[derive(Clone)]
struct ClipRow {
    /// 原始文本（加密条目在绘制时打码，见 `clip_display`）。
    text: String,
    /// 真实文本，复制 / 操作时用。
    raw: String,
    time: String,
    pin: bool,
    fav: bool,
    hid: bool,
    lock: bool,
    /// 本次已解锁(输对密码后置位,切 tab 保持)。
    unlock: bool,
    /// 云端已备份标记（卡片 ☁ 钮）。
    synced: bool,
    /// 加密密码（面板解锁比对用；未加密为 None）。
    pass: Option<String>,
}

/// 剪贴板卡片上「加密/解锁」模态层要做的事。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModalPurpose {
    /// 给条目设置密码（加密）。
    SetLock,
    /// 输密码解锁并**解除加密**（密码清掉）。
    UnlockToggle,
    /// 输密码解锁后复制到系统剪贴板。
    UnlockCopy,
    /// 输密码解锁后直接上屏（点卡片本体）。
    UnlockCommit,
}

/// 面板内的密码输入层（不是新窗口：画在面板上，模态拦截点击）。
struct ClipModal {
    purpose: ModalPurpose,
    /// 目标条目的真实文本。
    text: String,
    /// 密码输入缓冲（UTF-16，同 WM_CHAR）。
    input: Vec<u16>,
    /// 错误提示（「密码不能为空」/「密码不对」）。
    error: String,
}

/// 符号面板的一个分类标签。
///
/// `symbols.yaml` 里同一条分类注释会覆盖其后**多个**输入码（`/fh`、`/dn` …），
/// 原型 v3 的标签条是「一个分类一个标签」，所以这里按分类名聚合，避免出现
/// 「符號 符號 象棋 象棋」这种重复标签。
struct SymTab {
    /// 标签短名（`SymbolGroup::title()`，≤4 字）。
    title: String,
    /// 标签前的图标字（取该分类第一个符号）。
    icon: String,
    /// 合并后的符号（保持文件顺序、去重）。
    items: Vec<String>,
    /// 合并进来的输入码（`/fh`、`/dn` …），搜索时可匹配「不带斜杠的编码」。
    codes: Vec<String>,
}

/// 把 `symbols.yaml` 的分组按分类名聚合成标签条数据。
fn build_sym_tabs(groups: &[SymbolGroup]) -> Vec<SymTab> {
    let mut out: Vec<SymTab> = Vec::new();
    for g in groups {
        let title = g.title();
        if let Some(t) = out.iter_mut().find(|t| t.title == title) {
            for item in &g.items {
                if !t.items.iter().any(|x| x == item) {
                    t.items.push(item.clone());
                }
            }
            if !t.codes.iter().any(|c| c == &g.code) {
                t.codes.push(g.code.clone());
            }
        } else {
            out.push(SymTab {
                title,
                icon: g.items.first().cloned().unwrap_or_default(),
                items: g.items.clone(),
                codes: vec![g.code.clone()],
            });
        }
    }
    out
}

/// 面板内的键盘焦点（2026-11-07 三面板：两个搜索框 + 同步表单四个文本域）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    None,
    ClipSearch,
    SymSearch,
    Sync(SyncField),
}

/// 同步表单里可编辑的四个文本域。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SyncField {
    Server,
    User,
    Pass,
    Path,
}

impl SyncField {
    fn value<'a>(self, cfg: &'a sync::SyncConfig) -> &'a str {
        match self {
            SyncField::Server => &cfg.server,
            SyncField::User => &cfg.user,
            SyncField::Pass => &cfg.pass,
            SyncField::Path => &cfg.remote_path,
        }
    }
    fn value_mut<'a>(self, cfg: &'a mut sync::SyncConfig) -> &'a mut String {
        match self {
            SyncField::Server => &mut cfg.server,
            SyncField::User => &mut cfg.user,
            SyncField::Pass => &mut cfg.pass,
            SyncField::Path => &mut cfg.remote_path,
        }
    }
    fn placeholder(self) -> &'static str {
        match self {
            SyncField::Server => "https://dav.jianguoyun.com/dav/",
            SyncField::User => "you@jianguoyun.com",
            SyncField::Pass => "坚果云应用专用密码",
            SyncField::Path => "/Rime/user_dict",
        }
    }
}

/// 同步表单一行是什么控件（2026-11-07 用户「同步面板要可编辑表单+开关」）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SyncRowKind {
    /// 自动同步开关（auto）。
    ToggleAuto,
    /// 剪贴板页的剪贴板同步开关（receive 含 clipboard）。
    ToggleClipboard,
    /// 同步方式，点击循环（坚果云 ⇄ 通用 WebDAV）。
    CycleMode,
    /// 文本域（服务器 / 账号 / 应用密码 / 词库路径）。
    Text(SyncField),
    /// 同步频率，点击循环（仅手动/30 分钟/每小时/每天）。
    CycleInterval,
    /// 接收过滤：两个勾选框（剪贴板 / 用户词库）。
    ReceiveChecks,
    /// 只读信息行（已备份条数 / 上次同步）。
    Info,
}

struct SyncRowSpec {
    label: &'static str,
    kind: SyncRowKind,
}

/// 用户词库页：8 行表单（对齐原型字段，去掉后端不支持的「同步内容/条数上限/图片处理」）。
const SYNC_USER_ROWS: [SyncRowSpec; 8] = [
    SyncRowSpec { label: "自动同步", kind: SyncRowKind::ToggleAuto },
    SyncRowSpec { label: "同步方式", kind: SyncRowKind::CycleMode },
    SyncRowSpec { label: "服务器", kind: SyncRowKind::Text(SyncField::Server) },
    SyncRowSpec { label: "账号", kind: SyncRowKind::Text(SyncField::User) },
    SyncRowSpec { label: "应用密码", kind: SyncRowKind::Text(SyncField::Pass) },
    SyncRowSpec { label: "词库路径", kind: SyncRowKind::Text(SyncField::Path) },
    SyncRowSpec { label: "同步频率", kind: SyncRowKind::CycleInterval },
    SyncRowSpec { label: "接收过滤", kind: SyncRowKind::ReceiveChecks },
];
/// 剪贴板页：同步开关 + 已备份统计 + 上次同步。
const SYNC_CLIP_ROWS: [SyncRowSpec; 3] = [
    SyncRowSpec { label: "剪贴板同步", kind: SyncRowKind::ToggleClipboard },
    SyncRowSpec { label: "已备份", kind: SyncRowKind::Info },
    SyncRowSpec { label: "上次同步", kind: SyncRowKind::Info },
];

fn sync_rows(p: &Panel) -> &'static [SyncRowSpec] {
    if p.sync_tab == 1 { &SYNC_CLIP_ROWS } else { &SYNC_USER_ROWS }
}

/// 同步表单行高（比只读状态行高，容纳输入框）。
const SYNC_FORM_ROW_H: f32 = 34.0;
/// 表单左侧 label 宽。
const SYNC_LABEL_W: f32 = 92.0;
/// 开关尺寸（原型 42×24）。
const SYNC_SWITCH_W: f32 = 42.0;
const SYNC_SWITCH_H: f32 = 24.0;
/// 进度条动画定时器（WM_TIMER）id 与间隔。
const SYNC_TICK_ID: usize = 1;
const SYNC_TICK_MS: u32 = 60;

/// 同步方式循环：坚果云 ⇄ 通用 WebDAV（切到通用时清空服务器，让用户填自己的）。
fn cycle_sync_mode(server: &mut String) {
    if server.contains("jianguoyun") {
        server.clear();
    } else {
        *server = "https://dav.jianguoyun.com/dav/".to_string();
    }
}

fn sync_mode_label(server: &str) -> &'static str {
    if server.contains("jianguoyun") { "坚果云 WebDAV" } else { "通用 WebDAV" }
}

/// 同步频率循环：仅手动 → 30 分钟 → 每小时 → 每天 → 仅手动。
fn cycle_interval(cur: u32) -> u32 {
    const STEPS: [u32; 4] = [0, 30, 60, 1440];
    let idx = STEPS.iter().position(|&x| x == cur).unwrap_or(0);
    STEPS[(idx + 1) % STEPS.len()]
}

fn interval_label(min: u32) -> String {
    match min {
        0 => "仅手动".into(),
        30 => "30 分钟".into(),
        60 => "每小时".into(),
        1440 => "每天".into(),
        n => format!("{n} 分钟"),
    }
}

/// 接收过滤：加/删某类。顺序固定 clipboard 在前、userdb 在后（与默认值一致）。
fn set_receive(receive: &mut Vec<String>, what: &str, on: bool) {
    receive.retain(|s| !s.eq_ignore_ascii_case(what));
    if on {
        if what.eq_ignore_ascii_case("clipboard") {
            receive.insert(0, what.to_string());
        } else {
            receive.push(what.to_string());
        }
    }
}

/// 剪贴板同步统计：(已打同步标记条数, 总条数)。
fn clip_stat() -> (usize, usize) {
    let store = ClipboardStore::load(&user_root());
    let total = store.entries().len();
    let synced = store.entries().iter().filter(|e| e.synced).count();
    (synced, total)
}

/// 同步表单一行顶部的 y 坐标（含滚动）。
fn sync_row_y(p: &Panel, row: usize) -> f32 {
    content_top(p) + 8.0 + row as f32 * SYNC_FORM_ROW_H - p.scroll
}

/// 表单值区的左右边界。
fn sync_value_rect(p: &Panel) -> (f32, f32) {
    (p.pad + SYNC_LABEL_W, p.width as f32 - p.pad)
}

/// 接收过滤两个勾选框的矩形：(剪贴板, 用户词库)。
fn receive_check_boxes(p: &Panel, y: f32) -> ((f32, f32, f32, f32), (f32, f32, f32, f32)) {
    let (vl, vr) = sync_value_rect(p);
    let half = (vr - vl - 12.0) / 2.0;
    let top = y + (SYNC_FORM_ROW_H - 18.0) / 2.0;
    ((vl, top, half, 18.0), (vl + half + 12.0, top, half, 18.0))
}

struct Panel {
    hwnd: HWND,
    rt: Option<ID2D1HwndRenderTarget>,
    factory: ID2D1Factory1,
    write: IDWriteFactory,
    brush: Option<ID2D1SolidColorBrush>,
    text_format: Option<IDWriteTextFormat>,
    small_format: Option<IDWriteTextFormat>,
    /// 右对齐的小字格式（标题行右侧的提示：symbols.yaml / 同步状态）。
    right_format: Option<IDWriteTextFormat>,
    /// 符号格子的居中格式（水平+垂直都居中；字号 1em，见 `SYM_FONT_EM`）。
    center_format: Option<IDWriteTextFormat>,
    kind: Kind,
    clips: Vec<ClipRow>,
    /// 0 全部 / 1 置顶 / 2 收藏 / 3 隐藏
    clip_filter: usize,
    /// 标题行右侧的剪贴板同步状态（打开时算一次，避免每帧读盘）。
    clip_hint: String,
    /// 符号分类标签（按分类名聚合后）。
    sym_tabs: Vec<SymTab>,
    sym_group: usize,
    /// 符号面板搜索框（2026-11-07 用户点名要）。
    sym_search: Vec<u16>,
    /// 符号条横向滚动（物理像素）。
    scroll_x: f32,
    /// 符号分类标签分页（每页 5 个）
    tab_page: usize,
    /// 同步面板当前标签页（0 用户词库 / 1 剪贴板）。
    sync_tab: usize,
    /// 同步面板的可编辑表单（打开时从 sync.json 读；保存/同步时写回）。
    sync_form: sync::SyncConfig,
    /// 表单内容是否已保存（标题栏显示「有未保存改动」）。
    sync_saved: bool,
    /// 剪贴板同步统计：(已打同步标记条数, 总条数)。
    clip_stat: (usize, usize),
    /// 上次同步时间（sync_state.json，打开时读一次）。
    sync_last: String,
    sync_lines: Vec<String>,
    sync_busy: bool,
    /// 当前键盘焦点（两个搜索框 / 同步表单文本域）。
    focus: Focus,
    /// 进度条动画相位（WM_TIMER 每 60ms +1；不确定进度动画）。
    sync_phase: u32,
    /// 进度条定时器是否已启动。
    sync_timer: bool,
    pad: f32,
    title_h: f32,
    /// 当前 em（物理像素 = 候选字号 × dpi/72）；符号格的宽高都按它换算。
    em: f32,
    /// 主题是不是横流（`LayoutType::HorizontalSqueeze`）：右侧放不下时符号条改贴同步面板右侧。
    horizontal: bool,
    scroll: f32,
    hover: Option<Hit>,
    search: Vec<u16>,
    /// 本次面板会话里已解锁的剪贴板条目（按真实文本记；关面板即忘）。
    unlocked: std::collections::HashSet<String>,
    /// 剪贴板密码模态层（加密/解锁时出现；None = 无）。
    modal: Option<ClipModal>,
    width: i32,
    height: i32,
    /// 本面板一套 theme 颜色（打开时按当前配色算好，绘制只读它）。
    pal: Pal,
    /// 开面板时的候选窗矩形：候选窗句柄失效时作吸附/停靠的回落锚点。
    anchor: Option<(i32, i32, i32, i32)>,
}

const CLASS: PCWSTR = w!("Weasel.NextTheme.Panel");
const TITLE_FONT: f32 = 15.0;
const BODY_FONT: f32 = 14.0;

const TAB_H: f32 = 30.0;
const SEARCH_H: f32 = 30.0;
const ARROW_W: f32 = 22.0;
const CLOSE_W: f32 = 26.0;

// ── 原型 v3（`_shots/ime-prototype.html`）的布局常量，单位物理像素 ─────────
/// 标签之间的缝。
const TAB_GAP: f32 = 4.0;
/// 符号格之间的缝。
const SYM_GAP: f32 = 6.0;
/// 面板与候选窗之间的缝（同步为 0 = 贴紧）。
const DOCK_GAP: i32 = 8;
/// 同步面板默认尺寸（符号横流兜底要算「同步面板右侧」的 x，所以提成常量）。
const SYNC_W: i32 = 460;
const SYNC_H: i32 = 430;
// ── 符号格尺寸（用户 2026-11-07 21:2x 明确口径）────────────────────────────
// 「都是 1.5em，padding .25，字 1em」：格子 1.5em 见方（宽随字形加宽）、内边距 0.25em、
// 字形字号 1em —— 表情与文本符号同一套，表情只多开彩色字体。
/// 符号格边长倍数。
const SYM_CELL_EM: f32 = 1.5;
/// 符号字形字号倍数。
const SYM_FONT_EM: f32 = 1.0;
/// 符号格最小边长（物理像素；em 很小时兜底）。
const SYM_MIN_CELL: f32 = 28.0;
/// 窄字符（ASCII / 箭头等）的宽度倍数（只用估算格子宽，不参与文本渲染）。
const SYM_NARROW_EM: f32 = 0.62;
/// 标签底 / 卡片 / 按钮 / 输入框的圆角。
const BOX_R: f32 = 8.0;
/// 剪贴板卡片高度（正文两行 + 元信息 + 动作行）。
const CARD_H: f32 = 92.0;
/// 同步面板键值行高。
const SYNC_ROW_H: f32 = 28.0;
/// 同步面板底部动作条高度。
const SYNC_FOOT_H: f32 = 52.0;
const WM_SYNC_DONE: i32 = WM_APP + 0x51;
const VK_RETURN: usize = 0x0D;
/// 面板内文本域的粘贴 / 全清（2026-11-07 22:0x 用户「密码和用户帐号粘贴不了」）：
/// 面板是自绘的，没有编辑控件，`Ctrl+V` / `WM_PASTE` 必须自己接。
const VK_CONTROL: i32 = 0x11;
const VK_V: usize = 0x56;
const VK_A: usize = 0x41;
const WM_PASTE: i32 = 0x0302;
/// 外部请求关闭（Alt+F4 / 系统）。面板常驻：不理会，只有关闭钮/退出服务才关。
const WM_CLOSE: i32 = 0x0010;
/// 密码模态框最多接受的码元数。
const MODAL_MAX_CHARS: usize = 64;

/// 少量绑定白名单里没有的 Win32 原语，直接声明，避免为它们重跑 bindgen。
/// 放在独立模块里，名字不会和 `d2d_bindings::*` 的 glob 导入冲突。
mod raw {
    use core::ffi::c_void;
    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn GetClipboardSequenceNumber() -> u32;
        pub fn IsClipboardFormatAvailable(fmt: u32) -> i32;
        pub fn GetClipboardData(fmt: u32) -> *mut c_void;
        pub fn SetForegroundWindow(h: *mut c_void) -> i32;
        pub fn GetKeyState(n_virt_key: i32) -> i16;
        pub fn ValidateRect(h: *mut c_void, r: *const c_void) -> i32;
        pub fn PostMessageW(h: *mut c_void, msg: u32, w: usize, l: isize) -> i32;
        pub fn GetWindowRect(h: *mut c_void, r: *mut i32) -> i32; // [left,top,right,bottom]
        pub fn SetCapture(h: *mut c_void) -> *mut c_void;
        pub fn ReleaseCapture() -> i32;
        pub fn GetForegroundWindow() -> *mut c_void;
        pub fn SetFocus(h: *mut c_void) -> *mut c_void;
        pub fn IsWindow(h: *mut c_void) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GlobalLock(h: *mut c_void) -> *mut c_void;
        pub fn GlobalUnlock(h: *mut c_void) -> i32;
        pub fn GlobalSize(h: *mut c_void) -> usize;
    }
}

fn user_root() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Rime-Next")
}

/// 程序数据目录（`<exe 所在目录>/data`），`symbols.yaml` 在这里。
fn shared_data() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("data")))
}

/// 把文本写入系统剪贴板（Unicode）。失败仅记录，不影响关闭面板。
fn copy_to_clipboard(text: &str) {
    unsafe {
        if !OpenClipboard(None).as_bool() {
            return;
        }
        let _ = EmptyClipboard();
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let h: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE as u32, utf16.len() * 2);
        if !h.0.is_null() {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(utf16.as_ptr(), p, utf16.len());
                let _ = GlobalUnlock(h);
                let _ = SetClipboardData(CF_UNICODETEXT as u32, Some(h));
            }
        }
        let _ = CloseClipboard();
    }
}

/// 读系统剪贴板里的 Unicode 文本（后台采集线程用）。
fn read_clipboard_text() -> Option<String> {
    unsafe {
        if raw::IsClipboardFormatAvailable(CF_UNICODETEXT as u32) == 0 {
            return None;
        }
        if !OpenClipboard(None).as_bool() {
            return None;
        }
        let h = raw::GetClipboardData(CF_UNICODETEXT as u32);
        let out = if h.is_null() {
            None
        } else {
            let p = raw::GlobalLock(h) as *const u16;
            if p.is_null() {
                None
            } else {
                let max = (raw::GlobalSize(h) / 2).min(1 << 20);
                let mut len = 0usize;
                while len < max && *p.add(len) != 0 {
                    len += 1;
                }
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
                raw::GlobalUnlock(h);
                Some(text)
            }
        };
        let _ = CloseClipboard();
        out
    }
}

/// 可编辑文本域（面板没有控件，粘贴 / 全清共用这一套焦点解析）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditTarget {
    /// 密码模态层（设置/解锁加密条目）。
    Modal,
    /// 同步表单四个文本域之一。
    Sync(SyncField),
    /// 剪贴板面板搜索框。
    ClipSearch,
    /// 符号面板搜索框。
    SymSearch,
}

/// 当前焦点落在哪个可编辑文本域。剪贴板沿旧行为「一开就能打字」，没点搜索框也算。
fn edit_target(p: &Panel) -> Option<EditTarget> {
    if p.modal.is_some() {
        return Some(EditTarget::Modal);
    }
    match p.focus {
        Focus::Sync(field) => Some(EditTarget::Sync(field)),
        Focus::ClipSearch => Some(EditTarget::ClipSearch),
        Focus::SymSearch => Some(EditTarget::SymSearch),
        Focus::None => match p.kind {
            Kind::Clipboard => Some(EditTarget::ClipSearch),
            Kind::Symbol => Some(EditTarget::SymSearch),
            Kind::Sync => None,
        },
    }
}

fn ctrl_down() -> bool {
    unsafe { raw::GetKeyState(VK_CONTROL) < 0 }
}

/// 字段允许的最大字符数（同步表单与搜索框；密码框按码元截断）。
fn target_max_chars(target: EditTarget) -> usize {
    match target {
        EditTarget::Modal => MODAL_MAX_CHARS,
        EditTarget::Sync(SyncField::Pass) => 32,
        EditTarget::Sync(_) => 40,
        EditTarget::ClipSearch | EditTarget::SymSearch => 40,
    }
}

fn target_label(target: EditTarget) -> &'static str {
    match target {
        EditTarget::Modal => "密码模态",
        EditTarget::Sync(SyncField::Server) => "同步·服务器",
        EditTarget::Sync(SyncField::User) => "同步·账号",
        EditTarget::Sync(SyncField::Pass) => "同步·应用密码",
        EditTarget::Sync(SyncField::Path) => "同步·词库路径",
        EditTarget::ClipSearch => "剪贴板搜索",
        EditTarget::SymSearch => "符号搜索",
    }
}

/// 粘贴文本清洗：去掉控制字符（换行/制表）、去掉首尾空白、按字段上限截断。
///
/// 单独成函数是为了能单测——`paste_into_focused` 里的剪贴板读取和窗口无效化没法在
/// 单测里跑，但这段「剪贴板原文 → 字段值」的转换必须有人盯。
fn sanitized_paste_text(raw: &str, max: usize) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(max)
        .collect()
}

/// 把系统剪贴板文本**整段替换**进当前文本域（`Ctrl+V` / `WM_PASTE`）。
///
/// 面板是自绘的，没有光标也没有选区——字段本来就只有「整串」这一个概念，
/// 粘贴按「替换整个字段」处理，正好符合「把账号 / 应用密码粘进去」的用法。
/// 返回是否真的改动了内容。
fn paste_into_focused(p: &mut Panel) -> bool {
    let Some(target) = edit_target(p) else { return false };
    let Some(text) = read_clipboard_text() else { return false };
    let value = sanitized_paste_text(&text, target_max_chars(target));
    if value.is_empty() {
        return false;
    }
    match target {
        EditTarget::Modal => {
            if let Some(modal) = p.modal.as_mut() {
                modal.input = value.encode_utf16().collect();
                modal.error.clear();
            }
        }
        EditTarget::Sync(field) => {
            *field.value_mut(&mut p.sync_form) = value.clone();
            p.sync_saved = false;
        }
        EditTarget::ClipSearch => {
            p.search = value.encode_utf16().collect();
            p.scroll = 0.0;
        }
        EditTarget::SymSearch => {
            p.sym_search = value.encode_utf16().collect();
            p.scroll_x = 0.0;
            fit_symbol_height(p);
        }
    }
    crate::diagnostics::record(format_args!(
        "[panel] 粘贴 {} 字符 → {}",
        value.chars().count(),
        target_label(target)
    ));
    let _ = unsafe { InvalidateRect(Some(p.hwnd), None, false) };
    true
}

/// 清空当前文本域（`Ctrl+A`；没有选区，等价于「全选后删除」，方便重填账号）。
fn clear_focused_text(p: &mut Panel) -> bool {
    let Some(target) = edit_target(p) else { return false };
    match target {
        EditTarget::Modal => {
            if let Some(modal) = p.modal.as_mut() {
                modal.input.clear();
                modal.error.clear();
            }
        }
        EditTarget::Sync(field) => {
            field.value_mut(&mut p.sync_form).clear();
            p.sync_saved = false;
        }
        EditTarget::ClipSearch => {
            p.search.clear();
            p.scroll = 0.0;
        }
        EditTarget::SymSearch => {
            p.sym_search.clear();
            p.scroll_x = 0.0;
            fit_symbol_height(p);
        }
    }
    let _ = unsafe { InvalidateRect(Some(p.hwnd), None, false) };
    true
}

/// 渲染器启动时调用一次：起剪贴板采集 + 按间隔自动同步。
pub fn start_services() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // 主题 DLL 可能被宿主进程也 LoadLibrary 一份（Engine 侧只查询能力、不画界面）。
        // 采集与自动同步只允许在渲染器进程里跑，否则两个进程同时写 clipboard.json。
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()))
            .unwrap_or_default();
        if !exe.contains("renderer") {
            crate::diagnostics::record(format_args!("[panel] skip services in {exe}"));
            return;
        }
        let root = user_root();
        let _ = std::thread::Builder::new()
            .name("rime-clipboard-monitor".into())
            .spawn(move || {
                let mut last = unsafe { raw::GetClipboardSequenceNumber() };
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let seq = unsafe { raw::GetClipboardSequenceNumber() };
                    if seq == last {
                        continue;
                    }
                    last = seq;
                    let Some(raw_text) = read_clipboard_text() else {
                        continue;
                    };
                    let text = raw_text.trim().to_string();
                    // 空串 / 超长（图片、文件列表等）不入历史；面板自己复制的符号
                    // 和剪贴板条目也会进历史，这正是想要的行为（去重后自动置顶）。
                    if text.is_empty() || text.chars().count() > 4096 {
                        continue;
                    }
                    let mut store = ClipboardStore::load(&root);
                    store.push(&text);
                }
            });
        sync::start_auto(user_root());
    });
}

fn load_clips() -> Vec<ClipRow> {
    let store = ClipboardStore::load(&user_root());
    store
        .entries()
        .iter()
        .map(|e| ClipRow {
            text: e.text.clone(),
            raw: e.text.clone(),
            time: e.time.clone(),
            pin: e.pin,
            fav: e.fav,
            hid: e.hid,
            lock: e.lock,
            unlock: e.unlock,
            synced: e.synced,
            pass: e.pass.clone(),
        })
        .collect()
}

/// 卡片显示文本：加密且本次会话未解锁 ⇒ 打码；否则显示原文。
fn clip_mask_text(lock: bool, unlocked: bool, text: &str) -> String {
    if lock && !unlocked {
        "🔒 已加密".into()
    } else {
        text.to_string()
    }
}

fn clip_display(p: &Panel, r: &ClipRow) -> String {
    clip_mask_text(r.lock, p.unlocked.contains(&r.raw) || r.unlock, &r.text)
}

fn search_text(p: &Panel) -> String {
    String::from_utf16_lossy(&p.search).trim().to_lowercase()
}

fn visible_clips(p: &Panel) -> Vec<ClipRow> {
    let q = search_text(p);
    p.clips
        .iter()
        .filter(|r| {
            if r.hid && p.clip_filter != 3 {
                return false;
            }
            let by_filter = match p.clip_filter {
                1 => r.pin,
                2 => r.fav,
                3 => r.hid,
                _ => !r.hid,
            };
            // 加密条目照常显示（打码，见 `clip_display`）；搜索只匹配当前显示文本，
            // 未解锁时不会把密文内容匹配出来。
            by_filter && (q.is_empty() || clip_display(p, r).to_lowercase().contains(&q))
        })
        .cloned()
        .collect()
}

/// 当前符号分类的符号列表（单行横滚条的数据）。
///
/// 搜索框有内容时**跨全部分类过滤**（2026-11-07 用户点名要符号搜索）：
/// 匹配符号字符本身、分类短名、以及输入码（去掉 `/`）；清空查询回到当前分类。
fn visible_syms(p: &Panel) -> Vec<String> {
    let q = String::from_utf16_lossy(&p.sym_search).trim().to_lowercase();
    if q.is_empty() {
        return p.sym_tabs.get(p.sym_group).map(|t| t.items.clone()).unwrap_or_default();
    }
    let mut out: Vec<String> = Vec::new();
    for t in &p.sym_tabs {
        let by_tab = sym_title_matches(&t.title, &t.codes, &q);
        for item in &t.items {
            if (by_tab || item.to_lowercase().contains(&q)) && !out.iter().any(|x| x == item) {
                out.push(item.clone());
            }
        }
    }
    out
}

// ── 符号格自适应几何（绘制 / 命中 / 滚动 / 面板高度四处共用）────────────────

/// 是不是「彩色表情」字符。
///
/// 取值域主区（U+1F000..=U+1FAFF）一定按彩色表情画；文本区符号（☀…➿）只有带
/// U+FE0F 变体选择符才算 —— 「★」「✦」「■」这些是普通文本符号，不该按表情放大。
fn is_emoji_char(c: char, has_vs16: bool) -> bool {
    let u = c as u32;
    if (0x1F000..=0x1FAFF).contains(&u) || (0x1FC00..=0x1FFFD).contains(&u) {
        return true;
    }
    has_vs16 && ((0x2600..=0x27BF).contains(&u) || (0x2B00..=0x2BFF).contains(&u))
}

/// 单字符是否全角/宽字符（估格子宽用；只影响面板排布，不影响文本渲染）。
fn is_wide_char(c: char) -> bool {
    matches!(
        c as u32,
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x20000..=0x3FFFD
    )
}

/// 单个符号格的尺寸 `(宽, 高)`，单位物理像素。
///
/// 用户 2026-11-07 21:2x 明确口径「都是 1.5em，padding .25，字 1em」：
/// 格子 `1.5em`（高），宽按字形累加但不小于 1.5em；表情与文本符号**同一套格子**，
/// 表情只多开彩色字体（见 `label_color`）。
fn sym_cell_size(s: &str, em: f32, pad: f32) -> (f32, f32) {
    let cell = (em * SYM_CELL_EM).max(SYM_MIN_CELL);
    let has_vs16 = s.chars().any(|c| c == '\u{FE0F}');
    if s.chars().any(|c| is_emoji_char(c, has_vs16)) {
        return (cell, cell);
    }
    // 宽度按字形累加（多字符符号如「」自动加宽），不低于 1.5em。
    let adv: f32 = s
        .chars()
        .map(|c| if is_wide_char(c) { em } else { em * SYM_NARROW_EM })
        .sum();
    ((adv + pad * 2.0).max(cell), cell)
}

/// 符号条的整条几何：每项 `(x, y, 宽, 高)`，`x` 已含左内边距与横向滚动量。
/// 绘制 / 命中 / 横向滚动 / 面板高度全部从这里取，避免「画的和点的不一致」。
fn sym_geometry_for(items: &[String], p: &Panel) -> Vec<(f32, f32, f32, f32)> {
    let mut x = p.pad - p.scroll_x;
    let y = content_top(p) + SYM_GAP;
    let mut out = Vec::with_capacity(items.len());
    for s in items {
        let (cw, ch) = sym_cell_size(s, p.em, p.pad);
        out.push((x, y, cw, ch));
        x += cw + SYM_GAP;
    }
    out
}

/// 符号条所需的最大格高（决定符号面板高度）。
fn sym_strip_height(items: &[String], em: f32, pad: f32) -> f32 {
    items
        .iter()
        .map(|s| sym_cell_size(s, em, pad).1)
        .fold((em * 1.5).max(SYM_MIN_CELL), f32::max)
}

/// 符号面板高度 = 标题行 + 分类标签 + 搜索框 + 符号条 + 上下留白。
fn symbol_panel_height(strip_h: f32) -> f32 {
    34.0 + TAB_H + SEARCH_H + strip_h + SYM_GAP * 2.0 + 12.0
}

/// 搜索查询是否命中某个分类（分类短名 / 输入码去掉 `/`）。命中则该分类全部符号都显示。
fn sym_title_matches(title: &str, codes: &[String], q: &str) -> bool {
    let q = q.to_lowercase();
    title.to_lowercase().contains(&q)
        || codes.iter().any(|c| c.trim_start_matches('/').to_lowercase().contains(&q))
}

fn tab_titles(p: &Panel) -> Vec<String> {
    match p.kind {
        Kind::Clipboard => ["全部", "置顶", "收藏", "隐藏"].iter().map(|s| s.to_string()).collect(),
        Kind::Symbol => p.sym_tabs.iter().map(|t| t.title.clone()).collect(),
        Kind::Sync => ["用户词库", "剪贴板"].iter().map(|s| s.to_string()).collect(),
    }
}

/// 内容区顶部（每种面板的「头」高度不同）：
/// * 剪贴板 = 标题 + 过滤标签 + 搜索框
/// * 符号   = 标题 + 分类标签（下面是单行横向符号条）
/// * 同步   = 标题 + 双标签（下面是键值状态）
fn content_top(p: &Panel) -> f32 {
    match p.kind {
        Kind::Clipboard => p.title_h + TAB_H + SEARCH_H,
        // 符号面板 2026-11-07 加了搜索框（用户点名要）。
        Kind::Symbol => p.title_h + TAB_H + SEARCH_H,
        Kind::Sync => p.title_h + TAB_H,
    }
}

/// 标签几何：(起始 x, 单个宽, 本页可见数, 本页起始索引)
fn tab_geometry(p: &Panel) -> (f32, f32, usize, usize) {
    let w = p.width as f32;
    match p.kind {
        Kind::Clipboard => {
            let tw = (w - p.pad * 2.0 - 3.0 * TAB_GAP) / 4.0;
            (p.pad, tw, 4, 0)
        }
        Kind::Sync => {
            let tw = (w - p.pad * 2.0 - TAB_GAP) / 2.0;
            (p.pad, tw, 2, 0)
        }
        Kind::Symbol => {
            // 聚合后仍有 20+ 个分类：左右箭头分页，每页 5 个。
            let count = 5;
            let tw = (w - p.pad * 2.0 - ARROW_W * 2.0 - 4.0 * TAB_GAP) / count as f32;
            let pages = (tab_titles(p).len() + count - 1) / count;
            let page = p.tab_page.min(pages.saturating_sub(1));
            (p.pad + ARROW_W, tw, count, page * count)
        }
    }
}

/// 剪贴板竖向最大滚动量（卡片列表）。
fn max_scroll(p: &Panel) -> f32 {
    match p.kind {
        Kind::Clipboard => {
            let content_h = p.height as f32 - content_top(p) - p.pad;
            (visible_clips(p).len() as f32 * CARD_H - content_h).max(0.0)
        }
        Kind::Sync => {
            let content_h = p.height as f32 - content_top(p) - SYNC_FOOT_H - p.pad;
            let form_h = sync_rows(p).len() as f32 * SYNC_FORM_ROW_H + 8.0;
            let lines_h = p.sync_lines.len() as f32 * SYNC_ROW_H
                + if p.sync_busy { SYNC_ROW_H } else { 0.0 };
            (form_h + lines_h - content_h).max(0.0)
        }
        Kind::Symbol => 0.0,
    }
}

/// 符号条横向最大滚动量（格子宽按字形自适应，逐个累加，不再用固定步距×个数）。
fn max_scroll_x(p: &Panel) -> f32 {
    let items = visible_syms(p);
    let total: f32 = items
        .iter()
        .map(|s| sym_cell_size(s, p.em, p.pad).0)
        .sum::<f32>()
        + SYM_GAP * items.len().saturating_sub(1) as f32;
    (total - (p.width as f32 - p.pad * 2.0)).max(0.0)
}

fn truncate(s: &str, max: usize) -> String {
    let flat: String = s.chars().flat_map(|c| if c.is_control() { Some(' ') } else { Some(c) }).collect();
    let chars: Vec<char> = flat.chars().collect();
    if chars.len() <= max {
        flat
    } else {
        chars.iter().take(max).collect::<String>() + "…"
    }
}

// ── 命中测试 ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    None,
    Close,
    /// 标签页（剪贴板 0..3 / 符号分类 / 同步 0..1）
    Tab(usize),
    TabPrev,
    TabNext,
    Search,
    /// 剪贴板卡片 / 符号格
    Card(usize),
    Cell(usize),
    /// 剪贴板卡片内动作：0 收藏 / 1 置顶 / 2 隐藏 / 3 加密·解锁 / 4 云同步标记 /
    /// 5 复制 / 6 删除
    CardAction(usize, usize),
    /// 同步面板底部动作：0 测试连接 / 1 保存 / 2 立即同步
    SyncAction(usize),
    /// 同步表单：开关/循环行（点击切换）
    SyncToggle(usize),
    /// 同步表单：文本域（点击取焦点）
    SyncField(usize),
    /// 同步表单接收过滤：0 剪贴板 / 1 用户词库
    SyncCheck(usize),
    /// 剪贴板密码模态层：输入框 / 确定 / 取消
    ModalInput,
    ModalOk,
    ModalCancel,
}

/// 卡片动作按钮的个数（原型：收藏/置顶/隐藏/加密/同步/复制/删除，2026-11-07 用户
/// 「123 都要做」把原型的「加密」「云同步」补上，故 5 → 7）。
const CARD_ACTS: usize = 7;

fn inside(x: f32, y: f32, l: f32, t: f32, r: f32, b: f32) -> bool {
    x >= l && x < r && y >= t && y < b
}

/// 卡片动作按钮几何：(起点 x, 按钮宽)，第 k 个按钮左沿 = `x0 + k*(w+GAP)`。
fn card_act_geometry(p: &Panel) -> (f32, f32) {
    card_act_geometry_for(p.width as f32, p.pad)
}

/// 同上，纯函数版（单测用；7 个按钮要能塞进 430 宽的一行）。
fn card_act_geometry_for(width: f32, pad: f32) -> (f32, f32) {
    let n = CARD_ACTS as f32;
    let aw = (width - pad * 2.0 - 8.0 - (n - 1.0) * SYM_GAP) / n;
    (pad + 4.0, aw)
}

// ── 密码模态层几何（纯函数，绘制 / 命中两处共用） ─────────────────────────

/// 模态框尺寸。
const MODAL_W: f32 = 320.0;
const MODAL_H: f32 = 174.0;
/// 输入框高。
const MODAL_INPUT_H: f32 = 30.0;
/// 底部按钮行高。
const MODAL_BTN_H: f32 = 30.0;

/// 模态框矩形：(左, 上, 宽, 高)，面板内居中。
fn modal_rect(p: &Panel) -> (f32, f32, f32, f32) {
    let w = MODAL_W.min(p.width as f32 - 16.0);
    let h = MODAL_H.min(p.height as f32 - 16.0);
    (((p.width as f32 - w) / 2.0).max(8.0), ((p.height as f32 - h) / 2.0).max(8.0), w, h)
}

/// 密码输入框矩形。
fn modal_input_rect(p: &Panel) -> (f32, f32, f32, f32) {
    let (x, y, w, _) = modal_rect(p);
    (x + 16.0, y + 62.0, w - 32.0, MODAL_INPUT_H)
}

/// 底部两个按钮矩形：(确定, 取消)——左右各半。
fn modal_buttons(p: &Panel) -> ((f32, f32, f32, f32), (f32, f32, f32, f32)) {
    let (x, y, w, h) = modal_rect(p);
    let bw = (w - 32.0 - 10.0) / 2.0;
    let top = y + h - 16.0 - MODAL_BTN_H;
    ((x + 16.0, top, bw, MODAL_BTN_H), (x + 16.0 + bw + 10.0, top, bw, MODAL_BTN_H))
}

/// 模态层的命中：只认输入框/确定/取消，其它地方一律 `None`（模态拦截）。
fn modal_hit(p: &Panel, x: f32, y: f32) -> Hit {
    let (il, it, iw, ih) = modal_input_rect(p);
    if inside(x, y, il, it, il + iw, it + ih) {
        return Hit::ModalInput;
    }
    let ((ol, ot, ow, oh), (cl, ct, cw, ch)) = modal_buttons(p);
    if inside(x, y, ol, ot, ol + ow, ot + oh) {
        return Hit::ModalOk;
    }
    if inside(x, y, cl, ct, cl + cw, ct + ch) {
        return Hit::ModalCancel;
    }
    Hit::None
}

fn hit_test(p: &Panel, x: f32, y: f32) -> Hit {
    let w = p.width as f32;
    // 模态层打开时拦截一切点击（面板其余部分不可点、标题条不可拖）。
    if p.modal.is_some() {
        return modal_hit(p, x, y);
    }
    if inside(x, y, w - CLOSE_W, 0.0, w, p.title_h) {
        return Hit::Close;
    }
    if y < p.title_h {
        return Hit::None;
    }
    // 标签行（三种面板都有）
    if inside(x, y, 0.0, p.title_h, w, p.title_h + TAB_H) {
        if p.kind == Kind::Symbol {
            if x < p.pad + ARROW_W {
                return Hit::TabPrev;
            }
            if x > w - p.pad - ARROW_W {
                return Hit::TabNext;
            }
        }
        let (x0, tw, count, start) = tab_geometry(p);
        for k in 0..count {
            let l = x0 + k as f32 * (tw + TAB_GAP);
            if inside(x, y, l, p.title_h, l + tw, p.title_h + TAB_H) {
                let idx = start + k;
                if idx < tab_titles(p).len() {
                    return Hit::Tab(idx);
                }
            }
        }
        return Hit::None;
    }
    // 搜索框（剪贴板 / 符号面板都有；同步面板无）
    if matches!(p.kind, Kind::Clipboard | Kind::Symbol)
        && inside(x, y, p.pad, p.title_h + TAB_H, w - p.pad, p.title_h + TAB_H + SEARCH_H)
    {
        return Hit::Search;
    }
    match p.kind {
        Kind::Symbol => {
            let top = content_top(p);
            let items = visible_syms(p);
            let cells = sym_geometry_for(&items, p);
            // 竖直先粗判（格子高按字形自适应，整条高度取最大格高），再逐格精确命中。
            let strip_h = sym_strip_height(&items, p.em, p.pad) + SYM_GAP * 2.0;
            if y < top || y > top + strip_h {
                return Hit::None;
            }
            for (idx, (cx, cy, cw, ch)) in cells.iter().enumerate() {
                if inside(x, y, *cx, *cy, *cx + *cw, *cy + *ch) {
                    return Hit::Cell(idx);
                }
            }
            Hit::None
        }
        Kind::Clipboard => {
            let top = content_top(p);
            let local = y - top + p.scroll;
            if local < 0.0 {
                return Hit::None;
            }
            let idx = (local / CARD_H).floor() as usize;
            if idx >= visible_clips(p).len() {
                return Hit::None;
            }
            let card_y = top + idx as f32 * CARD_H - p.scroll;
            let ay = card_y + CARD_H - 30.0;
            if y >= ay && y <= ay + 22.0 {
                let (x0, aw) = card_act_geometry(p);
                for k in 0..CARD_ACTS {
                    let l = x0 + k as f32 * (aw + SYM_GAP);
                    if inside(x, y, l, ay, l + aw, ay + 22.0) {
                        return Hit::CardAction(idx, k);
                    }
                }
            }
            Hit::Card(idx)
        }
        Kind::Sync => {
            // 表单行：文本域取焦点、开关/循环行点击切换、接收过滤两个勾选框。
            let (vl, vr) = sync_value_rect(p);
            let specs = sync_rows(p);
            let bottom = p.height as f32 - SYNC_FOOT_H;
            for (row, spec) in specs.iter().enumerate() {
                let ry = sync_row_y(p, row);
                if ry + SYNC_FORM_ROW_H <= content_top(p) || ry >= bottom {
                    continue;
                }
                match spec.kind {
                    SyncRowKind::Text(_) => {
                        if inside(x, y, vl, ry, vr, ry + SYNC_FORM_ROW_H) {
                            return Hit::SyncField(row);
                        }
                    }
                    SyncRowKind::ReceiveChecks => {
                        let (c1, c2) = receive_check_boxes(p, ry);
                        if inside(x, y, c1.0, c1.1, c1.0 + c1.2, c1.1 + c1.3) {
                            return Hit::SyncCheck(0);
                        }
                        if inside(x, y, c2.0, c2.1, c2.0 + c2.2, c2.1 + c2.3) {
                            return Hit::SyncCheck(1);
                        }
                    }
                    SyncRowKind::Info => {}
                    _ => {
                        if inside(x, y, vl, ry, vr, ry + SYNC_FORM_ROW_H) {
                            return Hit::SyncToggle(row);
                        }
                    }
                }
            }
            // 底部动作条：测试连接 / 保存 / 立即同步（顺序与 draw_sync 一致）。
            let top = bottom;
            let n = 3.0;
            let bw = (w - p.pad * 2.0 - (n - 1.0) * 8.0) / n;
            for k in 0..3usize {
                let l = p.pad + k as f32 * (bw + 8.0);
                if inside(x, y, l, top + 8.0, l + bw, top + 38.0) {
                    return Hit::SyncAction(k);
                }
            }
            Hit::None
        }
    }
}

// ── 窗口创建 ──────────────────────────────────────────────────────────────

fn register_class() -> bool {
    static DONE: Once = Once::new();
    static OK: Mutex<bool> = Mutex::new(false);
    DONE.call_once(|| {
        unsafe {
            let wc = WNDCLASSW {
                style: (CS_HREDRAW | CS_VREDRAW) as u32,
                lpfnWndProc: Some(panel_proc),
                hInstance: HINSTANCE(std::ptr::null_mut()),
                lpszClassName: CLASS,
                hCursor: crate::d2d_bindings::LoadCursorW(None, IDC_ARROW),
                ..Default::default()
            };
            let ok = RegisterClassW(&wc).0 != 0;
            if !ok {
                crate::diagnostics::record(format_args!("panel class register failed"));
            }
            *OK.lock().unwrap() = ok;
        }
    });
    *OK.lock().unwrap()
}

/// 上次同步结果（面板正文末尾显示，最多 3 行）。
fn sync_result_lines() -> Vec<String> {
    let st = sync::load_state(&user_root());
    st.lines.into_iter().take(3).collect()
}

fn open(kind: Kind, anchor: Option<(i32, i32, i32, i32)>, horizontal: bool) {
    if !register_class() {
        return;
    }
    let kind_id = match kind {
        Kind::Sync => 0i8,
        Kind::Clipboard => 1,
        Kind::Symbol => 2,
    };
    // 每类一个槽位：**同类再点不再收起**（用户 2026-11-07 22:3x 原话：面板常驻，
    // 「只有点击关闭或者直接关闭服务才可以消失」）——已开就把它置顶保持，不销毁；
    // 别的类也一律不动（三个面板可同屏）。
    let slot = &PANEL_SLOTS[kind_id as usize];
    let old = slot.load(Ordering::SeqCst);
    let alive = old != 0
        && unsafe { raw::IsWindow(HWND(old as *mut c_void).0) } != 0;
    if alive {
        let hwnd_old = HWND(old as *mut core::ffi::c_void);
        unsafe {
            // 只提到最前面；不挪位置、不改尺寸、不激活（别抢应用焦点）。
            let _ = SetWindowPos(
                hwnd_old,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                (SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW) as u32,
            );
        }
        BUTTON_BITS.fetch_or(1 << kind_id, Ordering::Relaxed);
        ACTIVE_BUTTON.store(kind_id, Ordering::Relaxed);
        crate::diagnostics::record(format_args!(
            "[panel] 同类已在 → 置顶保持（不关）kind={kind_id}"
        ));
        return;
    }
    // 槽位里是死句柄（面板被外部销毁过）：清掉重建，别把面板卡死。
    slot.store(0, Ordering::SeqCst);
    BUTTON_BITS.fetch_and(!(1 << kind_id), Ordering::Relaxed);
    CURRENT_KIND.store(-1, Ordering::SeqCst);
    ACTIVE_BUTTON.store(-1, Ordering::Relaxed);
    let _ = SYNC_RESULT.lock().map(|mut g| *g = None);
    // 清掉可能残留的拖动状态（拖到一半面板被外部销毁时，鼠标抬起消息会丢，
    // 不重置的话下个面板一收到 WM_MOUSEMOVE 就会被当成「正在拖」）。
    PDRAG_X.store(PDRAG_OFF, Ordering::Relaxed);

    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let root = user_root();
    // 符号格子按候选字号 em 制：1.5em 见方、内边距 0.25em、字 1em（用户 2026-11-07 21:2x 口径）。
    let em = font_px();
    let pad = (em * 0.25).max(2.0);
    // 符号数据要在定面板尺寸**之前**就拿到：符号条高度取决于当前分类里最大的格子
    //（常用分类是彩色表情，格高 1.9em，比文本符号大一档）。
    let sym_tabs = if kind == Kind::Symbol {
        build_sym_tabs(&symbols::load(&root, shared_data().as_deref()))
    } else {
        Vec::new()
    };
    let strip_h = sym_strip_height(
        sym_tabs.first().map(|t| t.items.as_slice()).unwrap_or(&[]),
        em,
        pad,
    );
    // 原型 v3 的面板骨架：
    // * 符号条 = 标题行 + 分类标签行 + 单行横向符号格（贴候选窗下方，宽度跟随候选窗整宽）；
    // * 剪贴板 = 标题行 + 过滤标签 + 搜索 + 卡片列表（贴候选窗左侧）；
    // * 同步   = 标题行 + 双标签（用户词库/剪贴板）+ 键值状态 + 底部动作（贴候选窗上/下方）。
    let (w0, h) = match kind {
        Kind::Clipboard => (430, 620),
        // 符号面板：标题 + 标签 + 搜索框 + 符号条（高度跟着表情格自适应）。
        Kind::Symbol => (470, symbol_panel_height(strip_h).round() as i32),
        Kind::Sync => (SYNC_W, SYNC_H),
    };
    // 符号面板默认宽度 = 候选窗整宽（原型主列：两者同宽同左沿）。
    // 横流且右侧放不下 ⇒ 改走「同步面板右侧 + 上下停靠」，此时用面板自己的默认宽
    //（用户 2026-11-07 21:1x 的 ③④；判断口径与 `dock_point` 共用 `symbol_fits_beside`）。
    let w = match (anchor, kind) {
        (Some(a), Kind::Symbol) => {
            let cand_w = (a.2 - a.0).clamp(360, sw.max(360));
            if horizontal && !symbol_fits_beside(a, sw) {
                w0.min(sw.max(w0))
            } else {
                cand_w
            }
        }
        _ => w0.min(sw.max(w0)),
    };
    let h = h.min(sh.max(200));
    let factory: ID2D1Factory1 = match unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) } {
        Ok(f) => f,
        Err(e) => {
            crate::diagnostics::record(format_args!("D2D1CreateFactory failed: {e}"));
            return;
        }
    };
    let write: IDWriteFactory = match unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) } {
        Ok(w) => w,
        Err(e) => {
            crate::diagnostics::record(format_args!("DWriteCreateFactory failed: {e}"));
            return;
        }
    };

    // 同步面板数据读一次（避免每帧读盘）；每次保存 / 同步完成后再刷新。
    let (sync_form, sync_saved, sync_last, sync_lines, clip_stat_v) = if kind == Kind::Sync {
        let st = sync::load_state(&root);
        let cfg = sync::load_config(&root);
        (
            cfg.clone().unwrap_or_default(),
            cfg.is_some(),
            st.last_sync.clone().unwrap_or_else(|| "从未".into()),
            st.lines.into_iter().take(3).collect(),
            clip_stat(),
        )
    } else {
        (sync::SyncConfig::default(), false, String::new(), Vec::new(), (0, 0))
    };
    let panel = Box::new(Panel {
        hwnd: HWND(std::ptr::null_mut()),
        rt: None,
        factory,
        write,
        brush: None,
        text_format: None,
        small_format: None,
        right_format: None,
        center_format: None,
        kind,
        clips: if kind == Kind::Clipboard { load_clips() } else { Vec::new() },
        clip_filter: 0,
        clip_hint: if kind == Kind::Clipboard {
            match sync::load_config(&root) {
                Some(c) if c.accepts("clipboard") => "坚果云 · 已开启同步".into(),
                Some(_) => "坚果云 · 未同步剪贴板".into(),
                None => "坚果云 · 未配置".into(),
            }
        } else {
            String::new()
        },
        sym_tabs,
        sym_group: 0,
        sym_search: Vec::new(),
        scroll_x: 0.0,
        tab_page: 0,
        sync_tab: 0,
        sync_form,
        sync_saved,
        clip_stat: clip_stat_v,
        sync_last,
        sync_lines,
        sync_busy: false,
        focus: Focus::None,
        sync_phase: 0,
        sync_timer: false,
        pad,
        title_h: 34.0,
        em,
        horizontal,
        scroll: 0.0,
        hover: None,
        search: Vec::new(),
        unlocked: std::collections::HashSet::new(),
        modal: None,
        width: w,
        height: h,
        pal: current_pal(),
        anchor,
    });

    let raw = Box::into_raw(panel);
    let hwnd = unsafe {
        CreateWindowExW(
            // 2026-11-07：面板必须和候选窗一样是 **NOACTIVATE** 窗口。
            // 只在 WM_MOUSEACTIVATE 里返回 MA_NOACTIVATE 不够 —— 实测（预览注入鼠标）：
            // 真鼠标点面板时系统仍会把「本线程的活动窗口」（候选窗）提到前台，
            // 应用照样失焦 → TIP 丢焦点 → content=None → 符号 commit 退化成剪贴板。
            // 窗口样式在建窗时就带 NOACTIVATE，点击不再走激活协商，前台保持不变。
            WS_EX_TOPMOST as u32 | WS_EX_TOOLWINDOW as u32 | WS_EX_NOACTIVATE as u32,
            CLASS,
            w!(""),
            WS_POPUP as u32 | WS_CLIPSIBLINGS as u32,
            0,
            0,
            w,
            h,
            None,
            None,
            Some(HINSTANCE(std::ptr::null_mut())),
            Some(raw as *const core::ffi::c_void),
        )
    };
    if hwnd.0.is_null() {
        unsafe { drop(Box::from_raw(raw)); }
        return;
    }
    CURRENT_PANEL.store(hwnd.0 as isize, Ordering::SeqCst);
    CURRENT_KIND.store(kind_id, Ordering::SeqCst);
    PANEL_SLOTS[kind_id as usize].store(hwnd.0 as isize, Ordering::SeqCst);
    BUTTON_BITS.fetch_or(1 << kind_id, Ordering::Relaxed);
    ACTIVE_BUTTON.store(kind_id, Ordering::Relaxed);
    unsafe {
        let rgn = CreateRoundRectRgn(0, 0, w, h, 12, 12);
        if !rgn.0.is_null() {
            let _ = SetWindowRgn(hwnd, Some(rgn), true);
        }
        // 停靠定位（原型 v3：剪贴板贴左、符号贴候选窗下方、同步贴候选窗上方/下方；
        // 2026-11-07 用户口径「一一对应吸附」）——规则集中在纯函数 `dock_point` 里，单测覆盖。
        // 没有锚点（预览等）回落到光标旁。
        let (x, y) = match anchor {
            Some(a) => dock_point(kind, a, (w, h), (sw, sh), horizontal),
            None => {
                let mut pt = POINT { x: 0, y: 0 };
                let _ = GetCursorPos(&mut pt);
                let x = (pt.x + 8).clamp(0, (sw - w).max(0));
                let y = (pt.y + 8).clamp(0, (sh - h).max(0));
                (x, y)
            }
        };
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, (SWP_NOACTIVATE | SWP_SHOWWINDOW) as u32);
        crate::diagnostics::record(format_args!(
            "[dock] kind={kind:?} anchor={anchor:?} → x={x} y={y} w={w} h={h}"
        ));
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    // 2026-11-07 用户口径「面板常驻」：打开后只有右上角关闭钮（或退出服务）才收起。
    // 旧实现每 40ms 轮询「点面板外 → 收起」，现在整段删掉（候选窗收起也不关，见
    // `mod.rs::Window::hide`）。这里只剩同步面板的后台服务启动。
    if kind == Kind::Sync {
        start_services();
    }
}

/// 符号面板高度跟着当前符号条的实际格高走（用户 2026-11-07 21:1x「表情自动变化宽高」）。
///
/// 用户 21:2x 补口径后格高统一为 1.5em（表情/文本同格），本函数正常情况下是幂等的；
/// 留着是因为它保证「改字号/改分类后窗口尺寸与 render target 一定对得上」——
/// 尺寸变了必须重建 `ID2D1HwndRenderTarget`（pixelSize 不跟随窗口尺寸），并按停靠规则重落位。
fn fit_symbol_height(p: &mut Panel) {
    if p.kind != Kind::Symbol {
        return;
    }
    let items = visible_syms(p);
    let strip_h = sym_strip_height(&items, p.em, p.pad);
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let h = (symbol_panel_height(strip_h).round() as i32).min(sh.max(200));
    if h == p.height {
        return;
    }
    p.height = h;
    // 尺寸变了：旧 render target 的 pixelSize 还是旧尺寸，必须丢掉重建。
    p.rt = None;
    p.brush = None;
    unsafe {
        let rgn = CreateRoundRectRgn(0, 0, p.width, p.height, 12, 12);
        if !rgn.0.is_null() {
            let _ = SetWindowRgn(p.hwnd, Some(rgn), true);
        }
        // 重新落位：有锚点按停靠规则算；没有锚点（预览）保持当前左上角，只改尺寸。
        let (x, y) = match p.anchor {
            Some(a) => dock_point(Kind::Symbol, a, (p.width, p.height), (sw, sh), p.horizontal),
            None => {
                let mut r = [0i32; 4];
                if raw::GetWindowRect(p.hwnd.0, r.as_mut_ptr()) != 0 {
                    (r[0], r[1])
                } else {
                    (0, 0)
                }
            }
        };
        let _ = SetWindowPos(
            p.hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            p.width,
            p.height,
            (SWP_NOACTIVATE | SWP_SHOWWINDOW) as u32,
        );
        crate::diagnostics::record(format_args!(
            "[sym] 面板高度自适应 → h={} x={x} y={y}",
            p.height
        ));
        let _ = InvalidateRect(Some(p.hwnd), None, false);
    }
}

fn panel_from(hwnd: HWND) -> Option<*mut Panel> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Panel;
    (!raw.is_null()).then_some(raw)
}

/// 建立 D2D 渲染目标与文本格式（首次 WM_PAINT 前）。
fn ensure_graphics(p: &mut Panel) -> bool {
    if p.rt.is_some() {
        return true;
    }
    unsafe {
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: p.hwnd,
            pixelSize: D2D_SIZE_U { width: p.width as u32, height: p.height as u32 },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let rt: ID2D1HwndRenderTarget = match p.factory.CreateHwndRenderTarget(&props, &hwnd_props) {
            Ok(rt) => rt,
            Err(e) => {
                crate::diagnostics::record(format_args!("CreateHwndRenderTarget failed: {e}"));
                return false;
            }
        };
        let brush: ID2D1SolidColorBrush = match rt.CreateSolidColorBrush(&p.pal.ink, None) {
            Ok(b) => b,
            Err(e) => {
                crate::diagnostics::record(format_args!("CreateSolidColorBrush failed: {e}"));
                return false;
            }
        };
        let locale: Vec<u16> = "zh-CN".encode_utf16().chain(std::iter::once(0)).collect();
        let family: Vec<u16> = "Microsoft YaHei".encode_utf16().chain(std::iter::once(0)).collect();
        // ⚠️ DirectWrite 的 COM「clone」只是**同一个对象加引用**：旧代码 `let cf = tf.clone()`
        // 再 `SetTextAlignment(CENTER)` 会把标题格式也改成居中（2026-11-07 预览发现）。
        // 标题 / 正文 / 居中（符号格）/ 右对齐（标题提示）必须是四个独立对象。
        let Some(tf) = make_text_format(&p.write, &family, &locale, TITLE_FONT) else {
            crate::diagnostics::record(format_args!("CreateTextFormat(title) failed"));
            return false;
        };
        let Some(sf) = make_text_format(&p.write, &family, &locale, BODY_FONT) else {
            crate::diagnostics::record(format_args!("CreateTextFormat(small) failed"));
            return false;
        };
        // 符号格：字号 1em（用户 2026-11-07 21:2x「字 1em」），表情/文本符号共用同一个格式；
        // 表情绘制时另开彩色字体开关（`label_color`），格式本身不需要单独一套。
        let Some(cf) = make_text_format(
            &p.write,
            &family,
            &locale,
            (p.em * SYM_FONT_EM).max(18.0),
        ) else {
            crate::diagnostics::record(format_args!("CreateTextFormat(center) failed"));
            return false;
        };
        let _ = cf.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = cf.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        let Some(rf) = make_text_format(&p.write, &family, &locale, BODY_FONT) else {
            crate::diagnostics::record(format_args!("CreateTextFormat(right) failed"));
            return false;
        };
        let _ = rf.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING);
        p.rt = Some(rt);
        p.brush = Some(brush);
        p.center_format = Some(cf);
        p.right_format = Some(rf);
        p.text_format = Some(tf);
        p.small_format = Some(sf);
    }
    true
}

/// 独立创建一个文本格式（DirectWrite 的 COM「clone」只加引用，改对齐会连累原对象）。
unsafe fn make_text_format(
    write: &IDWriteFactory,
    family: &[u16],
    locale: &[u16],
    size: f32,
) -> Option<IDWriteTextFormat> {
    write
        .CreateTextFormat(
            PCWSTR(family.as_ptr()),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            PCWSTR(locale.as_ptr()),
        )
        .ok()
}

fn solid(brush: &ID2D1SolidColorBrush, c: D2D_COLOR_F) -> &ID2D1SolidColorBrush {
    unsafe { brush.SetColor(&c); }
    brush
}

unsafe fn fill(rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush, r: D2D_RECT_F, c: D2D_COLOR_F) {
    rt.FillRectangle(&r, solid(brush, c));
}

/// 圆角矩形填充（原型 v3 的标签底 / 卡片 / 按钮 / 输入框都是圆角）。
unsafe fn rounded(
    rt: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    r: D2D_RECT_F,
    radius: f32,
    c: D2D_COLOR_F,
) {
    let shape = D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius };
    rt.FillRoundedRectangle(&shape, solid(brush, c));
}

/// 便捷矩形构造（`left/top/right/bottom`）。
fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

unsafe fn label(
    rt: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    fmt: &IDWriteTextFormat,
    s: &str,
    r: D2D_RECT_F,
    c: D2D_COLOR_F,
) {
    let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    rt.DrawText(
        &wide,
        fmt,
        &r,
        solid(brush, c),
        D2D1_DRAW_TEXT_OPTIONS_NONE,
        DWRITE_MEASURING_MODE_NATURAL,
    );
}

/// 彩色字体版 `label`（用户 2026-11-07 21:1x「表情的颜色做回来」）。
///
/// 只有这里开 `D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT`：彩色表情（Segoe UI Emoji 的
/// COLR/CPAL 字形）按字体自带颜色画；其余文本仍走 `label`（`NONE`）—— 标题/按钮不会
/// 变彩色。非彩色字形下这个开关无副作用，颜色仍由 brush 决定。
unsafe fn label_color(
    rt: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    fmt: &IDWriteTextFormat,
    s: &str,
    r: D2D_RECT_F,
    c: D2D_COLOR_F,
) {
    let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    rt.DrawText(
        &wide,
        fmt,
        &r,
        solid(brush, c),
        D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
        DWRITE_MEASURING_MODE_NATURAL,
    );
}

fn draw(p: &mut Panel) {
    if !ensure_graphics(p) {
        return;
    }
    let rt = p.rt.as_ref().unwrap();
    let brush = p.brush.as_ref().unwrap();
    let small = p.small_format.as_ref().unwrap();
    // ⚠️ 这里原来有一段 `fill(...ORANGE)`（标题条底色）**放在 BeginDraw 之前**：
    // 面板第一帧 `EndDraw` 就返回 0x88990001 (D2DERR_WRONG_STATE) ⇒ 永远不呈现。
    // 面板窗口在、可见、置顶、可命中，但屏幕上一个像素都不画 ⇒ 用户点「符/贴/同」
    // 看起来就是"按不了"（2026-11-07 实测；标题条底色在 BeginDraw 里本来就用 BAR 再画一遍，
    // 这段纯属冗余且致命，删除）。
    let title_fmt = p.text_format.as_ref().unwrap();
    let right = p.right_format.as_ref().unwrap();
    let w = p.width as f32;
    unsafe {
        rt.BeginDraw();
        rt.Clear(Some(&p.pal.bg));
        // ── 标题行（原型 v3：标题在左、来源/状态提示在右、关闭钮最右） ─────
        fill(rt, brush, rect(0.0, 0.0, w, p.title_h), p.pal.bar);
        fill(rt, brush, rect(0.0, p.title_h - 1.0, w, p.title_h), p.pal.line);
        let title = match p.kind {
            Kind::Clipboard => "剪贴板",
            Kind::Symbol => "符号 / 表情",
            Kind::Sync => "同步",
        };
        label(rt, brush, title_fmt, title, rect(p.pad, 0.0, w - CLOSE_W - 8.0, p.title_h), p.pal.ink);
        let hint = match p.kind {
            Kind::Symbol => {
                if !p.sym_search.is_empty() {
                    format!("symbols.yaml · 搜索 {} 项", visible_syms(p).len())
                } else {
                    let name = p.sym_tabs.get(p.sym_group).map(|t| t.title.as_str()).unwrap_or("");
                    format!("symbols.yaml · {name}")
                }
            }
            Kind::Clipboard => p.clip_hint.clone(),
            Kind::Sync => {
                if p.sync_saved {
                    "sync.json · 已保存".to_string()
                } else {
                    "sync.json · 有未保存改动".to_string()
                }
            }
        };
        if !hint.is_empty() {
            label(rt, brush, right, &hint, rect(p.pad + 100.0, 0.0, w - CLOSE_W - 4.0, p.title_h), p.pal.mute);
        }
        label(rt, brush, small, "✕", rect(w - CLOSE_W, 0.0, w, p.title_h), p.pal.mute);

        // ── 正文（每种面板自己的骨架） ───────────────────────────────
        match p.kind {
            Kind::Clipboard => {
                draw_tabs(p, rt, brush);
                draw_search(p, rt, brush);
                draw_clips(p, rt, brush);
            }
            Kind::Symbol => {
                draw_tabs(p, rt, brush);
                draw_search(p, rt, brush);
                draw_syms(p, rt, brush);
            }
            Kind::Sync => {
                draw_tabs(p, rt, brush);
                draw_sync(p, rt, brush);
            }
        }
        let _ = rt.EndDraw(None, None);
        // 没有 BeginPaint/EndPaint，必须自己把更新区标为有效，否则 WM_PAINT 会连发。
        raw::ValidateRect(p.hwnd.0, std::ptr::null());
    }
}

unsafe fn draw_tabs(p: &Panel, rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush) {
    let small = p.small_format.as_ref().unwrap();
    let w = p.width as f32;
    let top = p.title_h;
    let titles = tab_titles(p);
    let (x0, tw, count, start) = tab_geometry(p);
    if p.kind == Kind::Symbol {
        // 分类很多（聚合后 20+）：左右箭头分页；到边时变灰。
        let prev = if p.tab_page > 0 { p.pal.ink } else { p.pal.mute };
        let next = if (p.tab_page + 1) * 5 < p.sym_tabs.len() { p.pal.ink } else { p.pal.mute };
        label(rt, brush, small, "◀", rect(p.pad, top, p.pad + ARROW_W, top + TAB_H), prev);
        label(rt, brush, small, "▶", rect(w - p.pad - ARROW_W, top, w - p.pad, top + TAB_H), next);
    }
    for k in 0..count {
        let idx = start + k;
        let Some(name) = titles.get(idx) else { continue };
        let l = x0 + k as f32 * (tw + TAB_GAP);
        let selected = match p.kind {
            Kind::Clipboard => idx == p.clip_filter,
            Kind::Symbol => idx == p.sym_group,
            Kind::Sync => idx == p.sync_tab,
        };
        let hovered = matches!(p.hover, Some(Hit::Tab(i)) if i == idx);
        if selected || hovered {
            let bg = if selected { p.pal.hover } else { p.pal.bar };
            rounded(rt, brush, rect(l, top + 3.0, l + tw, top + TAB_H - 3.0), BOX_R, bg);
        }
        // 符号分类标签带「首字符图标」（原型：★ 常用 / ∑ 数学 …）。
        let text = if p.kind == Kind::Symbol {
            p.sym_tabs.get(idx).map(|t| format!("{} {}", t.icon, t.title)).unwrap_or_default()
        } else {
            name.clone()
        };
        label(
            rt,
            brush,
            small,
            &text,
            rect(l + 8.0, top, l + tw - 8.0, top + TAB_H),
            if selected { p.pal.accent } else if hovered { p.pal.ink } else { p.pal.mute },
        );
    }
}

unsafe fn draw_search(p: &Panel, rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush) {
    let small = p.small_format.as_ref().unwrap();
    let w = p.width as f32;
    let top = p.title_h + TAB_H;
    // 原型 .clip-search：深色输入框 + 圆角。剪贴板 / 符号两个面板共用这一行。
    rounded(rt, brush, rect(p.pad, top + 3.0, w - p.pad, top + SEARCH_H - 3.0), BOX_R, p.pal.bar);
    let (buf, placeholder) = match p.kind {
        Kind::Symbol => (&p.sym_search, "🔍 搜索符号（字符 / 分类 / 编码）…"),
        _ => (&p.search, "🔍 搜索剪贴板内容…"),
    };
    let text = if buf.is_empty() {
        placeholder.to_string()
    } else {
        format!("🔍 {}│", truncate(&String::from_utf16_lossy(buf), 40))
    };
    label(
        rt,
        brush,
        small,
        &text,
        rect(p.pad + 10.0, top + 3.0, w - p.pad - 10.0, top + SEARCH_H - 3.0),
        if buf.is_empty() { p.pal.mute } else { p.pal.ink },
    );
}

unsafe fn draw_clips(p: &Panel, rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush) {
    let small = p.small_format.as_ref().unwrap();
    let list = visible_clips(p);
    let w = p.width as f32;
    let top = content_top(p);
    for (i, row) in list.iter().enumerate() {
        let y = top + (i as f32) * CARD_H - p.scroll;
        if y + CARD_H < top || y > p.height as f32 {
            continue;
        }
        // 卡片（原型 .clip-item：深色底 + 圆角卡片，卡片间留缝）。
        rounded(rt, brush, rect(p.pad, y + 2.0, w - p.pad, y + CARD_H - 2.0), BOX_R, p.pal.bar);
        let hovered = matches!(p.hover, Some(Hit::Card(h)) | Some(Hit::CardAction(h, _)) if h == i);
        if hovered {
            // 悬停只在卡片左侧画一条强调色（深色主题下整块打亮会糊）。
            fill(rt, brush, rect(p.pad, y + 8.0, p.pad + 2.0, y + CARD_H - 10.0), p.pal.accent);
        }
        // 正文（最多两行：卡片正文区高 36px，中文 ~28 字/行 ⇒ 截到 42 字）
        // 加密且未解锁的条目显示「🔒 已加密」，不会把密文画到屏幕上。
        label(
            rt,
            brush,
            small,
            &truncate(&clip_display(p, row), 42),
            rect(p.pad + 10.0, y + 6.0, w - p.pad - 10.0, y + 42.0),
            p.pal.ink,
        );
        // 元信息行：序号 · 时间 · 状态
        let mut meta = format!("#{} · {}", i + 1, row.time);
        if row.pin {
            meta.push_str(" · 置顶");
        }
        if row.fav {
            meta.push_str(" · 收藏");
        }
        if row.hid {
            meta.push_str(" · 隐藏");
        }
        if row.lock {
            meta.push_str(" · 🔒 加密");
        }
        if row.synced {
            meta.push_str(" · ☁ 已同步");
        }
        label(
            rt,
            brush,
            small,
            &meta,
            rect(p.pad + 10.0, y + 46.0, w - p.pad - 10.0, y + 60.0),
            p.pal.mute,
        );
        // 动作行（原型：收藏/置顶/隐藏/加密/同步/复制/删除，7 个二字钮；
        // 卡片动作按钮只有 ~53px 宽，多一个字就会折行，所以状态靠颜色 + meta 行表达。）
        let (x0, aw) = card_act_geometry(p);
        let ay = y + CARD_H - 30.0;
        let acts = [
            ("收藏", row.fav),
            ("置顶", row.pin),
            ("隐藏", row.hid),
            (if row.lock { "解锁" } else { "加密" }, row.lock),
            ("同步", row.synced),
            ("复制", false),
            ("删除", false),
        ];
        for (k, (text, on)) in acts.iter().enumerate() {
            let l = x0 + k as f32 * (aw + SYM_GAP);
            let bh = matches!(p.hover, Some(Hit::CardAction(a, kk)) if a == i && kk == k);
            rounded(rt, brush, rect(l, ay, l + aw, ay + 22.0), 6.0, if bh { p.pal.hover } else { p.pal.bg });
            let color = match k {
                6 => p.pal.danger,
                3 if row.lock => p.pal.accent,
                4 if row.synced => p.pal.accent,
                _ if *on => p.pal.pin,
                _ => p.pal.mute,
            };
            label(rt, brush, small, text, rect(l + 6.0, ay, l + aw - 2.0, ay + 22.0), color);
        }
    }
    if list.is_empty() {
        let hint = if !p.search.is_empty() {
            "（无匹配）"
        } else if p.clips.is_empty() {
            "（还没有历史：复制任意文本后会自动出现在这里）"
        } else {
            "（该分类下没有条目）"
        };
        label(rt, brush, small, hint, rect(p.pad, top + 16.0, w - p.pad, top + 48.0), p.pal.mute);
    }
    // 密码模态层最后画（盖在卡片列表上）。
    if p.modal.is_some() {
        draw_modal(p, rt, brush);
    }
}

/// 画剪贴板密码模态层：遮罩 + 弹框 + 输入框 + 错误行 + 确定/取消。
unsafe fn draw_modal(p: &Panel, rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush) {
    let Some(modal) = p.modal.as_ref() else { return };
    let small = p.small_format.as_ref().unwrap();
    let title_fmt = p.text_format.as_ref().unwrap();
    let w = p.width as f32;
    let h = p.height as f32;
    let shade = |a: f32| D2D_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a };
    // 整面板压暗：突出模态层，同时挡住背后的卡片。
    fill(rt, brush, rect(0.0, 0.0, w, h), shade(0.55));
    let (x, y, mw, mh) = modal_rect(p);
    rounded(rt, brush, rect(x + 2.0, y + 3.0, x + mw + 2.0, y + mh + 3.0), BOX_R + 2.0, shade(0.35));
    rounded(rt, brush, rect(x, y, x + mw, y + mh), BOX_R + 2.0, p.pal.bar);
    fill(rt, brush, rect(x, y + mh - 52.0, x + mw, y + mh - 51.0), p.pal.line);
    let title = match modal.purpose {
        ModalPurpose::SetLock => "设置密码 · 加密该条",
        _ => "输入密码 · 解锁该条",
    };
    label(rt, brush, title_fmt, title, rect(x + 16.0, y + 10.0, x + mw - 16.0, y + 42.0), p.pal.ink);
    // 密码输入框（圆点掩码 + 光标）。
    let (il, it, iw, ih) = modal_input_rect(p);
    rounded(rt, brush, rect(il, it, il + iw, it + ih), 6.0, p.pal.bg);
    let shown = if modal.input.is_empty() {
        "输入密码…".to_string()
    } else {
        format!("{}│", "●".repeat(modal.input.len()))
    };
    label(
        rt,
        brush,
        small,
        &shown,
        rect(il + 10.0, it, il + iw - 10.0, it + ih),
        if modal.input.is_empty() { p.pal.mute } else { p.pal.ink },
    );
    if !modal.error.is_empty() {
        label(rt, brush, small, &modal.error, rect(x + 16.0, y + 96.0, x + mw - 16.0, y + 120.0), p.pal.danger);
    }
    // 底部按钮：确定（强调色）/ 取消。
    let ((ol, ot, ow, oh), (cl, ct, cw, ch)) = modal_buttons(p);
    let buttons = [(ol, ot, ow, oh, "确定", p.pal.accent), (cl, ct, cw, ch, "取消", p.pal.ink)];
    for (idx, (l, t, bw, bh, text, color)) in buttons.iter().enumerate() {
        let hovered = match (idx, p.hover) {
            (0, Some(Hit::ModalOk)) | (1, Some(Hit::ModalCancel)) => true,
            _ => false,
        };
        rounded(rt, brush, rect(*l, *t, *l + *bw, *t + *bh), 6.0, if hovered { p.pal.hover } else { p.pal.bg });
        label(rt, brush, small, text, rect(*l + 8.0, *t, *l + *bw - 4.0, *t + *bh), *color);
    }
}

unsafe fn draw_syms(p: &Panel, rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush) {
    let fmt = p.text_format.as_ref().unwrap();
    let small = p.small_format.as_ref().unwrap();
    let w = p.width as f32;
    let items = visible_syms(p);
    // 几何与命中测试共用同一个函数（格子宽高按字形自适应，见 `sym_cell_size`）。
    let cells = sym_geometry_for(&items, p);
    let strip_h = sym_strip_height(&items, p.em, p.pad);
    // 原型 .sym-grid：**单行横滚**的符号条（不是多行网格）。
    for (i, sym) in items.iter().enumerate() {
        let (x, y, cw, ch) = cells[i];
        if x + cw < p.pad || x > w - p.pad {
            continue;
        }
        if matches!(p.hover, Some(Hit::Cell(h)) if h == i) {
            rounded(rt, brush, rect(x, y, x + cw, y + ch), 6.0, p.pal.hover);
        }
        // 表情：同 1em 格式 + 彩色字体开关；其它符号：普通文本。格子尺寸同一套（1.5em）。
        let has_vs16 = sym.chars().any(|c| c == '\u{FE0F}');
        let emoji = sym.chars().any(|c| is_emoji_char(c, has_vs16));
        let cf = p.center_format.as_ref().unwrap_or(fmt);
        let cell_rect = rect(x, y, x + cw, y + ch);
        if emoji {
            label_color(rt, brush, cf, sym, cell_rect, p.pal.ink);
        } else {
            label(rt, brush, cf, sym, cell_rect, p.pal.ink);
        }
    }
    if items.is_empty() {
        let hint = if !p.sym_search.is_empty() {
            "（没有匹配的符号）"
        } else {
            "（该分类没有符号）"
        };
        let top = content_top(p) + SYM_GAP;
        label(
            rt,
            brush,
            small,
            hint,
            rect(p.pad, top + 4.0, w - p.pad, top + strip_h),
            p.pal.mute,
        );
    }
}

unsafe fn draw_sync(p: &Panel, rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush) {
    let small = p.small_format.as_ref().unwrap();
    let w = p.width as f32;
    let top = content_top(p);
    let bottom = p.height as f32 - SYNC_FOOT_H;
    let specs = sync_rows(p);
    let (vl, vr) = sync_value_rect(p);
    // 表单行（2026-11-07：只读状态页 → 可编辑表单 + 开关）。
    for (row, spec) in specs.iter().enumerate() {
        let y = sync_row_y(p, row);
        if y + SYNC_FORM_ROW_H <= top || y >= bottom {
            continue;
        }
        fill(rt, brush, rect(p.pad, y + SYNC_FORM_ROW_H - 1.0, w - p.pad, y + SYNC_FORM_ROW_H), p.pal.line);
        label(
            rt,
            brush,
            small,
            spec.label,
            rect(p.pad, y, p.pad + SYNC_LABEL_W - 6.0, y + SYNC_FORM_ROW_H),
            p.pal.mute,
        );
        match spec.kind {
            SyncRowKind::ToggleAuto => draw_switch(p, rt, brush, vl, y, p.sync_form.auto),
            SyncRowKind::ToggleClipboard => {
                draw_switch(p, rt, brush, vl, y, p.sync_form.accepts("clipboard"))
            }
            SyncRowKind::CycleMode => {
                draw_cycle(p, rt, brush, vl, vr, y, sync_mode_label(&p.sync_form.server))
            }
            SyncRowKind::CycleInterval => {
                draw_cycle(p, rt, brush, vl, vr, y, &interval_label(p.sync_form.interval_min))
            }
            SyncRowKind::Text(field) => draw_text_field(p, rt, brush, field, vl, vr, y),
            SyncRowKind::ReceiveChecks => {
                let (c1, c2) = receive_check_boxes(p, y);
                draw_check(p, rt, brush, c1, p.sync_form.accepts("clipboard"), "剪贴板");
                draw_check(p, rt, brush, c2, p.sync_form.accepts("userdb"), "用户词库");
            }
            SyncRowKind::Info => {
                let text = if p.sync_tab == 1 && row == 1 {
                    format!("{} / {} 条已打同步标记", p.clip_stat.0, p.clip_stat.1)
                } else {
                    p.sync_last.clone()
                };
                label(
                    rt,
                    brush,
                    small,
                    &truncate(&text, 32),
                    rect(vl, y, vr, y + SYNC_FORM_ROW_H),
                    p.pal.ink,
                );
            }
        }
    }
    // 上次同步结果 / 本次执行结果（表单下方）。
    let mut y = top + 8.0 + specs.len() as f32 * SYNC_FORM_ROW_H - p.scroll;
    for line in p.sync_lines.iter() {
        if y + SYNC_ROW_H > top && y < bottom {
            label(rt, brush, small, &truncate(line, 60), rect(p.pad, y, w - p.pad, y + SYNC_ROW_H), p.pal.mute);
        }
        y += SYNC_ROW_H;
    }
    if p.sync_busy && y + SYNC_ROW_H > top && y < bottom {
        label(
            rt,
            brush,
            small,
            "执行中…（网络任务在后台跑，不冻界面）",
            rect(p.pad, y, w - p.pad, y + SYNC_ROW_H),
            p.pal.accent,
        );
    }
    // 底部动作条：测试连接 / 保存 / 立即同步。
    fill(rt, brush, rect(0.0, bottom, w, p.height as f32), p.pal.bar);
    fill(rt, brush, rect(0.0, bottom, w, bottom + 1.0), p.pal.line);
    // 不确定进度条：sync 后端没有真实进度回调，只能用 WM_TIMER 做往返动画。
    if p.sync_busy {
        rounded(rt, brush, rect(p.pad, bottom + 3.0, w - p.pad, bottom + 6.0), 2.0, p.pal.line);
        let seg = 90.0f32.min((w - p.pad * 2.0).max(1.0));
        let span = (w - p.pad * 2.0 - seg).max(1.0);
        let t = (p.sync_phase % 40) as f32 / 40.0;
        let x = p.pad + span * if t < 0.5 { t * 2.0 } else { (1.0 - t) * 2.0 };
        rounded(rt, brush, rect(x, bottom + 3.0, x + seg, bottom + 6.0), 2.0, p.pal.accent);
    }
    let n = 3.0;
    let bw = (w - p.pad * 2.0 - (n - 1.0) * 8.0) / n;
    for (k, name) in ["测试连接", "保存", "立即同步"].iter().enumerate() {
        let l = p.pad + k as f32 * (bw + 8.0);
        let hovered = matches!(p.hover, Some(Hit::SyncAction(i)) if i == k);
        rounded(
            rt,
            brush,
            rect(l, bottom + 8.0, l + bw, bottom + 38.0),
            BOX_R,
            if hovered { p.pal.hover } else { p.pal.bg },
        );
        // 保存按钮在「有未保存改动」时用强调色提示。
        let color = if k == 2 || (k == 1 && !p.sync_saved) { p.pal.accent } else { p.pal.ink };
        label(rt, brush, small, name, rect(l + 8.0, bottom + 8.0, l + bw - 4.0, bottom + 38.0), color);
    }
}

/// 开关控件：42×24 药丸 + 18px 圆钮。
unsafe fn draw_switch(
    p: &Panel,
    rt: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    l: f32,
    y: f32,
    on: bool,
) {
    let top = y + (SYNC_FORM_ROW_H - SYNC_SWITCH_H) / 2.0;
    rounded(
        rt,
        brush,
        rect(l, top, l + SYNC_SWITCH_W, top + SYNC_SWITCH_H),
        SYNC_SWITCH_H / 2.0,
        if on { p.pal.accent } else { p.pal.line },
    );
    let knob = 18.0;
    let kx = if on { l + SYNC_SWITCH_W - knob - 3.0 } else { l + 3.0 };
    rounded(
        rt,
        brush,
        rect(kx, top + 3.0, kx + knob, top + 3.0 + knob),
        knob / 2.0,
        if on { p.pal.bg } else { p.pal.mute },
    );
    let small = p.small_format.as_ref().unwrap();
    label(
        rt,
        brush,
        small,
        if on { "已开启" } else { "已关闭" },
        rect(l + SYNC_SWITCH_W + 8.0, y, l + SYNC_SWITCH_W + 88.0, y + SYNC_FORM_ROW_H),
        if on { p.pal.accent } else { p.pal.mute },
    );
}

/// 循环选择值（同步方式 / 同步频率）：点击换下一项。
unsafe fn draw_cycle(
    p: &Panel,
    rt: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    l: f32,
    r: f32,
    y: f32,
    text: &str,
) {
    let small = p.small_format.as_ref().unwrap();
    let top = y + (SYNC_FORM_ROW_H - 26.0) / 2.0;
    rounded(rt, brush, rect(l, top, r, top + 26.0), 6.0, p.pal.bg);
    label(
        rt,
        brush,
        small,
        &format!("{text}   ⟳"),
        rect(l + 10.0, top, r - 4.0, top + 26.0),
        p.pal.accent,
    );
}

/// 文本域（服务器 / 账号 / 应用密码 / 词库路径）。密码按码点打点；聚焦时底边画强调线。
unsafe fn draw_text_field(
    p: &Panel,
    rt: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    field: SyncField,
    l: f32,
    r: f32,
    y: f32,
) {
    let small = p.small_format.as_ref().unwrap();
    let top = y + (SYNC_FORM_ROW_H - 26.0) / 2.0;
    let focused = p.focus == Focus::Sync(field);
    rounded(rt, brush, rect(l, top, r, top + 26.0), 6.0, p.pal.bg);
    if focused {
        fill(rt, brush, rect(l + 4.0, top + 25.0, r - 4.0, top + 26.0), p.pal.accent);
    }
    let raw = field.value(&p.sync_form);
    let masked = if field == SyncField::Pass {
        "●".repeat(raw.chars().count())
    } else {
        raw.to_string()
    };
    let (text, color) = if masked.is_empty() {
        (field.placeholder().to_string(), p.pal.mute)
    } else if focused {
        (format!("{masked}│"), p.pal.ink)
    } else {
        (masked, p.pal.ink)
    };
    let max = if field == SyncField::Pass { 32 } else { 40 };
    label(rt, brush, small, &truncate(&text, max), rect(l + 10.0, top, r - 8.0, top + 26.0), color);
}

/// 勾选框（18×18）+ 右侧文字。
unsafe fn draw_check(
    p: &Panel,
    rt: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    (l, t, bw, bh): (f32, f32, f32, f32),
    on: bool,
    text: &str,
) {
    let small = p.small_format.as_ref().unwrap();
    rounded(rt, brush, rect(l, t, l + bh, t + bh), 4.0, if on { p.pal.accent } else { p.pal.bg });
    if on {
        let cf = p.center_format.as_ref().unwrap_or(small);
        label(rt, brush, cf, "✓", rect(l, t, l + bh, t + bh), p.pal.bg);
    }
    label(rt, brush, small, text, rect(l + bh + 6.0, t - 3.0, l + bw, t + bh + 3.0), p.pal.ink);
}

// ── 交互 ──────────────────────────────────────────────────────────────────

/// 后台跑网络任务，完成后 `WM_SYNC_DONE` 把结果送回 UI 线程。
fn spawn_task(hwnd: HWND, job: impl FnOnce() -> Vec<String> + Send + 'static) {
    let h = hwnd.0 as isize;
    let _ = std::thread::Builder::new().name("rime-sync-task".into()).spawn(move || {
        let lines = job();
        if let Ok(mut g) = SYNC_RESULT.lock() {
            *g = Some(lines);
        }
        unsafe {
            raw::PostMessageW(h as *mut c_void, WM_SYNC_DONE as u32, 0, 0);
        }
    });
}

/// 把表单写回 `sync.json`（自动同步线程每 20s 重读配置）。
fn sync_save(p: &mut Panel) {
    match sync::save_config(&user_root(), &p.sync_form) {
        Ok(()) => {
            p.sync_saved = true;
            p.sync_lines = vec![format!("已保存 {}", sync::config_path(&user_root()).display())];
        }
        Err(e) => p.sync_lines = vec![format!("保存失败：{e}")],
    }
}

/// 启动/重置不确定进度条动画（WM_TIMER 60ms 一跳）。
fn start_progress(p: &mut Panel) {
    p.sync_phase = 0;
    if !p.sync_timer {
        unsafe {
            let _ = SetTimer(Some(p.hwnd), SYNC_TICK_ID, SYNC_TICK_MS, None);
        }
        p.sync_timer = true;
    }
}

/// 同步面板底部三个动作：0 测试连接 / 1 保存 / 2 立即同步。
///
/// ⚠️ 旧实现的标签顺序是「测试连接 / 编辑配置 / 立即同步」，但 `match` 里 0 跑同步、
/// 1 测试、2 打开记事本 —— 标签与功能整体错位（2026-11-07 复查发现），这里统一按
/// 绘制顺序映射：0 测试、1 保存、2 同步，不再有错位。
fn sync_action(p: &mut Panel, which: usize) {
    if p.sync_busy {
        return;
    }
    let root = user_root();
    match which {
        // 0 测试连接：用表单**当前值**测（不写盘，立刻反映输入）。
        0 => {
            let cfg = p.sync_form.clone();
            if cfg.server.trim().is_empty() || cfg.user.trim().is_empty() {
                p.sync_lines = vec!["请先填「服务器」与「账号」".into()];
            } else {
                p.sync_busy = true;
                start_progress(p);
                spawn_task(p.hwnd, move || match sync::test_connection(&cfg) {
                    Ok(msg) => vec![msg],
                    Err(e) => vec![format!("测试失败：{e}")],
                });
            }
        }
        // 1 保存：表单 → sync.json。
        1 => sync_save(p),
        // 2 立即同步：run_once 从磁盘读配置 ⇒ 先保存再跑。
        _ => {
            if let Err(e) = sync::save_config(&root, &p.sync_form) {
                p.sync_lines = vec![format!("保存失败：{e}")];
            } else {
                p.sync_saved = true;
                p.sync_busy = true;
                start_progress(p);
                let root2 = root.clone();
                spawn_task(p.hwnd, move || match sync::run_once(&root2) {
                    Ok(res) => res.lines(),
                    Err(e) => vec![format!("同步失败：{e}")],
                });
            }
        }
    }
    let _ = unsafe { InvalidateRect(Some(p.hwnd), None, false) };
}

/// 同步表单里开关 / 循环行的点击行为（文本域由 `activate` 取焦点，不在这里）。
fn sync_toggle(p: &mut Panel, row: usize) {
    let Some(spec) = sync_rows(p).get(row) else { return };
    match spec.kind {
        SyncRowKind::ToggleAuto => {
            p.sync_form.auto = !p.sync_form.auto;
            // auto 与 interval_min>0 同时满足才跑；打开自动但频率还是「仅手动」时补成 30 分钟。
            if p.sync_form.auto && p.sync_form.interval_min == 0 {
                p.sync_form.interval_min = 30;
            }
        }
        SyncRowKind::ToggleClipboard => {
            let on = !p.sync_form.accepts("clipboard");
            set_receive(&mut p.sync_form.receive, "clipboard", on);
        }
        SyncRowKind::CycleMode => cycle_sync_mode(&mut p.sync_form.server),
        SyncRowKind::CycleInterval => {
            p.sync_form.interval_min = cycle_interval(p.sync_form.interval_min);
        }
        _ => return,
    }
    p.sync_saved = false;
    let _ = unsafe { InvalidateRect(Some(p.hwnd), None, false) };
}

/// 把文本直接送上屏：经候选窗 `WM_APP_COMMIT_TEXT` 直拟提交（符号/剪贴板同一条通道）；
/// 候选窗句柄还没登记（预览态从未点过编码栏按钮）才退回复制到系统剪贴板。
///
/// 2026-11-07「面板常驻」后候选窗可能已经收起（隐藏但窗口还在）：消息照样能投递，
/// `mod.rs::dispatch` 会改用最近一次渲染留下的 `EventSink`（`App::last_events`）
/// 把文本送回引擎 —— 所以上屏成功、面板不关。
fn commit_text(text: &str) {
    let hwnd = CANDIDATE_HWND.load(Ordering::SeqCst);
    if hwnd != 0 {
        let hwnd_ok = unsafe { raw::IsWindow(hwnd as *mut c_void) } != 0;
        if hwnd_ok {
            let boxed = Box::into_raw(Box::new(text.to_string()));
            unsafe {
                let _ = raw::PostMessageW(
                    hwnd as *mut c_void,
                    WM_APP_COMMIT_TEXT as u32,
                    0,
                    boxed as isize,
                );
            }
            return;
        }
    }
    // 窗口已不在（服务退出/重建）：至少别丢字。
    copy_to_clipboard(text);
}

/// 打开密码模态层，并让面板拿到键盘焦点（WM_CHAR 才进得来）。
fn open_modal(p: &mut Panel, purpose: ModalPurpose, text: String) {
    // 记住此刻的前台窗口(密码输完把焦点还给它)。
    unsafe {
        let fg = raw::GetForegroundWindow();
        PREV_FOCUS.store(fg as isize, Ordering::SeqCst);
    }
    p.modal = Some(ClipModal { purpose, text, input: Vec::new(), error: String::new() });
    unsafe {
        raw::SetForegroundWindow(p.hwnd.0);
        let _ = SetFocus(Some(p.hwnd));
    }
    let _ = unsafe { InvalidateRect(Some(p.hwnd), None, false) };
}

/// 确认模态层。返回 true = 需要关闭面板（密码正确后已上屏）。
fn confirm_modal(p: &mut Panel) -> bool {
    let Some(mut modal) = p.modal.take() else { return false };
    // 密码输入完毕:焦点立刻还给打开面板前的宿主应用。否则上屏/后续输入
    // 会经 IME 提交进面板自己的搜索框(用户 2026-11-07 实测,搜索框被自动填字)。
    {
        let pf = PREV_FOCUS.swap(0, Ordering::SeqCst);
        if pf != 0 {
            unsafe {
                let _ = raw::SetForegroundWindow(pf as *mut c_void);
                let _ = raw::SetFocus(pf as *mut c_void);
            }
        }
    }
    let input = String::from_utf16_lossy(&modal.input);
    match modal.purpose {
        ModalPurpose::SetLock => {
            if input.trim().is_empty() {
                modal.error = "密码不能为空".into();
                modal.input.clear();
                p.modal = Some(modal);
                return false;
            }
            let mut store = ClipboardStore::load(&user_root());
            store.set_lock_text(&modal.text, true, Some(input));
            p.unlocked.remove(&modal.text);
            p.clips = load_clips();
        }
        ModalPurpose::UnlockToggle | ModalPurpose::UnlockCopy | ModalPurpose::UnlockCommit => {
            if !ClipboardStore::load(&user_root()).unlock_ok(&modal.text, &input) {
                modal.error = "密码不对，请重试".into();
                modal.input.clear();
                p.modal = Some(modal);
                return false;
            }
            p.unlocked.insert(modal.text.clone());
            // 解锁标志持久化:切 tab/重载后不再重新打码(用户 2026-11-07)。
            ClipboardStore::load(&user_root()).set_unlock_text(&modal.text, true);
            match modal.purpose {
                ModalPurpose::UnlockToggle => {
                    // 解锁并**解除加密**（密码清掉），条目回到明文。
                    let mut store = ClipboardStore::load(&user_root());
                    store.set_lock_text(&modal.text, false, None);
                    p.unlocked.remove(&modal.text);
                    p.clips = load_clips();
                }
                ModalPurpose::UnlockCopy => copy_to_clipboard(&modal.text),
                _ => {
                    commit_text(&modal.text);
                    let _ = unsafe { InvalidateRect(Some(p.hwnd), None, false) };
                    // 2026-11-07 用户口径：面板常驻 —— 上屏后**不关**面板（要继续点）。
                    return false;
                }
            }
        }
    }
    p.scroll = p.scroll.min(max_scroll(p));
    let _ = unsafe { InvalidateRect(Some(p.hwnd), None, false) };
    false
}

/// 点击这个命中区后面板是否关闭。
///
/// 2026-11-07 用户口径「面板常驻」：**只有右上角关闭钮**会关；点卡片/符号/分类/
/// 搜索框/表单/模态都保持面板打开（可连续点、可继续打字，候选窗收起也不关）。
fn click_closes_panel(hit: Hit) -> bool {
    matches!(hit, Hit::Close)
}

/// 面板内按 Esc 是否关面板。
///
/// 用户 2026-11-07 22:3x 口径：只有右上角关闭钮或退出服务才消失，Esc 只退焦点 /
/// 关模态层（那两条在 `WM_KEYDOWN` 里单独处理），**不关面板**。
fn esc_closes_panel() -> bool {
    false
}

fn activate(p: &mut Panel, hit: Hit) -> bool {
    // 返回值 = **是否关闭面板**。2026-11-07 用户口径「面板常驻」：只有右上角关闭钮
    // 才返回 true；点符号 / 点卡片 / 上屏 / 点分类 / 点表单都**不关**面板。
    match hit {
        Hit::Close => return click_closes_panel(hit),
        Hit::Tab(idx) => {
            p.focus = Focus::None;
            match p.kind {
                Kind::Clipboard => p.clip_filter = idx,
                Kind::Symbol => {
                    p.sym_group = idx;
                    p.scroll_x = 0.0;
                    // 点分类标签 = 退出跨分类搜索，回到该分类。
                    p.sym_search.clear();
                    // 换分类后可见符号变了（表情格 1.9em / 文本格 1.5em）⇒ 面板高度自适应。
                    fit_symbol_height(p);
                }
                Kind::Sync => p.sync_tab = idx,
            }
            p.scroll = 0.0;
        }
        Hit::TabPrev => {
            p.tab_page = p.tab_page.saturating_sub(1);
            let pages = (p.sym_tabs.len() + 4) / 5;
            p.tab_page = p.tab_page.min(pages.saturating_sub(1));
        }
        Hit::TabNext => {
            let pages = (p.sym_tabs.len() + 4) / 5;
            p.tab_page = (p.tab_page + 1).min(pages.saturating_sub(1));
        }
        Hit::Search => unsafe {
            // 面板默认不抢焦点；点搜索框才主动取焦点，键盘输入才会进 WM_CHAR。
            p.focus = match p.kind {
                Kind::Symbol => Focus::SymSearch,
                _ => Focus::ClipSearch,
            };
            raw::SetForegroundWindow(p.hwnd.0);
            let _ = SetFocus(Some(p.hwnd));
        },
        Hit::CardAction(idx, which) => {
            let list = visible_clips(p);
            if let Some(row) = list.get(idx) {
                let text = row.raw.clone();
                let locked = row.lock && !p.unlocked.contains(&text);
                match which {
                    // 收藏 / 置顶 / 隐藏：按文本改标记（后台采集线程会重排，不能用下标）。
                    0 | 1 | 2 => {
                        let mut store = ClipboardStore::load(&user_root());
                        match which {
                            0 => store.toggle_fav_text(&text),
                            1 => store.toggle_pin_text(&text),
                            _ => store.toggle_hidden_text(&text),
                        }
                        p.clips = load_clips();
                    }
                    // 加密 / 解锁：未加密 → 设密码；已加密 → 输密码解锁并解除加密。
                    3 => {
                        let purpose = if row.lock {
                            ModalPurpose::UnlockToggle
                        } else {
                            ModalPurpose::SetLock
                        };
                        open_modal(p, purpose, text);
                    }
                    // 云同步标记：手动切换（真实同步仍由同步面板跑）。
                    4 => {
                        let mut store = ClipboardStore::load(&user_root());
                        store.set_synced_text(&text, !row.synced);
                        p.clips = load_clips();
                    }
                    // 复制：已加密未解锁 ⇒ 先输密码。
                    5 => {
                        if locked {
                            open_modal(p, ModalPurpose::UnlockCopy, text);
                        } else {
                            copy_to_clipboard(&text);
                        }
                    }
                    // 删除。
                    _ => {
                        let mut store = ClipboardStore::load(&user_root());
                        store.remove_text(&text);
                        p.clips = load_clips();
                    }
                }
            }
            p.scroll = p.scroll.min(max_scroll(p));
        }
        Hit::Card(idx) => {
            // 点卡片本体 = **上屏该条**(与符号同一条 CommitText 通道,用户 2026-11-07);
            // 「复制」钮仍在卡片上,要复制点它。无候选窗(预览)退回复制。
            // 加密且未解锁的条目先弹密码层，验证通过再上屏。
            if let Some(row) = visible_clips(p).get(idx).cloned() {
                if row.lock && !p.unlocked.contains(&row.raw) {
                    open_modal(p, ModalPurpose::UnlockCommit, row.raw);
                    return false;
                }
                commit_text(&row.raw);
            }
            // 常驻：上屏后面板不关（用户 2026-11-07 实测「点一下消失」要求修掉）。
            return false;
        }
        Hit::Cell(idx) => {
            // 符号格：优先直拟上屏（把文本送回候选窗 → 引擎提交，字直接进应用）；
            // 没有候选窗（预览态）退回复制到剪贴板。
            if let Some(sym) = visible_syms(p).get(idx) {
                commit_text(sym);
            }
            // 常驻：可连续点多个符号，面板不关。
            return false;
        }
        // 密码模态层：点输入框 = 取焦点；确定 = 校验；取消 = 收起。
        Hit::ModalInput => unsafe {
            raw::SetForegroundWindow(p.hwnd.0);
            let _ = SetFocus(Some(p.hwnd));
        },
        Hit::ModalOk => return confirm_modal(p),
        Hit::ModalCancel => {
            p.modal = None;
        }
        // 同步表单：文本域取焦点；开关/循环行点击切换；接收过滤两个勾。
        Hit::SyncField(row) => {
            if let Some(SyncRowSpec { kind: SyncRowKind::Text(field), .. }) = sync_rows(p).get(row) {
                p.focus = Focus::Sync(*field);
                unsafe {
                    raw::SetForegroundWindow(p.hwnd.0);
                    let _ = SetFocus(Some(p.hwnd));
                }
            }
        }
        Hit::SyncToggle(row) => sync_toggle(p, row),
        Hit::SyncCheck(which) => {
            let on = if which == 0 {
                !p.sync_form.accepts("clipboard")
            } else {
                !p.sync_form.accepts("userdb")
            };
            set_receive(
                &mut p.sync_form.receive,
                if which == 0 { "clipboard" } else { "userdb" },
                on,
            );
            p.sync_saved = false;
        }
        Hit::SyncAction(which) => sync_action(p, which),
        Hit::None => {}
    }
    let _ = unsafe { InvalidateRect(Some(p.hwnd), None, false) };
    false
}

/// 拖动中：按**屏幕光标位移**移动面板（2026-11-07 用户「好闪」的修法）。
///
/// 旧实现拿 WM_MOUSEMOVE 的**客户坐标差**累加窗口位置：面板一移动，下一条消息的
/// 客户坐标基准就变了，再取差值会来回抖（闪的主因）。屏幕坐标没有这个反馈环。
/// 移动参数：
/// * `SWP_NOZORDER` —— 窗口建时已是 `WS_EX_TOPMOST`，不必每帧重新置顶（z 序抖动来源之一）；
/// * `SWP_NOREDRAW` —— 面板内容不随位置变化，移动期间不重画（旧代码每移动一次整幅重画）；
///   松手时 `InvalidateRect` 补一次。
///
/// 吸附：**只看本面板自己的停靠矩形**，且只看几何中心是否进入 —— 一一对应、不互换位置。
fn drag_to_cursor(hwnd: HWND) {
    let Some(p) = panel_from(hwnd) else { return };
    let mut pt = POINT { x: 0, y: 0 };
    if !unsafe { GetCursorPos(&mut pt) }.as_bool() {
        return;
    }
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let (sx, sy) = (PDRAG_X.load(Ordering::Relaxed), PDRAG_Y.load(Ordering::Relaxed));
    // 拖出屏幕时至少留 60px 在屏内（沿用旧手感），吸附目标本身由 dock_point 夹屏。
    let mut nx = (PANEL_L.load(Ordering::Relaxed) + (pt.x - sx)).clamp(0, (sw - 60).max(0));
    let mut ny = (PANEL_T.load(Ordering::Relaxed) + (pt.y - sy)).clamp(0, (sh - 60).max(0));
    let (w, h) = unsafe { ((*p).width, (*p).height) };
    if let Some(anchor) = live_anchor(unsafe { (*p).anchor }) {
        let kind = unsafe { (*p).kind };
        if let Some((dx, dy)) = snap_point(
            kind,
            anchor,
            (w, h),
            (sw, sh),
            (nx + w / 2, ny + h / 2),
            unsafe { (*p).horizontal },
        ) {
            nx = dx;
            ny = dy;
        }
    }
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            nx,
            ny,
            0,
            0,
            (SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOREDRAW) as u32,
        );
    }
}

/// 收起所有面板。**只在主题后端销毁（渲染器/服务退出）时调用**。
///
/// 2026-11-07 用户口径：三个辅助面板「常驻」—— 点面板里的任何东西、候选窗收起、
/// 组字结束、点面板外都不收起，只有点面板右上角关闭钮或退出服务才消失。
/// 因此这里只留一条「服务退出」路径；`mod.rs::Window::drop` 调它。
pub fn close_all_panels() {
    DESTROY_CAUSE.store(DESTROY_SERVICE, Ordering::SeqCst);
    CANDIDATE_HWND.store(0, Ordering::SeqCst);
    for slot in PANEL_SLOTS.iter() {
        let h = slot.swap(0, Ordering::SeqCst);
        if h == 0 {
            continue;
        }
        let hwnd = HWND(h as *mut core::ffi::c_void);
        if unsafe { raw::IsWindow(hwnd.0) } != 0 {
            let _ = unsafe { DestroyWindow(hwnd) };
        }
    }
}

/// 当前是否有面板开着（供 `mod.rs` 打「常驻」诊断，不参与收起判定）。
pub fn any_panel_open() -> bool {
    PANEL_SLOTS.iter().any(|s| s.load(Ordering::SeqCst) != 0)
}

unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg as i32 {
        WM_CREATE => {
            let cs = lp.0 as *const CREATESTRUCTW;
            let panel = (*cs).lpCreateParams as *mut Panel;
            let p = &mut *panel;
            p.hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, panel as isize);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            if let Some(p) = panel_from(hwnd) {
                draw(&mut *p);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            // 面板拖动：拖动中本臂先接管（屏幕光标位移 + 本面板自身停靠矩形的吸附）。
            if PDRAG_X.load(std::sync::atomic::Ordering::Relaxed) != PDRAG_OFF {
                drag_to_cursor(hwnd);
                return LRESULT(0);
            }
            if let Some(p) = panel_from(hwnd) {
                let x = ((lp.0 & 0xFFFF) as u16 as i16) as f32;
                let y = (((lp.0 >> 16) & 0xFFFF) as u16 as i16) as f32;
                let h = hit_test(&*p, x, y);
                // 悬停态：标签 / 卡片 / 卡片按钮 / 符号格 / 同步按钮（Close 不算）。
                let hv = match h {
                    Hit::None | Hit::Close => None,
                    other => Some(other),
                };
                if hv != (*p).hover {
                    (*p).hover = hv;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if let Some(p) = panel_from(hwnd) {
                let x = ((lp.0 & 0xFFFF) as u16 as i16) as f32;
                let y = (((lp.0 >> 16) & 0xFFFF) as u16 as i16) as f32;
                // 顶部标题条 = 拖动条；右上 44px 留给关闭钮。密码模态层打开时不拖。
                if (*p).modal.is_none() && y < (*p).title_h && x < ((*p).width as f32) - 44.0 {
                    let mut r = [0i32; 4];
                    let mut pt = POINT { x: 0, y: 0 };
                    unsafe {
                        raw::GetWindowRect(hwnd.0, r.as_mut_ptr());
                        // 记「屏幕光标 + 窗口左上」，移动时只做屏幕坐标差（不再用客户坐标）。
                        if GetCursorPos(&mut pt).as_bool() {
                            PANEL_L.store(r[0], std::sync::atomic::Ordering::Relaxed);
                            PANEL_T.store(r[1], std::sync::atomic::Ordering::Relaxed);
                            PDRAG_X.store(pt.x, std::sync::atomic::Ordering::Relaxed);
                            PDRAG_Y.store(pt.y, std::sync::atomic::Ordering::Relaxed);
                            raw::SetCapture(hwnd.0);
                            crate::diagnostics::record(format_args!(
                                "[drag] start base=({},{}) cur=({},{}) client=({x},{y})",
                                r[0], r[1], pt.x, pt.y
                            ));
                        }
                    }
                    return LRESULT(0);
                }
                let hit = hit_test(&*p, x, y);
                if activate(&mut *p, hit) {
                    DESTROY_CAUSE.store(DESTROY_CLOSE_BTN, Ordering::SeqCst);
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if PDRAG_X.load(std::sync::atomic::Ordering::Relaxed) != PDRAG_OFF {
                PDRAG_X.store(PDRAG_OFF, std::sync::atomic::Ordering::Relaxed);
                unsafe {
                    raw::ReleaseCapture();
                    // 拖动期间用 SWP_NOREDRAW 暂停重画，松手补一次（内容没变，只是防止
                    // 拖动途中某些区域没画过）。
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            if let Some(p) = panel_from(hwnd) {
                let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as f32;
                match (*p).kind {
                    // 符号条是单行横滚：滚轮直接横向滚。
                    Kind::Symbol => {
                        let max = max_scroll_x(&*p);
                        (*p).scroll_x = ((*p).scroll_x - delta / 4.0).clamp(0.0, max);
                    }
                    _ => {
                        let max = max_scroll(&*p);
                        (*p).scroll = ((*p).scroll - delta / 4.0).clamp(0.0, max);
                    }
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_PASTE => {
            // 外部（测试脚本 / 输入法助手）显式投递的粘贴请求；真键盘走 WM_KEYDOWN 的
            // Ctrl+V 分支，两处落到同一个函数。
            if let Some(p) = panel_from(hwnd) {
                paste_into_focused(&mut *p);
            }
            LRESULT(0)
        }
        WM_CHAR => {
            if let Some(p) = panel_from(hwnd) {
                let p = &mut *p;
                let ch = wp.0 as u16;
                // 1) 密码模态层优先：所有字符都进密码框（退格 = 删一个码元）。
                if let Some(modal) = p.modal.as_mut() {
                    if ch == 8 {
                        modal.input.pop();
                        modal.error.clear();
                    } else if ch >= 32 && ch != 127 {
                        modal.input.push(ch);
                        modal.error.clear();
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return LRESULT(0);
                }
                // 2) 同步表单文本域（点行取焦点后才吃字符；密码同样走这里）。
                if let Focus::Sync(field) = p.focus {
                    if ch == 8 {
                        field.value_mut(&mut p.sync_form).pop();
                    } else if ch >= 32 && ch != 127 {
                        if let Some(c) = char::from_u32(ch as u32) {
                            field.value_mut(&mut p.sync_form).push(c);
                        }
                    }
                    p.sync_saved = false;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return LRESULT(0);
                }
                // 3) 剪贴板 / 符号搜索框（点搜索框才取焦点；剪贴板沿旧行为一开就能打字）。
                if p.kind == Kind::Clipboard {
                    if ch == 8 {
                        p.search.pop();
                    } else if ch >= 32 && ch != 127 {
                        p.search.push(ch);
                    }
                    p.scroll = 0.0;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                } else if p.kind == Kind::Symbol {
                    if ch == 8 {
                        p.sym_search.pop();
                    } else if ch >= 32 && ch != 127 {
                        p.sym_search.push(ch);
                    }
                    p.scroll_x = 0.0;
                    // 搜索结果可能混进表情（格高 1.9em）⇒ 面板高度自适应。
                    fit_symbol_height(p);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            // 密码模态层：Esc 只关模态层；Enter = 确定（上屏类会关面板）。
            if let Some(p) = panel_from(hwnd) {
                if (*p).modal.is_some() {
                    if wp.0 == 0x1B {
                        (*p).modal = None;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    } else if wp.0 == VK_RETURN {
                        if confirm_modal(&mut *p) {
                            DESTROY_CAUSE.store(DESTROY_OTHER, Ordering::SeqCst);
                            let _ = DestroyWindow(hwnd);
                        }
                    } else if ctrl_down() && wp.0 == VK_V {
                        // 2026-11-07 22:0x 用户「密码粘贴不了」：自绘模态框自己接粘贴。
                        paste_into_focused(&mut *p);
                    } else if ctrl_down() && wp.0 == VK_A {
                        clear_focused_text(&mut *p);
                    }
                    return LRESULT(0);
                }
            }
            // 同步表单：有焦点时 Esc 只退焦点，Enter = 保存（都不关面板）。
            if let Some(p) = panel_from(hwnd) {
                if (*p).focus != Focus::None {
                    if wp.0 == 0x1B {
                        (*p).focus = Focus::None;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        return LRESULT(0);
                    }
                    if wp.0 == VK_RETURN && matches!((*p).focus, Focus::Sync(_)) {
                        sync_save(&mut *p);
                        (*p).focus = Focus::None;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        return LRESULT(0);
                    }
                    if ctrl_down() && wp.0 == VK_V {
                        if paste_into_focused(&mut *p) {
                            return LRESULT(0);
                        }
                    }
                    if ctrl_down() && wp.0 == VK_A {
                        if clear_focused_text(&mut *p) {
                            return LRESULT(0);
                        }
                    }
                }
            }
            // 两个搜索框没点焦点也在吃键盘（旧行为）：Ctrl+V / Ctrl+A 同样接住。
            if ctrl_down() {
                if let Some(p) = panel_from(hwnd) {
                    if wp.0 == VK_V && paste_into_focused(&mut *p) {
                        return LRESULT(0);
                    }
                    if wp.0 == VK_A && clear_focused_text(&mut *p) {
                        return LRESULT(0);
                    }
                }
            }
            if wp.0 == 0x1B {
                // 用户 2026-11-07 22:3x 口径收紧：Esc 也**不关**面板（只退焦点 / 关模态层，
                // 那两条在上面已处理）；要关只有右上角关闭钮或退出服务。
                if esc_closes_panel() {
                    DESTROY_CAUSE.store(DESTROY_CLOSE_BTN, Ordering::SeqCst);
                    let _ = DestroyWindow(hwnd);
                } else {
                    crate::diagnostics::record(format_args!(
                        "[panel] Esc → 不关面板（等值常驻口径）"
                    ));
                }
            } else if wp.0 == VK_RETURN {
                if let Some(p) = panel_from(hwnd) {
                    let first = match (*p).kind {
                        Kind::Clipboard if !visible_clips(&*p).is_empty() => Some(Hit::Card(0)),
                        Kind::Symbol if !visible_syms(&*p).is_empty() => Some(Hit::Cell(0)),
                        _ => None,
                    };
                    if let Some(hit) = first {
                        if activate(&mut *p, hit) {
                            DESTROY_CAUSE.store(DESTROY_OTHER, Ordering::SeqCst);
                            let _ = DestroyWindow(hwnd);
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_SYNC_DONE => {
            if let Some(p) = panel_from(hwnd) {
                let extra = SYNC_RESULT.lock().ok().and_then(|mut g| g.take()).unwrap_or_default();
                (*p).sync_busy = false;
                if (*p).sync_timer {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), SYNC_TICK_ID);
                    }
                    (*p).sync_timer = false;
                }
                if extra.is_empty() {
                    (*p).sync_lines = sync_result_lines();
                } else {
                    (*p).sync_lines = extra;
                }
                // 同步/测试结束后刷新统计与上次同步时间（不重载表单，避免冲掉用户正在改的内容）。
                (*p).clip_stat = clip_stat();
                let st = sync::load_state(&user_root());
                (*p).sync_last = st.last_sync.unwrap_or_else(|| "从未".into());
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        // 同步进度条动画（60ms）定时器。2026-11-07 起「点面板外收起」的 40ms 轮询
        // 定时器整段删除：用户口径「面板常驻」，点外面不关（只有关闭钮/退出服务关）。
        WM_TIMER => {
            if let Some(p) = panel_from(hwnd) {
                if wp.0 == SYNC_TICK_ID {
                    if (*p).sync_busy {
                        (*p).sync_phase = (*p).sync_phase.wrapping_add(1);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    } else if (*p).sync_timer {
                        unsafe {
                            let _ = KillTimer(Some(hwnd), SYNC_TICK_ID);
                        }
                        (*p).sync_timer = false;
                    }
                }
            }
            LRESULT(0)
        }
        // 面板常驻：外部 WM_CLOSE（Alt+F4 等）也不许关面板 —— 只有关闭钮/退出服务才关。
        WM_CLOSE => {
            crate::diagnostics::record(format_args!("[panel] WM_CLOSE 忽略（常驻口径）"));
            LRESULT(0)
        }
        // 面板内点击不激活窗口（2026-11-07 用户实测「点符号不上屏、界面消失」的根因）：
        // 没有这个臂时，真鼠标点面板会把面板变成前台窗口 → 被输入的应用失焦 →
        // TIP 丢焦点 → 候选窗 hide / `content=None` → 符号 commit 静默退化成
        // 「复制到剪贴板」。`MA_NOACTIVATE` = 不激活窗口，但鼠标消息照常投递（不丢点击）。
        WM_MOUSEACTIVATE => {
            crate::diagnostics::record(format_args!("[panel] click 不激活（MA_NOACTIVATE）"));
            LRESULT(MA_NOACTIVATE as isize)
        }
        WM_DESTROY => {
            ACTIVE_BUTTON.store(-1, Ordering::Relaxed);
            // `open()` 换按钮时已经先把种类换成 -1 再销毁旧窗口；这里再兜一次底。
            CURRENT_KIND.store(-1, Ordering::SeqCst);
            let _ = CURRENT_PANEL.compare_exchange(hwnd.0 as isize, 0, Ordering::SeqCst, Ordering::SeqCst);
            let cause = DESTROY_CAUSE.swap(0, Ordering::SeqCst);
            let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Panel;
            if !raw.is_null() {
                if (*raw).sync_timer {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), SYNC_TICK_ID);
                    }
                }
                // 面板自己销毁（关闭钮 / Esc / 退出服务 close_all_panels）时把槽位和
                // 编码栏按钮高亮清掉，否则按钮会一直亮着。
                let kind_id = match (*raw).kind {
                    Kind::Sync => 0usize,
                    Kind::Clipboard => 1,
                    Kind::Symbol => 2,
                };
                crate::diagnostics::record(format_args!(
                    "[panel] destroy kind={kind_id} cause={cause}（1=关闭钮 2=退出服务 3=内部）"
                ));
                let _ = PANEL_SLOTS[kind_id].compare_exchange(
                    hwnd.0 as isize,
                    0,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                BUTTON_BITS.fetch_and(!(1 << kind_id), Ordering::Relaxed);
                drop(Box::from_raw(raw));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ── 三个入口（mod.rs 调用） ──────────────────────────────────────────────
// `anchor` = 候选窗屏幕矩形 (left, top, right, bottom)：面板贴着它停靠(原型 v3)。
pub fn open_clipboard(anchor: Option<(i32, i32, i32, i32)>) {
    crate::diagnostics::record(format_args!("[panel] open_clipboard"));
    open(Kind::Clipboard, anchor, false);
}
/// `horizontal` = 主题当前是横流（`LayoutType::HorizontalSqueeze`）：
/// 候选窗右侧放不下时，符号条改贴**同步面板右侧**、和同步一起放到候选窗上方/下方
///（用户 2026-11-07 21:1x 的 ④；见 `dock_point`）。
pub fn open_symbol(anchor: Option<(i32, i32, i32, i32)>, horizontal: bool) {
    crate::diagnostics::record(format_args!("[panel] open_symbol horizontal={horizontal}"));
    open(Kind::Symbol, anchor, horizontal);
}
pub fn open_sync(anchor: Option<(i32, i32, i32, i32)>) {
    crate::diagnostics::record(format_args!("[panel] open_sync"));
    open(Kind::Sync, anchor, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-11-07 用户口径：剪贴板贴候选窗左、符号贴右、同步贴候选窗**上方/下方**；
    // 拖动吸附「只有几何中心进入**本面板自己的**停靠矩形才吸，一一对应不互换位置」。

    /// 预览实测的候选窗矩形：1168×158 @ (406,461)。
    fn anchor() -> (i32, i32, i32, i32) {
        (406, 461, 1574, 619)
    }
    const SCREEN: (i32, i32) = (1920, 1080);

    #[test]
    fn sync_docks_tight_above_candidate_when_room() {
        let (x, y) = dock_point(Kind::Sync, anchor(), (380, 270), SCREEN, false);
        assert_eq!((x, y), (406, 461 - 270), "同步默认贴紧候选窗上方，横向左沿对齐");
    }

    #[test]
    fn sync_falls_below_when_no_room_above() {
        // 候选窗顶 = 200，上方只剩 200 < 270 ⇒ 翻到下方贴紧（下沿 = 候选窗下沿）。
        let (x, y) = dock_point(Kind::Sync, (406, 200, 1574, 358), (380, 270), SCREEN, false);
        assert_eq!((x, y), (406, 358));
    }

    #[test]
    fn sync_clamps_inside_screen_when_neither_side_fits() {
        // 上方 100 < 270、下方 1080-900 = 180 < 270 ⇒ 夹屏：下侧空间更大 → 贴屏幕底。
        let (x, y) = dock_point(Kind::Sync, (0, 100, 1920, 900), (380, 270), SCREEN, false);
        assert_eq!((x, y), (0, 1080 - 270));
        // 反过来：上侧空间更大（100 > 1080-1080=0? 用 ab=1040 时下方 40，上方 120）→ 贴屏幕顶。
        let (_, y2) = dock_point(Kind::Sync, (0, 120, 1920, 1040), (380, 270), SCREEN, false);
        assert_eq!(y2, 0);
    }

    #[test]
    fn clipboard_docks_left_of_candidate() {
        // 左沿 800 够放 400 宽（800-400-8 = 392）。
        let (x, y) = dock_point(Kind::Clipboard, (800, 461, 1568, 619), (400, 460), SCREEN, false);
        assert_eq!((x, y), (392, 461), "剪贴板贴候选窗左侧，竖直顶对齐");
    }

    #[test]
    fn clipboard_flips_right_when_left_overflow() {
        let (x, _) = dock_point(Kind::Clipboard, (300, 461, 1068, 619), (400, 460), SCREEN, false);
        assert_eq!(x, 1068 + 8, "左边放不下 ⇒ 翻到候选窗右侧");
    }

    #[test]
    fn clipboard_aligns_bottom_when_below_overflow() {
        let (_, y) = dock_point(Kind::Clipboard, (800, 700, 1568, 858), (400, 460), SCREEN, false);
        assert_eq!(y, 858 - 460, "下方装不下 ⇒ 与候选窗底对齐");
    }

    #[test]
    fn symbol_docks_right_of_candidate() {
        // 候选窗 200..800，右侧 808 + 470 = 1278 ≤ 1920 ⇒ 贴右。
        assert_eq!(
            dock_point(Kind::Symbol, (200, 461, 800, 619), (470, 360), SCREEN, false),
            (808, 461)
        );
        // 横流同样先试右侧（放得下就不动）。
        assert_eq!(
            dock_point(Kind::Symbol, (200, 461, 800, 619), (470, 360), SCREEN, true),
            (808, 461)
        );
    }

    #[test]
    fn symbol_never_flips_left_when_right_overflows() {
        // 用户 2026-11-07 21:1x ③：候选窗右侧放不下时**不许**翻到左侧 ——
        // 旧行为会落到 1000 - 470 - 8 = 522，正好压在剪贴板面板位上。
        let (x, y) = dock_point(Kind::Symbol, (1000, 461, 1500, 619), (470, 360), SCREEN, false);
        assert!(x >= 1000, "不许翻到候选窗左侧：x={x}");
        assert_eq!(x, 1450, "贴右夹屏（1920 - 470）");
        assert_eq!(y, 461 - 360, "上下停靠：上方放得下 ⇒ 贴候选窗上方");
    }

    #[test]
    fn symbol_horizontal_docks_right_of_sync_above() {
        // 用户 2026-11-07 21:1x ④：横流 + 右侧放不下 ⇒ 符号条在**同步面板右侧**，
        // 和同步一起放到候选窗上方。同步面板宽 460、左沿对齐候选窗左沿。
        let (x, y) = dock_point(Kind::Symbol, (800, 461, 1600, 619), (470, 360), SCREEN, true);
        assert_eq!(x, 800 + 460 + 8, "横流兜底：符号在同步面板右侧");
        assert_eq!(y, 461 - 360, "一起放到候选窗上方");
    }

    #[test]
    fn symbol_horizontal_falls_below_when_above_overflow() {
        // 横流 + 右侧放不下 + 上方也放不下 ⇒ 一起放到候选窗下方（x 仍在同步面板右侧）。
        let (x, y) = dock_point(Kind::Symbol, (800, 100, 1600, 258), (470, 360), SCREEN, true);
        assert_eq!(x, 800 + 460 + 8);
        assert_eq!(y, 258, "上方放不下 ⇒ 贴候选窗下方");
    }

    // ── 符号格尺寸（用户 2026-11-07 21:2x：都是 1.5em / padding .25 / 字 1em）──────

    #[test]
    fn all_symbol_cells_are_1_5em() {
        // 表情与文本符号**同一套格子**（em=20 ⇒ 30px 见方），表情只多开彩色字体。
        assert_eq!(sym_cell_size("😀", 20.0, 5.0), (30.0, 30.0));
        assert_eq!(sym_cell_size("★", 20.0, 5.0), (30.0, 30.0));
        assert_eq!(sym_cell_size("↑", 20.0, 5.0), (30.0, 30.0), "窄字符也按 1.5em 格子");
    }

    #[test]
    fn emoji_detection_does_not_hit_text_symbols() {
        // 值域表情（😀 🙏 ❄ 带 VS16）算表情；★ ✦ ■ ← 是普通文本符号。
        assert!(is_emoji_char('😀', false));
        assert!(is_emoji_char('❄', true), "带 U+FE0F 的 ❄️ 才按彩色表情画");
        assert!(!is_emoji_char('★', false));
        assert!(!is_emoji_char('■', false));
        assert!(!is_emoji_char('←', false));
    }

    #[test]
    fn symbol_cell_width_follows_glyph_width() {
        // 宽 = max(1.5em, 字形宽 + 2×padding)：多字符符号自动加宽，单枚不低于 1.5em。
        assert_eq!(sym_cell_size("，", 20.0, 5.0).0, 30.0, "单枚全角 = 1.5em");
        assert_eq!(sym_cell_size("「」", 20.0, 5.0).0, 50.0, "两枚全角 = 2em + 2×pad");
    }

    #[test]
    fn symbol_strip_height_is_1_5em_for_all_groups() {
        // 表情分类和纯文本分类的条高一样（都 1.5em）—— 面板高度不再随分类变。
        let emoji = vec!["😀".to_string(), "🙏".to_string()];
        let text = vec!["，".to_string(), "。".to_string()];
        assert_eq!(sym_strip_height(&emoji, 20.0, 5.0), 30.0);
        assert_eq!(sym_strip_height(&text, 20.0, 5.0), 30.0);
        assert_eq!(
            symbol_panel_height(30.0),
            34.0 + TAB_H + SEARCH_H + 30.0 + SYM_GAP * 2.0 + 12.0
        );
    }

    #[test]
    fn symbol_fits_beside_uses_candidate_width() {
        // 判断口径 = 候选窗整宽（面板默认宽就是它）。
        assert!(symbol_fits_beside((200, 461, 800, 619), 1920), "800+8+600 = 1408");
        assert!(!symbol_fits_beside((1000, 461, 1600, 619), 1920), "1600+8+600 > 1920");
        assert!(!symbol_fits_beside((406, 461, 1574, 619), 1920), "预览 1168 宽：1574+8+1168 > 1920");
    }

    #[test]
    fn snap_only_when_center_enters_own_dock_rect() {
        // 同步面板停在自身停靠矩形内（中心算出在矩形里）⇒ 吸到停靠点。
        let a = anchor();
        let dock = dock_point(Kind::Sync, a, (380, 270), SCREEN, false); // (406, 191)
        let center = (500 + 190, 250 + 135);
        assert_eq!(snap_point(Kind::Sync, a, (380, 270), SCREEN, center, false), Some(dock));
        // 中心刚进左边界也算（含边界）。
        assert_eq!(
            snap_point(Kind::Sync, a, (380, 270), SCREEN, (dock.0, dock.1), false),
            Some(dock)
        );
        // 中心在停靠矩形外 ⇒ 自由拖动。
        assert_eq!(snap_point(Kind::Sync, a, (380, 270), SCREEN, (100, 100), false), None);
    }

    #[test]
    fn snap_does_not_use_other_kinds_dock_rect() {
        // 一一对应：同步面板的中心落在**剪贴板**的停靠矩形里，但不在同步自己的矩形里
        // ⇒ 不吸附（不许互换位置）。剪贴板停靠矩形 = (392,461,400,460)。
        let a = (800, 461, 1568, 619);
        let clip = dock_point(Kind::Clipboard, a, (400, 460), SCREEN, false); // (392, 461)
        // 同步面板 380×270 放在 (600,700) ⇒ 中心 (790, 835)：
        // 在剪贴板矩形 [392..792]×[461..921] 内，但同步矩形在候选窗上方 ⇒ 不在其内。
        let center = (600 + 190, 700 + 135);
        let inside_clip = center.0 >= clip.0
            && center.0 <= clip.0 + 400
            && center.1 >= clip.1
            && center.1 <= clip.1 + 460;
        assert!(inside_clip, "测试前提：中心确实落在剪贴板矩形内");
        assert_eq!(snap_point(Kind::Sync, a, (380, 270), SCREEN, center, false), None);
    }

    // ── 配色：面板接入 theme ──────────────────────────────────────────

    /// D2D 颜色 → ARGB（测试断言用）。
    fn argb(c: D2D_COLOR_F) -> u32 {
        let ch = |f: f32| (f * 255.0).round().clamp(0.0, 255.0) as u32;
        (ch(c.a) << 24) | (ch(c.r) << 16) | (ch(c.g) << 8) | ch(c.b)
    }

    /// steam 配色（`Rime-Next/theme/default/default.theme.yaml:843`，解析后 alpha 已补 FF）。
    fn steam() -> ColorScheme {
        let mut s = ColorScheme::default();
        s.back_color = 0xFF14_1617;
        s.border_color = 0xFF35_3638;
        s.text_color = 0xFFCD_8C52;
        s.candidate_text_color = 0xFFFF_FFFF;
        s.comment_text_color = 0xFFA7_A7A9;
        s.hilited_candidate_back_color = 0xFF59_4231;
        s
    }

    #[test]
    fn panel_palette_follows_steam_theme() {
        let pal = pal_from_scheme(&steam());
        assert_eq!(argb(pal.bg), 0xFF14_1617, "面板底 = 主题 back_color（不透明）");
        assert_eq!(argb(pal.ink), 0xFFFF_FFFF, "正文 = 主题候选字色");
        assert_eq!(argb(pal.mute), 0xFFA7_A7A9, "次要文字 = comment_text_color");
        assert_eq!(argb(pal.accent), 0xFFCD_8C52, "强调 = text_color（steam 橙）");
        assert_eq!(argb(pal.hover), 0xFF59_4231, "悬停底 = hilited_candidate_back_color");
        // 可读性：正文/次要/强调压在各自底色上都要读得出来。
        assert!(color_readable(argb(pal.ink), argb(pal.bg)));
        assert!(color_readable(argb(pal.mute), argb(pal.bg)));
        assert!(color_readable(argb(pal.accent), argb(pal.bar)));
    }

    #[test]
    fn panel_palette_keeps_light_theme_readable() {
        // 浅色主题：白底 + 主题给的白候选字 ⇒ 正文回落中性深墨，不能白字白底。
        let mut s = ColorScheme::default();
        s.back_color = 0xFFFF_FFFF;
        s.text_color = 0xFF00_0000;
        s.candidate_text_color = 0xFFFF_FFFF;
        s.comment_text_color = 0;
        s.border_color = 0;
        s.hilited_candidate_back_color = 0;
        let pal = pal_from_scheme(&s);
        assert_eq!(argb(pal.bg), 0xFFFF_FFFF);
        assert!(color_readable(argb(pal.ink), argb(pal.bg)), "浅底必须用深墨");
        assert!(color_luma(argb(pal.ink)) < 0.5);
        assert!(color_readable(argb(pal.mute), argb(pal.bg)));
    }

    #[test]
    fn default_palette_is_readable() {
        assert!(color_readable(argb(DEFAULT_PAL.ink), argb(DEFAULT_PAL.bg)));
        assert!(color_readable(argb(DEFAULT_PAL.mute), argb(DEFAULT_PAL.bg)));
        assert!(color_readable(argb(DEFAULT_PAL.accent), argb(DEFAULT_PAL.bar)));
    }

    // ── 原型 v3：符号标签条按分类名聚合 ──────────────────────────────

    fn group(code: &str, label: &str, items: &[&str]) -> SymbolGroup {
        SymbolGroup {
            code: code.into(),
            label: label.into(),
            items: items.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn symbol_tabs_merge_groups_sharing_the_same_label() {
        // symbols.yaml 里一条分类注释覆盖多个输入码：原型标签条要合并成**一个**「符號」标签，
        // 不能出现「符號 符號 象棋 象棋」这种重复标签（2026-11-07 live 截图就是这个毛病）。
        let groups = vec![
            group("/fh", "符號、電腦", &["©", "®"]),
            group("/dn", "符號、電腦", &["⌘", "®"]),
            group("/xq", "象棋、麻將", &["♔"]),
        ];
        let tabs = build_sym_tabs(&groups);
        assert_eq!(tabs.len(), 2, "同分类应合并成一个标签");
        assert_eq!(tabs[0].title, "符號");
        assert_eq!(tabs[0].items, vec!["©", "®", "⌘"], "合并后按出现顺序去重");
        assert_eq!(tabs[0].icon, "©", "标签图标 = 该分类第一个符号");
        assert_eq!(tabs[0].codes, vec!["/fh", "/dn"], "同分类的输入码都要保留，供符号搜索");
        assert_eq!(tabs[1].title, "象棋");
    }

    #[test]
    fn symbol_tabs_keep_fallback_group_first() {
        // 读不到 symbols.yaml 时用内置兜底表：第一个标签必须是「常用」，且每组都有符号。
        let groups = symbols::load(std::path::Path::new("C:/__no_such_dir__"), None);
        let tabs = build_sym_tabs(&groups);
        assert!(tabs.len() >= 2, "兜底表至少 2 个分类，实际 {}", tabs.len());
        assert_eq!(tabs[0].title, "常用");
        assert!(tabs.iter().all(|t| !t.items.is_empty() && !t.icon.is_empty()));
    }

    // ── 2026-11-07「123 都要做」三项功能的纯函数回归 ──────────────────

    #[test]
    fn clipboard_card_has_seven_actions_in_one_row() {
        // 用户 2026-11-07「123 都要做」：卡片动作 5 → 7（补「加密」「云同步」）。
        assert_eq!(CARD_ACTS, 7);
        let (x0, aw) = card_act_geometry_for(430.0, 5.0);
        let last_right = x0 + (CARD_ACTS as f32 - 1.0) * (aw + SYM_GAP) + aw;
        assert!(aw >= 48.0, "7 个二字按钮每个至少 ~48px，实际 {aw}");
        assert!(last_right <= 430.0 - 5.0, "最后一个按钮不能越出卡片右沿：{last_right}");
    }

    #[test]
    fn locked_clip_is_masked_until_unlocked() {
        // 加密条目仍要显示（打码），不再整条隐藏；本会话解锁后显示原文。
        assert_eq!(clip_mask_text(true, false, "银行密码"), "🔒 已加密");
        assert_eq!(clip_mask_text(true, true, "银行密码"), "银行密码");
        assert_eq!(clip_mask_text(false, false, "普通文本"), "普通文本");
    }

    #[test]
    fn sync_mode_and_interval_cycle_through_all_steps() {
        // 同步方式：坚果云 ⇄ 通用 WebDAV（切通用时清空服务器让用户填）。
        let mut server = "https://dav.jianguoyun.com/dav/".to_string();
        assert_eq!(sync_mode_label(&server), "坚果云 WebDAV");
        cycle_sync_mode(&mut server);
        assert!(server.is_empty());
        assert_eq!(sync_mode_label(&server), "通用 WebDAV");
        cycle_sync_mode(&mut server);
        assert!(server.contains("jianguoyun"));
        // 同步频率：仅手动 → 30 分钟 → 每小时 → 每天 → 仅手动（0/30/60/1440）。
        assert_eq!(cycle_interval(0), 30);
        assert_eq!(cycle_interval(30), 60);
        assert_eq!(cycle_interval(60), 1440);
        assert_eq!(cycle_interval(1440), 0);
        assert_eq!(interval_label(0), "仅手动");
        assert_eq!(interval_label(1440), "每天");
    }

    #[test]
    fn receive_filter_keeps_clipboard_before_userdb() {
        let mut receive = vec!["clipboard".to_string(), "userdb".to_string()];
        set_receive(&mut receive, "clipboard", false);
        assert_eq!(receive, vec!["userdb"]);
        set_receive(&mut receive, "clipboard", true);
        assert_eq!(receive, vec!["clipboard", "userdb"], "剪贴板要排回 userdb 前面");
        // 重复置位不会产生重复项。
        set_receive(&mut receive, "userdb", true);
        assert_eq!(receive, vec!["clipboard", "userdb"]);
        let mut one = vec!["userdb".to_string()];
        set_receive(&mut one, "clipboard", true);
        assert_eq!(one, vec!["clipboard", "userdb"]);
    }

    #[test]
    fn symbol_search_matches_code_and_title() {
        // ② 符号搜索：分类短名 / 输入码（去掉 `/`）都能命中；字符命中走 item 分支。
        let codes = vec!["/fh".to_string(), "/dn".to_string()];
        assert!(sym_title_matches("符號", &codes, "fh"));
        assert!(sym_title_matches("符號", &codes, "符號"));
        assert!(!sym_title_matches("符號", &codes, "象棋"));
        assert!(!sym_title_matches("符號", &codes, "©"), "单字符不能靠分类命中");
    }

    // ── 2026-11-07 用户口径「面板常驻」回归 ────────────────────────────────
    // 「点同步/符号/剪贴板里的东西不消失，可以正常打字/选候选，输入结束也不消失，
    //   可以拖动、可以点击，只有点关闭或退出服务才消失。」

    #[test]
    fn only_close_button_closes_persistent_panel() {
        assert!(click_closes_panel(Hit::Close), "关闭钮才是关闭操作");
        assert!(!click_closes_panel(Hit::Cell(0)), "点符号上屏 ≠ 关面板");
        assert!(!click_closes_panel(Hit::Card(0)), "点剪贴板卡片上屏 ≠ 关面板");
        assert!(!click_closes_panel(Hit::CardAction(0, 5)), "复制/收藏/删除也不关");
        assert!(!click_closes_panel(Hit::Tab(1)), "切分类/标签不关");
        assert!(!click_closes_panel(Hit::TabPrev), "分类翻页不关");
        assert!(!click_closes_panel(Hit::TabNext), "分类翻页不关");
        assert!(!click_closes_panel(Hit::Search), "点搜索框不关");
        assert!(!click_closes_panel(Hit::SyncField(0)), "点同步表单文本域不关");
        assert!(!click_closes_panel(Hit::SyncAction(2)), "同步/测试/保存不关");
        assert!(!click_closes_panel(Hit::SyncToggle(0)), "点表单开关不关");
        assert!(!click_closes_panel(Hit::SyncCheck(0)), "点接收过滤勾不关");
        assert!(!click_closes_panel(Hit::ModalInput), "密码层输入框不关");
        assert!(!click_closes_panel(Hit::ModalOk), "密码层确定（上屏）不关");
        assert!(!click_closes_panel(Hit::ModalCancel), "密码层取消只关模态层");
        assert!(!click_closes_panel(Hit::None), "空白处不关");
        // 2026-11-07 22:3x 用户口径收紧：Esc 也不关；编码栏同一按钮再点只置顶
        // （`open()` 里那个「同类再点 → DestroyWindow」的分支已按口径删除）。
        assert!(!esc_closes_panel(), "Esc 不关面板");
    }

    #[test]
    fn service_shutdown_is_the_only_other_close_path() {
        // `close_all_panels` 只在主题后端销毁（渲染器/服务退出）时调用；
        // 它同时清掉候选窗句柄，避免面板再往已销毁的窗口投上屏消息。
        set_candidate_hwnd(4242, 25.0);
        assert_eq!(CANDIDATE_HWND.load(Ordering::SeqCst), 4242);
        close_all_panels();
        assert_eq!(CANDIDATE_HWND.load(Ordering::SeqCst), 0, "退出服务后句柄清空");
        assert!(!any_panel_open());
    }

    // ── 2026-11-07 22:0x 用户「密码和用户帐号粘贴不了」回归 ──────────────────

    #[test]
    fn paste_text_filters_control_chars_and_limits() {
        // 剪贴板常带换行/制表：单行字段要去掉控制字符。
        assert_eq!(sanitized_paste_text("2481036245@qq.com\r\n", 40), "2481036245@qq.com");
        assert_eq!(sanitized_paste_text("a\tb\nc", 40), "abc");
        // 首尾空白去掉（复制时常见的尾随空格）。
        assert_eq!(sanitized_paste_text("  pass word  ", 40), "pass word");
        // 按字段上限截断（账号 40 / 应用密码 32）。
        assert_eq!(sanitized_paste_text(&"字".repeat(50), 32).chars().count(), 32);
        // 全是空白/控制字符 ⇒ 空串（调用方据此判定「没粘到东西」）。
        assert!(sanitized_paste_text("\r\n\t ", 40).is_empty());
        // 字段上限表：密码框比其余字段短。
        assert_eq!(target_max_chars(EditTarget::Sync(SyncField::Pass)), 32);
        assert_eq!(target_max_chars(EditTarget::Sync(SyncField::User)), 40);
        assert_eq!(target_max_chars(EditTarget::Modal), MODAL_MAX_CHARS);
    }
}
