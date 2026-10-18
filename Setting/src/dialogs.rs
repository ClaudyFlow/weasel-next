//! 官方设置界面（Win32 对话框）的 Rust 实现。
//!
//! 界面本身来自 `ui/deployer.rc` 里的 DIALOGEX 模板——与官方 `WeaselDeployer.rc`
//! 同一套控件、同一套坐标。本模块只负责把数据填进去、把用户的操作写回配置：
//!
//! * `WM_INITDIALOG`：把 `preset_color_schemes` 的显示名填进下拉框，把当前配色的
//!   颜色键填进左侧列表，并按字段类型决定显示下拉框还是编辑框（官方也是二选一）。
//! * `WM_COMMAND`：`IDC_COLOR_SCHEME` 的下拉框换项 → 立刻写
//!   `patch: style/color_scheme: <id>`（等价于官方 `customize_string`）。
//! * `WM_MEASUREITEM` / `WM_DRAWITEM`：列表是 `LBS_OWNERDRAWFIXED`，自绘每一行：
//!   左边一个小色块，右边是颜色键名与当前值。
//!
//! 入口与官方一致：`Weasel-Next-Setting.exe /style` 打开界面风格设定，
//! `/switcher` 与 `/dict` 分别对应方案选单设定与用户词典管理。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use weasel_style::settings as style_settings;

/// 出厂皮肤名（与渲染器 `theme/official/src/config.rs::SKIN` 一致）：
/// 配色在 `Rime-Next\theme\default\default.theme.toml`，共 36 套。
const SKIN: &str = "default";

use weasel_common::bindings::*;
use windows_core::PCWSTR;

/// 对话框 ID，与 `ui/deployer.rc` 一致。
// Win32 宏常量：这些不是 API 也不是元数据里的项（都是 #define），
// 值在 Windows 头文件里固定不变，这里直接定义，省去向绑定生成器要它们。
//
// 数值来源：winuser.h / winnt.h / wingdi.h
const DWLP_USER: i32 = 16;
const IDOK: i32 = 1;
const IDCANCEL: i32 = 2;                 // DWLP_MSGRESULT(8) + sizeof(LRESULT)(8)
const WM_INITDIALOG_: u32 = 0x0110;
const WM_COMMAND_: u32 = 0x0111;
const WM_CLOSE_: u32 = 0x0010;
const WM_MEASUREITEM_: u32 = 0x002C;
const WM_DRAWITEM_: u32 = 0x002B;
const CB_ADDSTRING: u32 = 0x0143;
const LB_GETCURSEL: u32 = 0x0188;
const CB_RESETCONTENT: u32 = 0x014B;
const LBN_SELCHANGE_: u32 = 1;
const CBN_SELCHANGE_: u32 = 1;
const CB_SETCURSEL: u32 = 0x014E;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SHOWDROPDOWN: u32 = 0x014F;
const CBN_SELCHANGE: u32 = 1;
const LB_ADDSTRING: u32 = 0x0180;
const LB_SETCURSEL: u32 = 0x0186;
const LB_RESETCONTENT: u32 = 0x0184;
const ODS_SELECTED: u32 = 0x0001;
const SW_HIDE: i32 = 0;
const COLOR_WINDOW: i32 = 5;
const COLOR_WINDOWTEXT: i32 = 8;
const COLOR_HIGHLIGHT: i32 = 13;
const COLOR_HIGHLIGHTTEXT: i32 = 14;
const TRANSPARENT_: i32 = 1;
const DT_RIGHT: u32 = 0x00000002;
const DT_LEFT: u32 = 0x0000;
const DT_VCENTER: u32 = 0x0004;
const DT_SINGLELINE: u32 = 0x0020;
const MB_OK: u32 = 0x0000;
const MB_ICONWARNING: u32 = 0x0030;
const IDD_STYLE_SETTING: u16 = 130;
const IDD_SWITCHER_SETTING: u16 = 129;
const IDD_DICT_MANAGEMENT: u16 = 132;

const IDC_COLOR_SCHEME: i32 = 1004;
const IDC_COLOR_LIST: i32 = 1002;
const IDC_COLOR_VALUE: i32 = 1003;
const IDC_SET_COLOR: i32 = 1004;
const IDC_SELECT_FONT: i32 = 1005;
const IDC_STYLE_LIST: i32 = 1006;
const IDC_STYLE_VALUE_COMBO: i32 = 1007;
const IDC_STYLE_VALUE_EDIT: i32 = 1008;
const IDC_SET_STYLE: i32 = 1009;

/// 用户补丁里这条样式键的当前值。
///
/// `weasel.custom.toml` 用的是**斜杠键**（`style/color_scheme: aqua`），
/// 与 `weasel.toml` 的嵌套写法不同，所以这里按单层 map 直接查 —— 官方在
/// `UIStyleSettingsDialog` 里也是这么取的（`settings_->GetStyleValue(info, &current)`）。
/// 文件里没写、但官方有内置默认值的样式键。
///
/// 官方 `UIStyleSettingsDialog` 读的是**内存里的设置对象**（`settings_->GetStyleValue`），
/// 那个对象带着内置默认值，所以文件里没写的键它照样有值；我们读文件，
/// 出厂 `weasel.toml` 没写这几项就会空着 —— 这里按官方默认补齐。
fn builtin_style_default(key: &str) -> Option<&'static str> {
    match key {
        "style/enhanced_position" => Some("false"),
        "style/vertical_text_left_to_right" => Some("false"),
        "style/vertical_right_to_left" => Some("false"),
        // lib.rs 里 Layout::default 的 border_width 是 3（与 border 同义）。
        "style/layout/border" => Some("3"),
        // lib.rs 的 Layout::default：hilite_padding_x/y = 0、hilited_corner_radius = -1（-1 表示跟随 corner_radius）。
        "style/layout/hilite_padding_x" => Some("0"),
        "style/layout/hilite_padding_y" => Some("0"),
        "style/layout/hilited_corner_radius" => Some("-1"),
        _ => None,
    }
}

