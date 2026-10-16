//! ten 候选栏的纯布局、命中测试、配色和鼠标手势逻辑。

use crate::theme_api::{CandidateView, UiAction};

/// 设计基准到 DIP 的缩放比例。
pub const SCALE: f32 = 46.0 / 68.0;
/// 候选栏在设计坐标中的高度，单位为 DIP。
pub const HEIGHT: f32 = 46.0;
/// 中英文模式提示方框的边长，单位为 DIP。
pub const INDICATOR_SIZE: f32 = 36.0;
/// 候选序号列宽，单位为 DIP。
pub const NUMBER: f32 = 40.0 * SCALE;
/// 候选项右侧留白，单位为 DIP。
pub const PAD: f32 = 18.0 * SCALE;


/// Direct2D 设备丢失后的有限重试状态。
///
/// 每次失败最多安排一次延迟重绘；成功绘制会清零连续失败计数。
#[derive(Default)]
pub struct Recovery {
    /// 当前连续设备丢失次数；预算上限为三次。
    failures: u8,
    /// 定时重试尚未触发时为真，窗口过程据此暂缓绘制。
    pub waiting: bool,
}
impl Recovery {
    /// 记录失败；仅设备丢失且尚有预算时返回 `true` 并进入等待状态。
    pub fn failed(&mut self, device_lost: bool) -> bool {
        if !device_lost || self.failures >= 3 {
            return false;
        }
        self.failures += 1;
        self.waiting = true;
        true
    }
    /// 记录一次成功绘制并重置失败预算与等待状态。
    pub fn succeeded(&mut self) {
        self.failures = 0;
        self.waiting = false;
    }
}

/// 将 DIP 长度转换为物理像素，DPI 至少按 1 处理，结果至少为 1 像素。
pub fn pixels(dip: f32, dpi: u32) -> i32 {
    (dip * dpi.max(1) as f32 / 96.0).round().max(1.0) as i32
}

/// 候选栏中可被鼠标命中的语义区域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// 从零开始的候选项索引。
    Candidate(usize),
    /// 请求上一页候选项。
    Previous,
    /// 请求下一页候选项。
    Next,
    /// 打开表情面板。
    Emoji,
}

/// 单个可命中区域的横向 DIP 边界。
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    /// 此区域对应的候选项或操作。
    pub hit: Hit,
    /// 左边界，包含。
    pub left: f32,
    /// 右边界，不包含。
    pub right: f32,
}

/// 轴对齐矩形（DIP）。官方用 `CRect`，字段含义一致。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn width(&self) -> f32 {
        self.right - self.left
    }
    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// 一行候选的四个矩形（对应官方 StandardLayout 的 label/text/comment/candidate 四组数组）。
#[derive(Clone, Copy, Debug, Default)]
pub struct Row {
    /// 序号矩形。
    pub label: Rect,
    /// 主文本矩形。
    pub text: Rect,
    /// 注释矩形；无注释时为全 0。
    pub comment: Rect,
    /// 高亮矩形 = label / text / comment 上下界的并集（官方 L94-106）。
    pub rect: Rect,
}

/// 按绘制顺序排列的栏位及其总宽度，全部以 DIP 表示。
/// 官方 `VerticalLayout::DoLayout` 的 Rust 版：**纵向**候选栏布局。
///
/// 与官方一一对应（行号见 `theme\official\reference\VerticalLayout.cpp`）：
/// * 每个候选**一行**；每行有 label / text / comment 三个矩形，高亮矩形取三者上下界的并集（L94-106）；
/// * 行间 `candidate_spacing`、序号与文本间 `hilite_spacing`（L38 / L47）；
/// * 尺寸按 `Layout.cpp:44-60` 的根公式：`real_margin = max(|margin|, hilite_padding)`、
///   `offset = border*2`、`contentSize = (width + offset*2, height + offset*2)`（L158）。
///
/// 布局值目前用与官方 aqua 等价的常量；接 `theme.toml` 的 `Metrics` 是下一步
/// （见 `doc\official-layout-reference.md` 第六节）。
#[derive(Default)]
pub struct Layout {
    /// 命中用：每个候选的水平区间（保留给 mod.rs 的兼容用法）。
    pub cells: Vec<Cell>,
    /// 内容宽度（含左右 margin，不含 offset 外扩）。
    pub width: f32,
    /// 内容高度（含上下 margin）。
    pub height: f32,
    /// 每行候选的四个矩形（官方四组数组的对应物）。
    pub rows: Vec<Row>,
    /// 预编辑行矩形（官方 `GetPreeditRect`）；无预编辑时为空矩形。
    pub preedit: Rect,
    /// 辅助串矩形（官方 `GetAuxiliaryRect`）。
    pub aux: Rect,
    /// 内容矩形（官方 `GetContentRect`）：面板底与描边都用它。
    pub content: Rect,
}

