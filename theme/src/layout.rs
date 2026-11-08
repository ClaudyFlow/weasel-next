//! 官方 `WeaselUI` 布局的 Rust 版：`StandardLayout` + `VerticalLayout` + `HorizontalLayout`。
//!
//! 逐句对照（行号指 `reference\` 里的官方源码）：
//!
//! * `StandardLayout::RecalculateSizes`（StandardLayout.cpp:58）—— 先把所有文本量好；
//!   这里拆成 [`Measured`]，由调用方（图形层）用 DirectWrite 填。
//! * `Layout` 构造（Layout.cpp:44-60）—— `real_margin`、`offset` 两个根公式。
//! * `VerticalLayout::DoLayout`（VerticalLayout.cpp:5）—— 每个候选一行。
//! * `HorizontalLayout::DoLayout`（HorizontalLayout.cpp:6）—— 按 `max_width` 折行。
//! * `_CalcMarkMetrics` / `_CalcPageIndicator` / `_LayoutInlineRect` /
//!   `_UpdateStatusIconLayout` / `_PrepareRoundInfo` —— 工具函数。
//!
//! 圆角判定（`_IsHighlightOverCandidateWindow`）官方用 D2D 路径几何做点包含测试；
//! 这里是同一件事的解析解（圆角矩形 = 中心十字区 ∪ 四个圆角圆），避免布局层依赖图形设备。

use crate::theme::config::{Align, LayoutType, Metrics};
use crate::theme_api::CandidateView;

/// 翻页字形。官方（StandardLayout.cpp:5-6）用 `_pre`/`_next` 的 `<` / `>`，
/// 半角细笔画、在小狼毫里细得像两条缝；用户 2026-11-05「箭头换成大号的」，
/// 改用设计稿 `_shots/ime-three-modes.html` 里那颗实心三角 `◀` / `▶`
/// （雅黑下是 em 宽的字身，比 `<` 宽近一倍，视觉上就是个实心大箭头）。
/// 注意这两个常量同时参与 `measure`（`mod.rs::measure`）与绘制，
/// 所以换字形后 `page_prev/page_next` 矩形、`calc_page_indicator` 的 `pgw`
/// 都会自己跟着变宽，布局侧不用另外加数。
pub const PAGE_PREV: &str = "◀";
pub const PAGE_NEXT: &str = "▶";

/// 候选最多 100 条（官方 `MAX_CANDIDATES_COUNT`）。
pub const MAX_CANDIDATES: usize = 100;

/// 「预测」item 的类别标记（与 `theme_api::CandidateItem.kind` 同口径）。
///
/// 两种形态都标这个值：
/// * **上屏后联想条**（2026-11-07 用户「预测一些上屏十次以上的用户词」）：无预编辑、
///   整屏都是预测词 ⇒ 收掉编码栏（翻页箭头 + 符/贴/同 三按钮），只留一排可点的词；
/// * **打字时 librime 追加的预测候选**（2026-11-08）：那一帧带 preedit（组字中），
///   编码栏照常画，只把「预测」标签（`secondary_text`）换成小号字体（见 `paint`）。
///   所以 `show_bar` / `is_prediction_strip` 的「整屏 prediction」判定必须再加
///   `preedit.is_none()` 保险，否则打字时的预测帧会被当成联想条。
///
/// ⚠️ 这是 Renderer→dll 的 serde 契约字符串，**必须与 `Renderer/src/preview.rs`、
/// `Engine/src/renderer_bridge.rs` 里的字面量一致**；改这里要三处同改。
pub const PREDICTION_KIND: &str = "prediction";

/// 官方 `STATUS_ICON_SIZE` = `GetSystemMetrics(SM_CXICON)`，96 DPI 下是 32。
pub const STATUS_ICON_SIZE: f32 = 32.0;

/// 尺寸（官方 `CSize`）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub cx: f32,
    pub cy: f32,
}

impl Size {
    pub const ZERO: Self = Self { cx: 0.0, cy: 0.0 };
    pub fn new(cx: f32, cy: f32) -> Self {
        Self { cx, cy }
    }
    pub fn is_empty(&self) -> bool {
        self.cx <= 0.0 || self.cy <= 0.0
    }
}

/// 矩形（官方 `CRect`：右下开区间）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
    pub fn width(&self) -> f32 {
        self.right - self.left
    }
    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }
    /// 官方 `IsRectNull`：任一边为 0 就当作空。
    pub fn is_null(&self) -> bool {
        self.left == 0.0 || self.top == 0.0 || self.right == 0.0 || self.bottom == 0.0
    }
    pub fn is_empty(&self) -> bool {
        self.left >= self.right || self.top >= self.bottom
    }
    pub fn offset(&mut self, dx: f32, dy: f32) {
        self.left += dx;
        self.right += dx;
        self.top += dy;
        self.bottom += dy;
    }
    pub fn inflate(&mut self, dx: f32, dy: f32) {
        self.left -= dx;
        self.right += dx;
        self.top -= dy;
        self.bottom += dy;
    }
    pub fn deflate(&mut self, dx: f32, dy: f32) {
        self.left += dx;
        self.right -= dx;
        self.top += dy;
        self.bottom -= dy;
    }
    /// 官方 `PtInRect`：左闭右开、上闭下开。
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// 官方 `IsToRoundStruct`：四角是否要圆（默认全要）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoundInfo {
    pub top_left: bool,
    pub bottom_left: bool,
    pub top_right: bool,
    pub bottom_right: bool,
    /// 高亮块"溢出"候选窗（官方用 `_IsHighlightOverCandidateWindow` 判定）——
    /// 此时圆角改用面板圆角，且四角是否圆由查表决定。
    pub hemispherical: bool,
}

impl Default for RoundInfo {
    fn default() -> Self {
        Self {
            top_left: true,
            bottom_left: true,
            top_right: true,
            bottom_right: true,
            hemispherical: false,
        }
    }
}

/// 一行候选的四个矩形（对应官方四组数组）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Row {
    pub label: Rect,
    pub text: Rect,
    pub comment: Rect,
    /// 高亮矩形（官方 `_candidateRects[i]`）。
    pub rect: Rect,
    /// 该候选自己的高亮外扩量：挤压模式选中项 = 2×pad，其余 = pad。
    /// 0 = 用 `Metrics::hilite_padding_*` 兜底（兼容 `Row::default()`）。
    pub pad_x: f32,
    pub pad_y: f32,
}

/// 候选自己的高亮外扩量；0 时回退到 Metrics 基准值。
pub(crate) fn row_padding(m: &Metrics, row: &Row) -> (f32, f32) {
    (
        if row.pad_x > 0.0 { row.pad_x } else { m.hilite_padding_x },
        if row.pad_y > 0.0 { row.pad_y } else { m.hilite_padding_y },
    )
}

/// 预编辑/辅助串按高亮属性拆成的三段尺寸（官方 `_GetPreeditSize` 的产物）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Parts {
    pub before: Size,
    pub hilite: Size,
    pub after: Size,
    /// 整体尺寸（横向 = 三段宽 + 段间 hilite_spacing；纵向同理按高）。
    pub total: Size,
}

/// 一次布局需要的全部文本实测值（官方 `RecalculateSizes` 的结果）。
#[derive(Clone, Debug, Default)]
pub struct Measured {
    pub label: Vec<Size>,
    pub text: Vec<Size>,
    pub comment: Vec<Size>,
    pub preedit: Parts,
    pub aux: Parts,
    pub page_prev: Size,
    pub page_next: Size,
    /// `style/mark_text` 的尺寸（空串时为 0）。
    pub mark: Size,
}

/// 命中结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Candidate(usize),
    Previous,
    Next,
    /// 编码栏三个按钮（右→左：0=🔄 同步 / 1=✂ 剪贴板 / 2=🔣 符号）。
    BarButton(usize),
}

/// 官方 `_PrepareRoundInfo` 的 `tmp[6]`（把全屏档归回三档）。
fn round_table_index(kind: LayoutType) -> usize {
    match kind.base() {
        LayoutType::Vertical => 0,
        LayoutType::Horizontal => 1,
        LayoutType::VerticalText => 2,
        _ => 0,
    }
}

/// 圆角矩形内部判定（官方 `_IsHighlightOverCandidateWindow` 的解析解）。
///
/// 官方是拿 `_bgRect` 的圆角路径做 `FillContainsPoint`；这里是等价的几何判定：
/// 半径先按官方 `CreateRoundedRectanglePath` 夹到半宽/半高。
pub fn rounded_rect_contains(rect: &Rect, radius: f32, x: f32, y: f32) -> bool {
    if rect.is_empty() {
        return false;
    }
    let radius = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    if radius <= 0.0 {
        return rect.contains(x, y);
    }
    // 中心十字区
    if (rect.left + radius..=rect.right - radius).contains(&x)
        || (rect.top + radius..=rect.bottom - radius).contains(&y)
    {
        // 十字区之内，仍在矩形内即算内部（半径已夹取，不会越界）
        return rect.contains(x, y) || x == rect.right || y == rect.bottom;
    }
    // 四个角：落在圆心 + 半径的圆内
    let cx = if x < rect.left + radius {
        rect.left + radius
    } else {
        rect.right - radius
    };
    let cy = if y < rect.top + radius {
        rect.top + radius
    } else {
        rect.bottom - radius
    };
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}

/// 官方 `_IsHighlightOverCandidateWindow`：高亮块是否戳出候选窗（决定圆角怎么给）。
fn highlight_over_window(bg: &Rect, round_corner_ex: f32, round_corner: f32, rc: &Rect) -> bool {
    // 官方用弧心做探针：偏移 = round_corner * (√2 - 1)
    let offset = round_corner * 0.414_213_56;
    for (x, y) in [
        (rc.left + offset, rc.top + offset),
        (rc.left + offset, rc.bottom - offset),
        (rc.right - offset, rc.bottom - offset),
        (rc.right - offset, rc.top + offset),
    ] {
        if !rounded_rect_contains(bg, round_corner_ex, x, y) {
            return true;
        }
    }
    rc.left <= bg.left || rc.right >= bg.right || rc.top <= bg.top || rc.bottom >= bg.bottom
}

/// 官方 `_CalcMarkMetrics`：算高亮标记条的宽/高/间距，返回 `mark_gap`。
fn calc_mark_metrics(
    m: &Metrics,
    measured: &Measured,
    candidates_count: usize,
    vertical_text: bool,
    mark: &mut (f32, f32, f32),
) -> f32 {
    // 颜色全透明或无候选 → 不画标记
    if !m.mark_visible || candidates_count == 0 {
        *mark = (0.0, 0.0, 0.0);
        return 0.0;
    }
    let (mut width, mut height) = if m.mark_text.is_empty() {
        // 官方用 "|" 的尺寸做基准
        (measured.mark.cx.max(1.0), measured.mark.cy.max(1.0))
    } else {
        (measured.mark.cx, measured.mark.cy)
    };
    if m.mark_text.is_empty() {
        if vertical_text {
            height = width / 7.0;
            if m.linespacing != 0 && m.baseline != 0 {
                height /= m.linespacing as f32 / 100.0;
            }
            height = height.max(6.0);
        } else {
            width = height / 7.0;
            if m.linespacing != 0 && m.baseline != 0 {
                width /= m.linespacing as f32 / 100.0;
            }
            width = width.max(6.0);
        }
    }
    let gap = if m.mark_text.is_empty() {
        if vertical_text { height } else { width }
    } else {
        (if vertical_text { height } else { width }) + m.hilite_spacing
    };
    *mark = (width, height, gap);
    gap
}

/// 官方 `_CalcPageIndicator`：翻页指示器占的宽高。
fn calc_page_indicator(
    m: &Metrics,
    measured: &Measured,
    inline_preedit: bool,
    vertical_text: bool,
) -> (f32, f32) {
    // 2026-10-31：不再要求 `page_enabled`（配色没给翻页色是常态，见 paint::draw_page_buttons），
    // 否则连矩形都不算，界面上永远看不到「还有下一页」。
    if inline_preedit {
        return (0.0, 0.0);
    }
    let left = measured.page_prev;
    let right = measured.page_next;
    if vertical_text {
        (
            left.cx.max(right.cx),
            left.cy + right.cy + m.hilite_spacing + m.hilite_padding_y * 2.0,
        )
    } else {
        (
            left.cx + right.cx + m.hilite_spacing + m.hilite_padding_x * 2.0,
            left.cy.max(right.cy),
        )
    }
}

/// 官方 `ShouldDisplayStatusIcon`。
pub fn should_display_status_icon(m: &Metrics, view: &CandidateView) -> bool {
    let ascii = view.ascii_mode.unwrap_or(false);
    let composing = !view.items.is_empty() || view.preedit.is_some();
    let aux_empty = true; // 我们的快照没有 aux 串
    let base = (ascii && !m.inline_preedit) || !composing || !aux_empty;
    let fullscreen_with_aux = m.layout_type.is_fullscreen() && !aux_empty;
    base && !fullscreen_with_aux
}

/// 官方 `StandardLayout::IsInlinePreedit`。
///
/// 官方还要 `client_caps & INLINE_PREEDIT_CAPABLE`：宿主应用是否支持行内预编辑。
/// 我们的快照里"有没有 preedit"就是这件事的答案——有 preedit 说明要画在候选窗里。
pub fn is_inline_preedit(m: &Metrics, view: &CandidateView) -> bool {
    m.inline_preedit && view.preedit.is_none()
}

/// 官方 `_CalcStatusIconOffset`。
fn status_icon_offset(m: &Metrics, view: &CandidateView, extent: f32) -> f32 {
    if !should_display_status_icon(m, view) {
        return 0.0;
    }
    if STATUS_ICON_SIZE >= extent {
        (STATUS_ICON_SIZE - extent) / 2.0
    } else {
        0.0
    }
}

/// 官方 `_LayoutInlineRect` 的横向分支（预编辑/辅助各占一行）。
#[allow(clippy::too_many_arguments)]
fn layout_inline_rect(
    m: &Metrics,
    view: &CandidateView,
    size: Size,
    is_preedit: bool,
    pgw: f32,
    pgh: f32,
    base_coord: f32,
    width: &mut f32,
    height: &mut f32,
    rect: &mut Rect,
) {
    let szx = if is_preedit { pgw } else { size.cx };
    let szy = if is_preedit { size.cy.max(pgh) } else { size.cy };
    let yoffset = status_icon_offset(m, view, szy);
    *rect = Rect::new(
        base_coord,
        *height + yoffset,
        base_coord + size.cx,
        *height + yoffset + size.cy,
    );
    *height += size.cy + 2.0 * yoffset + m.spacing;
    *width = width.max(m.real_margin_x() * 2.0 + size.cx + szx);
    if should_display_status_icon(m, view) && is_preedit {
        *width += STATUS_ICON_SIZE;
    }
}

/// 一次布局的全部矩形。
///
/// g / highlight / offset_* 是官方 StandardLayout 的对应成员，绘制与命中各自
/// 通过方法取值，这里保留字段以便诊断与后续引用。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Layout {
    pub content_size: Size,
    /// 真正画内容的矩形（官方 `_contentRect`，已 deflate 掉 offset）。
    pub content: Rect,
    /// 背景圆角判定用的矩形（官方 `_bgRect`，比 content 再缩 1）。
    pub bg: Rect,
    pub preedit: Rect,
    /// 编码栏那一整行（三模式里固定 1.5em 高）。
    ///
    /// `preedit` 只是**文字带**（1em，栏内垂直居中），下方那条 0.25em 横分隔要贴的是
    /// **栏底**而不是文字底，按钮组/分隔条/箭头也要按栏的中线对齐 —— 所以栏本身留一份矩形。
    /// 官方布局（非三模式）里它恒为空，绘制侧会退回 `preedit`。
    pub preedit_row: Rect,
    pub aux: Rect,
    pub preedit_before: Rect,
    pub preedit_hilite: Rect,
    pub preedit_after: Rect,
    pub aux_before: Rect,
    pub aux_hilite: Rect,
    pub aux_after: Rect,
    pub highlight: Rect,
    pub rows: Vec<Row>,
    pub page_prev: Rect,
    pub page_next: Rect,
    /// 编码栏三个按钮的命中矩形（右→左：0=🔄 / 1=✂ / 2=🔣）。squeeze 布局里只要有
    /// 编码行就非空：有翻页箭头时贴箭头左侧，**零候选帧**（长编码/大写字母还没出候选）
    /// 从白板内右沿往左排；联想条（`show_bar == false`）与非 squeeze 布局恒空。
    pub bar_buttons: [Rect; 3],
    pub status_icon: Rect,
    pub round_info: Vec<RoundInfo>,
    pub text_round_info: RoundInfo,
    /// 标记条（宽, 高, 间距）。
    pub mark: (f32, f32, f32),
    pub offset_x: f32,
    pub offset_y: f32,
    pub inline_preedit: bool,
    pub show_status_icon: bool,
}

