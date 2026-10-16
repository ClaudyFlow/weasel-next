//! 主题配置：读 `theme.toml`（布局 + 用哪套皮肤）与 `<皮肤>/<皮肤>.theme.toml`（配色），
//! 再按官方 `RimeWithWeasel.cpp` 的语义算成渲染用的数值。
//!
//! 三件事必须与官方逐条对齐，否则界面就是"像而不一样"：
//!
//! 1. **出厂默认值写在代码里**（官方 `output/data/weasel.toml` 的 `style:` 段）。
//!    出厂侧不再分发 `theme.toml`，所以缺文件也要有官方观感。默认值由
//!    [`weasel_style::UIStyle::default`]／[`weasel_style::Layout::default`] 提供。
//! 2. **官方在读完配置后还会做一批夹取**（`RimeWithWeasel.cpp:1295-1341`）：
//!    `spacing ≥ hilite_padding*2`、`candidate_spacing ≥ hilite_padding*2`、
//!    `hilite_spacing ≥ hilite_padding`、`margin = sign(margin) * max(padding, |margin|)`。
//! 3. **尺寸在布局前统一乘 DPI**（官方 `Layout.cpp:24-43`），乘完取整；字号另乘
//!    `dpi/72`（点是 1/72 英寸）。两者比例不同，别混用。

use weasel_style::theme::load_theme;
use weasel_style::{ColorScheme, UIStyle};
use std::path::PathBuf;

/// 出厂皮肤名（数据在 `<程序文件夹>\Rime-Next\theme\default\default.theme.toml`）。
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
        .and_then(|exe| exe.parent().map(|dir| dir.join("Rime-Next")))?;
    let user = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|base| base.join("Rime-Next"))?;
    Some((shared, user))
}

/// 主题相关文件的"最后修改时间"（取最新的一份）。
///
/// 设置界面换皮肤只是改 `theme.toml` 的 `color_scheme` 一行，运行中的渲染器不会收到
/// 任何通知（主题只在系统深浅色变化时重载）。所以渲染前比一次时间戳，变了自己重载，
/// 换皮肤才不用重启输入法。
pub fn stamp(shared: &std::path::Path, user: &std::path::Path) -> Option<std::time::SystemTime> {
    let mut latest: Option<std::time::SystemTime> = None;
    let paths = weasel_style::theme::theme_candidates(shared, user, SKIN)
        .into_iter()
        .chain([
            user.join("theme").join("theme.toml"),
            shared.join("theme").join("theme.toml"),
        ]);
    for path in paths {
        if let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) {
            latest = Some(latest.map_or(modified, |current| current.max(modified)));
        }
    }
    latest
}