impl Layout {
    /// 为无候选的中英文模式提示创建一个不可交互的紧凑栏位。
    pub fn mode_indicator() -> Self {
        Self {
            cells: Vec::new(),
            width: INDICATOR_SIZE,
            height: INDICATOR_SIZE,
            rows: Vec::new(),
            preedit: Rect::default(),
            aux: Rect::default(),
            content: Rect { left: 0.0, top: 0.0, right: INDICATOR_SIZE, bottom: INDICATOR_SIZE },
        }
    }

    /// 按官方 `VerticalLayout::DoLayout` 构造纵向布局。
    ///
    /// `widths` 是每个候选**主文本 + 注释**的实测宽度（顺序与候选一致），
    /// `preedit_width` 为预编辑串的实测宽度（0 表示不显示）。
    pub fn new_with_preedit(
        widths: impl IntoIterator<Item = f32>,
        preedit_width: f32,
    ) -> Self {
        // 官方 Layout.cpp:44-60 的根公式（aqua 取值）。
        const MARGIN_X: f32 = 12.0_f32.max(2.0); // max(margin_x, hilite_padding_x)
        const MARGIN_Y: f32 = 12.0_f32.max(2.0); // max(margin_y, hilite_padding_y)
        const OFFSET: f32 = 3.0 * 2.0; // border * 2
        const ROW_GAP: f32 = 5.0; // candidate_spacing
        const LABEL_GAP: f32 = 4.0; // hilite_spacing
        const MIN_WIDTH: f32 = 160.0; // min_width
        const LINE: f32 = HEIGHT; // 行高（官方按字体实测，这里用设计基准）

        let mut result = Self::default();
        let mut y = OFFSET + MARGIN_Y;
        let mut widest = MIN_WIDTH;

        // 预编辑行（官方 L18-22）：非 inline 且有内容时占一行。
        if preedit_width > 0.0 {
            let rect = Rect {
                left: OFFSET + MARGIN_X,
                top: y,
                right: OFFSET + MARGIN_X + preedit_width,
                bottom: y + LINE,
            };
            result.preedit = rect;
            widest = widest.max(MARGIN_X * 2.0 + preedit_width);
            y += LINE + ROW_GAP;
        }

        for (index, text) in widths.into_iter().enumerate() {
            if index > 0 {
                y += ROW_GAP; // 官方 L38
            }
            let label = Rect {
                left: OFFSET + MARGIN_X,
                top: y,
                right: OFFSET + MARGIN_X + NUMBER,
                bottom: y + LINE,
            };
            let text_rect = Rect {
                left: label.right + LABEL_GAP, // 官方 L47：label 之后加 hilite_spacing
                top: y,
                right: label.right + LABEL_GAP + text.ceil(),
                bottom: y + LINE,
            };
            // 官方 L94-106：高亮矩形 = label / text / comment 三者上下界的并集（此处无注释）。
            let row_rect = Rect {
                left: label.left,
                top: label.top.min(text_rect.top),
                right: text_rect.right,
                bottom: label.bottom.max(text_rect.bottom),
            };
            widest = widest.max(MARGIN_X * 2.0 + NUMBER + LABEL_GAP + text.ceil());
            result.cells.push(Cell { hit: Hit::Candidate(index), left: row_rect.left, right: row_rect.right });
            result.rows.push(Row { label, text: text_rect, comment: Rect::default(), rect: row_rect });
            y += LINE;
        }

        // 官方 L142-147：收尾加下边距；有候选时受 min_width 约束。
        if !result.rows.is_empty() {
            y += MARGIN_Y;
        } else {
            y = OFFSET + MARGIN_Y + LINE + MARGIN_Y;
        }
        result.width = widest;
        result.height = y;
        // 官方 L158/L201：内容矩形与内容尺寸都含 offset 外扩。
        result.content = Rect {
            left: OFFSET,
            top: OFFSET,
            right: OFFSET + result.width,
            bottom: OFFSET + result.height,
        };
        result.width += OFFSET * 2.0;
        result.height += OFFSET * 2.0;
        result
    }

