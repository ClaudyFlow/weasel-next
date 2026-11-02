//! 主题配置：读 `theme.yaml`（布局 + 用哪套皮肤）与 `<皮肤>/<皮肤>.theme.yaml`（配色），
//! 再按官方 `RimeWithWeasel.cpp` 的语义算成渲染用的数值。
//!
//! 三件事必须与官方逐条对齐，否则界面就是"像而不一样"：
//!
//! 1. **出厂默认值写在代码里**（官方 `output/data/weasel.toml` 的 `style:` 段）。
//!    出厂侧不再分发 `theme.yaml`，所以缺文件也要有官方观感。默认值由
//!    [`weasel_style::UIStyle::default`]／[`weasel_style::Layout::default`] 提供。
//! 2. **官方在读完配置后还会做一批夹取**（`RimeWithWeasel.cpp:1295-1341`）：
//!    `spacing ≥ hilite_padding*2`、`candidate_spacing ≥ hilite_padding*2`、
//!    `hilite_spacing ≥ hilite_padding`、`margin = sign(margin) * max(padding, |margin|)`。
//! 3. **尺寸在布局前统一乘 DPI**（官方 `Layout.cpp:24-43`），乘完取整；字号另乘
//!    `dpi/72`（点是 1/72 英寸）。两者比例不同，别混用。

use weasel_style::theme::load_theme;
use weasel_style::{ColorScheme, UIStyle};
use std::path::PathBuf;

/// 出厂皮肤名（数据在 `<程序文件夹>\Rime-Next\theme\default\default.theme.yaml`）。
pub const SKIN: &str = "default";

/// 官方 `UIStyle::LayoutType`：六种布局。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutType {
    Vertical,
    Horizontal,
    VerticalText,
    VerticalFullscreen,
    HorizontalFullscreen,
    VerticalTextFullscreen,
}

impl LayoutType {
    /// 官方 `IS_FULLSCREENLAYOUT` 宏。
    pub fn is_fullscreen(self) -> bool {
        matches!(
            self,
            Self::VerticalFullscreen | Self::HorizontalFullscreen | Self::VerticalTextFullscreen
        )
    }
    /// 官方 `_PrepareRoundInfo` 里的 `tmp[6]` 归一（全屏归回非全屏那一档）。
    pub fn base(self) -> Self {
        match self {
            Self::Vertical | Self::VerticalFullscreen => Self::Vertical,
            Self::Horizontal | Self::HorizontalFullscreen => Self::Horizontal,
            Self::VerticalText | Self::VerticalTextFullscreen => Self::VerticalText,
        }
    }
    pub fn is_vertical_text(self) -> bool {
        matches!(self, Self::VerticalText | Self::VerticalTextFullscreen)
    }
}

/// 一次加载的结果：生效的 style、当前配色、以及配置来源（用于诊断）。
pub struct Loaded {
    pub style: UIStyle,
    pub scheme: ColorScheme,
    /// 说明配置从哪来（找不到文件时说明用了内置默认值）。
    pub source: String,
}

/// 主题配置涉及的目录：程序文件夹里的 `Rime-Next` 与用户目录。
///
/// 两个目录都拿不到（缺 `APPDATA` 或 exe 路径异常）时返回 `None`。
pub fn dirs() -> Option<(PathBuf, PathBuf)> {
    let shared = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| weasel_common::rime_layout::data_dir(dir)))?;
    let user = weasel_common::process::user_data_dir()?;
    Some((shared, user))
}

/// 主题相关文件的"最后修改时间"（取最新的一份）。
///
/// 设置界面换皮肤只是改 `theme.yaml` 的 `color_scheme` 一行，运行中的渲染器不会收到
/// 任何通知（主题只在系统深浅色变化时重载）。所以渲染前比一次时间戳，变了自己重载，
/// 换皮肤才不用重启输入法。
pub fn stamp(shared: &std::path::Path, user: &std::path::Path) -> Option<std::time::SystemTime> {
    let mut latest: Option<std::time::SystemTime> = None;
    let paths = weasel_style::theme::theme_candidates(shared, user, SKIN)
        .into_iter()
        .chain([
            user.join("theme").join("theme.yaml"),
            shared.join("theme").join("theme.yaml"),
            // 2026-11-02：材质档位也进时间戳 —— 设置里点「透明/亚克力/不透明」后，
            // 渲染器下一次按键就重算度量与配色（投影留边、浅底配色都跟档位绑死）。
            user.join("theme").join("material.txt"),
        ]);
    for path in paths {
        if let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) {
            latest = Some(latest.map_or(modified, |current| current.max(modified)));
        }
    }
    latest
}