/// 当前配色：用户目录 `theme/theme.toml` 的 `color_scheme`。
///
/// 皮肤选择现在的落点就是这一行（设置界面写它、主题 DLL 读它）。对话框如果继续从
/// 旧的 `weasel.toml` / `weasel.custom.toml` 取值，就会一直显示上一次之前的那套。
pub(crate) fn theme_color_scheme(user: &std::path::Path) -> Option<String> {
    let path = user.join("theme").join("theme.toml");
    let text = std::fs::read_to_string(path).ok()?;
    let table: toml::Value = toml::from_str(&text).ok()?;
    table
        .get("color_scheme")
        .and_then(toml::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// 渲染器**真正实现**的布局类型。
///
/// 官方 `style/layout/type` 有 6 种，但我们只移植了竖直/水平两种：
/// * 文字竖排（`vertical_text`）要 `VHorizontalLayout.cpp`，**未移植**，选回落成竖直；
/// * 全屏（`*+fullscreen`）要 `FullScreenLayout.cpp`（字体自适应 + 居中），**未移植**。
/// 见 `doc/renderer-official.md` 第五节「已知偏差」第 1、2 条。
///
/// 界面上把没实现的标出来，否则用户（和发出去测试的人）会以为"选了没反应是 bug"。
const IMPLEMENTED_LAYOUTS: &[&str] = &["vertical", "horizontal"];

/// 下拉里显示的文字：未实现的布局类型加后缀标注。
fn choice_label(key: &crate::style_keys::StyleKey, choice: &crate::style_keys::StyleChoice) -> String {
    if key.key == "style/layout/type" && !IMPLEMENTED_LAYOUTS.contains(&choice.value) {
        format!("{}（暂未支持）", choice.zh_hans)
    } else {
        choice.zh_hans.to_string()
    }
}

///
/// 官方 `weasel.toml` **不写** `layout/type` 这个键（`style/src/theme.rs` 的测试也钉着
/// `layout_type.is_empty()`），而是用 `horizontal` / `vertical_text` / `fullscreen`
/// 三个布尔推出来。界面若直接读那个空字段，"布局方向"就永远显示空白 ——
/// 而实际生效的明明是有方向的。规则与 `theme/official/src/config.rs::layout_type` 一致。
fn layout_type_name(style: &weasel_style::UIStyle) -> String {
    // 字符串名优先（用户显式写过就以它为准）
    match style.layout.layout_type.trim() {
        "" => {}
        named @ ("vertical" | "horizontal" | "vertical_text" | "vertical+fullscreen"
        | "horizontal+fullscreen" | "vertical_text+fullscreen") => return named.to_owned(),
        _ => {}
    }
    let vertical_text =
        style.vertical_text || style.text_orientation.eq_ignore_ascii_case("vertical");
    if vertical_text {
        if style.fullscreen { "vertical_text+fullscreen" } else { "vertical_text" }
    } else if style.horizontal {
        if style.fullscreen { "horizontal+fullscreen" } else { "horizontal" }
    } else if style.fullscreen {
        "vertical+fullscreen"
    } else {
        "vertical"
    }
    .to_owned()
}

/// 从**主题链**里取一条样式键的当前值（TOML）。
///
/// 主题链 = `theme.toml`（布局 + `color_scheme`）叠加 `theme\<皮肤>\<皮肤>.theme.toml`（配色），
/// 与渲染器读的是同一份；`key` 是官方路径式键（`style/layout/corner_radius`）。
fn theme_style_value_from(config: &weasel_style::WeaselConfig, key: &str) -> Option<String> {
    // 只映射右边那栏实际会显示的那些键；其余靠 `builtin_style_default` 兜底。
    match key {
        "style/color_scheme" => Some(config.style.color_scheme.clone()).filter(|v| !v.is_empty()),
        "style/color_scheme_dark" => {
            config.style.color_scheme_dark.clone().filter(|v| !v.is_empty())
        }
        "style/font_point" => Some(config.style.font_point.to_string()),
        "style/label_font_point" => Some(config.style.label_font_point.to_string()),
        "style/comment_font_point" => Some(config.style.comment_font_point.to_string()),
        "style/font_face" => Some(config.style.font_face.clone()).filter(|v| !v.is_empty()),
        "style/label_font_face" => Some(config.style.label_font_face.clone()).filter(|v| !v.is_empty()),
        "style/comment_font_face" => {
            Some(config.style.comment_font_face.clone()).filter(|v| !v.is_empty())
        }
        "style/layout/type" => Some(layout_type_name(&config.style)),
        "style/layout/align_type" => Some(config.style.layout.align_type.clone()).filter(|v| !v.is_empty()),
        "style/preedit_type" => Some(config.style.preedit_type.clone()).filter(|v| !v.is_empty()),
        "style/antialias_mode" => Some(config.style.antialias_mode.clone()).filter(|v| !v.is_empty()),
        "style/hover_type" => Some(config.style.hover_type.clone()).filter(|v| !v.is_empty()),
        "style/inline_preedit" => Some(config.style.inline_preedit.to_string()),
        "style/display_tray_icon" => Some(config.style.display_tray_icon.to_string()),
        "style/ascii_tip_follow_cursor" => Some(config.style.ascii_tip_follow_cursor.to_string()),
        "style/paging_on_scroll" => Some(config.style.paging_on_scroll.to_string()),
        "style/click_to_capture" => Some(config.style.click_to_capture.to_string()),
        "style/enhanced_position" => Some(config.style.enhanced_position.to_string()),
        "style/vertical_auto_reverse" => Some(config.style.vertical_auto_reverse.to_string()),
        "style/vertical_text_left_to_right" => {
            Some(config.style.vertical_text_left_to_right.to_string())
        }
        "style/vertical_text_with_wrap" => Some(config.style.vertical_text_with_wrap.to_string()),
        "style/vertical_right_to_left" => Some(config.style.vertical_right_to_left.to_string()),
        "style/candidate_abbreviate_length" => {
            Some(config.style.candidate_abbreviate_length.to_string())
        }
        "style/label_format" => Some(config.style.label_format.clone()).filter(|v| !v.is_empty()),
        "style/mark_text" => Some(config.style.mark_text.clone()),
        "style/layout/border" => Some(config.style.layout.border.to_string()),
        "style/layout/corner_radius" => Some(config.style.layout.corner_radius.to_string()),
        "style/layout/round_corner" => Some(config.style.layout.round_corner.to_string()),
        "style/layout/hilited_corner_radius" => {
            Some(config.style.layout.hilited_corner_radius.to_string())
        }
        "style/layout/margin_x" => Some(config.style.layout.margin_x.to_string()),
        "style/layout/margin_y" => Some(config.style.layout.margin_y.to_string()),
        "style/layout/spacing" => Some(config.style.layout.spacing.to_string()),
        "style/layout/candidate_spacing" => Some(config.style.layout.candidate_spacing.to_string()),
        "style/layout/hilite_spacing" => Some(config.style.layout.hilite_spacing.to_string()),
        "style/layout/hilite_padding_x" => Some(config.style.layout.hilite_padding_x.to_string()),
        "style/layout/hilite_padding_y" => Some(config.style.layout.hilite_padding_y.to_string()),
        "style/layout/shadow_radius" => Some(config.style.layout.shadow_radius.to_string()),
        "style/layout/shadow_offset_x" => Some(config.style.layout.shadow_offset_x.to_string()),
        "style/layout/shadow_offset_y" => Some(config.style.layout.shadow_offset_y.to_string()),
        _ => None,
    }
}

/// 取一条样式键的当前值：从**已经算好的**结果里查，绝不在绘制期读盘。
///
/// ⚠️ 这里曾经在每次调用时 `theme::load_theme()` —— 而它的调用点在 `WM_DRAWITEM`
/// 里（每条可见列表项一次）⇒ **每画一行就把 `theme.toml` 与 32 KB 的
/// `default.theme.toml`（36 套配色）重读重解析一遍**，滚动/选中时肉眼可见地卡。
/// 值只在打开对话框与切皮肤时刷新，所以由 `StyleDialog::refresh_style_values()` 预算好。
/// 把官方路径式键写进用户 `theme.toml`（`style/...` → `[style]`，`style/layout/...` → `[style.layout]`）。
///
/// 用**行编辑**而不是 `toml::Value` 整体重写：这份文件是给人看的，重新序列化会把
/// 注释和键顺序都洗掉（`set_theme_key` 当初也就是为这个才做行编辑）。
///
/// `value_text` 由调用方按类型给好（布尔给 `true`/`false`，数字给裸值，字符串给带引号的值）。
fn write_theme_style(path: &std::path::Path, key_path: &str, value_text: &str) -> std::io::Result<()> {
    // ⚠️⚠️ `style/layout/type` **不能**写成文件里的 `layout_type = "vertical"`。
    //
    // 渲染器读的是 `horizontal` / `vertical_text` / `fullscreen` 三个布尔
    // （`theme/official/src/config.rs` 的 `layout_type()` 从这里推导），
    // 它**从不读** `layout_type` 这个键。原来照路径直写 `layout_type`，
    // 于是"选竖直"看起来写成功了、右侧值也变了，但**候选窗方向一点没变**
    // —— 用户实测就是这个现象。
    // 正确做法：把枚举值展开成那三个布尔。
    if key_path == "style/layout/type" {
        let choice = value_text.trim_matches('"');
        let (horizontal, vertical_text, fullscreen) = match choice {
            "vertical" => ("false", "false", "false"),
            "horizontal" => ("true", "false", "false"),
            "vertical_text" => ("false", "true", "false"),
            "vertical+fullscreen" => ("false", "false", "true"),
            "horizontal+fullscreen" => ("true", "false", "true"),
            "vertical_text+fullscreen" => ("false", "true", "true"),
            _ => ("true", "false", "false"),
        };
        // 这三个键属于 [style] 段（theme.rs::StyleSection），不是 [style.layout]。
        // 带 layout/ 前缀会被写进 [style.layout]，变成"看起来写了、渲染器读不到"的废键。
        write_theme_style(path, "style/horizontal", horizontal)?;
        write_theme_style(path, "style/vertical_text", vertical_text)?;
        write_theme_style(path, "style/fullscreen", fullscreen)?;
        return Ok(());
    }
    let (section, key) = match key_path.strip_prefix("style/layout/") {
        Some(rest) => ("style.layout", rest),
        None => ("style", key_path.strip_prefix("style/").unwrap_or(key_path)),
    };
    // ⚠️ 官方路径键与 TOML 字段名有一处不同名：
    //   `style/layout/type`（官方 UIStyleSettings 的键）在文件里叫 `layout_type`
    //   （`style/src/theme.rs` 的 LayoutSection 字段名）。
    //   照路径直写成 `type = ...` 的话，`theme.rs` 读不到 ⇒ 改了等于没改。
    let key = if key == "type" { "layout_type" } else { key };
    let header = format!("[{section}]");
    let mut lines: Vec<String> = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let start = lines.iter().position(|line| line.trim() == header);
    match start {
        Some(start) => {
            // 段范围 = [start+1, 下一个段头之前)
            let end = lines
                .iter()
                .skip(start + 1)
                .position(|line| line.trim_start().starts_with('['))
                .map_or(lines.len(), |offset| start + 1 + offset);
            let found = (start + 1..end).find(|&i| {
                lines[i]
                    .split_once('=')
                    .is_some_and(|(name, _)| name.trim() == key)
            });
            match found {
                Some(i) => lines[i] = format!("{key} = {value_text}"),
                None => lines.insert(end, format!("{key} = {value_text}")),
            }
        }
        None => {
            lines.push(String::new());
            lines.push(header);
            lines.push(format!("{key} = {value_text}"));
        }
    }
    let mut text = lines.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

/// `style_value` 的写入端：把一条样式键的新值落到用户 `theme.toml`。
fn save_style_value(
    user: &Path,
    key: &crate::style_keys::StyleKey,
    choice: usize,
    schemes: &[(String, String)],
) -> Result<String, String> {
    use crate::style_keys::StyleValueType;
    let (value_text, shown) = match key.ty {
        // 布尔键的下拉是「开 / 关」两项
        StyleValueType::Boolean => {
            let on = choice == 0;
            (
                (if on { "true" } else { "false" }).to_string(),
                (if on { "开" } else { "关" }).to_string(),
            )
        }
        // ⚠️ 配色方案类（`style/color_scheme`、`style/color_scheme_dark`）的
        // `key.choices` 是**空数组** —— 候选来自运行时读到的皮肤列表。
        // 原来落进下面的 `_` 分支，`choices.get(choice)` 恒为 None，
        // 于是"换个配色"永远返回「无效的取值」，什么都不会写。
        // 这里改成从 schemes（id, 显示名）取。
        StyleValueType::Theme => {
            let Some((id, name)) = schemes.get(choice) else {
                return Err("无效的配色".into());
            };
            (format!("\"{id}\""), name.clone())
        }
        StyleValueType::Int => {
            let Some(choice) = key.choices.get(choice) else {
                return Err("无效的取值".into());
            };
            (choice.value.to_string(), choice.zh_hans.to_string())
        }
        _ => {
            let Some(choice) = key.choices.get(choice) else {
                return Err("无效的取值".into());
            };
            (format!("\"{}\"", choice.value), choice.zh_hans.to_string())
        }
    };
    // `style/color_scheme` 是**顶层键**（不在 [style] 段里），走已有的顶层行编辑。
    if key.key == "style/color_scheme" || key.key == "style/color_scheme_dark" {
        // 这两个都是**顶层键**（不在 [style] 段里），走已有的顶层行编辑。
        let file_key = if key.key == "style/color_scheme" {
            "color_scheme"
        } else {
            "color_scheme_dark"
        };
        let path = user.join("theme").join("theme.toml");
        weasel_style::settings::set_theme_key(&path, file_key, &value_text.trim_matches('"'))
            .map_err(|error| format!("写入失败：{error}"))?;
    } else {
        let path = user.join("theme").join("theme.toml");
        write_theme_style(&path, key.key, &value_text)
            .map_err(|error| format!("写入失败：{error}"))?;
    }
    Ok(shown)
}

fn style_value(state: &StyleDialog, key: &str) -> Option<String> {
    let index = crate::style_keys::KEYS.iter().position(|k| k.key == key)?;
    state
        .style_values
        .get(index)
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| builtin_style_default(key).map(|value| value.to_string()))
}


/// 颜色键清单与官方 `WeaselStyleKeys` 的颜色部分一一对应。
const COLOR_KEYS: &[&str] = &[
    "back_color",
    "border_color",
    "shadow_color",
    "text_color",
    "candidate_text_color",
    "candidate_back_color",
    "candidate_border_color",
    "candidate_shadow_color",
    "label_color",
    "comment_text_color",
    "hilited_text_color",
    "hilited_back_color",
    "hilited_border_color",
    "hilited_shadow_color",
    "hilited_candidate_text_color",
    "hilited_candidate_back_color",
    "hilited_candidate_border_color",
    "hilited_candidate_shadow_color",
    "hilited_label_color",
    "hilited_comment_text_color",
    "nextpage_color",
    "prevpage_color",
];

/// 对话框运行期的状态；只在对话框所在线程内访问。
struct StyleDialog {
    shared: PathBuf,
    user: PathBuf,
    /// 出厂 + 用户补丁合并后的配置。
    config: Option<weasel_style::WeaselConfig>,
    /// 配色（id、显示名）列表，顺序与下拉框一致。
    schemes: Vec<(String, String)>,
    /// 当前选中的配色下标。
    selected: usize,
    /// 左侧列表当前显示的颜色键与取值。
    colors: Vec<(String, u32)>,
    /// 右侧列表每条样式键的当前值，**下标与 `style_keys::KEYS` 对齐**。
    ///
    /// 必须在这里缓存：绘制期（WM_DRAWITEM）绝不能读盘 —— 每条可见行一次会把
    /// 32 KB 的皮肤文件重复解析几十遍（见 `style_value` 的注释）。
    style_values: Vec<String>,
    /// 右侧列表**当前选中的样式键下标**（与 `style_keys::KEYS` 对齐）。
    ///
    /// ⚠️ 为什么必须记下来：用户改值时，焦点已经移到下面的下拉框，
    /// 此时 `LB_GETCURSEL` 会返回 **-1**（ListBox 没了焦点就不再报告选中项）。
    /// 原来的写回逻辑用 `LB_GETCURSEL` 取"当前是哪条键"，于是永远拿到 -1、
    /// 整段写回被 `if sel >= 0 && key_index >= 0` 跳过 ——
    /// 表现就是"选了下拉里的新值，右侧那一栏的值却不变"（用户实测）。
    selected_style_key: usize,
}

impl StyleDialog {
    /// 从磁盘读配置并算出界面要显示的内容。
    fn reload(&mut self) {
        // 配色来自**主题链**（TOML），与渲染器同源：
        //   theme\theme.toml（用户布局 + color_scheme）叠加 theme\<皮肤>\<皮肤>.theme.toml（配色）
        //
        // ⚠️ 这里原来读的是 `Rime-Next\weasel.toml`（YAML 时代的出厂皮肤）——那个文件在
        // TOML 化改造后**已不在该位置**，于是 `config` 恒为 `None`：
        // 配色下拉框全空（`CB_GETCOUNT=0`）、左侧颜色列表也空，用户根本没法换配色。
        // 症状与"数据坏了"一模一样，其实只是找错文件。改走 `theme::load_theme`，
        // 与 `skins.rs` 共用同一条读取路径，设置界面看到的配色就是渲染器会画的配色。
        self.config = weasel_style::theme::load_theme(&self.shared, &self.user, SKIN)
            .ok()
            .flatten();
        self.schemes = self
            .config
            .as_ref()
            .map(|config| {
                style_settings::list_schemes(config, None)
                    .into_iter()
                    .map(|info| (info.id, info.name))
                    .collect()
            })
            .unwrap_or_default();
        // 当前配色优先取用户 theme.toml（那才是皮肤选择的落点），取不到再退回 YAML
        let active = theme_color_scheme(&self.user).unwrap_or_else(|| {
            self.config
                .as_ref()
                .map(|config| config.style.color_scheme.clone())
                .unwrap_or_default()
        });
        self.selected = self
            .schemes
            .iter()
            .position(|(id, _)| *id == active)
            .unwrap_or(0);
        self.refresh_style_values();
        self.collect_colors();
    }

    /// 把右边那栏 39 条样式键的当前值算一遍（只在打开 / 切皮肤时调用）。
    fn refresh_style_values(&mut self) {
        self.style_values = match &self.config {
            Some(config) => crate::style_keys::KEYS
                .iter()
                .map(|key| {
                    theme_style_value_from(config, key.key)
                        .or_else(|| builtin_style_default(key.key).map(str::to_string))
                        .unwrap_or_default()
                })
                .collect(),
            None => Vec::new(),
        };
    }

    /// 取出当前配色里每个颜色键的取值（缺项按 0 显示）。
    fn collect_colors(&mut self) {
        self.colors.clear();
        let Some(config) = &self.config else { return };
        let Some((id, _)) = self.schemes.get(self.selected) else {
            return;
        };
        let Some(scheme) = config.schemes.get(id) else {
            return;
        };
        for key in COLOR_KEYS {
            let value = match *key {
                "back_color" => scheme.back_color,
                "border_color" => scheme.border_color,
                "shadow_color" => scheme.shadow_color,
                "text_color" => scheme.text_color,
                "candidate_text_color" => scheme.candidate_text_color,
                "candidate_back_color" => scheme.candidate_back_color,
                "candidate_border_color" => scheme.candidate_border_color,
                "candidate_shadow_color" => scheme.candidate_shadow_color,
                "label_color" => scheme.label_color,
                "comment_text_color" => scheme.comment_text_color,
                "hilited_text_color" => scheme.hilited_text_color,
                "hilited_back_color" => scheme.hilited_back_color,
                "hilited_border_color" => scheme.hilited_border_color,
                "hilited_shadow_color" => scheme.hilited_shadow_color,
                "hilited_candidate_text_color" => scheme.hilited_candidate_text_color,
                "hilited_candidate_back_color" => scheme.hilited_candidate_back_color,
                "hilited_candidate_border_color" => scheme.hilited_candidate_border_color,
                "hilited_candidate_shadow_color" => scheme.hilited_candidate_shadow_color,
                "hilited_label_color" => scheme.hilited_label_color,
                "hilited_comment_text_color" => scheme.hilited_comment_text_color,
                "nextpage_color" => scheme.nextpage_color,
                "prevpage_color" => scheme.prevpage_color,
                _ => 0,
            };
            self.colors.push(((*key).to_string(), value));
        }
    }
}

/// 打开「界面风格设定」对话框。`/style` 走这里。
pub fn show_style(shared: &Path, user: &Path) -> Result<(), String> {
    let mut state = StyleDialog {
        shared: shared.to_path_buf(),
        user: user.to_path_buf(),
        config: None,
        schemes: Vec::new(),
        selected: 0,
        colors: Vec::new(),
        style_values: Vec::new(),
        selected_style_key: 0,
    };
    state.reload();
    let state = Rc::new(RefCell::new(state));
    let pointer = Rc::into_raw(state.clone());
    let instance = unsafe { GetModuleHandleW(None) };
    let result = unsafe {
        DialogBoxParamW(
            Some(HINSTANCE(instance.0)),
            PCWSTR(IDD_STYLE_SETTING as usize as *const u16),
            None,
            Some(style_proc),
            LPARAM(pointer as isize),
        )
    };
    // 无论正常关闭还是出错，都要把引用计数还回去。
    unsafe { drop(Rc::from_raw(pointer)) };
    if result == -1 {
        Err("对话框创建失败".into())
    } else {
        Ok(())
    }
}

/// 取回对话框状态（`DWLP_USER` 里存的是 `Rc<RefCell<StyleDialog>>` 的裸指针）。
unsafe fn state_of(hwnd: HWND) -> Option<Rc<RefCell<StyleDialog>>> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, DWLP_USER as i32) } as *const RefCell<StyleDialog>;
    if raw.is_null() {
        return None;
    }
    // 借用一份 Rc 但**不**接管所有权：指针由调用方在对话框结束时回收。
    let rc = unsafe { Rc::from_raw(raw) };
    let clone = rc.clone();
    std::mem::forget(rc);
    Some(clone)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 界面风格设定的对话框过程。
