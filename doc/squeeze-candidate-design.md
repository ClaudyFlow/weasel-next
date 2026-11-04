# 挤压式三模式候选窗 · D2D 实现设计文档

> 2026-11-04 立项。设计稿:`_shots/ime-three-modes.html`(三模式挤压版)+
> `_shots/ime-prototype.html`(辅助面板完整版:剪贴板左/候选窗上/符号面板底/同步弹窗)。

## 现状

- `theme/src/layout.rs` 已有 `LayoutType::HorizontalSqueeze / VerticalSqueeze / Cells` 枚举
  和 config 解析(`layout/type: horizontal_squeeze` 等),但 build() 只映射到基础布局——**没有真正的挤压逻辑**。
- `theme/src/paint.rs`(1748 行)画的是旧设计(高亮块换色,无挤压)。
- `Renderer-VK/src/cand.rs`(f107ee5)已在 Vulkan 原型验证了挤压布局逻辑(但 CPU 渲染太慢)。

## 架构变更:布局需感知选中态

**当前**: `Layout::build(metrics, view, measured)` 不含选中参数——布局是静态的,
选中态只在 paint 时换色。
**挤压设计**: 选中候选 padding 从 `hilite_padding`(0.25em)膨胀到 `hilite_padding * 2`(0.5em),
邻居被真实挤开。布局必须**每次选中变化时重算**。

### 方案

`Layout::build` 加一个 `selected: usize` 参数(或 Metrics 加 `squeeze: bool` + view 里取
highlighted index)。C++ 也是每次 Compose 重算布局,我们的 `Engine::compose` → `paint` 也是
每次按键触发,所以只需把 `selected` 穿透进 `Layout::build`。

### 挤压布局核心公式

```
pad_normal = hilite_padding     (0.25em,theme 可调)
pad_selected = hilite_padding * 2  (0.5em)
```

每候选宽度 = 文本宽(字符格排版) + 2×pad + 序号格(0.5em) + 红块(0.5em)
候选总宽 = Σ(候选宽) + (n-1)×0.5em(红块分隔)
选中候选的宽度膨胀 → 邻居 x 坐标偏移 → "挤压"

### 三模式映射

| LayoutType | 布局 | HTML 对应 |
|---|---|---|
| HorizontalSqueeze | 单行,左→右游标推进 | .row(横流) |
| VerticalSqueeze | 竖列,上→下 | .col(飞瀑) |
| Cells | 单行,但容器高度固定(7em),内容顶齐 | .cells(竹简) |

### 设计元素(逐条对应 HTML)

| 元素 | HTML | D2D 实现 |
|---|---|---|
| 灰壳 | `.ime` #9a9a9f, padding 0.5em, radius 1em | 外层 FillRoundedRectangle |
| 白内板 | `.ime-inner` #f5f5f7 | 内层 FillRoundedRectangle |
| 红方块 | `.space` 0.5em×0.5em red | FillRectangle(每 0.5em 一个) |
| 绿分隔 | `.vsep/.hsep` green + 黑线 2px | FillRectangle×2(绿底 + 黑线) |
| 字符格 | `.ch.w1`=1em / `.ch.w05`=0.5em | 每字符独立位置(宽 1em / 0.5em) |
| 候选底 | `.item` orange, radius 0.5em | FillRoundedRectangle |
| 核白板 | `.core` #f5f5f7, padding 0.25em | FillRoundedRectangle |
| 序号 | `.num` #999 | DrawText |
| 挤压 | `.item.first` padding 0.5em | 按选中态变 pad |
| 入场 | riseIn 250ms + 错峰 30ms | paint 时按 t 插值 y 偏移 + alpha |
| 缓动 | cubic-bezier(0.16,1,0.3,1) | easeOutExpo 近似 |

### paint.rs 需要的新绘制函数

```
draw_squeeze_shell(rt, ox, oy, w, h, mode, sel, old_sel, squeeze_t, view, metrics)
  ├── 灰壳 FillRoundedRectangle(#9a9a9f)
  ├── 白内板 FillRoundedRectangle(#f5f5f7)
  ├── 编码栏:字符格拼音 + vsep + 按钮(🔣✂️🔄) + vsep + 翻页◀▶
  ├── hsep(绿底黑线)
  └── 候选区(mode 分发):
       ├── 横流:游标推进,每 item 橙底+白核+序号+红块+文本
       ├── 飞瀑:竖列
       └── 竹简:格子行(容器 7em,顶齐)
```

### 主题 yaml 接入

```yaml
layout:
  type: horizontal_squeeze   # 或 vertical_squeeze / cells
```

### 辅助面板系统(ime-prototype.html,第二阶段)

布局:剪贴板面板(左 dock)+ 主列[候选窗(上) → 符号面板(下 dock)]
- 候选窗右侧 rail 按钮:符号 / 剪贴板 / 同步(字号 = 0.5×候选)
- 剪贴板面板:全部/置顶/收藏/隐藏 tabs + 搜索 + 操作(收藏/置顶/隐藏/加密/复制/同步)
- 符号面板:symbols.yaml 驱动,底部横向滚动条,分组 tabs
- 同步弹窗:双标签(用户词库/剪贴板),WebDAV/坚果云设置,频率,过滤
- 这些是 **Engine 快照 → Renderer 新增面板绘制** 的跨层工程,排在候选窗三模式之后

## 实施顺序

1. **layout.rs**: squeeze 布局函数(HorizontalSqueeze/VerticalSqueeze/Cells 真正实现)
2. **paint.rs**: 挤压绘制(灰壳/白板/红块/绿分隔/字符格/橙底白核)
3. **config**: theme.yaml 可选 `layout/type: horizontal_squeeze`
4. **辅助面板**(第二阶段):剪贴板/符号/同步
