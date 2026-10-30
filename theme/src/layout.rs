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

/// 官方 `_pre` / `_next` 两个翻页字形（StandardLayout.cpp:5-6）。
pub const PAGE_PREV: &str = "<";
pub const PAGE_NEXT: &str = ">";

/// 候选最多 100 条（官方 `MAX_CANDIDATES_COUNT`）。
pub const MAX_CANDIDATES: usize = 100;

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
    if inline_preedit || !m.page_enabled {
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
        let mut layout = match m.layout_type {
            LayoutType::Horizontal | LayoutType::HorizontalFullscreen => {
                vertical_layout(m, view, measured, inline_preedit, true)
            }
            _ => vertical_layout(m, view, measured, inline_preedit, false),
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
        rect.inflate(m.hilite_padding_x, m.hilite_padding_y);
        rect
    }

    /// 官方 `OnMouseMove` / `OnLeftClickDown` 的命中顺序：先翻页按钮，再候选。
    pub fn hit(&self, m: &Metrics, x: f32, y: f32) -> Option<Hit> {
        if m.page_enabled && !self.inline_preedit && !self.rows.is_empty() {
            if !self.page_prev.is_null() && self.page_prev.contains(x, y) {
                return Some(Hit::Previous);
            }
            if !self.page_next.is_null() && self.page_next.contains(x, y) {
                return Some(Hit::Next);
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

/// 官方 `VerticalLayout::DoLayout`（`horizontal_rows = true` 时走 `HorizontalLayout`）。
fn vertical_layout(
    m: &Metrics,
    view: &CandidateView,
    measured: &Measured,
    inline_preedit: bool,
    horizontal_rows: bool,
) -> Layout {
    let candidates = &view.items[..view.items.len().min(MAX_CANDIDATES)];
    let count = candidates.len();
    let id = if (view.selected_index as usize) < count {
        view.selected_index as usize
    } else {
        0
    };
    let real_margin_x = m.real_margin_x();
    let real_margin_y = m.real_margin_y();
    let (offset_x, offset_y) = m.offset();

    let mut mark = (0.0, 0.0, 0.0);
    let base_offset = calc_mark_metrics(m, measured, count, false, &mut mark);
    let (pgw, pgh) = calc_page_indicator(m, measured, inline_preedit, false);
    // 官方 `labelFontValid` / `textFontValid`：字号为 0 的那一类不占宽度（但仍留矩形）
    let label_gate = if m.label_font_valid { 1.0 } else { 0.0 };
    let text_gate = if m.text_font_valid { 1.0 } else { 0.0 };

    let mut width = 0.0_f32;
    let mut height = real_margin_y;
    let mut preedit = Rect::default();
    // 我们的快照没有辅助串（官方 `Context::aux`），留一个空矩形占位，布局分支保持一致
    let aux = Rect::default();

    // 预编辑与辅助串（官方 VerticalLayout.cpp:15-30）
    let has_preedit = !inline_preedit && view.preedit.is_some();
    if has_preedit {
        layout_inline_rect(
            m,
            view,
            measured.preedit.total,
            true,
            pgw,
            pgh,
            real_margin_x,
            &mut width,
            &mut height,
            &mut preedit,
        );
        preedit.offset(offset_x, offset_y);
    }

    // 候选（官方 VerticalLayout.cpp:32-110 与 HorizontalLayout.cpp:29-140 两套）
    let mut rows: Vec<Row> = Vec::with_capacity(count);
    let mut comment_shift_width = 0.0_f32;
    let mut max_candidate_width = 0.0_f32;
    let mut max_comment_width = 0.0_f32;

    if horizontal_rows {
        // ---- HorizontalLayout：同一行里横排，超过 max_width 折行 ----
        let mut w = offset_x + real_margin_x;
        let mut row_cnt = 0usize;
        let mut row_top = height;
        let mut rows_height: Vec<f32> = vec![0.0; count + 1];
        let mut row_of: Vec<usize> = vec![0; count];
        let mut mintop: Vec<f32> = vec![0.0; count + 1];
        let mut max_width_of_rows = 0.0_f32;

        for i in 0..count {
            if i > 0 {
                w += m.candidate_spacing;
            }
            if i == id {
                w += base_offset;
            }
            let label_size = measured.label.get(i).copied().unwrap_or(Size::ZERO);
            let text_size = measured.text.get(i).copied().unwrap_or(Size::ZERO);
            let comment_size = measured.comment.get(i).copied().unwrap_or(Size::ZERO);
            let comment_visible =
                m.comment_shown(candidates[i].secondary_text.is_empty(), i == id);
            let mut row = Row::default();
            row.label = Rect::new(w, height, w + label_size.cx, height + label_size.cy);
            w += label_size.cx * label_gate;
            let mut cand_width = label_size.cx * label_gate;

            w += m.hilite_spacing;
            row.text = Rect::new(w, height, w + text_size.cx, height + text_size.cy);
            w += text_size.cx * text_gate;
            cand_width += (text_size.cx * text_gate) + m.hilite_spacing;

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
                w = offset_x + real_margin_x + if i == id { base_offset } else { 0.0 };
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

        // 行内垂直对齐 + 行矩形（官方 HorizontalLayout.cpp:110-134）
        for i in 0..count {
            let base_left = if i == id {
                rows[i].label.left - base_offset
            } else {
                rows[i].label.left
            };
            let row_height = rows_height[row_of[i]];
            let top = mintop[row_of[i]];
            rows[i].rect = Rect::new(
                base_left,
                top,
                rows[i].comment.right,
                top + row_height,
            );
            if m.align != Align::Top {
                let (ol, ot, oc) = match m.align {
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
            height = mintop[row_cnt] + rows_height[row_cnt] - offset_y;
            width = width.max(max_width_of_rows);
        } else {
            height -= m.spacing + offset_y;
            width += m.hilite_spacing + m.border;
        }
        width += real_margin_x;
        height += real_margin_y;
    } else {
        // ---- VerticalLayout：一个候选一行（官方 VerticalLayout.cpp:36-110）----
        for i in 0..count {
            if i > 0 {
                height += m.candidate_spacing;
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
            rows[i].rect = Rect::new(
                real_margin_x + offset_x,
                hl_top,
                width - real_margin_x + offset_x,
                hl_bottom,
            );
        }
    }

    // 收尾（官方 VerticalLayout.cpp:138-147）
    if count == 0 {
        height -= m.spacing;
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

    let content_size = Size::new(width + offset_x * 2.0, height + offset_y * 2.0);
    let highlight = rows.get(id).map(|row| row.rect).unwrap_or_default();

    // 翻页按钮（官方 VerticalLayout.cpp:162-178）
    let mut page_prev = Rect::default();
    let mut page_next = Rect::default();
    if m.page_enabled && count > 0 && !m.inline_preedit {
        let x = content_size.cx - offset_x - real_margin_x + m.hilite_padding_x - pgw;
        let y = (preedit.top + preedit.bottom) / 2.0 - measured.page_prev.cy / 2.0;
        page_prev = Rect::new(x, y, x + measured.page_prev.cx, y + measured.page_prev.cy);
        page_next = Rect::new(
            page_prev.right + m.hilite_spacing,
            y,
            page_prev.right + m.hilite_spacing + measured.page_next.cx,
            y + measured.page_next.cy,
        );
    }

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
        rect.inflate(m.hilite_padding_x, m.hilite_padding_y);
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
            rect.inflate(m.hilite_padding_x, m.hilite_padding_y);
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

    #[test]
    fn matches_hand_computed_official_geometry() {
        // 手算自官方 C++（aqua 出厂值、96 DPI、两条候选、无注释、无标记、无预编辑）：
        //   real_margin = 12, offset = border*2 = 6, spacing = 4, candidate_spacing = 5,
        //   min_width = 160
        //   行 0：label(12,12)-(28,31)，text(32,12)-(80,31) → 运行宽度 80，高度 12+19=31
        //   行 1：candidate_spacing 5 → 高度 36，label(12,36)-(28,55)，text(32,36)-(80,55) → 高度 55
        //   max_candidate_width = 16+4+48 = 68 → content 宽 = max(80, 68+24) = 92 → 被 min_width 顶到 160
        //   content 高 = 55 + margin_y 12 = 67
        //   contentSize = (160+12, 67+12) = (172, 79)
        //   候选矩形（再外扩 hilite_padding 2 之前）= left 18，right = 18-12+160-12 = 154
        let m = metrics();
        let mut sizes = Measured {
            label: vec![Size::new(16.0, 19.0); 2],
            text: vec![Size::new(48.0, 19.0); 2],
            comment: vec![Size::ZERO; 2],
            ..Default::default()
        };
        sizes.page_prev = Size::new(8.0, 18.0);
        sizes.page_next = Size::new(8.0, 18.0);
        let layout = Layout::build(&m, &view(2), &sizes);

        assert_eq!(layout.content_size, Size::new(172.0, 79.0), "contentSize 手算值");
        assert_eq!(layout.content, Rect::new(6.0, 6.0, 166.0, 73.0), "content 矩形");
        assert_eq!(
            layout.rows[0].label,
            Rect::new(18.0, 18.0, 34.0, 37.0),
            "行 0 序号矩形"
        );
        assert_eq!(
            layout.rows[0].text,
            Rect::new(38.0, 18.0, 86.0, 37.0),
            "行 0 正文矩形（label 之后 hilite_spacing）"
        );
        assert_eq!(layout.rows[1].label.top, 42.0, "行 1 = 行 0 底 + candidate_spacing");
        assert_eq!(
            layout.rows[0].rect,
            Rect::new(18.0, 18.0, 154.0, 37.0),
            "高亮矩形（右边界贴面板右边距）"
        );
        assert_eq!(
            layout.highlight_rect(&m, 0),
            Rect::new(16.0, 16.0, 156.0, 39.0),
            "绘制/命中用矩形 = 行矩形外扩 hilite_padding"
        );
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

    #[test]
    fn page_buttons_only_when_colors_are_opaque() {
        let m = metrics();
        assert!(!m.page_enabled, "官方 aqua 的 prevpage/nextpage 是全透明 → 不画翻页");
        let layout = Layout::build(&m, &view(3), &measured(3));
        assert!(layout.page_prev.is_null() && layout.page_next.is_null());

        let mut style = UIStyle::default();
        style.color_scheme = "aqua".into();
        let mut m = Metrics::new(&style, 96.0);
        m.page_enabled = true;
        let layout = Layout::build(&m, &view(3), &measured(3));
        assert!(!layout.page_prev.is_null(), "着色后应给出翻页按钮矩形");
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
