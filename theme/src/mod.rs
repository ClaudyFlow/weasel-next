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

pub mod accent;
pub mod backdrop;
pub mod config;
pub mod layout;
pub mod paint;

use crate::d2d_bindings::*;
use crate::theme::paint::{d2d_color, Formats, Painter};
use crate::theme_api::{CandidateView, EventSink, ThemeBackend, UiAction, UiMode};
use config::Metrics;
use layout::{
    prediction_tag_visible, Hit, Layout, Measured, PREDICTION_KIND, Parts, Rect, Size,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};
use weasel_style::ColorScheme;
use windows_strings::w;

/// `hide()` 诊断（2026-11-07 23:xx）：候选窗被**谁**成串地收起。
///
/// 用户报「输入时不要关掉候选窗」；live `theme.log` 里出现过连续 154 行
/// `[panel] 候选窗收起 → 面板保留（常驻）`——说明有东西在成串地喂「不可见快照」。
/// 这里给每条收起路径打来源标记，并做 1 秒限频（被压掉的次数并入下一秒首条），
/// 免得诊断日志自己变成风暴。
static HIDE_LOG_ANCHOR: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
static HIDE_LOG_AT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HIDE_LOG_SINCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static HIDE_TOTAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const CLASS: windows_strings::PCWSTR = w!("Weasel.NextTheme.Default");
const RETRY_TIMER: usize = 1;

/// 抓屏亲和性：`WDA_NONE` = 正常（会被截进画面里）；
/// `WDA_EXCLUDEFROMCAPTURE` = 本窗口对**所有截屏 API 隐形**，截到的是它背后的画面。
const WDA_NONE: u32 = 0x0000_0000;
const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;

#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowDisplayAffinity(hwnd: HWND, affinity: u32) -> windows_core::BOOL;
}

/// `DWM_TIMING_INFO` 里的分数（`dwmapi.h` 的 `DWM_FRAME_RATE`）。
/// `#[repr(C, packed(1))]` 与 windows-rs 0.62 的元数据一致——这个结构体在 SDK
/// 头里被 `pshpack1` 包裹，字段之间没有填充。
#[repr(C, packed(1))]
#[derive(Clone, Copy, Default)]
struct UnsignedRatio {
    ui_numerator: u32,
    ui_denominator: u32,
}

/// `DWM_TIMING_INFO`：完整字段照 windows-rs 0.62 `Windows.Win32.Graphics.Dwm`
/// 移植。`DwmGetCompositionTimingInfo` 会校验 `cbSize == sizeof(DWM_TIMING_INFO)`，
/// 少一个字段 / 错一个类型都会让调用失败（E_INVALIDARG），所以这里不偷懒只声明
/// 需要的 `rateRefresh`，而是整份照抄，让 `size_of` 自动算出准确的字节数。
#[repr(C, packed(1))]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
struct DwmTimingInfo {
    cb_size: u32,
    rate_refresh: UnsignedRatio,
    qpc_refresh_period: u64,
    rate_compose: UnsignedRatio,
    qpc_v_blank: u64,
    c_refresh: u64,
    c_dx_refresh: u32,
    qpc_compose: u64,
    c_frame: u64,
    c_dx_present: u32,
    c_refresh_frame: u64,
    c_frame_submitted: u64,
    c_dx_present_submitted: u32,
    c_frame_confirmed: u64,
    c_dx_present_confirmed: u32,
    c_refresh_confirmed: u64,
    c_dx_refresh_confirmed: u32,
    c_frames_late: u64,
    c_frames_outstanding: u32,
    c_frame_displayed: u64,
    qpc_frame_displayed: u64,
    c_refresh_frame_displayed: u64,
    c_frame_complete: u64,
    qpc_frame_complete: u64,
    c_frame_pending: u64,
    qpc_frame_pending: u64,
    c_frames_displayed: u64,
    c_frames_complete: u64,
    c_frames_pending: u64,
    c_frames_available: u64,
    c_frames_dropped: u64,
    c_frames_missed: u64,
    c_refresh_next_displayed: u64,
    c_refresh_next_presented: u64,
    c_refreshes_displayed: u64,
    c_refreshes_presented: u64,
    c_refresh_started: u64,
    c_pixels_received: u64,
    c_pixels_drawn: u64,
    c_buffers_empty: u64,
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmFlush() -> i32;
    /// 拿 DWM 的合成刷新率（不是 `GetDeviceCaps` 的显示模式刷新率）。
    /// 返回 HRESULT：0 = S_OK，负数为失败（远程桌面 / 无合成器时会失败，调用方回退 60Hz）。
    fn DwmGetCompositionTimingInfo(hwnd: HWND, info: *mut DwmTimingInfo) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateRoundRectRgn(
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        w_ellipse: i32,
        h_ellipse: i32,
    ) -> isize;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowRgn(hwnd: HWND, hrgn: isize, b_redraw: i32) -> i32;
}

/// 抓屏期间临时把窗口排除出画面，离开作用域立刻还原（含早退/panic 路径）。
struct CaptureAffinity {
    hwnd: HWND,
    active: bool,
}

impl Drop for CaptureAffinity {
    fn drop(&mut self) {
        if self.active {
            unsafe {
                // 还原不必等帧：晚一帧被用户截图截到"少个候选窗"无所谓，
                // 但多等一帧要付 16ms（每键都付不起）。
                let _ = SetWindowDisplayAffinity(self.hwnd, WDA_NONE);
            }
        }
    }
}

/// 官方 `MoveTo` 里给插入点下方留的 6 像素（WeaselPanel.cpp:196/213）。
const CARET_GAP: i32 = 6;

/// 官方 `WeaselPanel::AUTOREV_TIMER` / `AUTOHIDE_TIMER` 的作用：自动恢复与自动隐藏。
const AUTOREV_TIMER: usize = 2;
const AUTOHIDE_TIMER: usize = 3;
/// **延迟隐藏**（2026-11-08 修「虎整句打字一直闪烁」）：引擎在整句 Lua 重算期间
/// 会夹带「无候选、无预编辑」的中间快照，主题收到就立刻 `ShowWindow(SW_HIDE)`
/// + 清屏 + `DwmFlush` + Cloak，下一帧有内容又重开亚克力、重抓高斯模糊 ——
/// 每键一次 hide→show 风暴，肉眼就是持续闪烁。
///
/// 改成：收到隐藏请求只挂一个短计时器，期间只要来了可见快照就撤销；
/// 队列空闲（用户真的停手）`HIDE_DEBOUNCE_MS` 后才真正隐藏。
/// WM_TIMER 只在消息队列空时生成，恰好与「打字期间别隐藏」的语义吻合。
///
/// 2026-11-08 17:5x 实测：原值 120ms 仍会闪 —— 上屏/确认后到下一键的间隔
/// 常大于 120ms（用户报「一直很闪，不是布局的问题」），于是每次提交都
/// hide→show 一次。改成 450ms：正常连续打字（键间隔 <450ms）全程不隐藏；
/// 真停手 450ms 后收窗，只收一次，不再反复闪。
const HIDE_DEBOUNCE_TIMER: usize = 4;
// 2026-11-08 18:3x 用户报「打字后界面按逗号句号消失太慢、上屏后消失也慢」：
// 450ms 是早先治 hide/show 风暴的保守值，实测太"粘手"；配合新 TIP/引擎后
// 回落到 200ms（保留去抖作用，肉眼几乎跟手）。
//
// 2026-11-08 20:5x 用户报「虎码 lua 一直闪烁」：live `theme.log` 最后 3000 行里
// 有 300 次「[不可见帧] → 将隐藏」、263 次 `src=debounced_hide` 实际收窗；
// 同期按键间隔 P75=238ms、>200ms 占 29%，200ms 去抖在「上屏/确认→下一键」的
// 常见间隔里必然触发，hide→show 风暴复发。450ms 时只有约 10% 的间隔会被打穿，
// 是记录里验证过能压住虎整句闪烁的值。用户确认接受「收窗稍慢」换「不闪」，
// 故回到 450ms，不要再往小调。
const HIDE_DEBOUNCE_MS: u32 = 450;

// ---------------------------------------------------------------- 图形资源

/// 持有 DirectWrite 格式及按需创建的 HWND 绘制目标。
///
/// 注意：**Direct2D 工厂不在这里** —— 路径几何、画刷必须与呈现层出自同一个工厂
/// （否则 `EndDraw` 报 `D2DERR_WRONG_FACTORY`），所以几何用
/// [`crate::presenter::Presenter::d2d_factory`]。
/// DirectWrite 文本测量结果缓存的容量上限。
///
/// 候选窗文本种类有限但用户词无限，不能让它跟着进程无限长；满了整份丢掉即可
/// （下一帧重新量一遍，命中率会短暂回落，但不会错）。
const MEASURE_CACHE_MAX: usize = 2048;

/// 区分四套文本格式 + 预测标签小号格式。字号/字体/DPI 变化由 [`Graphics::refresh_formats`]
/// 统一清空缓存兜底，所以 key 不必把整个 `Metrics` 塞进来。
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum MeasureFormat {
    Label,
    Text,
    Comment,
    PredictionTag,
    Preedit,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct MeasureKey {
    format: MeasureFormat,
    text: String,
    /// `max_width` 的位模式：竖排文本为 0，横排为 `m.max_width`。
    max_width_bits: u32,
}

#[derive(Default)]
struct MeasureCache(RefCell<HashMap<MeasureKey, Size>>);

impl MeasureCache {
    fn get(&self, key: &MeasureKey) -> Option<Size> {
        self.0.borrow().get(key).copied()
    }

    fn put(&self, key: MeasureKey, size: Size) {
        let mut map = self.0.borrow_mut();
        if map.len() >= MEASURE_CACHE_MAX && !map.contains_key(&key) {
            map.clear();
        }
        map.insert(key, size);
    }

    fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.borrow().len()
    }
}

struct Graphics {
    /// 供文本测量和布局使用的共享 DirectWrite 工厂。
    write: IDWriteFactory,
    /// 四套文本格式（官方 `InitFontFormats`）。
    formats: Formats,
    /// 中英文模式提示用的紧凑格式（官方没有这一项，是我们自己的小窗）。
    indicator: IDWriteTextFormat,
    /// 与此 HWND 关联的呈现层：D3D11 + DXGI 组合交换链 + DirectComposition + D2D 设备上下文。
    target: Option<crate::presenter::Presenter>,
    /// 2026-11-10：每次 `measure()` 原本要做 **两次** `CreateTextLayout` +
    /// `GetMetrics` + `GetOverhangMetrics`，`measure_all` 一屏 5 候选就是 15 次调用 /
    /// 30 次建布局，且零缓存。序号、编码、常用注释每帧重复，命中率很高。
    measure_cache: MeasureCache,
}

