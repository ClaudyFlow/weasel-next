//! 把官方小狼毫的皮肤（`weasel.yaml`）转成我们约定的主题文件：
//! `Rime-Next\theme\default.theme.toml`。
//!
//! 用法：`cargo run --release -p weasel-style --bin theme-toml -- <输入 weasel.yaml> <输出 default.theme.toml>`
//!
//! 约定（TOML v1）：顶层 `config_version` / `color_scheme` / `color_scheme_dark`，
//! `[style]` 段放官方 style 的 25 个键，`[style.layout]` 放 19 个布局键，
//! 每套配色一个 `[schemes.<id>]` 表，颜色一律写成带引号的 `0x...` 字符串，
//! 并保留官方的 `name`（显示名）与 `author`。
//!
//! 这样做的意义：皮肤是数据不是代码，官方的 36 套配色可以原样搬过来，
//! 改皮肤不需要重新编译，librime 按路径识别后由主题在运行时读取。

use std::fmt::Write as _;
use weasel_style::{ColorScheme, WeaselConfig};

/// 官方配色里出现的全部颜色键，顺序固定，便于 diff。
const COLOR_KEYS: &[(&str, fn(&ColorScheme) -> u32)] = &[
    ("back_color", |s| s.back_color),
    ("border_color", |s| s.border_color),
    ("shadow_color", |s| s.shadow_color),
    ("text_color", |s| s.text_color),
    ("candidate_text_color", |s| s.candidate_text_color),
    ("candidate_back_color", |s| s.candidate_back_color),
    ("candidate_border_color", |s| s.candidate_border_color),
    ("candidate_shadow_color", |s| s.candidate_shadow_color),
    ("label_color", |s| s.label_color),
    ("comment_text_color", |s| s.comment_text_color),
    ("hilited_text_color", |s| s.hilited_text_color),
    ("hilited_back_color", |s| s.hilited_back_color),
    ("hilited_border_color", |s| s.hilited_border_color),
    ("hilited_shadow_color", |s| s.hilited_shadow_color),
    ("hilited_candidate_text_color", |s| s.hilited_candidate_text_color),
    ("hilited_candidate_back_color", |s| s.hilited_candidate_back_color),
    ("hilited_candidate_border_color", |s| s.hilited_candidate_border_color),
    ("hilited_candidate_shadow_color", |s| s.hilited_candidate_shadow_color),
    ("hilited_label_color", |s| s.hilited_label_color),
    ("hilited_comment_text_color", |s| s.hilited_comment_text_color),
    ("nextpage_color", |s| s.nextpage_color),
    ("prevpage_color", |s| s.prevpage_color),
];

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn write_style(out: &mut String, config: &WeaselConfig) {
    let style = &config.style;
    out.push_str("[style]\n");
    let _ = writeln!(out, "font_face = \"{}\"", escape(&style.font_face));
    let _ = writeln!(out, "label_font_face = \"{}\"", escape(&style.label_font_face));
    let _ = writeln!(out, "comment_font_face = \"{}\"", escape(&style.comment_font_face));
    let _ = writeln!(out, "font_point = {}", style.font_point);
    let _ = writeln!(out, "label_font_point = {}", style.label_font_point);
    let _ = writeln!(out, "comment_font_point = {}", style.comment_font_point);
    let _ = writeln!(out, "candidate_abbreviate_length = {}", style.candidate_abbreviate_length);
    let _ = writeln!(out, "antialias_mode = \"{}\"", escape(&style.antialias_mode));
    let _ = writeln!(out, "ascii_tip_follow_cursor = {}", style.ascii_tip_follow_cursor);
    let _ = writeln!(out, "click_to_capture = {}", style.click_to_capture);
    let _ = writeln!(out, "horizontal = {}", style.horizontal);
    let _ = writeln!(out, "fullscreen = {}", style.fullscreen);
    let _ = writeln!(out, "inline_preedit = {}", style.inline_preedit);
    let _ = writeln!(out, "preedit_type = \"{}\"", escape(&style.preedit_type));
    let _ = writeln!(out, "display_tray_icon = {}", style.display_tray_icon);
    let _ = writeln!(out, "hover_type = \"{}\"", escape(&style.hover_type));
    let _ = writeln!(out, "paging_on_scroll = {}", style.paging_on_scroll);
    let _ = writeln!(out, "vertical_auto_reverse = {}", style.vertical_auto_reverse);
    let _ = writeln!(out, "vertical_text = {}", style.vertical_text);
    let _ = writeln!(out, "vertical_text_left_to_right = {}", style.vertical_text_left_to_right);
    let _ = writeln!(out, "vertical_text_with_wrap = {}", style.vertical_text_with_wrap);
    let _ = writeln!(out, "vertical_right_to_left = {}", style.vertical_right_to_left);
    let _ = writeln!(out, "enhanced_position = {}", style.enhanced_position);
    let _ = writeln!(out, "text_orientation = \"{}\"", escape(&style.text_orientation));
    let _ = writeln!(out, "label_format = \"{}\"", escape(&style.label_format));
    let _ = writeln!(out, "mark_text = \"{}\"", escape(&style.mark_text));

    let layout = &style.layout;
    out.push_str("\n[style.layout]\n");
    let _ = writeln!(out, "align_type = \"{}\"", escape(&layout.align_type));
    let _ = writeln!(out, "min_width = {}", layout.min_width);
    let _ = writeln!(out, "max_width = {}", layout.max_width);
    let _ = writeln!(out, "min_height = {}", layout.min_height);
    let _ = writeln!(out, "max_height = {}", layout.max_height);
    let _ = writeln!(out, "border = {}", layout.border);
    let _ = writeln!(out, "margin_x = {}", layout.margin_x);
    let _ = writeln!(out, "margin_y = {}", layout.margin_y);
    let _ = writeln!(out, "spacing = {}", layout.spacing);
    let _ = writeln!(out, "candidate_spacing = {}", layout.candidate_spacing);
    let _ = writeln!(out, "hilite_spacing = {}", layout.hilite_spacing);
    let _ = writeln!(out, "hilite_padding = {}", layout.hilite_padding);
    let _ = writeln!(out, "hilite_padding_x = {}", layout.hilite_padding_x);
    let _ = writeln!(out, "hilite_padding_y = {}", layout.hilite_padding_y);
    let _ = writeln!(out, "round_corner = {}", layout.round_corner);
    let _ = writeln!(out, "corner_radius = {}", layout.corner_radius);
    let _ = writeln!(out, "hilited_corner_radius = {}", layout.hilited_corner_radius);
    let _ = writeln!(out, "shadow_radius = {}", layout.shadow_radius);
    let _ = writeln!(out, "shadow_offset_x = {}", layout.shadow_offset_x);
    let _ = writeln!(out, "shadow_offset_y = {}", layout.shadow_offset_y);
    let _ = writeln!(out, "linespacing = {}", layout.linespacing);
    let _ = writeln!(out, "baseline = {}", layout.baseline);
    let _ = writeln!(out, "layout_type_mode = {}", layout.layout_type_mode);
}