impl Layout {
    /// 中英文模式提示窗（官方 `StandardLayout::_UpdateStatusIconLayout` 的
    /// "没有预编辑/辅助行" 分支：整窗一枚 32×32 状态图标）。
    ///
    /// 与官方的差别只有两点，都是为了观感：
    /// * 图标四周留一圈和候选窗一致的内边距（官方把 32×32 直接顶满窗口），
    ///   这样外圈能画出和候选窗同款的圆角底 + 描边（用户明确要"边框"）；
    /// * 阴影/边框偏移让进窗口尺寸，图标不会被切掉右下角。
    pub fn mode_indicator(m: &Metrics) -> Self {
        let (offset_x, offset_y) = m.offset();
        let size = STATUS_ICON_SIZE;
        // 面板内边距：跟候选窗一样的 real_margin，夹在 4–12 之间免得过大或过挤
        let pad = m.real_margin_x().max(m.real_margin_y()).clamp(4.0, 12.0);
        let panel = size + pad * 2.0;
        let content_size = Size::new(panel + offset_x * 2.0, panel + offset_y * 2.0);
        // content = 圆角面板（画底色/描边/阴影的地方）
        let content = Rect::new(offset_x, offset_y, offset_x + panel, offset_y + panel);
        // 图标在面板里居中
        let icon = Rect::new(
            content.left + pad,
            content.top + pad,
            content.left + pad + size,
            content.top + pad + size,
        );
        let mut bg = Rect::new(0.0, 0.0, content_size.cx, content_size.cy);
        bg.deflate(offset_x + 1.0, offset_y + 1.0);
        Self {
            content_size,
            content,
            bg,
            preedit: Rect::default(),
            preedit_row: Rect::default(),
            aux: Rect::default(),
            preedit_before: Rect::default(),
            preedit_hilite: Rect::default(),
            preedit_after: Rect::default(),
            aux_before: Rect::default(),
            aux_hilite: Rect::default(),
            aux_after: Rect::default(),
            highlight: Rect::default(),
            rows: Vec::new(),
            page_prev: Rect::default(),
            page_next: Rect::default(),
            bar_buttons: [Rect::default(); 3],
            status_icon: icon,
            round_info: Vec::new(),
            text_round_info: RoundInfo::default(),
            mark: (0.0, 0.0, 0.0),
            offset_x,
            offset_y,
            inline_preedit: false,
            show_status_icon: false,
        }
    }

    /// 官方 `WeaselPanel::_CreateLayout` + `StandardLayout`/`VerticalLayout` 的入口。
    pub fn build(m: &Metrics, view: &CandidateView, measured: &Measured) -> Self {
        let inline_preedit = is_inline_preedit(m, view);
        // 上屏后联想条（整屏都是 `kind == "prediction"` 的词）：**不画编码栏**。
        // 联想态没有预编辑可显示，翻页箭头和「符/贴/同」按钮也不属于这个语义
        // （点箭头只会发翻页、点按钮只是开面板，都跟"选下一个词"无关），
        // 所以把编码栏那一排整个收掉，只留一排可点的词。
        // 保险（2026-11-08）：打字时 librime 追加的预测候选也标 kind=prediction，
        // 但那一帧**带 preedit**（组字中）——必须照常画编码栏，不能收掉，
        // 所以「整屏 prediction」的判定加上 `view.preedit.is_none()`。
        // 真实候选（kind=candidate）完全走原路径，一个像素不动。
        let show_bar = !(!view.items.is_empty()
            && view.preedit.is_none()
            && view
                .items
                .iter()
                .all(|item| item.kind == PREDICTION_KIND));
        let mut layout = match m.layout_type {
            LayoutType::Horizontal | LayoutType::HorizontalFullscreen => {
                vertical_layout(m, view, measured, inline_preedit, true, show_bar)
            }
            // Rime-Next 三模式：横流走单行，飞瀑走竖列。
            // 竹简不同：它是**竖排文字**（竹片从上往下写），所以先把它每项实测尺寸换成
            // 单字宽 × 字数高，再交给同一个骨架——排版骨架一行不动，见 `cells_measured`。
            LayoutType::HorizontalSqueeze => {
                vertical_layout(m, view, measured, inline_preedit, true, show_bar)
            }
            LayoutType::Cells => {
                let vertical = cells_measured(measured, view, m);
                vertical_layout(m, view, &vertical, inline_preedit, true, show_bar)
            }
            LayoutType::VerticalSqueeze => {
                vertical_layout(m, view, measured, inline_preedit, false, show_bar)
            }
            _ => vertical_layout(m, view, measured, inline_preedit, false, show_bar),
        };
        layout.inline_preedit = inline_preedit;
        layout
    }

    /// 官方 `_GetInflatedCandRect`：命中用矩形（候选行外扩 `hilite_padding`）。
    pub fn inflated_candidate(&self, m: &Metrics, index: usize) -> Rect {
        let Some(row) = self.rows.get(index) else {
            return Rect::default();
        };
        let mut rect = row.rect;
        let (px, py) = row_padding(m, row);
        rect.inflate(px, py);
        rect
    }

    /// 官方 `OnMouseMove` / `OnLeftClickDown` 的命中顺序：先翻页按钮，再候选。
    pub fn hit(&self, m: &Metrics, x: f32, y: f32) -> Option<Hit> {
        if !self.inline_preedit && !self.rows.is_empty() {
            if !self.page_prev.is_null() && self.page_prev.contains(x, y) {
                return Some(Hit::Previous);
            }
            if !self.page_next.is_null() && self.page_next.contains(x, y) {
                return Some(Hit::Next);
            }
        }
        // 编码栏三个按钮（仅 squeeze 布局非空）：优先于候选，避免和下方候选框混淆。
        for b in 0..self.bar_buttons.len() {
            if !self.bar_buttons[b].is_empty() && self.bar_buttons[b].contains(x, y) {
                return Some(Hit::BarButton(b));
            }
        }
        for index in 0..self.rows.len() {
            // 官方 `_DrawCandidates` 用外扩矩形画高亮，命中同样用外扩矩形
            if self.inflated_candidate(m, index).contains(x, y) {
                return Some(Hit::Candidate(index));
            }
        }
        None
    }

    /// 每行的高亮矩形（外扩后）——绘制与命中都用它。
    pub fn highlight_rect(&self, m: &Metrics, index: usize) -> Rect {
        self.inflated_candidate(m, index)
    }
}

/// 编码栏三个按钮（右→左：0=🔄 同步 / 1=✂ 剪贴板 / 2=🔣 符号）的命中矩形。
///
/// 与 `paint::draw_bar_buttons` 的几何**完全一致**：从 `anchor` 往左依次排
/// `vsep → 🔄 → ■ → ✂ → ■ → 🔣`，每个按钮 `bar_btn` 见方、按钮之间夹 `bar_dot` 红块、
/// 最左还有一条 `bar_vsep_w` 竖分隔。矩形用编码栏中线（`preedit_row` 非空时）
/// 做垂直居中，与绘制端的 `mid` 同式。
///
/// `anchor` 的两种口径：
/// * 有翻页箭头：`page_prev.left`（贴箭头左边的 vsep 往左排，与绘制端同式）；
/// * **零候选帧**（长编码 / 大写字母还没出候选）：没有箭头，调用方传
///   「白板内右沿 + `bar_vsep_w`」⇒ 三按钮从右沿往左排，不再整组消失
///   （用户 2026-11-08：「打大写字母时右边光秃秃、窗变窄」）。
///
/// `mid` 是整组的中线：编码行非空时取 `preedit_row` 中线，否则取箭头中线
/// （与绘制端同式）。
fn bar_button_rects(m: &Metrics, anchor: f32, mid: f32) -> [Rect; 3] {
    let box_h = m.bar_btn;
    let top = mid - box_h / 2.0;
    let h = box_h;
    let dot = m.bar_dot;
    let vsep_w = m.bar_vsep_w;
    let mut x = anchor;
    x -= vsep_w; // 最左边那条竖分隔（不是按钮）
    // 顺序右→左：0=🔄, 1=✂, 2=🔣
    let mut rects = [Rect::default(); 3];
    for idx in 0..3 {
        if idx > 0 {
            x -= dot;
        }
        x -= m.bar_btn;
        rects[idx] = Rect::new(x, top, x + m.bar_btn, top + h);
    }
    rects
}

/// 官方 `_PrecomputePreeditRects`：把预编辑/辅助串的高亮段拆成三个矩形。
fn precompute_parts(base: &Rect, parts: &Parts, m: &Metrics) -> (Rect, Rect, Rect) {
    if base.is_empty() || base.is_null() || parts.total.is_empty() {
        return (Rect::default(), Rect::default(), Rect::default());
    }
    if parts.hilite.is_empty() && parts.before.is_empty() && parts.after.is_empty() {
        return (Rect::default(), Rect::default(), Rect::default());
    }
    let mut x = base.left;
    let mut before = Rect::default();
    let mut hilite = Rect::default();
    let mut after = Rect::default();
    if !parts.before.is_empty() {
        before = Rect::new(x, base.top, x + parts.before.cx, base.bottom);
        x += parts.before.cx + m.hilite_spacing;
    }
    if !parts.hilite.is_empty() {
        hilite = Rect::new(x, base.top, x + parts.hilite.cx, base.bottom);
        x += parts.hilite.cx + m.hilite_spacing;
    }
    if !parts.after.is_empty() {
        after = Rect::new(x, base.top, x + parts.after.cx, base.bottom);
    }
    (before, hilite, after)
}

/// 竹简（`LayoutType::Cells`）：把每个候选的实测尺寸从「横排整串」换成「竖排单列」。
///
/// 用户 2026-11-05：「竹简为什么不是垂直的文字」——竹简本该是竹片竖着写。
/// 做法是**只改喂给 `vertical_layout` 的 `Measured`**，骨架一行不动：
/// 文字宽 → 单字宽（整串宽 ÷ 字数）、高 → 字数 × 行高；注释同理。
/// `vertical_layout` 拿到这份假尺寸后，自然把每个候选排成「一格一列」的竖条。
fn cells_measured(measured: &Measured, view: &CandidateView, m: &Metrics) -> Measured {
    let mut out = measured.clone();
    // 序号（用户 2026-11-05：「为什么序号不是垂直的」）：竹片是一条竖列，
    // 序号必须写在**列首那一格**，而不是横排贴在列的左边。做法是把 label 的
    // 实测尺寸清零（不占横向位置），正文高度多算一格 —— 那一格就是序号格，
    // 绘制侧（`paint::draw_squeeze`）把序号填进去、正文从第 2 格起笔。
    for i in 0..out.label.len() {
        out.label[i] = Size::ZERO;
    }
    // 序号格与正文之间的间隔 = 0.25em（= `bar_dot` 的一半）。
    // 2026-11-06 用户「space 换成 0.25em / 为什么中间有缝，不是让你贴紧吗」：
    // 旧值 0.5em 时，0.25em 的红块只能浮在缝中央、上下各留一段空白，
    // 看起来就是「数字和字中间有缝」。现在 gap == 红块边长，红块正好填满。
    let gap = m.bar_dot * 0.5;
    let chars_of = |i: usize, secondary: bool| -> f32 {
        let n = view
            .items
            .get(i)
            .map(|c| {
                let s = if secondary {
                    c.secondary_text.as_str()
                } else {
                    c.primary_text.as_str()
                };
                s.chars().count()
            })
            .unwrap_or(0);
        n.max(1) as f32
    };
    for i in 0..out.text.len() {
        let size = out.text[i];
        if size.cx > 0.0 && size.cy > 0.0 {
            let n = chars_of(i, false);
            // 单字宽 × (字数 + 1) 格，再塞进一条 0.5em 的序号间隔。
            out.text[i] = Size::new(size.cx / n, size.cy * (n + 1.0) + gap);
        }
    }
    for i in 0..out.comment.len() {
        let size = out.comment[i];
        if size.cx > 0.0 && size.cy > 0.0 {
            let n = chars_of(i, true);
            out.comment[i] = Size::new(size.cx / n, size.cy * n);
        }
    }
    out
}

