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
use crate::theme::config::{Hover, Metrics};
use crate::theme::layout::{Layout, Rect, RoundInfo, PAGE_NEXT, PAGE_PREV};
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

/// `plain` 档（纯白面板）的浅底配色 —— 逐项照抄 `C:\Code\_shots\cand_preview.html`。
///
/// 配色里的 `steam` 是**深底浅字**：面板一换成纯白，编码/候选/注释全变成
/// "白底白字"，肉眼就是"看不见"（2026-11-01 用户截图：只有高亮项看得见）。
/// 所以只在 plain 档套这套 HTML 颜色，其它材质仍然用配色表里的值。
pub fn plain_scheme(base: &ColorScheme) -> ColorScheme {
    /// 正文 / 编码：HTML 的 `#1b1d22`（用户 16:1x：字体要黑的）。
    const INK: u32 = 0xFF_1B_1D_22;
    /// 注释：HTML 的 `#6b7280`。
    const MUTED: u32 = 0xFF_6B_72_80;
    /// 翻页箭头：HTML 的 `#9aa4b2`。
    const PAGER: u32 = 0xFF_9A_A4_B2;
    /// 高亮框底：HTML `.hilite` 的 `#3b4a63`；字用白色。
    const HILITE_BG: u32 = 0xFF_3B_4A_63;

    let mut scheme = base.clone();
    scheme.back_color = 0xFF_FF_FF_FF;
    // HTML 的面板没有描边、没有投影；边框和阴影全透明（透明就不画）。
    scheme.border_color = 0;
    scheme.shadow_color = 0;
    scheme.text_color = INK;
    scheme.hilited_text_color = INK; // 组字中的编码也走这里，不能用深底配色的浅色
    scheme.candidate_text_color = INK;
    scheme.label_color = INK;
    scheme.comment_text_color = MUTED;
    scheme.candidate_back_color = 0;
    scheme.candidate_border_color = 0;
    scheme.candidate_shadow_color = 0;
    scheme.hilited_candidate_back_color = HILITE_BG;
    scheme.hilited_candidate_text_color = 0xFF_FF_FF_FF;
    scheme.hilited_label_color = 0xFF_FF_FF_FF;
    scheme.hilited_comment_text_color = 0xFF_FF_FF_FF;
    scheme.hilited_candidate_border_color = 0;
    scheme.hilited_candidate_shadow_color = 0;
    scheme.prevpage_color = PAGER;
    scheme.nextpage_color = PAGER;
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
    /// 显示前抓好的模糊背景缓存（mod 侧 [`BackdropCache`]；`None` = 没抓到）。
    pub backdrop: Option<&'a BackdropCache>,
    /// 高亮滑动动画：`(from 下标, to 下标, 进度 0..1)`；`None` = 不播动画。
    pub hilite_anim: Option<(usize, usize, f32)>,
}

