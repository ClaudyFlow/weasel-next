//! Windows 10 候选栏主题后端，使用 Direct2D/DirectWrite 绘制独立弹出窗口。
//!
//! HWND、图形资源和输入状态均由本模块内部管理；窗口消息回调与主题接口共享稳定的
//! [`Window`] 分配，所有原生窗口和图形操作都应在创建它们的 UI 线程执行。
mod logic;

use crate::d2d_bindings::*;
use crate::theme_api::CandidateView;
use crate::{
    theme_api::EventSink,
    theme_api::{ThemeBackend, UiMode},
};
use logic::{Gesture, HEIGHT, Hit, Layout, NUMBER, Palette, Recovery, SCALE, enabled, pixels};
use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};
use windows_strings::w;

const CLASS: windows_strings::PCWSTR = w!("Weasel.ThemeOfficial.Official");
const RETRY_TIMER: usize = 1;

/// 持有 Direct2D 工厂、DirectWrite 格式及按需创建的 HWND 绘制目标。
///
/// 工厂和绘制目标随窗口在 UI 线程使用；目标失效后清空并在后续绘制时重建。
struct Graphics {
    /// 创建 HWND 绘制目标的单线程 Direct2D 工厂。
    factory: ID2D1Factory,
    /// 供文本测量和布局使用的共享 DirectWrite 工厂。
    write: IDWriteFactory,
    /// 候选序号格式。
    number: IDWriteTextFormat,
    /// 候选主文本格式。
    text: IDWriteTextFormat,
    /// 候选次要文本格式。
    comment: IDWriteTextFormat,
    /// 中英文模式提示格式。
    mode: IDWriteTextFormat,
    /// 翻页和表情操作的图标格式。
    icon: IDWriteTextFormat,
    /// 与此 HWND 关联、可在设备丢失后丢弃并重建的绘制目标。
    target: Option<ID2D1HwndRenderTarget>,
}

impl Graphics {
    /// 创建共享的 DirectWrite 工厂和本主题所需的字体格式。
    ///
    /// 字体格式创建失败会向上传播，使主题工厂能在窗口显示前回退。
    fn new() -> windows_core::Result<Self> {
        unsafe {
            let factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let write: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let format = |font, size, align| -> windows_core::Result<IDWriteTextFormat> {
                let f = write.CreateTextFormat(
                    font,
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size * SCALE,
                    w!("zh-CN"),
                )?;
                f.SetTextAlignment(align).ok()?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
                    .ok()?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP).ok()?;
                Ok(f)
            };
            Ok(Self {
                factory,
                number: format(w!("Segoe UI"), 27.0, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
                text: format(
                    w!("Microsoft YaHei UI"),
                    27.0,
                    DWRITE_TEXT_ALIGNMENT_LEADING,
                )?,
                comment: format(
                    w!("Microsoft YaHei UI"),
                    20.0,
                    DWRITE_TEXT_ALIGNMENT_LEADING,
                )?,
                mode: format(w!("Microsoft YaHei UI"), 25.0, DWRITE_TEXT_ALIGNMENT_CENTER)?,
                icon: format(w!("Segoe MDL2 Assets"), 20.0, DWRITE_TEXT_ALIGNMENT_CENTER)?,
                write,
                target: None,
            })
        }
    }
    /// 按给定格式测量文本宽度，结果以 DIP 表示并包含尾随空白。
    ///
    /// 调用方在内容布局阶段逐项测量，因此这里不缓存临时文本布局。
    fn measure(&self, text: &str, format: &IDWriteTextFormat) -> windows_core::Result<f32> {
        unsafe {
            let text: Vec<u16> = text.encode_utf16().collect();
            let layout = self
                .write
                .CreateTextLayout(&text, format, 1_000_000.0, HEIGHT)?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics).ok()?;
            Ok(metrics.widthIncludingTrailingWhitespace)
        }
    }
    /// 为窗口惰性创建绘制目标；已有目标由窗口的 resize/DPI 消息调整。
    ///
    /// 创建失败由调用方转换为主题错误或进入设备丢失恢复流程。
    fn ensure_target(&mut self, hwnd: HWND, dpi: u32) -> windows_core::Result<()> {
        if self.target.is_none() {
            unsafe {
                let mut rc = RECT::default();
                GetClientRect(hwnd, &mut rc).ok()?;
                let properties = D2D1_RENDER_TARGET_PROPERTIES {
                    dpiX: dpi as f32,
                    dpiY: dpi as f32,
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                self.target = Some(self.factory.CreateHwndRenderTarget(
                    &properties,
                    &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                        hwnd,
                        pixelSize: D2D_SIZE_U {
                            width: (rc.right - rc.left).max(1) as u32,
                            height: (rc.bottom - rc.top).max(1) as u32,
                        },
                        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                    },
                )?);
            }
        }
        Ok(())
    }
}

/// 一次呈现所需的不可变快照、事件发送端和预计算布局。
///
/// `primary_widths` 与候选项按相同索引对应，用于绘制时定位次要文本。
struct Content {
    /// 当前呈现的数据快照；索引与 `layout` 中的候选区域一致。
    snapshot: CandidateView,
    /// 点击操作发往渲染器的事件通道。
    events: EventSink,
    /// 根据文本测量宽度构造的命中与绘制几何。
    layout: Layout,
    primary_widths: Vec<f32>,
}
/// 窗口 UI 状态；由 [`Window::app`] 的动态借用保护，避免回调重入时别名可变访问。
struct App {
    /// 本窗口的 Direct2D/DirectWrite 资源。
    graphics: Graphics,
    /// 当前内容；为 `None` 时窗口没有可呈现的候选栏。
    content: Option<Content>,
    /// 当前鼠标按下与悬停状态。
    gesture: Gesture,
    /// 绘制时使用的当前外观色板。
    palette: Palette,
}

