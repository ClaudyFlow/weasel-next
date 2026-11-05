//! 官方 `WeaselPanel` 的绘制流程：`DoPaint` / `_HighlightRect` / `_DrawCandidates` /
//! `_DrawPreedit` / `_TextOut`（行号指 `C:\Code\weasel\WeaselUI\WeaselPanel.cpp`）。
//!
//! 与官方一致的几条硬约束：
//!
//! * **一次绘制三层**（`_HighlightRect`，L799）：先阴影（`FillGeometry(..., to_blur)` 走
//!   高斯模糊），再底色（圆角路径填充），最后描边（同一路径 `DrawGeometry`，线宽 = border，
//!   半径内缩 border/2）。
//! * **逐角圆角**由 `RoundInfo` 决定（`CreateRoundedRectanglePath`，d2d.cpp:828）：
//!   半径是三次贝塞尔近似，控制点系数 0.382。
//! * **文字用 `CreateTextLayout`**，并把 `GetOverhangMetrics` 的正向溢出补进偏移
//!   （L780-789）——不然斜体/异体字的左边会被切掉。
//! * 颜色一律来自配色（[`ColorScheme`]），**不透明才画**（官方 `COLORNOTTRANSPARENT`）。

use crate::d2d_bindings::*;
use crate::theme::config::{Hover, LayoutType, Metrics};
use crate::theme::layout::{row_padding, Layout, Rect, RoundInfo, PAGE_NEXT, PAGE_PREV};
use crate::theme_api::CandidateView;
use weasel_style::ColorScheme;
use windows_core::Interface;

/// 材质档位：`%APPDATA%\Rime-Next\theme\material.txt` → `WEASEL_MATERIAL` → 默认 glass。
///
/// 设置界面把用户选的档位写进 material.txt；文件不存在时才看环境变量。
pub fn material() -> String {
    std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .map(|base| base.join("Rime-Next").join("theme").join("material.txt"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .or_else(|| std::env::var("WEASEL_MATERIAL").ok())
        .unwrap_or_default()
}

/// 候选窗材质档位（2026-11-02 用户定：**只出一个 DLL，三档在设置里切**）。
///
/// 以前这三档是编译期 `--features`，安装目录里并排放着 `theme-glass.dll` /
/// `theme-acrylic.dll` / `theme-original.dll` 三个产物，换档得换文件。现在统一成
/// 运行时读 [`material`]，一个 DLL 全包；老值 `plain` 仍然当透明档（兼容旧配置）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    /// 透明：抓屏 + 自绘高斯模糊，面板底**全透明**（底下只剩糊过的那张，不染色）。
    Glass,
    /// 亚克力：同样抓屏模糊（糊得更狠），再铺一层 85% 的**配色蒙版** —— 一眼看得出有颜色、
    /// 比玻璃「实」得多，`back_color` 的明暗 + 配色个性色的色相（见 [`theme_veil`]）。
    Acrylic,
    /// 默认（设置里叫「默认」，2026-11-04 10:50 之前叫"不透明"）：用 yaml 的
    /// `back_color` 铺满，不抓屏、不模糊、不留投影边 —— 就是主题本来的样子。
    Opaque,
    /// 金属（老档，只为兼容旧的 `material.txt`）：不透明哑光 + 上白下黑渐变。
    Metal,
}

impl Material {
    /// 是否走「抓屏 + 自绘模糊」那一套（= 老代码里的 `plain` 档）。
    ///
    /// 影响三件事：窗口度量要不要收掉投影边、配色要不要换成浅底版本、面板底铺什么。
    pub fn flat(self) -> bool {
        matches!(self, Material::Glass | Material::Acrylic)
    }

    /// 从 `material.txt` 的原始值解析档位；未知值一律当透明档。
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "acrylic" => Material::Acrylic,
            // 设置界面 2026-11-04 10:50 起把这档叫「默认」，所以 `default` 也认；
            // `opaque` / `original` / `solid` 是它以前的名字，旧文件继续有效。
            "default" | "opaque" | "original" | "solid" => Material::Opaque,
            "metal" => Material::Metal,
            // "glass" / "plain" / 空 / 垃圾值 —— 都是透明档
            _ => Material::Glass,
        }
    }
}

/// 当前材质档位（读 `material.txt` → 解析）。绘制/度量都用它。
pub fn tier() -> Material {
    Material::parse(&material())
}

/// 抓屏模糊/亚克力档（`flat`）的配色 —— 面板底不再写死白色，字色**自动**跟着
/// 实际能看见的背景变（用户 2026-11-04：「字体自动变色」「亚克力识别配色」）。
///
/// 两个档位的"有效背景"不一样，所以分开算：
///
/// * **玻璃**：面板全透明，背后就是抓屏 + 高斯模糊的那张图 ⇒ 直接用它算出来的
///   平均亮度；抓不到背景（没有抓屏权限/老系统）时 `Painter::draw` 会退回 50% 白，
///   这里也按白底估。
/// * **亚克力**：背后是自绘高斯模糊（比玻璃糊一倍多，见 [`blur_sigma`]），面板再压一层
///   **配色蒙版**（[`VEIL_ALPHA`] 不透明，色相取配色的个性色、明度跟 `back_color`，
///   见 [`theme_veil`]）。桌面亮度事先不知道，就取"桌面纯黑 / 纯白"两种极端，要求两种情况下对比度都够 —— 否则深色壁纸上
///   是"深字压深底"，浅色壁纸上是"白字压白底"，总有一边看不清。
///
/// 主题自己的颜色**优先**：只有对比度掉到门槛以下才翻成兜底墨色，绝不无条件强制白/黑
/// （用户 18:2x：「识别 weasel 的 theme」）。
pub fn plain_scheme(
    base: &ColorScheme,
    tier: Material,
    backdrop_luma: Option<f32>,
) -> ColorScheme {
    /// 对比度不够时的兜底深色字（HTML 的 `#1b1d22`）。
    const INK: u32 = 0xFF_1B_1D_22;
    /// 兜底浅色字。
    const INK_LIGHT: u32 = 0xFF_F2_F3_F5;
    const MUTED: u32 = 0xFF_6B_72_80;
    const MUTED_LIGHT: u32 = 0xFF_C9_CE_D6;
    /// 高亮框底：HTML `.hilite` 的 `#3b4a63`；字用高对比色。
    const HILITE_BG: u32 = 0xFF_3B_4A_63;
    /// 正文/候选的最低对比度（WCAG AA 正文）。
    const TEXT_MIN: f32 = 4.5;
    /// 注释、编码、翻页箭头这类次要文字可以松一点。
    const MUTED_MIN: f32 = 3.0;

    /// 平均亮度（0..255），用来给"只知道一坨背景"的场景估色。
    fn luma(argb: u32) -> f32 {
        let r = ((argb >> 16) & 0xff) as f32;
        let g = ((argb >> 8) & 0xff) as f32;
        let b = (argb & 0xff) as f32;
        0.299 * r + 0.587 * g + 0.114 * b
    }
    /// sRGB 分量 → 线性。
    fn linear(value: u32) -> f32 {
        let v = (value & 0xff) as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    /// WCAG 相对亮度。
    fn rel(argb: u32) -> f32 {
        0.2126 * linear((argb >> 16) & 0xff)
            + 0.7152 * linear((argb >> 8) & 0xff)
            + 0.0722 * linear(argb & 0xff)
    }
    /// 把"平均亮度"当成灰色，取它的相对亮度。
    fn gray(luma: f32) -> f32 {
        let v = luma.clamp(0.0, 255.0).round() as u32;
        rel((v << 16) | (v << 8) | v)
    }
    /// WCAG 对比度（1:1 ~ 21:1）。
    fn contrast(a: f32, b: f32) -> f32 {
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
    /// 在**每一种可能的背景**下的最小对比度（最坏情况）。
    fn worst_contrast(color: u32, backgrounds: &[f32]) -> f32 {
        backgrounds
            .iter()
            .map(|bg| contrast(rel(color), gray(*bg)))
            .fold(f32::MAX, f32::min)
    }
    /// 配色自己的颜色优先；最坏情况都掉到 `min` 以下才换成兜底墨色。
    fn keep_or(color: u32, backgrounds: &[f32], min: f32, fallback: u32) -> u32 {
        if worst_contrast(color, backgrounds) >= min {
            color
        } else {
            fallback
        }
    }

    let backgrounds: [f32; 2] = match tier {
        // 亚克力：配色蒙版（它的亮度，不是 back_color 的）+ 桌面两端。
        // alpha 直接从**蒙版颜色自身**取 —— 它已经含了 `veil.txt` 的覆盖值，
        // 这样"算字色用的不透明度"和"绘制时真正铺上去的"永远是同一个数字。
        Material::Acrylic => {
            let veil_color = theme_veil(base);
            let veil_luma = luma(veil_color);
            let veil = ((veil_color >> 24) & 0xFF) as f32 / 255.0;
            [veil_luma * veil, veil_luma * veil + 255.0 * (1.0 - veil)]
        }
        // 玻璃：真的有一张糊过的背景图就当它准；没有就按视觉上的白底估。
        _ => [backdrop_luma.unwrap_or(255.0); 2],
    };
    // 兜底墨色取"最坏情况下更清楚"的那一边。
    let ink = if worst_contrast(INK, &backgrounds) >= worst_contrast(INK_LIGHT, &backgrounds) {
        INK
    } else {
        INK_LIGHT
    };
    let muted = if ink == INK { MUTED } else { MUTED_LIGHT };

    let mut scheme = base.clone();
    // 面板底保留**配色自己的底色**：亚克力档要拿它做蒙版的明度基准（「颜色按 theme 走」），
    // 玻璃档虽然全透明，模式提示之类的小窗也还要用它染色。
    // HTML 的面板没有描边、没有投影；边框和阴影全透明（透明就不画）。
    scheme.border_color = 0;
    scheme.shadow_color = 0;
    scheme.text_color = keep_or(base.text_color, &backgrounds, TEXT_MIN, ink);
    scheme.candidate_text_color = keep_or(base.candidate_text_color, &backgrounds, TEXT_MIN, ink);
    scheme.label_color = keep_or(base.label_color, &backgrounds, TEXT_MIN, ink);
    scheme.comment_text_color =
        keep_or(base.comment_text_color, &backgrounds, MUTED_MIN, muted);
    scheme.candidate_back_color = 0;
    scheme.candidate_border_color = 0;
    scheme.candidate_shadow_color = 0;
    // **高亮（小候选窗）用 theme.yaml 里的颜色**（用户 2026-11-02：「按 yaml 文件里的颜色
    // 做小候选窗的颜色」）。主题给了不透明的 hilited_candidate_back_color 就用它，
    // 没给（或全透明）才退回内置的 #3b4a63。
    let theme_hilite = base.hilited_candidate_back_color;
    let hilite_bg = if (theme_hilite >> 24) >= 0x40 {
        (0xFF00_0000 | (theme_hilite & 0x00FF_FFFF)) as u32
    } else {
        HILITE_BG
    };
    scheme.hilited_candidate_back_color = hilite_bg;
    // 高亮块上的字要对的是**高亮块底色**，不是面板背景 —— 两边分开算对比度。
    let hilite_luma = [luma(hilite_bg)];
    let hilite_ink = if worst_contrast(INK, &hilite_luma) >= worst_contrast(INK_LIGHT, &hilite_luma)
    {
        INK
    } else {
        INK_LIGHT
    };
    let theme_hilite_text = base.hilited_candidate_text_color;
    let hilite_text = if (theme_hilite_text >> 24) >= 0x40 {
        keep_or(theme_hilite_text, &hilite_luma, MUTED_MIN, hilite_ink)
    } else {
        hilite_ink
    };
    scheme.hilited_text_color = hilite_text;
    scheme.hilited_candidate_text_color = hilite_text;
    scheme.hilited_label_color = hilite_text;
    scheme.hilited_comment_text_color = hilite_text;
    scheme.hilited_candidate_border_color = 0;
    scheme.hilited_candidate_shadow_color = 0;
    scheme.prevpage_color = keep_or(base.prevpage_color, &backgrounds, MUTED_MIN, muted);
    scheme.nextpage_color = keep_or(base.nextpage_color, &backgrounds, MUTED_MIN, muted);
    scheme
}

/// 主题要用的四个 DirectWrite 文本格式（官方 `D2D::pPreeditFormat` / `pTextFormat` /
/// `pLabelFormat` / `pCommentFormat`）。
pub struct Formats {
    pub preedit: IDWriteTextFormat,
    pub text: IDWriteTextFormat,
    pub label: IDWriteTextFormat,
    pub comment: IDWriteTextFormat,
}

/// 绘制上下文：一帧所需的全部引用。
pub struct Painter<'a> {
    pub target: &'a ID2D1RenderTarget,
    pub factory: &'a ID2D1Factory,
    pub write: &'a IDWriteFactory,
    pub brush: &'a ID2D1SolidColorBrush,
    pub formats: &'a Formats,
    pub m: &'a Metrics,
    pub scheme: &'a ColorScheme,
    pub view: &'a CandidateView,
    /// 当前悬停候选（官方窗口成员 `m_hoverIndex`）。
    pub hover_index: Option<usize>,
    /// 是否启用系统毛玻璃：为真时面板底色降到半透明，让背后的 Acrylic 模糊透出来。
    pub glass: bool,
    /// 窗口句柄（`0` = 没给；自绘模糊的抓屏已挪到显示前，这里只留诊断用）。
    pub hwnd: isize,
    /// 用自绘高斯模糊代替 DWM 亚克力（`style/glass_blur: "self"`）。
    pub glass_self: bool,
    /// 亚克力蒙版的颜色（[`theme_veil`] 的结果，含 alpha）；`0` = 没给，按当前配色现算。
    ///
    /// mod 侧拿**原始配色**（`plain_scheme` 之前的那份）算好传进来，保证"算字色用的蒙版"
    /// 和"画出来的蒙版"是同一个颜色 —— 否则算得挺好却看不清。
    pub veil: u32,
    /// 显示前抓好的模糊背景缓存（mod 侧 [`BackdropCache`]；`None` = 没抓到）。
    pub backdrop: Option<&'a BackdropCache>,
    /// 高亮滑动动画：`(当前矩形, 目标下标)`；`None` = 不播动画。
    /// 矩形已经由模组侧按 CSS 语义插值好（只滑位置、宽高取目标），这里只管画。
    pub hilite_anim: Option<(Rect, usize)>,
}

// ── 挤压三模式固定配色（设计稿 `_shots/ime-three-modes.html`）──────────────
// 刻意不读 theme.yaml 的 37 套配色：这套壳自带灰/白/橙，叠配色只会互相打架；
// 而且配色解析失败回退默认值时（INI 残留那次），挤压壳不该跟着一起黑掉。
const SQ_SHELL: u32 = 0xFF9A_9A9F; // 灰壳
const SQ_PAPER: u32 = 0xFFF5_F5F7; // 白内板
const SQ_CELL_BG: u32 = 0xFFE8_EAF0; // 编码字符格底
const SQ_GRID: u32 = 0xFFC9_CED6; // 字符格线
const SQ_ORANGE: u32 = 0xFFF5_9E0B; // 候选橙底
const SQ_ORANGE_HI: u32 = 0xFFEA_580C; // 选中橙底
const SQ_CORE: u32 = 0xFFFF_FFFF; // 核白板
const SQ_RED: u32 = 0xFFE5_484D; // 候选间红块
const SQ_GREEN: u32 = 0xFF22_C55E; // 分隔绿条
const SQ_LINE: u32 = 0xFF11_1827; // 绿条上的黑线
const SQ_INK: u32 = 0xFF1F_2937; // 正文墨色
const SQ_NUM: u32 = 0xFF9C_A3AF; // 序号 / 注释灰

/// 显示前抓好的模糊背景（自绘高斯模糊的缓存单元）。
pub struct BackdropCache {
    pub width: i32,
    pub height: i32,
    pub bitmap: ID2D1Bitmap,
    /// 抓到的背景平均亮度（0..255）。面板不再铺白色之后，用它决定文字用深色还是浅色。
    pub luma: u32,
}

/// 模式提示图标的文件位置：`<程序文件夹>\Rime-Next\icon\{zh,en}-{dark,light}.ico`。
///
/// 官方把这两枚图标编在 WeaselUI 的资源里（`IDI_EN` / `IDI_ZH`，`LoadIconIfNeed`
/// 允许用主题的 `ascii_icon` / `zhung_icon` 覆盖）；我们仓库里是独立的 .ico，
/// 按"底色深浅"挑浅色/深色那一套。
fn status_icon_path(ascii: bool, dark_background: bool) -> Option<std::path::PathBuf> {
    let dir = weasel_common::rime_layout::data_dir(std::env::current_exe().ok()?.parent()?)
        .join("icon");
    let name = match (ascii, dark_background) {
        (true, true) => "en-light.ico",
        (true, false) => "en-dark.ico",
        (false, true) => "zh-light.ico",
        (false, false) => "zh-dark.ico",
    };
    let path = dir.join(name);
    path.is_file().then_some(path)
}

/// 官方 `D2D::GetCachedBmpFromIcon`：把 HICON 变成可以 `DrawBitmap` 的位图。
///
/// GDI 只能把图标画到 DC 上，所以走一张 32bpp 自顶向下的 DIB 中转；图标本身带
/// alpha，而 D2D 要求**预乘** alpha —— `DrawIconEx` 出来的值是否预乘取决于图标
/// 编码，这里逐像素判断（通道值大于 alpha 说明还没预乘）后再预乘。
fn load_status_icon(
    target: &ID2D1RenderTarget,
    pixels: u32,
    ascii: bool,
    dark_background: bool,
) -> Option<ID2D1Bitmap> {
    let path = status_icon_path(ascii, dark_background)?;
    let wide: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(path.as_os_str())
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let raw = LoadImageW(
            None,
            windows_core::PCWSTR(wide.as_ptr()),
            IMAGE_ICON as u32,
            pixels as i32,
            pixels as i32,
            LR_LOADFROMFILE as u32,
        );
        if raw.0.is_null() {
            return None;
        }
        let hicon = HICON(raw.0);
        let bitmap = icon_to_bitmap(target, hicon, pixels);
        let _ = DestroyIcon(hicon);
        bitmap
    }
}

