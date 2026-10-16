//! Windows 候选栏主题后端：照官方 `WeaselUI` 的窗口与绘制流程（`WeaselPanel` + `d2d`）。
//!
//! 与官方一致的四条主线：
//! * **窗口**：`WS_POPUP | WS_CLIPSIBLINGS` + `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE |
//!   WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP`（WeaselPanel.cpp:374-379）。官方**没有**
//!   `WS_EX_LAYERED`、没有 DWM 模糊、没有 `SetWindowRgn` —— 逐像素透明靠
//!   D3D11 + DXGI 组合交换链 + DirectComposition（见 `presenter.rs`）交给 DWM。
//! * **布局**：`config::Metrics`（官方 style 语义）+ `layout::Layout`（官方 DoLayout）。
//! * **绘制**：`paint::Painter`（官方 DoPaint）。
//! * **定位**：官方 `_Reposition`（下方放不下就翻到插入点上方并进入 sticky）。
//!
//! HWND、图形资源和输入状态都由本模块内部管理；窗口消息回调与主题接口共享稳定的
//! [`Window`] 分配，所有原生窗口和图形操作都在创建它们的 UI 线程执行。

pub mod config;
pub mod layout;
pub mod paint;

use crate::d2d_bindings::*;
use crate::theme::paint::{d2d_color, Formats, Painter};
use crate::theme_api::{CandidateView, EventSink, ThemeBackend, UiAction, UiMode};
use config::Metrics;
use layout::{Hit, Layout, Measured, Parts, Size};
use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};
use weasel_style::ColorScheme;
use windows_strings::w;

const CLASS: windows_strings::PCWSTR = w!("Weasel.ThemeOfficial.Official");
const RETRY_TIMER: usize = 1;

/// 官方 `MoveTo` 里给插入点下方留的 6 像素（WeaselPanel.cpp:196/213）。
const CARET_GAP: i32 = 6;

/// 官方 `WeaselPanel::AUTOREV_TIMER` / `AUTOHIDE_TIMER` 的作用：自动恢复与自动隐藏。
const AUTOREV_TIMER: usize = 2;
const AUTOHIDE_TIMER: usize = 3;

// ---------------------------------------------------------------- 图形资源

/// 持有 DirectWrite 格式及按需创建的 HWND 绘制目标。
///
/// 注意：**Direct2D 工厂不在这里** —— 路径几何、画刷必须与呈现层出自同一个工厂
/// （否则 `EndDraw` 报 `D2DERR_WRONG_FACTORY`），所以几何用
/// [`crate::presenter::Presenter::d2d_factory`]。
struct Graphics {
    /// 供文本测量和布局使用的共享 DirectWrite 工厂。
    write: IDWriteFactory,
    /// 四套文本格式（官方 `InitFontFormats`）。
    formats: Formats,
    /// 中英文模式提示用的紧凑格式（官方没有这一项，是我们自己的小窗）。
    indicator: IDWriteTextFormat,
    /// 与此 HWND 关联的呈现层：D3D11 + DXGI 组合交换链 + DirectComposition + D2D 设备上下文。
    target: Option<crate::presenter::Presenter>,
}

impl Graphics {
    /// 按 `Metrics` 里的字体建文本格式（官方 `D2D::InitFontFormats`）。
    fn new(m: &Metrics) -> windows_core::Result<Self> {
        let write: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let formats = Self::make_formats(&write, m)?;
        let indicator = Self::make_format(
            &write,
            "Segoe UI",
            (m.font_point * 0.75).max(9.0),
            DWRITE_WORD_WRAPPING_NO_WRAP,
            m,
        )?;
        Ok(Self {
            write,
            formats,
            indicator,
            target: None,
        })
    }

