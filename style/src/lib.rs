//! 官方 Weasel 皮肤文件（`weasel.yaml`）的解析与 `UIStyle` 中间层。
//!
//! 目标：**能识别官方的皮肤文件**。字段与 fxliang/weasel 的
//! `include/WeaselIPCData.h` 的 `UIStyle`、以及 `output/data/weasel.yaml` 的实际键面一一对齐：
//!
//! * `style:`            25 个键（字体、行为、文本格式、配色选择）
//! * `style/layout:`     19 个键（对齐、尺寸、间距、圆角、阴影、行距、基线）
//! * `preset_color_schemes/<名字>:` 每套 15+ 个颜色键（另支持 `color_format`）
//! * `app_options:`      按进程覆盖（如 `cmd.exe: { ascii_mode: true }`）
//! * `show_notifications` / `show_notifications_time` / `global_ascii` / `config_version`
//!
//! 设计约束：**不引入 YAML 第三方依赖**（本仓库没有，且要能离线构建），
//! 因此这里实现一个只覆盖 `weasel.yaml` 形状的极简 YAML 子集解析器：
//! 缩进映射、标量、`#` 注释、单双引号、空行。够用且可测。

pub mod theme;

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

// ---------------------------------------------------------------- YAML 子集

/// 解析出来的 YAML 子集：标量或映射（保持键的字母序，便于比较与测试）。
#[derive(Debug, Clone, PartialEq)]
pub enum Yaml {
    Scalar(String),
    Map(BTreeMap<String, Yaml>),
}

impl Yaml {
    pub fn as_map(&self) -> Option<&BTreeMap<String, Yaml>> {
        match self {
            Yaml::Map(map) => Some(map),
            Yaml::Scalar(_) => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Yaml> {
        self.as_map().and_then(|map| map.get(key))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Yaml::Scalar(text) => Some(text.as_str()),
            Yaml::Map(_) => None,
        }
    }
}

/// 去掉行尾注释：`#` 之前必须是行首或空白，且不在引号内。
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(mark) => {
                if byte == mark {
                    quote = None;
                }
            }
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'#' if index == 0 || bytes[index - 1].is_ascii_whitespace() => {
                    return &line[..index];
                }
                _ => {}
            },
        }
        index += 1;
    }
    line
}

/// 去掉包裹的引号（`""` 表示空串）。
fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.as_bytes()[0];
        let last = trimmed.as_bytes()[trimmed.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

/// 一层缩进映射：`indent` 是**其子项**应当位于的缩进，`key` 是这一层在父层里的键名。
struct Level {
    indent: usize,
    key: String,
    map: BTreeMap<String, Yaml>,
}

/// 解析极简 YAML。返回顶层映射。
///
/// 支持：任意层级的缩进映射、`key: value` 标量、`key:` 子映射、行尾注释、空行、引号标量。
/// 不支持：列表、多行标量、锚点（`weasel.yaml` 用不到）。
pub fn parse_yaml(text: &str) -> Result<Yaml, YamlError> {
    let mut stack: Vec<Level> = vec![Level {
        indent: 0,
        key: String::new(),
        map: BTreeMap::new(),
    }];
    for (number, raw) in text.lines().enumerate() {
        let line_number = number + 1;
        let line = strip_comment(raw).trim_end();
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let content = line.trim_start();
        let Some(colon) = content.find(':') else {
            return Err(YamlError::new(line_number, "缺少 ':'"));
        };
        let key = unquote(&content[..colon]);
        let value = content[colon + 1..].trim();

        // 先收掉所有「缩进已经不比本行深」的层：它们已结束
        while stack.len() > 1 && indent < stack[stack.len() - 1].indent {
            let done = stack.pop().expect("stack len > 1");
            stack
                .last_mut()
                .expect("parent exists")
                .map
                .insert(done.key, Yaml::Map(done.map));
        }

        if value.is_empty() {
            // 子映射：压入一层，要求子项缩进比本行更深
            stack.push(Level {
                indent: indent + 1,
                key,
                map: BTreeMap::new(),
            });
        } else {
            stack
                .last_mut()
                .expect("current level exists")
                .map
                .insert(key, Yaml::Scalar(unquote(value)));
        }
    }
    // 收尾：把剩余的层逐层合回父层
    while stack.len() > 1 {
        let done = stack.pop().expect("stack len > 1");
        stack
            .last_mut()
            .expect("parent exists")
            .map
            .insert(done.key, Yaml::Map(done.map));
    }
    let root = stack.pop().expect("root exists");
    Ok(Yaml::Map(root.map))
}

/// 解析错误（带行号）。
#[derive(Debug, Clone, PartialEq)]
pub struct YamlError {
    pub line: usize,
    pub message: String,
}

impl YamlError {
    fn new(line: usize, message: &str) -> Self {
        Self { line, message: message.to_string() }
    }
}

impl fmt::Display for YamlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "第 {} 行: {}", self.line, self.message)
    }
}

