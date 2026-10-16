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
}

/// 模式提示图标的文件位置：`<程序文件夹>\Rime-Next\icon\{zh,en}-{dark,light}.ico`。
///
/// 官方把这两枚图标编在 WeaselUI 的资源里（`IDI_EN` / `IDI_ZH`，`LoadIconIfNeed`
/// 允许用主题的 `ascii_icon` / `zhung_icon` 覆盖）；我们仓库里是独立的 .ico，
/// 按"底色深浅"挑浅色/深色那一套。
fn status_icon_path(ascii: bool, dark_background: bool) -> Option<std::path::PathBuf> {
    let dir = std::env::current_exe()
        .ok()?
        .parent()?
        .join("Rime-Next")
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
        }
    }
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
                self.highlight_rect(
                    &layout.content,
                    style.round_corner_ex,
                    style.border,
                    self.scheme.back_color,
                    self.scheme.shadow_color,
                    self.scheme.border_color,
                    &RoundInfo::default(),
                );
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
        }
        Ok(())
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
            let mut rc = hilite;
            rc.inflate(self.m.hilite_padding_x, self.m.hilite_padding_y);
            self.highlight_rect(
                &rc,
                self.m.round_corner,
                self.m.border,
                self.scheme.hilited_back_color,
                self.scheme.hilited_shadow_color,
                0,
                &layout.text_round_info,
            );
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

        // 翻页按钮（官方 L576-596）
        if !layout.rows.is_empty()
            && !self.m.inline_preedit
            && !transparent(self.scheme.prevpage_color)
            && !transparent(self.scheme.nextpage_color)
        {
            let prev_color = if self.view.can_page_previous {
                self.scheme.prevpage_color
            } else {
                self.scheme.text_color
            };
            self.text_out(
                &layout.page_prev,
                PAGE_PREV,
                prev_color,
                &self.formats.preedit,
            )?;
            let next_color = if self.view.can_page_next {
                self.scheme.nextpage_color
            } else {
                self.scheme.text_color
            };
            self.text_out(
                &layout.page_next,
                PAGE_NEXT,
                next_color,
                &self.formats.preedit,
            )?;
        }
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
            self.highlight_rect(
                &layout.highlight_rect(self.m, i),
                self.m.round_corner,
                self.m.border,
                back,
                0,
                border_color,
                &layout.round_info[i],
            );

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
                    .format_label(&(self.view.page_start + i as u32 + 1).to_string());
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