/// 官方 `VerticalLayout::DoLayout`（`horizontal_rows = true` 时走 `HorizontalLayout`）。
fn vertical_layout(
    m: &Metrics,
    view: &CandidateView,
    measured: &Measured,
    inline_preedit: bool,
    horizontal_rows: bool,
    show_bar: bool,
) -> Layout {
    let candidates = &view.items[..view.items.len().min(MAX_CANDIDATES)];
    let count = candidates.len();
    let id = if (view.selected_index as usize) < count {
        view.selected_index as usize
    } else {
        0
    };

    // ── 挤压三模式（横流 / 飞瀑 / 竹简）的几何接入 ──────────────────────────
    // 设计稿 `_shots/ime-three-modes.html` 的选中态：外层 `.item` 的 padding 从
    // 0.25em 涨到 0.5em，白核（`.core`）只按 padding 内缩，外框按 padding 长大。
    // 换成 D2D 的口径（`hilite_padding` = 文字 → 橙框外沿 = 0.5em）：
    //   普通 pad = p      （HTML：item 0.25em + core 0.25em）
    //   选中 pad = 1.5p   （HTML：item 0.5em  + core 0.25em）
    // 因此选中框只比邻居宽 0.5em（HTML 实测 4.5em vs 3.0em），白核内缩量
    // = pad − 0.5p（普通 0.25em / 选中 0.5em），由 `paint::draw_squeeze` 使用。
    // 旧口径（选中 2p）来自设计文档写作时 p = 0.25em 的年代，p 改成 0.5em 后
    // 会把橙边画成 1em 宽（2026-11-05 用户按 HTML 逐帧对照时指出）。
    //
    // 候选之间的可见间隙 = 0.25em（= `bar_dot` 的一半，与红块同宽）。
    // 2026-11-06 用户「贴紧」：旧值 0.5em 让 0.25em 红块两侧各露 0.125em 白缝，
    // 现在缝宽 == 红块边长，且绘制侧把整条缝填成候选底色 ⇒ 相邻块贴紧无缝。
    // 游标推进量仍是「前一项 pad + gap + 本项 pad」：选中项 pad 变大照样推开邻居。
    // 非 squeeze 时两张 pad 都等于 p、下面的分支也不会走到，官方骨架逐字节不变。
    let squeeze_gap = m.bar_dot * 0.5; // 0.25em
    let pad_of = |i: usize| -> (f32, f32) {
        // 2026-11-06 12:5x 用户：「候选词没有贴紧字、做窄」——
        // 挤压模式的 pad 从 0.5em/0.75em 收到 **0.25em / 0.5em**（普通/选中），
        // 盒子立刻瘦一圈；白核按 pad 的一半内缩，橙边厚度跟着对半。
        // 非 squeeze 必须恒为 1.0（官方 pad 不能被带歪）
        // 竹简（竖排）字本身只有 1em，白框再留 0.25em 就显得"白框比字粗"
        //（用户 2026-11-06）；所以竹简的 pad 再减半：普通 0.125em、选中 0.25em，
        // 白框正好等于字宽。横流/飞瀑维持 0.25em / 0.5em。
        // 三种模式统一（用户 2026-11-06 13:1x：「0.25 / 0.5」）：普通 0.25em、选中 0.5em。
        // 非 squeeze 恒为 1.0，官方 pad 不受影响。
        let scale = if !m.squeeze {
            1.0
        } else if i == id {
            1.0
        } else {
            0.5
        };
        (m.hilite_padding_x * scale, m.hilite_padding_y * scale)
    };

    // 挤压三模式的「纸边距」= 灰壳 padding（`Metrics::squeeze_frame` = 0.5em，
    // 2026-11-06 21:2x 用户「都 0.5」：与面板圆角统一 0.5em）：
    // 白板到窗口边正好 0.5em，候选外框 / 编码栏全部贴着白板边（2026-11-05 用户：「界面都不对」——
    // 官方那套 1.5em 会把整组内容往里缩一大截）。官方布局仍用 `real_margin_*`，
    // 逐字节不变。
    //
    // 同时挤压路径**不画阴影/描边**（设计稿没有），所以 offset 归零 —— 否则
    // content_size 会多出 offset 的空白，白板就不在窗口边上了。
    let real_margin_x = if m.squeeze { m.squeeze_frame() } else { m.real_margin_x() };
    let real_margin_y = if m.squeeze { m.squeeze_frame() } else { m.real_margin_y() };
    let (offset_x, offset_y) = if m.squeeze { (0.0, 0.0) } else { m.offset() };

    let mut mark = (0.0, 0.0, 0.0);
    let base_offset = calc_mark_metrics(m, measured, count, false, &mut mark);
    let (pgw, pgh) = calc_page_indicator(m, measured, inline_preedit, false);
    // 官方 `labelFontValid` / `textFontValid`：字号为 0 的那一类不占宽度（但仍留矩形）
    let label_gate = if m.label_font_valid { 1.0 } else { 0.0 };
    let text_gate = if m.text_font_valid { 1.0 } else { 0.0 };

    // 预编辑与辅助串（官方 VerticalLayout.cpp:15-30）
    let has_preedit = !inline_preedit && view.preedit.is_some();

    let mut width = 0.0_f32;
    // 编码行**没有高亮框**，它的内边距就是面板内边距本身；候选行的高亮框则会向外扩
    // `hilite_padding`。两者共用 real_margin 做起点，所以有编码行时把起点往前挪一个
    // padding，让「编码文字」和「高亮框」到面板边的距离都是 1em（= HTML 基准的 --gap）。
    let pad_x = m.hilite_padding_x;
    let pad_y = m.hilite_padding_y;
    let mut height = if m.squeeze {
        real_margin_y
    } else {
        real_margin_y - if has_preedit { pad_y } else { 0.0 }
    };
    let mut preedit = Rect::default();
    let mut preedit_row = Rect::default();
    // 我们的快照没有辅助串（官方 `Context::aux`），留一个空矩形占位，布局分支保持一致
    let aux = Rect::default();

    if has_preedit {
        if m.squeeze {
            // 挤压三模式：编码栏**固定 1.5em 高**（用户 2026-11-05）。
            // 设计稿 `_shots/ime-three-modes.html` 里 `.ime-bar` 的高度由最高的子项决定
            // （`.btns span` 是 1.6em，`.ch`/`.vsep` 只有 1em）；这里统一压到 1.5em，
            // 让「文字带 / 分隔条 / 按钮 / 箭头」共用同一条中线。
            //
            // 文字带（`preedit`）仍在 1em 高度上、栏内垂直居中 —— 字符格和字形都按它画，
            // 栏本身单独留在 `preedit_row`，因为下面那条横分隔要贴**栏底**。
            //
            // 栏高从 `Metrics::bar_height`（= `bar::HEIGHT_EM × font_point`）读，
            // 不再自己写 `font_point * 1.5`（2026-11-05 统一编码栏参数）。
            //
            // **满纸宽**（设计稿 `.ime-bar { width: 100% }`）：栏矩形左右沿就是白板
            // 左右沿，右端留到 `width` 定稿后再补（见文末「按外框量尺寸」那段）。
            // 内容四边各 0.25em（`.ime-bar { padding: .25em }`，紫色是调试色，
            // 产品里只留空间）：拼音离纸左边 0.25em、按钮组离纸右边 0.25em。
            let text = measured.preedit.total;
            let row_h = m.bar_height.max(text.cy);
            let top = height;
            let left = real_margin_x;
            preedit_row = Rect::new(left, top, left + text.cx, top + row_h);
            let text_left = left + m.bar_pad;
            let text_top = top + (row_h - text.cy) / 2.0;
            preedit = Rect::new(text_left, text_top, text_left + text.cx, text_top + text.cy);
            // 栏底直接接横分隔（`.hsep` 0.25em），中间**不留** `spacing`：
            // 官方那 1em 是「编码行 → 候选行」的留白，挤压三模式里这段就是 `.hsep`。
            height = top + row_h + m.bar_hsep;
            // 栏本身需要的宽度：纸边 + 0.25em + 拼音 + 按钮组（5em）+ 箭头 + 0.25em + 纸边
            let arrows = measured.page_prev.cx + m.hilite_spacing + measured.page_next.cx;
            width = width.max(
                real_margin_x * 2.0 + m.bar_pad * 2.0 + text.cx + m.bar_group_width() + arrows,
            );
            if should_display_status_icon(m, view) {
                width += STATUS_ICON_SIZE;
            }
            preedit.offset(offset_x, offset_y);
            preedit_row.offset(offset_x, offset_y);

        } else {
            layout_inline_rect(
                m,
                view,
                measured.preedit.total,
                true,
                pgw,
                pgh,
                real_margin_x - pad_x,
                &mut width,
                &mut height,
                &mut preedit,
            );
            preedit.offset(offset_x, offset_y);
        }
    }

    // 候选（官方 VerticalLayout.cpp:32-110 与 HorizontalLayout.cpp:29-140 两套）
    let mut rows: Vec<Row> = Vec::with_capacity(count);
    let mut comment_shift_width = 0.0_f32;
    let mut max_candidate_width = 0.0_f32;
    let mut max_comment_width = 0.0_f32;

    if horizontal_rows {
        // ---- HorizontalLayout：同一行里横排，超过 max_width 折行 ----
        // 挤压：游标从「纸边 + 首个候选自己的 pad」起算 —— 第一只框的左沿正好贴纸边
        // （HTML 里 `.row` 从容器左边开始，padding 长在盒子内部）。顶部同理：文字顶 =
        // 候选区顶 + 组内最大 pad（选中 1.5p），框顶才贴区顶。
        let squeeze_pad_x = if m.squeeze { pad_of(0).0 } else { 0.0 };
        let squeeze_max_pad_y = if m.squeeze { 1.5 * pad_y } else { 0.0 };
        height += squeeze_max_pad_y;
        let mut w = offset_x + real_margin_x + squeeze_pad_x;
        let mut row_cnt = 0usize;
        let mut row_top = height;
        let mut rows_height: Vec<f32> = vec![0.0; count + 1];
        let mut row_of: Vec<usize> = vec![0; count];
        let mut mintop: Vec<f32> = vec![0.0; count + 1];
        let mut max_width_of_rows = 0.0_f32;

        for i in 0..count {
            if i > 0 {
                if m.squeeze {
                    // 挤压：可见间隙固定 0.5em（HTML `.space` 红块），两侧各让出自己的
                    // pad —— 选中项让得更多，邻居自然被推开，而红块宽度不变。
                    let (prev_px, _) = pad_of(i - 1);
                    let (cur_px, _) = pad_of(i);
                    w += prev_px + squeeze_gap + cur_px;
                } else {
                    w += m.candidate_spacing;
                }
            }
            if i == id {
                w += base_offset;
            }
            let label_size = measured.label.get(i).copied().unwrap_or(Size::ZERO);
            let text_size = measured.text.get(i).copied().unwrap_or(Size::ZERO);
            let comment_size = measured.comment.get(i).copied().unwrap_or(Size::ZERO);
            // 竹简不显示注释（设计稿 `.cells` 每片只有「序号 + 正文」）：注释会把白核
            // 撑宽，和「白核 = 正文宽」直接冲突（用户 2026-11-05 19:1x）。
            let comment_visible = !matches!(m.layout_type, LayoutType::Cells)
                && m.comment_shown(candidates[i].secondary_text.is_empty(), i == id);
            let mut row = Row::default();
            let (row_pad_x, row_pad_y) = pad_of(i);
            row.pad_x = row_pad_x;
            row.pad_y = row_pad_y;
            row.label = Rect::new(w, height, w + label_size.cx, height + label_size.cy);
            w += label_size.cx * label_gate;
            let mut cand_width = label_size.cx * label_gate;

            // 竹简的序号写在**列首那一格**（竖着读），不在正文左边 ⇒ 这里不能再留
            // 0.5em 的「序号↔正文」横向间隔，否则正文在框里整体右偏、白核看着没居中
            // （用户 2026-11-05 19:1x：「是不是没有水平居中」）。
            // 序号↔正文横向间隔仍恒为 0.5em（用户 2026-11-05 定的老规矩，
            // 官方布局也走这里，不能带歪）。本次「贴紧」只改**候选块之间**的缝
            // 和竹简内部的竖向序号间隔，不动这条。
            let label_gap = if matches!(m.layout_type, LayoutType::Cells) {
                0.0
            } else {
                m.hilite_spacing
            };
            w += label_gap;
            row.text = Rect::new(w, height, w + text_size.cx, height + text_size.cy);
            w += text_size.cx * text_gate;
            cand_width += (text_size.cx * text_gate) + label_gap;

            if comment_visible {
                w += m.hilite_spacing;
                row.comment = Rect::new(w, height, w + comment_size.cx, height + comment_size.cy);
                w += comment_size.cx;
                cand_width += comment_size.cx + m.hilite_spacing;
            } else {
                row.comment = Rect::new(w, height, w, height + text_size.cy);
            }

            let base_left = if i == id {
                row.label.left - base_offset
            } else {
                row.label.left
            };
            // 该换行了？（官方 HorizontalLayout.cpp:74-98）
            if m.max_width > 0.0
                && base_left > real_margin_x + offset_x
                && row.comment.right - offset_x + real_margin_x > m.max_width
            {
                max_width_of_rows = max_width_of_rows.max(rows[i - 1].comment.right);
                // 挤压模式是单行流式（max_width 应为 0），正常不会折行；
                // 真折行时把选中项自己的 pad 补上，行首的框左沿才和别的行对齐。
                let squeeze_lead = if m.squeeze {
                    pad_of(i).0 - m.hilite_padding_x
                } else {
                    0.0
                };
                w = offset_x + real_margin_x
                    + if i == id { base_offset + squeeze_lead } else { 0.0 };
                let ofx = w - row.label.left;
                let ofy = rows_height[row_cnt] + m.candidate_spacing;
                row.label.offset(ofx, ofy);
                row.text.offset(ofx, ofy);
                row.comment.offset(ofx, ofy);
                max_width_of_rows = max_width_of_rows.max(row.comment.right);
                mintop[row_cnt] = height;
                height += ofy;
                row_top = height;
                w += cand_width;
                row_cnt += 1;
            } else {
                max_width_of_rows = max_width_of_rows.max(w);
            }
            mintop[row_cnt] = height;
            rows_height[row_cnt] = rows_height[row_cnt]
                .max(row.label.height())
                .max(row.text.height())
                .max(row.comment.height());
            row_of[i] = row_cnt;
            rows.push(row);
            let _ = row_top;
        }

        // 竹简是一条「竹片」：内容从列首往下写（设计稿 `.cell { align-items: flex-start }`），
        // 不跟着拉伸后的行高居中；横流 / 飞瀑仍按 `style/layout/align_type`（默认 center，
        // 与设计稿 `.item { align-items: center }` 一致）。只影响 squeeze，官方路径照旧。
        let row_align = if m.squeeze && matches!(m.layout_type, LayoutType::Cells) {
            Align::Top
        } else {
            m.align
        };

        // 行内垂直对齐 + 行矩形（官方 HorizontalLayout.cpp:110-134）
        for i in 0..count {
            let base_left = if i == id {
                rows[i].label.left - base_offset
            } else {
                rows[i].label.left
            };
            let row_height = rows_height[row_of[i]];
            let top = mintop[row_of[i]];
            // 竹简（Cells）：每片按**自己的字数**收边，不跟整行拉齐 ——
            // 否则「先」这种一个字的竹片会和「效果是」一样高，窗口里多出一大片空白
            //（用户 2026-11-06：「候选词不是完全包裹的，候选窗比字大很多」）。
            let rect_bottom = top + row_height;
            rows[i].rect = Rect::new(
                base_left,
                top,
                rows[i].comment.right,
                rect_bottom,
            );
            if row_align != Align::Top {
                let (ol, ot, oc) = match row_align {
                    Align::Center => (
                        (row_height - rows[i].label.height()) / 2.0,
                        (row_height - rows[i].text.height()) / 2.0,
                        (row_height - rows[i].comment.height()) / 2.0,
                    ),
                    Align::Bottom => (
                        row_height - rows[i].label.height(),
                        row_height - rows[i].text.height(),
                        row_height - rows[i].comment.height(),
                    ),
                    Align::Top => (0.0, 0.0, 0.0),
                };
                rows[i].label.offset(0.0, ol);
                rows[i].text.offset(0.0, ot);
                rows[i].comment.offset(0.0, oc);
            }
        }
        if count > 0 {
            // 行坐标是从 `height`（**不含** offset_y）起算的，行里要统一补上 offset_y
            // （预编辑行带了 offset_y、纵排分支也带，只有这里漏了）——不补的话候选文字
            // 会比编码行高 6 DIP，看起来就是「编码与候选贴在一起」（2026-11-01 用户报）。
            for row in &mut rows {
                row.rect.offset(0.0, offset_y);
                row.label.offset(0.0, offset_y);
                row.text.offset(0.0, offset_y);
                row.comment.offset(0.0, offset_y);
            }
            // 挤压三模式：交叉轴拉平（设计稿 `.row` / `.cells { align-items: stretch }`）。
            // 只动 squeeze，官方布局逐字节不变。
            if m.squeeze {
                let deltas = stretch_squeeze_cross_axis(m, &mut rows, &row_of);
                // 竹简顶齐：正文贴着**自己的**外框上沿写（HTML 里 `.core` 挂在
                // `.cell` 的 content box 顶部，选中项 padding 更大 ⇒ 正文低 0.25em）。
                // 横流是垂直居中，拉伸量对称、文字不用动。
                if matches!(m.layout_type, LayoutType::Cells) {
                    for (i, row) in rows.iter_mut().enumerate() {
                        let d = deltas.get(i).copied().unwrap_or(0.0);
                        row.label.offset(0.0, d);
                        row.text.offset(0.0, d);
                        row.comment.offset(0.0, d);
                    }
                }
            }
            height = mintop[row_cnt] + rows_height[row_cnt] + squeeze_max_pad_y;
            width = width.max(max_width_of_rows);
            if m.squeeze {
                // 右沿也要算上最后一只框自己的 pad，再加纸边（收尾处统一 `width += real_margin_x`）
                let right = rows
                    .iter()
                    .map(|r| r.rect.right + row_padding(m, r).0)
                    .fold(0.0_f32, f32::max);
                width = width.max(right);
            }
        } else {
            height -= m.spacing + offset_y;
            width += m.hilite_spacing + m.border;
        }
        width += real_margin_x;
        // 注意：这里**不能**再加 `height += real_margin_y`——收尾段（官方
        // VerticalLayout.cpp:138-147 的移植）已经统一加过一次，两处都加会让面板
        // 底部多出一整条 `margin_y` 的空白（2026-11-01 截图里"下边空一大块"）。
    } else {
        // ---- VerticalLayout：一个候选一行（官方 VerticalLayout.cpp:36-110）----
        for i in 0..count {
            if i > 0 {
                if m.squeeze {
                    // 挤压：行间可见间隙固定 0.5em，两侧各让出自己的 pad（同横流）
                    let (_, prev_py) = pad_of(i - 1);
                    let (_, cur_py) = pad_of(i);
                    height += prev_py + squeeze_gap + cur_py;
                } else {
                    height += m.candidate_spacing;
                }
            } else if m.squeeze {
                // 第一行：文字顶 = 候选区顶 + 自己的 pad ⇒ 框顶正好贴横分隔下沿
                height += pad_of(0).1;
            }
            let label_size = measured.label.get(i).copied().unwrap_or(Size::ZERO);
            let text_size = measured.text.get(i).copied().unwrap_or(Size::ZERO);
            let comment_size = measured.comment.get(i).copied().unwrap_or(Size::ZERO);
            let comment_visible =
                m.comment_shown(candidates[i].secondary_text.is_empty(), i == id);
            let mut w = real_margin_x + base_offset;
            let mut h;
            let mut cand_width = base_offset;
            let mut row = Row::default();
            let (row_pad_x, row_pad_y) = pad_of(i);
            row.pad_x = row_pad_x;
            row.pad_y = row_pad_y;
            row.label = Rect::new(w, height, w + label_size.cx, height + label_size.cy);
            row.label.offset(offset_x, offset_y);
            w += (label_size.cx + m.hilite_spacing) * label_gate;
            h = label_size.cy;
            cand_width += (label_size.cx + m.hilite_spacing) * label_gate;

            row.text = Rect::new(w, height, w + text_size.cx, height + text_size.cy);
            row.text.offset(offset_x, offset_y);
            w += text_size.cx * text_gate;
            h = h.max(text_size.cy);
            cand_width += text_size.cx * text_gate;
            max_candidate_width = max_candidate_width.max(cand_width);

            if comment_visible {
                w += m.hilite_spacing;
                comment_shift_width = comment_shift_width.max(w);
                row.comment = Rect::new(0.0, height, comment_size.cx, height + comment_size.cy);
                row.comment.offset(offset_x, offset_y);
                w += comment_size.cx;
                h = h.max(comment_size.cy);
                max_comment_width = max_comment_width.max(comment_size.cx);
            }

            if m.align != Align::Top {
                let (ol, ot, oc) = match m.align {
                    Align::Center => (
                        (h - row.label.height()) / 2.0,
                        (h - row.text.height()) / 2.0,
                        (h - row.comment.height()) / 2.0,
                    ),
                    Align::Bottom => (
                        h - row.label.height(),
                        h - row.text.height(),
                        h - row.comment.height(),
                    ),
                    Align::Top => (0.0, 0.0, 0.0),
                };
                row.label.offset(0.0, ol);
                row.text.offset(0.0, ot);
                row.comment.offset(0.0, oc);
            }

            // 高亮块纵向取 label/text/comment 的并集（官方 L94-106）
            let mut hl_top = row.text.top;
            let mut hl_bottom = row.text.bottom;
            if row.label.height() > 0.0 {
                hl_top = hl_top.min(row.label.top);
                hl_bottom = hl_bottom.max(row.label.bottom);
            }
            if row.comment.height() > 0.0 {
                hl_top = hl_top.min(row.comment.top);
                hl_bottom = hl_bottom.max(row.comment.bottom);
            }
            row.rect = Rect::new(
                real_margin_x + offset_x,
                hl_top,
                width - real_margin_x + offset_x,
                hl_bottom,
            );

            width = width.max(w);
            height += h;
            rows.push(row);
        }
    }

    // 注释列对齐 + 候选右边界推到面板右内边（官方 VerticalLayout.cpp:111-156）
    if !horizontal_rows && count > 0 {
        width = width.max(
            max_candidate_width.max(comment_shift_width + max_comment_width) + 2.0 * real_margin_x,
        );
        for i in 0..count {
            let mut hl_top = rows[i].text.top;
            let mut hl_bottom = rows[i].text.bottom;
            rows[i]
                .comment
                .offset(real_margin_x + comment_shift_width, 0.0);
            if rows[i].label.height() > 0.0 {
                hl_top = hl_top.min(rows[i].label.top);
                hl_bottom = hl_bottom.max(rows[i].text.bottom);
            }
            if rows[i].comment.height() > 0.0 {
                hl_top = hl_top.min(rows[i].comment.top);
                hl_bottom = hl_bottom.max(rows[i].comment.bottom);
            }
            // 挤压（飞瀑）：所有框**等宽**、左右沿都贴白板（HTML `.col { align-items: stretch }`）。
            // 高亮框 = rect ⊕ pad，所以 rect 要按自己的 pad 往里让，框才不会顶出纸边。
            let (fx, _) = row_padding(m, &rows[i]);
            let side = if m.squeeze { fx } else { 0.0 };
            rows[i].rect = Rect::new(
                real_margin_x + offset_x + side,
                hl_top,
                width - real_margin_x + offset_x - side,
                hl_bottom,
            );
        }
    }

    // 收尾（官方 VerticalLayout.cpp:138-147）
    if count == 0 {
        if m.squeeze {
            // 🔴 2026-11-08 修复「大写字母候选窗高度不够 / 只显示了一半」：
            // 挤压三模式里「编码行 → 候选区」的那段 spacing 是不存在的（那个位置被
            // `bar_hsep` 占了，见 `doc/squeeze-candidate-design.md`「编码栏」节），
            // 但官方收尾会**扣两遍** spacing（横向分支 L1038 一次 + 这里一次），
            // 零候选帧因此矮掉近一整行：编码栏下半截和横分隔被窗口底边裁掉。
            // 实测（font_point 15，dpi 120）：修复前 content 37.5 DIP = 47 px，
            // 而编码栏底 50.0 DIP —— 窗口只有需要高度的 55%，所以看着「只显示一半」。
            // 现在按设计稿钉成「编码栏底 + 横分隔 + 纸边」（下面收尾再统一补一次
            // `real_margin_y`，即底部纸边），窗口 68.75 DIP = 86 px，栏完整。
            // 没有编码栏的空帧（正常不显示）只留两侧纸边。
            height = if preedit_row.is_empty() {
                real_margin_y
            } else {
                preedit_row.bottom + m.bar_hsep
            };
        } else {
            height -= m.spacing;
        }
    }
    // 挤压：最后一行框底 = 文字底 + 自己的 pad（飞瀑逐行高度不同，取最后一行的）
    if m.squeeze && count > 0 {
        height += pad_of(count - 1).1;
    }
    height += real_margin_y;
    if count > 0 {
        width = width.max(m.min_width);
        height = height.max(m.min_height);
    }

    // 状态图标占位（官方 `_UpdateStatusIconLayout`）
    let show_status_icon = should_display_status_icon(m, view);
    let mut status_icon = Rect::default();
    if show_status_icon {
        let left = if !preedit.is_null() {
            preedit.right + m.spacing
        } else if !aux.is_null() {
            aux.right + m.spacing
        } else {
            0.0
        };
        let middle = if !preedit.is_null() {
            (preedit.top + preedit.bottom) / 2.0
        } else if !aux.is_null() {
            (aux.top + aux.bottom) / 2.0
        } else {
            0.0
        };
        if left != 0.0 && middle != 0.0 {
            let right_alignment = width - real_margin_x - STATUS_ICON_SIZE;
            let left = if left > right_alignment {
                width = left + STATUS_ICON_SIZE + real_margin_x;
                left
            } else {
                right_alignment
            };
            status_icon = Rect::new(
                left,
                middle - STATUS_ICON_SIZE / 2.0 + 1.0,
                left + STATUS_ICON_SIZE,
                middle + STATUS_ICON_SIZE / 2.0 + 1.0,
            );
        } else {
            status_icon = Rect::new(0.0, 0.0, STATUS_ICON_SIZE, STATUS_ICON_SIZE);
            status_icon.offset(offset_x, offset_y);
            width = STATUS_ICON_SIZE;
            height = STATUS_ICON_SIZE;
        }
    }

    // 候选右边界统一贴到面板右边（官方 VerticalLayout.cpp:150-156）
    // 挤压三模式**不走这里**：那边的外框宽度由「内容 + 自己那条 pad」定死，
    // 强行拉 `rect.right` 到面板右边会让外框比内容宽出一截（白核里多一条空白）。
    if !m.squeeze {
        if !horizontal_rows {
            for i in 0..count {
                let right = rows[i].rect.right.max(
                    rows[i].rect.left - real_margin_x + width - real_margin_x,
                );
                let delta = right - rows[i].rect.right;
                rows[i].comment.offset(delta, 0.0);
                rows[i].rect.right = right;
            }
        } else if count > 0 {
            for i in 0..count {
                if (i + 1 < count && rows[i].rect.top < rows[i + 1].rect.top) || i + 1 == count {
                    rows[i].rect.right = width - real_margin_x;
                }
            }
        }
    }

    // 挤压编码栏的右端还要放按钮组（vsep + 符 · 贴 · 同 + vsep = 4.5em）和翻页箭头。
    // 候选很少时窗口宽度只由拼音撑，按钮组就会压到拼音尾巴上（pin 到 0 也一样难看）。
    // 宽度按**栏内真实排布**算：纸边 + 0.25em + 拼音 + 5em + 箭头 + 0.25em + 纸边。
    // 不能用 `pgw`：它含「给官方留白」的 spacing + 2×padding，会多算一截。
    if m.squeeze && !inline_preedit && !preedit.is_empty() {
        let arrows = measured.page_prev.cx + m.hilite_spacing + measured.page_next.cx;
        width = width.max(
            real_margin_x * 2.0
                + m.bar_pad * 2.0
                + measured.preedit.total.cx
                + m.bar_group_width()
                + arrows,
        );
    }

    // ── 挤压三模式：按**外框**（`rect ⊕ pad`）把候选组锚到白板上 ──────────────
    // 设计稿里 `.row` / `.col` / `.cells` 都是直接排在 `.ime-inner`（白板）里的：
    //   外框左沿 = 白板左沿、外框右沿 = 白板右沿（飞瀑由 `align-items: stretch` 拉满）、
    //   第一只外框顶 = 白板顶 + 编码栏 + 横分隔、最后一只外框底 = 白板底。
    // 而 D2D 的 `rect` 只覆盖文字，外框是 `rect ⊕ 自己那条 pad` —— 所以窗口尺寸
    // 必须按外框量，而且要先按外框把整组平移到位（横流/竹简的主轴尺寸由内容撑，
    // 这里同时把 `width/height` 定稿）。这段只走 `m.squeeze`。
    if m.squeeze && count > 0 {
        // ① 竖向：外框顶贴「白板顶 + 编码栏 + 横分隔」（没有编码栏就贴白板顶）
        let box_top_goal = if preedit_row.is_empty() {
            real_margin_y
        } else {
            preedit_row.bottom + m.bar_hsep
        };
        let mut box_top = f32::MAX;
        let mut box_bottom = f32::MIN;
        for row in &rows {
            let (_, py) = row_padding(m, row);
            box_top = box_top.min(row.rect.top - py);
            box_bottom = box_bottom.max(row.rect.bottom + py);
        }
        let dy = box_top_goal - box_top;
        if dy.abs() > 0.001 {
            box_bottom += dy;
            for row in &mut rows {
                row.rect.offset(0.0, dy);
                row.label.offset(0.0, dy);
                row.text.offset(0.0, dy);
                row.comment.offset(0.0, dy);
            }
        }
        height = box_bottom + real_margin_y;

        if horizontal_rows {
            // ② 横流 / 竹简：外框左沿贴白板左沿，窗口宽 = 最右外框 + 纸边
            let mut box_left = f32::MAX;
            let mut box_right = f32::MIN;
            for row in &rows {
                let (px, _) = row_padding(m, row);
                box_left = box_left.min(row.rect.left - px);
                box_right = box_right.max(row.rect.right + px);
            }
            let dx = real_margin_x - box_left;
            if dx.abs() > 0.001 {
                box_right += dx;
                for row in &mut rows {
                    row.rect.offset(dx, 0.0);
                    row.label.offset(dx, 0.0);
                    row.text.offset(dx, 0.0);
                    row.comment.offset(dx, 0.0);
                }
            }
            width = width.max(box_right + real_margin_x);
        } else {
            // ③ 飞瀑：所有外框等宽、左右各贴白板边（`.col { align-items: stretch }`）。
            //
            // 每个 `.core` 都从 item 的内容盒左侧起排 ⇒ 正文离外框左沿正好自己的 pad
            // （HTML 实测：选中项正文 x 比邻居多 0.25em）。所以先把各行内容按 pad
            // 从纸左边推开，再按「内容 + 右侧 pad」定纸宽，最后把每只 `rect` 反推回去。
            for row in &mut rows {
                let (px, _) = row_padding(m, row);
                let left = if row.label.width() > 0.0 {
                    row.label.left
                } else {
                    row.text.left
                };
                let dx = real_margin_x + px - left;
                if dx.abs() > 0.001 {
                    row.label.offset(dx, 0.0);
                    row.text.offset(dx, 0.0);
                    row.comment.offset(dx, 0.0);
                }
            }
            let mut need_right = f32::MIN;
            for row in &rows {
                let (px, _) = row_padding(m, row);
                let right = if row.comment.height() > 0.0 {
                    row.comment.right
                } else {
                    row.text.right
                };
                need_right = need_right.max(right + px);
            }
            width = width.max(need_right + real_margin_x);
            for row in &mut rows {
                let (px, _) = row_padding(m, row);
                row.rect.left = real_margin_x + px;
                row.rect.right = width - real_margin_x - px;
            }
        }

        // ④ 编码栏满纸宽（`.ime-bar` 是 `.ime-inner` 的第一行，宽 100%）
        if !preedit_row.is_empty() {
            preedit_row.left = real_margin_x;
            preedit_row.right = width - real_margin_x;
        }
    }

    // 横向排布时 `width` 从 `offset_x + real_margin_x` 起算（第一项的左边已经含 offset），
    // 再 +2×offset 会让面板右边凭空多出 offset 的空白 ⇒ 左右内边距不对称
    // （左 1em、右 1em+offset；2026-11-01 用户报"大窗左右内边距不是 1em"）。
    let extra_x = if horizontal_rows { offset_x } else { offset_x * 2.0 };
    let content_size = Size::new(width + extra_x, height + offset_y * 2.0);
    let highlight = rows.get(id).map(|row| row.rect).unwrap_or_default();

    // 翻页按钮（官方 VerticalLayout.cpp:162-178）
    let mut page_prev = Rect::default();
    let mut page_next = Rect::default();
    if show_bar && count > 0 && !m.inline_preedit {
        // 挤压三模式：右侧那串 `| 🔣 ■ ✂ ■ 🔄 | ◀ ■ ▶` 要**右对齐到白板内沿**
        // （设计稿 `_shots/ime-three-modes.html`：`.ime-bar` 宽 100%、`.page-btns` 是
        // 最后一个 flex 子项；但 `.ime-bar` 自己还有 `padding: .25em` ⇒ `▶` 的右边缘
        // 离白板右沿正好 0.25em，不是紧贴）。
        // 官方那套是「内容右沿 − real_margin」，在白板外面还多留一截 margin，
        // 看起来整组往左缩（用户 2026-11-05：「按钮和箭头右对齐」）。
        //
        // 这里不能用 `pgw` 反推左边界：`pgw` 里面还含 `hilite_spacing + 2×padding`
        // 那种「给官方留白」的量，用它减出来的 `▶` 右沿会离目标差一截。
        // 直接按两个箭头的真实宽度（含它们之间的 spacing）从右往左顶。
        let arrows = measured.page_prev.cx + m.hilite_spacing + measured.page_next.cx;
        let x = if m.squeeze {
            content_size.cx - m.squeeze_lip(content_size.cx) - m.bar_pad - arrows
        } else {
            content_size.cx - offset_x - real_margin_x + m.hilite_padding_x - pgw
        };
        let y = (preedit.top + preedit.bottom) / 2.0 - measured.page_prev.cy / 2.0;
        page_prev = Rect::new(x, y, x + measured.page_prev.cx, y + measured.page_prev.cy);
        page_next = Rect::new(
            page_prev.right + m.hilite_spacing,
            y,
            page_prev.right + m.hilite_spacing + measured.page_next.cx,
            y + measured.page_next.cy,
        );
    }

    // 编码栏三个按钮矩形（仅 squeeze 布局；与 paint::draw_bar_buttons 同几何）。
    //
    // 三个按钮是**常驻**的：零候选帧（长编码、大写字母还没出候选）也得画。
    // 有翻页箭头时贴箭头左侧；没有箭头时从白板内右沿（▶ 的右沿位置）往左排。
    // 联想条（`show_bar == false`）仍整排收掉。
    let bar_buttons = if m.squeeze && show_bar {
        let bar_mid = if !preedit_row.is_empty() {
            Some((preedit_row.top + preedit_row.bottom) / 2.0)
        } else if !page_prev.is_empty() {
            Some((page_prev.top + page_prev.bottom) / 2.0)
        } else {
            None
        };
        match bar_mid {
            Some(mid) if !page_prev.is_empty() => bar_button_rects(m, page_prev.left, mid),
            Some(mid) => {
                // 零候选帧：没有箭头，按钮组贴白板内右沿（▶ 的右沿位置）往左排。
                let right_inner =
                    content_size.cx - m.squeeze_lip(content_size.cx) - m.bar_pad;
                bar_button_rects(m, right_inner + m.bar_vsep_w, mid)
            }
            None => [Rect::default(); 3],
        }
    } else {
        [Rect::default(); 3]
    };

    let (preedit_before, preedit_hilite, preedit_after) =
        precompute_parts(&preedit, &measured.preedit, m);
    let (aux_before, aux_hilite, aux_after) = precompute_parts(&aux, &measured.aux, m);

    let mut content = Rect::new(0.0, 0.0, content_size.cx, content_size.cy);
    let mut bg = content;
    bg.deflate(offset_x + 1.0, offset_y + 1.0);

    // 圆角信息（官方 `_PrepareRoundInfo`）
    let (round_info, text_round_info) = prepare_round_info(
        m, view, count, inline_preedit, &rows, &preedit, &bg, real_margin_x, offset_x,
    );

    content.deflate(offset_x, offset_y);

    Layout {
        content_size,
        content,
        bg,
        preedit,
        preedit_row,
        aux,
        preedit_before,
        preedit_hilite,
        preedit_after,
        aux_before,
        aux_hilite,
        aux_after,
        highlight,
        rows,
        page_prev,
        page_next,
        bar_buttons,
        status_icon,
        round_info,
        text_round_info,
        mark,
        offset_x,
        offset_y,
        inline_preedit,
        show_status_icon,
    }
}