impl std::error::Error for YamlError {}

// ------------------------------------------------------------------- UIStyle

/// 对齐官方 `UIStyle` 的样式面（含 `style/` 与 `style/layout/` 全部键）。
#[derive(Debug, Clone, PartialEq)]
pub struct UIStyle {
    // 字体
    pub font_face: String,
    pub label_font_face: String,
    pub comment_font_face: String,
    pub font_point: i32,
    pub label_font_point: i32,
    pub comment_font_point: i32,
    pub candidate_abbreviate_length: i32,
    // 行为
    pub antialias_mode: String,
    pub ascii_tip_follow_cursor: bool,
    pub click_to_capture: bool,
    pub horizontal: bool,
    pub fullscreen: bool,
    pub inline_preedit: bool,
    pub preedit_type: String,
    pub display_tray_icon: bool,
    pub hover_type: String,
    pub paging_on_scroll: bool,
    pub vertical_auto_reverse: bool,
    pub vertical_text: bool,
    pub vertical_text_left_to_right: bool,
    pub vertical_text_with_wrap: bool,
    /// 官方 `style/vertical_right_to_left`（竖排从右到左 ✓）
    pub vertical_right_to_left: bool,
    /// 官方 `style/enhanced_position`（增强定位 ✓）
    pub enhanced_position: bool,
    /// 官方 `style/text_orientation`（horizontal | vertical ✓ 与 vertical_text 冗余 ✓ 兼容 squirrel ✓）
    pub text_orientation: String,
    // 文本格式
    pub label_format: String,
    pub mark_text: String,
    // 配色选择
    pub color_scheme: String,
    pub color_scheme_dark: Option<String>,
    // 布局
    pub layout: Layout,
}

/// `style/layout/` 的 19 个键。
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub align_type: String,
    pub min_width: i32,
    pub max_width: i32,
    pub min_height: i32,
    pub max_height: i32,
    pub border_width: i32,
    pub margin_x: i32,
    pub margin_y: i32,
    pub spacing: i32,
    pub candidate_spacing: i32,
    pub hilite_spacing: i32,
    pub hilite_padding: i32,
    pub hilite_padding_x: i32,
    pub hilite_padding_y: i32,
    /// 官方 `style/layout/border`（与 `border_width` 同义 ✓ 两个键都认 ✓）
    pub border: i32,
    /// 官方 `style/layout/type`（0..5 六种布局 ✓，-1 = 未指定）
    pub layout_type_mode: i32,
    /// 官方 `style/layout/hilited_corner_radius`（高亮块自己的圆角 ✓ -1 = 跟随 corner_radius ✓）
    pub hilited_corner_radius: i32,
    pub round_corner: i32,
    pub corner_radius: i32,
    pub shadow_radius: i32,
    pub shadow_offset_x: i32,
    pub shadow_offset_y: i32,
    pub linespacing: i32,
    pub baseline: i32,
}

impl Default for Layout {
    /// 默认值 = 官方 `output/data/weasel.yaml` 里的值。
    fn default() -> Self {
        Self {
            align_type: "center".into(),
            min_width: 160,
            max_width: 0,
            min_height: 0,
            max_height: 0,
            border_width: 3,
            margin_x: 12,
            margin_y: 12,
            spacing: 10,
            candidate_spacing: 5,
            hilite_spacing: 4,
            hilite_padding: 2,
            hilite_padding_x: 0,
            hilite_padding_y: 0,
            border: 3,
            layout_type_mode: -1,
            hilited_corner_radius: -1,
            round_corner: 0,
            corner_radius: 0,
            shadow_radius: 0,
            shadow_offset_x: 0,
            shadow_offset_y: 0,
            linespacing: 0,
            baseline: 0,
        }
    }
}

