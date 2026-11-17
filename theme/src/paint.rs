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
use crate::theme::layout::{
    HEAD_ARROW, Layout, PAGE_NEXT, PAGE_PREV, PREDICTION_KIND, Rect, RoundInfo, SQ_PAGE_NEXT,
    SQ_PAGE_PREV, bar_text, bar_text_uses_ascii_format, prediction_tag_visible,
};
use crate::theme_api::CandidateView;
use weasel_style::ColorScheme;
use windows_core::Interface;

/// `material.txt` 的按 mtime 缓存：同一次绘制里 `tier()` 会被调 10 次左右，
/// 原实现每次都 `read_to_string` + 解析（实测 0.72ms/帧）。缓存后最多每 16ms
/// 查一次 mtime，切换材质最多晚一帧生效。
///
/// 材质档位：`%APPDATA%\Rime-Next\theme\material.txt` → `WEASEL_MATERIAL` → 默认 glass。
/// 设置界面把用户选的档位写进 material.txt；文件不存在时才看环境变量。
#[derive(Default)]
struct MaterialCache {
    stamp: Option<std::time::SystemTime>,
    checked_at: Option<std::time::Instant>,
    text: String,
}

std::thread_local! {
    static MATERIAL_CACHE: std::cell::RefCell<MaterialCache> =
        std::cell::RefCell::new(MaterialCache::default());
}

/// 材质文件最多每这么多个毫秒查一次 mtime（其余调用直接吃缓存）。
const MATERIAL_CHECK_MS: u128 = 16;