/// HICON → 32bpp DIB → `ID2D1Bitmap`（[`load_status_icon`] 的实现细节）。
unsafe fn icon_to_bitmap(
    target: &ID2D1RenderTarget,
    icon: HICON,
    pixels: u32,
) -> Option<ID2D1Bitmap> {
    unsafe {
        let mut info = ICONINFO::default();
        if !GetIconInfo(icon, &mut info).as_bool() {
            return None;
        }
        let mask = info.hbmMask;
        let color = info.hbmColor;
        let release = || {
            if !mask.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(mask.0));
            }
            if !color.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(color.0));
            }
        };

        let dc = CreateCompatibleDC(None);
        if dc.0.is_null() {
            release();
            return None;
        }
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = core::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = pixels as i32;
        bmi.bmiHeader.biHeight = -(pixels as i32); // 负高度 = 自顶向下，与 D2D 行序一致
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB as u32;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(
            Some(dc),
            &bmi,
            DIB_RGB_COLORS as u32,
            &mut bits,
            None,
            0,
        );
        if dib.0.is_null() || bits.is_null() {
            let _ = DeleteDC(dc);
            release();
            return None;
        }

        let previous = SelectObject(dc, HGDIOBJ(dib.0));
        let drawn = DrawIconEx(
            dc,
            0,
            0,
            icon,
            pixels as i32,
            pixels as i32,
            0,
            None,
            DI_NORMAL as u32,
        )
        .as_bool();
        let _ = SelectObject(dc, previous);

        let mut result = None;
        if drawn {
            let bytes = std::slice::from_raw_parts_mut(
                bits as *mut u8,
                (pixels as usize) * (pixels as usize) * 4,
            );
            for pixel in bytes.chunks_exact_mut(4) {
                let (blue, green, red, alpha) = (pixel[0], pixel[1], pixel[2], pixel[3]);
                if red > alpha || green > alpha || blue > alpha {
                    let scale = |channel: u8| ((channel as u32 * alpha as u32) / 255) as u8;
                    pixel[0] = scale(blue);
                    pixel[1] = scale(green);
                    pixel[2] = scale(red);
                }
            }
            let properties = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            result = target
                .CreateBitmap(
                    D2D_SIZE_U {
                        width: pixels,
                        height: pixels,
                    },
                    Some(bits as *const core::ffi::c_void),
                    pixels * 4,
                    &properties,
                )
                .ok();
        }

        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(dc);
        release();
        result
    }
}