impl Default for UIStyle {
    /// 默认值 = 官方 `weasel.yaml` 的 `style:` 段。
    fn default() -> Self {
        Self {
            font_face: "Microsoft YaHei".into(),
            label_font_face: "Microsoft YaHei".into(),
            comment_font_face: "Microsoft YaHei".into(),
            font_point: 14,
            label_font_point: 14,
            comment_font_point: 14,
            candidate_abbreviate_length: 0,
            antialias_mode: "default".into(),
            ascii_tip_follow_cursor: false,
            click_to_capture: false,
            horizontal: false,
            fullscreen: false,
            inline_preedit: false,
            preedit_type: "composition".into(),
            display_tray_icon: false,
            hover_type: "none".into(),
            paging_on_scroll: false,
            vertical_auto_reverse: false,
            vertical_text: false,
            vertical_text_left_to_right: false,
            vertical_text_with_wrap: false,
            vertical_right_to_left: false,
            enhanced_position: false,
            text_orientation: String::new(),
            label_format: "%s.".into(),
            mark_text: String::new(),
            color_scheme: "aqua".into(),
            color_scheme_dark: None,
            layout: Layout::default(),
        }
    }
}

/// 一套配色方案（官方 `preset_color_schemes/<名字>`）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColorScheme {
    pub name: String,
    pub author: String,
    /// 颜色写法：`argb` / `rgba` / 空（官方 `color_format`）。
    pub color_format: String,
    pub back_color: u32,
    pub border_color: u32,
    pub shadow_color: u32,
    pub text_color: u32,
    pub candidate_text_color: u32,
    pub candidate_back_color: u32,
    pub candidate_border_color: u32,
    pub candidate_shadow_color: u32,
    pub label_color: u32,
    pub comment_text_color: u32,
    pub hilited_text_color: u32,
    pub hilited_back_color: u32,
    pub hilited_border_color: u32,
    pub hilited_shadow_color: u32,
    pub hilited_candidate_text_color: u32,
    pub hilited_candidate_back_color: u32,
    pub hilited_candidate_border_color: u32,
    pub hilited_candidate_shadow_color: u32,
    pub hilited_label_color: u32,
    pub hilited_comment_text_color: u32,
    pub nextpage_color: u32,
    pub prevpage_color: u32,
}

/// 整份 `weasel.yaml`。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WeaselConfig {
    pub config_version: String,
    pub show_notifications: bool,
    pub show_notifications_time: i32,
    pub global_ascii: bool,
    pub style: UIStyle,
    pub schemes: BTreeMap<String, ColorScheme>,
    /// `app_options/<进程>: { ascii_mode }` —— 只保留布尔覆盖。
    pub app_options: BTreeMap<String, BTreeMap<String, bool>>,
}

/// 解析颜色：`0xRRGGBB` / `0xAARRGGBB` / 十进制 / 带引号。
pub fn parse_color(raw: &str) -> u32 {
    let text = raw.trim().trim_matches('"').trim_matches('\'');
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).unwrap_or(0);
    }
    if let Ok(value) = text.parse::<u32>() {
        return value;
    }
    // `#RRGGBB`（YAML 里少见，但兼容一下）
    if let Some(hex) = text.strip_prefix('#') {
        return u32::from_str_radix(hex, 16).unwrap_or(0);
    }
    0
}

fn text_of(node: Option<&Yaml>) -> Option<String> {
    node.and_then(Yaml::as_str).map(str::to_string)
}

fn int_of(node: Option<&Yaml>) -> Option<i32> {
    text_of(node).and_then(|text| text.trim().parse::<i32>().ok())
}

