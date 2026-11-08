# 挤压式三模式候选窗 · D2D 实现设计文档

> 2026-11-04 立项。设计稿:`_shots/ime-three-modes.html`(三模式挤压版)+
> `_shots/ime-prototype.html`(辅助面板完整版:剪贴板左/候选窗上/符号面板底/同步弹窗)。

## 现状（2026-11-05 更新：布局层已落地）

- ✅ **`theme/src/layout.rs` 挤压布局已完成**（外科手术式，见下）。枚举/config 解析早已就位，
  `build()` 现在按方向分发：横流/竹简走单行、飞瀑走竖列。
- ✅ **`theme/src/paint.rs` 挤压绘制已落地**（2026-11-05）：新增独立帧路径 `draw_squeeze()`，
  官方 `draw()` 在 `m.squeeze` 时整帧交给它，非 squeeze 逐字节走原路径（零回归）。
  灰壳 / 白内板 / 字符格编码 / 橙底白核 / 红块 / 绿分隔 / 方片圆角全部实现，
  外包 `catch_unwind` 兜底（panic 或 Err 自动退回官方绘制）。
- ⛔ `Renderer-VK` / `Setting-vk` 两条 Vulkan 原型线已于 **2026-11-05 整体移除**。
  原因：GDI 字形光栅化 + Win32 窗口 + `SetWindowRgn` + `BitBlt` 模糊全绑死 Windows，
  跨平台无望；且三模式几何是编译期硬编码（`base=20.0`、`PAD_N=0.25`），
  `theme.yaml` 改不动结构。仅 `ash` 那段命令流可复用，价值不足。
  未提交改动备份在 `dist/vk-uncommitted-20261105.patch`，代码可从 `f107ee5` 恢复。

## 架构：布局感知选中态（已按"外科手术式"落地）

**关键洞察**：`vertical_layout()` 内部**早就算出了 `id = view.selected_index`**（第 541 行），
所以根本不需要给 `Layout::build` 加 `selected` 参数（原设计文档这里想复杂了）。
选中态每次按键都由 `Content::snapshot` 带进来，`Layout::build` 每次重算，天然满足。

### 实际做法（不动 `vertical_layout` 几百行骨架）

1. `Metrics` 加 `squeeze: bool`，由 `layout_type` 三模式推导，穿透到布局层。
2. `Row` 加 `pad_x / pad_y`（per-candidate 可变 padding）：选中项 `2p`，其余 `p`。
   `Layout::inflated_candidate()` 与 `prepare_round_info()` 统一走 `row_padding()`，
   于是**绘制、鼠标命中、圆角判定自动跟随**，paint.rs 一行没改。
3. 游标推进处插入挤压位移：累加游标上补两次 `p`
   （`i == id` 补 `p` → 选中内容右移 `p`，框左边界仍是「文字左 − p」不越出面板；
    `i == id + 1` 补 `p` → 后续候选累计右移 `2p`，与选中框正好相切）。
   水平用 `w` / `squeeze_extra_x`，竖直用 `height` / `squeeze_extra_y`。
   非 squeeze 时 extra 恒 0，官方骨架逐字节不变。

公式（p = `hilite_padding_x/y` = 字高/2 = 0.5em，2026-11-05 18:4x 按 HTML 重算）：
```
pad_normal   = p          (文字 → 橙框外沿 = 0.5em = HTML item 0.25em + core 0.25em)
pad_selected = 1.5p       (0.75em = HTML item 0.5em + core 0.25em)
外框宽       = 文字宽 + 2×pad          (主轴；交叉轴由行/列拉平决定，三种模式都拉满)
框间可见间隙 = 0.5em       (= hilite_spacing；游标推进 = prev_pad + 0.5em + cur_pad)
白核         = 外框按 rim 内缩，rim = pad − 0.5p  (普通 0.25em / 选中 0.5em)
```
> 旧写法 `pad_selected = 2p` 是 p 还是 0.25em 年代的；p 改成 0.5em 后会把橙边画成 1em。
> HTML 的实测值见 `_shots/html-three-modes-measure.json`（headless Edge DOM，
> 单位 em）：选中 4.5×2.5em / 普通 3.0×2.5em / 框间 0.5em / 白核 3.5×1.5 与 2.5×2.0。

