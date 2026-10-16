//! 读取我们约定的主题文件 `theme\<名字>.theme.yaml`（例如 `default.theme.yaml`）。
//!
//! 这份文件由 `theme-toml` 这个 bin 从官方 `weasel.yaml` 生成（见 `src/bin/theme_toml.rs`），
//! 内容是官方 style 段与 layout 段的全部字段，加上 `preset_color_schemes` 里每一套配色。
//! librime 已打补丁把 `.theme.yaml` 归入 `theme/` 子目录，因此运行期路径是：
//!
//! * 共享数据：`<程序文件夹>\Rime-Next\theme\default.theme.yaml`
//! * 用户覆盖：`%APPDATA%\Rime-Next\theme\default.theme.yaml`（可选，优先）
//!
//! 设计要点与官方一致：**皮肤是数据不是代码**。本模块只负责把文件读成结构体，
//! 再转换成与 `weasel.yaml` 完全相同的 [`WeaselConfig`]，这样消费方（渲染器、设置界面）
//! 只需要认一种类型，不必关心数据来自 YAML 还是 TOML。

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{ColorScheme, UIStyle, WeaselConfig};

/// TOML 里的 `[style]` 段。字段与官方 style 键一一对应，缺项用官方默认值补齐。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StyleSection {
    pub font_face: Option<String>,
    pub label_font_face: Option<String>,
    pub comment_font_face: Option<String>,
    pub font_point: Option<i32>,
    pub label_font_point: Option<i32>,
    pub comment_font_point: Option<i32>,
    pub candidate_abbreviate_length: Option<i32>,
    pub antialias_mode: Option<String>,
    pub ascii_tip_follow_cursor: Option<bool>,
    pub click_to_capture: Option<bool>,
    pub horizontal: Option<bool>,
    pub fullscreen: Option<bool>,
    pub inline_preedit: Option<bool>,
    pub preedit_type: Option<String>,
    pub display_tray_icon: Option<bool>,
    pub hover_type: Option<String>,
    pub paging_on_scroll: Option<bool>,
    pub vertical_auto_reverse: Option<bool>,
    pub vertical_text: Option<bool>,
    pub vertical_text_left_to_right: Option<bool>,
    pub vertical_text_with_wrap: Option<bool>,
    pub vertical_right_to_left: Option<bool>,
    pub enhanced_position: Option<bool>,
    pub text_orientation: Option<String>,
    pub label_format: Option<String>,
    pub mark_text: Option<String>,
    pub layout: Option<LayoutSection>,
}

/// TOML 里的 `[style.layout]` 段。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LayoutSection {
    pub align_type: Option<String>,
    pub min_width: Option<i32>,
    pub max_width: Option<i32>,
    pub min_height: Option<i32>,
    pub max_height: Option<i32>,
    pub border: Option<i32>,
    pub margin_x: Option<i32>,
    pub margin_y: Option<i32>,
    pub spacing: Option<i32>,
    pub candidate_spacing: Option<i32>,
    pub hilite_spacing: Option<i32>,
    pub hilite_padding: Option<i32>,
    pub hilite_padding_x: Option<i32>,
    pub hilite_padding_y: Option<i32>,
    pub round_corner: Option<i32>,
    pub corner_radius: Option<i32>,
    pub hilited_corner_radius: Option<i32>,
    pub shadow_radius: Option<i32>,
    pub shadow_offset_x: Option<i32>,
    pub shadow_offset_y: Option<i32>,
    pub linespacing: Option<i32>,
    pub baseline: Option<i32>,
    pub layout_type_mode: Option<i32>,
    /// 官方 `style/layout/type` 的字符串形式（vertical / horizontal / …）
    pub layout_type: Option<String>,
}

/// TOML 里的一套配色 `[schemes.<id>]`。颜色写成 `"0xRRGGBBAA"` 之类的字符串。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SchemeSection {
    pub name: Option<String>,
    pub author: Option<String>,
    pub color_format: Option<String>,
    pub back_color: Option<String>,
    pub border_color: Option<String>,
    pub shadow_color: Option<String>,
    pub text_color: Option<String>,
    pub candidate_text_color: Option<String>,
    pub candidate_back_color: Option<String>,
    pub candidate_border_color: Option<String>,
    pub candidate_shadow_color: Option<String>,
    pub label_color: Option<String>,
    pub comment_text_color: Option<String>,
    pub hilited_text_color: Option<String>,
    pub hilited_back_color: Option<String>,
    pub hilited_border_color: Option<String>,
    pub hilited_shadow_color: Option<String>,
    pub hilited_candidate_text_color: Option<String>,
    pub hilited_candidate_back_color: Option<String>,
    pub hilited_candidate_border_color: Option<String>,
    pub hilited_candidate_shadow_color: Option<String>,
    pub hilited_label_color: Option<String>,
    pub hilited_comment_text_color: Option<String>,
    pub hilited_mark_color: Option<String>,
    pub nextpage_color: Option<String>,
    pub prevpage_color: Option<String>,
}