fn bool_of(node: Option<&Yaml>) -> Option<bool> {
    match text_of(node)?.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// 把解析结果映射成 `WeaselConfig`（缺省项用官方默认值补齐）。
pub fn build(yaml: &Yaml) -> WeaselConfig {
    let mut config = WeaselConfig {
        config_version: text_of(yaml.get("config_version")).unwrap_or_default(),
        show_notifications: bool_of(yaml.get("show_notifications")).unwrap_or(true),
        show_notifications_time: int_of(yaml.get("show_notifications_time")).unwrap_or(1200),
        global_ascii: bool_of(yaml.get("global_ascii")).unwrap_or(false),
        style: UIStyle::default(),
        schemes: BTreeMap::new(),
        app_options: BTreeMap::new(),
    };

    if let Some(style) = yaml.get("style") {
        let s = &mut config.style;
        if let Some(v) = text_of(style.get("font_face")) { s.font_face = v; }
        if let Some(v) = text_of(style.get("label_font_face")) { s.label_font_face = v; }
        if let Some(v) = text_of(style.get("comment_font_face")) { s.comment_font_face = v; }
        if let Some(v) = int_of(style.get("font_point")) { s.font_point = v; }
        if let Some(v) = int_of(style.get("label_font_point")) { s.label_font_point = v; }
        if let Some(v) = int_of(style.get("comment_font_point")) { s.comment_font_point = v; }
        if let Some(v) = int_of(style.get("candidate_abbreviate_length")) { s.candidate_abbreviate_length = v; }
        if let Some(v) = text_of(style.get("antialias_mode")) { s.antialias_mode = v; }
        if let Some(v) = bool_of(style.get("ascii_tip_follow_cursor")) { s.ascii_tip_follow_cursor = v; }
        if let Some(v) = bool_of(style.get("click_to_capture")) { s.click_to_capture = v; }
        if let Some(v) = bool_of(style.get("horizontal")) { s.horizontal = v; }
        if let Some(v) = bool_of(style.get("fullscreen")) { s.fullscreen = v; }
        if let Some(v) = bool_of(style.get("inline_preedit")) { s.inline_preedit = v; }
        if let Some(v) = text_of(style.get("preedit_type")) { s.preedit_type = v; }
        if let Some(v) = bool_of(style.get("display_tray_icon")) { s.display_tray_icon = v; }
        if let Some(v) = text_of(style.get("hover_type")) { s.hover_type = v; }
        if let Some(v) = bool_of(style.get("paging_on_scroll")) { s.paging_on_scroll = v; }
        if let Some(v) = bool_of(style.get("vertical_auto_reverse")) { s.vertical_auto_reverse = v; }
        if let Some(v) = bool_of(style.get("vertical_text")) { s.vertical_text = v; }
        if let Some(v) = bool_of(style.get("vertical_text_left_to_right")) { s.vertical_text_left_to_right = v; }
        if let Some(v) = bool_of(style.get("vertical_text_with_wrap")) { s.vertical_text_with_wrap = v; }
        if let Some(v) = bool_of(style.get("vertical_right_to_left")) { s.vertical_right_to_left = v; }
        if let Some(v) = bool_of(style.get("enhanced_position")) { s.enhanced_position = v; }
        if let Some(v) = text_of(style.get("text_orientation")) { s.text_orientation = v; }
        if let Some(v) = text_of(style.get("label_format")) { s.label_format = v; }
        if let Some(v) = text_of(style.get("mark_text")) { s.mark_text = v; }
        if let Some(v) = text_of(style.get("color_scheme")) { s.color_scheme = v; }
        s.color_scheme_dark = text_of(style.get("color_scheme_dark"));

        if let Some(layout) = style.get("layout") {
            let l = &mut s.layout;
            if let Some(v) = text_of(layout.get("align_type")) { l.align_type = v; }
            if let Some(v) = int_of(layout.get("min_width")) { l.min_width = v; }
            if let Some(v) = int_of(layout.get("max_width")) { l.max_width = v; }
            if let Some(v) = int_of(layout.get("min_height")) { l.min_height = v; }
            if let Some(v) = int_of(layout.get("max_height")) { l.max_height = v; }
            if let Some(v) = int_of(layout.get("border_width")) { l.border_width = v; }
            if let Some(v) = int_of(layout.get("margin_x")) { l.margin_x = v; }
            if let Some(v) = int_of(layout.get("margin_y")) { l.margin_y = v; }
            if let Some(v) = int_of(layout.get("spacing")) { l.spacing = v; }
            if let Some(v) = int_of(layout.get("candidate_spacing")) { l.candidate_spacing = v; }
            if let Some(v) = int_of(layout.get("hilite_spacing")) { l.hilite_spacing = v; }
            if let Some(v) = int_of(layout.get("hilite_padding")) { l.hilite_padding = v; }
            if let Some(v) = int_of(layout.get("hilite_padding_x")) { l.hilite_padding_x = v; }
            if let Some(v) = int_of(layout.get("hilite_padding_y")) { l.hilite_padding_y = v; }
            if let Some(v) = int_of(layout.get("border")) { l.border = v; }
            if let Some(v) = int_of(layout.get("border_width")) { l.border = v; }   // 出厂 yaml 用 border_width ✓ 两条都认 ✓
            if let Some(v) = int_of(layout.get("type")) { l.layout_type_mode = v; }
            if let Some(v) = int_of(layout.get("hilited_corner_radius")) { l.hilited_corner_radius = v; }
            if let Some(v) = int_of(layout.get("round_corner")) { l.round_corner = v; }
            if let Some(v) = int_of(layout.get("corner_radius")) { l.corner_radius = v; }
            if let Some(v) = int_of(layout.get("shadow_radius")) { l.shadow_radius = v; }
            if let Some(v) = int_of(layout.get("shadow_offset_x")) { l.shadow_offset_x = v; }
            if let Some(v) = int_of(layout.get("shadow_offset_y")) { l.shadow_offset_y = v; }
            if let Some(v) = int_of(layout.get("linespacing")) { l.linespacing = v; }
            if let Some(v) = int_of(layout.get("baseline")) { l.baseline = v; }
        }
    }

    if let Some(schemes) = yaml.get("preset_color_schemes").and_then(Yaml::as_map) {
        for (name, node) in schemes {
            let mut scheme = ColorScheme {
                name: text_of(node.get("name")).unwrap_or_else(|| name.clone()),
                author: text_of(node.get("author")).unwrap_or_default(),
                color_format: text_of(node.get("color_format")).unwrap_or_default(),
                ..ColorScheme::default()
            };
            let mut color = |key: &str, slot: &mut u32| {
                if let Some(raw) = text_of(node.get(key)) {
                    *slot = parse_color(&raw);
                }
            };
            color("back_color", &mut scheme.back_color);
            color("border_color", &mut scheme.border_color);
            color("shadow_color", &mut scheme.shadow_color);
            color("text_color", &mut scheme.text_color);
            color("candidate_text_color", &mut scheme.candidate_text_color);
            color("candidate_back_color", &mut scheme.candidate_back_color);
            color("candidate_border_color", &mut scheme.candidate_border_color);
            color("candidate_shadow_color", &mut scheme.candidate_shadow_color);
            color("label_color", &mut scheme.label_color);
            color("comment_text_color", &mut scheme.comment_text_color);
            color("hilited_text_color", &mut scheme.hilited_text_color);
            color("hilited_back_color", &mut scheme.hilited_back_color);
            color("hilited_border_color", &mut scheme.hilited_border_color);
            color("hilited_shadow_color", &mut scheme.hilited_shadow_color);
            color("hilited_candidate_text_color", &mut scheme.hilited_candidate_text_color);
            color("hilited_candidate_back_color", &mut scheme.hilited_candidate_back_color);
            color("hilited_candidate_border_color", &mut scheme.hilited_candidate_border_color);
            color("hilited_candidate_shadow_color", &mut scheme.hilited_candidate_shadow_color);
            color("hilited_label_color", &mut scheme.hilited_label_color);
            color("hilited_comment_text_color", &mut scheme.hilited_comment_text_color);
            color("nextpage_color", &mut scheme.nextpage_color);
            color("prevpage_color", &mut scheme.prevpage_color);
            config.schemes.insert(name.clone(), scheme);
        }
    }

    if let Some(apps) = yaml.get("app_options").and_then(Yaml::as_map) {
        for (app, node) in apps {
            let mut options = BTreeMap::new();
            if let Some(map) = node.as_map() {
                for (key, value) in map {
                    if let Some(flag) = bool_of(Some(value)) {
                        options.insert(key.clone(), flag);
                    }
                }
            }
            config.app_options.insert(app.clone(), options);
        }
    }

    config
}

/// 一步到位：解析文本。
pub fn parse(text: &str) -> Result<WeaselConfig, YamlError> {
    Ok(build(&parse_yaml(text)?))
}

/// 一步到位：解析文件。
pub fn parse_file(path: impl AsRef<Path>) -> Result<WeaselConfig, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    Ok(build(&parse_yaml(&text)?))
}