unsafe extern "system" fn style_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match message {
        WM_INITDIALOG_ => {
            unsafe { SetWindowLongPtrW(hwnd, DWLP_USER, lparam.0) };
            let Some(state) = (unsafe { state_of(hwnd) }) else {
                return 0;
            };
            let state = state.borrow();
            // 下拉框：配色显示名
            for (_, name) in &state.schemes {
                let text = wide(name);
                unsafe { SendDlgItemMessageW(hwnd, IDC_COLOR_SCHEME, CB_ADDSTRING, WPARAM(0), LPARAM(text.as_ptr() as isize)) };
            }
            unsafe {
                SendDlgItemMessageW(hwnd, IDC_COLOR_SCHEME, CB_SETCURSEL, WPARAM(state.selected), LPARAM(0));
                // 左侧颜色列表：先清空再逐条加入（自绘，文字内容仅用于行数）
                // 右侧「样式设置」列表：照官方 include/WeaselStyleKeys.h 的 GetStyleKeys() 逐条填。
                // 官方是自绘（OnMeasureItem/OnDrawItem，左标签右「key: 值」、改动加 +），
                // 这里先把「标签 + 键路径」填进去，保证列表有内容、可见可选中。
                unsafe { SendDlgItemMessageW(hwnd, IDC_STYLE_LIST, LB_RESETCONTENT, WPARAM(0), LPARAM(0)) };
                // 先取一次路径，避免在循环里反复借用 RefCell。
                let (shared_dir, user_dir) = unsafe { state_of(hwnd) }
                    .map(|state| {
                        let state = state.borrow();
                        (state.shared.clone(), state.user.clone())
                    })
                    .unwrap_or_default();
                for key in crate::style_keys::KEYS {
                    // 官方每条显示「标签 + key: 当前值」；值取自预读好的缓存
                    // （绘制期不读盘，见 `style_values` 字段）。
                    let value = unsafe { state_of(hwnd) }
                        .map(|state| { let state = state.borrow(); style_value(&state, key.key) })
                        .flatten();
                    let text = match value {
                        Some(value) => wide(&format!("{}   {}: {}", key.zh_hans, key.key, value)),
                        None => wide(&format!("{}   {}", key.zh_hans, key.key)),
                    };
                    unsafe {
                        SendDlgItemMessageW(
                            hwnd,
                            IDC_STYLE_LIST,
                            LB_ADDSTRING,
                            WPARAM(0),
                            LPARAM(text.as_ptr() as isize),
                        )
                    };
                }

                // 默认选中第一条样式键 —— 否则下方那个框是空的；再投一条选中变化通知，
                // 让已有的处理逻辑去填候选并把当前值预选上（官方同样默认选中第一条）。
                //
                // ⚠️ 这里必须用 **SendMessageW 同步**，不能用 PostMessageW：
                // PostMessage 是异步的，消息可能在 WM_INITDIALOG 期间被对话框的消息过滤吞掉，
                // 结果就是"右侧列表第一行高亮着、但下面那个下拉框永远空白"（用户实测截图确认）。
                // 同步调用会重入一次 dialog proc（depth +1），但深度守卫上限是 6，放得下。
                unsafe { SendDlgItemMessageW(hwnd, IDC_STYLE_LIST, LB_SETCURSEL, WPARAM(0), LPARAM(0)) };
                unsafe {
                    SendMessageW(
                        hwnd,
                        WM_COMMAND_,
                        WPARAM(((LBN_SELCHANGE_ as usize) << 16) | IDC_STYLE_LIST as usize),
                        LPARAM(0),
                    )
                };

                SendDlgItemMessageW(hwnd, IDC_COLOR_LIST, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
            }
            for (key, value) in &state.colors {
                let text = wide(&format!("{key}   0x{value:08x}"));
                unsafe { SendDlgItemMessageW(hwnd, IDC_COLOR_LIST, LB_ADDSTRING, WPARAM(0), LPARAM(text.as_ptr() as isize)) };
            }
            // 按官方做法：样式值的下拉框与编辑框二选一显示（这里默认显示下拉框）
            unsafe {
                ShowWindow(GetDlgItem(Some(hwnd), IDC_STYLE_VALUE_EDIT), SW_HIDE);
            }
            1
        }
        WM_COMMAND_ => {
            let id = (wparam.0 & 0xffff) as i32;
            let notification = ((wparam.0 >> 16) & 0xffff) as u32;
            if id == IDC_COLOR_SCHEME && notification == CBN_SELCHANGE {
                let Some(state) = (unsafe { state_of(hwnd) }) else {
                    return 0;
                };
                let index = unsafe {
                    SendDlgItemMessageW(hwnd, IDC_COLOR_SCHEME, CB_GETCURSEL, WPARAM(0), LPARAM(0))
                }
                .0 as usize;
                let mut state = state.borrow_mut();
                if index < state.schemes.len() && index != state.selected {
                    state.selected = index;
                    let id = state.schemes[index].0.clone();
                    // 写用户补丁：等价于官方 customize_string("style/color_scheme", id)
                    // 皮肤改动将写入 theme\<皮肤>\<皮肤>.theme.toml，不再写 weasel.custom.toml。
            let path = state.user.join("theme").join("theme.toml");
                    match style_settings::set_active_scheme(&path, &id) {
                        Ok(()) => {
                            state.reload();
                            // 列表内容随配色变化，重建一遍
                            unsafe {
                                SendDlgItemMessageW(hwnd, IDC_COLOR_LIST, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
                            }
                            for (key, value) in &state.colors {
                                let text = wide(&format!("{key}   0x{value:08x}"));
                                unsafe { SendDlgItemMessageW(hwnd, IDC_COLOR_LIST, LB_ADDSTRING, WPARAM(0), LPARAM(text.as_ptr() as isize)) };
                            }
                            unsafe { InvalidateRect(Some(hwnd), None, true) };
                        }
                        Err(error) => {
                            let text = wide(&format!("写入补丁失败：{error}"));
                            unsafe {
                                MessageBoxW(Some(hwnd), PCWSTR(text.as_ptr()), PCWSTR(wide("提示").as_ptr()), MB_OK | MB_ICONWARNING);
                            }
                        }
                    }
                }
                return 1;
            }
            // 选中一条样式键：枚举键把官方候选项填进下面的下拉框。
            if id == IDC_STYLE_LIST && ((wparam.0 >> 16) & 0xffff) as u32 == LBN_SELCHANGE_ {
                let index = unsafe { SendDlgItemMessageW(hwnd, IDC_STYLE_LIST, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
                if index >= 0 {
                    // 记住当前选中的键：改值时焦点会移到下面的下拉框，
                    // 那时 LB_GETCURSEL 返回 -1，只能靠这里存的下标。
                    if let Some(rc) = unsafe { state_of(hwnd) } {
                        if let Ok(mut state) = rc.try_borrow_mut() { state.selected_style_key = index as usize; }
                    }
                    if let Some(key) = crate::style_keys::KEYS.get(index as usize) {
                        unsafe { SendDlgItemMessageW(hwnd, IDC_STYLE_VALUE_COMBO, CB_RESETCONTENT, WPARAM(0), LPARAM(0)) };
                        // 当前值先取出来：Theme 类型和后面的"选中当前项"都要用，
                        // 而两者都需要 borrow state，别重复 borrow。
                        let state_rc = unsafe { state_of(hwnd) };
                        if key.ty == crate::style_keys::StyleValueType::Boolean {
                            // 布尔键：官方用勾选框，这里给「开 / 关」两项，写回 true / false。
                            for label in ["开", "关"] {
                                let text = wide(label);
                                unsafe { SendDlgItemMessageW(hwnd, IDC_STYLE_VALUE_COMBO, CB_ADDSTRING, WPARAM(0), LPARAM(text.as_ptr() as isize)) };
                            }
                        } else if key.ty == crate::style_keys::StyleValueType::Theme {
                            // ⚠️ 配色方案类（`style/color_scheme`、`style/color_scheme_dark`）的
                            // `choices` 在 `style_keys.rs` 里是**空数组**（`choices: &[]`），
                            // 因为候选来自运行时读到的皮肤列表，不是静态枚举。
                            // 原来只处理了 Boolean 和静态 choices，于是这两个键的下拉框**永远是空的**
                            // —— 用户截图确认过"该有个选择内容啊"。
                            // 这里用 state.schemes（和左上那个配色下拉框同源）。
                            if let Some(rc) = &state_rc {
                                let state_ref = rc.borrow();
                                for (id, name) in &state_ref.schemes {
                                    let text = wide(name);
                                    unsafe { SendDlgItemMessageW(hwnd, IDC_STYLE_VALUE_COMBO, CB_ADDSTRING, WPARAM(0), LPARAM(text.as_ptr() as isize)) };
                                    let _ = id;
                                }
                            }
                        } else {
                            for choice in key.choices {
                                // 未实现的布局类型（文字竖排 / 全屏）加"（暂未支持）"后缀，
                                // 免得用户以为选了没反应是 bug。
                                let text = wide(&choice_label(key, choice));
                                unsafe { SendDlgItemMessageW(hwnd, IDC_STYLE_VALUE_COMBO, CB_ADDSTRING, WPARAM(0), LPARAM(text.as_ptr() as isize)) };
                            }
                        }
                        // 把下拉框选中到**当前值**（官方即如此：显示「居中」这类中文标签）。
                        // 值取自缓存；布尔键的 true/false 对应「开 / 关」两项；
                        // Theme 类（配色方案）要到 state.schemes 里按 id 找位置 ——
                        // 它的 `key.choices` 是空的，用 choices 找永远返回 None，
                        // 结果就是"列表有内容但没预选"。
                        if let Some(key_ref) = crate::style_keys::KEYS.get(index as usize) {
                            let current = match &state_rc {
                                Some(rc) => {
                                    let state_ref = rc.borrow();
                                    style_value(&state_ref, key_ref.key)
                                }
                                None => None,
                            };
                            let target = match current.as_deref() {
                                Some("true") => Some(0),
                                Some("false") => Some(1),
                                Some(value) => {
                                    if key_ref.ty == crate::style_keys::StyleValueType::Theme {
                                        state_rc.as_ref().and_then(|rc| {
                                            let state_ref = rc.borrow();
                                            state_ref
                                                .schemes
                                                .iter()
                                                .position(|(id, _)| id == value)
                                                .map(|pos| pos)
                                        })
                                    } else {
                                        key_ref
                                            .choices
                                            .iter()
                                            .position(|choice| choice.value == value)
                                    }
                                }
                                None => None,
                            };
                            if let Some(sel) = target {
                                unsafe {
                                    SendDlgItemMessageW(
                                        hwnd,
                                        IDC_STYLE_VALUE_COMBO,
                                        CB_SETCURSEL,
                                        WPARAM(sel),
                                        LPARAM(0),
                                    )
                                };
                            }
                        }
                    }
                }
                return 1;
            }
            // 选值即写补丁 —— 官方那个「设置」按钮已按需求去掉，选择就是设置。
            if id == IDC_STYLE_VALUE_COMBO && ((wparam.0 >> 16) & 0xffff) as u32 == CBN_SELCHANGE_ {
                let sel = unsafe { SendDlgItemMessageW(hwnd, IDC_STYLE_VALUE_COMBO, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
                // 焦点在下拉框时 LB_GETCURSEL 会返回 -1（ListBox 失焦），
                // 所以取不到就回退到"上次选中时记下的下标"。
                let key_index = {
                    let from_list = unsafe { SendDlgItemMessageW(hwnd, IDC_STYLE_LIST, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
                    if from_list >= 0 {
                        from_list
                    } else {
                        unsafe { state_of(hwnd) }
                            .map(|rc| rc.borrow().selected_style_key as isize)
                            .unwrap_or(-1)
                    }
                };
                if sel >= 0 && key_index >= 0 {
                    if let Some(key) = crate::style_keys::KEYS.get(key_index as usize) {
                        let value = match key.ty {
                            crate::style_keys::StyleValueType::Boolean => {
                                Some(if sel == 0 { "true" } else { "false" })
                            }
                            // ⚠️ 配色方案类（`style/color_scheme` / `_dark`）的 `choices` 是**空数组**，
                            // 候选来自运行时皮肤列表。原来落进 `_` 分支取 `choices.get(sel)` 恒为 None，
                            // 于是"选个配色"根本走不到写回 —— 这就是"右侧两个皮肤选不了"。
                            // 真正的取值由 save_style_value 从 schemes 按 sel 索引取，这里只需放行。
                            crate::style_keys::StyleValueType::Theme => Some(""),
                            _ => key.choices.get(sel as usize).map(|choice| choice.value),
                        };
                        if let Some(value) = value {
                            // 去 YAML 后这里曾经是空挂（只写了 `let _ = (...)`），
                            // 所以"选了个值，什么都不会变"。现在真正写进用户 theme.toml。
                            let outcome = unsafe { state_of(hwnd) }.map(|state| {
                                let state = state.borrow();
                                save_style_value(&state.user, key, sel as usize, &state.schemes)
                            });
                            match outcome {
                                Some(Ok(shown)) => {
                                    // 写成功后重读并刷新右侧列表，让"值"那一栏立刻可见。
                                    let refreshed = unsafe { state_of(hwnd) }.map(|state| {
                                        let mut state = state.borrow_mut();
                                        state.reload();
                                        state.style_values.clone()
                                    });
                                    if refreshed.is_some() {
                                        unsafe {
                                            SendDlgItemMessageW(
                                                hwnd,
                                                IDC_STYLE_LIST,
                                                LB_RESETCONTENT,
                                                WPARAM(0),
                                                LPARAM(0),
                                            );
                                        }
                                        for (index, item) in crate::style_keys::KEYS.iter().enumerate() {
                                            let text = wide(&format!("{}   {}", item.zh_hans, item.key));
                                            unsafe {
                                                SendDlgItemMessageW(
                                                    hwnd,
                                                    IDC_STYLE_LIST,
                                                    LB_ADDSTRING,
                                                    WPARAM(0),
                                                    LPARAM(text.as_ptr() as isize),
                                                )
                                            };
                                            let _ = index;
                                        }
                                        // 保持选中项不跳走
                                        unsafe {
                                            SendDlgItemMessageW(
                                                hwnd,
                                                IDC_STYLE_LIST,
                                                LB_SETCURSEL,
                                                WPARAM(key_index as usize),
                                                LPARAM(0),
                                            );
                                            InvalidateRect(Some(hwnd), None, true);
                                        }
                                    }
                                    let _ = shown;
                                }
                                Some(Err(error)) => {
                                    let text = wide(&format!("保存失败：{error}"));
                                    unsafe {
                                        MessageBoxW(
                                            Some(hwnd),
                                            PCWSTR(text.as_ptr()),
                                            PCWSTR(wide("提示").as_ptr()),
                                            MB_OK | MB_ICONWARNING,
                                        );
                                    }
                                }
                                None => {}
                            }
                        }
                    }
                }
                return 1;
            }

            if id == IDOK || id == IDCANCEL {
                unsafe { EndDialog(hwnd, id as isize) };
                return 1;
            }
            0
        }
        WM_MEASUREITEM_ => {
            let item = lparam.0 as *mut MEASUREITEMSTRUCT;
            if !item.is_null() && unsafe { (*item).CtlID } == IDC_COLOR_LIST as u32 || unsafe { (*item).CtlID } == IDC_STYLE_LIST as u32 {
                unsafe { (*item).itemHeight = 30 };
                return 1;
            }
            0
        }
        WM_DRAWITEM_ => {
            let item = lparam.0 as *mut DRAWITEMSTRUCT;
            if item.is_null() || unsafe { (*item).CtlID } != IDC_COLOR_LIST as u32 && unsafe { (*item).CtlID } != IDC_STYLE_LIST as u32 {

                return 0;
            }
            // 右侧样式列表：两栏 —— 第一列（标签+键路径）左对齐，第二列（当前值）右对齐。
            if unsafe { (*item).CtlID } == IDC_STYLE_LIST as u32 {
                let item = unsafe { &*item };
                let Some(key) = crate::style_keys::KEYS.get(item.itemID as usize) else {
                    return 1;
                };
                // 值取自缓存（**不读盘**）——这条路径每画一行就走一次，读盘会肉眼可见地卡。
                let current = unsafe { state_of(hwnd) }
                    .map(|state| { let state = state.borrow(); style_value(&state, key.key) })
                    .flatten()
                    .unwrap_or_default();
                unsafe {
                    let selected = item.itemState & ODS_SELECTED != 0;
                    let background = if selected { GetSysColor(COLOR_HIGHLIGHT) } else { GetSysColor(COLOR_WINDOW) };
                    let foreground = if selected { GetSysColor(COLOR_HIGHLIGHTTEXT) } else { GetSysColor(COLOR_WINDOWTEXT) };
                    let brush = CreateSolidBrush(COLORREF(background));
                    FillRect(item.hDC, &item.rcItem, brush);
                    DeleteObject(HGDIOBJ(brush.0));
                    SetBkMode(item.hDC, TRANSPARENT_);
                    SetTextColor(item.hDC, COLORREF(foreground));
                    let mut left_rc = item.rcItem;
                    left_rc.left += 6;
                    // 左栏（标签 + 键路径）留 1/2：键路径最长的是
                    // `style/candidate_abbreviate_length`（32 字符），3/5 时会被截断成
                    // `…_colo`（实测），右栏的值也会被挤到列表右边界外。对半分刚好都放得下。
                    left_rc.right = left_rc.left + (left_rc.right - left_rc.left) / 2;
                    let left_text = wide(&format!("{}   {}", key.zh_hans, key.key));
                    DrawTextW(item.hDC, PCWSTR(left_text.as_ptr()), -1, &mut left_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
                    let mut right_rc = item.rcItem;
                    right_rc.left = left_rc.right;
                    right_rc.right -= 6;
                    let right_text = wide(&current);
                    DrawTextW(item.hDC, PCWSTR(right_text.as_ptr()), -1, &mut right_rc, DT_RIGHT | DT_VCENTER | DT_SINGLELINE);
                }
                return 1;
            }
            let item = unsafe { &*item };
            let Some(state) = (unsafe { state_of(hwnd) }) else {
                return 0;
            };
            let state = state.borrow();
            let index = item.itemID as usize;
            let Some((key, value)) = state.colors.get(index) else {
                return 1;
            };
            unsafe {
                // 选中行底色，其余用窗口底色
                let selected = item.itemState & ODS_SELECTED != 0;
                let background = if selected {
                    GetSysColor(COLOR_HIGHLIGHT)
                } else {
                    GetSysColor(COLOR_WINDOW)
                };
                let foreground = if selected {
                    GetSysColor(COLOR_HIGHLIGHTTEXT)
                } else {
                    GetSysColor(COLOR_WINDOWTEXT)
                };
                let mut brush = CreateSolidBrush(COLORREF(background));
                FillRect(item.hDC, &item.rcItem, brush);
                DeleteObject(HGDIOBJ(brush.0));

                // 色块：按颜色键的取值画一个小方块（圆整到 RGB）
                let r = ((value >> 16) & 0xff) as u8;
                let g = ((value >> 8) & 0xff) as u8;
                let b = (value & 0xff) as u8;
                let mut swatch = item.rcItem;
                swatch.left += 2;
                swatch.right = swatch.left + 24;
                swatch.top += 2;
                swatch.bottom -= 2;
                let color = COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16));
                brush = CreateSolidBrush(color);
                FillRect(item.hDC, &swatch, brush);
                DeleteObject(HGDIOBJ(brush.0));
                // 色块描边，浅色配色也能看清边界
                let mut frame = swatch;
                FrameRect(item.hDC, &frame, GetSysColorBrush(COLOR_WINDOWTEXT));

                // 文字：颜色键名 + 取值（按官方写法显示：不透明 6 位、带 alpha 8 位）
                let text = wide(&format!(
                    "{key}   {}",
                    weasel_style::color::format_text(*value, weasel_style::ColorFormat::Abgr)
                ));
                let mut text_area = item.rcItem;
                text_area.left = swatch.right + 6;
                SetBkMode(item.hDC, TRANSPARENT_);
                SetTextColor(item.hDC, COLORREF(foreground));
                DrawTextW(item.hDC, PCWSTR(text.as_ptr()), -1, &mut text_area, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            }
            1
        }
        WM_CLOSE_ => {
            unsafe { EndDialog(hwnd, IDCANCEL as isize) };
            1
        }
        _ => 0,
    }
}

/// 按官方的命令行习惯分发：`/style`、`/switcher`、`/dict`。
///
/// 返回 `true` 表示已由对话框处理（调用方不要再启动 Slint 界面）。
pub fn run_from_args(args: &[String], shared: &Path, user: &Path) -> bool {
    // SysListView32 属于通用控件库，创建任何含它的对话框之前都要先初始化。
    unsafe {
        let classes = weasel_common::bindings::INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<weasel_common::bindings::INITCOMMONCONTROLSEX>() as u32,
            dwICC: 0x0000_0001, // ICC_LISTVIEW_CLASSES
        };
        weasel_common::bindings::InitCommonControlsEx(&classes);
    }
    for arg in args {
        let arg = arg.to_ascii_lowercase();
        if arg == "/style" || arg == "--style" || arg == "-style" {
            if let Err(error) = show_style(shared, user) {
                eprintln!("Weasel-Next-Setting: {error}");
            }
            return true;
        }
        if arg == "/switcher" || arg == "--switcher" || arg == "-switcher" {
            // 数据填充尚未成功：列能建（返回 0），但 LVM_INSERTITEMW 插不进行
            // （LVM_GETITEMCOUNT 恒为 0）。先显示官方模板，保证入口可用；
            // 排查线索与诊断日志都留在 switcher_dialog.rs 里。
            if let Err(error) = crate::switcher_dialog::show(shared, user) {
                eprintln!("Weasel-Next-Setting: {error}");
            }
            return true;
        }
        if arg == "/stat" || arg == "--stat" || arg == "-stat" {
            // 输入统计（Slint 独立窗口）。数据来自 `%APPDATA%\Rime-Next\stats\*.toml`，
            // 采集还没接上，所以现在会显示 0 —— 先把界面结构立起来。
            if let Err(error) = crate::show_stats() {
                eprintln!("Weasel-Next-Setting: {error}");
            }
            return true;
        }
        if arg == "/deploy" || arg == "--deploy" || arg == "-deploy" {
            // 带进度条的部署。部署实际由 broker 执行，这里只负责发命令 + 轮询完成。
            if let Err(error) = crate::deploy_with_progress() {
                eprintln!("Weasel-Next-Setting: {error}");
            }
            return true;
        }
        if arg == "/dict" || arg == "--dict" || arg == "-dict" {
            if let Err(error) = show_placeholder(IDD_DICT_MANAGEMENT) {
                eprintln!("Weasel-Next-Setting: {error}");
            }
            return true;
        }
    }
    false
}

/// 仅显示模板、不接数据的对话框（方案选单设定与词典管理先用它占位，
/// 保证模板本身可编译可用；数据接线随后补）。
fn show_placeholder(dialog: u16) -> Result<(), String> {
    unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, _: LPARAM) -> isize {
        match message {
            WM_COMMAND_ => {
                let id = (wparam.0 & 0xffff) as i32;
                if id == IDOK || id == IDCANCEL {
                    unsafe { EndDialog(hwnd, id as isize) };
                    return 1;
                }
                0
            }
            WM_CLOSE_ => {
                unsafe { EndDialog(hwnd, IDCANCEL as isize) };
                1
            }
            _ => 0,
        }
    }
    let instance = unsafe { GetModuleHandleW(None) };
    let result = unsafe {
        DialogBoxParamW(
            Some(HINSTANCE(instance.0)),
            PCWSTR(dialog as usize as *const u16),
            None,
            Some(proc),
            LPARAM(0),
        )
    };
    if result == -1 {
        Err(format!("对话框 {dialog} 创建失败"))
    } else {
        Ok(())
    }
}







