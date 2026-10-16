//! 官方界面风格设定对话框暴露的样式键表。
//!
//! 这张表是照官方 `include/WeaselStyleKeys.h` 的 `weasel::GetStyleKeys()` 机械搬过来的
//! （键、简繁英三种标签、值类型、枚举候选项），顺序也与官方一致 —— 官方注释称其为
//! "Single source of truth for the style keys exposed in the settings dialog"。

/// 样式值的类型，决定右侧用下拉框还是编辑框。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleValueType {
    /// 主题（配色方案），候选项来自主题列表。
    Theme,
    /// 枚举，候选项见 `choices`。
    Enum,
    /// 布尔。
    Boolean,
    /// 整数。
    Int,
    /// 字符串。
    Str,
}

/// 一个枚举候选项：写入配置的规范值 + 三种语言的显示标签。
pub struct StyleChoice {
    pub value: &'static str,
    pub zh_hans: &'static str,
    pub zh_hant: &'static str,
    pub en: &'static str,
}

/// 一个样式键：配置路径 + 三种语言的标签 + 值类型 + 枚举候选项。
pub struct StyleKey {
    pub key: &'static str,
    pub zh_hans: &'static str,
    pub zh_hant: &'static str,
    pub en: &'static str,
    pub ty: StyleValueType,
    pub choices: &'static [StyleChoice],
}

/// 官方 kHoverTypeChoices。
pub const HOVERTYPE_CHOICES: &[StyleChoice] = &[
    StyleChoice { value: "none", zh_hans: "无", zh_hant: "無", en: "None" },
    StyleChoice { value: "semi_hilite", zh_hans: "半高亮", zh_hant: "半高亮", en: "Semi highlight" },
    StyleChoice { value: "hilite", zh_hans: "高亮", zh_hant: "高亮", en: "Highlight" },
];

/// 官方 kPreeditTypeChoices。
pub const PREEDITTYPE_CHOICES: &[StyleChoice] = &[
    StyleChoice { value: "composition", zh_hans: "编码", zh_hant: "編碼", en: "Composition" },
    StyleChoice { value: "preview", zh_hans: "预览", zh_hant: "預覽", en: "Preview" },
    StyleChoice { value: "preview_all", zh_hans: "预览全部", zh_hant: "預覽全部", en: "Preview all" },
];

/// 官方 kLayoutTypeChoices。
pub const LAYOUTTYPE_CHOICES: &[StyleChoice] = &[
    StyleChoice { value: "vertical", zh_hans: "竖直", zh_hant: "垂直", en: "Vertical" },
    StyleChoice { value: "horizontal", zh_hans: "水平", zh_hant: "水平", en: "Horizontal" },
    StyleChoice { value: "vertical_text", zh_hans: "竖直 + 文字竖排", zh_hant: "垂直 + 直書", en: "Vertical text" },
    StyleChoice { value: "vertical+fullscreen", zh_hans: "竖直 + 全屏", zh_hant: "垂直 + 全螢幕", en: "Vertical + fullscreen" },
    StyleChoice { value: "horizontal+fullscreen", zh_hans: "水平 + 全屏", zh_hant: "水平 + 全螢幕", en: "Horizontal + fullscreen" },
    StyleChoice { value: "vertical_text+fullscreen", zh_hans: "竖直 + 文字竖排 + 全屏", zh_hant: "直書 + 全螢幕", en: "Vertical text + fullscreen" },
];

/// 官方 kAlignTypeChoices。
pub const ALIGNTYPE_CHOICES: &[StyleChoice] = &[
    StyleChoice { value: "top", zh_hans: "顶部", zh_hant: "頂部", en: "Top" },
    StyleChoice { value: "center", zh_hans: "居中", zh_hant: "居中", en: "Center" },
    StyleChoice { value: "bottom", zh_hans: "底部", zh_hant: "底部", en: "Bottom" },
];

/// 官方 kAntialiasChoices。
pub const ANTIALIAS_CHOICES: &[StyleChoice] = &[
    StyleChoice { value: "default", zh_hans: "默认", zh_hant: "預設", en: "Default" },
    StyleChoice { value: "cleartype", zh_hans: "ClearType", zh_hant: "ClearType", en: "ClearType" },
    StyleChoice { value: "grayscale", zh_hans: "灰度", zh_hant: "灰階", en: "Grayscale" },
    StyleChoice { value: "aliased", zh_hans: "无抗锯齿", zh_hant: "無抗鋸齒", en: "Aliased" },
    StyleChoice { value: "force_dword", zh_hans: "强制 DWORD 对齐", zh_hant: "強制 DWORD 對齊", en: "Force DWORD alignment" },
];