impl WeaselConfig {
    /// 取配色：先按名字，找不到就回落到官方默认 `aqua`，再不行给默认值。
    pub fn scheme(&self, name: &str) -> ColorScheme {
        self.schemes
            .get(name)
            .cloned()
            .or_else(|| self.schemes.get("aqua").cloned())
            .unwrap_or_default()
    }

    /// 当前生效配色（暗色模式可传 `dark = true` 走 `color_scheme_dark`）。
    pub fn active_scheme(&self, dark: bool) -> ColorScheme {
        if dark {
            if let Some(name) = &self.style.color_scheme_dark {
                return self.scheme(name);
            }
        }
        self.scheme(&self.style.color_scheme)
    }
}

// ------------------------------------------------- 官方 weasel.custom.yaml 补丁

/// 把官方 `weasel.custom.yaml` 的 `patch:` 段应用到 `weasel.yaml` 之上。
///
/// 官方（Rime）的写法是「路径 + 值」：
///
/// ```yaml
/// patch:
///   style/color_scheme: luna
///   style/font_point: 16
///   style/layout/corner_radius: 8
///   preset_color_schemes/luna/back_color: 0x101010
/// ```
///
/// 支持：任意深度的 `/` 路径、自动创建中间层。不支持（官方也少用）：`+`/`-` 追加删除、`__patch`。
pub fn apply_patch(base: &mut Yaml, patch: &Yaml) {
    let Some(entries) = patch.as_map() else {
        return;
    };
    for (path, value) in entries {
        set_path(base, path, value.clone());
    }
}