    /// 兼容旧签名：没有预编辑串。
    pub fn new(widths: impl IntoIterator<Item = f32>) -> Self {
        Self::new_with_preedit(widths, 0.0)
    }

    /// 按行的矩形做命中测试（官方 `OnMouseMove` 用 `GetCandidateRect(i)` 逐个 `PtInRect`）。
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        for (index, row) in self.rows.iter().enumerate() {
            if row.rect.contains(x, y) {
                return Some(Hit::Candidate(index));
            }
        }
        for cell in &self.cells {
            if x >= cell.left && x < cell.right {
                return Some(cell.hit);
            }
        }
        None
    }
}
pub fn enabled(snapshot: &CandidateView, hit: Hit) -> bool {
    snapshot.visible
        && match hit {
            Hit::Candidate(i) => snapshot.items.get(i).is_some_and(|item| item.enabled),
            Hit::Previous => snapshot.can_page_previous,
            Hit::Next => snapshot.can_page_next,
            Hit::Emoji => true,
        }
}

/// 按下、移动、释放组成的鼠标手势状态。
///
/// 按下目标一旦因指针离开而取消，移回不会恢复；只有在同一有效目标上释放才产生动作。
#[derive(Default)]
pub struct Gesture {
    /// 当前仍有效的按下目标。
    pub pressed: Option<Hit>,
    /// 指针当前位置对应的目标，用于悬停绘制。
    pub hovered: Option<Hit>,
}
impl Gesture {
    /// 清空按下与悬停状态，用于失焦、捕获丢失或内容替换。
    pub fn cancel(&mut self) {
        self.pressed = None;
        self.hovered = None;
    }
    /// 仅当目标在当前快照中可用时记录按下状态。
    pub fn press(&mut self, hit: Option<Hit>, snapshot: &CandidateView) {
        self.pressed = hit.filter(|h| enabled(snapshot, *h));
    }
    /// 更新悬停目标；离开按下目标后永久取消本次点击。
    pub fn motion(&mut self, hit: Option<Hit>) {
        self.hovered = hit;
        // Once the pointer leaves its pressed target, returning cannot invoke it.
        if self.pressed != hit {
            self.pressed = None;
        }
    }
    /// 完成手势；仅同一仍启用的目标返回对应 UI 动作。
    ///
    /// 无有效按下、按下与释放目标不一致或目标已禁用时均返回 `None`；无论是否成功，
    /// 本次按下状态都会被消费。
    pub fn release(&mut self, hit: Option<Hit>, snapshot: &CandidateView) -> Option<UiAction> {
        let pressed = self.pressed.take()?;
        if Some(pressed) != hit || !enabled(snapshot, pressed) {
            return None;
        }
        Some(match pressed {
            Hit::Candidate(i) => UiAction::ItemInvoked(i as u32),
            Hit::Previous => UiAction::NavigatePrevious,
            Hit::Next => UiAction::NavigateNext,
            Hit::Emoji => UiAction::OpenEmojiPanel,
        })
    }
}