/// 多久重新 stat 一次主题文件（[`stamp_cached`]）。
///
/// 背景（2026-10-30 实测）：`Window::render` 每次收到快照都要比一次 stamp，
/// 而一次 `stamp()` 是 6 次 `std::fs::metadata`（4 个皮肤候选 + 2 个 theme.yaml），
/// 换皮肤后立刻生效只需要"下一次按键"这个量级的延迟，没必要每键都打文件系统。
/// 750ms 的代价是"换皮肤后最多 0.75 秒才生效"，完全可接受。
const STAMP_TTL: std::time::Duration = std::time::Duration::from_millis(750);

/// [`stamp`] 的带缓存版本：**每键调用几乎零成本**。
///
/// 渲染路径（每键一次）一律用这个；只在缓存过期时才真的去 stat。
pub fn stamp_cached() -> Option<std::time::SystemTime> {
    let mut guard = match stamp_cache().lock() {
        Ok(guard) => guard,
        // poisoning 不影响正确性：缓存值只是"主题有没有变"的提示
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.at.elapsed() < STAMP_TTL {
        return guard.value;
    }
    guard.at = std::time::Instant::now();
    guard.value = dirs().and_then(|(shared, user)| stamp(&shared, &user));
    guard.value
}

/// 刚读过/刚写过主题文件时调用：让缓存立刻反映新值。
///
/// 用于"重载完别再拿旧时间戳判定又变了"的场景。
pub fn refresh_stamp_cache() {
    let fresh = dirs().and_then(|(shared, user)| stamp(&shared, &user));
    if let Ok(mut guard) = stamp_cache().lock() {
        guard.at = std::time::Instant::now();
        guard.value = fresh;
    }
}

/// [`stamp_cached`] 的缓存槽：上次 stat 的时刻与结果。
struct StampCache {
    at: std::time::Instant,
    value: Option<std::time::SystemTime>,
}

/// `Instant::now()` 不是 const，所以用 `OnceLock` 在首次访问时初始化。
fn stamp_cache() -> &'static std::sync::Mutex<StampCache> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<StampCache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        // ⚠️ 初始时刻必须"已经过期"，否则第一次调用会直接返回还没 stat 过的 `None`，
        // 于是 `App::stamp = None` 而 `stamp_cached()` 稍后返回 Some —— 两边不等，
        // 又变成每键重载。`checked_sub` 是防进程刚启动就撞上时钟原点的极端情况。
        let at = std::time::Instant::now()
            .checked_sub(STAMP_TTL)
            .unwrap_or_else(std::time::Instant::now);
        std::sync::Mutex::new(StampCache { at, value: None })
    })
}

/// 按官方查找顺序读主题：用户目录优先，程序文件夹兜底。
///
/// 程序文件夹由宿主进程的 exe 目录推得（主题 DLL 与渲染器同目录）。两份文件都可缺：
/// 只有皮肤文件时布局走内置默认值；都没有时整份走内置默认值。
pub fn load(dark: bool) -> Loaded {
    let shared = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| weasel_common::rime_layout::data_dir(dir)));
    let user = weasel_common::process::user_data_dir();
    let (shared, user) = match (shared, user) {
        (Some(shared), Some(user)) => (shared, user),
        (shared, user) => {
            return Loaded {
                style: UIStyle::default(),
                scheme: ColorScheme::default().resolve(),
                source: format!(
                    "找不到配置目录（程序文件夹 {:?}·用户目录 {:?}），用官方内置默认值",
                    shared, user
                ),
            };
        }
    };

    match load_theme(&shared, &user, SKIN) {
        Ok(Some(config)) => {
            let scheme = config.active_scheme(dark);
            // 说明布局取自哪一份（官方顺序：用户目录的 theme.yaml 优先，其次程序文件夹）
            let user_main = user.join("theme").join("theme.yaml");
            let shared_main = shared.join("theme").join("theme.yaml");
            let layout_from = if user_main.is_file() {
                user_main.display().to_string()
            } else if shared_main.is_file() {
                shared_main.display().to_string()
            } else {
                format!("内置默认布局（{} 与 {} 都没有 theme.yaml）", user.display(), shared.display())
            };
            let source = format!(
                "{layout_from} · 配色 {}（{}）",
                if config.style.color_scheme.is_empty() {
                    "aqua".to_string()
                } else {
                    config.style.color_scheme.clone()
                },
                scheme.name
            );
            Loaded {
                style: config.style,
                scheme,
                source,
            }
        }
        Ok(None) => Loaded {
            style: UIStyle::default(),
            scheme: ColorScheme::default().resolve(),
            source: format!(
                "未找到 theme.yaml / {SKIN}.theme.yaml（用户 {} · 共享 {}），用官方内置默认值",
                user.display(),
                shared.display()
            ),
        },
        Err(error) => Loaded {
            style: UIStyle::default(),
            scheme: ColorScheme::default().resolve(),
            source: format!("主题解析失败（{error}），用官方内置默认值"),
        },
    }
}