    /// 官方 `GetOrCreateTextFormat`：字号 = 点 × `dpi/72`，LEADING + 段落居中，
    /// 行距/基线只在两者都非 0 时启用。
    fn make_format(
        write: &IDWriteFactory,
        face: &str,
        size: f32,
        wrapping: DWRITE_WORD_WRAPPING,
        m: &Metrics,
    ) -> windows_core::Result<IDWriteTextFormat> {
        unsafe {
            let face: Vec<u16> = face.encode_utf16().chain(Some(0)).collect();
            let format = write.CreateTextFormat(
                windows_core::PCWSTR(face.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("zh-CN"),
            )?;
            format
                .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)
                .ok()?;
            format
                .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
                .ok()?;
            format.SetWordWrapping(wrapping).ok()?;
            if m.linespacing != 0 && m.baseline != 0 {
                let line = size * (m.linespacing as f32 / 100.0);
                let base = size * (m.baseline as f32 / 100.0);
                let _ = format.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, line, base);
            } else {
                let _ = format.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_DEFAULT, 0.0, 0.0);
            }
            Ok(format)
        }
    }

    fn make_formats(write: &IDWriteFactory, m: &Metrics) -> windows_core::Result<Formats> {
        // 官方：max_width = 0（横向）或 max_height = 0（竖排）时用 NO_WRAP，否则整词换行；
        // 预编辑在可换行时用按字符换行（D2D::InitFontFormats）
        let wraps = if m.layout_type.is_vertical_text() {
            m.max_height != 0.0
        } else {
            m.max_width != 0.0
        };
        let wrapping = if wraps {
            DWRITE_WORD_WRAPPING_WHOLE_WORD
        } else {
            DWRITE_WORD_WRAPPING_NO_WRAP
        };
        let wrapping_preedit = if wraps {
            DWRITE_WORD_WRAPPING_CHARACTER
        } else {
            DWRITE_WORD_WRAPPING_NO_WRAP
        };
        Ok(Formats {
            preedit: Self::make_format(
                write,
                &m.font_face,
                m.font_point,
                wrapping_preedit,
                m,
            )?,
            text: Self::make_format(write, &m.font_face, m.font_point, wrapping, m)?,
            label: Self::make_format(
                write,
                &m.label_font_face,
                m.label_font_point,
                wrapping,
                m,
            )?,
            comment: Self::make_format(
                write,
                &m.comment_font_face,
                m.comment_font_point,
                wrapping,
                m,
            )?,
        })
    }

    /// 配置变了（明暗、字体、DPI）就重建格式。
    fn refresh_formats(&mut self, m: &Metrics) -> windows_core::Result<()> {
        self.formats = Self::make_formats(&self.write, m)?;
        self.indicator = Self::make_format(
            &self.write,
            "Segoe UI",
            (m.font_point * 0.75).max(9.0),
            DWRITE_WORD_WRAPPING_NO_WRAP,
            m,
        )?;
        Ok(())
    }

    /// 官方 `D2D::GetTextSize`：量宽度含尾随空白、向上取整，并把 overhang 的正向溢出补进尺寸。
    fn measure(&self, text: &str, format: &IDWriteTextFormat, m: &Metrics) -> Size {
        if text.is_empty() {
            return Size::ZERO;
        }
        unsafe {
            let wide: Vec<u16> = text.encode_utf16().collect();
            let max_width = if m.layout_type.is_vertical_text() {
                0.0
            } else {
                m.max_width
            };
            let Ok(layout) = self
                .write
                .CreateTextLayout(&wide, format, max_width, 0.0)
            else {
                return Size::ZERO;
            };
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut metrics).is_err() {
                return Size::ZERO;
            }
            let mut size = Size::new(
                metrics.widthIncludingTrailingWhitespace.ceil(),
                metrics.height.ceil(),
            );
            // 官方把 layout 重新按实际宽高建一次再取 overhang
            if let Ok(again) = self.write.CreateTextLayout(
                &wide,
                format,
                metrics.widthIncludingTrailingWhitespace,
                metrics.height,
            ) {
                if let Ok(overhang) = again.GetOverhangMetrics() {
                    if overhang.left > 0.0 {
                        size.cx += overhang.left + 1.0;
                    }
                    if overhang.right > 0.0 {
                        size.cx += overhang.right + 1.0;
                    }
                    if overhang.top > 0.0 {
                        size.cy += overhang.top + 1.0;
                    }
                    if overhang.bottom > 0.0 {
                        size.cy += overhang.bottom + 1.0;
                    }
                }
            }
            size
        }
    }

    /// 官方 `StandardLayout::RecalculateSizes`：把所有文本先量好。
    fn measure_all(&self, m: &Metrics, view: &CandidateView) -> Measured {
        let count = view.items.len().min(layout::MAX_CANDIDATES);
        let mut measured = Measured {
            label: Vec::with_capacity(count),
            text: Vec::with_capacity(count),
            comment: Vec::with_capacity(count),
            ..Default::default()
        };
        for i in 0..count {
            let item = &view.items[i];
            // 序号：官方来自 rime 的 labels（"1" "2" …续页继续累加），再套 style/label_format
            let label = m.format_label(&(view.page_start + i as u32 + 1).to_string());
            measured.label.push(if m.label_font_valid {
                self.measure(&label, &self.formats.label, m)
            } else {
                Size::ZERO
            });
            measured.text.push(if m.text_font_valid {
                self.measure(&item.primary_text, &self.formats.text, m)
            } else {
                Size::ZERO
            });
            measured.comment.push(if m.comment_font_valid {
                self.measure(&item.secondary_text, &self.formats.comment, m)
            } else {
                Size::ZERO
            });
        }
        // 预编辑：我们的快照只给"整段组字中"，所以整段都是高亮段（官方 composition 的常态）
        if let Some(preedit) = &view.preedit {
            let size = self.measure(&preedit.text, &self.formats.preedit, m);
            measured.preedit = Parts {
                before: Size::ZERO,
                hilite: size,
                after: Size::ZERO,
                total: size,
            };
        }
        // 翻页字形（官方用 `<` / `>`，字体同预编辑）
        measured.page_prev = self.measure(layout::PAGE_PREV, &self.formats.preedit, m);
        measured.page_next = self.measure(layout::PAGE_NEXT, &self.formats.preedit, m);
        // 标记：官方在没有 mark_text 时用 "|" 的尺寸做基准
        measured.mark = if m.mark_text.is_empty() {
            self.measure("|", &self.formats.text, m)
        } else {
            self.measure(&m.mark_text, &self.formats.text, m)
        };
        measured
    }

    /// 为窗口惰性创建绘制目标；已有目标由窗口的 resize/DPI 消息调整。
    ///
    /// ntialias 取 style/antialias_mode：官方在 D2D::InitDirect2D 里设一次
    /// （d2d.cpp:183），几何固定逐图元抗锯齿（d2d.cpp:182）。
    fn ensure_target(
        &mut self,
        hwnd: HWND,
        dpi: u32,
        antialias: config::Antialias,
    ) -> windows_core::Result<()> {
        // 官方 d2d.cpp:136 —— 同一个窗口且交换链/目标还在时**直接返回**，绝不重建。
        // 重建会让 CreateTargetForHwnd 对已合成的窗口失败（ALREADY_COMPOSED）。
        if self.target.is_some() {
            return Ok(());
        }
        unsafe {
            let mut rc = RECT::default();
            GetClientRect(hwnd, &mut rc).ok()?;
            let width = (rc.right - rc.left).max(0) as u32;
            let height = (rc.bottom - rc.top).max(0) as u32;
            if width == 0 || height == 0 {
                // 窗口还没显示时客户区就是空的：官方这时直接返回，什么都不做
                return Ok(());
            }
            let presenter = crate::presenter::Presenter::new(hwnd, width, height).map_err(
                |message| {
                    crate::diagnostics::record(format_args!("呈现层创建失败: {message}"));
                    windows_core::Error::from_hresult(E_UNEXPECTED)
                },
            )?;
            presenter.render_target().SetDpi(dpi as f32, dpi as f32);
            apply_antialias(&presenter, antialias);
            self.target = Some(presenter);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- 状态

/// 一次呈现所需的不可变快照、事件发送端、实测尺寸与布局。
struct Content {
    snapshot: CandidateView,
    events: EventSink,
    measured: Measured,
    layout: Layout,
}

/// 窗口 UI 状态；由 [`Window::app`] 的动态借用保护，避免回调重入时别名可变访问。
struct App {
    /// 最近一次读配置时主题文件的修改时间；变了就重载（换皮肤立刻生效）。
    stamp: Option<std::time::SystemTime>,
    graphics: Graphics,
    content: Option<Content>,
    /// 原始（未按 DPI 缩放）的 style：DPI 变化时靠它重新缩放。
    raw_style: weasel_style::UIStyle,
    /// 当前生效的 style（含 DPI 缩放）与配色。
    metrics: Metrics,
    scheme: ColorScheme,
    /// 当前悬停与按下（官方 `m_hoverIndex` / 点击计时）。
    hovered: Option<Hit>,
    pressed: Option<Hit>,
    /// 官方 `m_sticky`：面板被翻到插入点上方后保持在上方。
    sticky: bool,
}

/// 窗口回调可访问的稳定状态分配。
struct Window {
    hwnd: Cell<HWND>,
    dpi: Cell<u32>,
    app: RefCell<App>,
    error: RefCell<Option<String>>,
    recovery: RefCell<Recovery>,
    positioning: Cell<bool>,
    /// 为真时采用可激活、可关闭的预览窗口行为。
    preview: bool,
}

/// Direct2D 设备丢失后的有限重试状态。
#[derive(Default)]
struct Recovery {
    failures: u8,
    waiting: bool,
}

impl Recovery {
    fn failed(&mut self, device_lost: bool) -> bool {
        if !device_lost || self.failures >= 3 {
            return false;
        }
        self.failures += 1;
        self.waiting = true;
        true
    }
    fn succeeded(&mut self) {
        self.failures = 0;
        self.waiting = false;
    }
}

/// 实现主题接口的轻量句柄；窗口回调通过共享的稳定分配访问状态。
struct Ten {
    window: Rc<Window>,
}

/// 将 DIP 长度转换为物理像素，DPI 至少按 1 处理。
fn pixels(dip: f32, dpi: u32) -> i32 {
    (dip * dpi.max(1) as f32 / 96.0).round() as i32
}

/// 快照是否"没有内容"（官方 `Context::empty`：无候选、无预编辑、无辅助串）。
fn empty_context(view: &CandidateView) -> bool {
    view.items.is_empty() && view.preedit.is_none()
}

// ---------------------------------------------------------------- 创建

fn create(mode: UiMode) -> Result<Box<dyn ThemeBackend>, String> {
    let preview = mode != UiMode::Live;
    let loaded = config::load(crate::appearance::is_dark());
    let antialias = config::Antialias::from_name(&loaded.style.antialias_mode);
    let window = Rc::new(Window {
        hwnd: Cell::new(HWND::default()),
        dpi: Cell::new(96),
        app: RefCell::new(App::new(loaded, 96.0)?),
        error: RefCell::new(None),
        recovery: RefCell::new(Recovery::default()),
        positioning: Cell::new(false),
        preview,
    });
    unsafe {
        let instance = GetModuleHandleW(None);
        if instance.0.is_null() {
            return Err(windows_core::Error::from_thread().to_string());
        }
        let icon = LoadIconW(Some(instance), w!("WEASEL_ICON"));
        let mut existing = WNDCLASSW::default();
        if !GetClassInfoW(Some(instance), CLASS, &mut existing).as_bool() {
            let wc = WNDCLASSW {
                hInstance: instance,
                lpszClassName: CLASS,
                lpfnWndProc: Some(wnd_proc),
                hCursor: LoadCursorW(None, IDC_ARROW),
                hIcon: icon,
                ..Default::default()
            };
            if wc.hCursor.0.is_null() || RegisterClassW(&wc).0 == 0 {
                return Err(windows_core::Error::from_thread().to_string());
            }
        }
        // 照官方 WeaselPanel.cpp:374-379：不要 WS_EX_LAYERED（会和 DComp 合成冲突，
        // 未设分层属性时整窗全透明）、不要 DWM 模糊、不要 SetWindowRgn。
        let (ex_style, title) = if preview {
            (
                (WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP) as u32,
                w!("Weasel-Next 皮肤预览"),
            )
        } else {
            (
                (WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
                    | WS_EX_NOREDIRECTIONBITMAP) as u32,
                w!("Weasel candidates"),
            )
        };
        let hwnd = CreateWindowExW(
            ex_style,
            CLASS,
            title,
            (WS_POPUP as u32 | WS_CLIPSIBLINGS as u32)
                | if preview { WS_SYSMENU as u32 } else { 0 },
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            Some(Rc::as_ptr(&window).cast()),
        );
        if hwnd.0.is_null() {
            return Err(windows_core::Error::from_thread().to_string());
        }
        window.hwnd.set(hwnd);
        window.dpi.set(GetDpiForWindow(hwnd).max(1));
        if preview {
            apply_taskbar_icon(hwnd, icon);
        }
        // DPI 定下来后重算一次缩放后的度量
        window.app.borrow_mut().set_dpi(window.dpi.get());
        window
            .app
            .borrow_mut()
            .graphics
            .ensure_target(hwnd, window.dpi.get(), antialias)
            .map_err(|e| e.to_string())?;
    }
    window.health()?;
    Ok(Box::new(Ten { window }))
}

unsafe fn apply_taskbar_icon(hwnd: HWND, icon: HICON) {
    if icon.0.is_null() {
        return;
    }
    unsafe {
        let _ = SendMessageW(
            hwnd,
            WM_SETICON as u32,
            WPARAM(ICON_SMALL as usize),
            LPARAM(icon.0 as isize),
        );
        let _ = SendMessageW(
            hwnd,
            WM_SETICON as u32,
            WPARAM(ICON_BIG as usize),
            LPARAM(icon.0 as isize),
        );
    }
}

impl App {
    fn new(loaded: config::Loaded, dpi: f32) -> Result<Self, String> {
        let metrics = Metrics::new(&loaded.style, dpi).with_colors(&loaded.scheme);
        crate::diagnostics::record(format_args!("主题配置: {}", loaded.source));
        Ok(Self {
            stamp: config::dirs().and_then(|(shared, user)| config::stamp(&shared, &user)),
            graphics: Graphics::new(&metrics).map_err(|e| e.to_string())?,
            content: None,
            raw_style: loaded.style,
            metrics,
            scheme: loaded.scheme,
            hovered: None,
            pressed: None,
            sticky: false,
        })
    }

    /// DPI 变了：按原始 style 重新缩放度量与字号。
    fn set_dpi(&mut self, dpi: u32) {
        self.metrics = Metrics::new(&self.raw_style, dpi.max(1) as f32).with_colors(&self.scheme);
        let _ = self.graphics.refresh_formats(&self.metrics);
        if let Some(content) = &mut self.content {
            content.measured = self.graphics.measure_all(&self.metrics, &content.snapshot);
            content.layout = Layout::build(&self.metrics, &content.snapshot, &content.measured);
        }
    }

    /// 重新读配置（明暗外观变化、用户改皮肤时调用）。
    fn reload(&mut self, dark: bool, dpi: u32) {
        self.stamp = config::dirs()
            .and_then(|(shared, user)| config::stamp(&shared, &user));
        let loaded = config::load(dark);
        crate::diagnostics::record(format_args!("主题配置: {}", loaded.source));
        self.raw_style = loaded.style;
        self.scheme = loaded.scheme;
        self.metrics =
            Metrics::new(&self.raw_style, dpi.max(1) as f32).with_colors(&self.scheme);
        let _ = self.graphics.refresh_formats(&self.metrics);
        if let Some(presenter) = self.graphics.target.as_ref() {
            apply_antialias(presenter, self.metrics.antialias);
        }
        if let Some(content) = &mut self.content {
            content.measured = self.graphics.measure_all(&self.metrics, &content.snapshot);
            content.layout = Layout::build(&self.metrics, &content.snapshot, &content.measured);
        }
    }
}

/// 官方 d2d.cpp:183：把 style/antialias_mode 套到设备上下文的文字抗锯齿上。
///
/// 几何（圆角矩形/描边）固定用逐图元抗锯齿，与官方 d2d.cpp:182 一致。
fn apply_antialias(presenter: &crate::presenter::Presenter, antialias: config::Antialias) {
    unsafe {
        presenter
            .render_target()
            .SetTextAntialiasMode(
                // 官方是直接强转枚举值，两边的取值定义相同（0..3）
                core::mem::transmute::<i32, D2D1_TEXT_ANTIALIAS_MODE>(antialias.d2d()),
            );
    }
}

impl Window {
    /// 记录首个故障，保留最初错误作为后续健康检查的结果。
    fn fail(&self, error: impl ToString) {
        let mut slot = self.error.borrow_mut();
        if slot.is_none() {
            *slot = Some(error.to_string());
        }
    }
    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd.get()), None, false);
        }
    }
    fn cancel(&self) {
        let mut app = self.app.borrow_mut();
        app.pressed = None;
        app.hovered = None;
        unsafe {
            if GetCapture() == self.hwnd.get() {
                let _ = ReleaseCapture();
            }
        }
    }
    fn health(&self) -> Result<(), String> {
        self.error
            .borrow()
            .as_ref()
            .map_or(Ok(()), |e| Err(e.clone()))
    }
    /// 丢弃失效的绘制目标，并对设备丢失进行有限次数的定时重试。
    fn graphics_error(&self, error: windows_core::Error) {
        if !self
            .recovery
            .borrow_mut()
            .failed(error.code() == D2DERR_RECREATE_TARGET)
        {
            self.fail(error);
            return;
        }
        unsafe {
            if SetTimer(Some(self.hwnd.get()), RETRY_TIMER, 100, None) == 0 {
                self.fail(windows_core::Error::from_thread());
            }
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let hwnd = self.hwnd.get();
        if !hwnd.0.is_null() {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                self.cancel();
                let _ = KillTimer(Some(hwnd), RETRY_TIMER);
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}

impl ThemeBackend for Ten {
    fn render(&mut self, snapshot: &CandidateView, events: &EventSink) -> Result<(), String> {
        self.window.render(snapshot, events)
    }
    fn hide(&mut self) {
        self.window.hide();
    }
    fn refresh_appearance(&mut self) -> Result<(), String> {
        let dark = crate::appearance::is_dark();
        let dpi = self.window.dpi.get();
        self.window.app.borrow_mut().reload(dark, dpi);
        self.window.invalidate();
        self.window.health()
    }
    fn check_health(&mut self) -> Result<(), String> {
        self.window.health()
    }
}

impl Window {
    /// 应用候选快照：内容没变就只重新定位，变了就重新测量 + 布局 + 绘制。
    fn render(&self, snapshot: &CandidateView, events: &EventSink) -> Result<(), String> {
        let same = {
            let mut app = self.app.borrow_mut();
            let App {
                graphics,
                content,
                metrics,
                ..
            } = &mut *app;
            match content.as_mut() {
                Some(content)
                    if crate::presentation::is_visible(snapshot)
                        || crate::presentation::is_mode_indicator_visible(snapshot) =>
                {
                    if crate::theme_api::same_content(&content.snapshot, snapshot) {
                        content.snapshot = snapshot.clone();
                        content.events = events.clone();
                        content.measured = graphics.measure_all(metrics, &content.snapshot);
                        content.layout =
                            Layout::build(metrics, &content.snapshot, &content.measured);
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            }
        };
        if same {
            self.position().map_err(|e| e.to_string())?;
            self.invalidate();
            return self.health();
        }
        self.cancel();
        self.health()?;
        if !(crate::presentation::is_visible(snapshot)
            || crate::presentation::is_mode_indicator_visible(snapshot))
        {
            self.hide();
            return Ok(());
        }
        // 主题文件被改过（例如刚在设置里换了皮肤）就重载一次，不必重启输入法
        {
            let changed = self
                .app
                .borrow()
                .stamp
                .ne(&config::dirs().and_then(|(shared, user)| config::stamp(&shared, &user)));
            if changed {
                let dark = crate::appearance::is_dark();
                let dpi = self.dpi.get();
                self.app.borrow_mut().reload(dark, dpi);
            }
        }
        let was_hidden = self.app.borrow().content.is_none();
        {
            let mut app = self.app.borrow_mut();
            let measured = app.graphics.measure_all(&app.metrics, snapshot);
            let layout = if snapshot.mode_indicator.is_some() && snapshot.items.is_empty() {
                Layout::mode_indicator(&app.metrics)
            } else {
                Layout::build(&app.metrics, snapshot, &measured)
            };
            crate::diagnostics::record(format_args!(
                "布局: 面板 {:.1}x{:.1} DIP · 内容 {:.1},{:.1}-{:.1},{:.1} · 候选 {} 行 · 预编辑 {} · 翻页 {}",
                layout.content_size.cx,
                layout.content_size.cy,
                layout.content.left,
                layout.content.top,
                layout.content.right,
                layout.content.bottom,
                layout.rows.len(),
                if layout.preedit.is_null() { "无".to_string() } else { format!("{:.1},{:.1}-{:.1},{:.1}", layout.preedit.left, layout.preedit.top, layout.preedit.right, layout.preedit.bottom) },
                if layout.page_prev.is_null() { "无".to_string() } else { format!("<{:.0},{:.0} >{:.0},{:.0}", layout.page_prev.left, layout.page_prev.top, layout.page_next.left, layout.page_next.top) },
            ));
            for (index, row) in layout.rows.iter().enumerate() {
                crate::diagnostics::record(format_args!(
                    "  行 {index}: 高亮 {:.1},{:.1}-{:.1},{:.1} · 序号 {:.1},{:.1}-{:.1},{:.1} · 正文 {:.1},{:.1}-{:.1},{:.1} · 注释 {:.1},{:.1}-{:.1},{:.1}",
                    row.rect.left, row.rect.top, row.rect.right, row.rect.bottom,
                    row.label.left, row.label.top, row.label.right, row.label.bottom,
                    row.text.left, row.text.top, row.text.right, row.text.bottom,
                    row.comment.left, row.comment.top, row.comment.right, row.comment.bottom,
                ));
            }
            app.content = Some(Content {
                snapshot: snapshot.clone(),
                events: events.clone(),
                measured,
                layout,
            });
        }
        self.position().map_err(|e| e.to_string())?;
        if was_hidden {
            // 隐藏期间窗口保留上一帧，先画一次再显示，避免闪出旧内容
            if let Err(error) = self.paint() {
                self.graphics_error(error);
            }
        }
        unsafe {
            let _ = ShowWindow(
                self.hwnd.get(),
                if self.preview { SW_SHOW } else { SW_SHOWNOACTIVATE },
            );
        }
        self.invalidate();
        self.health()
    }

    /// 隐藏窗口并释放交互状态。
    fn hide(&self) {
        self.cancel();
        self.app.borrow_mut().content = None;
        self.recovery.borrow_mut().waiting = false;
        unsafe {
            let _ = KillTimer(Some(self.hwnd.get()), RETRY_TIMER);
            let _ = ShowWindow(self.hwnd.get(), SW_HIDE);
        }
    }

    /// 绘制一帧（官方 `DoPaint`）。
    fn paint(&self) -> windows_core::Result<()> {
        let mut app = self.app.borrow_mut();
        let App {
            graphics,
            content,
            metrics,
            scheme,
            hovered,
            ..
        } = &mut *app;
        graphics.ensure_target(self.hwnd.get(), self.dpi.get(), metrics.antialias)?;
        let Some(presenter) = graphics.target.as_ref() else {
            // 客户区还是空的：等窗口显示后的下一条消息再来建目标
            return Ok(());
        };
        let target = presenter.render_target();
        let scheme = &*scheme;
        let metrics = &*metrics;
        unsafe {
            let brush = target.CreateSolidColorBrush(&d2d_color(scheme.back_color), None)?;
            presenter.begin_draw();
            let draw = DrawGuard {
                presenter,
                ended: false,
            };
            // 官方：Clear 成全透明，圆角之外的像素没人画 → 透出背后内容
            target.Clear(Some(&D2D_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            if let Some(content) = content.as_ref() {
                let mut painter = Painter::new(
                    &target,
                    presenter.d2d_factory(),
                    &graphics.write,
                    &brush,
                    &graphics.formats,
                    metrics,
                    scheme,
                    &content.snapshot,
                );
                painter.hover_index = match hovered {
                    Some(Hit::Candidate(i)) => Some(*i),
                    _ => None,
                };
                if content.snapshot.mode_indicator.is_some() && content.snapshot.items.is_empty() {
                    draw_mode_indicator(
                        &painter,
                        &content.layout,
                        &graphics.indicator,
                        content.snapshot.ascii_mode.unwrap_or(false),
                    );
                } else {
                    let empty = empty_context(&content.snapshot);
                    painter
                        .draw(&content.layout, empty)
                        .map_err(|message| {
                            crate::diagnostics::record(format_args!("绘制失败: {message}"));
                            windows_core::Error::from_hresult(E_UNEXPECTED)
                        })?;
                }
            }
            draw.finish()?;
        }
        drop(app);
        self.recovery.borrow_mut().succeeded();
        Ok(())
    }

    /// 官方 `WeaselPanel::_ResizeWindow` + `D2D::OnResize`：把交换链尺寸对齐客户区。
    ///
    /// 必须做：窗口先以 1×1 创建（官方是 10×10），绘制目标若在定位前建好就是那个小尺寸，
    /// 之后不 `ResizeBuffers` 的话合成出去的永远是那张小图（看起来"什么都没有"）。
    fn resize(&self) -> windows_core::Result<()> {
        let mut app = self.app.borrow_mut();
        let dpi = self.dpi.get() as f32;
        let Some(presenter) = app.graphics.target.as_mut() else {
            return Ok(());
        };
        unsafe {
            let mut rc = RECT::default();
            GetClientRect(self.hwnd.get(), &mut rc).ok()?;
            let width = (rc.right - rc.left).max(1) as u32;
            let height = (rc.bottom - rc.top).max(1) as u32;
            presenter.render_target().SetDpi(dpi, dpi);
            let outcome = presenter.resize(width, height).map_err(|message| {
                crate::diagnostics::record(format_args!("交换链尺寸调整失败: {message}"));
                windows_core::Error::from_hresult(E_UNEXPECTED)
            });
            drop(app);
            outcome?;
        }
        Ok(())
    }

    /// 官方 `_Reposition`（WeaselPanel.cpp:825）：默认贴插入点下方，放不下就翻到上方。
    fn position(&self) -> windows_core::Result<()> {
        if self.positioning.replace(true) {
            return Ok(());
        }
        struct Reset<'a>(&'a Cell<bool>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _reset = Reset(&self.positioning);
        for _ in 0..3 {
            let dpi = self.dpi.get();
            // 注意：sticky 必须在**借用块之外**读写。
            // 之前把 popup_position()（内部要 borrow_mut 写 sticky）放在持有 `app`
            // 不可变借用的闭包里 —— RefCell 双重借用直接 panic，实时模式一打字渲染器就退出，
            // 表现就是"候选窗看不见、但能打字"。preview 走的是另一条分支，所以预览测不出来。
            let sticky = self.app.borrow().sticky;
            let bounds = {
                let app = self.app.borrow();
                let Some(content) = app.content.as_ref() else {
                    return Ok(());
                };
                let width = pixels(content.layout.content_size.cx, dpi);
                let height = pixels(content.layout.content_size.cy, dpi);
                if self.preview {
                    let (x, y) = crate::presentation::preview_position(width, height);
                    Some((x, y, width, height, None))
                } else {
                    content
                        .snapshot
                        .anchor
                        .as_ref()
                        .filter(|anchor| anchor.valid)
                        .map(|anchor| {
                            let work = crate::presentation::work_area(anchor);
                            let placed = reposition(
                                anchor,
                                work.map(|work| {
                                    (work.left, work.top, work.right, work.bottom)
                                }),
                                &app.metrics,
                                width,
                                height,
                                dpi,
                                sticky,
                            );
                            (placed.x, placed.y, width, height, Some(placed.sticky))
                        })
                }
            };
            let Some((x, y, width, height, new_sticky)) = bounds else {
                return Ok(());
            };
            // 借用块已结束，这里写 sticky 是安全的
            if let Some(sticky) = new_sticky {
                self.app.borrow_mut().sticky = sticky;
            }
            unsafe {
                SetWindowPos(
                    self.hwnd.get(),
                    Some(HWND_TOPMOST),
                    x,
                    y,
                    width,
                    height,
                    (SWP_NOACTIVATE | SWP_NOREDRAW) as u32,
                )
                .ok()?;
            }
            if dpi == self.dpi.get() {
                break;
            }
        }
        self.resize()?;
        Ok(())
    }

    /// 把鼠标坐标换算成 DIP 并命中。
    fn hit(&self, param: LPARAM) -> Option<Hit> {
        let app = self.app.borrow();
        let scale = 96.0 / self.dpi.get().max(1) as f32;
        let x = param.0 as u16 as i16 as f32 * scale;
        let y = (param.0 >> 16) as u16 as i16 as f32 * scale;
        app.content
            .as_ref()
            .and_then(|c| c.layout.hit(&app.metrics, x, y))
    }
}

/// 官方 `WeaselPanel::_Reposition`（WeaselPanel.cpp:825）的坐标数学，纯函数便于测。
///
/// 与官方逐条对应：
/// 1. 锚点用插入点，并把下边界下移 6 像素（官方 `MoveTo` 里 `m_inputPos.bottom += 6`）；
/// 2. 开了阴影时按 `offset` 回退，避免阴影把面板整体推离插入点；
/// 3. 左右夹进工作区；
/// 4. 下方放不下（或已经 sticky）就翻到插入点上方 6 像素，并记住 sticky；
/// 5. 最后再夹一次上边界。
///
/// 返回的 `sticky` 要写回窗口状态：官方 `m_sticky` 一旦置上，插入点没大幅移动前一直生效。
struct Position {
    x: i32,
    y: i32,
    sticky: bool,
}

#[allow(clippy::too_many_arguments)]
fn reposition(
    anchor: &crate::theme_api::Anchor,
    work: Option<(i32, i32, i32, i32)>,
    m: &Metrics,
    width: i32,
    height: i32,
    dpi: u32,
    sticky: bool,
) -> Position {
    let (work_left, work_top, work_right, work_bottom) = work.unwrap_or((0, 0, 1920, 1080));
    let mut x = anchor.left;
    let mut y = anchor.bottom + CARET_GAP;
    if m.shadow_radius != 0.0 {
        x -= pixels(m.offset().0, dpi);
        y -= pixels(m.offset().1, dpi);
    }
    if x + width > work_right {
        x = work_right - width;
    }
    if x < work_left {
        x = work_left;
    }
    let mut sticky = sticky;
    if y + height > work_bottom || sticky {
        sticky = true;
        y = anchor.top - CARET_GAP - height;
        if m.shadow_radius != 0.0 && m.shadow_offset_y > 0.0 {
            y -= pixels(m.shadow_offset_y, dpi);
        }
    }
    if y < work_top {
        y = work_top;
    }
    Position { x, y, sticky }
}

/// 模式提示（官方没有这个窗口：它用状态图标；我们按同一套配色画一个小提示）。
fn draw_mode_indicator(
    painter: &Painter<'_>,
    layout: &Layout,
    format: &IDWriteTextFormat,
    ascii: bool,
) {
    // 和候选窗同款的圆角底 + 描边（用户要的"边框"）：内容就是那个方形面板
    painter.highlight_rect(
        &layout.content,
        painter.m.round_corner_ex,
        painter.m.border,
        painter.scheme.back_color,
        painter.scheme.shadow_color,
        painter.scheme.border_color,
        &layout::RoundInfo::default(),
    );
    // 面板里居中放 32×32 的状态图标（中/英各一枚，按底色深浅选一套）
    let icon = if layout.status_icon.is_null() {
        layout.content
    } else {
        layout.status_icon
    };
    let pixels = (icon.width() * painter.m.dpi_scale).round().max(1.0) as u32;
    let dark_background = is_dark_color(painter.scheme.back_color);
    if painter.draw_mode_icon(&icon, ascii, pixels, dark_background) {
        crate::diagnostics::record(format_args!(
            "模式提示: 图标 {} {pixels}px · 面板 {:.1}x{:.1} DIP",
            if ascii { "en" } else { "zh" },
            layout.content_size.cx,
            layout.content_size.cy
        ));
        return;
    }
    // 图标读不到（缺文件/加载失败）时退化成居中文字，至少能看出中英状态
    crate::diagnostics::record(format_args!(
        "模式提示: 图标不可用，退化为文字 {}",
        if ascii { "英" } else { "中" }
    ));
    let text = if ascii { "英" } else { "中" };
    unsafe {
        let wide: Vec<u16> = text.encode_utf16().collect();
        if let Ok(text_layout) = painter.write.CreateTextLayout(
            &wide,
            format,
            layout.content.width(),
            layout.content.height(),
        ) {
            let _ = text_layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = text_layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = painter.brush.SetColor(&d2d_color(painter.scheme.text_color));
            painter.target.DrawTextLayout(
                windows_numerics::Vector2 {
                    x: layout.content.left,
                    y: layout.content.top,
                },
                &text_layout,
                painter.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
        }
    }
}

/// 底色是不是深的（决定用 `*-light.ico` 还是 `*-dark.ico` 那一套图标）。
///
/// 官方按 `appearance::is_dark()`（系统深浅色）取图标；我们的图标是按**面板底色**
/// 分的两套，所以这里看配色本身的亮度，跟着配色走更稳。
fn is_dark_color(argb: u32) -> bool {
    let r = ((argb >> 16) & 0xff) as f32;
    let g = ((argb >> 8) & 0xff) as f32;
    let b = (argb & 0xff) as f32;
    // 感知亮度（Rec. 601），阈值 128
    0.299 * r + 0.587 * g + 0.114 * b < 128.0
}

struct PaintGuard {
    hwnd: HWND,
    ps: PAINTSTRUCT,
}
impl PaintGuard {
    unsafe fn begin(hwnd: HWND) -> Self {
        let mut ps = PAINTSTRUCT::default();
        unsafe {
            let _ = BeginPaint(hwnd, &mut ps);
        }
        Self { hwnd, ps }
    }
}
impl Drop for PaintGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = EndPaint(self.hwnd, &self.ps);
        }
    }
}

struct DrawGuard<'a> {
    presenter: &'a crate::presenter::Presenter,
    ended: bool,
}
impl DrawGuard<'_> {
    fn finish(mut self) -> windows_core::Result<()> {
        self.ended = true;
        self.presenter.end_draw().map_err(|message| {
            crate::diagnostics::record(format_args!("呈现失败: {message}"));
            windows_core::Error::from_hresult(E_UNEXPECTED)
        })
    }
}
impl Drop for DrawGuard<'_> {
    fn drop(&mut self) {
        if !self.ended {
            let _ = self.presenter.end_draw();
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match catch_unwind(AssertUnwindSafe(|| unsafe { dispatch(hwnd, msg, wp, lp) })) {
        Ok(result) => result,
        Err(_) => {
            unsafe {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Window;
                if let Some(window) = ptr.as_ref() {
                    if let Ok(mut error) = window.error.try_borrow_mut() {
                        *error = Some("official 主题原生回调 panic".into());
                    }
                }
            }
            LRESULT(0)
        }
    }
}

/// 分发窗口消息：绘制、DPI、恢复、鼠标手势（官方 `WeaselPanel::MsgHandler`）。
unsafe fn dispatch(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE as u32 {
            let create = &*(lp.0 as *const CREATESTRUCTW);
            let ptr = create.lpCreateParams as *const Window;
            if ptr.is_null() {
                return LRESULT(0);
            }
            (*ptr).hwnd.set(hwnd);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
            return LRESULT(1);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Window;
        if let Some(window) = ptr.as_ref() {
            match msg as i32 {
                WM_PAINT => {
                    let _paint = PaintGuard::begin(hwnd);
                    let can_paint =
                        window.error.borrow().is_none() && !window.recovery.borrow().waiting;
                    if can_paint {
                        if let Err(e) = window.paint() {
                            window.graphics_error(e);
                        }
                    }
                    return LRESULT(0);
                }
                WM_ERASEBKGND => return LRESULT(1),
                WM_CLOSE => {
                    if window.preview {
                        PostQuitMessage(0);
                        return LRESULT(0);
                    }
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                WM_MOUSEACTIVATE => {
                    if window.preview {
                        return DefWindowProcW(hwnd, msg, wp, lp);
                    }
                    return LRESULT(MA_NOACTIVATE as isize);
                }
                WM_SIZE => {
                    if let Err(e) = window.resize() {
                        window.graphics_error(e);
                    }
                    window.invalidate();
                    return LRESULT(0);
                }
                WM_DPICHANGED => {
                    window.dpi.set((wp.0 as u32 & 0xffff).max(1));
                    window.cancel();
                    window.app.borrow_mut().set_dpi(window.dpi.get());
                    if let Err(e) = window.position() {
                        window.fail(e);
                    }
                    window.invalidate();
                    return LRESULT(0);
                }
                WM_TIMER if wp.0 == RETRY_TIMER => {
                    let _ = KillTimer(Some(hwnd), RETRY_TIMER);
                    window.recovery.borrow_mut().waiting = false;
                    window.invalidate();
                    return LRESULT(0);
                }
                WM_TIMER if wp.0 == AUTOREV_TIMER => {
                    let _ = KillTimer(Some(hwnd), AUTOREV_TIMER);
                    window.invalidate();
                    return LRESULT(0);
                }
                WM_TIMER if wp.0 == AUTOHIDE_TIMER => {
                    let _ = KillTimer(Some(hwnd), AUTOHIDE_TIMER);
                    window.hide();
                    return LRESULT(0);
                }
                WM_MOUSEMOVE => {
                    let hit = window.hit(lp);
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE as u32,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut track);
                    let mut app = window.app.borrow_mut();
                    let changed = app.hovered != hit;
                    // 官方 hover_type = none 时完全不悬停
                    app.hovered = if app.metrics.hover == config::Hover::None {
                        None
                    } else {
                        hit
                    };
                    if changed {
                        crate::diagnostics::record(format_args!(
                            "悬停: {:?} → {:?}（hover_type={:?}）",
                            changed, app.hovered, app.metrics.hover
                        ));
                    }
                    if changed {
                        drop(app);
                        window.invalidate();
                    }
                    return LRESULT(0);
                }
                WM_LBUTTONDOWN => {
                    let hit = window.hit(lp);
                    let event = {
                        let mut app = window.app.borrow_mut();
                        match hit {
                            // 官方在按下时就翻页（WeaselPanel.cpp:1116-1143）
                            Some(Hit::Previous) => {
                                app.pressed = None;
                                Some(UiAction::NavigatePrevious)
                            }
                            Some(Hit::Next) => {
                                app.pressed = None;
                                Some(UiAction::NavigateNext)
                            }
                            other => {
                                app.pressed = other;
                                None
                            }
                        }
                    };
                    if event.is_none() {
                        let _ = SetCapture(hwnd);
                    }
                    if let Some(action) = event {
                        let sender = window
                            .app
                            .borrow()
                            .content
                            .as_ref()
                            .map(|c| c.events.clone());
                        if let Some(sender) = sender {
                            sender.send(action);
                        }
                    }
                    return LRESULT(0);
                }
                WM_LBUTTONUP => {
                    let hit = window.hit(lp);
                    let event = {
                        let mut app = window.app.borrow_mut();
                        let pressed = app.pressed.take();
                        let selected = app
                            .content
                            .as_ref()
                            .map(|c| c.snapshot.selected_index as usize);
                        match (pressed, hit) {
                            // 官方：只有点在**当前高亮**的候选上才算选中（L1050-1057）
                            (Some(Hit::Candidate(i)), Some(Hit::Candidate(j)))
                                if i == j && Some(i) == selected =>
                            {
                                Some(UiAction::ItemInvoked(i as u32))
                            }
                            _ => None,
                        }
                    };
                    window.cancel();
                    if let Some(action) = event {
                        let sender = window
                            .app
                            .borrow()
                            .content
                            .as_ref()
                            .map(|c| c.events.clone());
                        if let Some(sender) = sender {
                            sender.send(action);
                        }
                    }
                    window.invalidate();
                    return LRESULT(0);
                }
                WM_MOUSEWHEEL => {
                    // 官方 OnScroll：paging_on_scroll 打开时滚轮翻页
                    let delta = ((wp.0 >> 16) & 0xffff) as u16 as i16;
                    let action = {
                        let app = window.app.borrow();
                        if !app.metrics.paging_on_scroll || delta == 0 {
                            None
                        } else if delta < 0 && app.content.as_ref().is_some_and(|c| c.snapshot.can_page_next)
                        {
                            Some(UiAction::NavigateNext)
                        } else if delta > 0
                            && app
                                .content
                                .as_ref()
                                .is_some_and(|c| c.snapshot.can_page_previous)
                        {
                            Some(UiAction::NavigatePrevious)
                        } else {
                            None
                        }
                    };
                    if let Some(action) = action {
                        let sender = window
                            .app
                            .borrow()
                            .content
                            .as_ref()
                            .map(|c| c.events.clone());
                        if let Some(sender) = sender {
                            sender.send(action);
                        }
                    }
                    return LRESULT(0);
                }
                WM_MOUSELEAVE | WM_CAPTURECHANGED | WM_CANCELMODE => {
                    window.cancel();
                    window.invalidate();
                    return LRESULT(0);
                }
                WM_NCDESTROY => {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    window.hwnd.set(HWND::default());
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

/// 官方皮肤主题的工厂入口。
pub struct Factory;

impl crate::theme_api::ThemeFactory for Factory {
    fn name(&self) -> &'static str {
        "official"
    }
    /// 候选栏 + 预编辑（官方会把未行内显示的组字串画在候选窗里）+ 模式提示。
    fn capabilities(&self) -> crate::theme_api::ThemeCapabilities {
        crate::theme_api::ThemeCapabilities {
            preedit: true,
            resident: false,
            mode_indicator: true,
        }
    }
    fn create(
        &self,
        mode: UiMode,
        settings: &weasel_common::settings::ConfigSnapshot,
    ) -> crate::theme_api::ThemeCreation {
        (|| -> Result<Box<dyn ThemeBackend>, String> {
            let _: serde_json::Map<String, serde_json::Value> = settings.theme_settings("official")?;
            create(mode)
        })()
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme_api::Anchor;
    use weasel_style::UIStyle;

    fn metrics(shadow_radius: i32) -> Metrics {
        let mut style = UIStyle::default();
        style.layout.shadow_radius = shadow_radius;
        Metrics::new(&style, 96.0)
    }

    fn anchor(left: i32, top: i32, right: i32, bottom: i32) -> Anchor {
        Anchor {
            left,
            top,
            right,
            bottom,
            valid: true,
        }
    }

    #[test]
    fn places_below_caret_with_official_six_pixel_gap() {
        let m = metrics(0);
        let work = Some((0, 0, 1920, 1080));
        // 插入点在屏幕上方，下方空间充足 → 面板顶边 = 插入点底边 + 6
        let placed = reposition(&anchor(100, 200, 104, 220), work, &m, 300, 200, 96, false);
        assert_eq!((placed.x, placed.y), (100, 226));
        assert!(!placed.sticky, "放得下就不该进入 sticky");
    }

    #[test]
    fn flips_above_caret_when_bottom_is_too_close() {
        let m = metrics(0);
        let work = Some((0, 0, 1920, 1080));
        // 插入点底部离工作区底部只剩 100px，面板高 200 → 翻到上方：top - 6 - 200
        let placed = reposition(&anchor(100, 900, 104, 1000), work, &m, 300, 200, 96, false);
        assert_eq!(placed.y, 900 - 6 - 200);
        assert!(placed.sticky, "翻上去之后要保持 sticky");
        // sticky 会一直生效，即使插入点又回到了下方有空间的位置；
        // 翻上去后越出工作区顶时，官方最后还会再夹一次（所以这里是 0 而不是 -6）
        let again = reposition(&anchor(100, 200, 104, 220), work, &m, 300, 200, 96, true);
        assert_eq!(again.y, 0, "翻到上方越界后被夹到工作区顶");
        assert!(again.sticky);
    }

    #[test]
    fn clamps_into_work_area_on_both_sides() {
        let m = metrics(0);
        let work = Some((100, 50, 500, 400));
        // 右边越界 → 贴右
        let right = reposition(&anchor(480, 100, 484, 120), work, &m, 300, 100, 96, false);
        assert_eq!(right.x, 500 - 300);
        // 左边越界 → 贴左
        let left = reposition(&anchor(20, 100, 24, 120), work, &m, 300, 100, 96, false);
        assert_eq!(left.x, 100);
        // 下方放不下（86 + 400 > 400）→ 翻到上方，再越界 → 夹到工作区顶
        let top = reposition(&anchor(120, 60, 124, 80), work, &m, 300, 400, 96, false);
        assert_eq!(top.y, 50);
        assert!(top.sticky);
    }

    #[test]
    fn shadow_offsets_pull_the_panel_back_by_offset() {
        // 开阴影后 offset = shadow_offset + radius*2 - radius/2 + border*2
        let m = metrics(6);
        let (ox, oy) = m.offset();
        let work = Some((0, 0, 1920, 1080));
        let placed = reposition(&anchor(300, 400, 304, 420), work, &m, 300, 200, 96, false);
        assert_eq!(placed.x, 300 - ox.round() as i32);
        assert_eq!(placed.y, 420 + CARET_GAP - oy.round() as i32);
    }

    #[test]
    fn no_work_area_falls_back_to_a_default_screen() {
        let m = metrics(0);
        let placed = reposition(&anchor(10, 10, 14, 30), None, &m, 200, 100, 96, false);
        assert_eq!(placed.x, 10, "拿不到显示器信息时仍按插入点摆");
        assert_eq!(placed.y, 36);
    }

    #[test]
    fn antialias_names_match_official() {
        use config::Antialias;
        assert_eq!(Antialias::from_name("default"), Antialias::Default);
        assert_eq!(Antialias::from_name("cleartype"), Antialias::ClearType);
        assert_eq!(Antialias::from_name("grayscale"), Antialias::Grayscale);
        assert_eq!(Antialias::from_name("aliased"), Antialias::Aliased);
        assert_eq!(Antialias::from_name("没写"), Antialias::Default);
        // 数值与 D2D1_TEXT_ANTIALIAS_MODE 一致（官方直接强转）
        assert_eq!(
            [
                Antialias::Default.d2d(),
                Antialias::ClearType.d2d(),
                Antialias::Grayscale.d2d(),
                Antialias::Aliased.d2d()
            ],
            [
                D2D1_TEXT_ANTIALIAS_MODE_DEFAULT as i32,
                D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE as i32,
                D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE as i32,
                D2D1_TEXT_ANTIALIAS_MODE_ALIASED as i32,
            ]
        );
    }
}