/// 亮色或暗色主题使用的 RGB 色板，颜色以 `0xRRGGBB` 表示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// 栏体背景色。
    pub background: u32,
    /// 外框及分隔线颜色。
    pub border: u32,
    /// 当前选中项的背景色。
    pub active: u32,
    /// 鼠标悬停背景色。
    pub hover: u32,
    /// 主文本颜色。
    pub text: u32,
    /// 次要文本颜色。
    pub secondary: u32,
    /// 选中项序号颜色。
    pub active_number: u32,
    /// 禁用内容颜色。
    pub disabled: u32,
}
impl Palette {
    /// 根据系统外观选择一组完整色板。
    ///
    /// 色板对齐 **Windows 7 Aero 玻璃**：底色偏冷蓝、外框蓝灰、内描边亮白，
    /// 选中项是"上浅下深"的淡蓝渐变块（Aero 的招牌观感）。
    pub fn new(dark: bool) -> Self {
        // 用户要求：面板**黑色**（深色玻璃），候选**高亮**（纯白文字 + 亮蓝选中块）。
        // dark/light 两个分支都按黑底设计，只是黑度略不同，避免跟随系统变回白底 ✗。
        if dark {
            Self {
                background: 0xEAF3FB,   // ★ Win7 磨砂玻璃：冰蓝白 ✓
                border: 0x93B4D4,   // 蓝灰外框 ✓
                active: 0xCFE7F9,   // Win7 选中：淡蓝 ✓
                hover: 0x2C5F8C,
                text: 0x14202B,   // 深墨蓝文字（磨砂白底上 ✓）
                secondary: 0x4A5A6A,
                active_number: 0x2C4A66,   // 深蓝序号 ✓
                disabled: 0x707070,
            }
        } else {
            Self {
                background: 0xE4EFFA,   // ★ 磨砂玻璃（暗色分支）✓
                border: 0x9CBCD9,
                active: 0xD6EBFB,
                hover: 0x2C5F8C,   // 悬停与选中同色（用户要求 ✓）
                text: 0x14202B,   // 深墨蓝文字（磨砂白底上 ✓）
                secondary: 0x46586A,
                active_number: 0x2C4A66,   // 深蓝序号 ✓
                disabled: 0x787878,
            }
        }
    }

    /// Aero 选中项的下半段颜色（上浅下深渐变的下端）。
    pub fn active_bottom(dark: bool) -> u32 {
        if dark { 0x1B4F86 } else { 0x2A5F9E }
    }