/// 判定布局类型：官方 `LoadWeaselUIStyle` 的 `horizontal`/`fullscreen`/`vertical_text`/
/// `text_orientation`，最后被 `style/layout/type` 覆盖。
pub fn layout_type(style: &UIStyle) -> LayoutType {
    let l = &style.layout;
    let vertical_text = style.vertical_text
        || style.text_orientation.eq_ignore_ascii_case("vertical");
    let horizontal = style.horizontal;
    let mut kind = if vertical_text {
        if style.fullscreen {
            LayoutType::VerticalTextFullscreen
        } else {
            LayoutType::VerticalText
        }
    } else if horizontal {
        if style.fullscreen {
            LayoutType::HorizontalFullscreen
        } else {
            LayoutType::Horizontal
        }
    } else if style.fullscreen {
        LayoutType::VerticalFullscreen
    } else {
        LayoutType::Vertical
    };
    // style/layout/type：字符串名（官方 weasel.toml 用这个）优先，其次是我们早先的整型
    if let Some(named) = match l.layout_type.trim() {
        "vertical" => Some(LayoutType::Vertical),
        "horizontal" => Some(LayoutType::Horizontal),
        "vertical_text" => Some(LayoutType::VerticalText),
        "vertical+fullscreen" => Some(LayoutType::VerticalFullscreen),
        "horizontal+fullscreen" => Some(LayoutType::HorizontalFullscreen),
        "vertical_text+fullscreen" => Some(LayoutType::VerticalTextFullscreen),
        _ => None,
    } {
        kind = named;
    } else if (0..=5).contains(&l.layout_type_mode) {
        kind = [
            LayoutType::Vertical,
            LayoutType::Horizontal,
            LayoutType::VerticalText,
            LayoutType::VerticalFullscreen,
            LayoutType::HorizontalFullscreen,
            LayoutType::VerticalTextFullscreen,
        ][l.layout_type_mode as usize];
    }
    // 全屏竖排要保持稳定（官方 1261-1263）
    if style.fullscreen && vertical_text {
        kind = LayoutType::VerticalTextFullscreen;
    }
    kind
}

/// 文字抗锯齿模式（官方 `UIStyle::AntiAliasMode`，数值与 `D2D1_TEXT_ANTIALIAS_MODE` 一致）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Antialias {
    /// `default`。
    #[default]
    Default,
    /// `cleartype`。
    ClearType,
    /// `grayscale`。
    Grayscale,
    /// `aliased`。
    Aliased,
}

impl Antialias {
    /// 官方 `_aliasModeArr` 的名字表；未知一律回落到 `default`。
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "cleartype" => Self::ClearType,
            "grayscale" => Self::Grayscale,
            "aliased" => Self::Aliased,
            _ => Self::Default,
        }
    }
    /// 对应的 `D2D1_TEXT_ANTIALIAS_MODE` 数值（官方直接强转枚举，取值一致）。
    pub fn d2d(self) -> i32 {
        match self {
            Self::Default => 0,
            Self::ClearType => 1,
            Self::Grayscale => 2,
            Self::Aliased => 3,
        }
    }
}

/// 对齐方式（官方 `style/layout/align_type`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Top,
    Center,
    Bottom,
}

/// 悬停行为（官方 `style/hover_type`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hover {
    None,
    SemiHilite,
    Hilite,
}

