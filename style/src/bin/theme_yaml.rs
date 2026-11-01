//! 把官方小狼毫的皮肤（`<程序文件夹>\Rime-Next\weasel.official.yaml`）转成我们约定的主题文件：
//! `Rime-Next\theme\default.theme.yaml`。
//!
//! 用法：`cargo run --release -p weasel-style --bin theme-yaml -- <输入 weasel.yaml> <输出 default.theme.yaml>`
//!
//! 约定（YAML，2026-10-27 起取代旧的 `.theme.toml`）：顶层 `config_version` /
//! `color_scheme`，`style:` 段放官方 style 的 25 个键，`style.layout` 放 19 个布局键，
//! 每套配色在 `schemes:` 下以其 id 为键，颜色一律写成带引号的 `0x...` 字符串，
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
    ("hilited_mark_color", |s| s.hilited_mark_color),
    ("nextpage_color", |s| s.nextpage_color),
    ("prevpage_color", |s| s.prevpage_color),
];

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 缩进两格写一行 `键: 值`（字符串一律加双引号）。
fn put_str(out: &mut String, indent: usize, key: &str, value: &str) {
    let _ = writeln!(out, "{}{key}: \"{}\"", " ".repeat(indent), escape(value));
}

fn put_num(out: &mut String, indent: usize, key: &str, value: impl std::fmt::Display) {
    let _ = writeln!(out, "{}{key}: {value}", " ".repeat(indent));
}

fn write_style(out: &mut String, config: &WeaselConfig) {
    let style = &config.style;
    out.push_str("style:\n");
    put_str(out, 2, "font_face", &style.font_face);
    put_str(out, 2, "label_font_face", &style.label_font_face);
    put_str(out, 2, "comment_font_face", &style.comment_font_face);
    put_num(out, 2, "font_point", style.font_point);
    put_num(out, 2, "label_font_point", style.label_font_point);
    put_num(out, 2, "comment_font_point", style.comment_font_point);
    put_num(out, 2, "candidate_abbreviate_length", style.candidate_abbreviate_length);
    put_str(out, 2, "antialias_mode", &style.antialias_mode);
    // 毛玻璃开关：候选窗与设置界面共用；缺省 true（这里显式写出，改配置时看得见）。
    put_num(out, 2, "glass", style.glass);
    put_num(out, 2, "ascii_tip_follow_cursor", style.ascii_tip_follow_cursor);
    put_num(out, 2, "click_to_capture", style.click_to_capture);
    put_num(out, 2, "horizontal", style.horizontal);
    put_num(out, 2, "fullscreen", style.fullscreen);
    put_num(out, 2, "inline_preedit", style.inline_preedit);
    put_str(out, 2, "preedit_type", &style.preedit_type);
    put_num(out, 2, "display_tray_icon", style.display_tray_icon);
    put_str(out, 2, "hover_type", &style.hover_type);
    put_num(out, 2, "paging_on_scroll", style.paging_on_scroll);
    put_num(out, 2, "vertical_auto_reverse", style.vertical_auto_reverse);
    put_num(out, 2, "vertical_text", style.vertical_text);
    put_num(out, 2, "vertical_text_left_to_right", style.vertical_text_left_to_right);
    put_num(out, 2, "vertical_text_with_wrap", style.vertical_text_with_wrap);
    put_num(out, 2, "vertical_right_to_left", style.vertical_right_to_left);
    put_num(out, 2, "enhanced_position", style.enhanced_position);
    put_str(out, 2, "text_orientation", &style.text_orientation);
    put_str(out, 2, "label_format", &style.label_format);
    put_str(out, 2, "mark_text", &style.mark_text);

    let layout = &style.layout;
    out.push_str("\n  layout:\n");
    // 官方皮肤里布局的**权威键**是 `style/layout/type`（字符串名，
    // `vertical` / `horizontal` / `vertical_text` / …），`style/horizontal` 只是它的旧布尔
    // 同义键。这里必须把字符串键写出来：只写 `horizontal` 会漏掉"竖排文字"与"+全屏"
    // 这两种布局（它们没有对应的布尔键），生成物一装上就与官方不一致。
    put_str(out, 4, "type", &layout.layout_type);
    put_str(out, 4, "align_type", &layout.align_type);
    put_num(out, 4, "min_width", layout.min_width);
    put_num(out, 4, "max_width", layout.max_width);
    put_num(out, 4, "min_height", layout.min_height);
    put_num(out, 4, "max_height", layout.max_height);
    put_num(out, 4, "border", layout.border);
    put_num(out, 4, "margin_x", layout.margin_x);
    put_num(out, 4, "margin_y", layout.margin_y);
    put_num(out, 4, "spacing", layout.spacing);
    put_num(out, 4, "candidate_spacing", layout.candidate_spacing);
    put_num(out, 4, "hilite_spacing", layout.hilite_spacing);
    put_num(out, 4, "hilite_padding", layout.hilite_padding);
    put_num(out, 4, "hilite_padding_x", layout.hilite_padding_x);
    put_num(out, 4, "hilite_padding_y", layout.hilite_padding_y);
    put_num(out, 4, "round_corner", layout.round_corner);
    put_num(out, 4, "corner_radius", layout.corner_radius);
    put_num(out, 4, "hilited_corner_radius", layout.hilited_corner_radius);
    put_num(out, 4, "shadow_radius", layout.shadow_radius);
    put_num(out, 4, "shadow_offset_x", layout.shadow_offset_x);
    put_num(out, 4, "shadow_offset_y", layout.shadow_offset_y);
    put_num(out, 4, "linespacing", layout.linespacing);
    put_num(out, 4, "baseline", layout.baseline);
    put_num(out, 4, "layout_type_mode", layout.layout_type_mode);
}