fn material_cached() -> String {
    let Some(path) = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .map(|base| base.join("Rime-Next").join("theme").join("material.txt"))
    else {
        return std::env::var("WEASEL_MATERIAL").unwrap_or_default();
    };
    let fresh = MATERIAL_CACHE.with(|cache| {
        let cache = cache.borrow();
        cache
            .checked_at
            .is_some_and(|at| at.elapsed().as_millis() < MATERIAL_CHECK_MS)
    });
    if fresh {
        return MATERIAL_CACHE.with(|cache| cache.borrow().text.clone());
    }
    let stamp = std::fs::metadata(&path).and_then(|meta| meta.modified()).ok();
    let hit = MATERIAL_CACHE.with(|cache| {
        let cache = cache.borrow();
        (cache.checked_at.is_some() && cache.stamp == stamp).then(|| cache.text.clone())
    });
    if let Some(text) = hit {
        MATERIAL_CACHE.with(|cache| cache.borrow_mut().checked_at = Some(std::time::Instant::now()));
        return text;
    }
    let text = std::fs::read_to_string(&path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .or_else(|| std::env::var("WEASEL_MATERIAL").ok())
        .unwrap_or_default();
    MATERIAL_CACHE.with(|cache| {
        *cache.borrow_mut() = MaterialCache {
            stamp,
            checked_at: Some(std::time::Instant::now()),
            text: text.clone(),
        };
    });
    text
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

/// 只使用「默认」档（不透明）。21:4x 用户：「隐藏玻璃和亚克力默认选项，
/// 只使用默认效果，也就是无透明的」——三档在设置界面里不再出现，也不再
/// 由 `material.txt` / `WEASEL_MATERIAL` 决定。cyime 液态玻璃光泽是**纯绘制层**，
/// 不依赖窗口透明、抓屏或 DWM backdrop，所以不透明档照样有玻璃光泽。
/// 想恢复三档：把 `FORCE_OPAQUE` 改回 `false` 即可（`Material::parse` 未动）。
const FORCE_OPAQUE: bool = true;

/// 当前材质档位（绘制/度量都用它）。
pub fn tier() -> Material {
    if FORCE_OPAQUE {
        return Material::Opaque;
    }
    Material::parse(&material_cached())
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

/// 主题要用的 DirectWrite 文本格式（官方 `D2D::pPreeditFormat` / `pTextFormat` /
/// `pLabelFormat` / `pCommentFormat`），外加「预测标签」小号格式与
/// 「编码栏 ASCII 编码」放大格式。
pub struct Formats {
    pub preedit: IDWriteTextFormat,
    /// 编码栏**纯 ASCII 编码**专用（2026-11-12）。同一 `font_point` 下用户
    /// 截图同阈值实测 `ff` 墨迹高 24px、中文按钮 28px，故单独放大 1.17×；
    /// 中文按钮 / 翻页箭头 /「智能预测」仍用 `preedit`，视觉字高才齐。
    /// `MeasureFormat::PreeditAscii` 保证测量缓存不与 `preedit` 串味。
    pub preedit_ascii: IDWriteTextFormat,
    pub text: IDWriteTextFormat,
    pub label: IDWriteTextFormat,
    pub comment: IDWriteTextFormat,
    /// 「预测」标签（预测项的 `secondary_text`）专用的小号格式：字号 = 候选字号的一半。
    ///
    /// 2026-11-08 先按「标签小一号、候选正文字号不变」接入；同日稍后用户又要求
    /// 「不要显示预测这个标签了」⇒ `layout::prediction_tag_visible()` 恒 false，
    /// 这个格式目前**不会被用于绘制**（`measure_all` 对预测项直接给 ZERO）。
    /// 保留它只是为了以后想恢复标签时不必重写半字号链路。见 `make_formats`。
    pub prediction_tag: IDWriteTextFormat,
    /// 挤压三模式的**小号**格式（11px = 0.6875em）：候选序号、拼音注释、底行页码。
    /// 2026-11-13「完全按 ime_ui_reference.html 做」：HTML 里这三处都是 11px，
    /// 之前它们和候选正文共用 14pt ⇒ 序号/拼音看起来和候选一样大（用户说难看）。
    pub squeeze_small: IDWriteTextFormat,
    /// 挤压三模式顶行预编辑（13px = 0.8125em、等宽、600）：HTML `.cand-head .preedit`。
    pub squeeze_head: IDWriteTextFormat,
    /// 挤压三模式顶行「首候选提示」（13px = 0.8125em、600）：HTML `.cand-head .hint`。
    pub squeeze_head_hint: IDWriteTextFormat,
    /// 挤压三模式底行按钮字（2/3 em，2026-11-13 用户定）：翻页 `‹`/`›` 与「同/贴/符」。
    pub squeeze_btn: IDWriteTextFormat,
    /// 挤压三模式顶行「拼音」pill 文字（10.5px = 0.65625em）：HTML `.tag`。
    ///
    /// 2026-11-13「方案②」加：数据源是 `CandidateView.schema_name`（Engine 的
    /// `RimeStatus.schema_name`）。方案名未知（空串）时不画 pill。
    pub squeeze_tag: IDWriteTextFormat,
    /// 挤压三模式底行页码（11px、等宽 Consolas）：HTML `.cand-foot .pg`「1 / N」。
    ///
    /// 与 `squeeze_small` 同字号但字体不同（HTML 里 `.pg` 是 `ui-monospace`），
    /// 且必须用独立格式：同一段文字用不同格式量/画，缓存不能串味。
    pub squeeze_pg: IDWriteTextFormat,
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
    /// 当前悬停的编码栏按钮（panels 工作新增，`mod.rs` 按 Hit::BarButton 赋值）。
    pub hover_button: Option<usize>,
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

// ── 挤压三模式配色（2026-11-06 起整帧读主题 + 三档材质）──────────────────
// 旧版这里是一套写死的灰 / 白 / 墨：外圈 `0xFF9A9A9F`（后来与白板同色）、
// 白内板 `0xFFF5F5F7`、正文 `0xFF1F2937`、分界线 `0xFF111827`、字符格线 `0xFFC9CED6`。
// 换主题时只有候选块跟着变、底板永远是白的 ⇒ 用户 2026-11-06「界面按 theme 的配色做」。
// 21:5x 又要求「接入默认玻璃亚克力三个配色」：挤压帧此前**整帧绕过材质三档** ——
// `draw_squeeze` 直接铺不透明 `back_color`，既不画抓屏模糊底图、也不铺亚克力蒙版，
// 于是设置里切 default / glass / acrylic 三档，挤压界面都是一个样。
// 现在由 `squeeze_palette_for` 按 `tier()` 分档产出 `surface`：
//   默认 Opaque  → 不透明主题实色；
//   玻璃 Glass   → 不铺（露出抓屏高斯模糊底图）；
//   亚克力 Acrylic → 半透明配色蒙版（mod 侧用原始配色算好，从 `Painter.veil` 注入）。
// `paper` 仍是「有效背景」：算字色对比度用的**不透明**代表色，三档各按真实能看见的底算。
// self.scheme 在 flat 档已经是 `plain_scheme` 做过对比度校正的那份；
// 配色解析失败时 `resolve()` 也会补齐 back/text，这里不用再防黑屏。
// **只换颜色，几何一行不动**。

/// 挤压一帧用到的全部主题色。外圈与内容区共用一层 `surface` ⇒ 21:3x「padding 去掉
/// 颜色」的成果保留（同一块圆角板，没有独立灰边）；候选底/字按选中态分两套。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SqueezePalette {
    /// 当前材质档位（分档逻辑 / 诊断 / 测试用）。
    tier: Material,
    /// 面板层实际铺的颜色（**含 alpha**；alpha==0 = 不铺，露出抓屏模糊底图）。
    surface: u32,
    /// 「有效背景」：不透明，文字 / 线 / 按钮的对比度校验与候选底回落都以它为准。
    /// 默认档 = 主题 `back_color`；玻璃档 = 模糊桌面平均亮度灰；
    /// 亚克力档 = 蒙版色按自身 alpha 叠在中灰桌面上的结果。
    paper: u32,
    /// 正文墨色（`text_color`）：未选中候选词 / 顶行首候选提示 / 编码栏。
    ink: u32,
    /// 分隔线与按钮描边（HTML `--line`）。配色里没有 `--line` 对应的角色
    /// （`border_color` 是玻璃亮边、压在浅底上读不出来），按「paper 与 dim 混 22%」
    /// 逼近 18 套配色各自的 `--line`（浅色 ≈ #e5e8ec / 深色 ≈ #3b4553）。
    line: u32,
    /// 次级文字（HTML `--dim`）：候选序号 / 拼音注释 / 页码。配色里对应
    /// `comment_text_color`（18 套里正是 `--dim`，实测 #66748a）。
    dim: u32,
    /// 未选中候选**正文**字色（HTML `--text`，不是灰）：2026-11-13「完全按 html 做」
    /// 取代 11-07「未选中候选词做成灰」——那条是旧色块设计的口径，HTML 里正文用正文色，
    /// 只有序号/拼音是 dim。
    cand_text: u32,
    /// 选中候选底（HTML `--sel`；18 套里映射在 `hilited_back_color`，
    /// 深色套是半透明 ⇒ 这里先按 `paper` 混成不透明）。
    hilite_bg: u32,
    /// 选中候选字色（HTML `--seltext`；18 套里映射在 `hilited_text_color`）。
    hilite_text: u32,
    /// 底行按钮盒底色（HTML `.cand-foot button{background:var(--panel2)}`）：
    /// 用 paper 与 dim 取 7% 混合近似（配色结构里没有 `--panel2` 角色）。
    btn_bg: u32,
}

/// 便捷入口：默认档（不透明）配色。旧单测与设计文档用它验证「按 theme 走」。
fn squeeze_palette(scheme: &ColorScheme) -> SqueezePalette {
    squeeze_palette_for(scheme, Material::Opaque, None, 0)
}

/// 按材质档位把 `ColorScheme` 映射成挤压帧的全部颜色，每项都带可读性校验：
/// 主题里写字色是给官方布局用的，深底配深字（或浅底配浅字）在挤压板上会隐形，
/// 这时按有效背景明暗回落到中性浅/深墨，而不是硬套主题值。
///
/// * `backdrop_luma`：`self.backdrop` 抓到的模糊桌面平均亮度；`None` = 没抓到。
/// * `_veil`：历史参数，mod 侧注入的亚克力蒙版。2026-11-14 起**大框颜色统一走
///   `back_color` 同款**（「默认同款颜色 + 档位透明度」），不用它的色相；透明度统一读
///   [`veil_alpha`]，与官方布局的 [`theme_veil`] 是同一个数字。保留参数只为兼容调用点。
///
/// 三档 `surface` 与官方 `Painter::draw` 的 `back_color` 分档一一对应：默认铺实色、
/// 玻璃不铺、亚克力铺蒙版；`tier.flat()` 的两档由 `draw_squeeze` 先把抓屏模糊位图
/// 铺在下面。字色之所以能跟着换，是因为 mod 侧在 flat 档已经把 `scheme` 过了一遍
/// [`plain_scheme`]（玻璃按真实桌面亮度、亚克力按黑白两端最坏情况校字色）。
fn squeeze_palette_for(
    scheme: &ColorScheme,
    tier: Material,
    backdrop_luma: Option<f32>,
    _veil: u32,
) -> SqueezePalette {
    let opaque = |c: u32| 0xFF00_0000u32 | (c & 0x00FF_FFFF);
    let lum = |c: u32| {
        let r = ((c >> 16) & 0xFF) as f32;
        let g = ((c >> 8) & 0xFF) as f32;
        let b = (c & 0xFF) as f32;
        (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
    };
    let readable = |a: u32, b: u32| (lum(a) - lum(b)).abs() > 0.30;
    /// 主题给了不透明色且压在 `bg` 上读得出来就用，否则用 `fallback`。
    let pick = |c: u32, bg: u32, fallback: u32| -> u32 {
        if (c >> 24) >= 0x40 {
            let v = opaque(c);
            if readable(v, bg) {
                return v;
            }
        }
        fallback
    };
    let neutral = |bg: u32| {
        if lum(bg) < 0.5 {
            0xFFF2_F3F5u32 // 深底 → 浅墨
        } else {
            0xFF1B_1D_22u32 // 浅底 → 深墨
        }
    };

    let back = opaque(scheme.back_color);
    let (surface, paper) = match tier {
        // 默认（含老金属档）：主题 `back_color` 铺满，不抓屏、不模糊。
        Material::Opaque | Material::Metal => (back, back),
        // 2026-11-14 用户定式:透明/亚克力 = **默认同款颜色** + 档位透明度
        // (两档都是 80% 透 = alpha 0x33),选中底也按同档透；差别在抓屏高斯模糊的
        // 强度(亚克力 σ=12 / 透明 σ=3)与官方布局那一层染色，不靠不透明度区分。
        Material::Glass => {
            let l = backdrop_luma
                .unwrap_or(255.0)
                .clamp(0.0, 255.0)
                .round() as u32;
            let gray = 0xFF00_0000u32 | (l << 16) | (l << 8) | l;
            let surface = (0x33 << 24) | (back & 0x00FF_FFFF);
            (surface, gray)
        }
        Material::Acrylic => {
            // 2026-11-14 20:0x 用户:「亚克力做成 80% 透明,然后加一层高斯模糊」——
            // 大框 = 默认档同款底色 + 80% 透的半透明蒙版；透明度走 veil_alpha()
            // （默认 0x33，可用 theme/veil.txt 实时调），与官方布局 theme_veil() 同一个数，
            // 字色计算与绘制不再打架。底下由 draw_squeeze 先铺 σ=12 的抓屏高斯模糊位图，
            // 大框只剩 20% 染色，磨砂质感全靠那张糊过的桌面撑住。
            let surface = (veil_alpha() << 24) | (back & 0x00FF_FFFF);
            // 21:3x「32 个配色都做好」：有效背景回到**这套配色自己的底色**，
            // 字色跟着皮肤走（浅底深字 / 深底浅字），不再锁成某一套参考图的酒红。
            let paper = back;
            (surface, paper)
        }
    };

    let ink = pick(scheme.text_color, paper, neutral(paper));
    // 2026-11-13「完全按 ime_ui_reference.html 做」：
    //   `--dim`  = 18 套配色里的 `comment_text_color`（实测 #66748a = HTML 的 --dim）
    //   `--line` = 配色里没有这个角色（`border_color` 被映射成玻璃亮边）；
    //              按 paper→dim 混 22% 逼近（浅色 ≈ #e5e8ec、深色 ≈ #3b4553）
    //   `--sel`  = `hilited_back_color`（深色套半透明 ⇒ 按 paper 混成不透明）
    //   `--seltext` = `hilited_text_color`
    //   `--panel2`  = 按钮底，配色里也没有 ⇒ paper→dim 混 7%
    let dim = pick(
        scheme.comment_text_color,
        paper,
        blend_toward(ink, paper, 0.45),
    );
    let line = blend_toward(paper, dim, 0.22);
    let cand_text = pick(scheme.candidate_text_color, paper, ink);
    // 选中底：优先 HTML `--sel`（`hilited_back_color`）；老主题没填时回退到
    // `hilited_candidate_back_color`（18 套里是 accent 实色）。
    let sel_raw = if (scheme.hilited_back_color >> 24) >= 0x40 {
        scheme.hilited_back_color
    } else {
        scheme.hilited_candidate_back_color
    };
    let hilite_bg = if tier == Material::Glass || tier == Material::Acrylic {
        // 透明/亚克力档:选中候选背景**按自身 alpha 透**(2026-11-14 用户:「包括当前候选的背景色」)。
        sel_raw
    } else {
        blend_alpha(sel_raw, paper)
    };
    // 选中字：HTML `--seltext`（`hilited_text_color`）；没填时回退候选高亮字色。
    let sel_text_raw = if (scheme.hilited_text_color >> 24) >= 0x40 {
        scheme.hilited_text_color
    } else {
        scheme.hilited_candidate_text_color
    };
    let hilite_text = pick(sel_text_raw, hilite_bg, neutral(hilite_bg));
    let btn_bg = blend_toward(paper, dim, 0.07);
    SqueezePalette {
        tier,
        surface,
        paper,
        ink,
        line,
        dim,
        cand_text,
        hilite_bg,
        hilite_text,
        btn_bg,
    }
}

/// 把 `fg` 按 `t`（0..1）混到 `bg` 上，输出不透明 ARGB。
/// 用于两种配色里没有对应角色的 HTML 色（`--line` / `--panel2` /
/// 选中序号 75% 透明度）—— 从已有角色推，18 套配色各自得到同色系的近似值。
fn blend_toward(bg: u32, fg: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |shift: u32| -> u32 {
        let f = ((fg >> shift) & 0xFF) as f32;
        let b = ((bg >> shift) & 0xFF) as f32;
        (f * t + b * (1.0 - t)).round().clamp(0.0, 255.0) as u32
    };
    0xFF00_0000u32 | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// 把带 alpha 的颜色按自身 alpha 混到不透明底 `bg` 上，输出不透明 ARGB。
/// 18 套深色配色的 `--sel` 是 26% 透明的蓝（`0x42...`），D2D 直接铺会盖掉面板色，
/// 先混成等效不透明色再画，与浏览器里的合成结果一致。
fn blend_alpha(c: u32, bg: u32) -> u32 {
    let a = ((c >> 24) & 0xFF) as f32 / 255.0;
    if a <= 0.0 {
        return 0xFF00_0000u32 | (bg & 0x00FF_FFFF);
    }
    if a >= 1.0 {
        return 0xFF00_0000u32 | (c & 0x00FF_FFFF);
    }
    let mix = |shift: u32| -> u32 {
        let f = ((c >> shift) & 0xFF) as f32;
        let b = ((bg >> shift) & 0xFF) as f32;
        (f * a + b * (1.0 - a)).round().clamp(0.0, 255.0) as u32
    };
    0xFF00_0000u32 | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// 两个不透明颜色逐通道取中值（50%）。11-07「未选中候选字 = 灰」口径已作废，\n/// 但保留这个工具函数备查（#[allow(dead_code)]，不参与本帧绘制）。
/// 白字压深底 → 中性灰，深字压浅底 → 中灰；输出恒不透明。
/// 例：`#FFFFFF` 与 `#141617` → `#898A8B`（约 #8B8B8B，与用户口径一致）。
#[allow(dead_code)]
fn muted_mid(fg: u32, bg: u32) -> u32 {
    let mix = |shift: u32| -> u32 {
        ((((fg >> shift) & 0xFF) + ((bg >> shift) & 0xFF)) / 2) & 0xFF
    };
    0xFF00_0000u32 | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// 底色专用：alpha ≥ 0x40 才认，否则回落到面板底色并强制不透明。\n/// 选中底的 alpha 回落现由 lend_alpha 统一处理，此函数保留备查。
#[allow(dead_code)]
fn pick_alpha(c: u32, fallback: u32) -> u32 {
    if (c >> 24) >= 0x40 {
        0xFF00_0000u32 | (c & 0x00FF_FFFF)
    } else {
        0xFF00_0000u32 | (fallback & 0x00FF_FFFF)
    }
}

// 识别界面第一步（2026-11-06）：候选橙底 / 核白板改读皮肤配色，这几个停用保留备查。
#[allow(dead_code)]
const SQ_ORANGE: u32 = 0xFFF5_9E0B; // 候选橙底（停用）
#[allow(dead_code)]
const SQ_ORANGE_HI: u32 = 0xFFEA_580C; // 选中橙底（停用）
#[allow(dead_code)]
const SQ_CORE: u32 = 0xFFFF_FFFF; // 核白板（停用）
#[allow(dead_code)]
const SQ_NUM: u32 = 0xFF9C_A3AF; // 序号 / 注释灰（停用备查）

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
            hover_button: None,
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
/// 2026-11-14 19:4x 用户「透明度压缩一下，上一层高斯模糊」：先压到 40% 透（0x99）；
/// 20:0x 又定为 **80% 透（0x33，20% 不透明）** —— 大框几乎不染色，视觉主体是底下
/// 那张 σ=12 抓屏高斯模糊位图。这个数字同时供官方布局 [`theme_veil`] 与挤压布局
/// [`squeeze_palette_for`] 使用 —— 字色计算与实际绘制必须是同一个 alpha，
/// 否则会「算得挺好却看不清」（19:42 版两处打架：官方 0x40 / 挤压 0x99）。
///
/// 三档的"透明度"口径（大框底的不透明占比）：
/// 透明档 **20%**（0x33，只剩抓屏模糊图；抓不到屏幕才退回 50% 白）
/// · 亚克力 **20%**（0x33，同透但模糊更狠 σ=12、且铺配色蒙版）
/// · 默认（原"不透明"）**100%**（yaml 的 `back_color` 铺满）。
///
/// `plain_scheme` 用它算"有效背景亮度"（决定字色），`Painter::draw` 用它铺面板 ——
/// 两处必须是同一个值，否则字色按 A 算、底按 B 画，看着就会"算得挺好却看不清"。
/// 运行时可用 [`veil_alpha`]（`veil.txt`）覆盖。
pub(crate) const VEIL_ALPHA: u32 = 0x33; // 2026-11-14 20:0x 用户「80% 透明」:20% 不透明 = 255*0.2 = 51

/// 蒙版不透明度的**实际生效值**：`VEIL_ALPHA` 可被
/// `%APPDATA%\Rime-Next\theme\veil.txt`（一个 5..95 的百分数）实时覆盖。
///
/// 和 `material.txt` / `blur.txt` 同一套机制：每次用时现读，改完下一次显示候选窗就生效，
/// 不用重编译、不用重装 DLL。调"亚克力多透"这种主观手感，用户自己滑比来回改代码快：
/// 数字是**不透明度百分数**，写 `20` = 80% 透（当前默认）、`40` = 60% 透、
/// `80` = 20% 透（接近不透明）。
///
/// 5 以下连配色蒙版都看不见（只剩模糊桌面）；95 以上磨砂感消失、跟不透明没区别 ——
/// 两头都当"没写"，用档位默认值。
pub fn veil_alpha() -> u32 {
    let tuned = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .map(|base| base.join("Rime-Next").join("theme").join("veil.txt"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| parse_veil_pct(&text));
    tuned.unwrap_or(VEIL_ALPHA)
}

/// `veil.txt` 的内容 → alpha。容忍结尾的 `%` 和空白；超出 5..95 的当没写。
fn parse_veil_pct(text: &str) -> Option<u32> {
    text.trim()
        .trim_end_matches('%')
        .parse::<f32>()
        .ok()
        .filter(|pct| (5.0..=95.0).contains(pct))
        .map(|pct| (pct / 100.0 * 255.0).round() as u32)
}

/// 玻璃档的抓屏模糊强度（**降采样后**的像素标准差；1/2 降采样 ⇒ 屏幕 σ 翻倍）。
const GLASS_SIGMA: f32 = 3.0;
/// 亚克力档的抓屏模糊强度：同一个屏幕区域糊得比玻璃狠一倍多。
///
/// 用户 2026-11-04 09:4x：「再模糊一些」。DWM 的亚克力模糊半径是系统写死的（不能调），
/// 所以亚克力档也改走自绘高斯 —— 半径、强度、染色全在自己手里。
/// σ=8 ⇒ 屏幕 σ≈16px（1/2 降采样），3σ 半径接近 50px：背后再大的色块也只剩一片渐变。
const ACRYLIC_SIGMA: f32 = 12.0; // σ=8 观感不够提到 12;20:0x 用户「加一层高斯模糊」再次确认。糊过的位图铺在**候选窗大框**最底层

/// 候选窗「光泽」：逐字照抄 `skins.html` 的 `.candidate` 两条 CSS。
///
/// 23:1x 用户「html的颜色直接照搬不就行了」⇒ 撤掉此前全部自适应加工：
///   ✗ 亮端按亮度混白、暗端等比压暗（原 `gloss_shade`）
///   ✗ 亮边按底色明暗取 24% / 72% 白
///   ✗ 118° 斜光按底色明暗分档（38% 白→10% 墨 / 22% 白→全透）
///   ✗ 底色渐变强制 78% / 90% 不透明
/// 现在代码里**不再产生任何颜色**：140° 两端、1px 亮边全部原样取 YAML，
/// 而 YAML 由 `sync_from_skins_html.py` 从 `skins.html` 的 THEMES 直搬。
///
/// ```css
/// .candidate{ background:linear-gradient(140deg,var(--cand),var(--cand2));
///             border:1px solid var(--glass-border);
///             opacity:var(--cand-op) }   /* :root = 1，32 配色都未覆盖 */
/// body[data-glass=on] .candidate::before{
///   background:linear-gradient(118deg,rgba(255,255,255,.18),
///     rgba(255,255,255,0) 38%,rgba(255,255,255,0) 62%,rgba(255,255,255,.09)) }
/// ```
/// 都是纯色带，没有网格 / 放射 / 任何线条。
///
/// 2026-11-15 23:0x 用户「像雾气一样，颜色的渐变呢，怎么没有渐变，纯棕褐色啊」⇒
/// 撤掉上一版的**白色雾状高光**（白亮带/亮线/向白提亮会把颜色洗成一片灰棕），
/// 改成 HTML 原稿的**彩色多层渐变**：
///   ① 140° 底色：`--cand` → 混进 `--accent` 的 `--cand2`；
///      （YAML 里 `back_color` 与 `candidate_back_color` 都来自 `--cand`，两端同色
///        ⇒ 这就是「怎么没有渐变」的真根因）
///   ② 左上 `--accent2` 光晕、右下 `--accent` 光晕（HTML `.stage` 两团
///      radial-gradient 的线性近似，中心有颜色、边缘全透，不依赖径向 API）；
///   ③ 118° 白色斜光回到 CSS 原值 .16 / 0 / 0 / .08，只做玻璃反光，不糊成雾；
///   ④ 1px 玻璃亮边原样取 `--glass-border`。
/// 配色 YAML / skins.html 一个字段都不动，颜色全部来自当前生效的 ColorScheme。
const GLOSS_BASE_DEG: f32 = 140.0;
const GLOSS_SHEEN_DEG: f32 = 118.0;
const GLOSS_SHEEN_HEAD: u32 = 0x29FF_FFFF; // .16 白（CSS 原值）
const GLOSS_SHEEN_MID: u32 = 0x0000_0000;
const GLOSS_SHEEN_TAIL: u32 = 0x14FF_FFFF; // .08 白
const GLOSS_SHEEN_AT_34: f32 = 0.38;
const GLOSS_SHEEN_AT_66: f32 = 0.62;
/// 两团彩色光晕的 alpha（中心→中段→边缘全透）与覆盖半径。
const GLOSS_GLOW_A2: u32 = 0x8C;      // 左上 .55
const GLOSS_GLOW_A2_MID: u32 = 0x2E;  // 左上 .18
const GLOSS_GLOW_A: u32 = 0x70;       // 右下 .44
const GLOSS_GLOW_A_MID: u32 = 0x1F;   // 右下 .12
const GLOSS_GLOW_SPAN_X: f32 = 0.78;
const GLOSS_GLOW_SPAN_Y: f32 = 0.68;
/// 140° 次端混进 `--accent` 的比例：让两端真的有颜色差。
const GLOSS_CAND2_ACCENT_MIX: f32 = 0.42;

/// 把 `b` 的 RGB 按 `t` 混进 `a`，alpha 取 `a` 的（颜色混合不改透明度）。
fn mix_gloss_rgb(a: u32, b: u32, t: f32) -> u32 {
    let mix = |x: u32, y: u32| -> u32 {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u32
    };
    let alpha = (a >> 24) & 0xff;
    (alpha << 24)
        | (mix((a >> 16) & 0xff, (b >> 16) & 0xff) << 16)
        | (mix((a >> 8) & 0xff, (b >> 8) & 0xff) << 8)
        | mix(a & 0xff, b & 0xff)
}

/// 用配色里的颜色做光晕：RGB 原样，alpha 换成渐变需要的档位。
fn gloss_glow(color: u32, alpha: u32) -> u32 {
    ((alpha & 0xff) << 24) | (color & 0x00ff_ffff)
}

/// 140° 两端 / 1px 亮边：首端原样、次端混进 `--accent`、亮边原样。
/// 具体 I/O 见单测 `gloss_endpoints_mix_accent_into_cand2`。
fn gloss_endpoints(back: u32, cand2: u32, border: u32, accent: u32) -> (u32, u32, u32) {
    (
        back,
        mix_gloss_rgb(cand2, accent, GLOSS_CAND2_ACCENT_MIX),
        border,
    )
}

/// 118° 方向的斜率 dy/dx = -cosθ/sinθ（118° ⇒ 0.531）。
fn sheen_slope(deg: f32) -> f32 {
    let rad = deg.to_radians();
    -rad.cos() / rad.sin()
}

/// CSS `linear-gradient(deg)` 在矩形上的渐变轴（CSS 标准算法）：
/// 方向 (sinθ, -cosθ)，轴长 L = |w·sinθ| + |h·cosθ|，起终点 = 中心 ∓ dir·L/2。
/// 具体例子见单测：100×50 矩形 / 140° ⇒ 起点 (17.03, -14.28)、终点 (82.97, 64.28)。
fn gradient_axis(
    rect: &Rect,
    deg: f32,
) -> (windows_numerics::Vector2, windows_numerics::Vector2) {
    let rad = deg.to_radians();
    let (dx, dy) = (rad.sin(), -rad.cos());
    let len = (rect.width() * dx).abs() + (rect.height() * dy).abs();
    let cx = (rect.left + rect.right) * 0.5;
    let cy = (rect.top + rect.bottom) * 0.5;
    let half = len * 0.5;
    (
        windows_numerics::Vector2 { x: cx - dx * half, y: cy - dy * half },
        windows_numerics::Vector2 { x: cx + dx * half, y: cy + dy * half },
    )
}

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
/// `0x528CCDFF`（蓝）—— 蒙版自然跟着蓝走，跟屏幕上文字的实际颜色一致，这才是对的。
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
#[allow(dead_code)]
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
                // 21:3x「全部按 cyime 光泽做」：官方六种布局也铺同一套光泽。
                // 必须放在 `highlight_rect` **之后** —— 它内部会用 `back_color`
                // 整片 `fill_geometry`（不透明档是实色），画在前面会被完全盖掉。
                // 与挤压三模式 `draw_squeeze` 共用 `draw_gloss`，颜色取每套配色
                // 自己的 `back_color`；16×2=32 套配色同款光泽、各自色相。
                // 之后才是 Metal 老渐变 / preedit / 候选 / 页码，都压在光泽上层。
                if tier != Material::Metal {
                    self.draw_gloss(
                        &back_rect,
                        style.round_corner_ex.max(0.0),
                        &RoundInfo::default(),
                        self.scheme.back_color,
                    );
                }
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
            // 预编辑（L452-457）：非 inline 且有内容才画。
            // 联想条没有真实 preedit，但编码栏写「智能预测」，同样要画。
            if !inline && bar_text(&self.view).is_some() {
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

    /// 本帧的挤压配色：材质档位每帧现读（`material.txt`），背景亮度 / 蒙版由 mod 侧注入。
    /// 面板、分隔线、字符格、编码栏按钮全部走这里，保证同一帧一个口径 ——
    /// 否则面板按亚克力算、线却按默认档算，切档后颜色会打架。
    fn squeeze_pal(&self) -> SqueezePalette {
        squeeze_palette_for(
            self.scheme,
            tier(),
            self.backdrop.map(|b| b.luma as f32),
            self.veil,
        )
    }

    /// 挤压三模式整帧绘制：材质分档面板 / 内容区 / 编码 / 主题候选块 / 无底色分隔线。
    ///
    /// 官方 `draw()` 在 `m.squeeze` 时把整帧交给这里，所以官方那套金属渐变不会
    /// 叠上来；但**材质三档本身要在这里自己实现**（2026-11-06「接入默认玻璃亚克力
    /// 三个配色」）：flat 档先铺抓屏模糊位图，再按档位决定面板层铺不铺色。
    /// 所有矩形都取自 `Layout`（窗口尺寸已经按 squeeze 算好），只换颜色与装饰，
    /// 绝不自己改坐标 —— 不会被窗口裁掉。
    /// 对角 wash(HTML `.bg-orbs` 同款观感):起点色 → 50% 衰减 → 透明。
    /// 2026-11-14 用户:「这种光泽,渐变变化大一点」——三档都画。
    fn diagonal_wash(&self, rect: &Rect, from_tl: bool, color: D2D_COLOR_F) {
        unsafe {
            let (start_point, end_point) = if from_tl {
                (
                    windows_numerics::Vector2 { x: rect.left, y: rect.top },
                    windows_numerics::Vector2 { x: rect.right, y: rect.bottom },
                )
            } else {
                (
                    windows_numerics::Vector2 { x: rect.right, y: rect.bottom },
                    windows_numerics::Vector2 { x: rect.left, y: rect.top },
                )
            };
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color,
                },
                D2D1_GRADIENT_STOP {
                    position: 0.5,
                    color: D2D_COLOR_F {
                        r: color.r,
                        g: color.g,
                        b: color.b,
                        a: color.a * 0.3,
                    },
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: D2D_COLOR_F {
                        r: color.r,
                        g: color.g,
                        b: color.b,
                        a: 0.0,
                    },
                },
            ];
            let Ok(collection) = self.target.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) else {
                return;
            };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: start_point,
                endPoint: end_point,
            };
            let Ok(brush) = self
                .target
                .CreateLinearGradientBrush(&props, None, &collection)
            else {
                return;
            };
            self.target.FillRectangle(&d2d_rect(rect), &brush);
        }
    }

    /// 白光斜扫(HTML `.candidate::before` 同款):25% → 12%,中段全透。
    /// 2026-11-14 晚:被 AC2 几何背景层取代,保留函数备用。
    #[allow(dead_code)]
    fn diagonal_sheen(&self, rect: &Rect) {
        unsafe {
            let (start_point, end_point) = (
                windows_numerics::Vector2 { x: rect.left, y: rect.top },
                windows_numerics::Vector2 {
                    x: rect.left + (rect.right - rect.left) * 0.72,
                    y: rect.bottom,
                },
            );
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: d2d_color(0x40FFFFFF),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.38,
                    color: d2d_color(0x00FFFFFF),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.62,
                    color: d2d_color(0x00FFFFFF),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: d2d_color(0x17FFFFFF),
                },
            ];
            let Ok(collection) = self.target.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) else {
                return;
            };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: start_point,
                endPoint: end_point,
            };
            let Ok(brush) = self
                .target
                .CreateLinearGradientBrush(&props, None, &collection)
            else {
                return;
            };
            self.target.FillRectangle(&d2d_rect(rect), &brush);
        }
    }

    /// 在圆角面板内铺一条 CSS 同款线性渐变（裁进圆角，不会在四角戳出方亮块）。
    fn fill_gloss_gradient(
        &self,
        shell: &Rect,
        radius: f32,
        info: &RoundInfo,
        start: windows_numerics::Vector2,
        end: windows_numerics::Vector2,
        stops: &[D2D1_GRADIENT_STOP],
    ) {
        unsafe {
            let Ok(path) = rounded_rect_path(self.factory, shell, radius, info) else {
                return;
            };
            let Ok(collection) = self.target.CreateGradientStopCollection(
                stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) else {
                return;
            };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: start,
                endPoint: end,
            };
            let Ok(brush) = self.target.CreateLinearGradientBrush(&props, None, &collection) else {
                return;
            };
            self.target.FillGeometry(&path, &brush, None::<&ID2D1Brush>);
        }
    }

    /// HTML 同款彩色多层渐变（11-15 23:0x「颜色的渐变呢，纯棕褐色啊」）：
    ///   ① 140° `--cand → --cand2+--accent` 彩色底色；
    ///   ② 左上 `--accent2` 光晕；③ 右下 `--accent` 光晕；
    ///   ④ 118° 白色斜光（CSS 原值，轻）；⑤ 1px 玻璃亮边。
    /// 三模式 + 官方六布局共用，与材质档解耦；不依赖窗口透明、不抓屏。
    fn draw_gloss(&self, shell: &Rect, radius: f32, info: &RoundInfo, back: u32) {
        let accent = self.scheme.hilited_candidate_back_color; // --accent
        let accent2 = self.scheme.hilited_text_color;          // --seltext ≈ --accent2
        let (cand, cand2, border) = gloss_endpoints(
            back,
            self.scheme.candidate_back_color,
            self.scheme.border_color,
            accent,
        );
        let w = shell.width();
        let h = shell.height();
        // ① 140° 彩色底色：两端必须有色差。
        let (base_start, base_end) = gradient_axis(shell, GLOSS_BASE_DEG);
        self.fill_gloss_gradient(shell, radius, info, base_start, base_end, &[
            D2D1_GRADIENT_STOP { position: 0.0, color: d2d_color(cand) },
            D2D1_GRADIENT_STOP { position: 1.0, color: d2d_color(cand2) },
        ]);
        // ② 左上 accent2 光晕（HTML radial-gradient(280px 180px at 12% 0%) 线性近似）。
        self.fill_gloss_gradient(
            shell, radius, info,
            windows_numerics::Vector2 { x: shell.left, y: shell.top },
            windows_numerics::Vector2 {
                x: shell.left + w * GLOSS_GLOW_SPAN_X,
                y: shell.top + h * GLOSS_GLOW_SPAN_Y,
            },
            &[
                D2D1_GRADIENT_STOP { position: 0.0, color: d2d_color(gloss_glow(accent2, GLOSS_GLOW_A2)) },
                D2D1_GRADIENT_STOP { position: 0.55, color: d2d_color(gloss_glow(accent2, GLOSS_GLOW_A2_MID)) },
                D2D1_GRADIENT_STOP { position: 1.0, color: d2d_color(0) },
            ],
        );
        // ③ 右下 accent 光晕（HTML radial-gradient(260px 200px at 95% 100%) 线性近似）。
        self.fill_gloss_gradient(
            shell, radius, info,
            windows_numerics::Vector2 { x: shell.right, y: shell.bottom },
            windows_numerics::Vector2 {
                x: shell.right - w * GLOSS_GLOW_SPAN_X,
                y: shell.bottom - h * GLOSS_GLOW_SPAN_Y,
            },
            &[
                D2D1_GRADIENT_STOP { position: 0.0, color: d2d_color(gloss_glow(accent, GLOSS_GLOW_A)) },
                D2D1_GRADIENT_STOP { position: 0.60, color: d2d_color(gloss_glow(accent, GLOSS_GLOW_A_MID)) },
                D2D1_GRADIENT_STOP { position: 1.0, color: d2d_color(0) },
            ],
        );
        // ④ 118° 白色斜光：CSS 原值，薄薄一层玻璃反光，不再糊成雾。
        let sheen_start = windows_numerics::Vector2 { x: shell.left, y: shell.top };
        let sheen_end = windows_numerics::Vector2 {
            x: shell.right,
            y: shell.top + h * sheen_slope(GLOSS_SHEEN_DEG),
        };
        self.fill_gloss_gradient(shell, radius, info, sheen_start, sheen_end, &[
            D2D1_GRADIENT_STOP { position: 0.0, color: d2d_color(GLOSS_SHEEN_HEAD) },
            D2D1_GRADIENT_STOP { position: GLOSS_SHEEN_AT_34, color: d2d_color(GLOSS_SHEEN_MID) },
            D2D1_GRADIENT_STOP { position: GLOSS_SHEEN_AT_66, color: d2d_color(GLOSS_SHEEN_MID) },
            D2D1_GRADIENT_STOP { position: 1.0, color: d2d_color(GLOSS_SHEEN_TAIL) },
        ]);
        // ⑤ 玻璃亮边画在各层渐变**之后**，否则会被渐变盖掉。
        self.stroke_geometry(shell, border, radius, self.m.bar_sep, info);
    }

    /// AC2(刺客信条 2)背景几何层(2026-11-14 用户:「做成刺客信条2的背景,不要
    /// 图片,只要它的背景配色的布局」)。纯 D2D 几何,无贴图:
    ///   ① 45° 菱形网格(两向细线)  ② 中央放射线 24 根
    ///   ③ 同心菱形徽记 + 超出四角的大菱形  ④ 四周晕影(vignette)
    /// 动画(漂移/旋转/呼吸)已于 2026-11-17 **永久废弃**(用户指示)——
    /// 相位取固定值,只在交互/数据变化时随整帧重绘,不再有定时重绘。
    /// 深色 = 白线;浅色 = 墨线(文艺复兴手稿感,AC2 正是文艺复兴题材)。
    /// 全部裁进面板圆角路径(PushLayer),不会戳到圆角外。
    /// 2026-11-14 20:1x 用户「去掉背景的白线」⇒ 停止调用（函数保留，想恢复直接放开）。
    #[allow(dead_code)]
    fn paint_ac2(&self, shell: &Rect, dark: bool) {
        unsafe {
            // 动画已永久废弃(2026-11-17 用户):相位全部取固定值,几何层纯静态。
            let now_ms = 0.0_f32;
            let w = shell.width();
            let h = shell.height();
            let cx = shell.left + w * 0.5;
            let cy = shell.top + h * 0.5;
            // 线色:深底白线 / 浅底墨线,按层分档(网格最淡、徽记最实)。
            let (grid_ink, ray_ink, mark_ink, veil_ink) = if dark {
                (0x14FF_FFFFu32, 0x24FF_FFFF, 0x3CFF_FFFF, 0x8C00_0000)
            } else {
                (0x1000_0000, 0x1C00_0000, 0x2E00_0000, 0x4600_0000)
            };
            let radius = (w.max(h) * 0.5).max(1.0);

            // 圆角面板做蒙版,把整个几何层裁进面板(PushLayer)。
            let mask = match rounded_rect_path(
                self.factory,
                &Rect::new(shell.left + 1.0, shell.top + 1.0, shell.right - 1.0, shell.bottom - 1.0),
                (self.m.panel_radius() - 1.0).max(0.0),
                &RoundInfo::default(),
            ) {
                Ok(p) => p,
                Err(_) => return,
            };
            let Ok(layer) = self.target.CreateLayer(None) else {
                return;
            };
            let params = D2D1_LAYER_PARAMETERS {
                contentBounds: d2d_rect(shell),
                geometricMask: core::mem::ManuallyDrop::new(Some(mask.into())),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: windows_numerics::Matrix3x2::identity(),
                opacity: 1.0,
                opacityBrush: core::mem::ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS_NONE,
            };
            self.target.PushLayer(&params, &layer);

            // ① 45° 菱形网格:两向平行线族,沿各自方向漂移(spacing 的 2 倍周期循环)。
            let spacing = 46.0_f32.max(self.m.panel_radius() * 2.2);
            let mut grid_segs: Vec<windows_numerics::Vector2> = Vec::new();
            let collect_family = |angle_deg: f32, phase: f32, out: &mut Vec<windows_numerics::Vector2>| {
                // 过面板的斜线族:法向 (cosA, sinA),族沿法向排开(相位已静态化)。
                let (nx, ny) = (angle_deg.to_radians().cos(), angle_deg.to_radians().sin());
                let diag = (w * w + h * h).sqrt();
                let count = ((diag / spacing).ceil() as i32 + 2).max(2);
                let offset = ((cx * nx + cy * ny) + phase) % spacing;
                for k in -(count / 2)..(count / 2 + 1) {
                    let d = offset + k as f32 * spacing;
                    let px = cx + nx * d;
                    let py = cy + ny * d;
                    let half = diag;
                    out.push(windows_numerics::Vector2 {
                        x: px - ny * half,
                        y: py + nx * half,
                    });
                    out.push(windows_numerics::Vector2 {
                        x: px + ny * half,
                        y: py - nx * half,
                    });
                }
            };
            let drift = (now_ms * 0.006) % (spacing * 2.0);
            collect_family(45.0, drift, &mut grid_segs);
            collect_family(135.0, drift * 0.7 + 23.0, &mut grid_segs);
            self.ac2_stroke_many(&grid_segs, grid_ink, 1.0);

            // ② 放射线 24 根:从中心向外,只画外段(r0 起),整组缓慢旋转(2.5°/s
            //    ≈ 144 秒一圈——可感知但不抢戏)。
            let rays = 24;
            let rot_deg = (now_ms * 0.0025) % 360.0;
            let r0 = radius * 0.30;
            let r1 = radius * 1.45;
            let mut ray_segs: Vec<windows_numerics::Vector2> = Vec::with_capacity(rays * 2);
            for i in 0..rays {
                let a = (rot_deg + i as f32 * (360.0 / rays as f32)).to_radians();
                let (ca, sa) = (a.cos(), a.sin());
                ray_segs.push(windows_numerics::Vector2 { x: cx + ca * r0, y: cy + sa * r0 });
                ray_segs.push(windows_numerics::Vector2 { x: cx + ca * r1, y: cy + sa * r1 });
            }
            self.ac2_stroke_many(&ray_segs, ray_ink, 1.0);

            // ③ 同心菱形徽记:两层嵌套 + 一个超出四角的大菱形(只露 4 个角,
            //    AC2 菜单的斜切框语言);alpha 呼吸(3.2 秒周期 ±30%)。
            let breath = 0.72 + 0.28 * (now_ms * std::f32::consts::TAU / 3200.0).sin();
            let breathe = |argb: u32, scale: f32| {
                let a = (((argb >> 24) as f32 / 255.0) * breath * scale * 255.0).round() as u32;
                (a << 24) | (argb & 0x00FF_FFFF)
            };
            let diamond = |half: f32, ink: u32| {
                let pts = [
                    windows_numerics::Vector2 { x: cx, y: cy - half },
                    windows_numerics::Vector2 { x: cx + half, y: cy },
                    windows_numerics::Vector2 { x: cx, y: cy + half },
                    windows_numerics::Vector2 { x: cx - half, y: cy },
                    windows_numerics::Vector2 { x: cx, y: cy - half },
                ];
                self.ac2_stroke(&pts, ink, 1.0);
            };
            diamond(radius * 0.34, breathe(mark_ink, 1.0));
            diamond(radius * 0.52, breathe(mark_ink, 0.6));
            diamond(radius * 1.55, breathe(mark_ink, 0.45));

            // ④ 四周晕影:四条向内的线性渐变(边缘 → 透明),AC2 的 vignette。
            let edge = (w.min(h) * 0.22).max(28.0);
            // 2026-11-14 实测:veil 铺全窗时轴外区域不被 clamp 成透明(整窗蒙暗),
            // FillRectangle 只铺各自边缘条带,渐变轴与条带重合。
            let veil = |rect: (f32, f32, f32, f32), from: (f32, f32), to: (f32, f32)| {
                let stops = [
                    D2D1_GRADIENT_STOP { position: 0.0, color: d2d_color(veil_ink) },
                    D2D1_GRADIENT_STOP {
                        position: 1.0,
                        color: D2D_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
                    },
                ];
                if let Ok(collection) =
                    self.target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
                {
                    let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                        startPoint: windows_numerics::Vector2 { x: from.0, y: from.1 },
                        endPoint: windows_numerics::Vector2 { x: to.0, y: to.1 },
                    };
                    if let Ok(vb) = self.target.CreateLinearGradientBrush(&props, None, &collection) {
                        self.target.FillRectangle(
                            &D2D_RECT_F { left: rect.0, top: rect.1, right: rect.2, bottom: rect.3 },
                            &vb,
                        );
                    }
                }
            };
            veil((shell.left, shell.top, shell.right, shell.top + edge),
                (shell.left, shell.top), (shell.left, shell.top + edge));
            veil((shell.left, shell.bottom - edge, shell.right, shell.bottom),
                (shell.left, shell.bottom), (shell.left, shell.bottom - edge));
            veil((shell.left, shell.top, shell.left + edge, shell.bottom),
                (shell.left, shell.top), (shell.left + edge, shell.top));
            veil((shell.right - edge, shell.top, shell.right, shell.bottom),
                (shell.right, shell.top), (shell.right - edge, shell.top));

            self.target.PopLayer();
        }
    }

    /// 线段族批量描边(2026-11-17 渲染提速):一族几十条线合进**一个** path
    /// (每 figure 一条线),一次 DrawGeometry —— 与设置窗同款优化,候选窗网格
    /// +放射线从 ~60 个几何对象/帧降到 3 个。
    fn ac2_stroke_many(
        &self,
        segs: &[windows_numerics::Vector2],
        ink: u32,
        width: f32,
    ) {
        if segs.is_empty() {
            return;
        }
        unsafe {
            let Ok(path) = self.factory.CreatePathGeometry() else {
                return;
            };
            let Ok(sink) = path.Open() else {
                return;
            };
            for pair in segs.chunks(2) {
                if pair.len() < 2 {
                    break;
                }
                sink.BeginFigure(pair[0], D2D1_FIGURE_BEGIN_HOLLOW);
                sink.AddLine(pair[1]);
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
            }
            let _ = sink.Close();
            let _ = self.brush.SetColor(&d2d_color(ink));
            self.target.DrawGeometry(&path, self.brush, width, None::<&ID2D1StrokeStyle>);
        }
    }

    /// 折线描边(AC2 层专用):一条共享 path,N 个点,HOLLOW 开始 + OPEN 结束。
    fn ac2_stroke(&self, pts: &[windows_numerics::Vector2], ink: u32, width: f32) {
        if pts.len() < 2 {
            return;
        }
        unsafe {
            let Ok(path) = self.factory.CreatePathGeometry() else {
                return;
            };
            let Ok(sink) = path.Open() else {
                return;
            };
            sink.BeginFigure(pts[0], D2D1_FIGURE_BEGIN_HOLLOW);
            for p in &pts[1..] {
                sink.AddLine(*p);
            }
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            let _ = sink.Close();
            let _ = self.brush.SetColor(&d2d_color(ink));
            self.target.DrawGeometry(&path, self.brush, width, None::<&ID2D1StrokeStyle>);
        }
    }

    /// 斜切高亮条(AC2 菜单选中语言):平行四边形,左右两侧各切 `slant` px,
    /// 直角不圆角。用于选中候选行 —— 颜色仍走皮肤的 hilite_bg(形状 AC2 化,
    /// 配色保持皮肤,2026-11-14 用户:「还有现在的选中背景呢」)。
    fn fill_slanted(&self, rect: &Rect, slant: f32, color: u32) {
        if transparent(color) || rect.width() <= slant * 2.0 || rect.height() <= 1.0 {
            return;
        }
        unsafe {
            let path = match self.factory.CreatePathGeometry() {
                Ok(p) => p,
                Err(_) => return,
            };
            let Ok(sink) = path.Open() else {
                return;
            };
            let point = |x: f32, y: f32| windows_numerics::Vector2 { x, y };
            sink.BeginFigure(point(rect.left + slant, rect.top), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(point(rect.right, rect.top));
            sink.AddLine(point(rect.right - slant, rect.bottom));
            sink.AddLine(point(rect.left, rect.bottom));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            let _ = sink.Close();
            let _ = self.brush.SetColor(&d2d_color(color));
            self.target.FillGeometry(&path, self.brush, None::<&ID2D1Brush>);
        }
    }

    fn draw_squeeze(&self, layout: &Layout, empty_context: bool) -> Result<(), String> {
        let _ = empty_context; // 自带壳始终画满；官方「空上下文不画背景」不适用于这里
        // 2026-11-06「界面按 theme 的配色做」+「接入默认玻璃亚克力三个配色」：
        // 整帧颜色只从这一份按档位算好的 `SqueezePalette` 取，不再有写死的灰/白/墨。
        let pal = self.squeeze_pal();
        let info = RoundInfo::default();
        let shell = Rect::new(0.0, 0.0, layout.content_size.cx, layout.content_size.cy);
        if shell.is_empty() {
            return Ok(());
        }
        // 面板圆角：挤压三模式按 HTML `--r:14px`（= 0.875em）走 `panel_radius()`，
        // 官方六种布局仍是原值。窗口裁剪与毛玻璃遮罩也走同一个函数，三处弧线同心。
        let round = self.m.panel_radius().max(0.0);
        // 2026-11-13 21:2x「完全按 html 做」：挤压三模式**取消灰壳 padding** ——
        // HTML 的 `.candidate` 没有内圈留白，`.cand-head/.cand-list/.cand-foot` 都是
        // 面板的直接子项，行内留白由各自 padding 给（13px / 6px / 12px）。
        // `Metrics::squeeze_frame()` 已归零，这里 `lip` 只保留给旧调用点做防御。
        let lip = self.m.squeeze_lip(shell.width());
        let _ = lip;

        // ① 面板：flat 档先铺抓屏模糊底图，再按 `pal.surface` 铺一层圆角板
        // （半透明蒙版只铺一遍，两遍会叠得更实）。
        if pal.tier.flat() {
            self.draw_backdrop(&shell, self.backdrop);
        }
        if !transparent(pal.surface) {
            self.fill_geometry(&shell, pal.surface, round, &info, false);
        }
        // ①b 面板 1px 描边（HTML `.candidate{border:1px solid ...;overflow:hidden}`）。
        //    主题的 `border_color` 在 18 套里是玻璃亮边（浅底上读不出来），统一改用
        //    按 paper→dim 推出来的 `--line`，与两条分隔线同色，边缘像 HTML 那样细而清。
        self.stroke_geometry(&shell, pal.line, round, self.m.bar_sep, &info);
        // 21:3x「全部按这个做，要 cyime 的光泽，16 皮肤 32 配色都做好」：
        //   光泽与**材质档解耦** —— 默认（不透明）/ 玻璃 / 亚克力三档一律铺，
        //   颜色取每套配色自己的 `back_color`（不是被档位改写过的 pal.paper），
        //   这样 16×2=32 套配色切到任意一套都是同款液态玻璃光泽、各自色相。
        //   AC2 几何网格 / 晕影 / 白光斜扫仍然停用（20:1x「去掉背景的白线」）。
        //   Metal 老档自带上白下黑分段渐变，跳过以免两层打架。
        if pal.tier != Material::Metal {
            self.draw_gloss(&shell, round, &info, self.scheme.back_color);
        }
        // 面板内容区（描边以内）：分隔线画在描边里侧，不压面板线。
        let mut paper = shell;
        paper.deflate(self.m.bar_sep, self.m.bar_sep);

        // ②b 两条 1px 分隔线（`.cand-head{border-bottom:1px}` / `.cand-foot{border-top}`）。
        let sep_left = shell.left + self.m.bar_sep;
        let sep_right = shell.right - self.m.bar_sep;
        if !layout.preedit_row.is_empty() {
            let y = layout.preedit_row.bottom;
            self.fill_geometry(
                &Rect::new(sep_left, y, sep_right, y + self.m.bar_sep),
                pal.line,
                0.0,
                &info,
                false,
            );
        }
        if !layout.foot_row.is_empty() {
            let y = layout.foot_row.top;
            self.fill_geometry(
                &Rect::new(sep_left, y, sep_right, y + self.m.bar_sep),
                pal.line,
                0.0,
                &info,
                false,
            );
        }
        // 顶行右侧的「首候选」提示（HTML `.cand-head .hint`：13px 600、颜色 --text）。
        if !layout.head_hint.is_empty() {
            if let Some(item) = self.view.items.first() {
                if !item.primary_text.is_empty() {
                    self.text_out(
                        &layout.head_hint,
                        &item.primary_text,
                        pal.ink,
                        &self.formats.squeeze_head_hint,
                    )?;
                }
            }
        }
        // 顶行预编辑与首候选之间的引导箭头 `›`（HTML `.cand-head .arrow`：
        // `--dim` + `opacity:.6`）；13px，与预编辑同字号。
        if !layout.head_arrow.is_empty() {
            self.text_out(
                &layout.head_arrow,
                HEAD_ARROW,
                blend_toward(pal.paper, pal.dim, 0.6),
                &self.formats.squeeze_head,
            )?;
        }
        // 顶行右侧「拼音」pill（HTML `.cand-head .tag`，2026-11-13「方案②」）：
        // `--panel2` 底 + `--line` 1px 描边 + 全圆角胶囊 + `--dim` 字 10.5px。
        // 数据源 = `view.schema_name`；空串时 layout 不给矩形 ⇒ 不画不占位。
        if !layout.head_tag.is_empty() {
            let info = RoundInfo::default();
            // 全圆角：HTML 是 `border-radius:999px`，实际就是半高。
            let radius = layout.head_tag.height() / 2.0;
            self.fill_geometry(&layout.head_tag, pal.btn_bg, radius, &info, false);
            self.stroke_geometry(&layout.head_tag, pal.line, radius, self.m.bar_sep, &info);
            self.text_out_centered(
                &layout.head_tag,
                &self.view.schema_name,
                pal.dim,
                &self.formats.squeeze_tag,
            )?;
        }

        // ③ 顶行预编辑（HTML `.cand-head .preedit`：13px 等宽 600、颜色 --preedit）。
        //    配色结构里没有独立的 `--preedit` 角色；18 套里它与「选中候选字」同色系
        //    （浅色 #1b53c4 vs HTML #2f6fed、深色 #b8c9ea vs HTML #99b1e0），
        //    取 `hilite_text` 最接近，且对比度已按有效背景校验过。
        //    2026-11-12 的「ASCII 编码放大 1.17×」只服务旧编码栏的左右对视，
        //    新顶行右侧是首候选、右下按钮另有 12px 字号，这里不再补偿。
        let preedit_text = bar_text(&self.view);
        if !layout.inline_preedit && !layout.preedit.is_empty() {
            if let Some(text) = preedit_text {
                self.text_out(
                    &layout.preedit,
                    text,
                    pal.hilite_text,
                    &self.formats.squeeze_head,
                )?;
            }
        }

        // ④ 候选：HTML `.cand-list` 的 `.ci` —— **未选中行不铺底**（面板渐变透出来），
        //    只有 `.ci.sel` 铺 `--sel` 浅蓝底 + `--rs` 7px 圆角。
        //    `layout.highlight_rect()` 已按行 pad 外扩，命中矩形与这块完全一致。
        let count = layout.rows.len();
        let highlighted = if (self.view.selected_index as usize) < count {
            self.view.selected_index as usize
        } else {
            usize::MAX
        };
        // 三模式在这里分形：竹简=竖排单列，飞瀑=竖列，横流=单行。
        let cells = matches!(self.m.layout_type, LayoutType::Cells);
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
            let selected = i == highlighted;
            // 2026-11-13 21:2x「完全按 html 做」：选中块 = 浅蓝底（HTML `--sel`）+ 蓝字
            // （`--seltext`）+ 7px 圆角（`--rs`）。旧实现是 accent 实心蓝 + 白字，正是
            // 用户说「太难看了」的主因；未选中行**不再铺色块**（旧实现铺满整行底色）。
            if selected {
                // 选中条保持圆角(2026-11-14 用户:「候选窗选中候选还是做成圆角」)
                // —— AC2 斜切只用于设置窗的导航/配色行,候选窗仍走 --rs 圆角。
                self.fill_geometry(&hi, pal.hilite_bg, self.m.sq_row_radius, &info, false);
            }
            // 字色（HTML）：正文 `--text`；选中正文 `--seltext`；
            // 序号未选中 `--dim`、选中 `--seltext` @75% 透明；
            // 拼音 `.py` 显式 `--dim`（选不选中都一样）。
            let word_ink = if selected {
                pal.hilite_text
            } else {
                pal.cand_text
            };
            let num_ink = if selected {
                blend_toward(pal.hilite_bg, pal.hilite_text, 0.75)
            } else {
                pal.dim
            };
            let py_ink = pal.dim;
            let text = &item.primary_text;
            // 竹简：序号写进竖列**首格**，和正文同一条竖线。序号格下面的间隔
            // 必须与 `layout::cells_measured` 用同一个数（0.25em = `bar_dot` 一半），
            // 否则 head 均分错开、字会偏；现在 gap == 0.25em ⇒ 序号与正文贴紧。
            let n_body = text.chars().count();
            let gap = self.m.bar_dot * 0.5;
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
                // 11px 小号（HTML `.ci .num`）+ 居中/左对齐：
                // 2026-11-13 21:5x 用户「布局和对齐好像也不对」—— HTML 里 `.num` 是
                // flex 首项、**默认左对齐**（只有 `min-width:13px`），不是在自己那格里
                // 居中；居中会让序号与候选词之间空出一截，看起来整行右移。
                // 竹简仍居中（那颗序号写在竖列首格正中，是另一套语义）。
                if cells {
                    self.text_out_centered(
                        &label_rect,
                        &label,
                        num_ink,
                        &self.formats.squeeze_small,
                    )?;
                } else {
                    self.text_out(&label_rect, &label, num_ink, &self.formats.squeeze_small)?;
                }
            }
            if !text.is_empty() {
                if cells {
                    // 竹简：竖排逐字（`.cell .core` 天然一列），**不画格线**。
                    self.text_out_vertical(&body, text, word_ink, &self.formats.text)?;
                } else {
                    self.text_out(&row.text, text, word_ink, &self.formats.text)?;
                }
            }
            let comment = &item.secondary_text;
            // 2026-11-08「不要显示预测这个标签了」：预测项的 secondary_text 恒不落笔
            // （与 `measure_all` 的 ZERO 同口径，避免往零宽矩形里画溢出字形）。
            let tag_hidden = item.kind == PREDICTION_KIND && !prediction_tag_visible();
            if !tag_hidden && !comment.is_empty() && !row.comment.is_null() {
                // 拼音注释 = HTML `.ci .py`：11px 小号、`--dim`。
                if cells {
                    self.text_out_vertical(
                        &row.comment,
                        comment,
                        py_ink,
                        &self.formats.squeeze_small,
                    )?;
                } else {
                    self.text_out(&row.comment, comment, py_ink, &self.formats.squeeze_small)?;
                }
            }
        }

        // ⑤ 底行：`‹`/`›` 翻页按钮 + 右下「同/贴/符」三按钮。
        //    2026-11-13 21:2x「完全按 ime_ui_reference.html 做」：全部改成 HTML
        //    `.cand-foot button` 那种**边框盒子**（22×20px、圆角 5px、`--panel2` 底、
        //    `--line` 描边、12px 字），不再是裸字形 / 裸大字。
        if !layout.inline_preedit {
            self.draw_bar_buttons(layout)?;
            // 底行页码「1 / N」（HTML `.cand-foot .pg`，2026-11-13「方案②」）：
            // 11px 等宽、左对齐在底行左内沿、`‹` 左侧。
            // 只有 layout 给了非零宽矩形才画（`page_no`/`page_count` 缺一即不画）。
            // 2026-11-14 20:49 用户「页码数字也和右面的字颜色一样」：原来用
            // `pal.dim`（--dim 淡蓝灰），在暖棕面板上几乎看不见；改成与右侧
            // 「符/贴/同」按钮未选中态**同一个字色来源** `bar_ink(false)`，
            // 无论皮肤深浅、哪档材质，两处都严格同色（不写死具体 RGB）。
            if !layout.foot_page.is_empty() {
                if let (Some(page_no), Some(page_count)) =
                    (self.view.page_no, self.view.page_count)
                {
                    self.text_out(
                        &layout.foot_page,
                        &format!("{} / {}", page_no.saturating_add(1), page_count),
                        self.bar_ink(false),
                        &self.formats.squeeze_pg,
                    )?;
                }
            }
            if !layout.page_prev.is_empty() {
                self.draw_foot_button(
                    &layout.page_prev,
                    SQ_PAGE_PREV,
                    self.bar_ink(self.view.can_page_previous),
                )?;
            }
            if !layout.page_next.is_empty() {
                self.draw_foot_button(
                    &layout.page_next,
                    SQ_PAGE_NEXT,
                    self.bar_ink(self.view.can_page_next),
                )?;
            }
        }
        Ok(())
    }

    /// 底行按钮盒子（HTML `.cand-foot button`）：`--panel2` 底 + `--line` 1px 描边 +
    /// 5px 圆角，字形居中。`color` 由调用方按「可翻页 / 悬停」决定。
    fn draw_foot_button(&self, rect: &Rect, glyph: &str, color: u32) -> Result<(), String> {
        if rect.is_empty() {
            return Ok(());
        }
        let pal = self.squeeze_pal();
        let info = RoundInfo::default();
        let radius = self.m.sq_btn_radius.max(1.0);
        self.fill_geometry(rect, pal.btn_bg, radius, &info, false);
        self.stroke_geometry(rect, pal.line, radius, self.m.bar_sep, &info);
        self.text_out_centered(rect, glyph, color, &self.formats.squeeze_btn)
    }

    /// 1px 圆角描边（面板 / 按钮盒子共用）：路径按半线宽内缩，描边外沿正好落在
    /// `rect` 边界上（与官方 `_HighlightRect` 的内描边同一套做法）。
    fn stroke_geometry(&self, rect: &Rect, color: u32, radius: f32, width: f32, info: &RoundInfo) {
        if transparent(color) || width <= 0.0 {
            return;
        }
        unsafe {
            let mut stroked = *rect;
            stroked.deflate(width / 2.0, width / 2.0);
            if stroked.width() <= 0.0 || stroked.height() <= 0.0 {
                return;
            }
            let inner = (radius - width / 2.0).max(0.0);
            if let Ok(path) = rounded_rect_path(self.factory, &stroked, inner, info) {
                let _ = self.brush.SetColor(&d2d_color(color));
                self.target.DrawGeometry(
                    &path,
                    self.brush,
                    width,
                    None::<&ID2D1StrokeStyle>,
                );
            }
        }
    }

    /// 编码栏按钮 / 翻页箭头的字色（用户 2026-11-06）：
    ///   · 未选中 = 主题的「未选中候选词字体颜色」`candidate_text_color`；
    ///   · 选中 / 可翻页 = 主题的「选中候选词字体颜色」`hilited_candidate_text_color`；
    ///     看不出高亮时依次回落选中底色、主题主色、序号色，最后兜一个亮橙。
    ///   · 2026-11-06 21:4x「界面按 theme 的配色做」：底色不再是写死的白内板，
    ///     一律取 `self.squeeze_pal().paper`（按材质档位算出的**有效背景**）；
    ///     21:5x 接入三档后，玻璃档它是模糊桌面亮度灰、亚克力档是蒙版混合色，
    ///     不再是主题 `back_color`。未选中色在读不出来时回落面板墨色 `pal.ink`；
    ///     高亮色还须与未选中色明显不同。
    ///   · **只换字色**：不画圆角背景、不加 padding、不改任何几何与间隔。
    ///   · 2026-11-07 用户「未选中候选词改灰」**不作用到这里**：候选正文/序号用
    ///     `SqueezePalette::cand_text_muted`，编码栏按钮与箭头仍按这里的主题色走 ——
    ///     按钮跟着变灰会被误读成「禁用」，而这正是本轮要修的「按不了」话题。
    fn bar_ink(&self, active: bool) -> u32 {
        let opaque = |c: u32| 0xFF00_0000u32 | (c & 0x00FF_FFFF);
        let lum = |c: u32| {
            let r = ((c >> 16) & 0xFF) as f32;
            let g = ((c >> 8) & 0xFF) as f32;
            let b = (c & 0xFF) as f32;
            (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
        };
        let readable = |a: u32, b: u32| (lum(a) - lum(b)).abs() > 0.30;
        let pal = self.squeeze_pal();
        let paper = pal.paper;
        let want = self.scheme.candidate_text_color;
        let idle = if (want >> 24) >= 0x40 && readable(opaque(want), paper) {
            opaque(want)
        } else {
            pal.ink
        };
        if !active {
            return idle;
        }
        for c in [
            self.scheme.hilited_candidate_text_color,
            self.scheme.hilited_candidate_back_color,
            self.scheme.text_color,
            self.scheme.label_color,
        ] {
            if (c >> 24) >= 0x40 {
                let v = opaque(c);
                if readable(v, paper) && readable(v, idle) {
                    return v;
                }
            }
        }
        // 主题里实在挑不出可区分的高亮色：用面板底的反色中性墨，不再兜固定亮橙。
        let fallback = if lum(paper) < 0.5 {
            0xFFF2_F3_F5
        } else {
            0xFF1B_1D_22
        };
        if readable(fallback, idle) {
            fallback
        } else {
            idle
        }
    }

    /// 编码栏右侧的按钮组（设计稿 `.btns` / `.page-btns`）。
    ///
    /// 布局是纯绘制层的：箭头矩形来自 `Layout`（命中测试不变），按钮组矩形也直接取
    /// `Layout::bar_buttons`（有箭头时贴 `page_prev` 左侧；**零候选帧**没有箭头，
    /// 从白板内右沿往左排——用户 2026-11-08「打大写字母时右边光秃秃」就是这种帧）。
    /// 2026-11-08 用户「符号剪贴板同步箭头之间做成 0.25em 间距，去掉竖的分界线」：
    /// **竖分隔（vsep）与红块全部不画**，按钮之间 / 按钮与箭头之间只留 `bar_gap`
    /// （0.25em）空白；几何统一由 `Layout` 给出，绘制端不再自己算分隔线位置。
    /// 所有尺寸仍从 `Metrics` 的编码栏统一参数读（见 `config::bar`）。
    fn draw_bar_buttons(&self, layout: &Layout) -> Result<(), String> {
        if layout.bar_buttons.iter().all(|rect| rect.is_empty()) {
            return Ok(());
        }
        // 2026-11-13 21:2x「完全按 ime_ui_reference.html 做」：三颗按钮改成 HTML
        // `.cand-foot button` 那种**边框盒子**（22×20px / 圆角 5px / `--panel2` 底 /
        // `--line` 描边 / 12px 字）；选中态（悬停 / 对应面板正打开）字色用强调色。
        let pal = self.squeeze_pal();
        let info = RoundInfo::default();
        let radius = self.m.sq_btn_radius.max(1.0);
        for (idx, icon) in ["同", "贴", "符"].iter().enumerate() {
            let rect = layout.bar_buttons[idx];
            if rect.is_empty() {
                continue;
            }
            let on = self.hover_button == Some(idx) || crate::panels::button_active(idx);
            let color = if on { pal.hilite_text } else { self.bar_ink(false) };
            self.fill_geometry(&rect, pal.btn_bg, radius, &info, false);
            self.stroke_geometry(&rect, pal.line, radius, self.m.bar_sep, &info);
            self.text_out_centered(&rect, icon, color, &self.formats.squeeze_btn)?;
        }
        Ok(())
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
        let line = self.squeeze_pal().line;
        let mut x = rect.left;
        for ch in text.chars() {
            let width = if ch.is_ascii() { em * 0.5 } else { em };
            let right = (x + width).min(rect.right);
            if right <= x {
                break;
            }
            let cell = Rect::new(x, rect.top, right, rect.bottom);
            if !transparent(bg) {
                // 字符格底由调用方给（备用函数，当前设计稿不调用）。
                self.fill_geometry(&cell, bg, 0.0, &info, false);
            }
            self.fill_geometry(
                &Rect::new(right - 1.0, cell.top, right, cell.bottom),
                line,
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
        // 别叫 `line`：下面那个画格线的闭包就叫 line，会把自己捕获进去。
        let grid_ink = self.squeeze_pal().line;
        let body_top = rect.top + head + gap;
        let step = (rect.bottom - body_top) / body_n as f32;
        if step <= 0.0 {
            return;
        }
        let mut line = |y: f32| {
            if y > rect.top + 0.5 && y <= rect.bottom + 0.5 {
                self.fill_geometry(
                    &Rect::new(rect.left, y - 1.0, rect.right, y),
                    grid_ink,
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
        // 真实预编辑优先；上屏后联想条回退为「智能预测」（2026-11-12）。
        let Some(text) = bar_text(&self.view) else {
            return Ok(());
        };
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
        // 2026-11-12：非挤压骨架同样补偿 ASCII 编码视觉字高，判定与
        // `measure_all` / `draw_squeeze` 共用，中文文案不受影响。
        let format = if bar_text_uses_ascii_format(text) {
            &self.formats.preedit_ascii
        } else {
            &self.formats.preedit
        };
        if !hilite.is_null() {
            // 用户 14:3x：预编辑（编码）那块**不要黑色底**，要透明——只画字。
            let mut rc = hilite;
            rc.inflate(self.m.hilite_padding_x, self.m.hilite_padding_y);
            let _ = &rc;
            // 编码 = steam 蓝(选中段白);用户 2026-11-07 定色。
            self.text_out(&hilite, text, 0xFFFFFFFF, format)?;
        } else if !base.is_null() {
            self.text_out(&base, text, 0xFF528CCD, format)?;
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
                // 2026-11-08「不要显示预测这个标签了」（同 `draw_squeeze`）。
                let tag_hidden =
                    self.view.items[i].kind == PREDICTION_KIND && !prediction_tag_visible();
                if !tag_hidden && !comment.is_empty() {
                    // 标签若重新打开：只有预测项换小号格式，
                    // 正文（`formats.text`）与普通候选的字号一致。
                    let comment_format = if self.view.items[i].kind == PREDICTION_KIND {
                        &self.formats.prediction_tag
                    } else {
                        &self.formats.comment
                    };
                    self.text_out(&row.comment, comment, comment_color, comment_format)?;
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

    /// 水平居中版 `text_out`（序号专用）。
    ///
    /// `text_out` 用的共享字体格式是 `DWRITE_TEXT_ALIGNMENT_LEADING`，文字一律贴矩形左边；
    /// 早期靠「估算字宽把矩形收窄再摆到中间」凑居中，估宽用 `font_point` 而序号字体其实是
    /// `label_font_point`，字号都对不上，必然偏。这里改为对**这一次的 layout** 单独设
    /// `CENTER`：DirectWrite 会按 layout 宽度（= rect.width()）与真实字形宽度精确居中，
    /// 与字宽/字号估算无关。只作用于本 layout，不污染共享的 `self.formats.label`。
    fn text_out_centered(
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
            // IDWriteTextLayout 继承 IDWriteTextFormat（Deref），可单独改本 layout 的对齐。
            let _ = layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            // 水平居中已由 CENTER 处理，不能再叠 overhang.left（会把字推歪）；
            // 垂直方向沿用 text_out 的 overhang.top 补偿，保持两种绘制一致。
            let mut y = rect.top;
            if let Ok(overhang) = layout.GetOverhangMetrics() {
                if overhang.top > 0.0 {
                    y += overhang.top;
                }
            }
            let _ = self.brush.SetColor(&d2d_color(color));
            self.target.DrawTextLayout(
                windows_numerics::Vector2 { x: rect.left, y: y },
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
        // 箭头(用户 2026-11-07):可用=白,不可用=灰,不读主题箭头色。
        let pal = self.squeeze_pal();
        let prev = if self.view.can_page_previous {
            pal.hilite_text
        } else {
            self.bar_ink(false)
        };
        let next = if self.view.can_page_next {
            pal.hilite_text
        } else {
            self.bar_ink(false)
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
    use super::{
        plain_scheme, squeeze_palette, squeeze_palette_for, theme_veil, veil_alpha, Material,
        VEIL_ALPHA,
    };
    use weasel_style::ColorScheme;

    /// 2026-11-06 21:4x「界面按 theme 的配色做」：输入 steam 原始配色，
    /// 输出挤压帧七层的具体颜色。外圈与内容区共用 `paper` ⇒ 21:3x「padding 去掉
    /// 颜色」的成果仍在（没有独立灰边，但仍是不透明实色，圆角才画得出来）。
    #[test]
    fn squeeze_palette_follows_theme() {
        // 2026-11-13 21:2x「完全按 ime_ui_reference.html 做」后的角色映射：
        //   --text = candidate_text_color、--dim = comment_text_color、
        //   --sel = hilited_back_color、--seltext = hilited_text_color。
        // （存储格式是 ABGR：HTML #e5efff → 0xFFFF_EFE5、#1b53c4 → 0xFFC4_531B）
        let base = scheme(0xFF14_1617, 0x528CCDFF);
        let s = ColorScheme {
            border_color: 0xFF35_3638,
            comment_text_color: 0xFFA7_A7A9,
            candidate_text_color: 0xFFFF_FFFF,
            hilited_back_color: 0xFFFF_EFE5,  // HTML --sel #e5efff
            hilited_text_color: 0xFFC4_531B,  // HTML --seltext #1b53c4
            ..base
        };
        let p = squeeze_palette(&s);
        assert_eq!(p.paper, 0xFF14_1617, "面板底 = back_color");
        // 主题里 text_color 是半透明蓝 `0x528CCDFF`（A=0x52），压到面板上必须**强制不透明**
        // ⇒ `0xFF8CCDFF`。旧断言写的是原始带 alpha 的 `0x528CCDFF`，与 `pick()` 的
        // 不透明化口径不一致，一直是红的（2026-11-07 本棒修正断言，不改实现）。
        assert_eq!(p.ink, 0xFF8C_CDFF, "编码文字 = text_color（不透明化，深底上可读）");
        assert_eq!(p.dim, 0xFFA7_A7A9, "序号/拼音 = comment_text_color（HTML --dim）");
        // --line 配色里没有对应角色 ⇒ paper→dim 混 22%（浅色 ≈ #e5e8ec、深色 ≈ #3b4553）。
        assert_eq!(p.line, 0xFF34_3637, "分隔线 = paper→dim 22%（近似 HTML --line）");
        assert_eq!(p.cand_text, 0xFFFF_FFFF, "未选中正文 = candidate_text_color（HTML --text）");
        assert_eq!(p.hilite_bg, 0xFFFF_EFE5, "选中底 = hilited_back_color（HTML --sel）");
        assert_eq!(p.hilite_text, 0xFFC4_531B, "选中字 = hilited_text_color（HTML --seltext）");
        assert_eq!(p.btn_bg, 0xFF1E_2021, "按钮底 = paper→dim 7%（近似 HTML --panel2）");
        assert_eq!(p.paper >> 24, 0xFF, "不透明实色，不是靠透明实现");
    }

    /// 2026-11-06 21:5x「接入默认玻璃亚克力三个配色」：同一份 steam 配色，
    /// 三档必须产出三种面板层，且有效背景与字色各自对得上。
    /// 具体输入输出：back=0xFF141617 / veil=0x801B242F / 桌面亮度 200。
    #[test]
    fn squeeze_three_material_tiers_produce_three_surfaces() {
        let base = scheme(0xFF14_1617, 0x528CCDFF);
        let s = ColorScheme {
            border_color: 0xFF35_3638,
            comment_text_color: 0xFFA7_A7A9,
            candidate_text_color: 0xFFFF_FFFF,
            hilited_candidate_back_color: 0xFF59_4231,
            hilited_candidate_text_color: 0xFFFF_FFFF,
            ..base
        };

        // ① 默认：不透明主题实色板，与上一轮「按 theme 配色」完全一致（零回归）。
        let d = squeeze_palette_for(&s, Material::Opaque, None, 0);
        assert_eq!(d.tier, Material::Opaque);
        assert_eq!(d.surface, 0xFF14_1617, "面板 = back_color 不透明");
        assert_eq!(d.paper, 0xFF14_1617, "有效背景 = 面板自身");
        assert!(!d.tier.flat(), "默认档不抓屏模糊");

        // ② 玻璃：大框 = 默认同款底色 + 80% 透（0x33），底下铺抓屏高斯模糊位图。
        //    有效背景仍按模糊桌面的亮度算（200 灰）。
        let g = squeeze_palette_for(&s, Material::Glass, Some(200.0), 0);
        assert_eq!(g.surface, 0x3314_1617, "玻璃档大框 = 同款底色 80% 透");
        assert_eq!(g.paper, 0xFFC8_C8C8, "有效背景 = 200 灰");
        assert!(g.tier.flat(), "玻璃档要铺模糊底图");
        // 抓不到背景 ⇒ 字色按白底估（与 plain_scheme 同口径），大框颜色不变。
        let g2 = squeeze_palette_for(&s, Material::Glass, None, 0);
        assert_eq!(g2.surface, 0x3314_1617);
        assert_eq!(g2.paper, 0xFFFF_FFFF);

        // ③ 亚克力：大框 = 默认同款底色 + **80% 透**蒙版（0x33）；
        //    底层由 draw_squeeze 铺 σ=12 的抓屏高斯模糊位图。
        //    veil 参数（历史遗留）不再影响大框颜色，两条布局路径共用 veil_alpha()。
        let veil = 0x331B_242F;
        let a = squeeze_palette_for(&s, Material::Acrylic, Some(40.0), veil);
        assert_eq!(a.surface >> 24, VEIL_ALPHA, "大框不透明度 = 20%（80% 透）");
        assert_eq!(a.surface & 0x00FF_FFFF, 0x14_1617, "大框颜色 = 默认档同款底色");
        // 21:3x 撤销酒红锁定：亚克力档有效背景 = 这套配色自己的底色（与默认档一致）。
        assert_eq!(a.paper, 0xFF14_1617, "有效背景 = 皮肤自己的 back_color");
        assert_eq!(a.paper >> 24, 0xFF, "有效背景恒不透明");
        assert!(a.tier.flat(), "亚克力档要铺模糊底图");

        assert_ne!(d.surface, g.surface);
        assert_ne!(d.surface, a.surface);
        // 透明档与亚克力同为 80% 透（用户 20:0x），区分靠模糊强度而非染色深浅。
        assert_eq!(g.surface, a.surface);
        assert!(super::ACRYLIC_SIGMA > super::GLASS_SIGMA * 2.0);
    }

    /// 三档下文字都不能与有效背景糊成一片（亮度差 > 0.30）。
    /// 具体输入输出：浅背景 200 灰 + 主题白字 ⇒ 回落深墨 0xFF1B1D22；
    /// 深蒙版 0xFF4D5257 + 主题白字 ⇒ 白字本来就可读，保留 0xFFFFFFFF（主题优先）。
    #[test]
    fn squeeze_text_stays_readable_in_every_tier() {
        let s = scheme(0xFFEC_EEEE, 0xFFFF_FFFF); // 故意给浅底 + 白字
        let lum = |c: u32| {
            let r = ((c >> 16) & 0xFF) as f32;
            let g = ((c >> 8) & 0xFF) as f32;
            let b = (c & 0xFF) as f32;
            (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
        };
        for (tier, luma, veil) in [
            (Material::Opaque, None, 0),
            (Material::Glass, Some(200.0), 0),
            (Material::Acrylic, Some(40.0), 0x801B_242F),
        ] {
            let p = squeeze_palette_for(&s, tier, luma, veil);
            assert!(
                (lum(p.ink) - lum(p.paper)).abs() > 0.30,
                "{tier:?} 编码文字 {:#010X} 压在 {:#010X} 上读不出来",
                p.ink,
                p.paper
            );
            assert!(
                (lum(p.cand_text) - lum(p.paper)).abs() > 0.30,
                "{tier:?} 候选字 {:#010X} 压在面板 {:#010X} 上读不出来",
                p.cand_text,
                p.paper
            );
            assert!(
                (lum(p.dim) - lum(p.paper)).abs() > 0.30,
                "{tier:?} 序号/拼音 {:#010X} 压在面板 {:#010X} 上读不出来",
                p.dim,
                p.paper
            );
            assert!(
                (lum(p.hilite_text) - lum(p.hilite_bg)).abs() > 0.30,
                "{tier:?} 选中字 {:#010X} 压在选中底 {:#010X} 上读不出来",
                p.hilite_text,
                p.hilite_bg
            );
        }
        // 玻璃档浅桌面：白字读不出来 ⇒ 深墨。
        // 亚克力：21:3x 起有效背景回到皮肤自己的 back_color（浅底），白字同样
        // 读不出来 ⇒ 深墨；这样 32 套配色切到任意一套，字色都跟着自己的明暗走。
        assert_eq!(
            squeeze_palette_for(&s, Material::Glass, Some(200.0), 0).ink,
            0xFF1B_1D_22
        );
        assert_eq!(
            squeeze_palette_for(&s, Material::Acrylic, Some(40.0), 0x991B_242F).ink,
            0xFF1B_1D_22
        );
    }

    /// 设置界面写进 `material.txt` 的三个档位名，必须各自落到对应材质。
    #[test]
    fn squeeze_material_file_names_map_to_tiers() {
        assert_eq!(Material::parse("default"), Material::Opaque);
        assert_eq!(Material::parse("glass"), Material::Glass);
        assert_eq!(Material::parse("acrylic"), Material::Acrylic);
    }

    /// 浅色主题 + 白色字：编码 / 候选字必须自动回落深墨，不能白字白底隐形。
    #[test]
    fn squeeze_palette_keeps_light_theme_readable() {
        let s = scheme(0xFFEC_EEEE, 0xFFFF_FFFF);
        let p = squeeze_palette(&s);
        assert_eq!(p.paper, 0xFFEC_EEEE, "面板底跟着浅色主题");
        assert_eq!(p.ink, 0xFF1B_1D_22, "白字在浅底读不出来 ⇒ 深墨");
        assert_eq!(p.cand_text, 0xFF1B_1D_22, "候选字同理");
    }

    /// 2026-11-13 21:2x「完全按 html 做」取代 2026-11-07「未选中候选词的字体颜色做成灰色」：
    /// HTML 里未选中正文用 `--text`（正文色），**只有序号 `.num` 与拼音 `.py` 用 `--dim`**。
    /// 旧「未选中字变灰」是 11-07 色块设计的口径，已作废。
    /// 具体 I/O：steam 深底 #141617 + 正文白 + --dim #A9A7A7 ⇒ 正文白、序号灰、选中蓝。
    #[test]
    fn squeeze_unselected_candidate_uses_text_color_not_grey() {
        let base = scheme(0xFF14_1617, 0x528CCDFF);
        let s = ColorScheme {
            comment_text_color: 0xFFA7_A7A9,
            candidate_text_color: 0xFFFF_FFFF,
            hilited_back_color: 0xFFFF_EFE5,
            hilited_text_color: 0xFFC4_531B,
            ..base
        };
        let p = squeeze_palette(&s);
        assert_eq!(p.cand_text, 0xFFFF_FFFF, "未选中正文 = --text，不再是灰");
        assert_eq!(p.dim, 0xFFA7_A7A9, "序号/拼音 = --dim");
        assert_ne!(p.cand_text, p.dim, "正文与序号/拼音必须是两种字色");
        assert_eq!(p.hilite_text, 0xFFC4_531B, "选中正文 = --seltext");
        assert_eq!(p.hilite_bg, 0xFFFF_EFE5, "选中底 = --sel");
    }

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
            corner_radius: None,
            hilited_corner_radius: None,
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
        // `veil.txt` 写**不透明度**百分数：20 = 80% 透（当前默认）、95 更实；
        // 带 % 和空白都认，5..95 之外当没写。
        assert_eq!(super::parse_veil_pct("20"), Some(51)); // 20% 不透明 = 0x33 = 档位默认
        assert_eq!(super::parse_veil_pct("50"), Some(128)); // 50%  → 0x80
        assert_eq!(super::parse_veil_pct("40"), Some(102));
        assert_eq!(super::parse_veil_pct(" 78% "), Some(199)); // 78%  → 0xC7
        assert_eq!(super::parse_veil_pct("95"), Some(242));
        assert_eq!(super::parse_veil_pct("5"), Some(13));
        // 两头越界（低于 5 没蒙版 / 高于 95 没磨砂）与乱写一律当"没写"。
        assert_eq!(super::parse_veil_pct("4"), None);
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
    fn acrylic_is_eighty_percent_transparent_with_heavy_blur() {
        // 2026-11-14 20:0x 用户「亚克力做成 80% 透明，然后加一层高斯模糊」：
        // 大框蒙版只剩 20% 不透明（0x33），磨砂感由 σ=12 的抓屏高斯模糊撑住。
        assert_eq!(VEIL_ALPHA, 0x33, "20% 不透明 = 80% 透明");
        assert_eq!((255 - VEIL_ALPHA) * 100 / 255, 80, "实测透明度 80%");
        assert_eq!(VEIL_ALPHA, super::GLASS_ALPHA, "与透明档同透，靠模糊强度区分");
        assert!(
            super::ACRYLIC_SIGMA > super::GLASS_SIGMA * 2.0,
            "亚克力那一层高斯模糊必须明显糊过透明档"
        );
        // 深色皮肤 + 任意壁纸（纯黑/纯白）都得用浅色字。
        for backdrop in [None, Some(0.0), Some(255.0)] {
            let out = plain_scheme(&steam(), Material::Acrylic, backdrop);
            assert_eq!(out.text_color, 0xFF_F2_F3_F5, "backdrop={backdrop:?}");
        }
    }

    /// 11-15 23:0x：彩色渐变（HTML 同款），白色斜光回到 CSS 原值，不再铺白雾。
    /// 具体 I/O：140°/118°、白色 alpha 41/0/0/20、光晕 alpha 107/31/87/20、
    /// 100×50 矩形 140° 轴 (17.03,-14.28)→(82.97,64.28)。
    #[test]
    fn gloss_uses_html_color_gradients() {
        assert_eq!(super::GLOSS_BASE_DEG, 140.0);
        assert_eq!(super::GLOSS_SHEEN_DEG, 118.0);
        assert_eq!(super::GLOSS_SHEEN_HEAD, 0x29FF_FFFF); // .16 白
        assert_eq!(super::GLOSS_SHEEN_MID, 0x0000_0000);
        assert_eq!(super::GLOSS_SHEEN_TAIL, 0x14FF_FFFF); // .08 白
        assert_eq!(super::GLOSS_SHEEN_AT_34, 0.38);
        assert_eq!(super::GLOSS_SHEEN_AT_66, 0.62);
        assert_eq!(super::GLOSS_GLOW_A2, 0x8C);
        assert_eq!(super::GLOSS_GLOW_A, 0x70);
        assert_eq!(super::GLOSS_GLOW_SPAN_X, 0.78);
        assert_eq!(super::GLOSS_CAND2_ACCENT_MIX, 0.42);
        assert_eq!(super::gloss_glow(0xFFE0_A422, 0x8C), 0x8CE0_A422);
        // 118° ⇒ 斜率 0.531（从左往右略向下，不是水平光带）。
        assert!((super::sheen_slope(118.0) - 0.531).abs() < 0.001);
        let r = crate::theme::Rect::new(0.0, 0.0, 100.0, 50.0);
        let (s, e) = super::gradient_axis(&r, 140.0);
        assert!((s.x - 17.03).abs() < 0.05 && (s.y + 14.28).abs() < 0.05, "start=({s:?})");
        assert!((e.x - 82.97).abs() < 0.05 && (e.y - 64.28).abs() < 0.05, "end=({e:?})");
        let (s2, e2) = super::gradient_axis(&r, 118.0);
        assert!((e2.x - s2.x) > (e2.y - s2.y), "光泽主轴偏水平");
    }

    /// 11-15 23:0x：140° 两端必须真的有色差 —— YAML 里 back_color 与
    /// candidate_back_color 都来自 `--cand`，次端混进 `--accent` 后渐变才看得出来。
    /// 具体 I/O：back=cand2=0xFF302A1A / border=0x3DFFFFFF / accent=0xFFE0A422
    ///   -> 首端完全不变、次端 R 由 0x30 升到 0x68、alpha 仍 0xFF、亮边原样。
    #[test]
    fn gloss_endpoints_mix_accent_into_cand2() {
        let back = 0xFF30_2A1A;
        let border = 0x3DFF_FFFF;
        let (cand, cand2, out_border) =
            super::gloss_endpoints(back, back, border, 0xFFE0_A422);
        assert_eq!(cand, back, "首端原样 --cand");
        assert_ne!(cand2, back, "次端必须混进 accent，否则渐变两端同色");
        assert!((cand2 >> 16) & 0xff > 0x30, "R 通道要被金色拉高");
        assert_eq!(cand2 >> 24, 0xFF, "混合不改 alpha");
        assert_eq!(out_border, border, "玻璃亮边原样");
        assert_eq!(super::gloss_endpoints(back, back, 0, 0xFFE0_A422).2, 0);
    }

    /// 21:3x「只使用默认效果，也就是无透明的」：默认不透明档也必须铺光泽，
    /// 而不是只有亚克力档才有（旧代码 `pal.tier == Acrylic` 会把 32 配色全漏掉）。
    #[test]
    fn gloss_is_independent_of_material_tier() {
        let tiers = [Material::Opaque, Material::Glass, Material::Acrylic, Material::Metal];
        assert_eq!(tiers.map(|t| t != Material::Metal), [true, true, true, false]);
        assert!(!Material::Opaque.flat(), "默认档不抓屏、不模糊 = 无透明");
        // 彩色渐变只在 RGB 上做，底色 alpha 仍由 YAML 决定（11-15）。
        assert_eq!(super::gloss_endpoints(0xFFFF_FFFF, 0xFFE8_EEFF, 0xB8FF_FFFF, 0xFFE0_A422).0 >> 24, 0xFF);
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