/// 已经过夹取与 DPI 缩放的布局数值，供 [`crate::layout`] 与 [`crate::paint`] 直接使用。
///
/// 长度单位是 DIP（96 DPI 下的像素）；字号是 DWrite 的点×比例后的值。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Metrics {
    pub layout_type: LayoutType,
    pub align: Align,
    /// 面板圆角（官方 `round_corner_ex`，键 `corner_radius`）。
    pub round_corner_ex: f32,
    /// 高亮块圆角（官方 `round_corner`，键 `hilited_corner_radius`）。
    pub round_corner: f32,
    pub border: f32,
    pub margin_x: f32,
    pub margin_y: f32,
    pub spacing: f32,
    pub candidate_spacing: f32,
    pub hilite_spacing: f32,
    pub hilite_padding_x: f32,
    pub hilite_padding_y: f32,
    pub shadow_radius: f32,
    pub shadow_offset_x: f32,
    pub shadow_offset_y: f32,
    pub min_width: f32,
    pub max_width: f32,
    pub min_height: f32,
    pub max_height: f32,
    pub linespacing: i32,
    pub baseline: i32,
    /// 文字抗锯齿模式（官方 style/antialias_mode）。
    pub antialias: Antialias,

    pub font_face: String,
    pub label_font_face: String,
    pub comment_font_face: String,
    pub font_point: f32,
    pub label_font_point: f32,
    pub comment_font_point: f32,
    /// DPI 比（`dpi/96`），官方 `m_dpiScaleLayout`。
    pub dpi_scale: f32,
    /// 点→DIP（`dpi/72`），官方 `m_dpiScaleFontPoint`。
    pub dpi_scale_font: f32,

    pub inline_preedit: bool,
    pub label_format: String,
    pub mark_text: String,
    pub hover: Hover,
    pub click_to_capture: bool,
    pub paging_on_scroll: bool,
    pub vertical_auto_reverse: bool,
    pub vertical_text_left_to_right: bool,
    pub vertical_text_with_wrap: bool,

    // ---- 下面这些来自配色而非 style（官方在 Layout 里直接读 `_style` 的颜色）----
    /// 官方 `_pageEnabled`：`prevpage_color` 与 `nextpage_color` 都不透明才画翻页。
    pub page_enabled: bool,
    /// 官方 `_CalcMarkMetrics` 的头一条：标记色全透明就不画标记。
    pub mark_visible: bool,
    /// 候选注释文字的透明度（官方 `cmtFontNotTrans`）：决定注释是否占位。
    pub comment_visible_normal: bool,
    pub comment_visible_hilited: bool,
    /// 官方 `labelFontValid` / `textFontValid` / `cmtFontValid`：字号 > 0 才占位。
    pub label_font_valid: bool,
    pub text_font_valid: bool,
    pub comment_font_valid: bool,
}