/// 窗口回调可访问的稳定状态分配。
///
/// 创建 HWND 时将此对象的地址写入窗口数据；因此分配必须至少存活到 HWND 销毁，且
/// `Rc` 持有者不得在窗口仍可能回调时释放。会同步派发消息的原生调用不得持有此对象
/// 或其 [`App`] 的独占引用；内部使用 `Cell`/`RefCell` 并在调用边界前结束借用。
struct Window {
    /// 关联的 HWND；窗口销毁后由 `WM_NCDESTROY` 清空。
    hwnd: Cell<HWND>,
    /// 当前窗口 DPI，始终至少为 1。
    dpi: Cell<u32>,
    /// 回调与后端共享的 UI/图形状态，借用冲突用于阻止重入别名访问。
    app: RefCell<App>,
    /// 首个原生操作错误或最近一次回调 panic 的文本表示。
    error: RefCell<Option<String>>,
    /// Direct2D 目标恢复预算与定时等待状态。
    recovery: RefCell<Recovery>,
    /// 防止 `SetWindowPos` 同步派发 DPI 消息时递归定位。
    positioning: Cell<bool>,
    /// 为真时采用可激活、可关闭的预览窗口行为。
    preview: bool,
}

/// 实现主题接口的轻量句柄；窗口回调通过共享的稳定分配访问状态。
struct Ten {
    /// 保持 HWND 保存的指针稳定，并允许后端移动时不移动窗口状态。
    window: Rc<Window>,
}