fn write_scheme(out: &mut String, id: &str, scheme: &ColorScheme) {
    let _ = write!(out, "\n[schemes.\"{}\"]\n", escape(id));
    let _ = writeln!(out, "name = \"{}\"", escape(&scheme.name));
    let _ = writeln!(out, "author = \"{}\"", escape(&scheme.author));
    if !scheme.color_format.is_empty() {
        let _ = writeln!(out, "color_format = \"{}\"", escape(&scheme.color_format));
    }
    for (key, get) in COLOR_KEYS {
        let _ = writeln!(out, "{key} = \"0x{:08x}\"", get(scheme));
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (input, output) = match (args.next(), args.next()) {
        (Some(input), Some(output)) => (input, output),
        _ => {
            eprintln!("用法: theme-toml <weasel.yaml> <default.theme.toml>");
            std::process::exit(2);
        }
    };

    let config = match weasel_style::parse_file(&input) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("解析 {input} 失败: {error}");
            std::process::exit(1);
        }
    };

    let mut out = String::new();
    out.push_str("# default.theme.toml —— 官方小狼毫的皮肤，逐字搬过来\n");
    out.push_str("#\n# 生成方式: cargo run --release -p weasel-style --bin theme-toml -- <weasel.yaml> <本文件>\n");
    out.push_str("# 内容: 官方 style 段与 layout 段全部字段 + preset_color_schemes 里每一套配色。\n");
    out.push_str("# 位置约定: <程序文件夹>\\Rime-Next\\theme\\default.theme.toml（由 librime 按路径识别）。\n\n");
    let _ = writeln!(out, "config_version = \"{}\"", escape(&config.config_version));
    let _ = writeln!(out, "color_scheme = \"{}\"", escape(&config.style.color_scheme));
    match &config.style.color_scheme_dark {
        Some(dark) => {
            let _ = writeln!(out, "color_scheme_dark = \"{}\"", escape(dark));
        }
        None => out.push_str("# color_scheme_dark = \"\"   # 官方默认没有暗色方案\n"),
    }

    write_style(&mut out, &config);
    for (id, scheme) in &config.schemes {
        write_scheme(&mut out, id, scheme);
    }

    if let Some(parent) = std::path::Path::new(&output).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(&output, &out) {
        eprintln!("写出 {output} 失败: {error}");
        std::process::exit(1);
    }
    println!(
        "已生成 {output}：{} 套配色，style {} 字段 + layout {} 字段",
        config.schemes.len(),
        out.matches("\n[").count(),
        out.lines().count()
    );
}