/// 显示前抓好的模糊背景（自绘高斯模糊的缓存单元）。
pub struct BackdropCache {
    pub width: i32,
    pub height: i32,
    pub bitmap: ID2D1Bitmap,
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
        {
            let style = self.m;
            let inline = layout.inline_preedit;
            let has_candidates = !layout.rows.is_empty();
            // 官方 should_draw_background（L438-440）
            let draw_background = (!empty_context && !style.inline_preedit)
                || (style.inline_preedit && (has_candidates || !layout.aux.is_empty()));
            if draw_background {
                // 材质三档（用户 21:0x 定名）：glass / acrylic / metal，走环境变量
                // `WEASEL_MATERIAL`（默认 glass，即自绘模糊那一档）。
                // 材质由设置界面写进 `%APPDATA%\Rime-Next	heme\material.txt`；
                // 文件不存在时退回环境变量，再退回 glass（用户 21:0x：要在设置里切）。
                let material = material();
                let plain = material == "plain";
                let back_color = match material.as_str() {
                    // 纯白框（重画里程碑 1）：不透明白，无玻璃无材质
                    "plain" => 0xFF_FFFF_FF,
                    // 亚克力：乳白半透（不模糊，比玻璃"实"）
                    "acrylic" => {
                        let base = lighten_color(self.scheme.back_color, 0.22);
                        (0xB4u32 << 24) | (base & 0x00FF_FFFF)
                    }
                    // 金属：不透明哑光
                    "metal" => 0xFF00_0000 | (darken_color(self.scheme.back_color, 0.78) & 0x00FF_FFFF),
                    // 玻璃：手绘底 + 20% 染色。glass_self 时 `self.glass` 为 false（关了 DWM 亚克力），
                    // 但面板仍要 20% alpha，否则 DComp 逐像素透明被不透明 back_color 盖死（用户 12:1x：为什么不透明）。
                    _ => {
                        if self.glass || self.glass_self {
                            glassify(self.scheme.back_color)
                        } else {
                            self.scheme.back_color
                        }
                    }
                };
                // 玻璃态：面板画满**整个窗口矩形**，而不是内缩 `offset` 的内框。
                // 否则窗口比面板大一圈，那圈被 DWM 亚克力填满 ⇒ 边框外面多出一层灰（用户 13:4x 报的"还有一层"）。
                // 代价：投影没有落脚处（`offset` 那圈就是给投影的），玻璃态干脆不要投影。
                let back_rect = if self.glass && !plain {
                    Rect::new(
                        0.0,
                        0.0,
                        layout.content_size.cx,
                        layout.content_size.cy,
                    )
                } else {
                    layout.content
                };
                if self.glass_self && !plain {
                    self.draw_hand_backdrop(&back_rect, style.round_corner_ex);
                }
                // 亮度层：先铺深色压住背景纹理，再叠皮肤色 tint。
                // 少了这层，20% 的面板叠在什么背景上都是一片浑浊的灰。
                if (self.glass || self.glass_self) && !plain {
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
                    if (self.glass || plain) { 0 } else { self.scheme.shadow_color }, // plain 不要投影，否则面板外一圈黑
                    // 玻璃档的描边用白色高光：原来用配色的 border_color
                    // （steam 是深灰 #353638）压在蓝色模糊上就是一圈脏黑边（21:2x 用户截图）。
                    // 但 25% 白太淡 ⇒ 窗口边界糊在背景里（"smeared"），提到 55% 才收得住边。
                    if (self.glass || self.glass_self) && !plain { 0x8CFF_FFFF } else { self.scheme.border_color },
                    &RoundInfo::default(),
                );
                if material == "metal" {
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
                let inner = radius - border / 2.0;
                if let Ok(path) = rounded_rect_path(self.factory, rect, inner, info) {
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
        // ③ 高亮滑动动画：把"旧位置 → 新位置"插值成一个矩形（必须画在文字之前）
        let anim_rect = self.hilite_anim.and_then(|(from, to, t)| {
            if from >= count || to >= count {
                return None;
            }
            let a = layout.highlight_rect(self.m, from);
            let b = layout.highlight_rect(self.m, to);
            // HTML 的 `.hilite` 只过渡 `left`/`top`，宽高**瞬间定死**
            //（原文注释：只滑位置；宽高瞬间定死 —— 否则动画中"小窗变大小"）。
            // 这里照抄：位置插值、宽高取目标框。四边一起插值会把框在中途压扁/拉长，
            // 还会盖到邻居候选的字上（2026-11-01 用户截图里"框卡在 2 和 3 之间"）。
            let lerp = |x: f32, y: f32| x + (y - x) * t;
            let left = lerp(a.left, b.left);
            let top = lerp(a.top, b.top);
            Some(Rect::new(left, top, left + b.width(), top + b.height()))
        });
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
            // 动画进行中跳过选中项的静态高亮（由下面的插值矩形代替）
            if !(is_selected && anim_rect.is_some()) {
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