/// 初始化图形资源和窗口，并返回候选栏后端。
///
/// `Live` 模式创建不激活且不显示任务栏按钮的工具窗口，其他模式创建可关闭的预览窗。
/// 窗口类、HWND 或初始绘制目标创建失败时返回错误；调用方可在首次显示前回退。
fn create(mode: UiMode) -> Result<Box<dyn ThemeBackend>, String> {
    let preview = mode != UiMode::Live;
    let window = Rc::new(Window {
        hwnd: Cell::new(HWND::default()),
        dpi: Cell::new(96),
        app: RefCell::new(App {
            graphics: Graphics::new().map_err(|e| e.to_string())?,
            content: None,
            gesture: Gesture::default(),
            palette: logic::palette_from_official(crate::appearance::is_dark()),
        }),
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
        // The class icon is the task-bar button icon for the preview window. The
        // live tool window has no task-bar presence, so it is harmless there too.
        let icon = LoadIconW(Some(instance), w!("WEASEL_ICON"));
        let mut existing = WNDCLASSW::default();
        if !GetClassInfoW(Some(instance), CLASS, &mut existing).as_bool() {
            let wc = WNDCLASSW {
                hInstance: instance,
                lpszClassName: CLASS,
                lpfnWndProc: Some(wnd_proc),
                hCursor: LoadCursorW(None, IDC_ARROW),
                // Class icon also acts as the preview task-bar icon fallback.
                hIcon: icon,
                ..Default::default()
            };
            if wc.hCursor.0.is_null() || RegisterClassW(&wc).0 == 0 {
                return Err(windows_core::Error::from_thread().to_string());
            }
        }
        // The preview keeps the borderless strip identical to input but must be
        // findable and closable: no WS_EX_TOOLWINDOW (task-bar button) and no
        // WS_EX_NOACTIVATE (activatable, so it can be closed).
        let (ex_style, title) = if preview {
            ((WS_EX_TOPMOST | WS_EX_LAYERED) as u32, w!("Weasel-Next 皮肤预览"))
        } else {
            (
                (WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED) as u32,
                w!("Weasel candidates"),
            )
        };
        let hwnd = CreateWindowExW(
            ex_style,
            CLASS,
            title,
            WS_POPUP | if preview { WS_SYSMENU as u32 } else { 0 },
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
        // 玻璃第 1 步：给原生窗口打开整窗模糊（hRgnBlur = NULL → 整个客户区）。
        // ⚠️ 单靠它看不见效果：当前的 ID2D1HwndRenderTarget 会不透明地盖住它。
        // 第 2 步（把渲染目标换成 ID2D1DCRenderTarget + 32 位 DIB，用 UpdateLayeredWindow
        // 提交逐像素 alpha）落地后，这一步才会真正显示为毛玻璃。
        #[repr(C)]
        struct DwmBlurBehind {
            flags: u32,
            enable: i32,
            blur_region: isize,
            transition_on_maximized: i32,
        }
        const DWM_BB_ENABLE: u32 = 0x1;
        #[link(name = "dwmapi")]
        unsafe extern "system" {
            fn DwmEnableBlurBehindWindow(hwnd: HWND, bb: *const DwmBlurBehind) -> i32;
        }
        let blur = DwmBlurBehind {
            flags: DWM_BB_ENABLE,
            enable: 0,   // [Weasel-Next] 关掉整窗 DWM 模糊: blur_region=NULL 会按矩形铺满窗口, 圆角之外会露出方形模糊层
            blur_region: 0,
            transition_on_maximized: 0,
        };
        unsafe {
            let _ = DwmEnableBlurBehindWindow(hwnd, &blur);
        }
        // 玻璃加强：在"逐像素 alpha + 透明清屏"都已具备的前提下，再上
        // ACCENT_ENABLE_ACRYLICBLURBEHIND —— 这才是**强糊**（Aero/亚克力那种），
        // 而 DwmEnableBlurBehindWindow 的模糊很弱、在纯色背景上几乎看不出来。
        // tint 用 40% 淡白（AABBGGRR）：既保住白字可读，又让面板是一块可辨的玻璃。
        #[repr(C)]
        struct AccentPolicy {
            state: i32,
            flags: i32,
            gradient_color: u32,
            animation_id: i32,
        }
        #[repr(C)]
        struct WindowCompositionAttributeData {
            attribute: i32,
            data: *mut core::ffi::c_void,
            size: usize,
        }
        const WCA_ACCENT_POLICY: i32 = 19;
        const ACCENT_ENABLE_ACRYLICBLURBEHIND: i32 = 4;
        #[link(name = "user32")]
        unsafe extern "system" {
            fn SetWindowCompositionAttribute(
                hwnd: HWND,
                data: *mut WindowCompositionAttributeData,
            ) -> i32;
        }
        let mut policy = AccentPolicy {
            state: 0, // ACCENT_DISABLED：液态玻璃不要磨砂，只保留 DwmEnableBlurBehindWindow 的轻微模糊
            flags: 2,
            gradient_color: 0x66ff_ffff,
            animation_id: 0,
        };
        let mut data = WindowCompositionAttributeData {
            attribute: WCA_ACCENT_POLICY,
            data: &mut policy as *mut _ as *mut core::ffi::c_void,
            size: core::mem::size_of::<AccentPolicy>(),
        };
        unsafe {
            let _ = SetWindowCompositionAttribute(hwnd, &mut data);
        }
        window.dpi.set(GetDpiForWindow(hwnd).max(1));
        if preview {
            apply_taskbar_icon(hwnd, icon);
        }
        // Eager initialization means the factory can fail over before first show.
        window
            .app
            .borrow_mut()
            .graphics
            .ensure_target(hwnd, window.dpi.get())
            .map_err(|e| e.to_string())?;
    }
    window.health()?;
    Ok(Box::new(Ten { window }))
}

/// 设置预览窗口的小图标和大图标，使任务栏按钮显示 Weasel 图标。
///
/// 空图标句柄不做处理；窗口类图标仍作为任务栏图标的后备值。
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

impl Window {
    /// 记录首个故障，保留最初错误作为后续健康检查的结果。
    fn fail(&self, error: impl ToString) {
        let mut slot = self.error.borrow_mut();
        if slot.is_none() {
            *slot = Some(error.to_string());
        }
    }
    /// 请求 Windows 在之后的消息循环中重绘窗口。
    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd.get()), None, false);
        }
    }
    /// 清除按下/悬停状态，并在本窗口持有鼠标捕获时释放捕获。
    fn cancel(&self) {
        self.app.borrow_mut().gesture.cancel();
        unsafe {
            if GetCapture() == self.hwnd.get() {
                let _ = ReleaseCapture();
            }
        }
    }
    /// 按当前内容、锚点和 DPI 定位顶置窗口。
    ///
    /// `SetWindowPos` 可能同步触发 DPI 消息；重入时直接返回，外层最多重算三次以
    /// 收敛显示器切换导致的 DPI 变化。无可定位内容时不移动窗口。
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
        // Moving between monitors may synchronously change DPI. Recompute once
        // that call returns, with a finite bound for pathological monitor layouts.
        for _ in 0..3 {
            let dpi = self.dpi.get();
            let bounds = {
                let app = self.app.borrow();
                if self.preview {
                    // The preview has no caret; center the strip on the primary
                    // monitor instead of anchoring to a rect.
                    app.content.as_ref().map(|c| {
                        let width = pixels(c.layout.width, dpi);
                        let height = pixels(c.layout.height + logic::BOTTOM_PAD, dpi);   // 窗口加高：底部留白 ✓
                        let (x, y) = crate::presentation::preview_position(width, height);
                        (x, y, width, height)
                    })
                } else {
                    app.content.as_ref().and_then(|c| {
                        c.snapshot
                            .anchor
                            .as_ref()
                            .filter(|a| a.valid)
                            .map(|anchor| {
                                let width = pixels(c.layout.width, dpi);
                                let height = pixels(c.layout.height + logic::BOTTOM_PAD, dpi);   // 窗口加高：底部留白 ✓
                                let (x, y) =
                                    crate::presentation::popup_position(anchor, width, height);
                                (x, y, width, height)
                            })
                    })
                }
            };
            let Some((x, y, width, height)) = bounds else {
                return Ok(());
            };
            unsafe {
                SetWindowPos(
                    self.hwnd.get(),
                    Some(HWND_TOPMOST),
                    x,
                    y,
                    width,
                    height,
                    SWP_NOACTIVATE as u32,
                )
                .ok()?;
            }
            if dpi == self.dpi.get() {
                break;
            }
        }
        Ok(())
    }
    /// 将 Direct2D 目标的 DPI 和像素尺寸同步到当前客户区。
    fn resize(&self) -> windows_core::Result<()> {
        let app = self.app.borrow();
        if let Some(target) = &app.graphics.target {
            unsafe {
                let mut rc = RECT::default();
                GetClientRect(self.hwnd.get(), &mut rc).ok()?;
                target.SetDpi(self.dpi.get() as f32, self.dpi.get() as f32);
                target
                    .Resize(&D2D_SIZE_U {
                        width: (rc.right - rc.left).max(1) as u32,
                        height: (rc.bottom - rc.top).max(1) as u32,
                    })
                    .ok()?;
            }
        }
        Ok(())
    }
    /// 丢弃失效的绘制目标，并对设备丢失进行有限次数的定时重试。
    ///
    /// 非设备丢失错误、超过重试预算或定时器创建失败都会成为持久健康错误。
    fn graphics_error(&self, error: windows_core::Error) {
        self.app.borrow_mut().graphics.target = None;
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
    /// 绘制当前内容；仅在绘制成功后重置设备恢复预算。
    ///
    /// 绘制开始前创建可能失败的画刷，`DrawGuard` 保证所有已开始的绘制最终调用
    /// `EndDraw`。设备目标创建或绘制失败均由消息处理层交给恢复逻辑。
    fn paint(&self) -> windows_core::Result<()> {
        let mut app = self.app.borrow_mut();
        app.graphics
            .ensure_target(self.hwnd.get(), self.dpi.get())?;
        let target = app
            .graphics
            .target
            .as_ref()
            .ok_or_else(|| windows_core::Error::from_hresult(E_UNEXPECTED))?;
        let p = app.palette;
        unsafe {
            // Allocate fallible resources before BeginDraw; EndDraw is RAII too.
            let brush = target.CreateSolidColorBrush(&color(p.text), None)?;
            target.BeginDraw();
            let draw = DrawGuard {
                target,
                ended: false,
            };
            // 玻璃：把背景清成完全透明（A=0）。窗口上已开 DwmEnableBlurBehindWindow，
            // 配合 PREMULTIPLIED 像素格式，DWM 才能把背后的模糊合成进来。
            // 想让文字在任何背景上都看得清时，把 a 调到 0.4~0.5 即可（半透明底色）。
            // 圆角玻璃：清屏保持**全透明** → 圆角之外的像素没人画，自然透出背后内容。
            let mut glass = color(p.background);
            glass.a = 0.0;
            target.Clear(Some(&glass));
            if let Some(c) = &app.content {
                let width = c.layout.width;
                let height = c.layout.height + logic::BOTTOM_PAD;   // 面板加高：底部留白 ✓
                    // 圆角裁剪 + 分层透明度：每次渲染都按当前尺寸重裁（圆角不能只在第一帧对 ✗）
                    unsafe { apply_round_region(self.hwnd.get(), width, height); }
                if width > 0.0 && height > 0.0 {
                    // 圆角半径 = 高度的四分之一（用户指定），并保证不超出半宽。
                    let radius = (height / 6.0).min(width / 2.0).max(4.0);   // 对齐截图：小圆角 ✓
                    // 圆角要"切干净"：显式用逐图元抗锯齿，边缘平滑无锯齿。
                    target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                    // 面板本体：圆角矩形 + 40% 淡罩。
                    // 原来 15% 在深色背景上几乎看不出轮廓 → 圆角"像没了" ✗；
                    // 透明度低一点（40%）才既有玻璃感、又能看清圆角形状。
                    // ★ 先铺满整窗黑底：面板绘制尺寸是按 DIP 算的，而窗口是**像素** ✗
                    // 高分屏或舍入下会留下 1~2px 未绘制像素 → 在整窗统一透明下显示为**黑斑** ✗
                    // 先铺一层满窗黑（圆角之外由 SetWindowRgn 裁掉 ✓），再画圆角面板 ✓
                    let mut base = color(p.background);
                    base.a = 1.0;
                    brush.SetColor(&base);
                    target.FillRectangle(&rect(0.0, 0.0, width + 4.0, height + 4.0), &brush);
let mut fill = color(p.background);
                    fill.a = 1.0;   // 面板用真彩色 ✓ 透明由整窗 LWA_ALPHA 提供（否则 DWM 忽略逐像素 alpha → 又暗又平 ✗）
                    brush.SetColor(&fill);
                    target.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: rect(0.0, 0.0, width, height),
                            radiusX: radius,
                            radiusY: radius,
                        },
                        &brush,
                    );
                    // 圆角描边：加厚到 2px、更亮 —— 轮廓清楚（内缩半个描边宽，避免被裁）。
                    let mut outline = color(p.border);   // 黑底外描边 ✓（下面把 alpha 调淡，避免顶部像一条黑边 ✗）
                    outline.a = 0.60;   // Aero 外框：蓝灰、清晰但不硬 ✓
                    brush.SetColor(&outline);
                    target.DrawRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: rect(1.0, 1.0, width - 1.0, height - 1.0),
                            radiusX: radius,
                            radiusY: radius,
                        },
                        &brush,
                        2.0,
                        None,
                    );
                    // 镜面高光：顶边 2px 亮线（沿直线段内缩 radius，避免戳出圆角）
                    let mut highlight = color(0xffffff);
                    highlight.a = 0.65;
                    brush.SetColor(&highlight);
                    // ★ Aero 招牌：顶部**高光带**（不是一条线 ✗）—— 24 层渐隐细线模拟线性渐变，
// 顶部 alpha 0.72 二次衰减到 0，覆盖上半 48% 高度（无需渐变画刷 API ✓ 低风险 ✓）
                    let band_start = 0.0;   // ★ 去掉顶部暗边：高光从最顶端开始 ✓
                    let band_end = height * 0.60;   // 高光带再宽一点 ✓
                    let steps = 24u32;
                    let band_h = (band_end - band_start) / steps as f32;
                    for i in 0..steps {
                        let f = i as f32 / steps as f32;
                        let mut hl = color(0xffffff);
                        hl.a = 0.55 * (1.0 - f) * (1.0 - f);   // ★ Aero 招牌：强顶光（玻璃上半那道白光 ✓）
                        brush.SetColor(&hl);
                        let y0 = band_start + i as f32 * band_h;
                        target.FillRectangle(&rect(0.0, y0, width, y0 + band_h), &brush);   // ★ 全宽高光带（溢出圆角的部分由 SetWindowRgn 裁掉 ✓）
                    }
                    let mut hl_line = color(0xffffff);
                    hl_line.a = 0.75;   // Aero 顶端亮线（很亮 ✓）
                    brush.SetColor(&hl_line);
                    target.FillRectangle(&rect(0.0, band_start, width, band_start + 1.0), &brush);   // 亮线跟着高光带顶边 ✓
                    // 左右淡边（同样内缩 radius，止于上下圆角之间）
                    highlight.a = 0.20;   // 黑底上侧边也要收细 ✓
                    brush.SetColor(&highlight);
                    // ★ 左右两侧对称的高光蓝边（Aero 的玻璃侧壁反光 ✓）
                    let mut edge_blue = color(0xFFFFFF);   // 黑底高亮：侧边用白色细高光 ✓
                    edge_blue.a = 0.22;
                    brush.SetColor(&edge_blue);
                    target.FillRectangle(&rect(0.0, radius, 2.0, height - radius), &brush);
                    target.FillRectangle(&rect(width - 2.0, radius, width, height - radius), &brush);
                    brush.SetColor(&highlight);
                    target.FillRectangle(
                        &rect(width - 2.0, radius, width, height - radius),
                        &brush,
                    );
                    // 底部暗边（厚度感）
                    let mut shade = color(0x000000);
                    shade.a = 0.30;
                    brush.SetColor(&shade);
                    target.FillRectangle(&rect(radius, height - 1.0, width - radius, height), &brush);
                    brush.SetColor(&color(p.text));
                }
            }
            if let Some(c) = &app.content {
                if let Some(indicator) = &c.snapshot.mode_indicator {
                    text(
                        target,
                        &brush,
                        &app.graphics.mode,
                        if indicator.ascii_mode { "英" } else { "中" },
                        rect(0.0, 0.0, c.layout.width, c.layout.height),
                        p.text,
                    );
                }

                // ★★ 三层结构（用户要求 ✓）: ① 底层(满窗底+圆角面板, 已画) / ② 左侧候选区 / ③ 右侧功能区
                let fn_left = c.layout.cells.iter()
                    .find(|cell| matches!(cell.hit, Hit::Previous))
                    .map(|cell| cell.left)
                    .unwrap_or(c.layout.width);
                if fn_left < c.layout.width {
                    let content_h = c.layout.height;
                    let mut zone = color(0x000000);   // 功能区底色 ✓
                    zone.a = 0.10;   // 功能区比面板略深 → 读作"独立一栏" ✓
                    brush.SetColor(&zone);
                    target.FillRectangle(&rect(fn_left, 0.0, c.layout.width, content_h), &brush);
                    let mut edge = color(0xFFFFFF);
                    edge.a = 0.22;   // 功能区左沿 1px 分隔线 ✓
                    brush.SetColor(&edge);
                    target.FillRectangle(&rect(fn_left, 0.0, fn_left + 1.0, content_h), &brush);
                    brush.SetColor(&color(p.text));
                }

                for cell in &c.layout.cells {
                    let selected = matches!(cell.hit, Hit::Candidate(i) if i == c.snapshot.selected_index as usize);
                    let usable = enabled(&c.snapshot, cell.hit);
                    let rect = rect(cell.left, 0.0, cell.right, c.layout.height);
                    if selected || (usable && app.gesture.hovered == Some(cell.hit)) {
                        // 用户要求：选中/悬停一律用灰色，不要蓝色。
                        // 取文字色（白）降透明度当灰底 —— 透底之上就是一层中性灰。
                        let mut gray = color(p.active);   // ★ 选中与悬停**同色**（用户要求 ✓）
                        gray.a = 1.0;   // ★ 高亮：选中/悬停块完全不透明，在黑底上最醒目 ✓
                        brush.SetColor(&gray);
                        target.FillRectangle(&rect, &brush);
                    }
                    // ★ 细分隔线（对齐截图：候选区 / 翻页箭头 / 表情之间各一条 ✓）
                    if matches!(cell.hit, Hit::Previous | Hit::Emoji) {
                        let mut divider = color(0xFFFFFF);
                        divider.a = 0.10;
                        brush.SetColor(&divider);
                        target.FillRectangle(&rect_with(rect, cell.left, cell.left + 1.0), &brush);   // 用 rect_with ✓ 此处 rect 是局部变量 ✓
                        brush.SetColor(&color(p.text));
                    }                    match cell.hit {
                        Hit::Candidate(i) => {
                            let item = &c.snapshot.items[i];
                            text(
                                target,
                                &brush,
                                &app.graphics.number,
                                &(i + 1).to_string(),
                                rect_with(rect, cell.left, cell.left + NUMBER - 8.0 * SCALE),
                                if !usable {
                                    p.disabled
                                } else if selected {
                                    p.active_number
                                } else {
                                    p.secondary
                                },
                            );
                            text(
                                target,
                                &brush,
                                &app.graphics.text,
                                &item.primary_text,
                                rect_with(rect, cell.left + NUMBER, cell.right - logic::PAD),
                                0xB0B0B0,   // 箭头/表情：暗灰 ✓（text() 要 u32 色值 ✓）
                            );
                            if !item.secondary_text.is_empty() {
                                text(
                                    target,
                                    &brush,
                                    &app.graphics.comment,
                                    &item.secondary_text,
                                    rect_with(
                                        rect,
                                        cell.left + NUMBER + c.primary_widths[i] + 8.0 * SCALE,
                                        cell.right - logic::PAD,
                                    ),
                                    if usable { p.secondary } else { p.disabled },
                                );
                            }
                        }
                        hit => {
                            let glyph = match hit {
                                Hit::Previous => "\u{2039}",   // ‹ 用普通文字字体画 ✓ 图标字体可能缺字形 ✗
                                Hit::Next => "\u{203A}",   // › 同上 ✓
                                _ => "\u{263A}",   // ☺ 用普通字体可显示的字形 ✓（原来 U+E76E 图标字体缺字形 → 空白 ✗）
                            };
                            let mut icon_rect = rect;
                            icon_rect.top -= 2.0 * SCALE;
                            icon_rect.bottom -= 2.0 * SCALE;
                            text(
                                target,
                                &brush,
                                &app.graphics.number,   // 箭头与表情改用正文/数字字体 ✓ 保证有字形 ✓
                                glyph,
                                icon_rect,
                                0xB0B0B0,   // 箭头/表情：暗灰 ✓（text() 要 u32 色值 ✓）
                            );
                        }
                    }
                }
                brush.SetColor(&color(p.border));
                for cell in &c.layout.cells {
                    if matches!(cell.hit, Hit::Previous | Hit::Emoji) {
                        target.FillRectangle(
                            &rect(cell.left, 0.0, cell.left + SCALE, c.layout.height),
                            &brush,
                        );
                    }
                }
                for edge in [
                    rect(0.0, 0.0, c.layout.width, SCALE),
                    rect(
                        0.0,
                        c.layout.height - SCALE,
                        c.layout.width,
                        c.layout.height,
                    ),
                    rect(0.0, 0.0, SCALE, c.layout.height),
                    rect(c.layout.width - SCALE, 0.0, c.layout.width, c.layout.height),
                ] {
                    target.FillRectangle(&edge, &brush);
                }
            }
            draw.finish()?;
        }
        self.recovery.borrow_mut().succeeded();
        Ok(())
    }
    /// 将鼠标消息中的有符号像素坐标换算为 DIP，并命中当前布局。
    fn hit(&self, param: LPARAM) -> Option<Hit> {
        let x = param.0 as u16 as i16 as f32 * 96.0 / self.dpi.get() as f32;
        let y = (param.0 >> 16) as u16 as i16 as f32 * 96.0 / self.dpi.get() as f32;
        self.app
            .borrow()
            .content
            .as_ref()
            .and_then(|c| c.layout.hit(x, y))
    }
}