/// 整份主题文件。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThemeFile {
    pub config_version: Option<String>,
    pub color_scheme: Option<String>,
    pub color_scheme_dark: Option<String>,
    pub style: Option<StyleSection>,
    pub schemes: BTreeMap<String, SchemeSection>,
}

/// 解析主题 TOML 文本。
pub fn parse_theme(text: &str) -> Result<ThemeFile, String> {
    weasel_common::yaml::from_str(text)
}

/// 一步到位：解析文件。
pub fn parse_file(path: impl AsRef<Path>) -> Result<ThemeFile, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse_theme(&text)?)
}

impl ThemeFile {
    /// 转成与 `weasel.yaml` 同构的 [`WeaselConfig`]，缺项沿用官方默认值。
    pub fn into_config(self) -> WeaselConfig {
        let mut config = WeaselConfig {
            config_version: self.config_version.unwrap_or_default(),
            style: UIStyle::default(),
            ..WeaselConfig::default()
        };

        if let Some(style) = self.style {
            let s = &mut config.style;
            macro_rules! take {
                ($field:ident) => {
                    if let Some(value) = style.$field {
                        s.$field = value;
                    }
                };
            }
            take!(font_face);
            take!(label_font_face);
            take!(comment_font_face);
            take!(font_point);
            take!(label_font_point);
            take!(comment_font_point);
            take!(candidate_abbreviate_length);
            take!(antialias_mode);
            take!(ascii_tip_follow_cursor);
            take!(click_to_capture);
            take!(horizontal);
            take!(fullscreen);
            take!(inline_preedit);
            take!(preedit_type);
            take!(display_tray_icon);
            take!(hover_type);
            take!(paging_on_scroll);
            take!(vertical_auto_reverse);
            take!(vertical_text);
            take!(vertical_text_left_to_right);
            take!(vertical_text_with_wrap);
            take!(vertical_right_to_left);
            take!(enhanced_position);
            take!(text_orientation);
            take!(label_format);
            take!(mark_text);

            if let Some(layout) = style.layout {
                let l = &mut s.layout;
                macro_rules! take_layout {
                    ($field:ident) => {
                        if let Some(value) = layout.$field {
                            l.$field = value;
                        }
                    };
                }
                take_layout!(align_type);
                take_layout!(min_width);
                take_layout!(max_width);
                take_layout!(min_height);
                take_layout!(max_height);
                take_layout!(border);
                take_layout!(margin_x);
                take_layout!(margin_y);
                take_layout!(spacing);
                take_layout!(candidate_spacing);
                take_layout!(hilite_spacing);
                take_layout!(hilite_padding);
                take_layout!(hilite_padding_x);
                take_layout!(hilite_padding_y);
                take_layout!(round_corner);
                take_layout!(corner_radius);
                take_layout!(hilited_corner_radius);
                take_layout!(shadow_radius);
                take_layout!(shadow_offset_x);
                take_layout!(shadow_offset_y);
                take_layout!(linespacing);
                take_layout!(baseline);
                take_layout!(layout_type_mode);
                take_layout!(layout_type);
            }
        }

        if let Some(name) = self.color_scheme {
            config.style.color_scheme = name;
        }
        config.style.color_scheme_dark = self.color_scheme_dark;

        for (id, section) in self.schemes {
            let color_format = section.color_format.clone().unwrap_or_default();
            let format = crate::ColorFormat::from_name(&color_format);
            let mut scheme = ColorScheme {
                name: section.name.unwrap_or_else(|| id.clone()),
                author: section.author.unwrap_or_default(),
                color_format,
                ..ColorScheme::default()
            };
            let put = |raw: Option<String>, slot: &mut u32| {
                if let Some(text) = raw {
                    if let Some(value) = crate::color::parse(&text, format) {
                        *slot = value;
                    }
                }
            };
            put(section.back_color, &mut scheme.back_color);
            put(section.border_color, &mut scheme.border_color);
            put(section.shadow_color, &mut scheme.shadow_color);
            put(section.text_color, &mut scheme.text_color);
            put(section.candidate_text_color, &mut scheme.candidate_text_color);
            put(section.candidate_back_color, &mut scheme.candidate_back_color);
            put(section.candidate_border_color, &mut scheme.candidate_border_color);
            put(section.candidate_shadow_color, &mut scheme.candidate_shadow_color);
            put(section.label_color, &mut scheme.label_color);
            put(section.comment_text_color, &mut scheme.comment_text_color);
            put(section.hilited_text_color, &mut scheme.hilited_text_color);
            put(section.hilited_back_color, &mut scheme.hilited_back_color);
            put(section.hilited_border_color, &mut scheme.hilited_border_color);
            put(section.hilited_shadow_color, &mut scheme.hilited_shadow_color);
            put(section.hilited_candidate_text_color, &mut scheme.hilited_candidate_text_color);
            put(section.hilited_candidate_back_color, &mut scheme.hilited_candidate_back_color);
            put(section.hilited_candidate_border_color, &mut scheme.hilited_candidate_border_color);
            put(section.hilited_candidate_shadow_color, &mut scheme.hilited_candidate_shadow_color);
            put(section.hilited_label_color, &mut scheme.hilited_label_color);
            put(section.hilited_comment_text_color, &mut scheme.hilited_comment_text_color);
            put(section.hilited_mark_color, &mut scheme.hilited_mark_color);
            put(section.nextpage_color, &mut scheme.nextpage_color);
            put(section.prevpage_color, &mut scheme.prevpage_color);
            config.schemes.insert(id, scheme.resolve());
        }

        config
    }
}