fn set_path(root: &mut Yaml, path: &str, value: Yaml) {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return;
    }
    let mut cursor = root;
    for part in &parts[..parts.len() - 1] {
        if !matches!(cursor, Yaml::Map(_)) {
            *cursor = Yaml::Map(BTreeMap::new());
        }
        let Yaml::Map(map) = cursor else { return };
        cursor = map
            .entry((*part).to_string())
            .or_insert_with(|| Yaml::Map(BTreeMap::new()));
    }
    if !matches!(cursor, Yaml::Map(_)) {
        *cursor = Yaml::Map(BTreeMap::new());
    }
    if let Yaml::Map(map) = cursor {
        map.insert(parts[parts.len() - 1].to_string(), value);
    }
}

/// 读官方两份文件：基底 `weasel.yaml` + 用户覆盖 `weasel.custom.yaml`。
pub fn parse_with_patch(base: &str, custom: Option<&str>) -> Result<WeaselConfig, YamlError> {
    let mut root = parse_yaml(base)?;
    if let Some(text) = custom {
        let user = parse_yaml(text)?;
        match user.get("patch") {
            Some(patch) => apply_patch(&mut root, patch),
            None => {
                if user.as_map().is_some_and(|map| !map.is_empty()) {
                    // 没有 patch: 段 —— 容错：当作整份补丁
                    apply_patch(&mut root, &user);
                }
            }
        }
    }
    Ok(build(&root))
}

/// 一步到位：基底文件 + 可选用户文件。
pub fn parse_files(
    base: impl AsRef<Path>,
    custom: Option<impl AsRef<Path>>,
) -> Result<WeaselConfig, Box<dyn std::error::Error>> {
    let base_text = std::fs::read_to_string(base)?;
    let custom_text = match custom {
        Some(path) => {
            let path = path.as_ref();
            if path.is_file() {
                Some(std::fs::read_to_string(path)?)
            } else {
                None
            }
        }
        None => None,
    };
    Ok(parse_with_patch(&base_text, custom_text.as_deref())?)
}

#[cfg(test)]
mod patch_tests {
    use super::*;