impl Metrics {
    /// 按官方顺序：先夹取（未缩放空间），再乘 DPI。
    pub fn new(style: &UIStyle, dpi: f32) -> Self {
        let l = &style.layout;
        let layout_type = layout_type(style);
        let scale = (dpi / 96.0).max(0.01);
        let font_scale = (dpi / 72.0).max(0.01);

        // ① 别名与缺省（官方 1270-1294）
        let border = if l.border != 0 { l.border } else { l.border_width };
        let pad_x = if l.hilite_padding_x != 0 {
            l.hilite_padding_x
        } else {
            l.hilite_padding
        };
        let pad_y = if l.hilite_padding_y != 0 {
            l.hilite_padding_y
        } else {
            l.hilite_padding
        };
        let round_corner = if l.hilited_corner_radius >= 0 {
            l.hilited_corner_radius
        } else {
            l.round_corner
        };
        let round_corner_ex = if l.corner_radius >= 0 {
            l.corner_radius
        } else {
            l.round_corner
        };
        // 全屏时禁用 max_width（官方 1264-1269）
        let mut max_width = l.max_width;
        let mut max_height = l.max_height;
        let mut inline_preedit = style.inline_preedit;
        match layout_type {
            LayoutType::HorizontalFullscreen | LayoutType::VerticalFullscreen => {
                max_width = 0;
                inline_preedit = false;
            }
            LayoutType::VerticalTextFullscreen => max_height = 0,
            _ => {}
        }

        // ② padding 与 spacing 的互相夹取（官方 1295-1333）
        let mut spacing = l.spacing;
        let mut candidate_spacing = l.candidate_spacing;
        let mut hilite_spacing = l.hilite_spacing;
        if layout_type != LayoutType::VerticalText {
            spacing = spacing.max(pad_y * 2);
            if matches!(
                layout_type,
                LayoutType::Vertical | LayoutType::VerticalFullscreen
            ) {
                candidate_spacing = candidate_spacing.max(pad_y * 2);
            } else {
                candidate_spacing = candidate_spacing.max(pad_x * 2);
            }
            if !inline_preedit {
                hilite_spacing = hilite_spacing.max(pad_x);
            }
        } else {
            spacing = spacing.max(pad_x * 2);
            candidate_spacing = candidate_spacing.max(pad_x * 2);
            if style.vertical_text_with_wrap {
                candidate_spacing = candidate_spacing.max(pad_y * 2);
            }
            if !inline_preedit {
                hilite_spacing = hilite_spacing.max(pad_y);
            }
        }

        // ③ margin 夹取（官方 1337-1341）：符号保留，绝对值不小于 padding
        let margin_sign_x = if l.margin_x < 0 { -1 } else { 1 };
        let margin_sign_y = if l.margin_y < 0 { -1 } else { 1 };
        let margin_x = margin_sign_x * pad_x.max(l.margin_x.abs());
        let margin_y = margin_sign_y * pad_y.max(l.margin_y.abs());

        // ⑤ 间距与圆角跟**字高**绑定（用户 2026-11-01 定）：yaml 里那些写死的像素
        //    不再驱动布局，字号一变整窗比例跟着变，不用再逐个改配置。
        //      候选之间   = 一个全角字符 = 字高
        //      候选小圆角 = 字高 / 2
        //      外侧大圆角 = 字高
        //      编码与候选的上下、左右各留一个字高
        let font_h = style.font_point as f32 * font_scale;
        // 上面 ①~③ 算出来的这几个值不再生效，显式吞掉免得编译告警。
        let _ = (
            round_corner,
            round_corner_ex,
            spacing,
            candidate_spacing,
            margin_x,
            margin_y,
        );

        // ④ 统一乘 DPI（官方 Layout.cpp:24-43，逐项取整）
        let scaled = |value: i32| (value as f32 * scale) as i32 as f32;
        let abs_scaled = |value: i32| (value.abs() as f32 * scale) as i32 as f32;

        let metrics = Self {
            layout_type,
            align: match l.align_type.to_ascii_lowercase().as_str() {
                "top" => Align::Top,
                "bottom" => Align::Bottom,
                _ => Align::Center,
            },
            // 圆角 / 间距 / 内边距**全部按字高推导**（= HTML 预览里的 em）：
            //   大框四边 1em、大框圆角 1em、小框四边 0.5em、小框圆角 0.5em、
            //   框与框 0.5em、编码与框 0.5em、编码与箭头最小间距 0.5em
            //
            // ⚠️ 关键：高亮框是**以文字为中心向外扩 hilite_padding**（官方语义，
            //    `Layout::inflated_candidate`），而 HTML 的 padding 是**向内撑**。
            //    所以「面板边→框边」= margin − pad、「编码→框」= spacing − pad、
            //    「框↔框」= candidate_spacing − 2×pad。要让外面的**可见间隙**等于
            //    HTML 基准（1em / 0.5em / 0.5em），必须把这些 pad 加回去。
            //    2026-11-01 16:2x 用户报「编码与候选贴在一起、左侧不是 1em」就是这么来的：
            //    原来 margin=1em、spacing=0.5em、candidate_spacing=0.5em 直接被 pad 吃掉，
            //    实际可见间隙是 0.5em / 0 / −0.5em（相邻两框还互相重叠）。
            round_corner_ex: font_h,
            round_corner: font_h / 2.0,
            border: abs_scaled(border),
            // margin 仍保留 yaml 的符号（负 margin = 内容往外挪），绝对值 = 1em 边距 + pad 外扩
            margin_x: margin_sign_x as f32 * (font_h + font_h / 2.0),
            margin_y: margin_sign_y as f32 * (font_h + font_h / 2.0),
            // 编码 ↔ 候选框：0.5em 可见间隙 + 框外扩 0.5em
            spacing: font_h,
            // 候选框 ↔ 相邻候选**文字**：框外扩 0.5em + 可见间隙 0.5em（= 1em）。
            // HTML 里只有"当前高亮项"带 0.5em padding，所以框边到邻居文字就是 --item-gap 0.5em；
            // 两框间距因此正好 2×pad（相切、不重叠，也满足官方的 candidate_spacing ≥ 2×padding）。
            candidate_spacing: font_h,
            // 编码 ↔ 翻页箭头：箭头不吃外扩，0.5em 就是 0.5em
            hilite_spacing: font_h / 2.0,
            hilite_padding_x: font_h / 2.0,
            hilite_padding_y: font_h / 2.0,
            shadow_radius: abs_scaled(l.shadow_radius),
            shadow_offset_x: scaled(l.shadow_offset_x),
            shadow_offset_y: scaled(l.shadow_offset_y),
            min_width: abs_scaled(l.min_width),
            max_width: abs_scaled(max_width),
            min_height: abs_scaled(l.min_height),
            max_height: abs_scaled(max_height),
            linespacing: l.linespacing,
            baseline: l.baseline,
            antialias: Antialias::from_name(&style.antialias_mode),
            font_face: style.font_face.clone(),
            label_font_face: style.label_font_face.clone(),
            comment_font_face: style.comment_font_face.clone(),
            font_point: style.font_point as f32 * font_scale,
            label_font_point: style.label_font_point as f32 * font_scale,
            comment_font_point: style.comment_font_point as f32 * font_scale,
            dpi_scale: scale,
            dpi_scale_font: font_scale,
            inline_preedit,
            label_format: if style.label_format.is_empty() {
                "%s.".to_string()
            } else {
                style.label_format.clone()
            },
            mark_text: style.mark_text.clone(),
            hover: match style.hover_type.to_ascii_lowercase().as_str() {
                "hilite" => Hover::Hilite,
                "semi_hilite" | "semi-hilite" | "semihilite" => Hover::SemiHilite,
                _ => Hover::None,
            },
            click_to_capture: style.click_to_capture,
            paging_on_scroll: style.paging_on_scroll,
            vertical_auto_reverse: style.vertical_auto_reverse,
            vertical_text_left_to_right: style.vertical_text_left_to_right,
            vertical_text_with_wrap: style.vertical_text_with_wrap,
            page_enabled: false,
            mark_visible: false,
            comment_visible_normal: true,
            comment_visible_hilited: true,
            label_font_valid: style.label_font_point > 0,
            text_font_valid: style.font_point > 0,
            comment_font_valid: style.comment_font_point > 0,
        };
        crate::diagnostics::record(format_args!(
            "尺寸生效：字高 {:.1} · marginX {:.1} · marginY {:.1} · spacing {:.1} · candSpacing {:.1} · hiliteSpacing {:.1} · hilitePad {:.1}/{:.1} · 圆角 {:.1}/{:.1}",
            metrics.font_point,
            metrics.margin_x,
            metrics.margin_y,
            metrics.spacing,
            metrics.candidate_spacing,
            metrics.hilite_spacing,
            metrics.hilite_padding_x,
            metrics.hilite_padding_y,
            metrics.round_corner_ex,
            metrics.round_corner
        ));
        metrics
    }

