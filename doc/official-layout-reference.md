# 官方布局算法参照（抄写用）

来源：`C:\Code\weasel\WeaselUI\`（官方 0.17.4 源码树）
用途：`theme\official`（Rust 版主题）要**逐条对齐**这套算法；我们的 `logic.rs` 目前是简化版，
      所以候选窗的布局与官方不一致。

## 一、根公式（Layout.cpp 构造函数 L24-60）

```
DPI 缩放：以下配置值全部 × scale（scale = dpi / 96）
  min_width  min_height  max_width  max_height  border
  margin_x  margin_y  spacing  candidate_spacing  hilite_spacing
  hilite_padding_x  hilite_padding_y  round_corner  round_corner_ex
  shadow_radius  shadow_offset_x  shadow_offset_y

real_margin_x = max(|margin_x|, hilite_padding_x)      ← 取较大者，不是相加
real_margin_y = max(|margin_y|, hilite_padding_y)

offsetX = offsetY = 0
if shadow_radius != 0:
    offsetX = |shadow_offset_x| + shadow_radius * 2
    offsetY = |shadow_offset_y| + shadow_radius * 2
    if shadow_offset_x != 0 or shadow_offset_y != 0:
        offsetX -= shadow_radius / 2
        offsetY -= shadow_radius / 2
offsetX += border * 2                                  ← 边框占位
offsetY += border * 2

contentSize = (width + offsetX * 2, height + offsetY * 2)
```

## 二、竖排布局 DoLayout（VerticalLayout.cpp L5-210）

```
height 从 real_margin_y 起
base_offset = _CalcMarkMetrics(false)          // 页码标记占宽（无标记时为 0）
(pgw, pgh)  = _CalcPageIndicator(false)        // 只有 prevpage/nextpage 颜色有 alpha 才非零

预编辑（!IsInlinePreedit() && preedit 非空）：
    _LayoutInlineRect(preeditSize, false, true, pgw, pgh, real_margin_x, width, height, rect)
辅助串 aux：同样处理

逐个候选 i：
    if i > 0: height += candidate_spacing
    w = real_margin_x + base_offset
    labelRect  = (w, height, w+labelSize.cx, height+labelSize.cy);  w += (labelSize.cx + hilite_spacing) * labelFontValid
    textRect   = (w, height, w+textSize.cx,  height+textSize.cy);   w += textSize.cx * textFontValid
    comment：w += hilite_spacing；comment_shift_width = max(..., w)
             commentRect 宽度 = commentSize.cx（稍后统一左对齐）
    h = max(labelSize.cy, textSize.cy, commentSize.cy)
    align_type: CENTER → 各矩形 OffsetRect(0, (h - rectHeight)/2)
                BOTTOM → OffsetRect(0, h - rectHeight)
    highlight 上下界 = label/text/comment 三者的最小 top 与最大 bottom
    candidateRect = (real_margin_x + offsetX, hlTop, width - real_margin_x + offsetX, hlBot)
    width = max(width, w);  height += h

comment 对齐：candidateCommentRects[i].left = real_margin_x + comment_shift_width
无候选：height -= spacing
height += real_margin_y
有候选：width = max(width, min_width);  height = max(height, min_height)
_UpdateStatusIconLayout(&width, &height)
候选矩形右对齐：candidateRects[i].right = max(right, left - real_margin_x + width - real_margin_x)
contentSize = (width + offsetX*2, height + offsetY*2)
highlightRect = candidateRects[id]                       // 高亮就是选中候选那个矩形
页码箭头（_pageEnabled && 有候选 && !inline_preedit）：
    _prex = contentSize.cx - offsetX - real_margin_x + hilite_padding_x - pgw
    _prey = (preeditRect.top + preeditRect.bottom)/2 - pagePrevSize.cy/2
预编辑分段：_PrecomputePreeditRects(...) → before / hilite / after 三段
```

## 三、标记与页码（StandardLayout.cpp L487-521 / L168-190）

```
_CalcMarkMetrics:  hilited_mark_color 没有 alpha 或没有候选 → mark_width/height/gap 全 0，返回 0
                   mark_text 为空时用 "|" 量尺寸，再按 1/7 与 linespacing 调整，最小 6
                   mark_gap = mark_width(或 mark_text 宽度) + hilite_spacing（mark_text 非空时）
_CalcPageIndicator: _pageEnabled = (prevpage_color & 0xff000000) && (nextpage_color & 0xff000000)
                   pgw = 左箭头宽 + 右箭头宽 + hilite_spacing + hilite_padding_x*2
                   pgh = max(左箭头高, 右箭头高)
```

## 四、我们已知的差异（待改）

1. `real_margin_x/y` 未做 `max(...)`，而是直接用 `margin_x/y` → 窗口偏大；
2. `offsetX/offsetY`（= border*2，含阴影）完全没算 → contentSize 偏小、边框位置不对；
3. 每个候选只有一个行矩形，没有 label / text / comment 三个独立矩形 → 序号列、注释列位置不对；
4. `align_type`（center/bottom）的垂直居中没做；
5. **预编辑条整条没画**（ThemeCapabilities 报 preedit: false）→ 官方最显眼的灰底 + 三角指针缺失；
6. `mark`（页码标记竖线）与页码箭头未实现（aqua 的 prevpage/nextpage 是 0x00000000，所以箭头本就不该显示 ✓）。