    const OFFICIAL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../Rime-Next/weasel.yaml");
    const BASE: &str = "style:\n  color_scheme: aqua\n  font_point: 14\n  layout:\n    corner_radius: 4\npreset_color_schemes:\n  aqua:\n    back_color: 0xeeeeee\n  luna:\n    back_color: 0x111111\n";

    #[test]
    fn patch_overrides_style_and_scheme() {
        let custom = "patch:\n  style/color_scheme: luna\n  style/font_point: 20\n  style/layout/corner_radius: 9\n  preset_color_schemes/aqua/back_color: 0xff0000\n";
        let config = parse_with_patch(BASE, Some(custom)).expect("解析");
        assert_eq!(config.style.color_scheme, "luna");
        assert_eq!(config.style.font_point, 20);
        assert_eq!(config.style.layout.corner_radius, 9);
        assert_eq!(config.schemes["aqua"].back_color, 0x00ff_0000, "用户覆盖配色应生效");
        assert_eq!(config.schemes["luna"].back_color, 0x0011_1111, "未覆盖的保持原值");
    }

    #[test]
    fn patch_creates_missing_branches() {
        let custom = "patch:\n  style/layout/hilite_padding_x: 6\n  preset_color_schemes/新配色/text_color: 0x00ff00\n";
        let config = parse_with_patch(BASE, Some(custom)).expect("解析");
        assert_eq!(config.style.layout.hilite_padding_x, 6);
        assert_eq!(config.schemes["新配色"].text_color, 0x0000_ff00);
    }

    #[test]
    fn custom_without_patch_block_is_tolerated() {
        let config = parse_with_patch(BASE, Some("style/color_scheme: luna\n")).expect("解析");
        assert_eq!(config.style.color_scheme, "luna");
    }

    #[test]
    fn no_custom_keeps_base() {
        let config = parse_with_patch(BASE, None).expect("解析");
        assert_eq!(config.style.color_scheme, "aqua");
        assert_eq!(config.style.font_point, 14);
    }