    /// Aero 选中项的描边色（比外框更亮的蓝）。
    pub fn active_border(dark: bool) -> u32 {
        if dark { 0x5FA8E8 } else { 0x6FB4F0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme_api::CandidateItem as RenderItem;
    fn snapshot() -> CandidateView {
        CandidateView {
            visible: true,
            content_id: 7,
            items: vec![RenderItem {
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }
    #[test]
    fn height_at_display_scales() {
        assert_eq!(
            [pixels(HEIGHT, 96), pixels(HEIGHT, 144), pixels(HEIGHT, 192)],
            [46, 69, 92]
        );
        let indicator = Layout::mode_indicator();
        assert_eq!((indicator.width, indicator.height), (36.0, 36.0));
        assert_eq!(
            [
                pixels(indicator.height, 96),
                pixels(indicator.height, 144),
                pixels(indicator.height, 192),
            ],
            [36, 54, 72]
        );
    }
    #[test]
    fn recovery_is_bounded_and_success_resets_budget() {
        let mut r = Recovery::default();
        for _ in 0..3 {
            assert!(r.failed(true));
            assert!(r.waiting);
            r.waiting = false;
        }
        assert!(!r.failed(true));
        r.succeeded();
        assert!(r.failed(true));
        r.succeeded();
        assert!(!r.failed(false));
        assert!(!r.waiting);
    }
    #[test]
    fn dynamic_layout_and_shared_hit_coordinates() {
        for n in [0, 1, 7, 256] {
            let l = Layout::new(vec![10.0; n]);
            assert_eq!(l.cells.len(), n + 3);
            for c in &l.cells {
                assert_eq!(l.hit(c.left, 0.0), Some(c.hit));
                for dpi in [96, 144, 192] {
                    let px = (c.left + c.right) / 2.0 * dpi as f32 / 96.0;
                    assert_eq!(l.hit(px * 96.0 / dpi as f32, 23.0), Some(c.hit));
                }
            }
            assert_eq!(l.hit(l.width, 20.0), None);
            assert_eq!(l.hit(0.0, HEIGHT), None);
            assert_eq!(l.hit(-1.0, 0.0), None);
        }
    }
    #[test]
    fn long_text_and_inline_comment_expand_cell() {
        let short = Layout::new([10.0]);
        let long = Layout::new([1200.0 + 300.0 + 8.0 * SCALE]);
        assert!(long.cells[0].right > short.cells[0].right);
        assert!(long.cells[0].right >= NUMBER + 1500.0 + PAD);
        assert_eq!(long.cells[1].left, long.cells[0].right);
    }
    #[test]
    fn click_requires_enabled_matching_press_and_release() {
        let mut s = snapshot();
        let mut g = Gesture::default();
        let h = Some(Hit::Candidate(0));
        assert!(g.release(h, &s).is_none());
        g.press(h, &s);
        let e = g.release(h, &s).unwrap();
        assert_eq!(e, UiAction::ItemInvoked(0));
        s.items[0].enabled = false;
        g.press(h, &s);
        assert!(g.release(h, &s).is_none());
        assert!(!enabled(&s, Hit::Previous));
        assert!(!enabled(&s, Hit::Next));
        s.can_page_next = true;
        assert!(enabled(&s, Hit::Next));
        g.press(Some(Hit::Next), &s);
        assert_eq!(
            g.release(Some(Hit::Next), &s).unwrap(),
            UiAction::NavigateNext
        );
        g.press(Some(Hit::Emoji), &s);
        assert_eq!(
            g.release(Some(Hit::Emoji), &s).unwrap(),
            UiAction::OpenEmojiPanel
        );
    }
    #[test]
    fn outside_captureloss_hide_and_replacement_cancel() {
        let s = snapshot();
        let h = Some(Hit::Candidate(0));
        let mut g = Gesture::default();
        g.press(h, &s);
        g.motion(None);
        g.motion(h);
        assert!(g.release(h, &s).is_none());
        for _ in 0..3 {
            g.press(h, &s);
            g.cancel();
            assert!(g.release(h, &s).is_none());
        }
        g.press(h, &s);
        assert!(g.release(Some(Hit::Emoji), &s).is_none());
        let mut hidden = s.clone();
        hidden.visible = false;
        g.press(h, &s);
        assert!(g.release(h, &hidden).is_none());
    }
    #[test]
    fn light_dark_palettes_have_distinct_states() {
        assert_ne!(Palette::new(false), Palette::new(true));
        for p in [Palette::new(false), Palette::new(true)] {
            assert_ne!(p.background, p.text);
            assert_ne!(p.active, p.hover);
            assert_ne!(p.disabled, p.text);
            assert_ne!(p.secondary, p.text);
        }
    }
}
// ---------------------------------------------------------------- 官方皮肤

/// 官方配色是 `0xAARRGGBB`（alpha 为 0 时按不透明处理），我们的 `Palette` 只存 RGB。
fn rgb_of(color: u32) -> u32 {
    color & 0x00FF_FFFF
}

/// 按给定路径读取 `*.theme.toml` 并覆盖内置色板；失败时原样返回内置色板。
///
/// 映射关系照官方语义：`back_color` → 栏体底色，`border_color` → 外框，
/// `hilited_candidate_back_color` → 选中与悬停背景（用户要求两者同色），
/// `candidate_text_color`（缺失时退回 `text_color`）→ 主文本，
/// `comment_text_color` → 次要文本，`hilited_candidate_text_color` → 选中序号。
pub fn palette_from_path(path: &std::path::Path, dark: bool) -> Palette {
    let mut palette = Palette::new(dark);
    let Ok(file) = weasel_style::theme::parse_file(path) else {
        crate::diagnostics::record(format_args!("皮肤读取失败: {}", path.display()));
        return palette;
    };
    let config = file.into_config();
    let scheme = config.active_scheme(dark);
    palette.background = rgb_of(scheme.back_color);
    palette.border = rgb_of(scheme.border_color);
    palette.active = rgb_of(scheme.hilited_candidate_back_color);
    palette.hover = palette.active;
    palette.text = if scheme.candidate_text_color != 0 {
        rgb_of(scheme.candidate_text_color)
    } else {
        rgb_of(scheme.text_color)
    };
    palette.secondary = rgb_of(scheme.comment_text_color);
    palette.active_number = if scheme.hilited_candidate_text_color != 0 {
        rgb_of(scheme.hilited_candidate_text_color)
    } else {
        palette.text
    };
    crate::diagnostics::record(format_args!(
        "皮肤已加载: {} · 配色 {}（{}）· 底色 #{:06x} · 文字 #{:06x}",
        path.display(),
        config.style.color_scheme,
        scheme.name,
        palette.background,
        palette.text
    ));
    palette
}

/// 读官方皮肤：用 `weasel_style::theme::load_theme` —— 它按我们的拆分约定读**两份**
/// （`theme\theme.toml` 的布局与 `color_scheme`，加 `theme\<名字>\<名字>.theme.toml` 的配色），
/// 用户目录优先、程序文件夹兜底，且兼容拆分前的扁平旧文件。
///
/// 程序文件夹从**宿主进程**的 exe 目录推得（主题 DLL 与渲染器同目录）。
pub fn palette_from_official(dark: bool) -> Palette {
    const NAME: &str = "default";
    let shared = match std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("Rime-Next")))
    {
        Some(dir) => dir,
        None => {
            crate::diagnostics::record(format_args!("取不到 exe 目录，使用内置色板"));
            return Palette::new(dark);
        }
    };
    let user = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .map(|base| base.join("Rime-Next"))
        .unwrap_or_else(|| shared.clone());
    let merged = match weasel_style::theme::load_theme(&shared, &user, NAME) {
        Ok(Some(config)) => config,
        Ok(None) => {
            crate::diagnostics::record(format_args!(
                "未找到 theme.toml / {NAME}.theme.toml（用户 {} · 共享 {}），使用内置色板",
                user.display(),
                shared.display()
            ));
            return Palette::new(dark);
        }
        Err(error) => {
            crate::diagnostics::record(format_args!("皮肤解析失败: {error}"));
            return Palette::new(dark);
        }
    };
    let scheme = merged.active_scheme(dark);
    let mut palette = Palette::new(dark);
    palette.background = rgb_of(scheme.back_color);
    palette.border = rgb_of(scheme.border_color);
    palette.active = rgb_of(scheme.hilited_candidate_back_color);
    palette.hover = palette.active;
    palette.text = if scheme.candidate_text_color != 0 {
        rgb_of(scheme.candidate_text_color)
    } else {
        rgb_of(scheme.text_color)
    };
    palette.secondary = rgb_of(scheme.comment_text_color);
    palette.active_number = if scheme.hilited_candidate_text_color != 0 {
        rgb_of(scheme.hilited_candidate_text_color)
    } else {
        palette.text
    };
    crate::diagnostics::record(format_args!(
        "皮肤已加载: 配色 {}（{}）· 底色 #{:06x} · 文字 #{:06x}",
        merged.style.color_scheme, scheme.name, palette.background, palette.text
    ));
    palette
}

#[cfg(test)]
mod skin_tests {
    use super::*;

    const DEFAULT_THEME: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../Rime-Next/theme/default.theme.toml");

    #[test]
    fn palette_comes_from_official_theme_file() {
        let builtin = Palette::new(false);
        let palette = palette_from_path(std::path::Path::new(DEFAULT_THEME), false);
        println!(
            "底色 #{:06x}（内置 #{:06x}）· 文字 #{:06x} · 选中 #{:06x} · 序号 #{:06x}",
            palette.background, builtin.background, palette.text, palette.active, palette.active_number
        );
        // 官方 aqua 的 back_color 是 0x00eceeee，映射后应为 0xeceeee
        assert_eq!(palette.background, 0x00ec_eeee, "底色应来自 default.theme.toml");
        assert_ne!(palette.background, builtin.background, "不能还是内置色板");
        // aqua 的 candidate_text_color 是 0x00000000，也就是黑色 —— 0 是合法颜色 ✓
        assert_eq!(palette.text, 0x000000, "文字色应等于官方 aqua 的 text_color");
        assert_ne!(palette.active, builtin.active, "选中块颜色也应来自官方皮肤");
        assert_eq!(palette.hover, palette.active, "悬停与选中同色");
    }

    #[test]
    fn missing_file_falls_back_to_builtin() {
        let builtin = Palette::new(true);
        let palette = palette_from_path(std::path::Path::new("Z:\\不存在\\x.theme.toml"), true);
        assert_eq!(palette.background, builtin.background, "读不到时应回退内置色板");
    }
}