impl ThemeBackend for Ten {
    /// 更新候选栏；相同内容的锚点/布局更新只重新定位窗口。
    fn render(&mut self, snapshot: &CandidateView, events: &EventSink) -> Result<(), String> {
        self.window.render(snapshot, events)
    }
    /// 隐藏窗口并清除内容、输入捕获和待重试状态。
    fn hide(&mut self) {
        self.window.hide();
    }
    /// 重新读取系统明暗外观并请求重绘。
    fn refresh_appearance(&mut self) -> Result<(), String> {
        self.window.app.borrow_mut().palette = logic::palette_from_official(crate::appearance::is_dark());
        self.window.invalidate();
        self.window.health()
    }
    /// 返回已记录的首个窗口或图形故障。
    fn check_health(&mut self) -> Result<(), String> {
        self.window.health()
    }
}

impl Window {
    /// 应用候选快照并显示窗口。
    ///
    /// 可见且内容未变时仅更新快照/事件发送端并重新定位，保留当前手势和已测量布局；
    /// 内容变化时取消手势、重测文本并重建布局。隐藏或锚点无效的快照会隐藏窗口。
    /// 原生定位错误直接返回；首次显示前的绘制错误进入图形恢复流程。
    fn render(&self, snapshot: &CandidateView, events: &EventSink) -> Result<(), String> {
        // Layout-only snapshots must not cancel a pressed candidate, rebuild
        // text layouts, or repaint content. position() handles DPI transitions.
        let moved = {
            let mut app = self.app.borrow_mut();
            if let Some(content) = app.content.as_mut()
                && (crate::presentation::is_visible(snapshot)
                    || crate::presentation::is_mode_indicator_visible(snapshot))
                && crate::theme_api::same_content(&content.snapshot, snapshot)
            {
                content.snapshot = snapshot.clone();
                content.events = events.clone();
                true
            } else {
                false
            }
        };
        if moved {
            self.position().map_err(|e| e.to_string())?;
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
        let was_hidden = self.app.borrow().content.is_none();
        {
            let mut app = self.app.borrow_mut();
            let mut primary_widths = Vec::with_capacity(snapshot.items.len());
            let mut widths = Vec::with_capacity(snapshot.items.len());
            for item in &snapshot.items {
                let primary = app
                    .graphics
                    .measure(&item.primary_text, &app.graphics.text)
                    .map_err(|e| e.to_string())?;
                let secondary = if item.secondary_text.is_empty() {
                    0.0
                } else {
                    8.0 * SCALE
                        + app
                            .graphics
                            .measure(&item.secondary_text, &app.graphics.comment)
                            .map_err(|e| e.to_string())?
                };
                primary_widths.push(primary);
                widths.push(primary + secondary);
            }
            let layout = if snapshot.mode_indicator.is_some() {
                Layout::mode_indicator()
            } else {
                Layout::new(widths)
            };
            app.content = Some(Content {
                snapshot: snapshot.clone(),
                events: events.clone(),
                layout,
                primary_widths,
            });
        }
        self.position().map_err(|e| e.to_string())?;
        if was_hidden {
            // The HWND render target retains its previous frame while hidden.
            // Present this composition before making the window visible.
            if let Err(error) = self.paint() {
                self.graphics_error(error);
                // Keep the existing recovery path; device loss may require a
                // fresh target and the timer-driven repaint below.
            }
        }
        unsafe {
            let _ = ShowWindow(
                self.hwnd.get(),
                if self.preview {
                    SW_SHOW
                } else {
                    SW_SHOWNOACTIVATE
                },
            );
        }
        self.invalidate();
        self.health()
    }
    /// 隐藏 HWND，并释放当前交互及定时恢复状态。
    fn hide(&self) {
        self.cancel();
        self.app.borrow_mut().content = None;
        self.recovery.borrow_mut().waiting = false;
        unsafe {
            let _ = KillTimer(Some(self.hwnd.get()), RETRY_TIMER);
            let _ = ShowWindow(self.hwnd.get(), SW_HIDE);
        }
    }
    /// 返回窗口回调或原生操作记录的健康错误（若有）。
    fn health(&self) -> Result<(), String> {
        self.error
            .borrow()
            .as_ref()
            .map_or(Ok(()), |e| Err(e.clone()))
    }
}

impl Drop for Window {
    /// 先清除回调指针，再释放捕获、定时器和 HWND，避免回调访问正在析构的状态。
    fn drop(&mut self) {
        let hwnd = self.hwnd.get();
        if !hwnd.0.is_null() {
            unsafe {
                // Detach before destruction: no callback can access a dropping App.
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                self.cancel();
                let _ = KillTimer(Some(hwnd), RETRY_TIMER);
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}

struct PaintGuard {
    hwnd: HWND,
    ps: PAINTSTRUCT,
}
impl PaintGuard {
    /// 开始一次 WM_PAINT 区域处理；析构时与之配对调用 `EndPaint`。
    unsafe fn begin(hwnd: HWND) -> Self {
        let mut ps = PAINTSTRUCT::default();
        unsafe {
            let _ = BeginPaint(hwnd, &mut ps);
        }
        Self { hwnd, ps }
    }
}
impl Drop for PaintGuard {
    /// 即使绘制提前返回，也结束 Windows 的绘制事务。
    fn drop(&mut self) {
        unsafe {
            let _ = EndPaint(self.hwnd, &self.ps);
        }
    }
}
struct DrawGuard<'a> {
    target: &'a ID2D1HwndRenderTarget,
    ended: bool,
}
impl DrawGuard<'_> {
    /// 显式结束 Direct2D 绘制，并将设备错误交给调用方。
    fn finish(mut self) -> windows_core::Result<()> {
        self.ended = true;
        unsafe { self.target.EndDraw(None, None).ok() }
    }
}
impl Drop for DrawGuard<'_> {
    /// 尚未显式结束时尽力配对 `EndDraw`，避免遗留未完成的绘制事务。
    fn drop(&mut self) {
        if !self.ended {
            unsafe {
                let _ = self.target.EndDraw(None, None);
            }
        }
    }
}

/// Windows 窗口过程的 ABI 边界；捕获 Rust panic，避免展开穿过系统回调。
/// 把窗口裁剪成**圆角矩形**，并开启分层窗口的统一透明度。
///
/// 为什么必须这么做：当前用的是 `ID2D1HwndRenderTarget`，它**不会**把逐像素 alpha 交给 DWM ✗，
/// 于是"清屏 alpha=0"的圆角外区域会以 RGB(0,0,0) **不透明**地显示出来 —— 就是用户看到的
/// "圆角边上多一层黑角" ✓。用 `SetWindowRgn` 在系统层面裁掉圆角之外，圆角就一定是干净的 ✓。
/// 真正的逐像素玻璃（面板半透明）需要 `ID2D1DCRenderTarget` + 32 位 DIB + `UpdateLayeredWindow` ✓（下一步）。
/// 分层窗口扩展样式（0x00080000）。d2d 绑定集里没有导出它，这里本地定义 ✓
/// 只有设了它，`SetLayeredWindowAttributes` 才生效（整窗统一透明度 ✓）。
const WS_EX_LAYERED: i32 = 0x0008_0000;
unsafe fn apply_round_region(hwnd: HWND, width: f32, height: f32) {
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateRoundRectRgn(x1: i32, y1: i32, x2: i32, y2: i32, w: i32, h: i32) -> isize;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowRgn(hwnd: HWND, rgn: isize, redraw: i32) -> i32;
        fn SetLayeredWindowAttributes(hwnd: HWND, key: u32, alpha: u8, flags: u32) -> i32;
    }
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    // ★ 单位换算（今晚踩的坑 ✗）：调用方给的是 **DIP**，而 SetWindowRgn 要的是**设备像素** ✗。
    // 不换算时 DPI>96（125%/150% 缩放）区域会偏小 → 面板**右侧与底部被切掉** ✗（用户报的"右侧不全" ✓）。
    let dpi = GetDpiForWindow(hwnd).max(96) as f32;
    let scale = dpi / 96.0;
    let radius = (height / 6.0).max(4.0) * scale;   // 半径同样换算成像素 ✓
    let d = (radius * 2.0).round() as i32;
    let rgn = CreateRoundRectRgn(
        0,
        0,
        (width * scale).round() as i32 + 1,
        (height * scale).round() as i32 + 1,
        d,
        d,
    );
    if rgn != 0 {
        // SetWindowRgn 接管 region 所有权，不需要 DeleteObject ✓
        SetWindowRgn(hwnd, rgn, 1);
    // ★ 磨砂玻璃：给 DWM 模糊也传**圆角区域**（传 NULL 会按矩形铺满 → 圆角外露出方形磨砂层 ✗）
    // 注意 SetWindowRgn 会接管 rgn 的所有权 ✗ 所以这里必须再建一个区域 ✓
    let blur_rgn = CreateRoundRectRgn(
        0,
        0,
        (width * scale).round() as i32 + 1,
        (height * scale).round() as i32 + 1,
        d,
        d,
    );
    if blur_rgn != 0 {
        #[link(name = "dwmapi")]
        unsafe extern "system" {
            fn DwmEnableBlurBehindWindow(hwnd: HWND, bb: *const BlurBehind) -> i32;
        }
        #[repr(C)]
        struct BlurBehind {
            flags: u32,
            enable: i32,
            blur_region: isize,
            transition_on_maximized: i32,
        }
        const DWM_BB_ENABLE: u32 = 0x1;
        const DWM_BB_BLURREGION: u32 = 0x2;
        let bb = BlurBehind {
            flags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
            enable: 1,
            blur_region: blur_rgn,
            transition_on_maximized: 0,
        };
        DwmEnableBlurBehindWindow(hwnd, &bb);
    }
    }
    // 分层窗口：整个（已裁圆的）窗口带统一透明度 ✓（逐像素 alpha 要等 DIB + UpdateLayeredWindow ✓）
    const LWA_ALPHA: u32 = 0x2;
    SetLayeredWindowAttributes(hwnd, 0, 200, LWA_ALPHA);   // ★ Aero 玻璃 200 ≈ 78%：让 DWM 磨砂真正透出来 ✓
}
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Catch every Rust callback, including paint and event delivery, before ABI return.
    match catch_unwind(AssertUnwindSafe(|| unsafe { dispatch(hwnd, msg, wp, lp) })) {
        Ok(result) => result,
        Err(_) => {
            unsafe {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Window;
                if let Some(window) = ptr.as_ref() {
                    if let Ok(mut error) = window.error.try_borrow_mut() {
                        *error = Some("theme_ten native callback panicked".into());
                    }
                }
            }
            LRESULT(0)
        }
    }
}