    #[test]
    fn real_official_pair() {
        let base = std::fs::read_to_string(OFFICIAL).expect("读官方 weasel.yaml");
        let custom = "patch:\n  style/color_scheme: luna\n  style/font_point: 18\n  style/hover_type: hilite\n  style/layout/shadow_radius: 6\n  preset_color_schemes/luna/back_color: 0x202020\n";
        let config = parse_with_patch(&base, Some(custom)).expect("解析");
        assert_eq!(config.style.color_scheme, "luna");
        assert_eq!(config.style.font_point, 18);
        assert_eq!(config.style.hover_type, "hilite");
        assert_eq!(config.style.layout.shadow_radius, 6);
        assert_eq!(config.schemes["luna"].back_color, 0x0020_2020);
        assert_eq!(config.schemes.len(), 36, "补丁不该减少配色数量");
        println!(
            "官方基底 + 用户补丁 → 方案 {:?} · 字号 {} · hover {:?} · 阴影 {} · luna 底色 {:#010x} · 配色 {} 套",
            config.style.color_scheme, config.style.font_point, config.style.hover_type,
            config.style.layout.shadow_radius, config.schemes["luna"].back_color, config.schemes.len()
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// 官方原版皮肤文件（仓库出厂数据，与 fxliang/weasel 的 output/data/weasel.yaml 同一份）。
    const OFFICIAL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../Rime-Next/weasel.yaml");

    fn official() -> WeaselConfig {
        parse_file(OFFICIAL).expect("官方 weasel.yaml 必须能解析")
    }

    #[test]
    fn parses_official_skin() {
        let config = official();
        println!("config_version = {:?}", config.config_version);
        println!("配色方案 {} 套: {:?}", config.schemes.len(), config.schemes.keys().collect::<Vec<_>>());
        println!("font_face = {:?}  font_point = {}", config.style.font_face, config.style.font_point);
        println!("color_scheme = {:?}  hover_type = {:?}", config.style.color_scheme, config.style.hover_type);
        println!("layout = {:?}", config.style.layout);
        let aqua = config.scheme("aqua");
        println!("aqua = back {:#010x} text {:#010x} hilited_candidate_back {:#010x}",
            aqua.back_color, aqua.text_color, aqua.hilited_candidate_back_color);

        assert_eq!(config.config_version, "0.23");
        assert!(config.show_notifications);
        assert_eq!(config.show_notifications_time, 1200);
        // 官方自带 20 套以上配色
        assert!(config.schemes.len() >= 20, "配色太少: {}", config.schemes.len());
        for name in ["aqua", "azure", "luna", "ink", "starcraft", "google", "ps4"] {
            assert!(config.schemes.contains_key(name), "缺少配色 {name}");
        }
        // style 段
        assert_eq!(config.style.font_face, "Microsoft YaHei");
        assert_eq!(config.style.font_point, 14);
        assert_eq!(config.style.color_scheme, "aqua");
        assert_eq!(config.style.label_format, "%s.");
        assert_eq!(config.style.preedit_type, "composition");
        assert_eq!(config.style.hover_type, "none"); // 行尾注释必须被剥掉
        assert_eq!(config.style.mark_text, "");
        // layout 段
        assert_eq!(config.style.layout.align_type, "center");
        assert_eq!(config.style.layout.min_width, 160);
        assert_eq!(config.style.layout.border_width, 3);
        assert_eq!(config.style.layout.margin_x, 12);
        assert_eq!(config.style.layout.hilite_padding, 2);
        assert_eq!(config.style.layout.max_width, 0); // 带 tab 注释`r`n        assert_eq!(config.style.layout.border, 3, "border_width 应同时填进官方 border 键");`r`n        assert_eq!(config.style.layout.hilited_corner_radius, -1, "官方未填则保持 -1（跟随 corner_radius）");
        // app_options
        assert_eq!(
            config.app_options.get("cmd.exe").and_then(|m| m.get("ascii_mode")).copied(),
            Some(true)
        );
        // 每套配色都应至少解析出一个非零颜色
        for (name, scheme) in &config.schemes {
            let any = scheme.back_color | scheme.text_color | scheme.candidate_text_color
                | scheme.hilited_candidate_back_color;
            assert!(any != 0, "配色 {name} 一个颜色都没解析出来");
        }
    }

    #[test]
    fn dark_scheme_falls_back_to_light() {
        let config = official();
        assert!(config.style.color_scheme_dark.is_none(), "官方默认没有暗色方案");
        let active = config.active_scheme(true);
        // 用颜色值比较：官方配色里的 `name:` 是**显示名**（如「碧水」），不是键名
        assert_eq!(active.back_color, config.schemes["aqua"].back_color);
    }

    #[test]
    fn unknown_scheme_falls_back_to_aqua() {
        let config = official();
        let fallback = config.scheme("__不存在__");
        assert_eq!(fallback.back_color, config.schemes["aqua"].back_color);
    }

    #[test]
    fn scheme_display_names_are_kept() {
        let config = official();
        let aqua = config.scheme("aqua");
        println!("aqua 显示名 = {:?} · 作者 = {:?}", aqua.name, aqua.author);
        let mut names: Vec<String> = config
            .schemes
            .values()
            .map(|scheme| format!("{}={}", scheme.name, scheme.author))
            .collect();
        names.sort();
        println!("全部 {} 套显示名: {}", names.len(), names.join(" · "));
        assert!(!aqua.name.is_empty(), "显示名不该为空");
    }

    #[test]
    fn inline_text_parses_nested_blocks() {
        let text = "style:\n  color_scheme: luna\n  layout:\n    min_width: 42\n    shadow_radius: 7\npreset_color_schemes:\n  x:\n    back_color: 0x112233\n";
        let config = parse(text).expect("内联解析");
        assert_eq!(config.style.color_scheme, "luna");
        assert_eq!(config.style.layout.min_width, 42);
        assert_eq!(config.style.layout.shadow_radius, 7);
        assert_eq!(config.schemes["x"].back_color, 0x0011_2233);
    }

    #[test]
    fn color_forms() {
        assert_eq!(parse_color("0xff0000"), 0x00ff_0000);
        assert_eq!(parse_color("0xFF0000"), 0x00ff_0000);
        assert_eq!(parse_color("16711680"), 0x00ff_0000);
        assert_eq!(parse_color("\"#ff0000\""), 0x00ff_0000);
        assert_eq!(parse_color("不是颜色"), 0);
    }
}