/// 预测标签（预测项的 `secondary_text`）字号 = 候选字号的 1/2。
///
/// 2026-11-08 用户口径：「我说的是预测这个标签做成小一号，预测的候选字号不变」
/// —— 小一号的是「预测」这两个字的标签，**不是**预测候选的正文文字（正文仍用
/// `formats.text`，字号跟普通候选一致）。「小一号」= `font_point * 0.5`，此前已确认。
///
/// 2026-11-08 稍后用户「不要显示预测这个标签了」⇒ 见
/// [`layout::prediction_tag_visible`]，标签恒不绘制；本函数与半字号格式保留备用。
///
/// 抽成独立函数是为了让单测能钉住 1/2 这个口径（真实 DirectWrite 格式无法在
/// 单测里建）；`Graphics::make_formats` 用它建 `Formats::prediction_tag`。
fn prediction_tag_point(m: &Metrics) -> f32 {
    m.font_point * 0.5
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
            measure_cache: MeasureCache::default(),
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
            // 「预测」标签小一号（2026-11-08 用户：「预测这个标签做成小一号，预测的
            // 候选字号不变」）：只缩预测项的 secondary_text，正文文字仍用 `text`。
            // 1/2 的口径此前已确认（小号 = 候选字号的一半）；其余参数与 `comment`
            // 完全相同（face / wrapping / linespacing / baseline 都走 `make_format`）。
            prediction_tag: Self::make_format(
                write,
                &m.comment_font_face,
                prediction_tag_point(m),
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
        // 字体/DPI 换了，之前量出来的尺寸一律作废。
        self.measure_cache.clear();
        Ok(())
    }

    /// 官方 `D2D::GetTextSize`：量宽度含尾随空白、向上取整，并把 overhang 的正向溢出补进尺寸。
    ///
    /// 2026-11-10 加测量结果 memo：`format` 用 [`MeasureFormat`] 标身份（COM 对象地址
    /// 没法当稳定 key），`max_width` 按位模式入 key；字体/DPI 变化由 `refresh_formats`
    /// 清空。测量失败不缓存，免得把临时的 DWrite 故障固化成永久零尺寸。
    fn measure(
        &self,
        text: &str,
        tag: MeasureFormat,
        format: &IDWriteTextFormat,
        m: &Metrics,
    ) -> Size {
        if text.is_empty() {
            return Size::ZERO;
        }
        let max_width = if m.layout_type.is_vertical_text() {
            0.0
        } else {
            m.max_width
        };
        let key = MeasureKey {
            format: tag,
            text: text.to_owned(),
            max_width_bits: max_width.to_bits(),
        };
        if let Some(size) = self.measure_cache.get(&key) {
            return size;
        }
        unsafe {
            let wide: Vec<u16> = text.encode_utf16().collect();
            let Ok(layout) = self.write.CreateTextLayout(&wide, format, max_width, 0.0) else {
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
            self.measure_cache.put(key, size);
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
            let label = m.format_label(&(i as u32 + 1).to_string());
            measured.label.push(if m.label_font_valid {
                self.measure(&label, MeasureFormat::Label, &self.formats.label, m)
            } else {
                Size::ZERO
            });
            measured.text.push(if m.text_font_valid {
                self.measure(
                    &item.primary_text,
                    MeasureFormat::Text,
                    &self.formats.text,
                    m,
                )
            } else {
                Size::ZERO
            });
            measured.comment.push(
                if item.kind == PREDICTION_KIND && !prediction_tag_visible() {
                    // 2026-11-08「不要显示预测这个标签了」：不量宽 ⇒ layout 不给注释
                    // 留位置、paint 两侧同口径跳过落笔，候选框不会留空注释占位。
                    // 引擎侧 `renderer_bridge` 也已不再下发该注释（两道独立防线）。
                    Size::ZERO
                } else if m.comment_font_valid {
                    // 标签若将来重新打开：kind == prediction 的 secondary_text 用
                    // font_point/2 的小号格式量；普通候选的注释仍用 `comment`。
                    let (format, tag) = if item.kind == PREDICTION_KIND {
                        (&self.formats.prediction_tag, MeasureFormat::PredictionTag)
                    } else {
                        (&self.formats.comment, MeasureFormat::Comment)
                    };
                    self.measure(&item.secondary_text, tag, format, m)
                } else {
                    Size::ZERO
                },
            );
        }
        // 预编辑：我们的快照只给"整段组字中"，所以整段都是高亮段（官方 composition 的常态）
        if let Some(preedit) = &view.preedit {
            let size = self.measure(
                &preedit.text,
                MeasureFormat::Preedit,
                &self.formats.preedit,
                m,
            );
            measured.preedit = Parts {
                before: Size::ZERO,
                hilite: size,
                after: Size::ZERO,
                total: size,
            };
        }
        // 翻页字形（官方用 `<` / `>`，字体同预编辑）
        measured.page_prev = self.measure(
            layout::PAGE_PREV,
            MeasureFormat::Preedit,
            &self.formats.preedit,
            m,
        );
        measured.page_next = self.measure(
            layout::PAGE_NEXT,
            MeasureFormat::Preedit,
            &self.formats.preedit,
            m,
        );
        // 标记：官方在没有 mark_text 时用 "|" 的尺寸做基准
        measured.mark = if m.mark_text.is_empty() {
            self.measure("|", MeasureFormat::Text, &self.formats.text, m)
        } else {
            self.measure(&m.mark_text, MeasureFormat::Text, &self.formats.text, m)
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

/// 决定 `measured` / `layout` 能不能跨帧复用的两件事：配置（含字体/DPI）代数，
/// 以及 `bar_position: auto` 每帧按光标方位算出来的编码栏上下位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayoutKey {
    generation: u64,
    bar_at_bottom: bool,
}

/// 内容没变且布局参数没变时，整份测量结果和布局都能直接复用。
fn reusable_layout(same_content: bool, old: LayoutKey, new: LayoutKey) -> bool {
    same_content && old == new
}

/// 一次呈现所需的不可变快照、事件发送端、实测尺寸与布局。
struct Content {
    snapshot: CandidateView,
    events: EventSink,
    measured: Measured,
    layout: Layout,
    layout_key: LayoutKey,
}

/// 窗口 UI 状态；由 [`Window::app`] 的动态借用保护，避免回调重入时别名可变访问。
struct App {
    /// 最近一次生效的配置来源（[`App::reload`] 用来判断"值不值得写一行日志"）。
    source: String,
    /// 最近一次读配置时主题文件的修改时间；变了就重载（换皮肤立刻生效）。
    /// 取的是 [`config::stamp_cached`]，比较时不会真去打文件系统。
    stamp: Option<std::time::SystemTime>,
    graphics: Graphics,
    content: Option<Content>,
    /// 最近一次渲染拿到的 `EventSink`。
    ///
    /// 2026-11-07 用户要求「三辅助面板常驻」：候选窗收起后（`content = None`）面板
    /// 还要能点符号/剪贴板卡片直拟上屏，不能再退化成复制到剪贴板 —— 这里留着最新
    /// 的 sink（携带当前会话/令牌/修订号），`WM_APP_COMMIT_TEXT` 在 content 为空时用它。
    last_events: Option<EventSink>,
    /// 原始（未按 DPI 缩放）的 style：DPI 变化时靠它重新缩放。
    raw_style: weasel_style::UIStyle,
    /// 当前生效的 style（含 DPI 缩放）与配色。
    metrics: Metrics,
    scheme: ColorScheme,
    /// 每次 `reload` / `set_dpi` 换字体时 +1，用来让缓存的测量结果失效。
    generation: u64,
    /// 当前悬停与按下（官方 `m_hoverIndex` / 点击计时）。
    hovered: Option<Hit>,
    pressed: Option<Hit>,
    /// 官方 `m_sticky`：面板被翻到插入点上方后保持在上方。
    sticky: bool,
    /// 是否启用系统毛玻璃（DWM Acrylic）；默认开，可用环境变量 `WEASEL_GLASS=0` 关掉。
    glass: bool,
    /// 显示前抓好的模糊背景（`glass_blur: "self"` 时由 [`Window::capture_backdrop`] 填充）。
    backdrop: Option<paint::BackdropCache>,
}

/// 窗口回调可访问的稳定状态分配。
/// 高亮滑动动画时长（秒）。
const ANIM_DURATION: f32 = 0.10;
/// 动画帧消息。
///
/// 2026-11-01：原来用 `WM_TIMER` 驱动每 8ms 一帧，**实测一帧都没到** —— theme.log 里
/// `动画计时器 armed` 有记录、`动画帧 t=` 一条都没有，所以高亮是瞬移的（用户看到
/// “候选没有从左向右运动”）。
///
/// 根因：`WM_TIMER` 和 `WM_PAINT` 一样是“队列空了才生成”的低优先级消息，只要渲染器
/// 那条消息循环没把它 `DispatchMessageW` 出去，就永远收不到。改成 **自排程**：画完一
/// 帧 `PostMessageW` 下一帧 —— 就是 VSCode / CSS 那套 `requestAnimationFrame` 模型，
/// 帧消息是普通投递消息，一定会被取走派发，不再看计时器的脸色。
const ANIM_MSG: i32 = WM_APP + 0x2a;

/// 动画帧间隔（毫秒）的**兜底值** ≈ 60Hz。实际帧间隔在动画启动时按 DWM 合成
/// 刷新率算：60Hz → 17ms、120Hz → 8ms、144Hz → 7ms（被下限钳到 8ms）。
///
/// 2026-11-10 抄 msime `Window.cpp` 的 `GetNextAnimationDeadline()`：原来固定
/// `ANIM_FRAME_MS = 8`（≈125FPS），在 60Hz 屏上 DWM 只按 60Hz 合成，一帧动画
/// 12 次 paint 里有约一半是无效帧。改成按刷新节拍走后 60Hz 只需 6 帧，绘制开销
/// 直接减半；高刷屏仍保持 8ms 不退化。
const ANIM_FRAME_FALLBACK_MS: u32 = 16;
/// 帧间隔下限（≈120Hz，避免异常高的刷新率又把动画变回过采样）。
const ANIM_FRAME_MIN_MS: u32 = 8;
/// 帧间隔上限（≈50Hz，防止远程桌面 / 低刷新率把 100ms 动画拉成两三帧幻灯片）。
const ANIM_FRAME_MAX_MS: u32 = 20;

/// 显示刷新率（Hz）→ 一帧的目标间隔（毫秒）。纯函数，便于单测。
fn frame_interval_ms(hz: f32) -> u32 {
    if !hz.is_finite() || hz <= 0.0 {
        return ANIM_FRAME_FALLBACK_MS;
    }
    ((1000.0 / hz).round() as u32).clamp(ANIM_FRAME_MIN_MS, ANIM_FRAME_MAX_MS)
}

/// 从 DWM 拿当前合成刷新率（Hz）。远程桌面 / 无合成器等场合会失败，返回 `None`
/// 让调用方回退到 60Hz——**失败是安全的**，不会让动画停摆。
unsafe fn composition_refresh_hz(hwnd: HWND) -> Option<f32> {
    let mut info: DwmTimingInfo = std::mem::zeroed();
    info.cb_size = std::mem::size_of::<DwmTimingInfo>() as u32;
    if unsafe { DwmGetCompositionTimingInfo(hwnd, &mut info) } < 0 {
        return None;
    }
    // packed(1) 结构体：按值 Copy 出字段，不取引用（避免 unaligned reference）。
    let num = info.rate_refresh.ui_numerator;
    let den = info.rate_refresh.ui_denominator;
    if num == 0 || den == 0 {
        return None;
    }
    Some(num as f32 / den as f32)
}

/// 系统计时器精度（`winmm`）。只在动画期间抬到 1ms，结束立刻还回去 —— 否则
/// `Sleep(interval_ms)` 会被系统按 15.6ms 粒度四舍五入，帧间隔就抖成
/// 15.6~31.2ms，连按 ↓ 时肉眼可见一顿。
///
/// 2026-11-10 调研时考虑过彻底去掉这个全局精度抬升（msime 靠 DWM 合成节拍
/// 不需要它），但 Weasel 的帧间等待仍走 `Sleep`：去掉会让 60Hz 帧抖动、
/// 高刷屏退化到 60FPS。要安全去掉得引入 high-resolution waitable timer +
/// `MsgWaitForMultipleObjects` 重排消息循环——那会碰 2026-11-01 出过
/// 「WM_TIMER 一帧都没到」事故的核心循环，超出本轮改动范围。保留：它只在
/// 100ms 的动画窗口内生效，是微软认可的「短时必要」用法。
#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}

/// 帧间等待（`kernel32`）。只等“到下一帧”的那几毫秒，画完立刻把控制权还给消息循环，
/// 所以按键最多被压后一帧的时间。
#[link(name = "kernel32")]
unsafe extern "system" {
    fn Sleep(ms: u32);
}

/// `PostMessageW`（`user32`）。绑定里没有，直接声明即可（HWND 是 `#[repr(transparent)]`）。
#[link(name = "user32")]
unsafe extern "system" {
    fn PostMessageW(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> i32;
}

/// 排下一帧（自排程的 rAF）。
unsafe fn post_anim_frame(hwnd: HWND) {
    unsafe {
        let _ = PostMessageW(hwnd, ANIM_MSG as u32, 0, 0);
    }
}

/// CSS `cubic-bezier(x1, y1, x2, y2)` 求值 —— VSCode 的过渡、CSS 的 `transition` 都用它。
///
/// 曲线在 x 上单调，所以对 x 二分求出参数 `s`，再回代算 y。以前用 `1 - (1-t)^3` 近似
/// `cubic-bezier(.22,.61,.36,1)`：那条近似在起步阶段比真曲线慢一半（t=0.2 时 0.49 vs
/// 0.71），头两帧几乎没位移，看起来就是“没在动”。
struct Bezier {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
}

impl Bezier {
    const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }
    fn at(&self, s: f32, a: f32, b: f32) -> f32 {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    }
    /// 时间进度 `t`（0..1）→ 动画进度（0..1）。
    fn ease(&self, t: f32) -> f32 {
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        let mut lo = 0.0f32;
        let mut hi = 1.0f32;
        for _ in 0..18 {
            let s = (lo + hi) * 0.5;
            if self.at(s, self.x1, self.x2) < t {
                lo = s;
            } else {
                hi = s;
            }
        }
        self.at((lo + hi) * 0.5, self.y1, self.y2)
    }
}

/// 高亮滑动的缓出曲线，与 HTML 预览里 `.hilite` 的 `cubic-bezier(.22,.61,.36,1)` 同一个。
const SLIDE_EASE: Bezier = Bezier::new(0.22, 0.61, 0.36, 1.0);

/// 一段高亮滑动（照 HTML 的 CSS transition 语义）。
///
/// HTML 里 `.hilite` 只对 `left`/`top` 做 100ms `cubic-bezier(.22,.61,.36,1)` 过渡，
/// 宽高瞬间定死；而且 CSS 过渡**改向时从当前算出来的位置接着滑**。所以这里存的是
/// 「起点矩形 / 终点矩形」，不是「旧下标 / 新下标」——旧下标只用来起头，动画中途
/// 再按键就直接从视觉位置改向。
#[derive(Clone, Copy)]
struct Anim {
    from: Rect,
    to: Rect,
    /// 目标候选下标（这段动画期间跳过它的静态高亮块，由插值矩形代替）。
    target: usize,
    start: std::time::Instant,
    /// 上一帧的时刻（用来算“还要等多久才到下一帧”）。
    last: std::time::Instant,
    /// 已经画了几帧（结束后记进日志，便于确认动画真的在跑）。
    frames: u32,
    /// 这一段的帧间间隔（毫秒）：启动时按 DWM 合成刷新率定，不再固定 8ms。
    interval_ms: u32,
}

struct Window {
    hwnd: Cell<HWND>,
    dpi: Cell<u32>,
    app: RefCell<App>,
    error: RefCell<Option<String>>,
    recovery: RefCell<Recovery>,
    positioning: Cell<bool>,
    /// 为真时采用可激活、可关闭的预览窗口行为。
    preview: bool,
    /// 自绘高斯模糊（`style/glass_blur: "self"`）：关 DWM 亚克力，改在显示前抓屏。
    glass_self: bool,
    /// 窗口当前是否**已经上屏**（`ShowWindow(SW_SHOW*)` 之后、`SW_HIDE` 之前）。
    ///
    /// 抓背景必须在上屏**之前**：上屏后抓到的"背景"就是这个窗口自己（DWM 已经把
    /// 它合成进桌面了），糊一遍再垫到面板底下，右半边就是一团白云 + 文字鬼影
    /// （2026-11-01 17:5x 用户：「右面是什么」）。加宽候选窗时走
    /// `WM_WINDOWPOSCHANGED` 重抓，那时窗口早已上屏 —— 就是这个 bug 的来源。
    shown: Cell<bool>,
    /// 上次成功抓背景的时刻（上屏后重抓的节流用）。
    last_backdrop: Cell<Option<std::time::Instant>>,
    /// 高亮滑动动画；`None` = 没在播。
    anim: std::cell::RefCell<Option<Anim>>,
    /// 上一帧真正画出来的高亮矩形（动画改向时从它接着滑）。
    visual_hilite: std::cell::Cell<Option<Rect>>,
    /// 上一帧的选中下标（用来判断"刚换了候选"）。
    last_selected: Cell<usize>,
    /// 是否已挂起「延迟隐藏」（`HIDE_DEBOUNCE_TIMER` 在跑）。
    /// 打字期间的空快照只置位，不真的 `SW_HIDE`；收到可见快照立即撤销。
    hide_pending: Cell<bool>,
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

/// 把窗口从 DWM 合成树里摘除/放回（`DWMWA_CLOAK`，2026-11-07 残留根治）。
///
/// `WS_EX_NOREDIRECTIONBITMAP` 窗口的 D2D `Present` 与 `DwmFlush` 不同步：
/// hide() 里"清成全透明再 SW_HIDE"的最后一帧偶尔没真正进合成器，DWM 就把
/// 上一帧缓存住了 —— 屏幕上留一块几分钟不散的透明面板。Cloak 之后合成器
/// 不再持有该窗口的任何视觉，残留从机制上不可能。显示前先解 Cloak，配对不漏。
fn cloak_window(hwnd: HWND, on: bool) {
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: HWND,
            attribute: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    let cloak: i32 = on as i32;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            13, // DWMWA_CLOAK
            &cloak as *const _ as *const core::ffi::c_void,
            core::mem::size_of::<i32>() as u32,
        );
    }
}

/// 快照是否"没有内容"（官方 `Context::empty`：无候选、无预编辑、无辅助串）。
fn empty_context(view: &CandidateView) -> bool {
    view.items.is_empty() && view.preedit.is_none()
}

// ---------------------------------------------------------------- 创建

fn create(mode: UiMode) -> Result<Box<dyn ThemeBackend>, String> {
    let preview = mode != UiMode::Live;
    // 编码栏三面板的后台服务（剪贴板采集 + 自动同步）在真实运行模式下随渲染器启动一次；
    // 预览窗口不启线程，免得开个设置预览就开始写用户数据。
    if !preview {
        crate::panels::start_services();
    }
    let loaded = config::load(crate::appearance::is_dark());
    let antialias = config::Antialias::from_name(&loaded.style.antialias_mode);
    // 毛玻璃：配置文件 `style/glass` 说了算（缺省 true，老配置没有该键也走 true）；
    // `WEASEL_GLASS=0/ false / off / no` 仍可强制关掉，便于排查。
    let glass = match std::env::var("WEASEL_GLASS") {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => loaded.style.glass,
    };
    // 模糊实现由**材质档位**决定，不再让 `style/glass_blur` 单独指挥
    // （用户 2026-11-04 08:5x：「设置是不是玻璃？磨砂透明的才叫玻璃，
    //   磨砂带颜色的是亚克力，亚克力是取背景色的」）。
    //
    // 原来的毛病：档位（material.txt）和实现（`glass_blur`，默认 `dwm`）是两个
    // 互不相干的开关。`dwm` = DWM 亚克力，本身就取背景色 + 叠 20% 白 tint ⇒
    // **选「透明」出来的是亚克力**，名不符实。
    //
    // 现在是**两档都自绘**（用户 2026-11-04 09:4x：「亚克力怎么是透明的，再模糊一些」）：
    //
    // 对应关系：
    // - [`Material::Glass`]（透明）  → 自绘高斯模糊 σ=3，面板底**全透明**
    //                                  ⇒ 真玻璃：磨砂但**不染色**
    // - [`Material::Acrylic`]（亚克力）→ 自绘高斯模糊 σ=8（更糊）+ 配色蒙版 85%
    //                                  ⇒ 磨砂 + 有色：色相取配色个性色、明度跟 back_color
    // - 默认 / 金属              → 两者都不用，压根没有模糊
    //
    // 亚克力为什么不再用 DWM：`SetWindowCompositionAttribute` 的模糊半径是系统写死的，
    // 「再模糊一些」根本调不动；而且 Win11 上那一档常常退化成普通模糊、tint 还被吃掉了，
    // 出来就是"透明"。自绘之后半径/强度/染色全在自己手里。
    //
    // `WEASEL_GLASS_BLUR=self|dwm` 仍可强制覆盖，只作排查用（`dwm` = 亚克力档走回 DWM，
    // 也是"抓屏权限被系统禁掉"时的退路）；`style/glass_blur` 从此只是兼容旧配置的残留键。
    let tier_now = paint::tier();
    let glass_self = match std::env::var("WEASEL_GLASS_BLUR") {
        Ok(value) => value.trim().eq_ignore_ascii_case("self"),
        Err(_) => tier_now.flat(),
    };
    let how = if glass_self {
        format!(
            "抓屏模糊 σ={:.1} · 蒙版 {:?}",
            paint::blur_sigma(tier_now),
            if tier_now == paint::Material::Acrylic {
                // 实际色打出来：配色文件是官方 ABGR 字节序，肉眼以为的"橙字"到内部是
                // `0xFF528CCD`（蓝）—— 蒙版跟的是**内部 ARGB**，对账时一眼就能看出差在哪。
                format!(
                    "配色蒙版 alpha={:#04X}（{:.0}%）· 实际色 {:#010X}",
                    paint::veil_alpha(),
                    paint::veil_alpha() as f32 / 255.0 * 100.0,
                    paint::theme_veil(&loaded.scheme)
                )
            } else {
                "无（全透明）".to_string()
            }
        )
    } else {
        "DWM 亚克力·取背景色".to_string()
    };
    crate::diagnostics::record(format_args!(
        "材质 {:?} + glass={glass} + glass_blur(残留配置)={:?} → glass_self={glass_self}（{how}）",
        tier_now, loaded.style.glass_blur
    ));
    // 毛玻璃染色用配色背景色；`loaded` 之后会被 `App::new` 吞掉，先取出来。
    let tint = loaded.scheme.back_color;
    let window = Rc::new(Window {
        hwnd: Cell::new(HWND::default()),
        dpi: Cell::new(96),
        glass_self,
        shown: Cell::new(false),
        last_backdrop: Cell::new(None),
        anim: std::cell::RefCell::new(None),
        visual_hilite: std::cell::Cell::new(None),
        last_selected: Cell::new(usize::MAX),
        hide_pending: Cell::new(false),
        app: RefCell::new(App::new(loaded, 96.0, glass)?),
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
            // + WS_THICKFRAME：Win11 的 DWM 圆角模板只对**有边框**的窗口生效；
            // 无边框 popup 上 `DWMWA_WINDOW_CORNER_PREFERENCE` 会被忽略（试过）。
            (WS_POPUP as u32 | WS_CLIPSIBLINGS as u32 | 0x0004_0000u32) // WS_THICKFRAME
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
        // 系统级毛玻璃：DWM 在窗口矩形背后做模糊，面板（半透明）与四角都透出桌面。
        // 染色用配色背景色，和皮肤观感保持一致。
        // 自绘模糊这档：关掉 DWM 给无边框窗口加的**系统投影**
        // （DWMWA_NCRENDERING_POLICY = DWMNCRP_DISABLED），否则圆角外侧仍会有一圈黑边。
        {
            unsafe extern "system" {
                fn DwmSetWindowAttribute(
                    hwnd: HWND,
                    attribute: u32,
                    value: *const core::ffi::c_void,
                    size: u32,
                ) -> i32;
            }
            // Win11 系统圆角（带 WS_THICKFRAME 之后才会生效）
            let round: i32 = 2;
            let _ = DwmSetWindowAttribute(
                hwnd,
                33,
                &round as *const _ as *const core::ffi::c_void,
                core::mem::size_of::<i32>() as u32,
            );
            let disabled: i32 = 2; // DWMNCRP_DISABLED
            let _ = DwmSetWindowAttribute(
                hwnd,
                2, // DWMWA_NCRENDERING_POLICY
                &disabled as *const _ as *const core::ffi::c_void,
                core::mem::size_of::<i32>() as u32,
            );
            // **单层**（用户 17:3x：「我是不是让你做一个单层，怎么还有一个外层」）：
            // Win11 会给带 `WS_THICKFRAME` 的窗口、沿 DWM 圆角轮廓画一圈 1px 的系统描边。
            // 面板不透明时被完全盖住看不出；改成 50% 透明之后，窗口边缘（面板外 ~8px）
            // 就露出一圈浅色"外层"（实测：屏幕坐标下比面板边外 8px 处那条线比背景亮 26 级）。
            // `DWMWA_BORDER_COLOR(34) = DWMWA_COLOR_NONE(0xFFFFFFFE)` 让 DWM 不画它。
            // Win10 不认这个属性 —— 调用失败就算了（`let _`）。
            let no_border: u32 = 0xFFFF_FFFE;
            let _ = DwmSetWindowAttribute(
                hwnd,
                34, // DWMWA_BORDER_COLOR
                &no_border as *const _ as *const core::ffi::c_void,
                core::mem::size_of::<u32>() as u32,
            );
        }
        // 亚克力**只给亚克力档**：
        // - 玻璃档走自绘抓屏模糊，DWM 再叠一层就成了"糊上糊"，边缘还会有 DWM 那层
        //   系统着色（用户 2026-11-04：「磨砂透明的才叫玻璃」）。
        // - 默认/金属档窗口外面那圈（offset ≈ 10px）没有面板盖住，开了亚克力就是
        //   "面板外多一层浅色"（用户 2026-11-04 00:5x 截图）。
        if glass && tier_now.flat() && !glass_self {
            accent::sync_acrylic(hwnd, true, tint);
        }
        if glass {
            // Win11：让 DWM 把**窗口本身**裁成圆角。亚克力铺满窗口矩形，
            // 不裁的话圆角外侧会露出 4 个方角色块（用户 13:4x：「圆角外面是有角的」）。
            unsafe extern "system" {
                fn DwmSetWindowAttribute(
                    hwnd: HWND,
                    attribute: u32,
                    value: *const core::ffi::c_void,
                    size: u32,
                ) -> i32;
            }
            let round: i32 = 2; // DWMWCP_ROUND
            let _ = DwmSetWindowAttribute(
                hwnd,
                33, // DWMWA_WINDOW_CORNER_PREFERENCE
                &round as *const _ as *const core::ffi::c_void,
                core::mem::size_of::<i32>() as u32,
            );
        }
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
    fn new(loaded: config::Loaded, dpi: f32, glass: bool) -> Result<Self, String> {
        let metrics = config::metrics(&loaded.style, dpi, paint::tier().flat())
            .with_colors(&loaded.scheme);
        crate::diagnostics::record(format_args!("主题配置: {}", loaded.source));
        crate::diagnostics::record(format_args!(
            "毛玻璃: {}",
            if glass { "开" } else { "关（style/glass=false 或 WEASEL_GLASS=0）" }
        ));
        // 首次读配置：把缓存里的时间戳对齐成"刚读过"，避免下一帧立刻判定为已改。
        config::refresh_stamp_cache();
        Ok(Self {
            stamp: config::stamp_cached(),
            source: loaded.source.clone(),
            graphics: Graphics::new(&metrics).map_err(|e| e.to_string())?,
            content: None,
            last_events: None,
            raw_style: loaded.style,
            metrics,
            scheme: loaded.scheme,
            generation: 0,
            hovered: None,
            pressed: None,
            sticky: false,
            glass,
            backdrop: None,
        })
    }

    /// DPI 变了：按原始 style 重新缩放度量与字号。
    fn set_dpi(&mut self, dpi: u32) {
        self.metrics = config::metrics(&self.raw_style, dpi.max(1) as f32, paint::tier().flat())
            .with_colors(&self.scheme);
        let _ = self.graphics.refresh_formats(&self.metrics);
        self.generation = self.generation.wrapping_add(1);
        if let Some(content) = &mut self.content {
            content.measured = self.graphics.measure_all(&self.metrics, &content.snapshot);
            content.layout = Layout::build(&self.metrics, &content.snapshot, &content.measured);
        }
    }

    /// 重新读配置（明暗外观变化、用户改皮肤时调用）。
    fn reload(&mut self, dark: bool, dpi: u32) {
        // 缓存里的时间戳可能已经过期（TTL 内没刷新过），这里强制对齐到"刚读过"，
        // 否则下一帧拿到的还是旧值，`changed` 会一直是 true —— 那正是每键重载的由来。
        config::refresh_stamp_cache();
        self.stamp = config::stamp_cached();
        let loaded = config::load(dark);
        // 2026-10-30：这一行原来**无条件**写 `theme.log`。实测每次 `record()` 都要
        // open+append+close（0.386ms），而 `reload` 在"时间戳被判为已变"时每键都会跑，
        // 于是 `theme.log` 长到 25 万行、几十 MB。改成**只在来源真的变了**才写一行 ——
        // 换皮肤照样看得见，日常打字不再落盘。
        if loaded.source != self.source {
            crate::diagnostics::record(format_args!("主题配置: {}", loaded.source));
            self.source = loaded.source.clone();
        }
        self.raw_style = loaded.style;
        self.scheme = loaded.scheme;
        // 重读毛玻璃开关：环境变量优先，其次配置（与 create() 里一致）。
        // 用户改 `style/glass` 或设 `WEASEL_GLASS` 后，热切换即时生效。
        self.glass = match std::env::var("WEASEL_GLASS") {
            Ok(value) => !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            ),
            Err(_) => self.raw_style.glass,
        };
        self.metrics =
            config::metrics(&self.raw_style, dpi.max(1) as f32, paint::tier().flat())
                .with_colors(&self.scheme);
        let _ = self.graphics.refresh_formats(&self.metrics);
        self.generation = self.generation.wrapping_add(1);
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
    /// `hide()` 限频诊断：1 秒内的连发只打首条，下一秒首条带出被压掉的次数。
    ///
    /// `panels_open` 只影响文案（面板开着才是「面板保留（常驻）」）；面板没开时也照记 ——
    /// 用户报的「输入时候选窗被关掉」不一定只在开面板时出现，漏记就查不到来源。
    fn log_hide(why: &'static str, panels_open: bool) {
        let total = HIDE_TOTAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let anchor = HIDE_LOG_ANCHOR.get_or_init(std::time::Instant::now);
        let now_ms = anchor.elapsed().as_millis() as u64;
        let last = HIDE_LOG_AT_MS.load(std::sync::atomic::Ordering::Relaxed);
        let since = HIDE_LOG_SINCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if now_ms.saturating_sub(last) >= 1000 {
            HIDE_LOG_AT_MS.store(now_ms, std::sync::atomic::Ordering::Relaxed);
            HIDE_LOG_SINCE.store(0, std::sync::atomic::Ordering::Relaxed);
            let tail = if panels_open {
                " → 面板保留（常驻）"
            } else {
                ""
            };
            crate::diagnostics::record(format_args!(
                "[panel] 候选窗收起{tail} src={why} 累计{total}次（上1秒压掉{since}次）"
            ));
        }
    }
    fn cancel(&self) {
        // 2026-11-07 修重入 panic：**先把 RefCell 借用放掉，再 ReleaseCapture**。
        // ReleaseCapture 会**同步**把 WM_CAPTURECHANGED 派发回本窗口过程，那个分支
        // 又调用 cancel()；如果这里还握着 `app.borrow_mut()`，嵌套借第二层就 panic
        // 「RefCell already borrowed」（本棒用合成点击复现，确为 msg=0x215）——
        // 而 panic 会被 wnd_proc 的 catch_unwind 记成主题错误，候选窗从此不再重绘。
        // 释放捕获本身不需要借用，字段清空在借用作用域内完成即可。
        {
            let mut app = self.app.borrow_mut();
            app.pressed = None;
            app.hovered = None;
        }
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
        // 主题后端销毁 = 渲染器/服务退出：常驻面板这时才收起（用户 2026-11-07：
        // 只有点关闭钮或退出服务才消失）。同时清掉候选窗句柄，避免面板向已销毁窗口投消息。
        crate::panels::close_all_panels();
    }
}

impl ThemeBackend for Ten {
    fn render(&mut self, snapshot: &CandidateView, events: &EventSink) -> Result<(), String> {
        self.window.render(snapshot, events)
    }
    fn hide(&mut self) {
        self.window.hide("backend_hide");
    }
    fn refresh_appearance(&mut self) -> Result<(), String> {
        let dark = crate::appearance::is_dark();
        let dpi = self.window.dpi.get();
        self.window.app.borrow_mut().reload(dark, dpi);
        self.window.sync_glass();
        self.window.invalidate();
        self.window.health()
    }
    fn check_health(&mut self) -> Result<(), String> {
        self.window.health()
    }
}

impl Window {
    /// 把当前 [`App::glass`] / [`App::scheme`] 同步到 DWM Acrylic：
    /// 开关变了就开/关，没变但配色变了就更新染色。
    ///
    /// 必须在 [`Window::hwnd`] 已创建之后调用；窗口还没建好时静默跳过。
    fn sync_glass(&self) {
        let hwnd = self.hwnd.get();
        if hwnd.0.is_null() {
            return;
        }
        let app = self.app.borrow();
        let glass = self.acrylic_on();
        let tint = app.scheme.back_color;
        accent::sync_acrylic(hwnd, glass, tint);
    }

    /// 现在该不该开 DWM 亚克力。
    ///
    /// 三个条件缺一不可：`style/glass` 开着、不是自绘模糊（`glass_blur: self`）、
    /// 而且**材质档位必须是玻璃/亚克力**（[`paint::Material::flat`]）。
    ///
    /// 最后这条是 2026-11-04 00:5x 补的：窗口比面板大一圈（[`config::Metrics::offset`]
    /// ≈ `border*2 + 投影` ≈ 10px），那一圈没有任何东西盖住。默认档还开着亚克力时，
    /// 它就直接露成"面板外面多一层浅色"——和同一次截图里那道白描边一起，看着就是
    /// "外面一圈白的"。不透明/金属档底色本来就不透，亚克力没有任何东西可透，关掉也
    /// 不损失效果，还省一次 DWM 合成。
    fn acrylic_on(&self) -> bool {
        paint::tier().flat() && self.app.borrow().glass && !self.glass_self
    }

    /// 抓屏 + 高斯模糊 + 建 D2D 位图，存进 [`App::backdrop`]。
    ///
    /// 失败静默（`paint` 端缓存为 `None` 就不铺模糊，面板底色照旧）。
    fn capture_backdrop(&self) {
        let hwnd = self.hwnd.get();
        if hwnd.0.is_null() {
            return;
        }
        // 「只识别背景」（用户 17:4x）：抓屏前把**本窗口**从屏幕内容里排除掉
        // （`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`），抓完立刻恢复
        // `WDA_NONE` —— 否则用户自己截图就截不到候选窗了。
        // 这样即使窗口已经上屏（加宽 / 跟着光标移动），抓到的也只有背景，
        // 不会把自己那块白面板糊进去（17:3x「右面是什么」就是自拍鬼影）。
        // 只有 Win10 2004+ 支持；老系统这一步失败 ⇒ 上屏后放弃抓屏，
        // 回退成"显示前抓一次 + 拉伸"。
        // **每次都排除自己**（不只是上屏时）。`hide()` 之后 DWM 不一定已经把上一帧撤掉，
        // 那一刻抓屏抓到的还是我们自己那块面 —— 实测抓到过 `平均亮度 127` 的"背景"
        // （真背景 28），面板 = 50% 白 + 一张本来就 50% 白的自拍 ⇒ 198，看着就是不透明
        // （用户 18:0x：「为什么不透明」）。
        if self.shown.get() {
            // 节流：上屏后重抓最多 ~8 次/秒。抓一次要 DwmFlush 等一帧 + 抓屏 +
            // 高斯（≈20ms），每键都做会让打字发涩；背景本身不会一帧一变。
            if let Some(last) = self.last_backdrop.get() {
                if last.elapsed() < std::time::Duration::from_millis(120) {
                    return;
                }
            }
        }
        let excluded = unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).as_bool() };
        // 老系统（不支持这个亲和性）：上屏状态下抓不到干净背景，干脆不抓，
        // 回退成"显示前抓一次 + 拉伸"。
        if !excluded && self.shown.get() {
            return;
        }
        if excluded {
            // 等一帧，让 DWM 真的按新的抓屏亲和性合成一次
            unsafe {
                let _ = DwmFlush();
            }
        }
        let _affinity = CaptureAffinity {
            hwnd,
            active: excluded,
        };
        let mut app = self.app.borrow_mut();
        let antialias = app.metrics.antialias;
        let App {
            graphics,
            metrics,
            backdrop,
            ..
        } = &mut *app;
        let _ = metrics;
        if graphics.ensure_target(hwnd, self.dpi.get(), antialias).is_err() {
            return;
        }
        let Some(presenter) = graphics.target.as_ref() else {
            return;
        };
        unsafe {
            let mut wr = RECT::default();
            if !GetWindowRect(hwnd, &mut wr).as_bool() {
                return;
            }
            let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
            // **别抓"还没定位"的窗口**：创建/隐藏期间窗口只有 16x16，抓到的是一小块
            // 黑图（平均亮度 0），既会把 8x8 的位图拉伸成整面板的"糊墙"，也会让
            // 字色判断永远按"深色背景"算（用户 2026-11-04 问"字体自动变色呢"）。
            // 真实候选窗至少也有几十像素，小于这个门槛直接跳过，保留上一张好图。
            const MIN_CAPTURE: i32 = 48;
            if w < MIN_CAPTURE || h < MIN_CAPTURE {
                return;
            }
            // 模糊强度按档位给：透明档 σ=3（毛玻璃），亚克力档 σ=6（糊一行多）。
            let sigma = paint::blur_sigma(paint::tier());
            let Some((mut rgba, bw, bh)) = backdrop::capture_blurred(wr.left, wr.top, w, h, sigma)
            else {
                return;
            };
            // 圆角遮罩：背景只留在**面板**范围内（窗口比面板大 offset 一圈，那圈给阴影/描边，
            // 必须透明），半径就用面板圆角本身——之前写 `radius / 2.0`，遮罩比面板圆角小一半，
            // 圆角外的背景没被裁掉，四角就露出方角（用户 11-01："圆角怎么还没切掉"）。
            // ⚠️ `panel_radius()` 已经是**像素**（`Metrics` 里按 DPI 缩放过），
            // 再乘 dpi/96 会让遮罩半径大 25% ⇒ 面板弧和遮罩弧曲率不一致（21:1x 用户报的）。
            // 2026-11-06：挤压三模式大圆角减半，遮罩必须跟着窗口/灰壳一起减半。
            let radius = metrics.panel_radius();
            // 2026-11-06「接入默认玻璃亚克力三个配色」：挤压三模式的**窗口就是面板**
            // （`draw_squeeze` 从 (0,0) 画满 `content_size`，没有给投影/描边留的那圈
            // `offset`）。这里若还按 `offset()` 内缩（用户主题 border=3 ⇒ 6px），
            // 模糊位图四周就有一圈透明 ⇒ 玻璃档露出一圈**没糊的桌面**锐边；
            // 亚克力档那圈又被半透明蒙版单独染一遍，颜色也对不上。
            // 官方六种布局仍用原 inset（那圈是给描边/阴影的），零回归。
            let (inset_x, inset_y) = if metrics.squeeze {
                (0.0, 0.0)
            } else {
                metrics.offset()
            };
            // 抓屏位图可能跟窗口不同尺寸，按比例换算到像素
            let sx = bw as f32 / w.max(1) as f32;
            let sy = bh as f32 / h.max(1) as f32;
            backdrop::mask_rounded(
                &mut rgba,
                bw as usize,
                bh as usize,
                radius * sx,
                inset_x * sx,
                inset_y * sy,
            );
            let props = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    // 逐像素 alpha（圆角外透明）。之前是 IGNORE —— alpha 直接被丢弃，
                    // 上面算出来的遮罩等于白做，背景就整块方形贴上来，四角露方角。
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            let Ok(bitmap) = presenter.render_target().CreateBitmap(
                D2D_SIZE_U {
                    width: bw,
                    height: bh,
                },
                Some(rgba.as_ptr().cast()),
                bw * 4,
                &props,
            ) else {
                return;
            };
            let mean = {
                // 只统计**不透明**像素：圆角遮罩把四角刷成 alpha=0（RGB 也是 0），
                // 一并平均进去会把亮度算低一截，字色就偏"按深底算"。
                let mut sum: u64 = 0;
                let mut n: u64 = 0;
                for pixel in rgba.chunks_exact(4).step_by(37) {
                    if pixel[3] == 0 {
                        continue;
                    }
                    sum += pixel[1] as u64;
                    n += 1;
                }
                if n == 0 { 0 } else { (sum / n) as u32 }
            };
            crate::diagnostics::record(format_args!(
                "自绘模糊: 抓屏 {w}x{h} → 位图 {bw}x{bh} · 平均亮度 {mean} · 排除自身={excluded}"
            ));
            *backdrop = Some(paint::BackdropCache {
                width: w,
                height: h,
                bitmap,
                luma: mean,
            });
            self.last_backdrop.set(Some(std::time::Instant::now()));
        }
    }

    /// 应用候选快照：内容没变就只重新定位，变了就重新测量 + 布局 + 绘制。
    fn render(&self, snapshot: &CandidateView, events: &EventSink) -> Result<(), String> {
        // 2026-11-08 防闪烁：只要又收到一帧快照，就撤销挂起的延迟隐藏。
        // 打字时「空快照 → 有内容」交替，挂起永远到不了期；真停手 120ms 后
        // WM_TIMER 才会落地隐藏。不可见分支会在下面立刻重新挂起，不冲突。
        if self.hide_pending.get() {
            // 2026-11-08 诊断（排查虎整句闪烁）：记录「挂起隐藏又被谁撤销」。
            crate::diagnostics::record(format_args!(
                "[取消挂起隐藏] 新快照 visible={} items={} preedit={} content_id={}",
                snapshot.visible,
                snapshot.items.len(),
                snapshot.preedit.is_some(),
                snapshot.content_id
            ));
        }
        self.cancel_pending_hide();
        // 2026-11-08 `bar_position: auto`：本帧的编码栏位置按候选窗相对光标的
        // 方位决定，做成一份局部 metrics，不污染 `App::metrics` 里的全局配置。
        let layout_metrics = self.layout_metrics(snapshot);
        let layout_key = LayoutKey {
            generation: self.app.borrow().generation,
            bar_at_bottom: layout_metrics.bar_at_bottom,
        };
        // 面板常驻（2026-11-07 用户要求）：**每次**渲染都留下最新的 EventSink。
        // 候选窗收起后 `content` 会被清空，面板点按（`WM_APP_COMMIT_TEXT`）靠它把
        // 文本送进引擎；配合 `Factory::capabilities` 的 `resident=true`，渲染器在
        // 「输入上下文仍活动但无候选」的快照上也会调到这里（否则只有 hide()，拿不到新 sink）。
        self.app.borrow_mut().last_events = Some(events.clone());
        let same = {
            let mut app = self.app.borrow_mut();
            let App { content, .. } = &mut *app;
            match content.as_mut() {
                Some(content)
                    if crate::presentation::is_visible(snapshot)
                        || crate::presentation::is_mode_indicator_visible(snapshot) =>
                {
                    if reusable_layout(
                        crate::theme_api::same_content(&content.snapshot, snapshot),
                        content.layout_key,
                        layout_key,
                    ) {
                        // 2026-11-10：注释早写着「内容没变就只重新定位」，旧实现却每帧
                        // 重跑 measure_all（15 次 DirectWrite 测量）+ Layout::build。
                        // 现在只换快照与事件，测量和布局整份复用，下面 `position()` 照旧。
                        content.snapshot = snapshot.clone();
                        content.events = events.clone();
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            }
        };
        if same {
            {
            // 有真实候选时撤掉模式提示的自毁计时器
            let app = self.app.borrow();
            let keep = app
                .content
                .as_ref()
                .is_some_and(|c| c.snapshot.mode_indicator.is_none() || !c.snapshot.items.is_empty());
            if keep {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd.get()), AUTOHIDE_TIMER);
                }
            }
        }
        self.position().map_err(|e| e.to_string())?;
            self.invalidate();
            return self.health();
        }
        self.cancel();
        self.health()?;
        if !(crate::presentation::is_visible(snapshot)
            || crate::presentation::is_mode_indicator_visible(snapshot))
        {
            // 2026-11-08 诊断（排查虎整句「一直很闪」）：只在**窗口正显示着**时记录，
            // 因为「已隐藏时的空帧」不会造成闪烁，记了只会把 theme.log 写爆。
            if self.shown.get() {
                crate::diagnostics::record(format_args!(
                    "[不可见帧] 窗口正显示 → 将隐藏：items={} preedit={} active={} visible={} content_id={} pending={}",
                    snapshot.items.len(),
                    snapshot.preedit.is_some(),
                    snapshot.active,
                    snapshot.visible,
                    snapshot.content_id,
                    self.hide_pending.get()
                ));
            }
            self.hide("render_invisible");
            return Ok(());
        }
        // 主题文件被改过（例如刚在设置里换了皮肤）就重载一次，不必重启输入法。
        //
        // 2026-10-30：这里原来每次渲染都 `dirs()` + 6 次 `metadata()` 去比时间戳，
        // 等于**每键打一次文件系统**；改成带 TTL 的缓存（见 `config::stamp_cached`），
        // 换皮肤最迟 0.75 秒生效，日常打字不再碰磁盘。
        let changed = self.app.borrow().stamp.ne(&config::stamp_cached());
        if changed {
            let dark = crate::appearance::is_dark();
            let dpi = self.dpi.get();
            self.app.borrow_mut().reload(dark, dpi);
            self.sync_glass();
        }
        let was_hidden = self.app.borrow().content.is_none();
        {
            let mut app = self.app.borrow_mut();
            let measured = app.graphics.measure_all(&layout_metrics, snapshot);
            let layout = if snapshot.mode_indicator.is_some() && snapshot.items.is_empty() {
                Layout::mode_indicator(&layout_metrics)
            } else {
                Layout::build(&layout_metrics, snapshot, &measured)
            };
            crate::diagnostics::record_verbose(format_args!(
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
                crate::diagnostics::record_verbose(format_args!(
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
                layout_key,
            });
        }
        self.position().map_err(|e| e.to_string())?;
        if was_hidden {
            // 自绘高斯模糊：**显示之前**抓屏——此刻窗口内容还是全透明，
            // 抓到的是背后真实画面；paint 里现场抓会把自己抓进去（检查点 §7.5）。
            if self.glass_self {
                self.capture_backdrop();
            }
            // ★ 重新打开 DWM 亚克力（hide 里关了）。必须在 ShowWindow 之前开，
            // 否则第一帧没有亚克力、显示后 DWM 才补上 ⇒ 闪一下没模糊的面板。
            if self.acrylic_on() {
                accent::sync_acrylic(self.hwnd.get(), true, self.app.borrow().scheme.back_color);
            }
            // 隐藏期间窗口保留上一帧，先画一次再显示，避免闪出旧内容
            if let Err(error) = self.paint() {
                self.graphics_error(error);
            }
        }
        unsafe {
            // 显示前先解除 hide() 留下的 Cloak(配对见 cloak_window 注)。
            cloak_window(self.hwnd.get(), false);
            let _ = ShowWindow(
                self.hwnd.get(),
                if self.preview { SW_SHOW } else { SW_SHOWNOACTIVATE },
            );
        }
        self.shown.set(true);
        // 真有候选/预编辑时撤掉模式提示的自毁计时器（不然 900ms 后会把面板隐藏）
        self.invalidate();
        self.health()
    }

    /// 生成本帧布局用的 metrics：`bar_position: auto` 时按候选窗相对光标的方位
    /// 覆盖编码栏位置，做成本帧局部变量，不污染 `App::metrics` 的全局配置。
    fn layout_metrics(&self, snapshot: &CandidateView) -> Metrics {
        let mut m = self.app.borrow().metrics.clone();
        if m.bar_position_auto {
            m.bar_at_bottom = self.desired_bar_at_bottom(snapshot, &m);
        }
        m
    }

    /// `auto` 的判据：**编码栏永远贴着光标那一侧**。
    ///
    /// 光标下方放不下整窗时 `reposition` 会把窗口翻到光标上方（`y = anchor.top - h`），
    /// 此时编码栏放窗口**底部**才紧挨光标；窗口在下方时则放**顶部**。
    /// 用上一帧窗口高度估算，第一帧没有高度就先按配置落点。
    fn desired_bar_at_bottom(&self, snapshot: &CandidateView, m: &Metrics) -> bool {
        let Some(anchor) = snapshot.anchor.as_ref().filter(|a| a.valid) else {
            return m.bar_at_bottom;
        };
        let Some(work) = crate::presentation::work_area(anchor) else {
            return m.bar_at_bottom;
        };
        let est_h = self
            .app
            .borrow()
            .content
            .as_ref()
            .map(|c| pixels(c.layout.content_size.cy, self.dpi.get()))
            .unwrap_or(0);
        anchor.bottom.saturating_add(est_h) > work.bottom
    }

    /// 撤销挂起的延迟隐藏（`render()` 收到新快照时调）。
    fn cancel_pending_hide(&self) {
        if self.hide_pending.replace(false) {
            unsafe {
                let _ = KillTimer(Some(self.hwnd.get()), HIDE_DEBOUNCE_TIMER);
            }
        }
    }

    /// **请求**隐藏：不立即 `SW_HIDE`，只挂一个短计时器（防打字闪烁）。
    ///
    /// `why` 是来源标记（诊断，2026-11-07）：
    /// `render_invisible` = `render()` 收到不可见快照；`backend_hide` = 渲染器
    /// `Presentation::apply/hide` 直接调后端；`autohide_timer` = 模式提示到期。
    fn hide(&self, why: &'static str) {
        let _ = why;
        // 根本没上屏：没有「闪」可防，直接收尾（清 content、停动画等）。
        if !self.shown.get() {
            self.hide_now("debounced_hide");
            return;
        }
        // 已挂起就不重复设计计时器；计时器从**第一次**隐藏请求起算，
        // 期间任何可见快照都会把它撤销（见 `render` 开头）。
        if self.hide_pending.get() {
            return;
        }
        self.hide_pending.set(true);
        unsafe {
            let _ = SetTimer(
                Some(self.hwnd.get()),
                HIDE_DEBOUNCE_TIMER,
                HIDE_DEBOUNCE_MS,
                None,
            );
        }
    }

    /// 真正隐藏窗口并释放交互状态（由 `HIDE_DEBOUNCE_TIMER` 落地）。
    fn hide_now(&self, why: &'static str) {
        self.hide_pending.set(false);
        unsafe {
            let _ = KillTimer(Some(self.hwnd.get()), HIDE_DEBOUNCE_TIMER);
        }
        // 2026-11-07 用户口径「面板常驻」：候选窗收起（组字结束/上屏/失焦）**不再**
        // 关闭三辅助面板 —— 面板要能继续点、继续拖，输入结束也不消失。
        // 只有面板自己的关闭钮或主题后端销毁（服务退出）才关，见 `panels::close_all_panels`。
        Self::log_hide(why, crate::panels::any_panel_open());
        // 是否真的显示着：已经隐藏时不必再做「D2D 清透明 + DwmFlush」这两步最贵的动作
        // （hide 风暴下它们是纯浪费），但 SW_HIDE/Cloak/亚克力关等保障性动作照做。
        let was_shown = self.shown.get();
        self.cancel();
        self.app.borrow_mut().content = None;
        self.recovery.borrow_mut().waiting = false;
        unsafe {
            let _ = KillTimer(Some(self.hwnd.get()), RETRY_TIMER);
            if self.anim.borrow_mut().take().is_some() {
                crate::diagnostics::record(format_args!("hide 清掉了进行中的动画"));
                unsafe {
                    timeEndPeriod(1);
                }
            }
            // ★ 关掉 DWM 亚克力：亚克力是 DWM 合成层特效，不随 SW_HIDE 立刻消失。
            // D2D 内容清成透明后，DWM 仍保留那一帧模糊 ⇒ 打完字偶尔残留半透明界面。
            // 先 disable_acrylic 再清内容 + SW_HIDE，DWM 合成层就没有模糊可残留。
            // （render 的 was_hidden 分支会重新 sync_glass 打开，配对不漏。）
            if self.acrylic_on() {
                accent::sync_acrylic(self.hwnd.get(), false, 0);
            }
            // 隐藏前先把表面清成**全透明**再上屏一次：DWM 有时不会立刻把窗口
            // 撤出合成，残留的就会是上一帧那张模糊背景（用户 18:1x：「打完字有时
            // 候这个背景还是会显示」）。清成空气之后，就算晚一帧也看不见东西。
            let presenter = self.app.borrow().graphics.target.is_some();
            // 只在「真的显示着」时做清屏 + DwmFlush：已经隐藏时合成器里早没有这帧了，
            // 再清一遍是白费 —— hide 风暴里这两步是最贵的（2026-11-07 诊断需求）。
            if was_shown {
                if let Some(presenter) = self.app.borrow().graphics.target.as_ref() {
                    // ⚠️ D2D 的 `Clear` 必须在 `BeginDraw`/`EndDraw` 之间调，否则上下文进入
                    // 非法状态 —— 下一次 `EndDraw` 就报 `0x88990001 (D2DERR_WRONG_STATE)`，
                    // 进而 `graphics_error` → 健康检查失败 → **渲染器整个退出**（候选窗消失）。
                    // 2026-11-01 18:3x 就这么把候选窗整没了。
                    presenter.begin_draw();
                    presenter.clear_transparent();
                    let _ = presenter.end_draw();
                } else {
                    // 没画布可清：这一帧注定留在屏幕上，至少留个可查的痕迹。
                    crate::diagnostics::record(format_args!("hide 时没有绘制目标，无法清屏"));
                }
                // 2026-11-02 03:1x **修「打完字残留磨砂面板」**：
                // 光清屏不够 —— `Present` 只是把帧排进队列，紧接着的 `SW_HIDE` 会让 DWM
                // 直接按"窗口已隐藏"合成，**那一帧全透明的画面根本不会上屏**，屏幕上留着的
                // 还是上一帧那块高斯模糊面板。实测（用户 03:1x 截图 + 外部枚举窗口）：
                // 此时 `IsWindowVisible=false`，但屏幕 (1321,678)-(1920,885) 那块一直是糊的，
                // 能停好几分钟不动。
                // ⇒ 清屏之后必须 `DwmFlush()` 等这一帧真的被合成出去，再撤窗口。
                if presenter {
                    let _ = DwmFlush();
                }
            }
            let _ = ShowWindow(self.hwnd.get(), SW_HIDE);
            // SW_HIDE 之后再 Cloak:合成器彻底丢弃本窗口的视觉(见 cloak_window 注)。
            cloak_window(self.hwnd.get(), true);
        }
        self.shown.set(false);
    }

    /// 绘制一帧（官方 `DoPaint`）。
    fn paint(&self) -> windows_core::Result<()> {
        let mut app = self.app.borrow_mut();
        let glass = app.glass;
        let App {
            graphics,
            content,
            metrics,
            scheme,
            hovered,
            backdrop,
            ..
        } = &mut *app;
        graphics.ensure_target(self.hwnd.get(), self.dpi.get(), metrics.antialias)?;
        let Some(presenter) = graphics.target.as_ref() else {
            // 客户区还是空的：等窗口显示后的下一条消息再来建目标
            return Ok(());
        };
        let target = presenter.render_target();
        let scheme = &*scheme;
        // 抓屏模糊档（透明/亚克力）：面板不再铺死白，配色与字色要跟着**真实背景**走。
        // 其它材质（默认/金属）底色是自己铺的，配色表原样即可。
        let plain_palette;
        let tier = paint::tier();
        // 亚克力蒙版色：拿**原始配色**（`plain_scheme` 之前）算，plain_scheme 内部算
        // 字色时用的也是这一个值，画的时候直接铺它 —— 算的和画的是同一个颜色。
        let veil = paint::theme_veil(scheme);
        let scheme = if tier.flat() {
            // 玻璃档：抓到的背景平均亮度就是"有效背景"；
            // 亚克力档：DWM 模糊 + 50% 配色底，`plain_scheme` 会按"桌面纯黑/纯白"
            // 两种极端一起算，保证换壁纸也不会出现"深字压深底"。
            plain_palette = paint::plain_scheme(
                scheme,
                tier,
                backdrop.as_ref().map(|b| b.luma as f32),
            );
            &plain_palette
        } else {
            scheme
        };
        let metrics = &*metrics;
        // 本帧是否画坏（画失败 / EndDraw 失败）。画坏只丢帧，不再让渲染器退出。
        let mut frame_failed = false;
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
            // ---- 高亮滑动动画：照 HTML 的 `.hilite`（只滑位置，宽高瞬间定死）----
            let selected_now = content
                .as_ref()
                .map(|c| c.snapshot.selected_index as usize)
                .unwrap_or(0);
            let target_rect = content
                .as_ref()
                .map(|c| c.layout.highlight_rect(metrics, selected_now));
            {
                let mut anim = self.anim.borrow_mut();
                if self.last_selected.get() != usize::MAX
                    && self.last_selected.get() != selected_now
                {
                    if let Some(to) = target_rect {
                        // CSS 过渡的语义：**改向时从当前算出来的位置接着滑**，而不是从
                        // 旧候选的矩形重新起步 —— 连按 ↓ 才不会一格一格地顿。
                        let from = self.visual_hilite.get().unwrap_or(to);
                        // 2026-11-10：逐帧/高频明细一律走 record_verbose（默认零写盘），
                        // 排查动画时 WEASEL_THEME_LOG=1 全量恢复。
                        crate::diagnostics::record_verbose(format_args!(
                            "高亮动画: {} → {}（{}ms 缓出）",
                            self.last_selected.get(), selected_now, (ANIM_DURATION * 1000.0) as u32
                        ));
                        // 帧间隔按 DWM 合成刷新率定一次；失败（远程桌面等）回退 60Hz。
                        // 不盲抄 msime 的 DwmFlush 驱动：WS_EX_NOREDIRECTIONBITMAP
                        // 窗口的 D2D Present 与 DwmFlush 不同步（hide 残留过透明面板）。
                        let interval_ms = unsafe {
                            composition_refresh_hz(self.hwnd.get())
                                .map(frame_interval_ms)
                                .unwrap_or(ANIM_FRAME_FALLBACK_MS)
                        };
                        *anim = Some(Anim {
                            from,
                            to,
                            target: selected_now,
                            start: std::time::Instant::now(),
                            last: std::time::Instant::now(),
                            frames: 0,
                            interval_ms,
                        });
                        timeBeginPeriod(1);
                        // 自排程：画完这一帧再投下一帧，不依赖 WM_TIMER 是否被派发。
                        post_anim_frame(self.hwnd.get());
                        crate::diagnostics::record_verbose(format_args!(
                            "高亮动画启动 hwnd={:?}（{:.0}ms / {}ms 一帧，DWM 节拍）",
                            self.hwnd.get().0,
                            ANIM_DURATION * 1000.0,
                            interval_ms
                        ));
                    }
                }
                self.last_selected.set(selected_now);
            }
            let hilite_anim = {
                let anim = self.anim.borrow();
                match anim.as_ref() {
                    Some(a) => {
                        let t = (a.start.elapsed().as_secs_f32() / ANIM_DURATION).clamp(0.0, 1.0);
                        if t >= 1.0 {
                            None
                        } else {
                            // 与 HTML 预览同一条 CSS 缓动曲线（cubic-bezier(.22,.61,.36,1)）
                            let eased = SLIDE_EASE.ease(t);
                            let lerp = |x: f32, y: f32| x + (y - x) * eased;
                            // **照 HTML/CSS 的语义**：`.hilite` 过渡的是 left 与 width，
                            // 等于四条边各自插值 —— 换到更宽/更窄的候选时框跟着伸缩。
                            // 2026-11-01 18:4x 用户：「宽度没有变化，和 html 版不一样」；
                            // 之前为了治"边滑边压扁"把宽高钉成目标值，钉过头了。
                            let left = lerp(a.from.left, a.to.left);
                            let top = lerp(a.from.top, a.to.top);
                            let right = lerp(a.from.right, a.to.right);
                            let bottom = lerp(a.from.bottom, a.to.bottom);
                            Some((Rect::new(left, top, right, bottom), a.target))
                        }
                    }
                    None => None,
                }
            };
            self.visual_hilite
                .set(hilite_anim.map(|(rect, _)| rect).or(target_rect));
            if let Some(content) = content.as_ref() {
                let backdrop_ref = backdrop.as_ref();
                let mut painter = Painter::new(
                    &target,
                    presenter.d2d_factory(),
                    &graphics.write,
                    &brush,
                    &graphics.formats,
                    metrics,
                    scheme,
                    &content.snapshot,
                    glass,
                );
                painter.hwnd = self.hwnd.get().0 as isize;
                painter.glass_self = self.glass_self;
                painter.veil = veil;
                painter.backdrop = backdrop_ref;
                painter.hilite_anim = hilite_anim;
                painter.hover_index = match hovered {
                    Some(Hit::Candidate(i)) => Some(*i),
                    _ => None,
                };
                painter.hover_button = match hovered {
                    Some(Hit::BarButton(i)) => Some(*i),
                    _ => None,
                };
                let indicator = content.snapshot.mode_indicator.is_some();
                // 提示窗有两种形态：**只有图标**（68×68 的小窗，无候选）
                // 和 **组字中徽标**（候选窗还开着，在预编辑后面叠一个小「中/英」）。
                let icon_only = indicator && content.snapshot.items.is_empty();
                if indicator {
                    // 防呆：模式提示那个小窗（68×68）如果因为任何原因没收到"过期"重绘，
                    // 会一直停在屏幕上（用户 2026-11-01 22:5x：「打完字会出现半透明的界面，
                    // 还关不掉」）。这里挂一个一次性自毁计时器兜底。
                    unsafe {
                        let _ = SetTimer(Some(self.hwnd.get()), AUTOHIDE_TIMER, 900, None);
                    }
                }
                let ascii = content.snapshot.ascii_mode.unwrap_or(false);
                // 这次提示是不是 CapsLock 造成的（引擎判定后带下来的）。
                let caps_lock = content
                    .snapshot
                    .mode_indicator
                    .as_ref()
                    .is_some_and(|indicator| indicator.caps_lock);
                if icon_only {
                    draw_mode_indicator(
                        &painter,
                        &content.layout,
                        &graphics.indicator,
                        ascii,
                        caps_lock,
                    );
                } else {
                    // 2026-11-02 02:3x **修**：原来只要 `mode_indicator` 有值就整块改写，
                    // 组字中切中英时**只画了那个小徽标、候选一个都没画** ——
                    // 用户看到的就是"候选窗整个不见了"（不是崩，是压根没画）。
                    // 现在：正常内容照画，徽标只是叠在上面的一层。
                    let empty = empty_context(&content.snapshot);
                    // 一帧画坏**不再上抛**。上抛会被渲染器当成致命错误
                    // （UI 循环退出：渲染失败 灾难性故障 0x8000FFFF）。这里只记日志、丢掉这一帧。
                    if let Err(message) = painter.draw(&content.layout, empty) {
                        crate::diagnostics::record(format_args!("绘制失败（丢弃本帧）: {message}"));
                        frame_failed = true;
                    }
                    if indicator {
                        // 组字中切中英（打一半拼音按 Shift）：候选窗不会消失，
                        // 所以在预编辑文字后面挂一个小徽标「中 / 英」。
                        draw_mode_badge(&painter, &content.layout, &graphics.indicator, ascii, caps_lock);
                    }
                }
            }
            if let Err(error) = draw.finish() {
                // 同样的道理：EndDraw 失败（D2DERR_WRONG_STATE）只丢这一帧，
                // 并把呈现层丢掉，下一帧 ensure_target 重建。别再让渲染器整个退出。
                crate::diagnostics::record(format_args!("呈现失败（丢弃本帧、下帧重建目标）: {error}"));
                frame_failed = true;
            }
        }
        drop(app);
        if frame_failed {
            // 触碰呈现层放在 `app` 借用之外：重建是下一帧 ensure_target 的事。
            self.app.borrow_mut().graphics.target = None;
        }
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
                    // 2026-11-07：锚点无效（重启后从未打过字的首键方案選單/候选窗）
                    // 不再整体放弃定位——那会让窗口停在旧位置甚至从不显示，
                    // 看起来像"按键直接过去了"。回退到主屏工作区居中（与预览同源），
                    // 等真实锚点/布局到达后自然重定位。
                    match content.snapshot.anchor.as_ref().filter(|a| a.valid) {
                        Some(anchor) => {
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
                            Some((placed.x, placed.y, width, height, Some(placed.sticky)))
                        }
                        None => {
                            let (x, y) = crate::presentation::preview_position(width, height);
                            Some((x, y, width, height, None))
                        }
                    }
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
        // 把窗口物理裁成圆角矩形，彻底消除「圆角外侧还有方角」。
        // `DWMWA_WINDOW_CORNER_PREFERENCE` 在无边框 popup 上常被忽略（见 `new()`），
        // 光靠它窗口仍是方的、四角露出方块；`SetWindowRgn` 直接裁窗口区域，
        // 无论材质档、无论 DWM 是否配合，方角都不会再出现。
        // 尺寸用 `SetWindowPos` 设的全窗尺寸（含 WS_THICKFRAME 边框），不能用
        // `GetClientRect`（那只量客户区，会漏掉边框那圈方角）。半径 = 面板圆角
        // **加上 offset**：面板是内缩 offset 画的，不补上会把面板自己的角也裁掉
        // （同心圆问题）。窗口每 resize（每次 `position`）就重裁一次。
        let (w_px, h_px, rad) = {
            let app = self.app.borrow();
            let Some(content) = app.content.as_ref() else {
                return Ok(());
            };
            let dpi = self.dpi.get();
            let w = pixels(content.layout.content_size.cx, dpi);
            let h = pixels(content.layout.content_size.cy, dpi);
            // 面板把 `panel_radius()` 当 D2D 坐标用（D2D 目标已 SetDpi），所以面板角的
            // 设备像素半径 = `pixels(panel_radius(), dpi)`。窗口角半径一致取这个值，
            // 再叠加 offset 的像素——这样窗口圆角与面板圆角**同心同半径**外扩 offset，
            // 面板角正好嵌在里面，offset 环里的阴影也跟着被圆角裁掉，方角不再出现。
            let rad = (pixels(app.metrics.panel_radius(), dpi) as f32
                + pixels(app.metrics.offset().0, dpi) as f32)
                .max(0.0) as i32;
            (w, h, rad)
        };
        self.apply_round_region(w_px, h_px, rad);
        Ok(())
    }

    /// 用圆角矩形区域裁切窗口，覆盖掉矩形窗口本身的四个直角。
    /// `w`/`h` 是 `SetWindowPos` 设的全窗尺寸（设备像素），`radius` 已是设备像素。
    fn apply_round_region(&self, w: i32, h: i32, radius: i32) {
        unsafe {
            // `CreateRoundRectRgn` 的椭圆参数是**直径**，乘 2 才等价于圆角半径。
            let hrgn = CreateRoundRectRgn(0, 0, w, h, radius * 2, radius * 2);
            if hrgn != 0 {
                let _ = SetWindowRgn(self.hwnd.get(), hrgn, 1);
            }
        }
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
/// 4. 仅当「光标下方容不下整窗」时才翻到插入点上方 6 像素；只要下方放得下就落回下方，
///    不再让 sticky 永久锁死在上方（否则移到顶部输入框时窗口会卡在屏幕顶、盖住输入框）；
/// 5. 最后再夹一次上边界。
///
/// 返回的 `sticky` 只反映本帧是否真的翻到了上方，写回窗口状态供下一帧参考。
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
    // 只在「光标下方放不下整窗」时翻到上方；否则一律落回光标下方，
    // 不让 sticky 永久锁死在上方（否则移到顶部输入框时窗口会卡在屏幕顶、盖住输入框）。
    let flip = y + height > work_bottom;
    let mut sticky = flip;
    if flip {
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

/// 组字中的中/英提示：预编辑文字后面一个**小圆角徽标**（「中」/「英」）。
///
/// 为什么不复用 [`draw_mode_indicator`]：那个是"候选窗只剩一个图标"的整窗模式，
/// 组字时候选窗里还有候选，只能用一小块地方表达。
fn draw_mode_badge(
    painter: &Painter<'_>,
    layout: &Layout,
    format: &IDWriteTextFormat,
    ascii: bool,
    caps_lock: bool,
) {
    // 参考高度：有预编辑行就用预编辑行高，否则用候选行高。
    let row = if !layout.preedit.is_null() {
        layout.preedit
    } else if let Some(first) = layout.rows.first() {
        first.rect
    } else {
        layout.content
    };
    let pad = painter.m.hilite_padding_y.max(2.0);
    let size = (row.height() - pad * 0.5).max(12.0);
    let width = size * 1.35;
    // 位置：紧跟在预编辑文字后面；靠到面板右边界就往回收。
    let gap = size * 0.35;
    let right = (row.right + gap + width).min(layout.content.right - pad);
    let left = (right - width).max(layout.content.left + pad);
    let top = row.top + (row.height() - size) * 0.5;
    let rect = Rect::new(left, top, right, top + size);
    // 底：主题底色压 75% 不透明，保证在任何背景上都看得清；圆角取小框圆角。
    let back = (0xC0u32 << 24) | (painter.scheme.back_color & 0x00FF_FFFF);
    painter.highlight_rect(
        &rect,
        painter.m.round_corner.min(size * 0.5),
        painter.m.border,
        back,
        0,
        painter.scheme.border_color,
        &layout::RoundInfo::default(),
    );
    let text = if caps_lock { "A" } else if ascii { "英" } else { "中" };
    let wide: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        if let Ok(text_layout) = painter
            .write
            .CreateTextLayout(&wide, format, rect.width(), rect.height())
        {
            let _ = text_layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = text_layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = painter.brush.SetColor(&d2d_color(painter.scheme.text_color));
            painter.target.DrawTextLayout(
                windows_numerics::Vector2 {
                    x: rect.left,
                    y: rect.top,
                },
                &text_layout,
                painter.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
        }
    }
    crate::diagnostics::record(format_args!("模式提示: 组字中徽标 {}", if caps_lock { "A(大写)" } else if ascii { "英" } else { "中" }));
}

/// 模式提示（官方没有这个窗口：它用状态图标；我们按同一套配色画一个小提示）。
fn draw_mode_indicator(
    painter: &Painter<'_>,
    layout: &Layout,
    format: &IDWriteTextFormat,
    ascii: bool,
    caps_lock: bool,
) {
    // 和候选窗同款的圆角底 + 描边（用户要的"边框"）：内容就是那个方形面板
    // 圆角：**面板边长 × 0.125**（用户要求）。
    // 提示窗是图标大小的方窗，跟主题的固定 4px 不成比例，按边长取比例更顺眼。
    let indicator_radius = layout.content.width().max(1.0) * 0.125;
    let back_color = if painter.glass {
        paint::glassify(painter.scheme.back_color)
    } else {
        painter.scheme.back_color
    };
    painter.highlight_rect(
        &layout.content,
        indicator_radius,
        painter.m.border,
        back_color,
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
    // 大写锁定点亮时画一个「A」，而不是中/英图标（用户 2026-11-02 03:0x：
    // 「按大写的时候应该是一个大写 A 的图标」）。判据来自**引擎**（`mode_indicator.caps_lock`）：
    // 主题进程不接收键盘输入，自己 `GetKeyState(VK_CAPITAL)` 读不到锁存状态。
    if ascii && caps_lock {
        let size = (layout.content.height() * 0.62).max(10.0);
        let Ok(format) = Graphics::make_format(
            &painter.write,
            "Segoe UI",
            size,
            DWRITE_WORD_WRAPPING_NO_WRAP,
            &painter.m,
        ) else {
            return;
        };
        let wide: Vec<u16> = "A".encode_utf16().collect();
        unsafe {
            if let Ok(text_layout) = painter.write.CreateTextLayout(
                &wide,
                &format,
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
        crate::diagnostics::record(format_args!(
            "模式提示: 大写 A {:.1}pt · 面板 {:.1}x{:.1} DIP",
            size, layout.content_size.cx, layout.content_size.cy
        ));
        return;
    }
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

/// 这一帧是不是「上屏后联想条」：**无预编辑**，非空，且整屏 items 都是 `kind == "prediction"`。
///
/// 联想条**不是**候选列表 —— 引擎只是为了按官方候选画法给第一个词画个高亮，
/// 才把 `selected_index` 写死成 0（见 `renderer_bridge::render_snapshot`），
/// 它没有"用户当前选中项"的语义。
///
/// 保险（2026-11-08）：打字时 librime 追加的预测候选也标 `kind == "prediction"`
/// （comment == "预测"），但那一帧**带 preedit**（组字中）——不能按联想条处理，
/// 否则编码栏/点击语义会被一起收掉。所以「整屏 prediction」的判定必须再加
/// `preedit.is_none()`，与 `layout::Layout::build` 的 `show_bar` 同口径。
fn is_prediction_strip(snapshot: &CandidateView) -> bool {
    snapshot.preedit.is_none()
        && !snapshot.items.is_empty()
        && snapshot
            .items
            .iter()
            .all(|item| item.kind == PREDICTION_KIND)
}

/// 鼠标抬起时是否派发 `ItemInvoked(index)`。
///
/// 官方 `WeaselPanel::OnLeftClickDown`（WeaselPanel.cpp:1145-1152）在**按下**时就把
/// 非高亮候选改成高亮（`m_uiCallback(NULL, &i, ...)`），抬起再按高亮项回调；
/// 我们的 `UiAction` 没有"改选中"这个动作，于是普通候选只能点**当前高亮**那一项
/// （等价于官方在 `hover_type = none` 下的行为）。
///
/// 联想条必须例外：用户要的是"想点哪个词就上屏哪个"（2026-11-08 需求「可点」），
/// 而联想条永远只有第一个词带高亮。所以：整屏都是 prediction 时，按下/抬起落在
/// **同一项**就派发，不再要求它是高亮项。普通候选的判定一个像素不动。
fn release_invocation(
    pressed: Option<Hit>,
    released: Option<Hit>,
    selected: Option<usize>,
    prediction_strip: bool,
) -> Option<u32> {
    let (Some(Hit::Candidate(pressed_index)), Some(Hit::Candidate(released_index))) =
        (pressed, released)
    else {
        return None;
    };
    if pressed_index != released_index {
        return None;
    }
    if prediction_strip || selected == Some(pressed_index) {
        Some(pressed_index as u32)
    } else {
        None
    }
}

/// 候选窗的屏幕矩形 (left, top, right, bottom)，面板停靠的锚点；取不到给 `None`。
fn cand_anchor(window: &Window) -> Option<(i32, i32, i32, i32)> {
    let mut r = RECT::default();
    if unsafe { GetWindowRect(window.hwnd.get(), &mut r) }.as_bool() {
        Some((r.left, r.top, r.right, r.bottom))
    } else {
        None
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match catch_unwind(AssertUnwindSafe(|| unsafe { dispatch(hwnd, msg, wp, lp) })) {
        Ok(result) => result,
        Err(e) => {
            // 2026-11-07 三按钮排查:panic 不再无声——把载荷写进 theme.log。
            let what = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&'static str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "非字符串 panic".into());
            crate::diagnostics::record(format_args!(
                "[panic] msg={msg:#x} wp={:#x} lp={:#x}: {what}",
                wp.0,
                lp.0
            ));
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
        // 2026-11-07 符号面板直拟上屏：面板把 `Box<String>` 指针 PostMessage 过来，
        // 这里取走文本、经 EventSink 发 `CommitText` 给宿主 → 引擎 `commit_string`。
        // 放在 match 之前是自定义消息（WM_APP 段），不参与下面的官方消息分发。
        if msg as u32 == crate::panels::WM_APP_COMMIT_TEXT as u32 {
            let text = unsafe { Box::from_raw(lp.0 as *mut String) };
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Window;
            if let Some(window) = unsafe { ptr.as_ref() } {
                let app = window.app.borrow();
                // 面板常驻后候选窗可能已经收起（content=None）：用最近一次渲染留下的
                // EventSink 直拟上屏。没有 sink 才退回复制（真·无活动会话，例如预览）。
                let events = app
                    .last_events
                    .clone()
                    .or_else(|| app.content.as_ref().map(|c| c.events.clone()));
                drop(app);
                crate::diagnostics::record(format_args!(
                    "[commit] 转发 {} 字符",
                    text.chars().count()
                ));
                if let Some(events) = events {
                    events.send(UiAction::CommitText(*text));
                } else {
                    // 没有可用事件通道（预览/服务刚退出）：退回复制，至少不丢字。
                    // 2026-11-07 加诊断：走这条路 = **没上屏**，日志里必须能直接看出来。
                    crate::diagnostics::record(format_args!(
                        "[commit] 无可用事件通道 → 复制到剪贴板（未上屏）"
                    ));
                    crate::panels::copy_text_to_clipboard(&text);
                }
            }
            return LRESULT(0);
        }
        // 2026-11-07 三按钮「按不了」修复记录（别再把这套临时补丁加回来）：
        //   这里原有一段「点击族消息直通」补丁（`[in]`/`[direct]` 诊断 + 0x201 直接
        //   `panels::open_*`）。它存在的唯一原因，是当时 `WM_NCCALCSIZE` 不在构建脚本
        //   白名单里（见 `theme/support/build_d2d.rs`），导致下面 match 的
        //   `WM_NCCALCSIZE if wp.0 != 0` 臂被编译成「**任何** wParam != 0 的消息一律
        //   return 0」的通配臂 —— WM_LBUTTONDOWN 的 wParam 恒含 MK_LBUTTON(1)，于是
        //   永远到不了 match 里的按钮分支，点三按钮毫无反应。
        //   白名单补上后该臂恢复成真正的常量模式，点击走下面 match 的 WM_LBUTTONDOWN 臂；
        //   若保留直通块，同一次点击会 open_* 两次 ⇒ 用户看到的是「点了没反应」
        //   （旧 open 语义=同类再点收起；2026-11-07 22:3x 起改成「同类再点只置顶、
        //   永不销毁」，但直通块仍是重复打开，别加回来）。所以直通块整体删除。
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
                // 加了 WS_THICKFRAME 之后：客户区要铺满整个窗口，且不提供缩放/拖动边框。
                // ⚠️ `WM_NCCALCSIZE` 这个常量必须列在 `theme/support/build_d2d.rs` 的
                //   白名单里！漏了它编译器不会报错，而是把这条臂当成「变量绑定 + 守卫」的
                //   **通配臂**（只有 non_snake_case 警告）＝「任何 wParam != 0 的消息一律
                //   return 0」—— 2026-11-07「编码栏三按钮按不了」就是这么来的：鼠标左键
                //   按下 wParam 恒含 MK_LBUTTON(1)，在进 match 之前就被这条臂吞掉了。
                WM_NCCALCSIZE if wp.0 != 0 => return LRESULT(0),
                WM_NCHITTEST => return LRESULT(1), // HTCLIENT
                // 动画定时器：到点**直接画**一帧（不走 invalidate → WM_PAINT，
                // 那条路要等消息队列空下来，实测就是"滑动一顿一顿"的来源）。
                // 自排程的一帧：补齐帧间等待 → 画 → 还没到点就再投一帧。
                ANIM_MSG => {
                    let (running, wait_ms) = {
                        let mut slot = window.anim.borrow_mut();
                        match slot.as_mut() {
                            Some(a) => {
                                if a.start.elapsed().as_secs_f32() >= ANIM_DURATION {
                                    (false, 0u32)
                                } else {
                                    let next = a.last
                                        + std::time::Duration::from_millis(a.interval_ms as u64);
                                    let now = std::time::Instant::now();
                                    let wait = if next > now {
                                        ((next - now).as_millis() as u32).min(a.interval_ms)
                                    } else {
                                        0
                                    };
                                    a.last = now;
                                    a.frames += 1;
                                    (true, wait)
                                }
                            }
                            None => (false, 0),
                        }
                    };
                    if wait_ms > 0 {
                        Sleep(wait_ms);
                    }
                    let t_now = window.anim.borrow().as_ref().map(|a| {
                        (a.start.elapsed().as_secs_f32() / ANIM_DURATION).clamp(0.0, 1.0)
                    });
                    if let Some(t) = t_now {
                        crate::diagnostics::record_verbose(format_args!(
                            "动画帧 t={t:.2}（第 {} 帧）",
                            window.anim.borrow().as_ref().map(|a| a.frames).unwrap_or(0)
                        ));
                    }
                    let started = std::time::Instant::now();
                    if let Err(error) = window.paint() {
                        window.graphics_error(error);
                    }
                    let cost = started.elapsed().as_secs_f32() * 1000.0;
                    if cost > 8.0 {
                        crate::diagnostics::record(format_args!(
                            "动画帧绘制 {cost:.1}ms（>8ms 预警；帧预算按 DWM 节拍）"
                        ));
                    }
                    if running {
                        let over = window
                            .anim
                            .borrow()
                            .as_ref()
                            .map(|a| a.start.elapsed().as_secs_f32() >= ANIM_DURATION);
                        match over {
                            Some(true) => {
                                let frames =
                                    window.anim.borrow().as_ref().map(|a| a.frames).unwrap_or(0);
                                *window.anim.borrow_mut() = None;
                                timeEndPeriod(1);
                                crate::diagnostics::record_verbose(format_args!(
                                    "动画结束：共 {frames} 帧"
                                ));
                            }
                            Some(false) => post_anim_frame(hwnd),
                            None => {}
                        }
                    }
                    return LRESULT(0);
                }
                // 设备丢失重试 / 自动恢复 / 自动隐藏的窗口计时器。
                //
                // 2026-11-01：这三条原来排在 `0x0113 =>` 兜底分支**后面**，是死代码 ——
                // 任何 WM_TIMER 都会先被兜底分支吃掉。挪到前面才会真的被执行。
                0x0113 if wp.0 == RETRY_TIMER => {
                    let _ = KillTimer(Some(hwnd), RETRY_TIMER);
                    window.recovery.borrow_mut().waiting = false;
                    window.invalidate();
                    return LRESULT(0);
                }
                0x0113 if wp.0 == AUTOREV_TIMER => {
                    let _ = KillTimer(Some(hwnd), AUTOREV_TIMER);
                    window.invalidate();
                    return LRESULT(0);
                }
                0x0113 if wp.0 == HIDE_DEBOUNCE_TIMER => {
                    // 延迟隐藏到期：期间没有新的可见快照来撤销，才真的收窗口。
                    let _ = KillTimer(Some(hwnd), HIDE_DEBOUNCE_TIMER);
                    window.hide_now("debounced_hide");
                    return LRESULT(0);
                }
                0x0113 if wp.0 == AUTOHIDE_TIMER => {
                    let _ = KillTimer(Some(hwnd), AUTOHIDE_TIMER);
                    // 2026-11-02 02:4x：提示分两种。**纯提示窗**（没有候选）到期整窗隐藏；
                    // **组字中徽标**到期只能擦掉徽标那一层 —— 整窗 hide 会把还在组字的
                    // 候选窗一起收走（900ms 后候选突然消失，正是用户报过的现象）。
                    let badge_on_candidates = {
                        let mut app = window.app.borrow_mut();
                        match app.content.as_mut() {
                            Some(content) if !content.snapshot.items.is_empty() => {
                                content.snapshot.mode_indicator = None;
                                true
                            }
                            _ => false,
                        }
                    };
                    if badge_on_candidates {
                        window.invalidate();
                    } else {
                        window.hide("autohide_timer");
                    }
                    return LRESULT(0);
                }
                0x0113 => {
                    crate::diagnostics::record(format_args!(
                        "WM_TIMER 未知 id={} 线程={:?}",
                        wp.0,
                        std::thread::current().id()
                    ));
                    return DefWindowProcW(hwnd, msg, wp, lp);
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
                // 移动/尺寸变化后重抓背景（用户 11:5x 要的"跟着走"）。
                // 窗口**上屏之后也能抓**：`capture_backdrop` 会先把本窗口从画面里排除
                // （WDA_EXCLUDEFROMCAPTURE），抓到的只有背景 —— 17:4x「只识别背景」。
                (0x0003 | 0x0047) => { // WM_MOVE | WM_WINDOWPOSCHANGED（绑定里没有这两个常量）
                    if window.glass_self {
                        window.capture_backdrop();
                        window.invalidate();
                    }
                    return DefWindowProcW(hwnd, msg, wp, lp);
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
                    // 编码栏三按钮即使 `hover_type = none` 也要亮（它们是按钮，不是候选）。
                    app.hovered = if matches!(hit, Some(Hit::BarButton(_)))
                        || app.metrics.hover != config::Hover::None
                    {
                        hit
                    } else {
                        None
                    };
                    if changed {
                        crate::diagnostics::record_verbose(format_args!(
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
                    // 2026-11-07 排查:hit 前后各记一条,死在哪一半一眼可见。
                    crate::diagnostics::record(format_args!("[lb] 前 lp={:#x}", lp.0));
                    let hit = window.hit(lp);
                    crate::diagnostics::record(format_args!(
                        "[click] lp={:#x} hit={:?}",
                        lp.0, hit
                    ));
                    // 编码栏三按钮：打开对应面板（🔄 同步 / ✂ 剪贴板 / 🔣 符号），
                    // 不走候选按压流程、不抢焦点、不触发 SetCapture。
                    if let Some(Hit::BarButton(idx)) = hit {
                        crate::diagnostics::record(format_args!("[panel] click bar button {idx}"));
                        let horizontal = {
                            // 记下候选窗句柄（面板点符号要经它把文本送回输入流直拟上屏）
                            // 和当前主题配色（面板接入 theme，用户 2026-11-07）。
                            let app = window.app.borrow();
                            crate::panels::set_candidate_hwnd(
                                window.hwnd.get().0 as isize,
                                app.metrics.font_point,
                            );
                            crate::panels::set_theme_scheme(&app.scheme);
                            // 横流（挤压水平）时符号条右侧放不下要改贴同步面板右侧（用户 21:1x ④）。
                            app.metrics.layout_type == config::LayoutType::HorizontalSqueeze
                        };
                        let anchor = cand_anchor(window);
                        match idx {
                            0 => crate::panels::open_sync(anchor),
                            1 => crate::panels::open_clipboard(anchor),
                            _ => crate::panels::open_symbol(anchor, horizontal),
                        }
                        return LRESULT(0);
                    }
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
                    let (event, prediction_strip) = {
                        let mut app = window.app.borrow_mut();
                        let pressed = app.pressed.take();
                        let (selected, prediction_strip) = match app.content.as_ref() {
                            Some(content) => (
                                Some(content.snapshot.selected_index as usize),
                                is_prediction_strip(&content.snapshot),
                            ),
                            None => (None, false),
                        };
                        (
                            release_invocation(pressed, hit, selected, prediction_strip)
                                .map(UiAction::ItemInvoked),
                            prediction_strip,
                        )
                    };
                    window.cancel();
                    if let Some(action) = event {
                        if let UiAction::ItemInvoked(index) = &action {
                            // 2026-11-08：联想条点击以前只点得动高亮的第一项（官方候选口径），
                            // 2/3 号词点了没反应。留一行日志让真机 theme.log 能直接看出派发与否。
                            crate::diagnostics::record(format_args!(
                                "[click] 派发 ItemInvoked({index})（联想条={prediction_strip}）"
                            ));
                        }
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

/// 出厂皮肤主题的工厂入口。
pub struct Factory;

impl crate::theme_api::ThemeFactory for Factory {
    fn name(&self) -> &'static str {
        // 2026-11-02 用户定名：对外叫 **default**（不再叫 official）。
        // 这个 id 要跟 `weasel.yaml` 的 `theme:`、renderer 的 `supports_theme` 列表、
        // `themeSettings.<id>` 三处同名，改名必须一起改。
        "default"
    }
    /// 候选栏 + 预编辑（官方会把未行内显示的组字串画在候选窗里）+ 模式提示。
    ///
    /// `resident`（2026-11-07）：用户要求三辅助面板常驻。声明后渲染器在「输入上下文
    /// 仍活动但没有候选/预编辑」的快照上也会调用 `render`（而不是只调 `hide`），主题
    /// 借此拿到最新的 `EventSink` —— 面板在组字结束后点符号/卡片仍能直拟上屏。
    /// 主题自身仍按 `presentation::is_visible` 决定候选窗是否显示，行为不变。
    fn capabilities(&self) -> crate::theme_api::ThemeCapabilities {
        crate::theme_api::ThemeCapabilities {
            preedit: true,
            resident: true,
            mode_indicator: true,
        }
    }
    fn create(
        &self,
        mode: UiMode,
        settings: &weasel_common::settings::ConfigSnapshot,
    ) -> crate::theme_api::ThemeCreation {
        (|| -> Result<Box<dyn ThemeBackend>, String> {
            let _: serde_json::Map<String, serde_json::Value> = settings.theme_settings("default")?;
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

    // 2026-11-10 第二条：帧间隔按 DWM 合成刷新率算，替掉固定 8ms 帧循环。
    #[test]
    fn anim_interval_60hz_is_one_refresh_tick() {
        // 60Hz → 1000/60 = 16.67 → 17ms；100ms 动画从 12 帧降到 6 帧。
        assert_eq!(frame_interval_ms(60.0), 17);
    }

    #[test]
    fn anim_interval_120hz_keeps_high_refresh() {
        // 120Hz → 8ms，和高刷屏原来的固定 8ms 一致，不退化。
        assert_eq!(frame_interval_ms(120.0), 8);
    }

    #[test]
    fn anim_interval_144hz_clamps_to_floor() {
        // 144Hz → 6.94 → 7ms，被下限钳到 8ms（不追求 144FPS 过采样）。
        assert_eq!(frame_interval_ms(144.0), 8);
    }

    #[test]
    fn anim_interval_30hz_clamps_to_ceiling() {
        // 30Hz → 33ms，被上限钳到 20ms（避免 100ms 动画只剩 3 帧变幻灯片）。
        assert_eq!(frame_interval_ms(30.0), 20);
    }

    #[test]
    fn anim_interval_75hz_rounds_to_nearest_tick() {
        assert_eq!(frame_interval_ms(75.0), 13);
    }

    #[test]
    fn anim_interval_invalid_hz_falls_back_to_60hz() {
        // 远程桌面 / 无 DWM 时 DwmGetCompositionTimingInfo 失败 → 回退 16ms。
        assert_eq!(frame_interval_ms(0.0), 16);
        assert_eq!(frame_interval_ms(-60.0), 16);
        assert_eq!(frame_interval_ms(f32::NAN), 16);
        assert_eq!(frame_interval_ms(f32::INFINITY), 16);
        assert_eq!(frame_interval_ms(f32::NEG_INFINITY), 16);
    }

    fn metrics(shadow_radius: i32) -> Metrics {
        let mut style = UIStyle::default();
        style.layout.shadow_radius = shadow_radius;
        Metrics::new(&style, 96.0)
    }

    fn measure_key(format: MeasureFormat, text: &str, max_width: f32) -> MeasureKey {
        MeasureKey {
            format,
            text: text.to_owned(),
            max_width_bits: max_width.to_bits(),
        }
    }

    #[test]
    fn measure_cache_hits_same_text_and_keeps_formats_apart() {
        let cache = MeasureCache::default();
        let key = measure_key(MeasureFormat::Text, "中国", 400.0);
        assert_eq!(cache.get(&key), None, "冷缓存必须 miss");
        cache.put(key.clone(), Size::new(32.0, 20.0));
        assert_eq!(cache.get(&key), Some(Size::new(32.0, 20.0)), "同文本同格式命中");
        assert_eq!(cache.len(), 1);

        // 同一个字串用序号格式量，或横排/竖排 max_width 不同，都不能串味。
        assert_eq!(
            cache.get(&measure_key(MeasureFormat::Label, "中国", 400.0)),
            None
        );
        assert_eq!(
            cache.get(&measure_key(MeasureFormat::Text, "中国", 0.0)),
            None
        );
        assert_eq!(
            cache.get(&measure_key(MeasureFormat::Text, "中", 400.0)),
            None
        );
        cache.clear();
        assert_eq!(cache.get(&key), None, "换字体/DPI 后必须全部失效");
    }

    #[test]
    fn measure_cache_caps_growth_without_losing_existing_keys() {
        let cache = MeasureCache::default();
        for i in 0..MEASURE_CACHE_MAX {
            cache.put(
                measure_key(MeasureFormat::Text, &format!("词{i}"), 400.0),
                Size::new(i as f32, 1.0),
            );
        }
        assert_eq!(cache.len(), MEASURE_CACHE_MAX);
        // 重复 put 已有键不该触发清空。
        cache.put(
            measure_key(MeasureFormat::Text, "词0", 400.0),
            Size::new(99.0, 1.0),
        );
        assert_eq!(cache.len(), MEASURE_CACHE_MAX);
        assert_eq!(
            cache.get(&measure_key(MeasureFormat::Text, "词0", 400.0)),
            Some(Size::new(99.0, 1.0))
        );
        // 第 2049 个新键触发整份丢弃，缓存重新从 1 开始长。
        cache.put(
            measure_key(MeasureFormat::Text, "新词", 400.0),
            Size::new(7.0, 1.0),
        );
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache.get(&measure_key(MeasureFormat::Text, "词5", 400.0)),
            None
        );
    }

    #[test]
    fn layout_reuse_requires_both_same_content_and_same_key() {
        let old = LayoutKey {
            generation: 3,
            bar_at_bottom: true,
        };
        assert!(reusable_layout(true, old, old), "内容没变、参数没变 → 复用");
        assert!(
            !reusable_layout(false, old, old),
            "候选内容变了必须重新测量"
        );
        assert!(
            !reusable_layout(
                true,
                old,
                LayoutKey {
                    generation: 4,
                    bar_at_bottom: true
                }
            ),
            "reload 换字体后必须重新测量"
        );
        assert!(
            !reusable_layout(
                true,
                old,
                LayoutKey {
                    generation: 3,
                    bar_at_bottom: false
                }
            ),
            "bar_position:auto 翻到光标上方后必须重建布局"
        );
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
        assert!(placed.sticky, "下方放不下时要翻到上方并标 sticky");
        // 插入点回到下方有空间的位置 → 不再被旧的 sticky 锁死在上方，
        // 而是落回光标下方（top + 6），这样顶部输入框就不会被盖住。
        let again = reposition(&anchor(100, 200, 104, 220), work, &m, 300, 200, 96, true);
        assert_eq!(again.y, 220 + 6, "有空间就落回光标下方，不再卡在屏幕顶");
        assert!(!again.sticky);
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

    /// 鼠标抬起派发：普通候选仍只认高亮项（官方候选口径），联想条点哪项都派发。
    ///
    /// 2026-11-08 用户要「联想条可点」：联想条的 `selected_index` 恒为 0，若沿用
    /// 候选口径，2/3 号词点了没反应。正负对照都在这里。
    #[test]
    fn release_invokes_highlighted_candidate_but_any_prediction_item() {
        // 普通候选：正对照——点高亮项派发。
        assert_eq!(
            release_invocation(Some(Hit::Candidate(0)), Some(Hit::Candidate(0)), Some(0), false),
            Some(0)
        );
        // 普通候选：负对照——点非高亮项不派发（官方 hover_type=none 的口径）。
        assert_eq!(
            release_invocation(Some(Hit::Candidate(2)), Some(Hit::Candidate(2)), Some(0), false),
            None
        );
        // 联想条：2/3 号词不是高亮项，也必须派发。
        assert_eq!(
            release_invocation(Some(Hit::Candidate(1)), Some(Hit::Candidate(1)), Some(0), true),
            Some(1)
        );
        assert_eq!(
            release_invocation(Some(Hit::Candidate(2)), Some(Hit::Candidate(2)), Some(0), true),
            Some(2)
        );
        // 按下/抬起不在同一项（拖动）不派发。
        assert_eq!(
            release_invocation(Some(Hit::Candidate(1)), Some(Hit::Candidate(2)), Some(1), true),
            None
        );
        // 翻页箭头 / 编码栏按钮 / 空白不派发。
        assert_eq!(
            release_invocation(Some(Hit::Previous), Some(Hit::Previous), Some(0), true),
            None
        );
        assert_eq!(
            release_invocation(Some(Hit::BarButton(0)), Some(Hit::BarButton(0)), Some(0), true),
            None
        );
        assert_eq!(release_invocation(None, None, Some(0), true), None);
        // 候选窗已隐藏（拿不到 selected）时：普通候选不派发，联想条仍按同一项派发。
        assert_eq!(
            release_invocation(Some(Hit::Candidate(1)), Some(Hit::Candidate(1)), None, false),
            None
        );
        assert_eq!(
            release_invocation(Some(Hit::Candidate(1)), Some(Hit::Candidate(1)), None, true),
            Some(1)
        );
    }

    /// 2026-11-08 用户口径：「预测」这个标签做成小一号 = 候选字号的 1/2
    /// （而**不是**把预测候选的正文文字缩小 —— `make_formats` 里 `text` 格式仍按
    /// `m.font_point` 建，预编辑/候选正文字号一个像素不动）。
    #[test]
    fn prediction_tag_is_half_of_candidate_font_size() {
        let m = metrics(0);
        assert!(m.font_point > 0.0, "默认 style 字高应 > 0");
        let tag = prediction_tag_point(&m);
        assert!(
            (tag - m.font_point / 2.0).abs() < 0.001,
            "预测标签字号应为候选字号的一半：{tag} vs {}",
            m.font_point
        );
        // 负对照：默认 style 的注释字号 == 候选字号，标签必须明显更小。
        assert!(
            tag < m.comment_font_point,
            "预测标签必须比候选注释小：{tag} vs {}",
            m.comment_font_point
        );
    }

    /// 联想条识别：非空且**每一项**都是 prediction；空/混合/普通候选/旧版无 kind 都不算。
    #[test]
    fn prediction_strip_requires_every_item_to_be_prediction() {
        fn view(kinds: &[&str]) -> CandidateView {
            CandidateView {
                items: kinds
                    .iter()
                    .map(|kind| crate::theme_api::CandidateItem {
                        primary_text: "词".into(),
                        secondary_text: String::new(),
                        enabled: true,
                        kind: (*kind).into(),
                    })
                    .collect(),
                selected_index: 0,
                ..Default::default()
            }
        }
        assert!(
            is_prediction_strip(&view(&["prediction", "prediction", "prediction"])),
            "三条 prediction 就是联想条"
        );
        assert!(!is_prediction_strip(&view(&[])), "空快照不是联想条");
        assert!(
            !is_prediction_strip(&view(&["prediction", "candidate"])),
            "混合快照不是联想条"
        );
        assert!(!is_prediction_strip(&view(&["candidate"])), "普通候选不是联想条");
        assert!(
            !is_prediction_strip(&view(&[""])),
            "旧渲染器缺 kind（空串）按普通候选处理"
        );
        // 负（2026-11-08 保险）：打字时 librime 追加的预测候选帧也是整屏
        // kind=prediction，但它**带预编辑**（组字中），不能按联想条收掉编码栏/
        // 点击语义。正对照：同样的 items 去掉 preedit ⇒ 是联想条。
        let mut composing = view(&["prediction", "prediction", "prediction"]);
        composing.preedit = Some(crate::theme_api::Preedit {
            text: "nihao".into(),
            cursor: 5,
        });
        assert!(
            !is_prediction_strip(&composing),
            "带预编辑的整屏 prediction 是打字帧，不是上屏后联想条"
        );
    }
}