/// 全部样式键，顺序与官方 `GetStyleKeys()` 一致。
pub const KEYS: &[StyleKey] = &[
    StyleKey { key: "style/color_scheme", zh_hans: "配色方案", zh_hant: "配色方案", en: "Color scheme", ty: StyleValueType::Theme, choices: &[] },
    StyleKey { key: "style/color_scheme_dark", zh_hans: "暗色主题", zh_hant: "暗色主題", en: "Dark theme", ty: StyleValueType::Theme, choices: &[] },
    StyleKey { key: "style/layout/type", zh_hans: "布局方向", zh_hant: "佈局方向", en: "Layout direction", ty: StyleValueType::Enum, choices: LAYOUTTYPE_CHOICES },
    StyleKey { key: "style/layout/align_type", zh_hans: "对齐方式", zh_hant: "對齊方式", en: "Alignment", ty: StyleValueType::Enum, choices: ALIGNTYPE_CHOICES },
    StyleKey { key: "style/preedit_type", zh_hans: "编码显示方式", zh_hant: "編碼顯示方式", en: "Preedit display", ty: StyleValueType::Enum, choices: PREEDITTYPE_CHOICES },
    StyleKey { key: "style/antialias_mode", zh_hans: "字体渲染", zh_hant: "字型渲染", en: "Font rendering", ty: StyleValueType::Enum, choices: ANTIALIAS_CHOICES },
    StyleKey { key: "style/hover_type", zh_hans: "悬停效果", zh_hant: "懸停效果", en: "Hover effect", ty: StyleValueType::Enum, choices: HOVERTYPE_CHOICES },
    StyleKey { key: "style/inline_preedit", zh_hans: "内联编码", zh_hant: "內聯編碼", en: "Inline preedit", ty: StyleValueType::Boolean, choices: &[] },
    StyleKey { key: "style/display_tray_icon", zh_hans: "显示托盘图标", zh_hant: "顯示系統匣圖示", en: "Show tray icon", ty: StyleValueType::Boolean, choices: &[] },
    StyleKey { key: "style/ascii_tip_follow_cursor", zh_hans: "英文提示跟随光标", zh_hant: "英文提示跟隨游標", en: "ASCII tip follows cursor", ty: StyleValueType::Boolean, choices: &[] },
    StyleKey { key: "style/paging_on_scroll", zh_hans: "滚轮翻页", zh_hant: "滾輪翻頁", en: "Page on scroll", ty: StyleValueType::Boolean, choices: &[] },
    StyleKey { key: "style/click_to_capture", zh_hans: "点击截屏", zh_hant: "點擊截屏", en: "Click to capture", ty: StyleValueType::Boolean, choices: &[] },
    StyleKey { key: "style/enhanced_position", zh_hans: "增强定位", zh_hant: "增強定位", en: "Enhanced positioning", ty: StyleValueType::Boolean, choices: &[] },
    StyleKey { key: "style/vertical_auto_reverse", zh_hans: "竖直自动换向", zh_hant: "垂直自動換向", en: "Vertical auto reverse", ty: StyleValueType::Boolean, choices: &[] },
    StyleKey { key: "style/vertical_text_left_to_right", zh_hans: "竖排从左到右", zh_hant: "直書從左到右", en: "Vertical text LTR", ty: StyleValueType::Boolean, choices: &[] },
    StyleKey { key: "style/vertical_text_with_wrap", zh_hans: "竖排自动换行", zh_hant: "直書自動換行", en: "Vertical text wrap", ty: StyleValueType::Boolean, choices: &[] },
    StyleKey { key: "style/vertical_right_to_left", zh_hans: "竖直布局从右到左", zh_hant: "垂直佈局從右到左", en: "Vertical layout RTL", ty: StyleValueType::Boolean, choices: &[] },
    StyleKey { key: "style/candidate_abbreviate_length", zh_hans: "候选缩写长度", zh_hant: "候選縮寫長度", en: "Candidate abbreviation length", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/label_format", zh_hans: "序号格式", zh_hant: "序號格式", en: "Label format", ty: StyleValueType::Str, choices: &[] },
    StyleKey { key: "style/mark_text", zh_hans: "标记文本", zh_hant: "標記文本", en: "Mark text", ty: StyleValueType::Str, choices: &[] },
    StyleKey { key: "style/layout/border", zh_hans: "边框宽度", zh_hant: "邊框寬度", en: "Border width", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/margin_x", zh_hans: "左右边距", zh_hant: "左右邊距", en: "Horizontal margin", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/margin_y", zh_hans: "上下边距", zh_hant: "上下邊距", en: "Vertical margin", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/spacing", zh_hans: "间距", zh_hant: "間距", en: "Spacing", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/candidate_spacing", zh_hans: "候选间距", zh_hant: "候選間距", en: "Candidate spacing", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/hilite_spacing", zh_hans: "高亮间距", zh_hant: "高亮間距", en: "Highlight spacing", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/hilite_padding_x", zh_hans: "高亮内边距（水平）", zh_hant: "高亮內邊距（水平）", en: "Highlight padding (X)", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/hilite_padding_y", zh_hans: "高亮内边距（垂直）", zh_hant: "高亮內邊距（垂直）", en: "Highlight padding (Y)", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/hilited_corner_radius", zh_hans: "高亮圆角半径", zh_hant: "高亮圓角半徑", en: "Highlight corner radius", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/corner_radius", zh_hans: "圆角半径", zh_hant: "圓角半徑", en: "Corner radius", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/shadow_radius", zh_hans: "阴影半径", zh_hant: "陰影半徑", en: "Shadow radius", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/shadow_offset_x", zh_hans: "阴影水平偏移", zh_hant: "陰影水平偏移", en: "Shadow offset X", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/shadow_offset_y", zh_hans: "阴影垂直偏移", zh_hant: "陰影垂直偏移", en: "Shadow offset Y", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/min_width", zh_hans: "最小宽度", zh_hant: "最小寬度", en: "Min width", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/max_width", zh_hans: "最大宽度", zh_hant: "最大寬度", en: "Max width", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/min_height", zh_hans: "最小高度", zh_hant: "最小高度", en: "Min height", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/max_height", zh_hans: "最大高度", zh_hant: "最大高度", en: "Max height", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/baseline", zh_hans: "基线偏移（%）", zh_hant: "基線偏移（%）", en: "Baseline (%)", ty: StyleValueType::Int, choices: &[] },
    StyleKey { key: "style/layout/linespacing", zh_hans: "行距（%）", zh_hant: "行距（%）", en: "Line spacing (%)", ty: StyleValueType::Int, choices: &[] },
];
