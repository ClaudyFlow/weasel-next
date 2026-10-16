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
---

## 五、绘制流程移植清单（照 WeaselPanel.cpp:423-509 DoPaint）

官方顺序（逐句对照，我们照做）：

```
1. BeginDraw()
2. dc->Clear(D2D1::ColorF{0,0,0,0})                    整窗清成**全透明**
3. 判断要不要画背景：
     有预编辑（且非 inline） 或 有候选 或 有辅助串      ← 三者皆无则不画（窗口空）
4. 画面板底：_HighlightRect(
       m_layout->GetContentRect(),                      ← ★ 内容矩形（已含 offset 外扩）
       round_corner_ex, border, back_color, shadow_color, border_color)
5. 画预编辑：_DrawPreedit(m_ctx.preedit, true)           ← 白底块 + 文字 + 右端小三角
6. 画辅助串：_DrawPreedit(m_ctx.aux, false)
7. 画候选：  _DrawCandidates()                           ← 选中行先 _HighlightRect(该行矩形, hilited_candidate_back_color)
                                                           再画所有行的 label/text/comment
8. 状态图标（我们不用 status icon，跳过）
9. EndDraw()  → 失败则 DeviceResources::Reset() + InitDirect2D() 重来
10. swapChain->Present(1, 0)   ← 垂直同步；设备丢失同样 Reset+Init
```

`_HighlightRect`（L799-823）——**所有圆角块共用**：

```
if roundInfo.Hemispherical: radius = round_corner_ex - border/2
if shadow_color 非全透明 且 shadow_radius:
    FillGeometry(rect, shadow_color, radius, 模糊=true)          ← 阴影
if back_color 非全透明:
    FillGeometry(rect, back_color, radius)                        ← 底色
if border_color 非全透明 且 border != 0:
    pGeometry = CreateRoundedRectanglePath(rect, radius - border/2)
    DrawGeometry(pGeometry, brush, 笔宽 = border)                  ← 描边（画在矩形内侧）
```

我们这边的对应位置（`theme\official\src\mod.rs`）：

```
paint()  L499 起
  现状：Clear(透明) ✓ → FillRectangle(整窗, background) ✗（应为"内容矩形"）
        → FillRoundedRectangle(整窗, background) ✗（应为"内容矩形"）
        → 描边 ✓ 但矩形也是整窗 ✗
  要改：把三处的矩形换成 layout 的**内容矩形**（= 内容尺寸 + offset 外扩）
        并把选中行的高亮从"整窗"改成"该候选行矩形"
logic.rs Layout
  现状：只有 cells（横向区间）+ width/height ✗
  要改：加 内容矩形、每候选矩形、预编辑矩形；按 VerticalLayout::DoLayout 计算
        （根公式见本文件第二节：real_margin = max(|margin|, hilite_padding)、offset = border*2）
```

**顺序建议**（每步都能单独验证 ✓）：

```
第一步  只换矩形：面板底/描边改成"内容矩形"，选中高亮改成"该行矩形"
         → 效果：窗口不再"红块铺满"，背景与高亮各就各位
第二步  补内容矩形与 offset（real_margin / border*2）→ 窗口大小和留白对上官方
第三步  补预编辑行（白底 + 右端小三角）→ 最显眼的差别消失
第四步  label / text / comment 三矩形 + align_type 垂直居中 → 序号列与注释列对齐官方
第五步  mark 与页码箭头（aqua 全透明 → 本就不显示；仅实现规则以便别的皮肤正确）
```