单测（`theme/src/layout.rs`，41 全绿）：
`squeeze_flag_inflates_selected_and_pushes_neighbors` /
`non_squeeze_rows_keep_official_padding` /
`all_three_squeeze_modes_light_the_flag`。

### 挤压布局核心公式

```
pad_normal   = hilite_padding        (0.5em = 文字→橙框外沿，theme 可调)
pad_selected = hilite_padding × 1.5  (0.75em)
外框宽 = 文字宽 + 2×pad（主轴）；交叉轴 = 行内最大外框（横流/竹简）或纸宽（飞瀑）
框间 = 0.5em（红块）；白核 rim = pad − 0.5×hilite_padding
```

每候选宽度 = 文本宽(字符格排版) + 2×pad + 序号格(0.5em) + 红块(0.5em)
候选总宽 = Σ(候选宽) + (n-1)×0.5em(红块分隔)
选中候选的宽度膨胀 → 邻居 x 坐标偏移 → "挤压"

### 三模式映射

| LayoutType | 布局 | HTML 对应 |
|---|---|---|
| HorizontalSqueeze | 单行,左→右游标推进 | .row(横流) |
| VerticalSqueeze | 竖列,上→下 | .col(飞瀑) |
| Cells | 单行,每片一条竖列(序号在列首格),内容顶齐 | .cells(竹简) |

> **竹简是一整条竖列**（2026-11-05 两次修正的用户口径）：
> ① 「为什么不是垂直的文字」→ 正文逐字竖排，每字一格；
> ② 「为什么序号不是垂直的」→ 序号不再横排贴在列左边，而是写进**列首那一格**，
>    整列自上而下读作 `1 / 候 / 选 / 0`。
> 实现上 `cells_measured()` 把 label 实测尺寸清零、正文高度算 `(字数+1)` 格；
> 绘制侧 `draw_squeeze` 用首格画序号（裸数字，不走 `label_format`，否则 "1." 会顶出格线）、
> 正文从第 2 格起笔。布局骨架仍未改动。

### 设计元素(逐条对应 HTML)