/// 分发窗口消息并驱动绘制、DPI、恢复和鼠标手势状态机。
///
/// `WM_NCCREATE` 将创建参数中的稳定 `Window` 指针绑定到 HWND；后续消息仅在该指针
/// 存在时访问状态。调用者必须保证消息参数符合对应 Win32 消息的约定。
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
                    // Closing the preview window ends the process. The live tool
                    // window is destroyed by the shutdown path and never posts
                    // WM_CLOSE.
                    if window.preview {
                        PostQuitMessage(0);
                        // Leave HWND destruction to the owning backend's Drop.
                        return LRESULT(0);
                    }
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                WM_MOUSEACTIVATE => {
                    // The preview window must be activatable so it can be closed
                    // (Alt+F4); the live strip never activates the host.
                    if window.preview {
                        return DefWindowProcW(hwnd, msg, wp, lp);
                    }
                    return LRESULT(MA_NOACTIVATE as isize);
                }
                WM_SIZE => {
                    if let Err(e) = window.resize() {
                        window.graphics_error(e);
                    }
                    return LRESULT(0);
                }
                WM_DPICHANGED => {
                    window.dpi.set((wp.0 as u32 & 0xffff).max(1));
                    window.cancel();
                    if let Err(e) = window.position() {
                        window.fail(e);
                    }
                    if let Err(e) = window.resize() {
                        window.graphics_error(e);
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
                WM_LBUTTONDOWN => {
                    let hit = window.hit(lp);
                    let pressed = {
                        let mut app = window.app.borrow_mut();
                        let App {
                            content, gesture, ..
                        } = &mut *app;
                        if let Some(c) = content {
                            gesture.press(hit, &c.snapshot);
                        }
                        gesture.pressed.is_some()
                    };
                    if pressed {
                        let _ = SetCapture(hwnd);
                    }
                    return LRESULT(0);
                }
                WM_LBUTTONUP => {
                    let hit = window.hit(lp);
                    let event = {
                        let mut app = window.app.borrow_mut();
                        let App {
                            content, gesture, ..
                        } = &mut *app;
                        content.as_ref().and_then(|c| {
                            gesture
                                .release(hit, &c.snapshot)
                                .map(|e| (c.events.clone(), e))
                        })
                    };
                    window.cancel();
                    if let Some((sender, event)) = event {
                        sender.send(event);
                    }
                    window.invalidate();
                    return LRESULT(0);
                }
                WM_MOUSEMOVE => {
                    let hit = window.hit(lp);
                    window.app.borrow_mut().gesture.motion(hit);
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE as u32,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut track);
                    window.invalidate();
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

/// 将 `0xRRGGBB` 转为不透明的 Direct2D 浮点颜色。
fn color(rgb: u32) -> D2D_COLOR_F {
    D2D_COLOR_F {
        r: ((rgb >> 16) & 255) as f32 / 255.0,
        g: ((rgb >> 8) & 255) as f32 / 255.0,
        b: (rgb & 255) as f32 / 255.0,
        a: 1.0,
    }
}
/// 按 DIP 坐标构造 Direct2D 矩形。
fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left,
        top,
        right,
        bottom,
    }
}
/// 保留矩形的垂直范围，仅替换水平边界。
fn rect_with(mut rect: D2D_RECT_F, left: f32, right: f32) -> D2D_RECT_F {
    rect.left = left;
    rect.right = right;
    rect
}
/// 设置画刷颜色并在给定裁剪矩形内绘制单行 UTF-16 文本。
unsafe fn text(
    target: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    format: &IDWriteTextFormat,
    value: &str,
    rect: D2D_RECT_F,
    rgb: u32,
) {
    let value: Vec<u16> = value.encode_utf16().collect();
    unsafe {
        brush.SetColor(&color(rgb));
        target.DrawText(
            &value,
            format,
            &rect,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
}

/// ten 候选栏主题的工厂入口。
pub struct Factory;

impl crate::theme_api::ThemeFactory for Factory {
    /// 返回用于主题选择和诊断的稳定名称。
    fn name(&self) -> &'static str {
        "official"
    }
    /// 声明此主题只提供候选栏界面。
    fn capabilities(&self) -> crate::theme_api::ThemeCapabilities {
        crate::theme_api::ThemeCapabilities {
            mode_indicator: true,
            ..crate::theme_api::ThemeCapabilities::CANDIDATES_ONLY
        }
    }
    /// 校验 ten 专属配置并创建对应模式的窗口后端。
    ///
    /// 当前接受任意 JSON 对象作为主题配置；窗口或配置校验失败会转换为创建错误。
    fn create(
        &self,
        mode: UiMode,
        settings: &weasel_common::settings::ConfigSnapshot,
    ) -> crate::theme_api::ThemeCreation {
        (|| -> Result<Box<dyn ThemeBackend>, String> {
            // Theme-local validation; style fields will be defined by this theme.
            let _: serde_json::Map<String, serde_json::Value> = settings.theme_settings("official")?;
            create(mode)
        })()
        .into()
    }
}