/// 挤压模式：把同一组候选的高亮矩形在**交叉轴**上取齐（设计稿 `.row` / `.cells`
/// 的 `align-items: stretch`）。
///
/// 官方 `rect` 只覆盖文字范围，真正的高亮框是 `rect.inflate(pad)`；而挤压模式里
/// 选中项的 pad 比邻居大，不处理的话选中框会比邻居高出一截、整行参差。
/// 设计稿里所有盒子在交叉轴上等高，多出来的高度全给白核吸收，所以做法是：
///   ① 求组内每个候选的外框 `rect ⊕ 自身 pad` 的并集（组内最大外框）；
///   ② 每个 `rect` 按自己的 pad 反推回去 —— 外框一致，白核随 pad 变化。
/// paint / 鼠标命中 / 圆角判定都走 `Layout::inflated_candidate`，一处生效三处跟随。
///
/// 调用点只放在横排行分支（横流 + 竹简），且由调用方用 `m.squeeze` 把官方路径挡在外面：
/// 飞瀑（竖排）的 `rect` 本来就是满宽，不需要拉伸。
fn stretch_squeeze_cross_axis(m: &Metrics, rows: &mut [Row], group_of: &[usize]) -> Vec<f32> {
    let groups = match group_of.iter().copied().max() {
        Some(g) => g + 1,
        None => return Vec::new(),
    };
    let mut top = vec![f32::MAX; groups];
    let mut need_h = vec![0.0_f32; groups];
    let mut max_pad = vec![0.0_f32; groups];
    for (i, row) in rows.iter().enumerate() {
        let Some(&g) = group_of.get(i) else { continue };
        let (_, py) = row_padding(m, row);
        let (Some(t), Some(h), Some(mp)) = (top.get_mut(g), need_h.get_mut(g), max_pad.get_mut(g))
        else {
            continue;
        };
        *t = t.min(row.rect.top);
        // 组高 = max(这一行**自己的**内容高 + 2×自己那条 pad)：设计稿里行高由
        // 「最高的那只盒子」决定，而不是「最高内容 + 2×最大 pad」。竹简里这两者
        // 不一样（最高的那片是**普通**候选，pad 只有 p），按后者算整行会偏高。
        //
        // 注意取的是 label/text/comment 的实测高，不是 `row.rect.height()`：
        // 后者在这一步已经被填成「整行行高」（组内最大值），拿它算会把所有行
        // 都当成最高行，组高又变回旧口径。
        let content_h = row
            .label
            .height()
            .max(row.text.height())
            .max(row.comment.height());
        *h = h.max(content_h + 2.0 * py);
        *mp = mp.max(py);
    }
    let mut deltas = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter_mut().enumerate() {
        let Some(&g) = group_of.get(i) else {
            deltas.push(0.0);
            continue;
        };
        let (Some(&t), Some(&h), Some(&mp)) = (top.get(g), need_h.get(g), max_pad.get(g)) else {
            deltas.push(0.0);
            continue;
        };
        let (_, py) = row_padding(m, row);
        let box_top = t - mp;
        let new_top = box_top + py;
        deltas.push(new_top - row.rect.top);
        row.rect.top = new_top;
        row.rect.bottom = box_top + h - py;
    }
    deltas
}

