# 官方 0.17.4 的 style 设置（TOML 形式，逐键与官方 weasel.toml 对齐）

来源：`C:\Program Files\Rime\weasel-0.17.4\data\weasel.toml` 的 `style:` 段
（已逐键逐值核对：**0 处值差异**；仅同义键命名不同，两个名字都认）

```toml
# theme.toml —— 主题主文件：布局 + **当前使用哪套皮肤**。
#
# 拆分布局：
#   theme\theme.toml                    布局（[style] / [style.layout]）+ color_scheme 指定用哪套皮肤
#   theme\<皮肤名>\<皮肤名>.theme.toml     只有配色（[schemes."名字"] 各段的颜色值）
#
# 用户覆盖：%APPDATA%\Rime-Next\theme\theme.toml（可选，优先）

config_version = "0.23"
color_scheme = "aqua"
# color_scheme_dark = ""   # 官方默认没有暗色方案
[style]
font_face = "Microsoft YaHei"
label_font_face = "Microsoft YaHei"
comment_font_face = "Microsoft YaHei"
font_point = 14
label_font_point = 14
comment_font_point = 14
candidate_abbreviate_length = 0
antialias_mode = "default"
ascii_tip_follow_cursor = false
click_to_capture = false
horizontal = false
fullscreen = false
inline_preedit = false
preedit_type = "composition"
display_tray_icon = false
hover_type = "none"
paging_on_scroll = false
vertical_auto_reverse = false
vertical_text = false
vertical_text_left_to_right = false
vertical_text_with_wrap = false
vertical_right_to_left = false
enhanced_position = false
text_orientation = ""
label_format = "%s."
mark_text = ""

[style.layout]
align_type = "center"
min_width = 160
max_width = 0
min_height = 0
max_height = 0
border = 3
# 官方风格 yaml 里叫 border_width，同义；两个名字都认 ✓
border_width = 3
margin_x = 12
margin_y = 12
spacing = 10
candidate_spacing = 5
hilite_spacing = 4
hilite_padding = 2
hilite_padding_x = 2
hilite_padding_y = 2
round_corner = 4
corner_radius = 4
hilited_corner_radius = 4
shadow_radius = 0
shadow_offset_x = 4
shadow_offset_y = 4
linespacing = 0
baseline = 0
layout_type_mode = -1

```