/// 按官方查找顺序读主题：用户目录优先，程序文件夹兜底。
///
/// 程序文件夹由宿主进程的 exe 目录推得（主题 DLL 与渲染器同目录）。两份文件都可缺：
/// 只有皮肤文件时布局走内置默认值；都没有时整份走内置默认值。
pub fn load(dark: bool) -> Loaded {
    let shared = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("Rime-Next")));
    let user = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|base| base.join("Rime-Next"));
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
            // 说明布局取自哪一份（官方顺序：用户目录的 theme.toml 优先，其次程序文件夹）
            let user_main = user.join("theme").join("theme.toml");
            let shared_main = shared.join("theme").join("theme.toml");
            let layout_from = if user_main.is_file() {
                user_main.display().to_string()
            } else if shared_main.is_file() {
                shared_main.display().to_string()
            } else {
                format!("内置默认布局（{} 与 {} 都没有 theme.toml）", user.display(), shared.display())
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
                "未找到 theme.toml / {SKIN}.theme.toml（用户 {} · 共享 {}），用官方内置默认值",
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

        // ④ 统一乘 DPI（官方 Layout.cpp:24-43，逐项取整）
        let scaled = |value: i32| (value as f32 * scale) as i32 as f32;
        let abs_scaled = |value: i32| (value.abs() as f32 * scale) as i32 as f32;

        Self {
            layout_type,
            align: match l.align_type.to_ascii_lowercase().as_str() {
                "top" => Align::Top,
                "bottom" => Align::Bottom,
                _ => Align::Center,
            },
            round_corner_ex: abs_scaled(round_corner_ex),
            round_corner: abs_scaled(round_corner),
            border: abs_scaled(border),
            margin_x: scaled(margin_x),
            margin_y: scaled(margin_y),
            spacing: abs_scaled(spacing),
            candidate_spacing: abs_scaled(candidate_spacing),
            hilite_spacing: abs_scaled(hilite_spacing),
            hilite_padding_x: abs_scaled(pad_x),
            hilite_padding_y: abs_scaled(pad_y),
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
        }
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

    fn official_default() -> UIStyle {
        UIStyle::default()
    }

    #[test]
    fn defaults_equal_official_weasel_yaml() {
        let m = Metrics::new(&official_default(), 96.0);
        assert_eq!(m.layout_type, LayoutType::Vertical);
        assert_eq!(m.align, Align::Center);
        assert_eq!(m.border, 3.0);
        assert_eq!(m.margin_x, 12.0);
        assert_eq!(m.margin_y, 12.0);
        assert_eq!(m.spacing, 10.0);
        assert_eq!(m.candidate_spacing, 5.0);
        assert_eq!(m.hilite_spacing, 4.0);
        assert_eq!(m.hilite_padding_x, 2.0);
        assert_eq!(m.hilite_padding_y, 2.0);
        assert_eq!(m.round_corner, 4.0, "hilited_corner_radius 未填时跟随 round_corner");
        assert_eq!(m.round_corner_ex, 4.0);
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
        assert_eq!(m.margin_x, 18.0);
        assert_eq!(m.min_width, 240.0);
        assert_eq!(m.round_corner, 6.0);
        assert!((m.font_point - 28.0).abs() < 0.01, "14 * 144/72");
        // border 3 在 144 DPI = 4（取整），offset 是 border*2 → 8
        let (ox, oy) = m.offset();
        assert_eq!((ox, oy), (8.0, 8.0), "border 取整后再乘 2");
    }

    #[test]
    fn padding_and_spacing_fixups_follow_official() {
        let mut style = official_default();
        // 官方 1299-1315：spacing/candidate_spacing/hilite_spacing 都要被 padding 顶上去
        style.layout.hilite_padding = 6;
        style.layout.hilite_padding_x = 6;
        style.layout.hilite_padding_y = 6;
        style.layout.spacing = 1;
        style.layout.candidate_spacing = 1;
        style.layout.hilite_spacing = 1;
        style.layout.margin_x = 0;
        style.layout.margin_y = 0;
        let m = Metrics::new(&style, 96.0);
        assert_eq!(m.spacing, 12.0, "spacing ≥ padding_y*2");
        assert_eq!(m.candidate_spacing, 12.0, "纵向：candidate_spacing ≥ padding_y*2");
        assert_eq!(m.hilite_spacing, 6.0, "非 inline：hilite_spacing ≥ padding_x");
        assert_eq!(m.margin_x, 6.0, "margin = max(padding, |margin|)");
        assert_eq!(m.margin_y, 6.0);
    }

    #[test]
    fn negative_margin_keeps_sign() {
        let mut style = official_default();
        style.layout.margin_x = -20;
        style.layout.margin_y = -1;
        let m = Metrics::new(&style, 96.0);
        assert_eq!(m.margin_x, -20.0, "负 margin 保持负号");
        assert_eq!(m.margin_y, -2.0, "绝对值被 padding 顶到 2");
        // 官方 real_margin 取绝对值
        assert_eq!(m.real_margin_x(), 20.0);
        assert_eq!(m.real_margin_y(), 2.0);
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