    /// 把配色带来的开关补上（官方 `Layout` 里直接读 `_style` 的那些颜色判断）。
    pub fn with_colors(mut self, scheme: &ColorScheme) -> Self {
        let opaque = |color: u32| (color >> 24) != 0;
        // 官方 StandardLayout 构造：_pageEnabled = prevpage && nextpage 都不透明
        self.page_enabled = opaque(scheme.prevpage_color) && opaque(scheme.nextpage_color);
        self.mark_visible = opaque(scheme.hilited_mark_color);
        self.comment_visible_normal = opaque(scheme.comment_text_color);
        self.comment_visible_hilited = opaque(scheme.hilited_comment_text_color);
        self
    }

    /// 官方 `VerticalLayout` 里判断注释列是否占位：
    /// `!comments[i].str.empty() && cmtFontValid && cmtFontNotTrans`。
    pub fn comment_shown(&self, text_empty: bool, is_selected: bool) -> bool {
        if text_empty || !self.comment_font_valid {
            return false;
        }
        if is_selected {
            self.comment_visible_hilited
        } else {
            self.comment_visible_normal
        }
    }

    /// 官方 `Layout` 构造里的 `real_margin = max(|margin|, hilite_padding)`。
    ///
    /// 夹取后 margin 已经满足这个关系，这里再取一次是为了对齐官方（负 margin 时也用绝对值）。
    pub fn real_margin_x(&self) -> f32 {
        self.margin_x.abs().max(self.hilite_padding_x)
    }
    pub fn real_margin_y(&self) -> f32 {
        self.margin_y.abs().max(self.hilite_padding_y)
    }

    /// 官方 `Layout.cpp:51-60` 的 `offsetX/offsetY`：给阴影与描边留出的外扩。
    pub fn offset(&self) -> (f32, f32) {
        let mut x = 0.0;
        let mut y = 0.0;
        if self.shadow_radius != 0.0 {
            x = self.shadow_offset_x.abs() + self.shadow_radius * 2.0;
            y = self.shadow_offset_y.abs() + self.shadow_radius * 2.0;
            if self.shadow_offset_x != 0.0 || self.shadow_offset_y != 0.0 {
                x -= self.shadow_radius / 2.0;
                y -= self.shadow_radius / 2.0;
            }
        }
        (x + self.border * 2.0, y + self.border * 2.0)
    }