/// 读主题里的"编码画在哪"这一个开关（官方 `style/inline_preedit`）。
///
/// 语义与官方一致：**false = 组字串画进候选窗**（官方出厂默认），true = 画在宿主应用里。
/// 这是主题（布局）的一部分，所以来源是 `theme.yaml`，**不是**宿主配置：
///
/// * 用户目录 `%APPDATA%\Rime-Next\theme\theme.yaml` 优先；
/// * 其次程序文件夹 `<程序>\Rime-Next\theme\theme.yaml`；
/// * 都没有（或解析失败）就用官方出厂值 `false`。
///
/// `shared` 传程序文件夹里的 `Rime-Next`，`user` 传 `%APPDATA%\Rime-Next`。
pub fn inline_preedit(shared: &Path, user: &Path, name: &str) -> bool {
    match load_theme(shared, user, name) {
        Ok(Some(config)) => config.style.inline_preedit,
        _ => UIStyle::default().inline_preedit,
    }
}

/// 主题文件的搜索路径：先用户目录，再共享目录（与 librime 的查找顺序一致）。
///
/// `shared` 传程序文件夹里的 `Rime-Next`，`user` 传 `%APPDATA%\Rime-Next`，
/// `name` 是主题名（例如 `default`）。
pub fn theme_candidates(shared: &Path, user: &Path, name: &str) -> Vec<PathBuf> {
    let file = format!("{name}.theme.yaml");
    vec![
        // 树结构：theme/<名字>/<名字>.theme.yaml（首选）
        user.join("theme").join(name).join(&file),
        shared.join("theme").join(name).join(&file),
        // 兼容旧的扁平位置：theme/<名字>.theme.yaml
        user.join("theme").join(&file),
        shared.join("theme").join(file),
    ]
}

/// 按搜索顺序读第一个存在的主题文件。
/// 合并两份主题文件：主文件（布局 + 用哪套皮肤）与皮肤文件（只有配色）。
fn merge_theme(main: ThemeFile, skin: ThemeFile) -> ThemeFile {
    ThemeFile {
        config_version: main.config_version.or(skin.config_version),
        color_scheme: main.color_scheme.or(skin.color_scheme),
        color_scheme_dark: main.color_scheme_dark.or(skin.color_scheme_dark),
        style: main.style.or(skin.style),
        // 配色以皮肤文件为准；皮肤文件没有则退回主文件里的（兼容拆分前的旧文件）。
        schemes: if skin.schemes.is_empty() { main.schemes } else { skin.schemes },
    }
}