impl<'a> Painter<'a> {
    /// 画模式提示图标（官方 `WeaselPanel::OnPaint` 里的 `GetCachedBmpFromIcon` 分支）。
    ///
    /// 官方加载的是 32×32 的 HICON（`SM_CXICON`），中/英各一枚；我们把仓库里的
    /// `icon\zh-*.ico` / `icon\en-*.ico` 走同一条路：`LoadImage` → `GetIconInfo` →
    /// 32bpp DIB（`DrawIconEx`）→ `ID2D1Bitmap`。图标读不到时返回 `false`，
    /// 调用方退化成画文字。
    ///
    /// `pixels` 是图标边长（物理像素 = 32 × DPI 缩放），`dark_background` 决定
    /// 用深色底还是浅色底那一套图标。
    pub fn draw_mode_icon(&self, rect: &Rect, ascii: bool, pixels: u32, dark_background: bool) -> bool {
        let Some(bitmap) = load_status_icon(self.target, pixels, ascii, dark_background) else {
            return false;
        };
        unsafe {
            self.target.DrawBitmap(
                &bitmap,
                Some(&D2D_RECT_F {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                }),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
        true
    }

    /// 组装一帧的绘制上下文。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        target: &'a ID2D1RenderTarget,
        factory: &'a ID2D1Factory,
        write: &'a IDWriteFactory,
        brush: &'a ID2D1SolidColorBrush,
        formats: &'a Formats,
        m: &'a Metrics,
        scheme: &'a ColorScheme,
        view: &'a CandidateView,
        glass: bool,
    ) -> Self {
        Self {
            target,
            factory,
            write,
            brush,
            formats,
            m,
            scheme,
            view,
            hover_index: None,
            glass,
            hwnd: 0,
            glass_self: false,
            veil: 0,
            hilite_anim: None,
            backdrop: None,
        }
    }
}

/// 把面板底色降到半透明，让背后的系统毛玻璃（DWM Acrylic）透出来。
///
/// 仅压低 alpha：完全不透明的面板（如官方 aqua 的 `0xFFxxxxxx`）降到
/// [`GLASS_ALPHA`]，本就半透明的配色保持原样。文字仍是实色绘制，可读性不受影响。
pub(crate) const GLASS_ALPHA: u32 = 0x33;

/// 亚克力档面板上那层**配色蒙版**的不透明度（0..255）。
///
/// 这段百分比调过三轮，根因值得记下来：09:4x 用户说「亚克力怎么是透明的」时它**就是**
/// `0x80`（50%），于是一路往上加到 `0xD8`（85%）；10:2x 用户仍说「这是玻璃的效果啊」——
/// 真凶其实是**颜色被面积加权平均洗成了中性灰**，跟不透明度关系不大。改完
/// [`theme_veil`] 的"借色相不借明度"（10:5x）后，10:41 回调到 `0xCC`（80%）。
///
/// 11:10 用户：「亚克力做成 50 透明」⇒ 回到 **`0x80`（50%）**。有配色染色 + σ=8 打底，
/// 50% 已经是"透光不透影"：背景的明暗起伏明显能感知，但认不出内容，也不再像玻璃那样清透。
///
/// 三档的"透明度"口径（面板底的不透明占比）：
/// 透明档 **0%**（面板底全透明，只剩抓屏模糊图；抓不到屏幕才退回 50% 白）
/// · 亚克力 **50%** · 默认（原"不透明"）**100%**（yaml 的 `back_color` 铺满）。
///
/// `plain_scheme` 用它算"有效背景亮度"（决定字色），`Painter::draw` 用它铺面板 ——
/// 两处必须是同一个值，否则字色按 A 算、底按 B 画，看着就会"算得挺好却看不清"。
/// 运行时可用 [`veil_alpha`]（`veil.txt`）覆盖。
pub(crate) const VEIL_ALPHA: u32 = 0x80;

/// 蒙版不透明度的**实际生效值**：`VEIL_ALPHA` 可被
/// `%APPDATA%\Rime-Next\theme\veil.txt`（一个 30..95 的百分数）实时覆盖。
///
/// 和 `material.txt` / `blur.txt` 同一套机制：每次用时现读，改完下一次显示候选窗就生效，
/// 不用重编译、不用重装 DLL。调"亚克力多透"这种主观手感，用户自己滑比来回改代码快：
/// 写 `40` 更透（偏玻璃）、写 `80` 更实（接近不透明）。
///
/// 30 以下就真成玻璃了（背后什么都看得见）；95 以上磨砂感消失、跟不透明没区别 ——
/// 两头都当"没写"，用档位默认值。
pub fn veil_alpha() -> u32 {
    let tuned = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .map(|base| base.join("Rime-Next").join("theme").join("veil.txt"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| parse_veil_pct(&text));
    tuned.unwrap_or(VEIL_ALPHA)
}

/// `veil.txt` 的内容 → alpha。容忍结尾的 `%` 和空白；超出 30..95 的当没写。
fn parse_veil_pct(text: &str) -> Option<u32> {
    text.trim()
        .trim_end_matches('%')
        .parse::<f32>()
        .ok()
        .filter(|pct| (30.0..=95.0).contains(pct))
        .map(|pct| (pct / 100.0 * 255.0).round() as u32)
}

/// 玻璃档的抓屏模糊强度（**降采样后**的像素标准差；1/2 降采样 ⇒ 屏幕 σ 翻倍）。
const GLASS_SIGMA: f32 = 3.0;
/// 亚克力档的抓屏模糊强度：同一个屏幕区域糊得比玻璃狠一倍多。
///
/// 用户 2026-11-04 09:4x：「再模糊一些」。DWM 的亚克力模糊半径是系统写死的（不能调），
/// 所以亚克力档也改走自绘高斯 —— 半径、强度、染色全在自己手里。
/// σ=8 ⇒ 屏幕 σ≈16px（1/2 降采样），3σ 半径接近 50px：背后再大的色块也只剩一片渐变。
const ACRYLIC_SIGMA: f32 = 8.0;

/// 当前档位的抓屏模糊强度（σ，降采样后像素）。
///
/// 可以用 `%APPDATA%\Rime-Next\theme\blur.txt`（一个数字）**实时覆盖** —— 和
/// `material.txt` 一样是每次用时现读，改完下一次显示候选窗就生效，不用重编译、
/// 不用重装 DLL。想"再模糊一些"就写个更大的数（6 → 9），觉得糊过了就写小。
/// 留空/没有这个文件时按档位默认值。
pub fn blur_sigma(tier: Material) -> f32 {
    let tuned = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .map(|base| base.join("Rime-Next").join("theme").join("blur.txt"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| text.trim().parse::<f32>().ok())
        // 0 或负数会让"模糊"退化成原图，太大则是几百次卷积 —— 都当没写。
        .filter(|sigma| *sigma > 0.0 && *sigma <= 48.0);
    tuned.unwrap_or(match tier {
        Material::Acrylic => ACRYLIC_SIGMA,
        _ => GLASS_SIGMA,
    })
}

/// 亚克力蒙版：**底色打底 + 配色的个性色**（[`VEIL_ALPHA`] 不透明）。
///
/// 用户 2026-11-04 10:2x：「亚克力是这个效果，不那么透明，然后颜色按 theme 走」。
///
/// 上一版是按"面积"加权平均整套配色 —— 问题在于**平均必然把颜色洗掉**：steam 的底色
/// `#141617` 权重 8、橙色文字 `#CD8C52` 只有 0.25，混出来 `RGB(34,35,40)` 跟中性灰没差别，
/// 于是亚克力看着跟透明档一模一样（用户：「这是玻璃的效果啊」）。
///
/// 现在改成**借色不借亮**：
///
/// 1. 从整套配色里挑**饱和度最高**的那个当"个性色"（steam = 文字橙 `#CD8C52`）；
///    整套都是中性灰（纯黑白皮肤）就挑不出来 ⇒ 蒙版 = 底色，绝不凭空造色；
/// 2. 取它的**色相**，饱和度打对折、封顶 0.35（免得像糖果）；
/// 3. **明度用底色自己的**（再抬 0.06，让面板像一块"面"而不是一个"洞"）。
///
/// 这样深色皮肤糊出来还是深的（明度跟底色走），但一定带上皮肤自己的色偏 ——
/// "颜色按 theme 走"，而不会把深底压成浅底。
///
/// ⚠️ 这里的"个性色"取的是**内部 ARGB**（`weasel_style` 已经把官方 `abgr` 字节序
/// 翻译过了）。所以 yaml 里写着 `0xcd8c52`（看着像橙）的 steam，进到这里的字色其实是
/// `0xFF528CCD`（蓝）—— 蒙版自然跟着蓝走，跟屏幕上文字的实际颜色一致，这才是对的。
pub fn theme_veil(base: &ColorScheme) -> u32 {
    let alpha = veil_alpha();
    let back = base.back_color & 0x00FF_FFFF;
    let (_, _, back_l) = hsl(back);
    // 挑不出个性色（无彩配色）：蒙版就是底色本身。半透明底色取不到就退回纯黑，
    // 蒙版至少还是一块实色，不会让面板整个消失。
    let Some((hue, sat)) = accent_hue(base) else {
        return (alpha << 24) | back;
    };
    let light = if back_l < 0.5 {
        (back_l + 0.06).min(0.42)
    } else {
        (back_l - 0.04).max(0.58)
    };
    let tint = from_hsl(hue, (sat * 0.5).min(0.35), light);
    (alpha << 24) | tint
}

/// 配色里的"个性色"：饱和度最高、且明度不在两端的那个（返回 `(色相°, 饱和度)`）。
///
/// 只有**不透明**（alpha ≥ 0x40）的项参与：没设颜色的键是 `0x00000000`，
/// 拿它算色相会得到"黑色"，白白把整块蒙版拉黑。
fn accent_hue(base: &ColorScheme) -> Option<(f32, f32)> {
    let palette = [
        base.text_color,
        base.candidate_text_color,
        base.hilited_text_color,
        base.hilited_candidate_text_color,
        base.label_color,
        base.hilited_label_color,
        base.comment_text_color,
        base.hilited_comment_text_color,
        base.nextpage_color,
        base.prevpage_color,
        base.hilited_candidate_back_color,
        base.hilited_back_color,
        base.candidate_back_color,
        base.border_color,
        base.hilited_border_color,
        base.hilited_mark_color,
    ];
    let mut best: Option<(f32, f32)> = None;
    for color in palette {
        if (color >> 24) < 0x40 {
            continue;
        }
        let (hue, sat, light) = hsl(color & 0x00FF_FFFF);
        // 灰阶（含纯黑/纯白）没有"色"可借；明度贴到两端的也不算，缩放后看不出来。
        if sat < 0.15 || !(0.12..=0.92).contains(&light) {
            continue;
        }
        if best.map_or(true, |(_, best_sat)| sat > best_sat) {
            best = Some((hue, sat));
        }
    }
    best
}

/// `0xRRGGBB` → `(色相 0..360, 饱和度 0..1, 明度 0..1)`。
fn hsl(rgb: u32) -> (f32, f32, f32) {
    let r = ((rgb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((rgb >> 8) & 0xFF) as f32 / 255.0;
    let b = (rgb & 0xFF) as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let light = (max + min) / 2.0;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0, light);
    }
    let sat = if light > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if max == r {
        ((g - b) / delta + if g < b { 6.0 } else { 0.0 }) * 60.0
    } else if max == g {
        ((b - r) / delta + 2.0) * 60.0
    } else {
        ((r - g) / delta + 4.0) * 60.0
    };
    (hue, sat, light)
}

/// `(色相 0..360, 饱和度 0..1, 明度 0..1)` → `0xRRGGBB`。
fn from_hsl(hue: f32, sat: f32, light: f32) -> u32 {
    let h = hue.rem_euclid(360.0) / 60.0;
    let chroma = (1.0 - (2.0 * light - 1.0).abs()) * sat;
    let second = chroma * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let base = light - chroma / 2.0;
    let channel = |value: f32| ((value + base).clamp(0.0, 1.0) * 255.0).round() as u32;
    (channel(r) << 16) | (channel(g) << 8) | channel(b)
}

/// 玻璃态的**亮度层**（scrim）：铺在 tint 之下，压住背景穿透。
///
/// 只有一层 20% 的 tint 时，背后的桌面/网页纹理会整片透上来 ⇒ 面板发灰发脏、
/// 文字读不清（截图里那种"murky"）。Fluent Acrylic 的 `luminosityOpacity` 就是
/// 干这个的：tint 保留玻璃的色调，亮度层负责把背景压暗到能读字。
/// 叠完后整体不透明度 ≈ 1-(1-0.20)*(1-0.63) ≈ 70%，还透得出一点背景。
pub(crate) const GLASS_SCRIM: u32 = 0xA00A_0C10; // 深蓝黑（跟手绘玻璃底 #141617 同调）

/// 取亮度层颜色；`WEASEL_GLASS_SCRIM` 可以只覆盖 alpha（十六进制，如 `B0`），
/// 用来在实机上直接调深浅，不用重新编译。
fn glass_scrim() -> u32 {
    std::env::var("WEASEL_GLASS_SCRIM")
        .ok()
        .and_then(|text| u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok())
        .map(|alpha| ((alpha & 0xff) << 24) | (GLASS_SCRIM & 0x00ff_ffff))
        .unwrap_or(GLASS_SCRIM)
}

pub(crate) fn glassify(argb: u32) -> u32 {
    let alpha = (argb >> 24) & 0xff;
    let alpha = alpha.min(GLASS_ALPHA);
    (alpha << 24) | (argb & 0x00ff_ffff)
}

/// RGB 分量乘系数（压暗）。
fn darken_color(argb: u32, factor: f32) -> u32 {
    let r = (((argb >> 16) & 0xFF) as f32 * factor) as u32 & 0xFF;
    let g = (((argb >> 8) & 0xFF) as f32 * factor) as u32 & 0xFF;
    let b = ((argb & 0xFF) as f32 * factor) as u32 & 0xFF;
    (r << 16) | (g << 8) | b
}

/// RGB 分量往白里混（提亮）。
fn lighten_color(argb: u32, amount: f32) -> u32 {
    let mix = |channel: u32| ((channel as f32) + (255.0 - channel as f32) * amount).min(255.0) as u32;
    (mix((argb >> 16) & 0xFF) << 16) | (mix((argb >> 8) & 0xFF) << 8) | mix(argb & 0xFF)
}

/// 内部 ARGB（`0xAARRGGBB`）→ Direct2D 颜色。
pub fn d2d_color(argb: u32) -> D2D_COLOR_F {
    D2D_COLOR_F {
        r: ((argb >> 16) & 0xff) as f32 / 255.0,
        g: ((argb >> 8) & 0xff) as f32 / 255.0,
        b: (argb & 0xff) as f32 / 255.0,
        a: ((argb >> 24) & 0xff) as f32 / 255.0,
    }
}

/// 官方 `COLORTRANSPARENT` / `COLORNOTTRANSPARENT`：alpha 为 0 即全透明。
fn transparent(argb: u32) -> bool {
    (argb >> 24) == 0
}

/// 官方 `HALF_ALPHA_COLOR`：把 alpha 减半，用于半高亮悬停。
fn half_alpha(argb: u32) -> u32 {
    ((((argb >> 24) / 2) & 0xff) << 24) | (argb & 0x00ff_ffff)
}

/// 官方 `CreateRoundedRectanglePath`（d2d.cpp:828）：逐角决定是否画圆角。
pub fn rounded_rect_path(
    factory: &ID2D1Factory,
    rect: &Rect,
    radius: f32,
    info: &RoundInfo,
) -> windows_core::Result<ID2D1PathGeometry> {
    unsafe {
        let path = factory.CreatePathGeometry()?;
        let sink = path.Open()?;
        // SetFillMode 声明在基接口上
        let simplified: ID2D1SimplifiedGeometrySink = sink.cast()?;
        simplified.SetFillMode(D2D1_FILL_MODE_WINDING);
        let point = |x: f32, y: f32| windows_numerics::Vector2 { x: x, y: y };
        if radius <= 0.0 {
            sink.BeginFigure(point(rect.left, rect.top), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(point(rect.right, rect.top));
            sink.AddLine(point(rect.right, rect.bottom));
            sink.AddLine(point(rect.left, rect.bottom));
            sink.AddLine(point(rect.left, rect.top));
        } else {
            let radius = radius
                .min(rect.width() / 2.0)
                .min(rect.height() / 2.0)
                .max(0.0);
            let good = 0.382 * radius;
            if info.top_left {
                sink.BeginFigure(point(rect.left + radius, rect.top), D2D1_FIGURE_BEGIN_FILLED);
            } else {
                sink.BeginFigure(point(rect.left, rect.top), D2D1_FIGURE_BEGIN_FILLED);
            }
            sink.AddLine(point(rect.right - radius, rect.top));
            if info.top_right {
                sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                    point1: point(rect.right - good, rect.top),
                    point2: point(rect.right, rect.top + good),
                    point3: point(rect.right, rect.top + radius),
                });
            } else {
                sink.AddLine(point(rect.right, rect.top));
            }
            sink.AddLine(point(rect.right, rect.bottom - radius));
            if info.bottom_right {
                sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                    point1: point(rect.right, rect.bottom - good),
                    point2: point(rect.right - good, rect.bottom),
                    point3: point(rect.right - radius, rect.bottom),
                });
            } else {
                sink.AddLine(point(rect.right, rect.bottom));
            }
            sink.AddLine(point(rect.left + radius, rect.bottom));
            if info.bottom_left {
                sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                    point1: point(rect.left + good, rect.bottom),
                    point2: point(rect.left, rect.bottom - good),
                    point3: point(rect.left, rect.bottom - radius),
                });
            } else {
                sink.AddLine(point(rect.left, rect.bottom));
            }
            sink.AddLine(point(rect.left, rect.top + radius));
            if info.top_left {
                sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                    point1: point(rect.left, rect.top + good),
                    point2: point(rect.left + good, rect.top),
                    point3: point(rect.left + radius, rect.top),
                });
            } else {
                sink.AddLine(point(rect.left, rect.top));
            }
        }
        simplified.EndFigure(D2D1_FIGURE_END_CLOSED);
        let _ = simplified.Close();
        Ok(path)
    }
}