    /// 把官方 `style/label_format`（`%s` 一处）套到候选序号上。
    ///
    /// 官方用 `swprintf_s(buffer, format, label)`，只认一个 `%s`；`%%` 是转义的百分号。
    pub fn format_label(&self, label: &str) -> String {
        let mut out = String::new();
        let mut chars = self.label_format.chars().peekable();
        let mut used = false;
        while let Some(ch) = chars.next() {
            if ch == '%' {
                match chars.peek() {
                    Some('%') => {
                        chars.next();
                        out.push('%');
                    }
                    Some('s') => {
                        chars.next();
                        out.push_str(label);
                        used = true;
                    }
                    _ => out.push(ch),
                }
            } else {
                out.push(ch);
            }
        }
        if !used {
            // 官方格式里没有 %s 时整串原样输出——那也照做，但补上序号更接近官方的可用观感
            out.push_str(label);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-30：这条守的是"每键重载主题"那个 bug。
    ///
    /// `Window::render` 拿 `stamp_cached()` 跟 `App::stamp` 比，只要两边取值不一致
    /// 就会判定"主题变了"→ 重载（读 YAML + 写日志）。两边现在都走同一个缓存，
    /// 所以 TTL 内必须恒等；真正读过配置后（`refresh_stamp_cache`）也必须恒等。
    #[test]
    fn stamp_cached_is_stable_so_reload_does_not_fire_per_key() {
        let first = stamp_cached();
        let second = stamp_cached();
        assert_eq!(first, second, "TTL 内两次取值必须一致");
        refresh_stamp_cache();
        assert_eq!(stamp_cached(), first, "文件没动，刷新后仍是同一个时间戳");
    }

    fn official_default() -> UIStyle {
        UIStyle::default()
    }

    #[test]
    fn defaults_equal_official_weasel_yaml() {
        let m = Metrics::new(&official_default(), 96.0);
        assert_eq!(m.layout_type, LayoutType::Vertical);
        assert_eq!(m.align, Align::Center);
        assert_eq!(m.border, 3.0);
        // 间距/圆角跟字高绑定（见 Metrics::new ⑤）：14pt 在 96 DPI 下 = 18.6667 DIP
        let font_h = 14.0f32 * 96.0 / 72.0;
        let pad = font_h / 2.0;
        // margin = 1em 面板内边距 + 0.5em 高亮框外扩 = 1.5em
        assert!((m.margin_x - font_h * 1.5).abs() < 0.01, "margin_x = {}", m.margin_x);
        assert!((m.margin_y - font_h * 1.5).abs() < 0.01, "margin_y = {}", m.margin_y);
        assert!((m.spacing - font_h).abs() < 0.01, "spacing = {}", m.spacing);
        assert!(
            (m.candidate_spacing - font_h).abs() < 0.01,
            "候选间文字距 = 1em（框外可见 0.5em）= {}",
            m.candidate_spacing
        );
        assert!((m.hilite_spacing - pad).abs() < 0.01, "hilite_spacing = {}", m.hilite_spacing);
        assert!((m.hilite_padding_x - pad).abs() < 0.01, "小框四边 = 0.5em = {}", m.hilite_padding_x);
        assert!((m.hilite_padding_y - pad).abs() < 0.01);
        assert!(
            (m.round_corner - font_h / 2.0).abs() < 0.01,
            "候选小圆角 = 半个字高 = {}",
            m.round_corner
        );
        assert!(
            (m.round_corner_ex - font_h).abs() < 0.01,
            "外侧大圆角 = 一个字高 = {}",
            m.round_corner_ex
        );
        assert_eq!(m.min_width, 160.0);
        assert_eq!(m.shadow_radius, 0.0);
        // 官方 aqua 没有阴影 → offset 只有 border*2
        assert_eq!(m.offset(), (6.0, 6.0));
        // 14pt 在 96 DPI 下 = 14 * 96/72 DIP
        assert!((m.font_point - 18.6667).abs() < 0.01, "font_point = {}", m.font_point);
    }

    #[test]
    fn dpi_scaling_matches_official_layout_ctor() {
        let m = Metrics::new(&official_default(), 144.0);
        assert_eq!(m.border, 4.0, "3 * 144/96 取整");
        // 字高 = 14 * 144/72 = 28 DIP ⇒ margin = 1.5em = 42
        assert!((m.margin_x - 42.0).abs() < 0.01, "margin_x = {}", m.margin_x);
        assert!((m.margin_y - 42.0).abs() < 0.01, "margin_y = {}", m.margin_y);
        assert!((m.spacing - 28.0).abs() < 0.01, "spacing = 1em");
        assert!((m.candidate_spacing - 28.0).abs() < 0.01, "candidate_spacing = 1em");
        assert_eq!(m.min_width, 240.0);
        assert!((m.round_corner - 14.0).abs() < 0.01, "候选小圆角 = 字高/2 = {}", m.round_corner);
        assert!((m.round_corner_ex - 28.0).abs() < 0.01, "外侧大圆角 = 字高 = {}", m.round_corner_ex);
        assert!((m.font_point - 28.0).abs() < 0.01, "14 * 144/72");
        // border 3 在 144 DPI = 4（取整），offset 是 border*2 → 8
        let (ox, oy) = m.offset();
        assert_eq!((ox, oy), (8.0, 8.0), "border 取整后再乘 2");
    }

    /// 2026-11-01 定：⑤ 段把 margin / spacing / candidate_spacing / padding 全部
    /// 改成**按字高推导**（等价 CSS em），yaml 里这些旋钮一律失效，只剩 `font_point`
    /// 能调大小 —— 这条守住"改 yaml 也不会偷偷改布局"。
    #[test]
    fn layout_knobs_in_yaml_are_ignored() {
        let mut style = official_default();
        style.layout.hilite_padding = 6;
        style.layout.hilite_padding_x = 6;
        style.layout.hilite_padding_y = 6;
        style.layout.spacing = 1;
        style.layout.candidate_spacing = 1;
        style.layout.hilite_spacing = 1;
        style.layout.margin_x = 0;
        style.layout.margin_y = 0;
        let m = Metrics::new(&style, 96.0);
        let font_h = 14.0f32 * 96.0 / 72.0;
        assert!((m.margin_x - font_h * 1.5).abs() < 0.01, "margin_x = {}", m.margin_x);
        assert!((m.margin_y - font_h * 1.5).abs() < 0.01, "margin_y = {}", m.margin_y);
        assert!((m.spacing - font_h).abs() < 0.01, "spacing = {}", m.spacing);
        assert!(
            (m.candidate_spacing - font_h).abs() < 0.01,
            "candidate_spacing = {}",
            m.candidate_spacing
        );
        assert!((m.hilite_spacing - font_h / 2.0).abs() < 0.01, "hilite_spacing = {}", m.hilite_spacing);
        assert!((m.hilite_padding_x - font_h / 2.0).abs() < 0.01, "hilite_padding_x = {}", m.hilite_padding_x);
        assert!((m.hilite_padding_y - font_h / 2.0).abs() < 0.01, "hilite_padding_y = {}", m.hilite_padding_y);
    }

    #[test]
    fn negative_margin_keeps_sign() {
        let mut style = official_default();
        style.layout.margin_x = -20;
        style.layout.margin_y = -1;
        let m = Metrics::new(&style, 96.0);
        // 绝对值不再被 padding 顶，固定 1.5em（1em 边距 + 0.5em 框外扩）；yaml 的 -20 / -1 只留符号
        let font_h = 14.0f32 * 96.0 / 72.0;
        assert!(
            (m.margin_x + font_h * 1.5).abs() < 0.01,
            "负 margin 保持负号 = {}",
            m.margin_x
        );
        assert!((m.margin_y + font_h * 1.5).abs() < 0.01, "= {}", m.margin_y);
        // real_margin 取绝对值
        assert!((m.real_margin_x() - font_h * 1.5).abs() < 0.01);
        assert!((m.real_margin_y() - font_h * 1.5).abs() < 0.01);
    }

    #[test]
    fn layout_type_switches() {
        let mut style = official_default();
        assert_eq!(layout_type(&style), LayoutType::Vertical);
        style.horizontal = true;
        assert_eq!(layout_type(&style), LayoutType::Horizontal);
        style.fullscreen = true;
        assert_eq!(layout_type(&style), LayoutType::HorizontalFullscreen);
        style.horizontal = false;
        assert_eq!(layout_type(&style), LayoutType::VerticalFullscreen);
        style.vertical_text = true;
        assert_eq!(layout_type(&style), LayoutType::VerticalTextFullscreen);
        style.fullscreen = false;
        assert_eq!(layout_type(&style), LayoutType::VerticalText);
        // style/layout/type 覆盖
        style.vertical_text = false;
        style.layout.layout_type = "horizontal".into();
        assert_eq!(layout_type(&style), LayoutType::Horizontal);
    }

    #[test]
    fn label_format_applies_like_swprintf() {
        let m = Metrics::new(&official_default(), 96.0);
        assert_eq!(m.format_label("1"), "1.");
        let mut style = official_default();
        style.label_format = "[%s] 100%%".into();
        let m = Metrics::new(&style, 96.0);
        assert_eq!(m.format_label("7"), "[7] 100%");
    }

    #[test]
    fn fullscreen_disables_max_width_and_inline_preedit() {
        let mut style = official_default();
        style.fullscreen = true;
        style.inline_preedit = true;
        style.layout.max_width = 500;
        let m = Metrics::new(&style, 96.0);
        assert_eq!(m.max_width, 0.0);
        assert!(!m.inline_preedit);
    }
}

/// 构造度量；`plain = true`（纯白/毛玻璃档）时把"给投影留的窗口边"收掉。
///
/// 那圈边（[`Metrics::offset`]，≈ shadow_radius×2 + border×2 ≈ 8px）是给投影落脚用的。
/// plain 档不画投影，却仍然在面板外留一圈**没糊的桌面**：面板里是糊过的、外面那圈是锐的，
/// 界线上就是一道"黑框"（用户 2026-11-01 17:5x：「为什么还是有黑框」）。
pub fn metrics(style: &UIStyle, dpi: f32, plain: bool) -> Metrics {
    let mut metrics = Metrics::new(style, dpi);
    if plain {
        metrics.shadow_radius = 0.0;
        metrics.shadow_offset_x = 0.0;
        metrics.shadow_offset_y = 0.0;
    }
    metrics
}
