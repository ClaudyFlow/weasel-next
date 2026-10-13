//! 读取我们约定的主题文件 `theme\<名字>.theme.toml`（例如 `default.theme.toml`）。
//!
//! 这份文件由 `theme-toml` 这个 bin 从官方 `weasel.yaml` 生成（见 `src/bin/theme_toml.rs`），
//! 内容是官方 style 段与 layout 段的全部字段，加上 `preset_color_schemes` 里每一套配色。
//! librime 已打补丁把 `.theme.toml` 归入 `theme/` 子目录，因此运行期路径是：
//!
//! * 共享数据：`<程序文件夹>\Rime-Next\theme\default.theme.toml`
//! * 用户覆盖：`%APPDATA%\Rime-Next\theme\default.theme.toml`（可选，优先）
//!
//! 设计要点与官方一致：**皮肤是数据不是代码**。本模块只负责把文件读成结构体，
//! 再转换成与 `weasel.yaml` 完全相同的 [`WeaselConfig`]，这样消费方（渲染器、设置界面）
//! 只需要认一种类型，不必关心数据来自 YAML 还是 TOML。

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{ColorScheme, Layout, UIStyle, WeaselConfig};

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
pub fn parse_toml(text: &str) -> Result<ThemeFile, toml::de::Error> {
    toml::from_str(text)
}

/// 一步到位：解析文件。
pub fn parse_file(path: impl AsRef<Path>) -> Result<ThemeFile, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse_toml(&text)?)
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
            }
        }

        if let Some(name) = self.color_scheme {
            config.style.color_scheme = name;
        }
        config.style.color_scheme_dark = self.color_scheme_dark;

        for (id, section) in self.schemes {
            let mut scheme = ColorScheme {
                name: section.name.unwrap_or_else(|| id.clone()),
                author: section.author.unwrap_or_default(),
                color_format: section.color_format.unwrap_or_default(),
                ..ColorScheme::default()
            };
            let mut put = |raw: Option<String>, slot: &mut u32| {
                if let Some(text) = raw {
                    *slot = crate::parse_color(&text);
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
            put(section.nextpage_color, &mut scheme.nextpage_color);
            put(section.prevpage_color, &mut scheme.prevpage_color);
            config.schemes.insert(id, scheme);
        }

        config
    }
}

/// 主题文件的搜索路径：先用户目录，再共享目录（与 librime 的查找顺序一致）。
///
/// `shared` 传程序文件夹里的 `Rime-Next`，`user` 传 `%APPDATA%\Rime-Next`，
/// `name` 是主题名（例如 `default`）。
pub fn theme_candidates(shared: &Path, user: &Path, name: &str) -> Vec<PathBuf> {
    let file = format!("{name}.theme.toml");
    vec![
        // 树结构：theme/<名字>/<名字>.theme.toml（首选）
        user.join("theme").join(name).join(&file),
        shared.join("theme").join(name).join(&file),
        // 兼容旧的扁平位置：theme/<名字>.theme.toml
        user.join("theme").join(&file),
        shared.join("theme").join(file),
    ]
}

/// 按搜索顺序读第一个存在的主题文件。
pub fn load_theme(
    shared: &Path,
    user: &Path,
    name: &str,
) -> Result<Option<WeaselConfig>, Box<dyn std::error::Error>> {
    for candidate in theme_candidates(shared, user, name) {
        if candidate.is_file() {
            return Ok(Some(parse_file(candidate)?.into_config()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_THEME: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../Rime-Next/theme/default.theme.toml");

    #[test]
    fn parses_generated_default_theme() {
        let config = parse_file(DEFAULT_THEME)
            .expect("生成的 default.theme.toml 必须能解析")
            .into_config();
        println!("config_version = {:?}", config.config_version);
        println!("当前配色 = {:?}", config.style.color_scheme);
        println!("配色数量 = {}", config.schemes.len());
        println!("字体 = {:?} {}", config.style.font_face, config.style.font_point);
        println!("布局 = {:?}", config.style.layout);
        let aqua = config.scheme("aqua");
        println!("aqua = {:?} 底色 {:#010x}", aqua.name, aqua.back_color);

        assert_eq!(config.config_version, "0.23");
        assert_eq!(config.style.color_scheme, "aqua");
        assert_eq!(config.schemes.len(), 36, "官方 36 套配色必须齐全");
        assert_eq!(config.style.font_face, "Microsoft YaHei");
        assert_eq!(config.style.font_point, 14);
        assert_eq!(config.style.layout.min_width, 160);
        assert_eq!(config.style.layout.border, 3);
        assert_eq!(config.style.layout.round_corner, 4);
        assert_eq!(config.style.layout.corner_radius, 4);
        assert_eq!(config.style.layout.shadow_offset_x, 4);
        assert!(!aqua.name.is_empty(), "配色的显示名不能为空");
        // 每套配色都要能解出非零颜色（官方文件里就是有值的）
        for (id, scheme) in &config.schemes {
            let any = scheme.back_color
                | scheme.text_color
                | scheme.candidate_text_color
                | scheme.hilited_candidate_back_color;
            assert!(any != 0, "配色 {id} 没有解出任何颜色");
        }
    }

    #[test]
    fn search_order_prefers_user_directory() {
        let shared = Path::new("C:\\share");
        let user = Path::new("C:\\user");
        let candidates = theme_candidates(shared, user, "default");
        assert_eq!(candidates[0], PathBuf::from("C:\\user\\theme\\default.theme.toml"));
        assert_eq!(candidates[1], PathBuf::from("C:\\share\\theme\\default.theme.toml"));
    }
}