fn write_scheme(out: &mut String, id: &str, scheme: &ColorScheme) {
    let _ = writeln!(out, "  \"{}\":", escape(id));
    put_str(out, 4, "name", &scheme.name);
    put_str(out, 4, "author", &scheme.author);
    if !scheme.color_format.is_empty() {
        put_str(out, 4, "color_format", &scheme.color_format);
    }
    // 颜色按官方 `color_format`（默认 abgr）写回官方文本形式：**不透明写 6 位、
    // 带 alpha 写 8 位**。官方正是靠位数区分"不透明黑 0x000000"与"全透明
    // 0x00000000"，一律写 8 位会把这个区别抹掉（旧版生成器就是这么丢的 ✗）。
    let format = weasel_style::ColorFormat::from_name(&scheme.color_format);
    for (key, get) in COLOR_KEYS {
        put_str(out, 4, key, &weasel_style::color::format_text(get(scheme), format));
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (input, output, skin_only) = match (args.next(), args.next(), args.next()) {
        (Some(input), Some(output), mode) => (
            input,
            output,
            matches!(mode.as_deref(), Some("--skin")),
        ),
        _ => {
            eprintln!("用法: theme-yaml <weasel.yaml> <输出.theme.yaml> [--skin]");
            eprintln!("  --skin: 只写配色（出厂皮肤文件 default.theme.yaml 用这个；");
            eprintln!("          布局与配色选择属于用户设置，放在 %APPDATA%\\Rime-Next\\theme\\theme.yaml）");
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
    if skin_only {
        out.push_str("# default.theme.yaml —— 皮肤「default」：**只有配色**。\n");
        out.push_str("#\n");
        out.push_str(&format!(
            "# 内容由官方皮肤（{input}）生成；\n# 布局与\"用哪套皮肤\"在 theme\\theme.yaml 里。\n"
        ));
        out.push_str("# 颜色沿用官方语义：位数决定 alpha（6 位不透明、8 位带 alpha），\n");
        out.push_str("# 通道顺序按 color_format（官方默认 abgr，即文本是 AABBGGRR）。\n");
        out.push_str("# 位置约定: <程序文件夹>\\Rime-Next\\theme\\<皮肤名>\\<皮肤名>.theme.yaml\n\n");
    } else {
        out.push_str("# default.theme.yaml —— 官方小狼毫的皮肤，逐字搬过来\n");
        out.push_str("#\n# 生成方式: cargo run --release -p weasel-style --bin theme-yaml -- <weasel.yaml> <本文件>\n");
        out.push_str("# 内容: 官方 style 段与 layout 段全部字段 + 每套配色。\n");
        out.push_str("# 位置约定: <程序文件夹>\\Rime-Next\\theme\\default.theme.yaml（由 librime 按路径识别）。\n\n");
    }
    put_str(&mut out, 0, "config_version", &config.config_version);
    put_str(&mut out, 0, "color_scheme", &config.style.color_scheme);
    // 只有一套配色（用户 2026-10-18 要求：不要明暗两套、不要夜间配色）。
    // `color_scheme_dark` 不再写出；旧文件里残留的那行仍能被解析，但运行时不使用。
    out.push_str("# 只有一套配色：不做明暗两套，也不跟随系统深浅色切换（用户 2026-10-18）。\n");

    if !skin_only {
        write_style(&mut out, &config);
        out.push('\n');
    }
    out.push_str("schemes:\n");
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
        "已生成 {output}：{} 套配色，{} 行",
        config.schemes.len(),
        out.lines().count()
    );
}