fn d2d_rect(rect: &Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

impl Painter<'_> {
    /// 官方 `DoPaint`（WeaselPanel.cpp:423）。
    pub fn draw(&self, layout: &Layout, empty_context: bool) -> Result<(), String> {
        // 挤压三模式走自带皮肤的独立帧路径；下面官方几百行一行未动 ——
        // 非 squeeze 逐字节回原路径，零回归（用户 2026-11-05「外科手术式」）。
        if self.m.squeeze {
            // 兜底：挤压绘制若 panic（或返回 Err），不返回、让控制流落回下面的官方
            // `{}` 块（逐字节未动），保证候选窗绝不消失
            // （2026-11-05 竹简「没有界面」= draw_squeeze 首帧崩了把 Renderer 拖死）。
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.draw_squeeze(layout, empty_context)
            }));
            if let Ok(Ok(())) = r {
                return Ok(());
            }
            // 否则继续执行下面的官方绘制（不 return）
        }
        {
            let style = self.m;
            let inline = layout.inline_preedit;
            let has_candidates = !layout.rows.is_empty();
            // 官方 should_draw_background（L438-440）
            let draw_background = (!empty_context && !style.inline_preedit)
                || (style.inline_preedit && (has_candidates || !layout.aux.is_empty()));
            if draw_background {
                // 材质三档（用户 2026-11-02 定名）：glass / acrylic / opaque，
                // **运行时**读 `%APPDATA%\Rime-Next\theme\material.txt`（设置界面「外观」页写），
                // 不再是编译期 feature。老值 `plain` = glass，`metal` = 老金属档。
                let tier = tier();
                let flat = tier.flat();
                let back_color = match tier {
                    // 不透明：yaml 的 back_color 铺满（原来编译期 `--features original` 那一档）。
                    Material::Opaque => {
                        (0xFF00_0000 | (self.scheme.back_color & 0x00FF_FFFF)) as u32
                    }
                    // 金属：不透明哑光，下面再叠上白下黑的渐变。
                    Material::Metal => {
                        0xFF00_0000 | (darken_color(self.scheme.back_color, 0.78) & 0x00FF_FFFF)
                    }
                    // 亚克力：模糊底上再铺**配色蒙版**（`VEIL_ALPHA` 不透明）。
                    // 演变：写死白 0x80FF_FFFF → 配色底 back_color（「亚克力识别配色」）
                    // → 整配色加权平均 + 0xC0（09:4x「要识别整个 theme 的颜色」，
                    //   但平均把颜色洗成中性灰 ⇒ 看着还是玻璃）
                    // → **底色明度 + 个性色色相 + 0xD8**（10:2x「不那么透明，
                    //   然后颜色按 theme 走」）。
                    Material::Acrylic => {
                        if self.veil != 0 {
                            self.veil
                        } else {
                            theme_veil(self.scheme)
                        }
                    }
                    // 玻璃：**不要白色**（用户 18:1x「不要白色了，直接透明」）。
                    // 抓不到背景（没抓屏权限/老系统）时才退回 50% 白，否则面板会整个消失。
                    Material::Glass => {
                        if self.backdrop.is_some() {
                            // 自绘模糊：底下就是那张糊过的图，白色 tint 全透明
                            0x0000_0000
                        } else if self.glass || self.glass_self {
                            // DWM 亚克力模式：面板仍要 20% 染色，否则 DComp 逐像素透明
                            // 被不透明 back_color 盖死（用户 12:1x：为什么不透明）
                            glassify(self.scheme.back_color)
                        } else {
                            0x80FF_FFFF
                        }
                    }
                };
                // 玻璃态：面板画满**整个窗口矩形**，而不是内缩 `offset` 的内框。
                // 否则窗口比面板大一圈，那圈被 DWM 亚克力填满 ⇒ 边框外面多出一层灰（用户 13:4x 报的"还有一层"）。
                // 代价：投影没有落脚处（`offset` 那圈就是给投影的），玻璃态干脆不要投影。
                // 抓屏模糊档（透明/亚克力）：面板画满**整个窗口矩形**。
                // 以前用 `layout.content`（比窗口小一圈 offset），那一圈没东西可画
                // ⇒ 露出**没糊的桌面**，跟面板里糊过的背景在边界上形成一道"黑框"
                // （用户 17:5x）。`Metrics` 里 flat 已经把投影留边收成 0，这里画满即可。
                let back_rect = if flat || self.glass {
                    Rect::new(
                        0.0,
                        0.0,
                        layout.content_size.cx,
                        layout.content_size.cy,
                    )
                } else {
                    layout.content
                };
                // 抓屏模糊档：把**抓屏 + 高斯模糊**的位图铺在面板底下，再叠白 tint —— 毛玻璃。
                // 2026-11-01 17:0x 之前这张位图抓了却从没被画过（`draw_backdrop` 是死代码），
                // 所以面板底下透出来的是**没糊的桌面**（用户："做高斯模糊"）。
                if flat {
                    self.draw_backdrop(&back_rect, self.backdrop);
                }
                if self.glass_self && !flat {
                    self.draw_hand_backdrop(&back_rect, style.round_corner_ex);
                }
                // 亮度层：先铺深色压住背景纹理，再叠皮肤色 tint。
                // 少了这层，20% 的面板叠在什么背景上都是一片浑浊的灰。
                if (self.glass || self.glass_self) && !flat {
                    self.fill_geometry(
                        &back_rect,
                        glass_scrim(),
                        style.round_corner_ex,
                        &RoundInfo::default(),
                        false,
                    );
                }
                self.highlight_rect(
                    &back_rect,
                    style.round_corner_ex,
                    style.border,
                    back_color,
                    // 自绘模糊这档**不画投影**：投影画在 offset 那圈里，而窗口就是面板本身，
                    // 投影会从圆角外侧顶出一圈深色方框（20:5x 用户截图里的"黑边"）。
                    if (self.glass || flat) { 0 } else { self.scheme.shadow_color }, // 模糊档不要投影，否则面板外一圈黑
                    // 描边一律用**配色自己的** border_color。
                    //
                    // 这里原来是玻璃白高光 `if (self.glass || self.glass_self) && !flat
                    // { 0x8CFF_FFFF }`，但 `self.glass` 读的是 theme.yaml 的 `style/glass`
                    // （默认 true）——**不透明档它也是 true**，于是 3px 的 55% 白描边照样
                    // 画在不透明面板外面，就是用户 2026-11-04 00:5x 截图里的"外面一圈白的"。
                    //
                    // 2026-11-04 11:31 用户拍板：「直接去掉这个边框」——
                    // 面板不再描边，圆角外侧只有一块干净的纯色/蒙版。三档因此彻底一致
                    // （透明/亚克力本来就在 `plain_scheme` 里把 border_color 清成 0）。
                    // `border` 参数照旧传进去：它同时决定路径内缩量，传 0 只是不画而已。
                    // 模式提示小窗（`draw_mode_indicator`）仍保留描边 —— 那是图标窗口，
                    // 用户当初明确要过"边框"。
                    0,
                    &RoundInfo::default(),
                );
                if tier == Material::Metal {
                    // 金属：**分段渐变**（用户 23:1x：4:2:3:1 的黑白黑白，但每一段之间要有过渡）。
                    // 做法：给出若干控制点，段内线性插值 ⇒ 既保留 4:2:3:1 的比例，又不再是刀切边。
                    let h = back_rect.height().max(1.0);
                    let info = RoundInfo::default();
                    // (位置, 亮度)：暗→亮→暗→亮的 4 段，边界用 4% 宽度做斜坡
                    // 用户 23:2x 拍板：**上白下黑**，就是一条竖直渐变（顶部最亮 → 底部最暗）。
                    // 之前那套 3:2:1:4 的黑白黑白分段全撤掉。
                    // 用户 23:2x 定：**上 30% 纯白 → 10% 渐变 → 60% 纯黑**（正好 100%）
                    // 用户 23:2x：白色还是多 ⇒ **12% 纯白 + 8% 渐变 + 80% 纯黑**
                    const POINTS: [(f32, f32); 4] = [
                        (0.00, 1.00), // 0–12%   纯白
                        (0.12, 1.00), //         渐变起点
                        (0.20, 0.10), // 12–20%  渐变到黑
                        (1.00, 0.10), // 20–100% 纯黑（80%）
                    ];
                    let steps = 96;
                    for index in 0..steps {
                        let t = index as f32 / (steps - 1) as f32;
                        // 找所在区间并线性插值
                        let mut lit: f32 = POINTS[POINTS.len() - 1].1;
                        for pair in POINTS.windows(2) {
                            let (t0, v0) = pair[0];
                            let (t1, v1) = pair[1];
                            if t <= t1 && t >= t0 {
                                let k = if (t1 - t0).abs() < 1e-6 { 1.0 } else { (t - t0) / (t1 - t0) };
                                lit = v0 + (v1 - v0) * k;
                                break;
                            }
                        }
                        let alpha = (lit.clamp(0.0, 1.0) * 0x9Eu32 as f32) as u32;
                        if alpha == 0 {
                            continue;
                        }
                        let top = back_rect.top + h * t;
                        let bottom = back_rect.top + h * ((index + 1) as f32 / steps as f32) + 0.5;
                        self.fill_geometry(
                            &Rect::new(back_rect.left, top, back_rect.right, bottom),
                            (alpha << 24) | 0xFF_FFFF,
                            0.0,
                            &info,
                            false,
                        );
                    }
                    // 顶部/底部 1px 硬边收口
                    self.fill_geometry(&Rect::new(back_rect.left, back_rect.top, back_rect.right, back_rect.top + 1.0), 0xCCFF_FFFF, 0.0, &info, false);
                    self.fill_geometry(&Rect::new(back_rect.left, back_rect.bottom - 1.0, back_rect.right, back_rect.bottom), 0x8800_0000, 0.0, &info, false);
                }
            }
            // 预编辑（L452-457）：非 inline 且有内容才画
            if !inline && self.view.preedit.is_some() {
                self.draw_preedit(layout, true)?;
            }
            if has_candidates {
                self.draw_candidates(layout)?;
            }
            if layout.show_status_icon {
                self.draw_status_icon(layout);
            }
            // 翻页提示独立画：不依赖 preedit、也不要求配色给翻页色。
            self.draw_page_buttons(layout);
        }
        Ok(())
    }

    /// 挤压三模式整帧绘制：灰壳 / 白内板 / 字符格编码 / 橙底白核 / 红块 / 绿分隔。
    ///
    /// 官方 `draw()` 在 `m.squeeze` 时把整帧交给这里，所以不会与材质三档、
    /// 玻璃蒙版、金属渐变那套互相覆盖。所有矩形都取自 `Layout`（窗口尺寸已经
    /// 按 squeeze 算好），只换颜色与装饰，绝不自己改坐标 —— 不会被窗口裁掉。
    fn draw_squeeze(&self, layout: &Layout, empty_context: bool) -> Result<(), String> {
        let _ = empty_context; // 自带壳始终画满；官方「空上下文不画背景」不适用于这里
        let info = RoundInfo::default();
        let shell = Rect::new(0.0, 0.0, layout.content_size.cx, layout.content_size.cy);
        if shell.is_empty() {
            return Ok(());
        }
        let round = self.m.round_corner_ex.max(0.0);
        // 灰壳那一圈就是 **padding**（HTML `.ime` 的 padding: 0.25em）。
        // 白内板是直角矩形，所以 padding 至少要把「外壳圆角被切掉的那块」让出来，
        // 否则白板的直角会从灰壳的圆角里戳出去：要求 lip ≥ R(1 − 1/√2) ≈ 0.293R。
        // 公式在 `Metrics::squeeze_lip`（layout 排按钮组/箭头时要拿同一个数顶右沿）。
        let lip = self.m.squeeze_lip(shell.width());

        // ① 灰壳（外圈 = padding）→ ② 白内板（内容区，**直角**）
        self.fill_geometry(&shell, SQ_SHELL, round, &info, false);
        let mut paper = shell;
        paper.deflate(lip, lip);
        if !paper.is_empty() {
            self.fill_geometry(&paper, SQ_PAPER, 0.0, &info, false);
        }

        // ③ 编码栏：左侧字符格拼音 + 右侧按钮组（🔣 ✂ 🔄）+ 翻页箭头（见 ⑤），
        //    下方一条 0.5em 绿横分隔（设计稿 `.ime-bar` / `.hsep`）。
        //
        //    这里量 1em 用 `font_point`（= config 里的 font_h），和 layout 内部
        //    「圆角/间距全部按字高推导」的 em 同口径；`em` 那个变量是绘制用的放大值，
        //    拿它算按钮会整体偏大 1/3，两边就对不齐了。
        let bar_em = self.m.font_point.max(8.0);
        let preedit_text = self
            .view
            .preedit
            .as_ref()
            .map(|p| p.text.as_str())
            .filter(|t| !t.is_empty());
        if !layout.inline_preedit && !layout.preedit.is_empty() {
            if let Some(text) = preedit_text {
                // 设计稿的编码栏**没有字符格**：`.ch` 只是给每个字定宽的 flex 项，
                // 没有底色也没有格线（`.bar-inner` 是纯白）。旧实现画了浅灰格底 +
                // 逐格竖线，跟设计稿对不上，2026-11-05 按 `.ime-bar` 去掉。
                self.text_out(&layout.preedit, text, SQ_INK, &self.formats.preedit)?;
            }
            // 绿分隔贴**编码栏底**，不是贴文字底：栏高 1.5em 而文字带只有 1em，
            // 按文字底画会把分隔条切进栏里、把 0.5em 的空白挤掉。
            // 2026-11-05 18:5x：布局那边已经把「候选区顶」放在绿分隔下沿，这里**不能**
            // 再 +1（会把整条分隔往下推 1px，和第一只框的顶沿错开）。
            let bar_bottom = if layout.preedit_row.is_empty() {
                layout.preedit.bottom
            } else {
                layout.preedit_row.bottom
            };
            let y = bar_bottom;
            let band = Rect::new(
                paper.left,
                y,
                paper.right,
                (y + self.m.bar_hsep.max(3.0)).min(paper.bottom),
            );
            if !band.is_empty() {
                self.fill_geometry(&band, SQ_GREEN, 0.0, &info, false);
                // 黑线压在绿条**正中**（设计稿 `.hsep::before` 是 flex 居中，不是贴顶）
                let mid = (band.top + band.bottom) / 2.0;
                self.fill_geometry(
                    &Rect::new(band.left, mid - 1.0, band.right, mid + 1.0),
                    SQ_LINE,
                    0.0,
                    &info,
                    false,
                );
            }
        }

        // ④ 候选：橙底（选中更深）+ 核白板 + 序号 / 正文 / 注释。
        //    `layout.highlight_rect()` 已带 per-candidate padding（选中 2p、其余 p），
        //    挤压后的框与鼠标命中矩形天然一致，paint 不用再算一遍。
        let count = layout.rows.len();
        let highlighted = if (self.view.selected_index as usize) < count {
            self.view.selected_index as usize
        } else {
            usize::MAX
        };
        // 三模式在这里分形：竹简=方片+正文字符格，飞瀑=竖列绿横条，横流=单行红块。
        let cells = matches!(self.m.layout_type, LayoutType::Cells);
        let waterfall = matches!(self.m.layout_type, LayoutType::VerticalSqueeze);
        for i in 0..count {
            let row = layout.rows[i];
            let hi = layout.highlight_rect(self.m, i);
            if hi.is_empty() {
                continue;
            }
            // 防御：items 与 rows 长度偶发不一致时跳过该项而不是越界 panic 把 Renderer 拖崩
            // （2026-11-05：竹简首帧报「没有界面」疑似此处越界，改 get 兜底）。
            let Some(item) = self.view.items.get(i) else {
                continue;
            };
            // 候选块（橘红选中 / 橘黄未选中）圆角恒为 **0.25em**（用户 2026-11-05）。
            //
            // 以前三种模式各写一套，而且都不是 0.25em：
            //   横流/飞瀑 = `hi.height() / 2.0` —— 胶囊，框高约 2em 时圆角约 1em；
            //   竹简      = `em * 0.12`       —— 且 `em` 已乘过 `dpi_scale_font`（4/3），
            //                                   实际约 0.16em，还随 DPI 漂移。
            // 现在统一用 `bar_em`（= `font_point`，和 layout 里预算 1em 同口径，
            // **不再**乘 `dpi_scale_font`），三种模式只差一个值来源。
            let radius = (bar_em * 0.25).max(2.0);
            self.fill_geometry(
                &hi,
                if i == highlighted { SQ_ORANGE_HI } else { SQ_ORANGE },
                radius,
                &info,
                false,
            );
            let mut core = hi;
            // 白核（`.core`）同样**直角**。橙底是有圆角的，直角白核必须内缩到圆角之内，
            // 否则四个角会从橙底戳出来（胶囊形半径 R 时要 ≥ 0.293R）。
            //
            // 内缩量 = 本候选自己的橙边厚度，而不是常量 p（2026-11-05 用户按 HTML 对照）：
            //   rim = pad − 0.5p   ⇒  普通 0.25em、选中 0.5em
            // 于是「选中时橙边变粗、白核不变」，与设计稿 `.item.first { padding:.5em }`
            // + `.core { padding:.25em }` 的叠法一致；旧写法用常量 p，选中/未选中的
            // 橙边一样粗、白核跟着外框长，看起来就是"只是换了个颜色"。
            let (pad_x, pad_y) = row_padding(self.m, &row);
            // 三种模式**同一套 padding**（用户 2026-11-06：「按那两个的效果做 padding，
            // 现在太宽了」）：白核 = 外框按 rim 内缩，rim = pad − 0.5p
            // ⇒ 普通 0.25em、选中 0.5em；文字在白核里水平居中、顶部对齐（竹简）/
            // 垂直居中、左对齐（横流、飞瀑）。
            let rim_x = (pad_x - self.m.hilite_padding_x * 0.5).max(0.0);
            let rim_y = (pad_y - self.m.hilite_padding_y * 0.5).max(0.0);
            let inset_x = (radius * 0.3).max(rim_x);
            let inset_y = (radius * 0.3).max(rim_y);
            core.deflate(inset_x, inset_y);
            // 白核限高(2026-11-06 用户实机:"候选词的白框太高了"):
            // 白核只包住文字(1em)+核内边距(0.25em 上下)= 1.5em,
            // 不跟随整个橙底外框高——竖排时外框高度由布局决定可能远大于文字,
            // 白核跟着长就是"字飘在超长白框里"的根因。
            let core_h = bar_em * 1.5;
            if core.height() > core_h {
                let excess = core.height() - core_h;
                // 竹简(内容顶齐):白核贴顶部;横流/飞瀑:白核垂直居中于橙底
                if cells {
                    core.bottom = core.top + core_h;
                } else {
                    core.top += excess / 2.0;
                    core.bottom = core.top + core_h;
                }
            }
            if !core.is_empty() {
                self.fill_geometry(&core, SQ_CORE, 0.0, &info, false);
            }
            let text = &item.primary_text;
            // 竹简：序号写进竖列**首格**，和正文同一条竖线（用户 2026-11-05：
            // 「为什么序号不是垂直的」）。序号格下面留 `hilite_spacing`（Metrics 里恒为
            // font_h / 2 = 0.5em）当间隔，正文再逐字往下 —— 与横流/飞瀑里
            // 「序号右边到正文左边」的那条缝是同一个数（用户：「所有序号和候选词差 0.5em」）。
            let n_body = text.chars().count();
            let gap = self.m.hilite_spacing;
            let mut head = 0.0;
            let mut body = row.text;
            let label_rect = if cells && n_body > 0 && !row.text.is_empty() {
                // head = 序号格高：整列去掉间隔后，按「序号 + 字数」均分
                head = ((row.text.height() - gap).max(1.0) / (n_body + 1) as f32).max(1.0);
                let bottom = row.text.top + head;
                body = Rect::new(
                    row.text.left,
                    (bottom + gap).min(row.text.bottom),
                    row.text.right,
                    row.text.bottom,
                );
                Rect::new(row.text.left, row.text.top, row.text.right, bottom)
            } else {
                row.label
            };
            if !label_rect.is_empty() {
                // 竹简的首格只有一个字宽，`label_format` 默认是 "%s."（会画成 "1."），
                // 横着塞进去会顶出格线，所以竹简里直接用裸序号；其余模式照旧走格式串。
                let label = if cells {
                    (i as u32 + 1).to_string()
                } else {
                    self.m.format_label(&(i as u32 + 1).to_string())
                };
                self.text_out(&label_rect, &label, SQ_NUM, &self.formats.label)?;
            }
            // 序号与正文之间那颗 0.5em 红块：设计稿每项都是「序号 ■ 正文」，
            // `.num` 和正文之间夹一个 `.space`（0.5em 见方）——三种模式都有。
            //   横流 / 飞瀑：横向那道缝（`hilite_spacing` 宽），竖直居中；
            //   竹简      ：序号格与正文之间那道竖向缝，水平居中（`.core` 是 column）。
            let dot = self.m.bar_dot;
            if cells {
                if head > 0.5 && !row.text.is_empty() {
                    let band_top = row.text.top + head;
                    let side = dot.min(gap).max(1.0);
                    let x = (row.text.left + row.text.right) / 2.0 - side / 2.0;
                    let y = band_top + (gap - side) / 2.0;
                    self.fill_geometry(
                        &Rect::new(x, y, x + side, y + side),
                        SQ_RED,
                        0.0,
                        &info,
                        false,
                    );
                }
            } else if !row.label.is_empty() {
                let seam = row.text.left - row.label.right;
                if seam > 1.0 {
                    let side = dot.min(seam).max(1.0);
                    let x = row.label.right + (seam - side) / 2.0;
                    let y = (row.text.top + row.text.bottom) / 2.0 - side / 2.0;
                    self.fill_geometry(
                        &Rect::new(x, y, x + side, y + side),
                        SQ_RED,
                        0.0,
                        &info,
                        false,
                    );
                }
            }
            if !text.is_empty() {
                if cells {
                    // 竹简：竖排逐字（设计稿 `.cell .core { flex-direction: column }`，
                    // 每个字是一个 flex item ⇒ 天然一列）。**不画格线**：
                    // 设计稿里 `.cell` 没有 border，旧实现的 `squeeze_grid_v` 会画出
                    // 一片横线，跟「一模一样」冲突（2026-11-05 按设计稿去掉）。
                    self.text_out_vertical(&body, text, SQ_INK, &self.formats.text)?;
                } else {
                    self.text_out(&row.text, text, SQ_INK, &self.formats.text)?;
                }
            }
            let comment = &item.secondary_text;
            if !comment.is_empty() && !row.comment.is_null() {
                if cells {
                    self.text_out_vertical(&row.comment, comment, SQ_NUM, &self.formats.comment)?;
                } else {
                    self.text_out(&row.comment, comment, SQ_NUM, &self.formats.comment)?;
                }
            }
            // 分隔：相邻两候选之间 0.5em 红块（设计稿 `.row > .space`）。
            // 飞瀑也是红块，只是贴在**列的左边**（`.col > .space { align-self: flex-start }`），
            // 不再画绿横带（2026-11-05 按设计稿改；红/绿在 HTML 里都是调试色，
            // 但形状与位置以设计稿为准）。
            if i + 1 < count {
                let next = layout.highlight_rect(self.m, i + 1);
                let gap = if waterfall {
                    next.top - hi.bottom
                } else {
                    next.left - hi.right
                };
                if gap > 1.0 {
                    // 红块边长 = 0.5em；间隙更小时缩到间隙，别盖到橙框上。
                    let side = dot.min(gap).max(1.0);
                    let (x, y) = if waterfall {
                        (paper.left, hi.bottom + (gap - side) / 2.0)
                    } else {
                        (
                            hi.right + (gap - side) / 2.0,
                            hi.top + (hi.height() - side) / 2.0,
                        )
                    };
                    self.fill_geometry(
                        &Rect::new(x, y, x + side, y + side),
                        SQ_RED,
                        0.0,
                        &info,
                        false,
                    );
                }
            }
        }

        // ⑤ 编码栏右端：按钮组（🔣 符号 / ✂ 剪贴板 / 🔄 同步）+ 翻页箭头。
        //    设计稿 `_shots/ime-three-modes.html` 的 `.ime-bar` 就是这么排的：
        //      拼音 … |vsep| 🔣 ■ ✂ ■ 🔄 |vsep| ◀ ■ ▶
        //    箭头矩形由 layout 给出（官方语义，命中测试也用它们），
        //    按钮组贴着 `page_prev` 的左边往左排，所以点和点自动对齐。
        if !layout.inline_preedit {
            self.draw_bar_buttons(layout)?;
            if !layout.page_prev.is_empty() {
                self.text_out(&layout.page_prev, PAGE_PREV, SQ_INK, &self.formats.preedit)?;
            }
            if !layout.page_next.is_empty() {
                self.text_out(&layout.page_next, PAGE_NEXT, SQ_INK, &self.formats.preedit)?;
            }
        }
        Ok(())
    }

    /// 编码栏右侧的按钮组（设计稿 `.btns` / `.page-btns`）。
    ///
    /// 布局是纯绘制层的：箭头矩形来自 `Layout`（命中测试不变），
    /// 按钮组从 `page_prev.left` 开始向左依次排
    /// `vsep → 🔄 → ■ → ✂ → ■ → 🔣 → vsep`，每个按钮 1em 见方、
    /// 按钮之间夹 0.5em 红块、vsep 是 0.5em 绿底黑竖线。
    ///
    /// 所有尺寸从 `Metrics` 的编码栏统一参数读（见 `config::bar`），
    /// 不再自己算 `em * 倍数`（2026-11-05 统一编码栏参数）。
    fn draw_bar_buttons(&self, layout: &Layout) -> Result<(), String> {
        if layout.page_prev.is_empty() || layout.page_next.is_empty() {
            return Ok(());
        }
        let info = RoundInfo::default();
        // 整组（分隔条 / 按钮 / 红块 / 箭头）共用**编码栏的中线**做垂直居中
        // （用户 2026-11-05：「这些编码分隔线按钮箭头都做成垂直居中」）。
        // 栏高 1.5em 而箭头/按钮的字形只有 ~1em，按各自矩形贴顶排会高低不齐。
        let mid = if layout.preedit_row.is_empty() {
            (layout.page_prev.top + layout.page_prev.bottom) / 2.0
        } else {
            (layout.preedit_row.top + layout.preedit_row.bottom) / 2.0
        };
        // 分隔条高度取 `bar_vsep_h`（= 1em），栏内居中留出 0.25em 上下气。
        let vsep_h = if layout.preedit_row.is_empty() {
            layout.page_prev.height().max(self.m.bar_vsep_h)
        } else {
            layout.preedit_row.height().min(self.m.bar_vsep_h)
        };
        let vsep_top = mid - vsep_h / 2.0;
        // 按钮/红块用自己的见方盒子，同样按中线居中。
        let box_h = self.m.bar_btn;
        let top = mid - box_h / 2.0;
        let h = box_h;
        let dot = self.m.bar_dot;
        let vsep_w = self.m.bar_vsep_w;

        // 两个箭头之间那颗红块（`.page-btns .space`）
        let arrow_gap = layout.page_next.left - layout.page_prev.right;
        if arrow_gap > 1.5 {
            let side = dot.min(arrow_gap);
            let x = layout.page_prev.right + (arrow_gap - side) / 2.0;
            let y = top + (h - side) / 2.0;
            self.fill_geometry(
                &Rect::new(x, y, x + side, y + side),
                SQ_RED,
                0.0,
                &info,
                false,
            );
        }

        let mut x = layout.page_prev.left;
        // ① 箭头左边的 vsep
        x -= vsep_w;
        self.draw_vsep(x, vsep_top, vsep_w, vsep_h, &info);
        // ② 三个按钮，从右往左；除第一个外每个前面一颗红块
        for (idx, icon) in ["🔄", "✂", "🔣"].iter().enumerate() {
            if idx > 0 {
                x -= dot;
                let y = top + (h - dot) / 2.0;
                self.fill_geometry(
                    &Rect::new(x, y, x + dot, y + dot),
                    SQ_RED,
                    0.0,
                    &info,
                    false,
                );
            }
            x -= self.m.bar_btn;
            if x + self.m.bar_btn > 0.0 {
                self.text_out(
                    &Rect::new(x, top, x + self.m.bar_btn, top + h),
                    icon,
                    SQ_INK,
                    &self.formats.preedit,
                )?;
            }
        }
        // ③ 按钮组左边的 vsep（把拼音和按钮隔开）
        x -= vsep_w;
        if x > 0.0 {
            self.draw_vsep(x, vsep_top, vsep_w, vsep_h, &info);
        }
        Ok(())
    }

    /// 竖向分隔条：绿底 + 居中 2px 黑线（设计稿 `.vsep::before`）。
    fn draw_vsep(&self, x: f32, top: f32, w: f32, h: f32, info: &RoundInfo) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.fill_geometry(
            &Rect::new(x, top, x + w, top + h),
            SQ_GREEN,
            0.0,
            info,
            false,
        );
        let cx = x + w / 2.0;
        self.fill_geometry(
            &Rect::new(cx - 1.0, top, cx + 1.0, top + h),
            SQ_LINE,
            0.0,
            info,
            false,
        );
    }

    /// 字符格：ASCII 半格、其它全格，逐格画浅底 + 右边界线。
    /// 文字仍由调用方整串画一遍，字形连续，不会因为分格被切断。
    ///
    /// ⚠️ **当前设计稿不用它**：`_shots/ime-three-modes.html` 的 `.ch` 只是给每个字
    /// 定宽的 flex 项，没有底色也没有格线，所以 `draw_squeeze` 不再调用（2026-11-05）。
    /// 实现留着备用：设计文档 `doc/squeeze-candidate-design.md` 的「字符格」元素，
    /// 以后若要恢复，把调用加回编码栏那段即可。
    #[allow(dead_code)]
    fn squeeze_grid(&self, rect: &Rect, text: &str, em: f32, bg: u32) {
        if rect.is_empty() || text.is_empty() {
            return;
        }
        let info = RoundInfo::default();
        let mut x = rect.left;
        for ch in text.chars() {
            let width = if ch.is_ascii() { em * 0.5 } else { em };
            let right = (x + width).min(rect.right);
            if right <= x {
                break;
            }
            let cell = Rect::new(x, rect.top, right, rect.bottom);
            if !transparent(bg) {
                // 字符格底也是白的 —— 跟白内板/白核保持一致，直角。
                self.fill_geometry(&cell, bg, 0.0, &info, false);
            }
            self.fill_geometry(
                &Rect::new(right - 1.0, cell.top, right, cell.bottom),
                SQ_GRID,
                0.0,
                &info,
                false,
            );
            x = right;
        }
    }

    /// 竖排字符格（竹简）：列首序号格 → 0.5em 间隔（不画线）→ 正文逐字格，
    /// 每格下沿画一条格线。与横排的 [`Self::squeeze_grid`] 对应，只是格子长边竖直。
    ///
    /// ⚠️ **当前设计稿不用它**：`.cell` 没有 border，竹简是「一片竖条 + 逐字竖排」，
    /// 画格线会多出一片横线，跟设计稿对不上（2026-11-05 去掉调用）。实现留作备用。
    #[allow(dead_code)]
    fn squeeze_grid_v(&self, rect: &Rect, head: f32, gap: f32, body_n: usize) {
        if rect.is_empty() || body_n == 0 {
            return;
        }
        let info = RoundInfo::default();
        let body_top = rect.top + head + gap;
        let step = (rect.bottom - body_top) / body_n as f32;
        if step <= 0.0 {
            return;
        }
        let mut line = |y: f32| {
            if y > rect.top + 0.5 && y <= rect.bottom + 0.5 {
                self.fill_geometry(
                    &Rect::new(rect.left, y - 1.0, rect.right, y),
                    SQ_GRID,
                    0.0,
                    &info,
                    false,
                );
            }
        };
        // 序号格的下边线（= 间隔的上沿），间隔本身留白
        if head > 0.5 {
            line(rect.top + head);
        }
        for i in 0..body_n {
            line(body_top + (i + 1) as f32 * step);
        }
    }

    /// 竖排文字：按字符把矩形切成等高横带，每带单独画一个字（自上而下）。
    ///
    /// DirectWrite 的 `CreateTextLayout` 只排横排，竖排只能自己切格；
    /// 每个格子高度 = `rect.height() / 字数`，正好等于一行的行高，
    /// 所以字形不会被压扁或截断。
    fn text_out_vertical(
        &self,
        rect: &Rect,
        text: &str,
        color: u32,
        format: &IDWriteTextFormat,
    ) -> Result<(), String> {
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() || rect.is_empty() {
            return Ok(());
        }
        let step = rect.height() / chars.len() as f32;
        for (i, ch) in chars.iter().enumerate() {
            let top = rect.top + i as f32 * step;
            let bottom = if i + 1 == chars.len() {
                rect.bottom
            } else {
                top + step
            };
            let cell = Rect::new(rect.left, top, rect.right, bottom);
            let mut buf = [0u8; 4];
            self.text_out(&cell, ch.encode_utf8(&mut buf), color, format)?;
        }
        Ok(())
    }

    /// 自绘背景模糊：把**显示前抓好**的模糊背景铺满面板。
    ///
    /// 抓屏在 [`crate::Window::capture_backdrop`](mod 侧)完成——那时窗口还没
    /// 显示，抓到的才是背后真实画面；paint 里现场抓会把候选窗自己抓进去，
    /// 糊成一团灰（2026-10-31 检查点 §7.5 的教训）。这里只读缓存，无缓存跳过
    /// （面板底色照旧盖住），尺寸对不上就拉伸（模糊图伸缩无碍观感）。
    /// 手绘玻璃底（用户 11:5x）：不抓屏、不依赖背后内容，
    /// 直接在**圆角路径里**画一条竖直渐变 + 顶部高光。因为是自己画、又用圆角几何裁，
    /// 所以圆角外侧天然透明，彻底没有"方角一圈"。
    fn draw_hand_backdrop(&self, rect: &Rect, radius: f32) {
        if let Ok(path) = rounded_rect_path(self.factory, rect, radius, &RoundInfo::default()) {
            // 用**真线性渐变刷**一次填满圆角路径。
            // 之前是 24 条横带近似竖直渐变，而且每条带都各画一遍圆角矩形 ⇒
            // 带与带的抗锯齿接缝叠在一起就是一片可见的**横线**（用户 11-01 截图：
            // "这不是横线吗，这哪是模糊"）。渐变刷没有分带，也就没有接缝。
            // 亮度：顶部 1.0 → 底部 0.75（玻璃受光），基色 #141617，alpha 0x33。
            let lit = |factor: f32| -> u32 {
                let r = (0x14u32 as f32 * factor) as u32;
                let g = (0x16u32 as f32 * factor) as u32;
                let b = (0x17u32 as f32 * factor) as u32;
                (0x33u32 << 24) | (r << 16) | (g << 8) | b
            };
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: d2d_color(lit(1.0)),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: d2d_color(lit(0.75)),
                },
            ];
            unsafe {
                let Ok(collection) = self.target.CreateGradientStopCollection(
                    &stops,
                    D2D1_GAMMA_2_2,
                    D2D1_EXTEND_MODE_CLAMP,
                ) else {
                    return;
                };
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: windows_numerics::Vector2 { x: rect.left, y: rect.top },
                    endPoint: windows_numerics::Vector2 { x: rect.left, y: rect.bottom },
                };
                let Ok(brush) = self
                    .target
                    .CreateLinearGradientBrush(&props, None, &collection)
                else {
                    return;
                };
                self.target.FillGeometry(&path, &brush, None::<&ID2D1Brush>);
            }
        }
    }

    fn draw_backdrop(&self, rect: &Rect, cached: Option<&BackdropCache>) {
        let Some(cached) = cached else {
            return;
        };
        if cached.width <= 0 || cached.height <= 0 {
            return;
        }
        let dest = d2d_rect(rect);
        unsafe {
            self.target.DrawBitmap(
                &cached.bitmap,
                Some(&dest as *const D2D_RECT_F),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
    }

    /// 官方 `_HighlightRect`（WeaselPanel.cpp:799）：阴影 / 底色 / 描边三层。
    #[allow(clippy::too_many_arguments)]
    pub fn highlight_rect(
        &self,
        rect: &Rect,
        radius: f32,
        border: f32,
        back_color: u32,
        shadow_color: u32,
        border_color: u32,
        info: &RoundInfo,
    ) {
        // 高亮块"溢出"窗口时，圆角改用面板圆角（官方 L806-807）
        let mut radius = radius;
        if info.hemispherical {
            radius = self.m.round_corner_ex - border / 2.0;
        }
        if !transparent(shadow_color) && self.m.shadow_radius != 0.0 {
            self.fill_geometry(rect, shadow_color, radius, info, true);
        }
        if !transparent(back_color) {
            self.fill_geometry(rect, back_color, radius, info, false);
        }
        if !transparent(border_color) && border != 0.0 {
            unsafe {
                // 描边要**完全落在面板内**。`DrawGeometry` 的线宽是沿路径**居中**铺开的：
                // 直接拿面板矩形当路径，`border/2` 那一半就溢到面板外面去了 —— 圆角处
                // 看着就是"圆角外面还镶了一圈"（用户 2026-11-04 11:2x，默认档最明显：
                // 不透明档的 border_color 是实色，玻璃/亚克力档在 `plain_scheme` 里被清成 0）。
                //
                // 修法：路径按半个线宽内缩、半径同减半个线宽（等距内偏），描边的**外沿**
                // 正好回到面板边界 —— 面板外形不变，描边变成真正的内描边。
                let mut stroked = *rect;
                stroked.deflate(border / 2.0, border / 2.0);
                let inner = radius - border / 2.0;
                if stroked.width() > 0.0 && stroked.height() > 0.0 {
                    if let Ok(path) = rounded_rect_path(self.factory, &stroked, inner, info) {
                        let _ = self.brush.SetColor(&d2d_color(border_color));
                        self.target.DrawGeometry(
                            &path,
                            self.brush,
                            border,
                            None::<&ID2D1StrokeStyle>,
                        );
                    }
                }
            }
        }
    }

    /// 官方 `D2D::FillGeometry`（d2d.cpp:915）：`to_blur` 时走高斯模糊（阴影）。
    fn fill_geometry(&self, rect: &Rect, color: u32, radius: f32, info: &RoundInfo, to_blur: bool) {
        unsafe {
            if transparent(color) {
                return;
            }
            let _ = self.brush.SetColor(&d2d_color(color));
            if !to_blur && radius == 0.0 {
                self.target.FillRectangle(&d2d_rect(rect), self.brush);
                return;
            }
            let mut rc = *rect;
            if to_blur {
                rc.offset(
                    self.m.dpi_scale * self.m.shadow_offset_x,
                    self.m.dpi_scale * self.m.shadow_offset_y,
                );
            }
            let Ok(path) = rounded_rect_path(self.factory, &rc, radius, info) else {
                return;
            };
            if !to_blur {
                self.target.FillGeometry(&path, self.brush, None::<&ID2D1Brush>);
                return;
            }
            // 阴影：先画到一张离屏位图，再套高斯模糊（官方 d2d.cpp:935-968）
            let Ok(render_target) = self.target.CreateCompatibleRenderTarget(
                None,
                None,
                None,
                D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE,
            ) else {
                // 拿不到离屏目标就退化为一层实色（至少不会丢阴影的整体形状）
                self.target.FillGeometry(&path, self.brush, None::<&ID2D1Brush>);
                return;
            };
            render_target.BeginDraw();
            render_target.Clear(Some(&D2D_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            render_target.FillGeometry(&path, self.brush, None::<&ID2D1Brush>);
            let _ = render_target.EndDraw(None, None);
            let Ok(bitmap) = render_target.GetBitmap() else {
                return;
            };
            let context: ID2D1DeviceContext = match self.target.cast() {
                Ok(context) => context,
                Err(_) => return,
            };
            let Ok(effect) = context.CreateEffect(&CLSID_D2D1GaussianBlur) else {
                return;
            };
            let image: ID2D1Image = match bitmap.cast() {
                Ok(image) => image,
                Err(_) => return,
            };
            effect.SetInput(0, &image, true);
            let deviation = self.m.dpi_scale * self.m.shadow_radius;
            let bytes = deviation.to_ne_bytes();
            let _ = effect.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &bytes,
            );
            let Ok(output) = effect.GetOutput() else {
                return;
            };
            context.DrawImage(
                &output,
                None,
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
        }
    }

    /// 官方 `_DrawPreedit`（WeaselPanel.cpp:511）。
    pub fn draw_preedit(&self, layout: &Layout, is_preedit: bool) -> Result<(), String> {
        let Some(preedit) = &self.view.preedit else {
            return Ok(());
        };
        if preedit.text.is_empty() {
            return Ok(());
        }
        let (base, before, hilite, after) = if is_preedit {
            (
                layout.preedit,
                layout.preedit_before,
                layout.preedit_hilite,
                layout.preedit_after,
            )
        } else {
            (
                layout.aux,
                layout.aux_before,
                layout.aux_hilite,
                layout.aux_after,
            )
        };
        // 我们的快照只给"整段都是组字中"，所以官方那套 before/hilite/after 里
        // hilite 是整段、before/after 为空（composition 预编辑的常态）。
        if !hilite.is_null() {
            // 用户 14:3x：预编辑（编码）那块**不要黑色底**，要透明——只画字。
            let mut rc = hilite;
            rc.inflate(self.m.hilite_padding_x, self.m.hilite_padding_y);
            let _ = &rc;
            self.text_out(
                &hilite,
                &preedit.text,
                self.scheme.hilited_text_color,
                &self.formats.preedit,
            )?;
        } else if !base.is_null() {
            self.text_out(
                &base,
                &preedit.text,
                self.scheme.text_color,
                &self.formats.preedit,
            )?;
        }
        let _ = (before, after);

        Ok(())
    }

    /// 官方 `_DrawCandidates`（WeaselPanel.cpp:618）。
    fn draw_candidates(&self, layout: &Layout) -> Result<(), String> {
        let count = layout.rows.len();
        if count == 0 {
            return Ok(());
        }
        let selected = (self.view.selected_index as usize) < count;
        let highlighted = if selected {
            self.view.selected_index as usize
        } else {
            usize::MAX
        };
        let hovered = self.hovered_index(layout);

        // ① 阴影（官方 L641-650）
        for i in 0..count {
            if Some(i) == hovered {
                continue;
            }
            let shadow = if i == highlighted {
                self.scheme.hilited_candidate_shadow_color
            } else {
                self.scheme.candidate_shadow_color
            };
            if !transparent(shadow) {
                self.highlight_rect(
                    &layout.highlight_rect(self.m, i),
                    self.m.round_corner,
                    0.0,
                    0,
                    shadow,
                    0,
                    &layout.round_info[i],
                );
            }
        }
        // ② 悬停块（官方 L651-656）：半透明三件套
        if let Some(index) = hovered {
            if index < count {
                self.highlight_rect(
                    &layout.highlight_rect(self.m, index),
                    self.m.round_corner,
                    self.m.border,
                    half_alpha(self.scheme.hilited_candidate_back_color),
                    half_alpha(self.scheme.hilited_candidate_shadow_color),
                    half_alpha(self.scheme.hilited_candidate_border_color),
                    &layout.round_info[index],
                );
            }
        }
        // ③ 高亮滑动动画：模组侧已经按 CSS 语义插值好矩形（只滑位置、宽高取目标），
        //    这里只负责画，并且跳过目标项的静态高亮（否则会叠出两块）。
        let anim_rect = self.hilite_anim.map(|(rect, _)| rect);
        let anim_target = self.hilite_anim.map(|(_, target)| target);
        if let Some(rect) = anim_rect {
            self.highlight_rect(
                &rect,
                self.m.round_corner,
                self.m.border,
                self.scheme.hilited_candidate_back_color,
                0,
                self.scheme.hilited_candidate_border_color,
                &RoundInfo::default(),
            );
        }
        // ③ 每个候选：底 + 框 + 序号 + 正文 + 注释（官方 L670-699）
        for i in 0..count {
            let is_selected = i == highlighted;
            let row = layout.rows[i];
            let back = if is_selected {
                self.scheme.hilited_candidate_back_color
            } else {
                self.scheme.candidate_back_color
            };
            let border_color = if is_selected {
                self.scheme.hilited_candidate_border_color
            } else {
                self.scheme.candidate_border_color
            };
            // 动画进行中跳过**目标项**的静态高亮（由上面的滑动矩形代替）
            if anim_target != Some(i) {
                self.highlight_rect(
                    &layout.highlight_rect(self.m, i),
                    self.m.round_corner,
                    self.m.border,
                    back,
                    0,
                    border_color,
                    &layout.round_info[i],
                );
            }

            let label_color = if is_selected {
                self.scheme.hilited_label_color
            } else {
                self.scheme.label_color
            };
            let text_color = if is_selected {
                self.scheme.hilited_candidate_text_color
            } else {
                self.scheme.candidate_text_color
            };
            let comment_color = if is_selected {
                self.scheme.hilited_comment_text_color
            } else {
                self.scheme.comment_text_color
            };

            if !transparent(label_color) && !row.label.is_null() {
                let label = self
                    .m
                    .format_label(&(i as u32 + 1).to_string());
                self.text_out(&row.label, &label, label_color, &self.formats.label)?;
            }
            if !transparent(text_color) {
                let text = &self.view.items[i].primary_text;
                if !text.is_empty() {
                    self.text_out(&row.text, text, text_color, &self.formats.text)?;
                }
            }
            if !transparent(comment_color) && !row.comment.is_null() {
                let comment = &self.view.items[i].secondary_text;
                if !comment.is_empty() {
                    self.text_out(&row.comment, comment, comment_color, &self.formats.comment)?;
                }
            }
        }
        // ④ 高亮标记（官方 L701-747）
        if !transparent(self.scheme.hilited_mark_color) && highlighted < count {
            let rc = layout.highlight_rect(self.m, highlighted);
            if !self.m.mark_text.is_empty() {
                let (mark_width, mark_height, _) = layout.mark;
                let vgap = if mark_height != 0.0 {
                    (rc.height() - mark_height) / 2.0
                } else {
                    0.0
                };
                let hgap = if mark_width != 0.0 {
                    (rc.width() - mark_width) / 2.0
                } else {
                    0.0
                };
                let mark_rect = Rect::new(
                    rc.left + self.m.hilite_padding_x,
                    rc.top + vgap,
                    rc.left + self.m.hilite_padding_x + mark_width,
                    rc.bottom - vgap,
                );
                let _ = hgap;
                self.text_out(
                    &mark_rect,
                    &self.m.mark_text,
                    self.scheme.hilited_mark_color,
                    &self.formats.text,
                )?;
            } else {
                // 官方 L719-746：一根圆头竖条，宽度 = 行高/7（上限 0.618）
                let (mark_width, _, _) = layout.mark;
                let mut height = (rc.height() - self.m.hilite_padding_y * 2.0)
                    .min(rc.height() - self.m.round_corner * 2.0);
                let mut width = (rc.width() - self.m.hilite_padding_x * 2.0)
                    .min(rc.width() - self.m.round_corner * 2.0);
                width = width.min(rc.width() * 0.618);
                height = height.min(rc.height() * 0.618);
                let y = rc.top + (rc.height() - height) / 2.0;
                let bar = Rect::new(
                    rc.left,
                    y,
                    rc.left + mark_width.min(width.max(1.0)),
                    y + height,
                );
                let radius = bar.width() / 2.0;
                self.highlight_rect(
                    &bar,
                    radius,
                    0.0,
                    self.scheme.hilited_mark_color,
                    0,
                    0,
                    &RoundInfo::default(),
                );
            }
        }
        Ok(())
    }

    /// 悬停中的候选（官方 `m_hoverIndex`：`hover_type = none` 时不悬停）。
    fn hovered_index(&self, layout: &Layout) -> Option<usize> {
        if self.m.hover == Hover::None {
            return None;
        }
        self.hover_index.filter(|index| *index < layout.rows.len())
    }

    /// 官方 `_TextOut`（WeaselPanel.cpp:751）：建文本布局、补 overhang、画。
    fn text_out(
        &self,
        rect: &Rect,
        text: &str,
        color: u32,
        format: &IDWriteTextFormat,
    ) -> Result<(), String> {
        if text.is_empty() || rect.is_empty() || rect.width() <= 0.0 || rect.height() <= 0.0 {
            return Ok(());
        }
        unsafe {
            let wide: Vec<u16> = text.encode_utf16().collect();
            let layout = self
                .write
                .CreateTextLayout(&wide, format, rect.width(), rect.height())
                .map_err(|e| e.to_string())?;
            let mut x = rect.left;
            let mut y = rect.top;
            if let Ok(overhang) = layout.GetOverhangMetrics() {
                if overhang.left > 0.0 {
                    x += overhang.left;
                }
                if overhang.top > 0.0 {
                    y += overhang.top;
                }
            }
            let _ = self.brush.SetColor(&d2d_color(color));
            self.target.DrawTextLayout(
                windows_numerics::Vector2 { x: x, y: y },
                &layout,
                self.brush,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
        }
        Ok(())
    }

    /// 翻页提示（‹ ›）。
    ///
    /// 官方把它画在 `_DrawPreedit` 里、且要求 `prevpage_color`/`nextpage_color` 不透明
    /// （`config.rs` 的 `page_enabled`）。我们改成**独立绘制 + 颜色兜底**：
    /// 配色里没给翻页色（出厂 37 套全是 `0x00000000`）时用文字色半透明，保证「还有下一页」看得见；
    /// 也不能再依赖 preedit（行内预编辑时 preedit 为 None ⇒ 原来永远不画）。
    fn draw_page_buttons(&self, layout: &Layout) {
        if layout.inline_preedit || layout.page_prev.is_empty() || layout.page_next.is_empty() {
            return;
        }
        let text = self.scheme.text_color;
        let fallback = |alpha: u32| (alpha << 24) | (text & 0x00ff_ffff);
        let prev = if transparent(self.scheme.prevpage_color) {
            fallback(if self.view.can_page_previous { 0xCC } else { 0x59 })
        } else if self.view.can_page_previous {
            self.scheme.prevpage_color
        } else {
            fallback(0x40)
        };
        let next = if transparent(self.scheme.nextpage_color) {
            fallback(if self.view.can_page_next { 0xCC } else { 0x59 })
        } else if self.view.can_page_next {
            self.scheme.nextpage_color
        } else {
            fallback(0x40)
        };
        let _ = self.text_out(&layout.page_prev, PAGE_PREV, prev, &self.formats.preedit);
        let _ = self.text_out(&layout.page_next, PAGE_NEXT, next, &self.formats.preedit);
    }

    /// 状态图标位（官方画 HICON；我们没有图标资源，退化为中/英方块提示）。
    fn draw_status_icon(&self, layout: &Layout) {
        if layout.status_icon.is_empty() {
            return;
        }
        let ascii = self.view.ascii_mode.unwrap_or(false);
        let glyph = if ascii { "英" } else { "中" };
        unsafe {
            let _ = self.brush.SetColor(&d2d_color(self.scheme.text_color));
            let wide: Vec<u16> = glyph.encode_utf16().collect();
            if let Ok(text_layout) = self.write.CreateTextLayout(
                &wide,
                &self.formats.label,
                layout.status_icon.width(),
                layout.status_icon.height(),
            ) {
                self.target.DrawTextLayout(
                    windows_numerics::Vector2 {
                        x: layout.status_icon.left,
                        y: layout.status_icon.top,
                    },
                    &text_layout,
                    self.brush,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{plain_scheme, theme_veil, veil_alpha, Material, VEIL_ALPHA};
    use weasel_style::ColorScheme;

    /// 只关心 back/text 的配色；其余按 `resolve` 的缺省关系填成同色，
    /// 免得"没填的颜色"混进对比度判断。
    fn scheme(back: u32, text: u32) -> ColorScheme {
        ColorScheme {
            name: "test".into(),
            author: String::new(),
            color_format: String::new(),
            back_color: back,
            border_color: text,
            shadow_color: 0,
            text_color: text,
            candidate_text_color: text,
            candidate_back_color: 0,
            candidate_border_color: 0,
            candidate_shadow_color: 0,
            label_color: text,
            comment_text_color: text,
            hilited_text_color: text,
            hilited_back_color: back,
            hilited_border_color: 0,
            hilited_shadow_color: 0,
            hilited_candidate_text_color: text,
            hilited_candidate_back_color: 0,
            hilited_candidate_border_color: 0,
            hilited_candidate_shadow_color: 0,
            hilited_label_color: text,
            hilited_comment_text_color: text,
            hilited_mark_color: 0,
            nextpage_color: text,
            prevpage_color: text,
        }
    }

    /// steam：深底（#1B1D22 ≈ 29）浅字（#F2F3F5）。
    fn steam() -> ColorScheme {
        scheme(0xFF_1B_1D_22, 0xFF_F2_F3_F5)
    }

    #[test]
    fn glass_keeps_theme_text_on_matching_backdrop() {
        // 深色桌面 + 深底浅字：主题自己的白字对比度足够 ⇒ 原样保留。
        let out = plain_scheme(&steam(), Material::Glass, Some(20.0));
        assert_eq!(out.text_color, 0xFF_F2_F3_F5);
        assert_eq!(out.candidate_text_color, 0xFF_F2_F3_F5);
        assert_eq!(out.comment_text_color, 0xFF_F2_F3_F5);
    }

    #[test]
    fn glass_flips_text_on_opposite_backdrop() {
        // 白网页 + 深底浅字：白字压白底看不清 ⇒ 自动换成深墨色。
        let out = plain_scheme(&steam(), Material::Glass, Some(245.0));
        assert_eq!(out.text_color, 0xFF_1B_1D_22);
        assert_eq!(out.comment_text_color, 0xFF_6B_72_80);
        // 高亮块底色没变，块上的字仍要对它（#3b4a63 是深色底 ⇒ 白字）。
        assert_eq!(out.hilited_candidate_text_color, 0xFF_F2F3_F5);
    }

    #[test]
    fn glass_without_backdrop_assumes_light() {
        // 抓不到背景（老系统/无抓屏权限）时面板退回 50% 白 ⇒ 按白底挑字色。
        let out = plain_scheme(&steam(), Material::Glass, None);
        assert_eq!(out.text_color, 0xFF_1B_1D_22);
    }

    #[test]
    fn acrylic_veil_takes_palette_hue() {
        // 「颜色按 theme 走」：真实 steam（#141617 底 + #CD8C52 橙字 + #594231 棕高亮）
        // 的蒙版必须**偏暖**（红 > 蓝），而不是上一版面积平均糊出来的中性灰。
        let mut real = scheme(0xFF_14_16_17, 0xFF_CD_8C_52);
        real.hilited_candidate_back_color = 0xFF_59_42_31;
        real.comment_text_color = 0xFF_A7_A7_A9;
        let veil = theme_veil(&real);
        let (r, g, b) = ((veil >> 16) & 0xFF, (veil >> 8) & 0xFF, veil & 0xFF);
        assert!(r > b + 12, "蒙版没带上橙色偏：r={r} g={g} b={b}");
        assert!(r > g && g > b, "色相不对（应偏橙）：r={r} g={g} b={b}");
        // 用的是**底色自己的明度**：深底不能被文字色提亮成浅底。
        assert!(r < 0x50, "蒙版被文字色提亮了：r={r}");
        assert_eq!(veil >> 24, veil_alpha());
    }

    #[test]
    fn acrylic_veil_keeps_back_color_lightness() {
        // 深浅两种底 + 同一个橙字：明度各自跟底色走，只换色相。
        let dark = theme_veil(&scheme(0xFF_14_16_17, 0xFF_CD_8C_52));
        let light = theme_veil(&scheme(0xFF_F5_F6_F8, 0xFF_CD_8C_52));
        let luma = |argb: u32| {
            let (r, g, b) = (
                ((argb >> 16) & 0xFF) as f32,
                ((argb >> 8) & 0xFF) as f32,
                (argb & 0xFF) as f32,
            );
            0.299 * r + 0.587 * g + 0.114 * b
        };
        assert!(luma(dark) < 120.0, "深底的蒙版变亮了：{:#010X}", dark);
        assert!(luma(light) > 180.0, "浅底的蒙版变暗了：{:#010X}", light);
        // 两边都要带上橙（红 > 蓝），只是明暗不同。
        assert!((dark >> 16) & 0xFF > dark & 0xFF);
        assert!((light >> 16) & 0xFF > light & 0xFF);
    }

    #[test]
    fn acrylic_veil_monochrome_palette_stays_neutral() {
        // 无彩配色（纯黑白皮肤）没有"Hue"可借 ⇒ 蒙版就是底色，绝不凭空造色。
        assert_eq!(theme_veil(&steam()), (veil_alpha() << 24) | 0x00_1B_1D_22);
    }

    #[test]
    fn veil_txt_overrides_opacity() {
        // `veil.txt` 写百分数：40 更透、95 更实；带 % 和空白都认。档位默认 50 也在范围内。
        assert_eq!(super::parse_veil_pct("50"), Some(128)); // 50%  → 0x80（档位默认值）
        assert_eq!(super::parse_veil_pct("40"), Some(102));
        assert_eq!(super::parse_veil_pct(" 78% "), Some(199)); // 78%  → 0xC7
        assert_eq!(super::parse_veil_pct("95"), Some(242));
        // 两头越界（低于 30 变玻璃 / 高于 95 没磨砂）与乱写一律当"没写"。
        assert_eq!(super::parse_veil_pct("25"), None);
        assert_eq!(super::parse_veil_pct("99"), None);
        assert_eq!(super::parse_veil_pct("abc"), None);
        assert_eq!(super::parse_veil_pct(""), None);
    }

    #[test]
    fn material_default_is_a_tier_name() {
        // 设置界面 2026-11-04 10:50 起第三档叫「默认」，文件里也可能写 `default`
        // —— 必须解析成同一档（不抓屏、不模糊、back_color 铺满），旧名继续有效。
        use super::Material;
        for text in ["default", "opaque", "original", "solid", " DEFAULT "] {
            assert_eq!(Material::parse(text), Material::Opaque, "{text:?}");
        }
        // 另外两档不受影响。
        assert_eq!(Material::parse("glass"), Material::Glass);
        assert_eq!(Material::parse("acrylic"), Material::Acrylic);
        assert_eq!(Material::parse(""), Material::Glass);
    }

    #[test]
    fn acrylic_veil_is_firmly_wedged_between_glass_and_solid() {
        // 11:10 用户定档「50 透明」⇒ 蒙版不透明度 50%（0x80）。
        // 底线：必须比透明档（0x33）实一大截，否则又退回玻璃；上限留出余量。
        assert!(
            (0x80..0xB3).contains(&VEIL_ALPHA),
            "蒙版 {:.0}% 不在「半透但仍是磨砂面」的区间",
            VEIL_ALPHA as f32 / 255.0 * 100.0
        );
        assert!(VEIL_ALPHA > super::GLASS_ALPHA * 2);
        // 深色皮肤 + 任意壁纸（纯黑/纯白）都得用浅色字。
        for backdrop in [None, Some(0.0), Some(255.0)] {
            let out = plain_scheme(&steam(), Material::Acrylic, backdrop);
            assert_eq!(out.text_color, 0xFF_F2_F3_F5, "backdrop={backdrop:?}");
        }
    }

    #[test]
    fn acrylic_text_survives_both_desktop_extremes() {
        // steam 深底：不管壁纸是纯黑还是纯白，文字都要够亮（浅色字）。
        let out = plain_scheme(&steam(), Material::Acrylic, None);
        assert_eq!(out.text_color, 0xFF_F2_F3_F5);
        // 反过来：浅底深字的配色（#F5F6F8 底 + #1B1D22 字）也得用深字。
        let light = scheme(0xFF_F5_F6_F8, 0xFF_1B_1D_22);
        let out = plain_scheme(&light, Material::Acrylic, None);
        assert_eq!(out.text_color, 0xFF_1B_1D_22);
    }
}