/// 主题主文件的位置：`theme\theme.yaml`（用户覆盖优先，其次共享）。
pub fn theme_main_candidates(shared: &Path, user: &Path) -> Vec<PathBuf> {
    vec![user.join("theme").join("theme.yaml"), shared.join("theme").join("theme.yaml")]
}

/// 读主题：主文件（`theme\theme.yaml`）+ 皮肤文件（`theme\<名字>\<名字>.theme.yaml`）合并。
///
/// 两者都可缺：只有主文件时按旧行为直接用；只有皮肤文件时也能工作。都没有则返回 `None`。
pub fn load_theme(
    shared: &Path,
    user: &Path,
    name: &str,
) -> Result<Option<WeaselConfig>, Box<dyn std::error::Error>> {
    let mut main: Option<ThemeFile> = None;
    for candidate in theme_main_candidates(shared, user) {
        if candidate.is_file() {
            main = Some(parse_file(candidate)?);
            break;
        }
    }
    let mut skin: Option<ThemeFile> = None;
    for candidate in theme_candidates(shared, user, name) {
        if candidate.is_file() {
            skin = Some(parse_file(candidate)?);
            break;
        }
    }
    let merged = match (main, skin) {
        (Some(main), Some(skin)) => merge_theme(main, skin),
        (Some(main), None) => main,
        (None, Some(skin)) => skin,
        (None, None) => return Ok(None),
    };
    Ok(Some(merged.into_config()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 出厂皮肤文件（只有配色；布局与配色选择属于用户设置）。
    const DEFAULT_THEME: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../Rime-Next/theme/default/default.theme.yaml");

    #[test]
    fn factory_skin_stands_alone_with_official_defaults() {
        // 出厂侧不再分发 theme.yaml（布局/配色选择是用户设置），所以皮肤文件必须能单独用：
        // 配色来自文件，布局走 UIStyle::default()（= 官方 weasel.yaml 的出厂值）。
        let skin = parse_file(DEFAULT_THEME).expect("default.theme.yaml 必须能解析");
        let config = merge_theme(ThemeFile::default(), skin).into_config();
        println!("配色数量 = {}", config.schemes.len());
        println!("布局 = {:?}", config.style.layout);
        println!("字体 = {:?} {}", config.style.font_face, config.style.font_point);

        assert_eq!(config.schemes.len(), 36, "官方 36 套配色必须齐全");
        assert_eq!(config.style.font_face, "Microsoft YaHei");
        assert_eq!(config.style.font_point, 14);
        // 布局缺省必须等于官方 weasel.yaml，否则"没写 theme.yaml"就会渲染走样
        assert_eq!(config.style.layout.min_width, 160);
        assert_eq!(config.style.layout.border, 3);
        assert_eq!(config.style.layout.round_corner, 4, "官方 weasel.yaml 的 round_corner");
        assert_eq!(config.style.layout.corner_radius, 4);
        assert_eq!(config.style.layout.shadow_offset_x, 4);
    }

    #[test]
    fn skin_colors_keep_official_alpha_semantics() {
        let skin = parse_file(DEFAULT_THEME).expect("default.theme.yaml 必须能解析");
        let config = merge_theme(ThemeFile::default(), skin).into_config();
        let aqua = config.scheme("aqua");
        // 不透明黑与全透明必须分得开（生成器写 6 位/8 位就是为这个）
        assert_eq!(aqua.text_color >> 24, 0xff, "text_color 0x000000 应是不透明黑");
        assert_eq!(aqua.shadow_color, 0, "shadow_color 0x00000000 应是全透明");
        // 高亮候选底色 0xfa3a0a（abgr）→ RGB(0x0a,0x3a,0xfa)
        assert_eq!(aqua.hilited_candidate_back_color, 0xff0a_3afa);
        for (id, scheme) in &config.schemes {
            assert!(
                scheme.text_color >> 24 == 0xff,
                "配色 {id} 的文字色应被补齐为不透明"
            );
        }
    }

    #[test]
    fn search_order_prefers_user_directory() {
        let shared = Path::new("C:\\share");
        let user = Path::new("C:\\user");
        let candidates = theme_candidates(shared, user, "default");
        assert_eq!(candidates[0], PathBuf::from("C:\\user\\theme\\default\\default.theme.yaml"));
        assert_eq!(candidates[1], PathBuf::from("C:\\share\\theme\\default\\default.theme.yaml"));
    }
}