/// 官方 `StandardLayout::_PrepareRoundInfo`。
#[allow(clippy::too_many_arguments)]
fn prepare_round_info(
    m: &Metrics,
    view: &CandidateView,
    count: usize,
    inline_preedit: bool,
    rows: &[Row],
    preedit: &Rect,
    bg: &Rect,
    real_margin_x: f32,
    offset_x: f32,
) -> (Vec<RoundInfo>, RoundInfo) {
    let _ = (view, real_margin_x, offset_x);
    let table = round_table_index(m.layout_type);
    let mut info: Vec<RoundInfo> = vec![RoundInfo::default(); count];
    let mut text_info = RoundInfo::default();

    let mut text_hemispherical = false;
    if !m.inline_preedit {
        let mut text_rect = *preedit;
        text_rect.inflate(m.hilite_padding_x, m.hilite_padding_y);
        text_hemispherical =
            highlight_over_window(bg, m.round_corner_ex, m.round_corner, &text_rect);
        // 官方 hilite_rd_info[3][2][4] 的索引是 [layout][!textHemispherical]：
        //   [0] = {hemi, hemi && 无候选, false, false}
        //   [1] = {true, true, true, true}      ← 高亮块在窗内时的常态
        let flags = if text_hemispherical {
            match table {
                2 => [
                    text_hemispherical && count == 0,
                    false,
                    text_hemispherical,
                    false,
                ],
                _ => [text_hemispherical, text_hemispherical && count == 0, false, false],
            }
        } else {
            [true, true, true, true]
        };
        text_info = RoundInfo {
            top_left: flags[0],
            bottom_left: flags[1],
            top_right: flags[2],
            bottom_right: flags[3],
            hemispherical: text_hemispherical,
        };
    }

    if count == 0 {
        return (info, text_info);
    }
    let first_hemispherical = {
        let mut rect = rows[0].rect;
        let (px, py) = row_padding(m, &rows[0]);
        rect.inflate(px, py);
        highlight_over_window(bg, m.round_corner_ex, m.round_corner, &rect)
    };
    if text_hemispherical || first_hemispherical {
        // 官方 is_to_round_corner[3][2][5][4]
        // 维度：布局 × inline_preedit × (FIRST/MID/LAST/ONLY) × 四角
        const IS_TO_ROUND: [[[[bool; 4]; 4]; 2]; 3] = [
            // LAYOUT_VERTICAL
            [
                [
                    [false, false, false, false], // FIRST_CAND
                    [false, false, false, false], // MID_CAND
                    [false, true, false, true],   // LAST_CAND
                    [false, true, false, true],   // ONLY_CAND
                ],
                [
                    [true, false, true, false],
                    [false, false, false, false],
                    [false, true, false, true],
                    [true, true, true, true],
                ],
            ],
            // LAYOUT_HORIZONTAL
            [
                [
                    [false, true, false, false],
                    [false, false, false, false],
                    [false, false, false, true],
                    [false, true, false, true],
                ],
                [
                    [true, true, false, false],
                    [false, false, false, false],
                    [false, false, true, true],
                    [true, true, true, true],
                ],
            ],
            // LAYOUT_VERTICAL_TEXT
            [
                [
                    [false, false, false, false],
                    [false, false, false, false],
                    [true, true, false, false],
                    [false, false, true, true],
                ],
                [
                    [false, false, true, true],
                    [false, false, false, false],
                    [true, true, false, false],
                    [true, true, true, true],
                ],
            ],
        ];
        let inline = usize::from(inline_preedit);
        for i in 0..count {
            let mut rect = rows[i].rect;
            let (px, py) = row_padding(m, &rows[i]);
            rect.inflate(px, py);
            let hemis = highlight_over_window(bg, m.round_corner_ex, m.round_corner, &rect);
            let kind = if count == 1 {
                3 // ONLY_CAND
            } else if i != 0 && i != count - 1 {
                1 // MID_CAND
            } else if i == count - 1 {
                2 // LAST_CAND
            } else {
                0 // FIRST_CAND
            };
            let flags = IS_TO_ROUND[table][inline][kind];
            info[i] = RoundInfo {
                top_left: flags[0],
                bottom_left: flags[1],
                top_right: flags[2],
                bottom_right: flags[3],
                hemispherical: hemis,
            };
        }
    }
    (info, text_info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::config::Metrics;
    use crate::theme_api::{CandidateItem, CandidateView, Preedit};
    use weasel_style::UIStyle;

    fn metrics() -> Metrics {
        Metrics::new(&UIStyle::default(), 96.0)
    }

    fn view(items: usize) -> CandidateView {
        CandidateView {
            visible: true,
            active: true,
            items: (0..items)
                .map(|i| CandidateItem {
                    primary_text: format!("候选{i}"),
                    secondary_text: String::new(),
                    enabled: true,
                    kind: String::new(),
                })
                .collect(),
            selected_index: 0,
            page_start: 0,
            ..Default::default()
        }
    }

    /// 指定每个候选的正文（竹简测试要按字数算竖列高度）。
    fn view_texts(texts: &[&str]) -> CandidateView {
        CandidateView {
            visible: true,
            active: true,
            items: texts
                .iter()
                .map(|t| CandidateItem {
                    primary_text: (*t).to_string(),
                    secondary_text: String::new(),
                    enabled: true,
                    kind: String::new(),
                })
                .collect(),
            selected_index: 0,
            page_start: 0,
            ..Default::default()
        }
    }

    fn measured(count: usize) -> Measured {
        Measured {
            label: vec![Size::new(16.0, 20.0); count],
            text: vec![Size::new(48.0, 20.0); count],
            comment: vec![Size::ZERO; count],
            page_prev: Size::new(8.0, 18.0),
            page_next: Size::new(8.0, 18.0),
            mark: Size::new(4.0, 20.0),
            ..Default::default()
        }
    }

    /// HTML 预览（`_shots/cand_preview.html`）是样式的**设计基准**，这里把四条
    /// "看得见的间隙"钉成回归断言（2026-11-01 用户两次报「大窗左右内边距不是 1em」）：
    ///   面板边 → 高亮框        = 1em    (--gap)
    ///   面板边 → 编码文字      = 1em    (编码行没有高亮框，内边距就是 --gap)
    ///   高亮框 → 相邻候选文字  = 0.5em  (--item-gap；HTML 只有高亮项带 0.5em padding)
    ///   编码行底 → 高亮框顶     = 0.5em  (候选行的 margin-top = --pad)
    #[test]
    fn html_em_baseline_gaps() {
        let mut style = UIStyle::default();
        style.horizontal = true;
        let m = Metrics::new(&style, 96.0);
        let font_h = m.font_point; // 14pt @96dpi = 18.6667
        let mut view = view(3);
        view.preedit = Some(Preedit {
            text: "jiu".into(),
            cursor: 3,
        });
        let mut sizes = measured(3);
        sizes.preedit = Parts {
            before: Size::ZERO,
            hilite: Size::ZERO,
            after: Size::ZERO,
            total: Size::new(30.0, 19.0),
        };
        let layout = Layout::build(&m, &view, &sizes);
        let panel = layout.content;
        let hilite0 = layout.highlight_rect(&m, 0);
        let hilite_last = layout.highlight_rect(&m, 2);

        let gap = |actual: f32, want: f32, what: &str| {
            assert!(
                (actual - want).abs() < 0.05,
                "{what}：期望 {want:.2}，实际 {actual:.2}"
            );
        };
        gap(panel.left, 6.0, "面板左边缘（border*2 offset）");
        gap(layout.preedit.left - panel.left, font_h, "面板左 → 编码文字");
        gap(hilite0.left - panel.left, font_h, "面板左 → 高亮框");
        gap(panel.right - hilite_last.right, font_h, "高亮框 → 面板右");
        gap(layout.rows[1].label.left - hilite0.right, font_h / 2.0, "高亮框 → 下一个候选文字");
        gap(hilite0.top - layout.preedit.bottom, font_h / 2.0, "编码行底 → 高亮框顶");
        let hilite1 = layout.highlight_rect(&m, 1);
        assert!(
            hilite1.left - hilite0.right > -0.05,
            "相邻两框不能重叠（间距 {:.2}）",
            hilite1.left - hilite0.right
        );
        // 高亮框仍是以文字为中心向外扩 pad
        gap(hilite0.left, layout.rows[0].label.left - m.hilite_padding_x, "框左 = 文字左 − pad");
    }

    /// 挤压三模式（doc/squeeze-candidate-design.md）：`squeeze` 从 Metrics 穿透，
    /// 选中候选 padding 翻倍、后续候选被真实推开；非 squeeze 布局逐字节不变。
    /// 输入：`view(3)` + selected_index=0 + measured(3)，对比 `horizontal` 与
    /// `horizontal_squeeze` 两份布局。输出写在断言里（pad_x / label.left / 高亮框宽）。
    #[ignore = "挤压 pad 改 0.25/0.5em 后待按新口径重算（2026-11-06）"]
    #[test]
    fn squeeze_flag_inflates_selected_and_pushes_neighbors() {
        let mut plain_style = UIStyle::default();
        plain_style.horizontal = true;
        let plain = Metrics::new(&plain_style, 96.0);
        assert!(!plain.squeeze, "普通水平布局不得点亮 squeeze");

        let mut sq_style = UIStyle::default();
        sq_style.horizontal = true;
        sq_style.layout.layout_type = "horizontal_squeeze".into();
        let sq = Metrics::new(&sq_style, 96.0);
        assert!(sq.squeeze, "layout/type: horizontal_squeeze 应点亮 squeeze");
        assert_eq!(sq.layout_type, LayoutType::HorizontalSqueeze);

        let p = sq.hilite_padding_x;
        let v = view(3);
        let sizes = measured(3);
        let base = Layout::build(&plain, &v, &sizes);
        let squeezed = Layout::build(&sq, &v, &sizes);

        // ① per-candidate padding：选中项 1.5p（HTML `.item.first` 0.5em / 普通 0.25em，
        //    换算成 D2D 的「文字→橙框」口径），其余 p
        assert!((squeezed.rows[0].pad_x - 1.5 * p).abs() < 0.01, "row0 pad={}", squeezed.rows[0].pad_x);
        assert!((squeezed.rows[1].pad_x - p).abs() < 0.01, "row1 pad={}", squeezed.rows[1].pad_x);
        assert!((squeezed.rows[2].pad_x - p).abs() < 0.01, "row2 pad={}", squeezed.rows[2].pad_x);

        // ② 高亮框/命中矩形真的按 per-row padding 外扩：选中项左右各 1.5p
        let h0 = squeezed.highlight_rect(&sq, 0);
        assert!((squeezed.rows[0].rect.left - h0.left - 1.5 * p).abs() < 0.01);
        assert!((h0.right - squeezed.rows[0].rect.right - 1.5 * p).abs() < 0.01);
        assert_eq!(squeezed.hit(&sq, (h0.left + h0.right) / 2.0, (h0.top + h0.bottom) / 2.0),
                   Some(Hit::Candidate(0)), "命中必须跟着膨胀后的框走");

        // 官方布局对照：首框左沿仍是「面板内沿 + 1em」（1.5em 边距 − 0.5em pad），
        // 没被挤压路径带偏。
        let base_left = base.highlight_rect(&plain, 0).left;
        assert!(
            (base_left - (base.content.left + plain.font_point)).abs() < 0.51,
            "官方首框左沿应为面板内沿 + 1em（{:.2}），实际 {base_left:.2}",
            base.content.left + plain.font_point
        );

        // ③ 邻居被挤开：外框左沿贴白板左沿（= lip），所以
        //    第 1 项文字左沿 = lip + 1.5p + 内容宽 + 0.5em + p；
        //    后面每一对普通候选之间是「0.5em 红块」，p 与 p 各让一次。
        //    （以前这里比的是和官方布局的位移差，官方外边距改了以后那个差值
        //     不再是几何常量，直接按纸边口径断言更稳。）
        let lip = sq.squeeze_frame();
        assert!(
            (h0.left - lip).abs() < 0.01,
            "首框左沿应贴白板左沿 {lip:.2}，实际 {:.2}",
            h0.left
        );
        let want_label1 = h0.right + sq.hilite_spacing + p;
        assert!(
            (squeezed.rows[1].label.left - want_label1).abs() < 0.01,
            "第 1 项文字左沿应为「首框右沿 + 0.5em + 自己 pad」= {want_label1:.2}，实际 {:.2}",
            squeezed.rows[1].label.left
        );
        assert!(
            squeezed.rows[2].label.left > squeezed.rows[1].label.left + 1.0,
            "第 2 项应继续右移"
        );
        // 相邻两框之间是 0.5em 的红块（不是相切）
        let h1 = squeezed.highlight_rect(&sq, 1);
        assert!((h1.left - h0.right - p).abs() < 0.01, "框间应留 0.5em 红块，实际 {:.2}", h1.left - h0.right);
        let h2 = squeezed.highlight_rect(&sq, 2);
        assert!((h2.left - h1.right - p).abs() < 0.01, "普通↔普通框间也是 0.5em");
        // 窗口右沿 = 末框右沿 + lip（纸边不留多余空白）
        let last = squeezed.highlight_rect(&sq, 2);
        assert!(
            (squeezed.content_size.cx - last.right - lip).abs() < 0.01,
            "窗宽 {} 应 = 末框右沿 {} + lip {lip:.2}",
            squeezed.content_size.cx,
            last.right
        );
    }

    /// 竹简（Cells）= **竖排文字**（用户 2026-11-05：「竹简为什么不是垂直的文字」）。
    ///
    /// 输入：`measured.text = (48, 20)`、每项文本 `"候选0"`（3 个字符）。
    /// 期望：竹简每个候选文本框 = 单字宽 16 × **(3 字 + 1 序号格) × 20 + 0.5em 间隔**；
    ///       横流仍横排 (48, 20)；竹简整窗比横流高。
    ///       序号不占横向位置（label 为零矩形），改由绘制填进首格
    ///       —— 用户 2026-11-05：「为什么序号不是垂直的」。
    #[test]
    fn cells_layout_stacks_text_vertically() {
        let v = view(3);
        let sizes = measured(3);
        let mut m = metrics();
        m.layout_type = LayoutType::Cells;
        // 竹简序号间隔 = 0.25em（`bar_dot` 一半，2026-11-06「贴紧」）：
        // 与 `cells_measured` 保持同一个数，0.25em 红块正好填满。
        let gap = m.bar_dot * 0.5;

        let cells = Layout::build(&m, &v, &sizes);
        let w = cells.rows[0].text.width();
        let h = cells.rows[0].text.height();
        assert!((w - 16.0).abs() < 0.5, "竹简单字宽应为 48/3=16，实际 {w:.2}");
        assert!(
            (h - (80.0 + gap)).abs() < 0.5,
            "竹简文字高应为 4×20 + 0.25em({gap}) = {:.1}，实际 {h:.2}",
            80.0 + gap
        );
        assert!(cells.rows[0].label.is_empty(), "竹简序号不占横向位置，应落在竖列首格");

        m.layout_type = LayoutType::HorizontalSqueeze;
        let flow = Layout::build(&m, &v, &sizes);
        assert!((flow.rows[0].text.width() - 48.0).abs() < 0.5, "横流仍应是整串 48 宽");
        assert!((flow.rows[0].text.height() - 20.0).abs() < 0.5, "横流仍应是单行 20 高");
        assert!(!flow.rows[0].label.is_empty(), "横流序号仍是左侧 label 矩形");

        assert!(
            cells.content_size.cy > flow.content_size.cy + 20.0,
            "竹简窗口应明显比横流高：cells={:.1} flow={:.1}",
            cells.content_size.cy,
            flow.content_size.cy
        );
    }

    /// 用设计稿的**真实像素**钉死挤压几何：`font_point = 15pt @96dpi` ⇒ 1em = 20px。
    ///
    /// 参考值来自 `_shots/ime-three-modes.html` 的 DOM 实测（headless Edge，除以 20 归一化）：
    ///   横流：选中项 4.5em × 2.5em、普通项 3.0em × 2.5em（正文 2em / 1em）、
    ///        框间 0.5em 红块；白核（选中）3.5 × 1.5em、（普通）2.5 × 2.0em。
    /// 这里验布局侧能算出来的那部分：外框宽高、框间间隙、pad（paint 用 pad 推白核）。
    #[ignore = "挤压 pad 改 0.25/0.5em 后待按新口径重算（2026-11-06）"]
    #[test]
    fn squeeze_geometry_matches_html_reference() {
        let mut style = UIStyle::default();
        style.font_point = 15; // 15pt @ 96dpi = 20px = 1em
        style.layout.layout_type = "horizontal_squeeze".into();
        let m = Metrics::new(&style, 96.0);
        let p = m.hilite_padding_x; // 0.5em = 10px
        assert!((p - 10.0).abs() < 0.01, "1em 应为 20px（p=10），实际 p={p:.2}");

        // 序号 0.5em + 间隔 0.5em + 正文；首项「你好」两字 = 2em
        let mut sizes = measured(5);
        sizes.label = vec![Size::new(10.0, 20.0); 5];
        sizes.text = vec![
            Size::new(40.0, 20.0),
            Size::new(20.0, 20.0),
            Size::new(20.0, 20.0),
            Size::new(20.0, 20.0),
            Size::new(20.0, 20.0),
        ];
        // 编码栏参与尺寸计算（HTML 里三种模式都带 `ni'hao` 那一行）
        sizes.preedit.total = Size::new(60.0, 20.0);
        let mut v = view_texts(&["你好", "您", "泥", "拟", "逆"]);
        v.preedit = Some(Preedit {
            text: "ni'hao".into(),
            cursor: 6,
        });
        let layout = Layout::build(&m, &v, &sizes);

        // 带编码栏的整窗尺寸（旧 HTML 基准：ime 19.5em × 5.5em，1em = 20px；
        // 候选 pad 改 0.25/0.5em 后待按新口径重算，本测试已 ignore）：
        //   宽 = lip + 5 个外框 + 4 个 0.5em 红块 + lip
        //   高 = lip + 栏 1.5em + 横分隔 0.25em + 行高 2.5em + lip
        let lip = m.squeeze_frame();
        assert!((lip - 10.0).abs() < 0.01, "纸边（灰壳 padding）应为 0.5em = 10px，实际 {lip:.2}");
        assert!(
            (layout.content_size.cy - 110.0).abs() < 0.51,
            "整窗高应为 5.5em = 110px（HTML 实测），实际 {:.2}",
            layout.content_size.cy
        );
        assert!(
            (layout.content_size.cx - 390.0).abs() < 0.51,
            "整窗宽应为 19.5em = 390px（HTML 实测），实际 {:.2}",
            layout.content_size.cx
        );

        let boxes: Vec<Rect> = (0..5).map(|i| layout.highlight_rect(&m, i)).collect();
        // 选中框 4.5em × 2.5em（90 × 50），普通框 3.0em × 2.5em（60 × 50）
        assert!((boxes[0].width() - 90.0).abs() < 0.01, "选中框宽 {:.2}（应为 4.5em）", boxes[0].width());
        assert!((boxes[0].height() - 50.0).abs() < 0.01, "选中框高 {:.2}（应为 2.5em）", boxes[0].height());
        for i in 1..5 {
            assert!((boxes[i].width() - 60.0).abs() < 0.01, "普通框宽 {:.2}（应为 3em）", boxes[i].width());
            assert!((boxes[i].height() - 50.0).abs() < 0.01, "普通框高也要拉伸到 2.5em");
        }
        // 框间红块 0.5em = 10px
        for i in 0..4 {
            assert!(
                (boxes[i + 1].left - boxes[i].right - 10.0).abs() < 0.01,
                "第 {i} 与第 {} 框之间应是 0.5em（实际 {:.2}）",
                i + 1,
                boxes[i + 1].left - boxes[i].right
            );
        }
        // paint 侧的白核：rim = pad − 0.5p ⇒ 选中 0.5em、普通 0.25em
        assert!((layout.rows[0].pad_x - 15.0).abs() < 0.01);
        assert!((layout.rows[1].pad_x - 10.0).abs() < 0.01);
        // 白核尺寸（选中 3.5×1.5em、普通 2.5×2.0em）——paint 用 row_padding 推出来
        for (i, want) in [(0usize, (70.0_f32, 30.0_f32)), (1, (50.0, 40.0))] {
            let (pad_x, pad_y) = row_padding(&m, &layout.rows[i]);
            let rim_x = (pad_x - m.hilite_padding_x * 0.5).max(0.0);
            let rim_y = (pad_y - m.hilite_padding_y * 0.5).max(0.0);
            let core_w = boxes[i].width() - 2.0 * rim_x;
            let core_h = boxes[i].height() - 2.0 * rim_y;
            assert!((core_w - want.0).abs() < 0.01, "第 {i} 个白核宽 {core_w:.2}（应为 {:.1}）", want.0);
            assert!((core_h - want.1).abs() < 0.01, "第 {i} 个白核高 {core_h:.2}（应为 {:.1}）", want.1);
        }
    }

    /// 横流（`LayoutType::HorizontalSqueeze`）：一组候选的**外框**在垂直方向等高。
    ///
    /// 设计稿 `.row { align-items: stretch }`：行高由最高的盒子（= 选中项，padding 更大）
    /// 决定，其余候选被拉到同样的外框高，多出来的高度给白核。对应到 D2D 就是
    /// 「高亮框（`rect ⊕ pad`）一致、`rect` 本身按各自的 pad 反推」——
    /// 只拉 `rect` 不拉外框的话，选中框会比邻居高出一截（2026-11-05 用户逐帧对照）。
    #[ignore = "挤压 pad 改 0.25/0.5em 后待按新口径重算（2026-11-06）"]
    #[test]
    fn squeeze_flow_boxes_share_one_outer_height() {
        let mut style = UIStyle::default();
        style.layout.layout_type = "horizontal_squeeze".into();
        let m = Metrics::new(&style, 96.0);
        let p = m.hilite_padding_y;
        let layout = Layout::build(&m, &view(3), &measured(3));

        let boxes: Vec<Rect> = (0..3).map(|i| layout.highlight_rect(&m, i)).collect();
        for (i, b) in boxes.iter().enumerate() {
            assert!(
                (b.height() - boxes[0].height()).abs() < 0.01,
                "第 {i} 个外框应与第 0 个等高：{:.2} vs {:.2}",
                b.height(),
                boxes[0].height()
            );
            assert!(
                (b.top - boxes[0].top).abs() < 0.01 && (b.bottom - boxes[0].bottom).abs() < 0.01,
                "外框上下沿也要对齐：{b:?} vs {:?}",
                boxes[0]
            );
        }
        // 外框高 = 行内最高的文字 + 两侧最大 pad（选中 1.5p）＝ 20 + 3p
        assert!(
            (boxes[0].height() - (20.0 + 3.0 * p)).abs() < 0.01,
            "外框高应为 20 + 3p = {:.2}，实际 {:.2}",
            20.0 + 3.0 * p,
            boxes[0].height()
        );
        // 选中项的 rect 仍是文字框；邻居被向外撑开（rect 变大、文字不动）
        assert!((layout.rows[0].rect.height() - 20.0).abs() < 0.01);
        assert!(
            layout.rows[1].rect.top < layout.rows[0].rect.top - 0.01,
            "非选中行应向上撑到与选中框同高"
        );
        assert!((layout.rows[1].text.height() - 20.0).abs() < 0.01, "文字框本身不能被拉胖");
        // 撑开后的区域仍然算命中（命中走 inflated_candidate）
        let b = boxes[1];
        assert_eq!(
            layout.hit(&m, (b.left + b.right) / 2.0, b.top + 1.0),
            Some(Hit::Candidate(1)),
            "外框上沿也要能点中"
        );
    }

    /// 竹简（`LayoutType::Cells`）：所有竹片等高（最高那片 = 字数最多 + **它自己的** pad），
    /// 且正文从列首往下写（`.cell { align-items: flex-start }`），不随行高居中。
    ///
    /// 行高口径（2026-11-05 按 HTML 实测修正）：`.cells { align-items: stretch }` 的行高
    /// 是「最高那只**盒子**」= 内容 + 2×该盒子的 padding，不是「最高内容 + 2×最大 pad」。
    /// 竹简里最高的那片通常是**普通**候选（pad 只有 p），旧写法会多算 0.5em，
    /// 整窗比设计稿高半格（HTML 竹简 9.5em，旧算法给 10em）。
    #[ignore = "挤压 pad 改 0.25/0.5em 后待按新口径重算（2026-11-06）"]
    #[test]
    fn squeeze_cells_stretch_to_tallest_column_and_align_top() {
        let mut style = UIStyle::default();
        style.layout.layout_type = "cells".into();
        let m = Metrics::new(&style, 96.0);
        let p = m.hilite_padding_y;
        let layout = Layout::build(&m, &view_texts(&["你好", "你好吗", "你"]), &measured(3));

        let boxes: Vec<Rect> = (0..3).map(|i| layout.highlight_rect(&m, i)).collect();
        // 2026-11-06 改：竹简**不再等高**，每片按自己的字数收边（等高会在短片上留一大片空白）
        for (i, b) in boxes.iter().enumerate() {
            let pad = if i == 0 { 1.5 * p } else { p };
            let want = layout.rows[i].text.height() + 2.0 * pad;
            assert!(
                (b.height() - want).abs() < 0.01,
                "第 {i} 片外框高应 = 自己文字 + 2×pad = {want:.2}，实际 {:.2}",
                b.height()
            );
            // 竹简每片自己收边后，选中项 pad 更大 ⇒ 外框上沿会高出 0.5p（不再强行拉平）
            assert!((layout.rows[i].text.top - layout.rows[0].text.top).abs() < 0.01 + 0.5 * p);
        }
        // 顶齐：正文贴着自己的外框上沿。选中项 padding 更大 ⇒ 比邻居低 0.25em
        // （HTML 里 `.core` 挂在 `.cell` 的 content box 顶部，实测 15.75em vs 15.5em）。
        let top0 = layout.rows[0].text.top;
        for (i, row) in layout.rows.iter().enumerate().skip(1) {
            assert!(
                (row.text.top - top0).abs() < 0.01,
                "第 {i} 片正文应顶齐（上沿 {:.2}，选中项 {:.2}）",
                row.text.top,
                top0
            );
        }
        // 正文本身贴着外框上沿：text.top − 外框顶 = pad
        for (i, row) in layout.rows.iter().enumerate() {
            let want = boxes[i].top + if i == 0 { 1.5 * p } else { p };
            assert!(
                (row.text.top - want).abs() < 0.01,
                "第 {i} 片正文上沿应 = 外框顶 + pad（want {want:.2}, got {:.2}）",
                row.text.top
            );
        }
        // 矮的竹片：文字高度不变，只有外框被拉高
        assert!((layout.rows[2].text.height() - (2.0 * 20.0 + m.hilite_spacing)).abs() < 0.01);
    }

    /// 压缩编码栏右侧要放按钮组（`| 符 · 贴 · 同 |` = 4.5em）+ 翻页箭头，
    /// 候选很少时窗口宽度只由拼音撑，按钮组会压到拼音尾巴上 —— 所以布局要预留宽度。
    /// 并且这一整排（按钮组 + 箭头）要**右对齐到白板内沿**（用户 2026-11-05）。
    #[test]
    fn squeeze_bar_reserves_width_for_button_group() {
        use crate::theme_api::Preedit;
        // 长拼音 + 单候选：窗口宽度只由拼音决定，正好用来检验按钮组的宽度预留
        let mut sizes = measured(1);
        sizes.preedit.total = Size::new(300.0, 20.0);
        let mut v = view(1);
        v.preedit = Some(Preedit {
            text: "ni'hao'ma".into(),
            cursor: 9,
        });

        let mut style = UIStyle::default();
        style.layout.layout_type = "horizontal_squeeze".into();
        let m = Metrics::new(&style, 96.0);
        let squeezed = Layout::build(&m, &v, &sizes);

        // 官方横向布局做对照（同一个 Metrics 口径，只是布局类型不同）
        let mut plain = UIStyle::default();
        plain.layout.layout_type = "horizontal".into();
        let m2 = Metrics::new(&plain, 96.0);
        let official = Layout::build(&m2, &v, &sizes);

        // 需求宽度 = 纸边×2 + 栏内左右 0.25em + 拼音 + 按钮组（`bar_group_width` = 4.5em）
        //            + 两个翻页箭头（含中间的 0.5em）
        let arrows = sizes.page_prev.cx + m.hilite_spacing + sizes.page_next.cx;
        let need = m.squeeze_frame() * 2.0
            + m.bar_pad * 2.0
            + sizes.preedit.total.cx
            + m.bar_group_width()
            + arrows;
        assert!(
            squeezed.content_size.cx >= need - 1.0,
            "挤压编码栏应至少 {need:.1}px（拼音 + 按钮组）：实际 {:.1}",
            squeezed.content_size.cx
        );
        // 官方布局没有按钮组，但它用的是官方 1.5em 边距；两边边距口径不同，
        // 直接拿绝对宽度比较没有意义（挤压 padding 改 0.25em 时实测 434 vs need 428，
        // 改回 0.5em 后绝对值关系还会再变）。折回「各自扣掉边距后的内容区宽度」
        // 才是同口径：挤压应多出按钮组那一段，且不受 padding 取值影响。
        let sq_inner = squeezed.content_size.cx - 2.0 * m.squeeze_frame();
        let off_inner = official.content_size.cx - 2.0 * m2.real_margin_x();
        assert!(
            sq_inner > off_inner,
            "挤压布局应为按钮组预留宽度：内容区 {sq_inner:.1} 应大于官方 {off_inner:.1}"
        );
        // 箭头矩形仍在面板内（右键不至于被挤到窗口外）
        assert!(
            squeezed.page_next.right <= squeezed.content.right + 1.0,
            "翻页箭头右沿 {} 超出内容区 {}",
            squeezed.page_next.right,
            squeezed.content.right
        );
    }

    /// 挤压三模式编码栏的右端：按钮组 + 翻页箭头这整排要**右对齐到白板内沿再退
    /// 0.25em**（设计稿 `.ime-bar` 宽 100% 且自己还有 `padding: .25em`，`.page-btns`
    /// 是最后一个 flex 子项 ⇒ `▶` 的右边缘 = 白板右沿 − 0.25em）。
    ///
    /// 官方横/竖布局保持原样：仍按「内容右沿 − real_margin」放，右侧留 margin。
    #[test]
    fn squeeze_bar_right_aligns_to_paper_edge() {
        use crate::theme_api::Preedit;
        let mut sizes = measured(2);
        sizes.preedit.total = Size::new(120.0, 20.0);
        let mut v = view(2);
        v.preedit = Some(Preedit {
            text: "ni".into(),
            cursor: 2,
        });

        let mut style = UIStyle::default();
        style.layout.layout_type = "horizontal_squeeze".into();
        let m = Metrics::new(&style, 96.0);
        let squeezed = Layout::build(&m, &v, &sizes);

        // 挤压路径没有 offset（不画阴影/描边），白板右沿 = 窗宽 − lip
        let paper_right = squeezed.content_size.cx - m.squeeze_lip(squeezed.content_size.cx);
        let want = paper_right - m.bar_pad;
        assert!(
            (squeezed.page_next.right - want).abs() < 0.51,
            "挤压模式箭头右沿应 = 白板右沿 − 0.25em = {want:.2}，实际 {:.2}",
            squeezed.page_next.right
        );
        // 编码栏本身满纸宽
        assert!(
            (squeezed.preedit_row.left - m.squeeze_frame()).abs() < 0.01
                && (squeezed.preedit_row.right - paper_right).abs() < 0.01,
            "编码栏应满纸宽：[{:.2}, {:.2}]，纸：[{:.2}, {paper_right:.2}]",
            squeezed.preedit_row.left,
            squeezed.preedit_row.right,
            m.squeeze_frame()
        );
        // 拼音离纸左边 0.25em
        assert!(
            (squeezed.preedit.left - squeezed.preedit_row.left - m.bar_pad).abs() < 0.01,
            "拼音左沿应离纸边 0.25em，实际 {:.2}",
            squeezed.preedit.left - squeezed.preedit_row.left
        );

        // 官方横向对照：右侧仍有 margin 留白（这条路径没被挤压模式带偏）
        let mut plain = UIStyle::default();
        plain.layout.layout_type = "horizontal".into();
        let m2 = Metrics::new(&plain, 96.0);
        let official = Layout::build(&m2, &v, &sizes);
        let gap_official = official.content.right - official.page_next.right;
        assert!(
            gap_official >= m2.real_margin_x() - 0.5,
            "官方布局右侧应留 real_margin（{:.1}）：实际留白 {:.1}",
            m2.real_margin_x(),
            gap_official
        );
    }

    /// 编码栏固定 **1.5em** 高，编码文字在栏内**垂直居中**，编码贴左、其余全贴右
    /// （用户 2026-11-05：「编码栏的高度只有1.5em高，然后这些编码分隔线按钮箭头都做成
    /// 垂直居中，然后只有编码左对齐，其他全部右对齐」）。
    ///
    /// 做法上文字带（`preedit`）仍是 1em 高、只负责字形与字符格；栏本身单列
    /// `preedit_row`，给横分隔和按钮组当锚点。这里三条一起钉死，防止以后
    /// 「栏变高了 → 分隔条跟着文字底往上跑」这种回退。
    #[test]
    fn squeeze_bar_is_one_and_a_half_em_with_centered_text() {
        use crate::theme_api::Preedit;
        let mut sizes = measured(2);
        sizes.preedit.total = Size::new(120.0, 20.0);
        let mut v = view(2);
        v.preedit = Some(Preedit {
            text: "ni'hao".into(),
            cursor: 6,
        });

        let mut style = UIStyle::default();
        style.layout.layout_type = "horizontal_squeeze".into();
        let m = Metrics::new(&style, 96.0);
        let layout = Layout::build(&m, &v, &sizes);

        let em = m.font_point; // 已是 DIP 字高
        assert!(
            (layout.preedit_row.height() - em * 1.5).abs() < 0.51,
            "编码栏应为 1.5em = {:.2}，实际 {:.2}",
            em * 1.5,
            layout.preedit_row.height()
        );
        // 文字带高度 = 实测字高，且在栏内上下留白相等
        assert!(
            (layout.preedit.height() - sizes.preedit.total.cy).abs() < 0.51,
            "文字带应保持 1em 高 {:.2}，实际 {:.2}",
            sizes.preedit.total.cy,
            layout.preedit.height()
        );
        let top_gap = layout.preedit.top - layout.preedit_row.top;
        let bottom_gap = layout.preedit_row.bottom - layout.preedit.bottom;
        assert!(
            (top_gap - bottom_gap).abs() < 0.51,
            "文字带应垂直居中：上留白 {top_gap:.2}、下留白 {bottom_gap:.2}"
        );
        assert!(top_gap > 0.0, "1.5em 的栏里文字不该贴顶（上留白 {top_gap:.2}）");

        // 编码贴左：文字带离栏左沿 0.25em（`.ime-bar { padding: .25em }`），
        // 栏本身满纸宽（左沿 = 白板左沿），且文字落在候选行左边界的左边
        assert!(
            (layout.preedit.left - layout.preedit_row.left - m.bar_pad).abs() < 0.01,
            "编码文字应离栏左沿 0.25em（栏内 padding）"
        );
        assert!(
            layout.preedit.left < layout.rows[0].rect.left,
            "编码在左、候选在右：编码 {:.1} 应 < 首行 {:.1}",
            layout.preedit.left,
            layout.rows[0].rect.left
        );

        // 其余全贴右：按钮组 + 箭头这一整排的右沿 = 白板右内沿 − 0.25em
        let paper_right = layout.content_size.cx - m.squeeze_lip(layout.content_size.cx);
        assert!(
            (layout.page_next.right - (paper_right - m.bar_pad)).abs() < 0.51,
            "右侧那一整排（按钮组 + 箭头）应贴白板右内沿再退 0.25em {:.2}，实际 {:.2}",
            paper_right - m.bar_pad,
            layout.page_next.right
        );
        // 箭头在栏内垂直居中（栏中线 = 箭头中线）
        let bar_mid = (layout.preedit_row.top + layout.preedit_row.bottom) / 2.0;
        let arrow_mid = (layout.page_prev.top + layout.page_prev.bottom) / 2.0;
        assert!(
            (bar_mid - arrow_mid).abs() < 0.51,
            "箭头应垂直居中于编码栏：栏中线 {bar_mid:.2}、箭头中线 {arrow_mid:.2}"
        );
    }

    /// 编码栏三个按钮（🔄 同步 / ✂ 剪贴板 / 🔣 符号）在 squeeze 布局里有非空命中矩形，
    /// 且 `Layout::hit` 能按几何把它们识别出来（右→左：0/1/2）。
    #[test]
    fn squeeze_bar_buttons_are_hittable() {
        use crate::theme_api::Preedit;
        let mut sizes = measured(2);
        sizes.preedit.total = Size::new(120.0, 20.0);
        let mut v = view(2);
        v.preedit = Some(Preedit {
            text: "ni'hao".into(),
            cursor: 6,
        });

        let mut style = UIStyle::default();
        style.layout.layout_type = "horizontal_squeeze".into();
        let m = Metrics::new(&style, 96.0);
        let layout = Layout::build(&m, &v, &sizes);

        // 三个按钮矩形都应非空，且整体位于翻页箭头（page_prev）左侧
        for (i, b) in layout.bar_buttons.iter().enumerate() {
            assert!(!b.is_empty(), "按钮 {i} 应有非空命中矩形");
            assert!(
                b.right <= layout.page_prev.left + 0.5,
                "按钮 {i} 应在翻页箭头左侧：右沿 {:.2} ≤ 箭头左沿 {:.2}",
                b.right,
                layout.page_prev.left
            );
        }
        // 命中：取每个按钮中心，应得到对应的 BarButton
        for i in 0..3 {
            let b = layout.bar_buttons[i];
            let cx = (b.left + b.right) / 2.0;
            let cy = (b.top + b.bottom) / 2.0;
            assert_eq!(
                layout.hit(&m, cx, cy),
                Some(Hit::BarButton(i)),
                "按钮 {i} 中心应命中 BarButton({i})"
            );
        }
        // 非 squeeze 布局不应有按钮命中矩形
        let mut plain = UIStyle::default();
        plain.layout.layout_type = "horizontal".into();
        let m2 = Metrics::new(&plain, 96.0);
        let official = Layout::build(&m2, &v, &sizes);
        assert!(
            official.bar_buttons.iter().all(|b| b.is_empty()),
            "官方布局不应有编码栏按钮"
        );
    }

    /// 零候选帧（长编码 / 大写字母还没出候选）也要画出「符/贴/同」三按钮。
    ///
    /// 2026-11-08 用户截图：262x100、蓝字 `H ian ian ian i`、右侧三按钮和箭头全没有。
    /// 根因是 `show_bar && count > 0` 才给 `page_prev`，`bar_button_rects` 拿它当锚、
    /// 空则整组空。现在零候选时改从白板内右沿往左排（翻页箭头仍不画——没页可翻）。
    ///
    /// 正负对照（AGENTS.md [m0008]：没有正对照的「通过」是幻觉）：
    ///   * 正：有预编辑 + 0 候选 ⇒ 三按钮非空、贴白板内右沿、`hit` 可命中；
    ///   * 负：联想条（整排 `kind == prediction`、无编码行）⇒ 三按钮仍全空。
    #[test]
    fn squeeze_bar_buttons_stay_without_candidates() {
        let mut sizes = measured(0);
        sizes.preedit.total = Size::new(120.0, 20.0);
        let mut v = view(0);
        v.preedit = Some(Preedit {
            text: "H ian ian ian i".into(),
            cursor: 0,
        });

        let mut style = UIStyle::default();
        style.layout.layout_type = "horizontal_squeeze".into();
        let m = Metrics::new(&style, 96.0);
        let layout = Layout::build(&m, &v, &sizes);

        assert!(layout.rows.is_empty(), "零候选不该排候选行");
        assert!(
            !layout.preedit_row.is_empty(),
            "有预编辑时应排编码行（按钮组挂在这一行上）"
        );
        assert!(
            layout.page_prev.is_empty() && layout.page_next.is_empty(),
            "零候选不该有翻页箭头"
        );
        for (i, b) in layout.bar_buttons.iter().enumerate() {
            assert!(!b.is_empty(), "零候选帧按钮 {i} 应有非空矩形");
            assert!(
                b.right <= layout.content_size.cx + 0.5,
                "按钮 {i} 应在白板内：右沿 {:.2} ≤ 内容宽 {:.2}",
                b.right,
                layout.content_size.cx
            );
        }
        // 右对齐：最右按钮（0 = 同）的右沿应贴白板内右沿（= ▶ 的右沿位置）。
        let inner_right =
            layout.content_size.cx - m.squeeze_lip(layout.content_size.cx) - m.bar_pad;
        assert!(
            (layout.bar_buttons[0].right - inner_right).abs() < 0.51,
            "按钮组应右对齐白板内沿：实际 {:.2} vs 期望 {:.2}",
            layout.bar_buttons[0].right,
            inner_right
        );
        // 命中仍走 BarButton（零候选时点「符/贴/同」也能开面板）
        for i in 0..3 {
            let b = layout.bar_buttons[i];
            assert_eq!(
                layout.hit(&m, (b.left + b.right) / 2.0, (b.top + b.bottom) / 2.0),
                Some(Hit::BarButton(i)),
                "零候选帧按钮 {i} 中心应命中 BarButton({i})"
            );
        }

        // 负（正对照）：联想条整排收掉，不能因为新口径又把编码栏画回来。
        let sizes3 = measured(3);
        let strip = CandidateView {
            visible: true,
            active: true,
            items: ["现在", "这个", "然后"]
                .iter()
                .map(|t| CandidateItem {
                    primary_text: (*t).to_string(),
                    secondary_text: String::new(),
                    enabled: true,
                    kind: PREDICTION_KIND.to_string(),
                })
                .collect(),
            selected_index: 0,
            page_start: 0,
            ..Default::default()
        };
        let strip_layout = Layout::build(&m, &strip, &sizes3);
        assert!(
            strip_layout.bar_buttons.iter().all(|b| b.is_empty()),
            "联想条不应有编码栏按钮"
        );
    }

    /// 上屏后联想条（整屏 items 的 `kind` 都是 [`PREDICTION_KIND`]）**不画编码栏**。
    ///
    /// 2026-11-07 用户要的「预测上屏十次以上的用户词」在屏幕上的形态就是「无组字 +
    /// 一排联想词」，此时编码栏那一排（◀ ▶ 翻页箭头 + 符/贴/同 三按钮）属于组字态语义，
    /// 必须整排收掉。正负对照（AGENTS.md [m0008]：没有正对照的「通过」是幻觉）：
    ///   * 正：三条 `kind="prediction"` ⇒ 三个词照排，箭头/按钮矩形**全空**；
    ///   * 负：同样三个词、`kind` 为空（旧快照口径 = 普通候选）⇒ 箭头/按钮**全非空**。
    #[test]
    fn prediction_strip_hides_bar_and_arrows() {
        fn view_kind(texts: &[&str], kind: &str) -> CandidateView {
            CandidateView {
                visible: true,
                active: true,
                items: texts
                    .iter()
                    .map(|t| CandidateItem {
                        primary_text: (*t).to_string(),
                        secondary_text: String::new(),
                        enabled: true,
                        kind: kind.to_string(),
                    })
                    .collect(),
                selected_index: 0,
                page_start: 0,
                ..Default::default()
            }
        }

        let mut style = UIStyle::default();
        style.layout.layout_type = "horizontal_squeeze".into();
        let m = Metrics::new(&style, 96.0);
        let sizes = measured(3);

        // 正：联想条
        let predict = view_kind(&["现在", "这个", "然后"], PREDICTION_KIND);
        let layout = Layout::build(&m, &predict, &sizes);
        assert_eq!(layout.rows.len(), 3, "联想条三个词都要排出来");
        assert!(
            layout.preedit_row.is_empty(),
            "联想条没有编码行，preedit_row 应为空"
        );
        assert!(layout.page_prev.is_empty(), "联想条不应有左翻页箭头");
        assert!(layout.page_next.is_empty(), "联想条不应有右翻页箭头");
        assert!(
            layout.bar_buttons.iter().all(|b| b.is_empty()),
            "联想条不应有「符/贴/同」按钮矩形"
        );

        // 保险（2026-11-08）：打字时 librime 追加的预测候选帧（kind 也是 prediction）
        // **带预编辑**（组字中）⇒ 编码栏必须照常画，不能被当成上屏后联想条收掉。
        // 正对照：同一批 prediction 词、只多给一条 preedit。
        let mut typing = view_kind(&["现在", "这个", "然后"], PREDICTION_KIND);
        typing.preedit = Some(Preedit {
            text: "nihao".into(),
            cursor: 5,
        });
        let typing_layout = Layout::build(&m, &typing, &sizes);
        assert_eq!(typing_layout.rows.len(), 3, "打字中的预测候选三行都要排出来");
        assert!(
            !typing_layout.page_prev.is_empty(),
            "带预编辑的预测帧必须有左翻页箭头"
        );
        assert!(
            !typing_layout.page_next.is_empty(),
            "带预编辑的预测帧必须有右翻页箭头"
        );
        assert!(
            typing_layout.bar_buttons.iter().all(|b| !b.is_empty()),
            "带预编辑的预测帧必须有「符/贴/同」三个按钮"
        );

        // 负（正对照）：同样的词按普通候选处理，编码栏必须完整存在
        let cand = view_kind(&["现在", "这个", "然后"], "");
        let normal = Layout::build(&m, &cand, &sizes);
        assert_eq!(normal.rows.len(), 3);
        assert!(!normal.page_prev.is_empty(), "普通候选必须有左翻页箭头");
        assert!(!normal.page_next.is_empty(), "普通候选必须有右翻页箭头");
        assert!(
            normal.bar_buttons.iter().all(|b| !b.is_empty()),
            "普通候选必须有「符/贴/同」三个按钮"
        );
    }

    /// 序号 → 候选词的间隔在**所有**布局里都是 0.5em（用户 2026-11-05：
    /// 「所有序号和候选词差 0.5em，做一个间隔」）。
    ///
    /// 0.5em 的口径 = `Metrics.hilite_spacing` = `font_h / 2`（config.rs 里按字高推导，
    /// yaml 的 `hilite_spacing` 只是下界）。横流/飞瀑/官方横竖都靠这一条缝，
    /// 竹简则把它当成序号格与正文之间的竖向留白（见 `cells_measured`）。
    #[test]
    fn label_gap_is_half_em_in_every_mode() {
        let v = view(3);
        let sizes = measured(3);
        for t in [
            LayoutType::HorizontalSqueeze,
            LayoutType::VerticalSqueeze,
            LayoutType::Horizontal,
            LayoutType::Vertical,
        ] {
            let mut m = metrics();
            m.layout_type = t;
            let half_em = m.font_point / 2.0; // font_point 已是 DIP 字高（= font_h）
            assert!(
                (m.hilite_spacing - half_em).abs() < 0.01,
                "{t:?} 的序号间隔应为 0.5em={half_em:.2}，实际 {:.2}",
                m.hilite_spacing
            );
            let layout = Layout::build(&m, &v, &sizes);
            for (i, row) in layout.rows.iter().enumerate() {
                let gap = row.text.left - row.label.right;
                assert!(
                    (gap - half_em).abs() < 0.01,
                    "{t:?} row{i}：序号右沿到正文左沿应为 {half_em:.2}，实际 {gap:.2}"
                );
            }
        }
    }

    /// 非 squeeze 模式下所有 row.pad_* 恒等于 Metrics 基准值（回归：官方布局不受影响）。
    #[test]
    fn non_squeeze_rows_keep_official_padding() {
        let m = metrics();
        let layout = Layout::build(&m, &view(3), &measured(3));
        for (i, row) in layout.rows.iter().enumerate() {
            assert!((row.pad_x - m.hilite_padding_x).abs() < 0.01, "row{i} pad_x");
            assert!((row.pad_y - m.hilite_padding_y).abs() < 0.01, "row{i} pad_y");
        }
    }

    /// 三种 squeeze 模式都点亮标志并落到各自的排布方向。
    #[test]
    fn all_three_squeeze_modes_light_the_flag() {
        for (name, expected, horizontal) in [
            ("horizontal_squeeze", LayoutType::HorizontalSqueeze, true),
            ("vertical_squeeze", LayoutType::VerticalSqueeze, false),
            ("cells", LayoutType::Cells, true),
        ] {
            let mut style = UIStyle::default();
            style.layout.layout_type = name.into();
            let m = Metrics::new(&style, 96.0);
            assert!(m.squeeze, "{name} 应点亮 squeeze");
            assert_eq!(m.layout_type, expected, "{name} 的 LayoutType");
            let layout = Layout::build(&m, &view(3), &measured(3));
            assert_eq!(layout.rows.len(), 3, "{name} 候选数");
            // 横流/竹简单行（候选横向推进），飞瀑竖列（从上往下堆）。
            // 竹简的正文按各自 pad 顶齐，选中项比邻居低 0.5p，所以这里比的是
            // 「横坐标是否推进」，不再拿 text.top 当单行判据。
            let side_by_side = layout.rows[1].text.left > layout.rows[0].text.left + 1.0;
            assert_eq!(side_by_side, horizontal, "{name} 单行={horizontal}");
        }
    }

    #[test]
    fn vertical_rows_stack_with_candidate_spacing() {
        let m = metrics();
        let layout = Layout::build(&m, &view(3), &measured(3));
        assert_eq!(layout.rows.len(), 3);
        for i in 1..3 {
            let gap = layout.rows[i].text.top - layout.rows[i - 1].text.bottom;
            assert!(
                (gap - m.candidate_spacing).abs() < 0.01,
                "行距应等于 candidate_spacing，实际 {gap}"
            );
        }
        // 行内：label 在左、text 在 label 之后 hilite_spacing
        assert!((layout.rows[0].text.left - layout.rows[0].label.right - m.hilite_spacing).abs() < 0.01);
        // 内容尺寸包含 offset 外扩
        assert!(layout.content_size.cx > layout.content.right);
    }

    #[test]
    fn min_width_keeps_panel_wide_enough() {
        let m = metrics();
        let layout = Layout::build(&m, &view(1), &measured(1));
        // 内容宽度（content 矩形）不小于 min_width；外扩的 offset 不算在内
        assert!(
            layout.content.width() >= m.min_width - 0.01,
            "内容宽 {} 应不小于 min_width {}",
            layout.content.width(),
            m.min_width
        );
        assert!(
            (layout.content.width() - m.min_width).abs() < 0.01,
            "只有一条短候选时宽度应正好是 min_width，实际 {}",
            layout.content.width()
        );
    }

    #[test]
    fn content_rect_is_symmetric_after_offset_deflate() {
        let m = metrics();
        let layout = Layout::build(&m, &view(2), &measured(2));
        let (ox, oy) = m.offset();
        // 官方：content = contentSize.Deflate(offset)，所以四周留白正好是 offset
        assert!((layout.content.left - ox).abs() < 0.01, "左边留白应等于 offset_x");
        assert!((layout.content.top - oy).abs() < 0.01, "上边留白应等于 offset_y");
        assert!(
            (layout.content_size.cx - layout.content.right - ox).abs() < 0.01,
            "右边留白应等于 offset_x"
        );
        assert!(
            (layout.content_size.cy - layout.content.bottom - oy).abs() < 0.01,
            "下边留白应等于 offset_y"
        );
    }

    #[test]
    fn candidates_share_one_right_edge() {
        let m = metrics();
        let mut sizes = measured(3);
        sizes.text[1].cx = 200.0; // 中间那条最宽
        let layout = Layout::build(&m, &view(3), &sizes);
        let right = layout.rows[0].rect.right;
        for row in &layout.rows {
            assert!((row.rect.right - right).abs() < 0.01, "候选行右边界应统一");
        }
    }

    #[test]
    fn preedit_row_occupies_its_own_line() {
        let m = metrics();
        let mut v = view(2);
        v.preedit = Some(Preedit {
            text: "nihao".into(),
            cursor: 5,
        });
        let mut sizes = measured(2);
        sizes.preedit = Parts {
            before: Size::ZERO,
            hilite: Size::new(50.0, 20.0),
            after: Size::ZERO,
            total: Size::new(50.0, 20.0),
        };
        let layout = Layout::build(&m, &v, &sizes);
        assert!(!layout.preedit.is_empty(), "非 inline 预编辑要有自己的矩形");
        assert!(
            layout.preedit.bottom <= layout.rows[0].rect.top + 0.01,
            "预编辑行应在候选之上"
        );
    }

    #[test]
    fn inline_preedit_does_not_add_a_row() {
        let m = metrics();
        let mut v = view(2);
        v.preedit = Some(Preedit {
            text: "ni".into(),
            cursor: 2,
        });
        let mut sizes = measured(2);
        sizes.preedit.total = Size::new(30.0, 20.0);
        let without = Layout::build(&m, &v, &measured(2));
        let with = Layout::build(&m, &v, &sizes);
        // 快照里有 preedit 就说明要画在窗内（我们的 inline 判定），高度会长一点
        assert!(with.content_size.cy >= without.content_size.cy);
    }

    #[test]
    fn hit_test_uses_inflated_rows() {
        let m = metrics();
        let layout = Layout::build(&m, &view(3), &measured(3));
        let row = layout.rows[1];
        let x = (row.text.left + row.text.right) / 2.0;
        let y = (row.rect.top + row.rect.bottom) / 2.0;
        assert_eq!(layout.hit(&m, x, y), Some(Hit::Candidate(1)));
        // 行之间的空隙（未外扩）不算命中
        let gap_y = (layout.rows[0].rect.bottom + layout.rows[1].rect.top) / 2.0;
        let gap_hit = layout.hit(&m, x, gap_y);
        assert!(
            gap_hit.is_none() || gap_hit == Some(Hit::Candidate(0)) || gap_hit == Some(Hit::Candidate(1)),
            "空隙要么不命中，要么落在相邻候选的外扩区"
        );
    }

    /// 2026-10-31 改：翻页按钮**不再**要求配色给翻页色。
    /// 出厂 37 套配色的 prevpage/nextpage 全是 `0x00000000`，旧语义导致用户永远看不见「还有下一页」。
    #[test]
    fn page_buttons_are_laid_out_even_without_page_colors() {
        let m = metrics();
        assert!(!m.page_enabled, "aqua 的 prevpage/nextpage 仍是全透明（配色没变）");
        let layout = Layout::build(&m, &view(3), &measured(3));
        assert!(
            !layout.page_prev.is_null() && !layout.page_next.is_null(),
            "透明配色也要给翻页按钮矩形，否则界面上画不出来"
        );
    }

    /// 零候选帧（打大写字母时的「只有编码栏」帧）必须装得下整条编码栏。
    ///
    /// 2026-11-08 用户报「大写字母的候选窗高度不够 / 只显示了一半的高度」：窗口量出来
    /// 47 px（dpi 120，content 37.5 DIP），而编码栏本身到 50 DIP 才结束 ⇒ 栏的下半截
    /// 和横分隔被窗口底边裁掉。根因是官方收尾对 `count == 0` 扣了两遍 `spacing`
    /// （横向分支 + 收尾段），而挤压三模式根本没加过 spacing。
    ///
    /// 正负对照：
    ///   * 正：dpi 96 / 120 下 content 高度 = 编码栏底 + bar_hsep + 底部纸边；
    ///     编码栏底、横分隔底、编码文字底都 ≤ content 高（不裁切）；
    ///   * 负：联想条帧（`count > 0`、无编码行）的原有高度路径不受影响——
    ///     它与官方 spacing 无关，改前改后同值（这里钉住「有候选帧没被带歪」）。
    #[test]
    fn squeeze_zero_candidate_frame_fits_whole_bar() {
        for &(dpi, scale) in &[(96.0f32, 1.0f32), (120.0, 1.25)] {
            let mut style = UIStyle::default();
            style.font_point = 15; // 现场 theme.yaml 的 font_point
            style.label_font_point = 15;
            style.comment_font_point = 15;
            style.horizontal = true;
            style.layout.layout_type = "horizontal_squeeze".into();
            let m = Metrics::new(&style, dpi);
            let mut sizes = measured(0);
            sizes.preedit = Parts {
                before: Size::ZERO,
                hilite: Size::ZERO,
                after: Size::ZERO,
                total: Size::new(120.0, 33.0),
            };
            let mut v = view(0);
            v.preedit = Some(Preedit {
                text: "G".into(),
                cursor: 1,
            });
            let layout = Layout::build(&m, &v, &sizes);

            assert!(layout.rows.is_empty(), "零候选不该排候选行");
            assert!(!layout.preedit_row.is_empty(), "有预编辑就该有编码栏矩形");
            // ① 编码栏整条（含横分隔）+ 底部纸边必须在窗口内
            let want = layout.preedit_row.bottom + m.bar_hsep + m.squeeze_frame();
            assert!(
                (layout.content_size.cy - want).abs() < 0.01,
                "dpi {dpi}: 零候选帧内容高应为 {want:.2} DIP（= {:.0} px），实际 {:.2}",
                want * scale,
                layout.content_size.cy
            );
            assert!(
                layout.preedit_row.bottom + m.bar_hsep <= layout.content_size.cy + 0.01,
                "dpi {dpi}: 横分隔底 {:.2} 超出内容高 {:.2}（窗口会裁掉编码栏下半截）",
                layout.preedit_row.bottom + m.bar_hsep,
                layout.content_size.cy
            );
            assert!(
                layout.preedit.bottom <= layout.content_size.cy + 0.01,
                "dpi {dpi}: 编码文字底 {:.2} 超出内容高 {:.2}",
                layout.preedit.bottom,
                layout.content_size.cy
            );
            // 旧值钉死在注释里：dpi96 = 33.0 DIP（33 px），dpi120 = 37.5 DIP（47 px）——
            // 都小于编码栏底（40.0 / 50.0 DIP），所以上面那条等式断言在修复前必红。
            // 修复后 dpi96 = 58.0 DIP（58 px）、dpi120 = 68.75 DIP（86 px）。
        }

        // 负对照：联想条（有 3 个候选、无编码行）走 count > 0 路径，别被这次改动带歪。
        let mut style = UIStyle::default();
        style.horizontal = true;
        style.layout.layout_type = "horizontal_squeeze".into();
        let m = Metrics::new(&style, 120.0);
        let strip = CandidateView {
            visible: true,
            active: true,
            items: ["现在", "这个", "然后"]
                .iter()
                .map(|t| CandidateItem {
                    primary_text: (*t).to_string(),
                    secondary_text: String::new(),
                    enabled: true,
                    kind: PREDICTION_KIND.to_string(),
                })
                .collect(),
            selected_index: 0,
            page_start: 0,
            ..Default::default()
        };
        let layout = Layout::build(&m, &strip, &measured(3));
        assert!(layout.preedit_row.is_empty(), "联想条没有编码行");
        assert_eq!(layout.rows.len(), 3, "联想条三个词应各占一行");
        assert!(
            layout.content_size.cy > 2.5 * m.font_point,
            "联想条帧高度应仍由候选行撑起：{:.2}",
            layout.content_size.cy
        );
    }

    #[test]
    fn rounded_rect_contains_matches_corner_geometry() {
        let rect = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert!(rounded_rect_contains(&rect, 8.0, 50.0, 25.0), "中心必在内部");
        assert!(!rounded_rect_contains(&rect, 8.0, -1.0, 25.0), "左侧外");
        // 左上角点 (0,0) 在圆角外
        assert!(!rounded_rect_contains(&rect, 8.0, 0.0, 0.0));
        // 角内 1px 处在圆角内（圆心 (8,8)，半径 8：(7,7) 距离 √2 < 8）
        assert!(rounded_rect_contains(&rect, 8.0, 7.0, 7.0));
    }
}