| 元素 | HTML | D2D 实现 |
|---|---|---|
| 灰壳 | `.ime` #9a9a9f, padding 0.5em, radius 1em | 外层 FillRoundedRectangle — **2026-11-06 最终：padding 0.5em / radius 0.5em；底色 = 按材质档位的 `surface`（见文末三档表）** |
| 白内板 | `.ime-inner` #f5f5f7 | 原内层直角板 — **21:5x 接入半透明材质后删除，只留外层圆角板** |
| 红方块 | `.space` 0.5em×0.5em red | FillRectangle(每 0.5em 一个) — **2026-11-06 已停画** |
| 绿分隔 | `.vsep/.hsep` green + 黑线 2px | FillRectangle×2(绿底 + 黑线) — **2026-11-06 去绿底、改 0.25em** |
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
  ├── 外圈圆角板 FillRoundedRectangle(= 白内板同色，2026-11-06 去掉灰边)
  ├── 白内板 FillRoundedRectangle(#f5f5f7)
  ├── 编码栏:字符格拼音 + vsep + 按钮(🔣✂️🔄) + vsep + 翻页◀▶
  ├── hsep(0.25em 占位 + 居中 2px 线，无绿底)
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

## 📌 2026-11-05 18:5x 追加：外边距 / 编码栏 / 装饰按设计稿对齐（本棒实测）

设计稿以 `_shots/ime-three-modes.html`（444 行）为唯一基准。上面「p / 选中 pad / 白核」那套
不变，本节补的是**纸边、编码栏、行高口径、装饰**四件事。

### 1. 纸边 = 灰壳 padding = 0.5em（只走 squeeze 路径）

- 设计稿：`.ime { padding: .5em }` + `.ime-inner`（白板）无 padding ⇒ 白板到窗口边正好 0.5em，
  候选外框 / 编码栏都**贴着白板边**，没有任何额外外边距。
- D2D：`Metrics::squeeze_frame()` = `max(0.5×font_point, 2.0, 0.3×round_corner_ex)` = 0.5em；
  `squeeze_lip(W)` = 上面这个再夹一次 `min(W/8)`（绘制侧用）。
  布局里 `real_margin_x/y` 在 squeeze 时取 `squeeze_frame()`，**offset 归零**（设计稿不画阴影/描边）。
- 官方六种布局仍用 `real_margin_*` + `offset()`，逐字节不变。

### 2. 编码栏：满纸宽、1.5em 高、内容四边 0.25em

- 设计稿：`.ime-bar { height: 1.5em; padding: .25em; width: 100% }`、`.bar-inner { height: 1em }`，
  拼音左对齐、`| vsep | 🔣 ■ ✂ ■ 🔄 | vsep | ◀ ■ ▶` 整组右对齐；
  `▶` 的右沿 = **白板右沿 − 0.25em**（栏自己的 padding），不是紧贴白板右沿。
- `.hsep`（2026-11-06 起：0.25em 占位 + 2px 线居中，**无绿底**；原为 0.5em 绿底）
  紧接栏底，**中间没有缝**；候选第一只外框的顶沿 = 横分隔下沿。
- 布局：`preedit_row` 就是栏矩形（左右沿 = 白板左右沿），`preedit` 是栏内文字带
  （左沿 = 纸左 + 0.25em、竖直居中）；`height = 栏底 + bar_hsep`，不再加 `spacing`。
- 尺寸参数全部收在 `config::bar`（`HEIGHT_EM/VSEP_W_EM/VSEP_H_EM/BTN_EM/DOT_EM/HSEP_EM/PAD_EM`），
  layout 与 paint 都从 `Metrics` 的像素字段读。

### 3. 行高口径：`max(内容 + 2×自己那条 pad)`

- 设计稿 `.row/.cells { align-items: stretch }`：行高 = 最高的那只**盒子**。
- 竹简里最高的那片通常是**普通**候选（pad 只有 p），所以旧口径
  「最高内容 + 2×最大 pad」会多算 0.5em（HTML 竹简 9.5em，旧算法 10em）。
  实现在 `stretch_squeeze_cross_axis`：`h_need = max(label/text/comment 实测高 + 2×py)`，
  注意必须用实测高，不能用 `row.rect.height()`（那一步已经是整行行高）。

### 4. 重锚：候选组按**外框**贴纸边

- 横流 / 竹简：`min(rect.left − pad_x) = lip`，`width = max(rect.right + pad_x) + lip`。
- 飞瀑：每只外框等宽 `[lip, W − lip]`；内容按各自 pad 从纸左边推开
  （选中项正文比邻居右 0.25em / 低 0.25em，与 HTML 实测一致）。
- 竖向：`min(rect.top − pad_y) = 栏底 + hsep`；`height = max(rect.bottom + pad_y) + lip`。

### 5. 装饰（按设计稿）

| 元素 | 设计稿 | 现在 |
|---|---|---|
| 项内序号↔正文 | `.core > .num + .space + 正文`，0.5em 见方红块 | `draw_squeeze` 画 0.5em 红块（竹简画在竖向缝里、水平居中） |
| 候选之间 | `.row > .space` / `.col > .space` 0.5em 红块 | 横流/竹简在框间画；**飞瀑改为贴纸左边的红方块**（原来是绿横带） |
| 编码栏字符格 | `.ch` 只有宽度，无底色/无格线 | **不画**（`squeeze_grid` 保留为 `#[allow(dead_code)]` 备用） |
| 竹简格线 | `.cell` 无 border | **不画**（`squeeze_grid_v` 同上） |
| 动画 | riseIn 250ms + padding 过渡 | 用户说过「少做动画」⇒ **不搬** |

### 6. 验证

- `cargo test -p weasel-theme-default --offline` = **50 passed / 0 failed**。
- `squeeze_geometry_matches_html_reference` 现在钉到整窗：**19.5em × 5.5em**（1em=20px → 390×110），
  首框左沿 = lip = 10px，末框右沿 = 窗宽 − lip。
- 真窗口（`target/release` 预览，DPI 125%、font_point 15 ⇒ 1em = 25 DIP = 31.25px）：
  `_shots/d2d-flow-1857.png` / `d2d-waterfall-1856.png` / `d2d-cells-1850.png`。
  **注意**：cargo 产物叫 `weasel_theme_default.dll`，预览要的是把它复制成
  `target/release/weasel-next-theme.dll`（直接从自身目录加载），忘了复制会看到旧界面。

  `target/release/weasel-next-theme.dll`（直接从自身目录加载），忘了复制会看到旧界面。

## 实施顺序（2026-11-05 更新）

1. ~~**layout.rs**: squeeze 布局函数~~ ✅ 已完成（外科手术式，见上）
2. **paint.rs**: 挤压绘制(灰壳/白板/红块/绿分隔/字符格/橙底白核/入场动画)——**下一棒**
   - 入口 `Painter::draw_candidates()`，候选框已通过 `layout.highlight_rect()` 自动带
     per-candidate padding；新绘制函数 `draw_squeeze_shell()` 叠在现有底之上即可
   - 设计元素逐条映射见下表；缓动 easeOutExpo 近似 `cubic-bezier(0.16,1,0.3,1)`
3. **theme.yaml 接入** ✅ 配置链路已通（`layout/type` 字符串 + 三个布尔由 Setting 写合法 YAML）
4. **辅助面板**(第二阶段):剪贴板/符号/同步

## 去色变更（2026-11-06，按用户逐条拍板）

- 候选词整块（含 padding）改读 theme 的 `candidate_back_color` / `hilited_candidate_back_color`，
  字统一白色；序号用 DirectWrite `CENTER` 在自身 `label_rect` 内水平居中。
- 编码栏三按钮改中文「同 / 贴 / 符」；按钮与箭头不画圆角底、不加 padding，
  字色取主题候选字色（选中常亮、箭头按 `can_page_previous/next` 高亮）。
- **去掉红点**：`.space` 的 `SQ_RED` 填充共 5 处全删（序号↔正文、候选间、箭头间、按钮间），
  `SQ_RED` 常量删除；0.25em 间隔几何与候选间填 `bg` 的贴合缝保留（不要重新露出白缝）。
- **分隔线改 0.25em + 去底色**：`bar::HSEP_EM` / `VSEP_W_EM` 均 0.5em → 0.25em，
  `bar_group_width()` 由 5em 变 4.5em；hsep / vsep 的 `SQ_GREEN` 绿底整层不画，
  只剩各自占区正中的 2px `SQ_LINE` 线。hsep 高度不再 `.max(3.0)`，与 layout 同数。
- **候选间黑条去掉**：撤销为「贴紧」而把 `seam` 填成候选底色 `bg` 的做法，
  深色主题下缝会连成一整条黑带；现在缝露出面板底色，几何不变。
- **大圆角 / padding**：新增 `Metrics::panel_radius()`，挤压三模式返回
  `round_corner_ex × 0.5`（1em → 0.5em），官方六种布局不变；窗口 `SetWindowRgn`、
  毛玻璃遮罩、灰壳圆角三处同源调用。`squeeze_frame()` 一度改 0.25em，
  2026-11-06 21:2x 用户「都 0.5」后又回到 **0.5em** ⇒ 最终圆角与 padding 同为 0.5em；
  `squeeze_lip()` 与 layout 纸边距自动跟随。
  实机字高 25 DIP：圆角 12.5 / padding 12.5（原值 25 / 12.5）。
- **大界面 padding 去掉颜色**：`SQ_SHELL` 由独立灰 `0xFF9A9A9F` 改为 `SQ_PAPER`
  （`0xFFF5F5F7`），外圈与白内板同色 ⇒ 一整块纯色圆角板，没有灰边。
  0.5em 留白与 0.5em 圆角形状都保留；改成 alpha=0 会露出白板直角、圆角失效，故不采用。
- **整帧按 theme 配色（21:4x）**：新增 `SqueezePalette` / `squeeze_palette(&ColorScheme)`，
  旧写死的 `SQ_SHELL / SQ_PAPER / SQ_INK / SQ_LINE / SQ_GRID / SQ_CELL_BG` 常量全删。
  映射：`paper` = `back_color`（外圈与内容区共用）、`ink` = `text_color`、
  `line` = `border_color` → `comment_text_color` → `ink`、
  候选底 = `candidate_back_color` / `hilited_candidate_back_color`（透明时回落面板底）、
  候选字 = `candidate_text_color` / `hilited_candidate_text_color`。
  每项都带亮度差 >0.30 的可读性校验，深底配深字 / 浅底配浅字时回落中性墨；
  `bar_ink()` 的对比基准也改成动态 `pal.paper`，最后兜底不再是写死的亮橙。
  steam 实测输入 `back=0x141617 / text=0xcd8c52`，输出 `paper=0xFF141617`、
  `ink=0xFFCD8C52`、`line=0xFFA7A7A9`、`cand_bg=0xFF141617`、
  `hilite_bg=0xFF594231`、两组候选字均 `0xFFFFFFFF`（与之前白字视觉一致）。
  单测 `squeeze_palette_follows_theme` / `squeeze_palette_keeps_light_theme_readable`。
- **接入默认 / 玻璃 / 亚克力三档（21:5x）**：此前 `draw_squeeze` 整帧绕过材质三档，
  无论 `%APPDATA%\Rime-Next\theme\material.txt` 写什么，都铺不透明 `back_color`，
  既不画抓屏模糊底图、也不铺亚克力蒙版 ⇒ 三档一个样。
  现在 `SqueezePalette` 增加 `tier` / `surface`，由 `squeeze_palette_for(scheme, tier,
  backdrop_luma, veil)` 按 `tier()` 分档；`Painter::squeeze_pal()` 是唯一入口
  （面板 / 分隔线 / 字符格 / 编码栏按钮全部走它，同帧同口径）：

  | 档位 | `surface`（面板层） | `paper`（算字色的有效背景） | 底层 |
  |---|---|---|---|
  | `default` / `metal` | `0xFF` + `back_color` | 同左 | 无 |
  | `glass` | `0`（抓到背景）/ `0x80FFFFFF`（没抓到） | 模糊桌面平均亮度灰（没抓到按 255 白） | 抓屏高斯模糊 σ=3 |
  | `acrylic` | `theme_veil(scheme)`（实机 `0x801B242F`） | 蒙版按自身 alpha 叠中灰 128 的结果 | 抓屏高斯模糊 σ=8 |

  `draw_squeeze` 在 `tier.flat()` 时先 `draw_backdrop(&shell, ...)` 铺模糊位图，
  再按 `surface` 铺**一层**圆角板；旧版「外圈圆角 + 内缩 lip 直角」两层同色
  在半透明蒙版下会叠成 75% 不透明、中心比外圈实一圈，故删掉内层
  （`paper` 这个 Rect 仍保留给横分隔 band 定位）。
  字色沿用 mod 侧 `plain_scheme` 校正过的 `scheme`（玻璃按真实桌面亮度、
  亚克力按黑白两端最坏情况），`bar_ink()` 对比基准也换成动态 `pal.paper`。
  `Window::capture_backdrop` 的 `mask_rounded` inset 在 squeeze 下归零：
  挤压窗口就是面板、没有 `offset` 留边，否则玻璃档四周会露一圈未模糊的锐边。
  单测 `squeeze_three_material_tiers_produce_three_surfaces` /
  `squeeze_text_stays_readable_in_every_tier` /
  `squeeze_material_file_names_map_to_tiers`。

## 📌 2026-11-08 追加：零候选帧高度（「大写字母候选窗只显示一半」的根因）

零候选 + 有预编辑的帧（打大写字母时的形态）就是「只有编码栏」：**0.5em 纸边 +
1.5em 编码栏 + 0.25em 横分隔 + 0.5em 纸边 = 2.75em**。`font_point 15 / dpi 120`
时 = 68.75 DIP = 86 px。

- **坑**：官方收尾对 `count == 0` **扣两遍 `spacing`**（HorizontalLayout 分支
  `height -= spacing + offset_y` 一次 + 共用收尾 `if count == 0 { height -= spacing }`
  一次），而挤压三模式从没加过 spacing（那个位置是 `bar_hsep`）。结果零候选帧只有
  **1.5em**：dpi120 实测 **47 px**，而编码栏底在 50 DIP(62.5 px) ⇒ 编码栏下半截和
  下横线被窗口底边裁掉。用户 2026-11-08 报「大写字母的候选窗高度不够，只显示了一半」，
  截图 312x117 是**屏幕区域截图**（含窗口上下方应用背景），不是窗口尺寸 —— 别用截图
  尺寸反推窗口。
- **修法**（`theme/src/layout.rs` 收尾段）：`if m.squeeze { height =
  preedit_row.bottom + m.bar_hsep } else { height -= m.spacing }`，随后统一
  `height += real_margin_y`（底部纸边）。修复后 dpi96 = 58 px、dpi120 = 86 px。
- **判据**：`preedit_row.bottom + bar_hsep + squeeze_frame() ≤ content_size.cy`；
  窗口 px = `content_size × dpi/96`（preview 与 live **同源**，`theme/src/mod.rs`
  `pixels()`），以后布局尺寸可以直接用 `--preview` 量。
- 回归：`theme::layout::tests::squeeze_zero_candidate_frame_fits_whole_bar`（正负对照）。
