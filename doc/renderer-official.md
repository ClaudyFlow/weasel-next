# official 渲染器（候选窗）—— 与官方 WeaselUI 逐项对齐

这份文档说清 `theme\official\` 这个主题 DLL 是什么、它怎么读配置、以及"和官方一样"是靠哪些
规则保证的。参照实现是 `C:\Code\weasel\WeaselUI`（fxliang/weasel 的 C++ 版），行号都指那份源码。

## 一、它在哪儿、谁加载它

```
C:\Program Files\Weasel-Next\
  Weasel-Next-Renderer.exe            渲染器宿主：进程、消息循环、设置下发
  weasel_theme_official.dll           本主题（渲染器从**自身目录**加载；也兼容 themes\ 子目录）
  Rime-Next\theme\default\default.theme.toml   出厂皮肤（36 套配色）
%APPDATA%\Rime-Next\theme\theme.toml  用户布局 + `color_scheme`（用哪套皮肤；可选）
```

渲染器按 `.theme` 找 `weasel_theme_<名字>.dll`；主题的 `create()` 走 UI 线程，窗口与图形资源
都由创建它们的线程管理（官方 `WeaselPanel` 同构）。

## 二、模块分工（`theme\official\src\`）

| 文件 | 对应官方 | 职责 |
| --- | --- | --- |
| `config.rs` | `Layout.cpp:24-60` + `RimeWithWeasel.cpp:1160-1341` | 官方 style 默认值、别名、夹取、DPI 缩放、配色解析与默认链 |
| `layout.rs` | `StandardLayout.cpp` + `VerticalLayout.cpp` + `HorizontalLayout.cpp` | 量文本 → 算矩形（含圆角信息） |
| `paint.rs` | `WeaselPanel.cpp:423-797` + `d2d.cpp:828-978` | `DoPaint` / `_HighlightRect` / `_DrawCandidates` / `_TextOut` |
| `mod.rs` | `WeaselPanel.cpp` 其余部分 | 窗口样式、消息、定位、手势、主题接口 |
| `presenter.rs` | `d2d.cpp:155-311` | D3D11 + DXGI 组合交换链 + DirectComposition + D2D 设备上下文 |

## 三、"和官方一样"靠这六条

1. **出厂默认值写在代码里**（`UIStyle::default` / `Layout::default` = 官方 `weasel.toml`
   的 `style:` 段）。出厂侧不再分发 `theme.toml`，所以"没有配置文件"也必须画出官方观感。
2. **颜色语义照官方**（`style\src\color.rs`）：
   * 位数决定 alpha —— `0x000000` 是**不透明黑**，`0x00000000` 是**全透明**；
   * 通道顺序由 `color_format` 决定，**默认 `abgr`**（文本是 `AABBGGRR`）。
     aqua 的高亮 `0xfa3a0a` 因此是**蓝色** `#0a3afa`，不是橙红 —— 这条最容易搞反。
   * 缺项按官方 fallback 链补齐（`candidate_text ← text`、`border ← text`、
     `label ← blend(candidate_text, candidate_back)` 等），内部统一存 ARGB。
3. **读完配置还要夹取**（官方 1295-1341）：`spacing ≥ padding_y*2`、
   `candidate_spacing ≥ padding*2`、`hilite_spacing ≥ padding_x`、
   `margin = sign * max(padding, |margin|)`；再**统一乘 DPI**（`dpi/96`；字号另乘 `dpi/72`）。
4. **布局逐句移植**：`real_margin = max(|margin|, padding)`、`offset = border*2`（有阴影时
   再加阴影外扩）、一候选一行、行距 `candidate_spacing`、注释列对齐到同一列、候选右边界贴
   面板右内边、`contentSize = (width + offset*2, height + offset*2)`、最后 `content` 再
   deflate 掉 offset。**手算对照测试**在 `layout.rs::matches_hand_computed_official_geometry`。
5. **绘制三层**：阴影（离屏位图 + 高斯模糊，标准差 = `dpi * shadow_radius`）/ 底色（逐角圆角
   路径填充）/ 描边（同一路径 `DrawGeometry`，线宽 = `border`，半径内缩 `border/2`）；
   文字用 `CreateTextLayout` 并把 `GetOverhangMetrics` 的正向溢出补进偏移。
6. **窗口与定位**：`WS_POPUP|WS_CLIPSIBLINGS` + `WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE|
   WS_EX_TOPMOST|WS_EX_NOREDIRECTIONBITMAP`；逐像素透明交给 DWM（**没有** `WS_EX_LAYERED`、
   没有 DWM 模糊、没有 `SetWindowRgn`）。定位照 `_Reposition`：插入点下方 6 像素，放不下翻到
   上方并 sticky，最后夹进工作区（`reposition()` 有纯函数测试）。

## 四、怎么验证

```powershell
# 编译
$env:LIBCLANG_PATH='C:\Program Files\llvm-mingw\bin'; cargo build --release
# 单元测试（颜色语义、默认值/夹取、布局手算值、定位数学）
cargo test --release -p weasel-style -p weasel-theme-official

# 打包 + 安装（安装要提权；装完只有程序文件夹是运行目录）
powershell -File script\build-installer.ps1
C:\WINDOWS\system32\sudo.exe cmd /c "artifact\installer\Weasel-Next-0.18.0-x64-setup.exe /S"
```

出问题时看这几份日志：

```
%LOCALAPPDATA%\Rime-Next\Logs\theme.log        主题侧（含"主题配置:"与"布局:"几何诊断）
%LOCALAPPDATA%\Rime-Next\Logs\renderer.<日期>.log   渲染器侧（capabilities / 主题选择）
```

对照官方截图用的脚本（都在 `script\`）：

| 脚本 | 用途 |
| --- | --- |
| `capture-window.ps1` | 按窗口类名/进程抓图（截图进程会先设 DPI 感知，否则抓错区域） |
| `measure-candidate.py` | 量面板/高亮块尺寸与四边内缩、行带 |
| `ink-bands.py` | 量文字墨迹行高/行距（可把缩放过的截图折算回实际像素） |
| `compare-candidates.py` | 官方与我们的候选窗并排成一张图 |
| `capture-official-preview.ps1` | 打开官方设置对话框抓它的候选窗预览 |

## 五、已知偏差（都还没做，按影响排序）

1. **竖排文字**（`vertical_text = true`）：`VHorizontalLayout.cpp` 尚未移植，目前回落成竖排
   候选列表。
2. **全屏布局**（`fullscreen = true`）：`FullScreenLayout` 的字体自适应缩放与居中未移植。
3. **字体回退链**：官方用占位字体名 + `IDWriteFontFallbackBuilder` 构造回退表（支持
   `font_face: "字体A,字体B:区间"` 与 `:bold`/`:italic` 标记）；我们直接用字体名建格式，
   缺字形时走 DWrite 的系统回退。默认配置（Microsoft YaHei）观感一致。
4. **状态图标**：官方画资源里的 HICON；我们画"中/英"方块（数据文件夹里的
   `Rime-Next\icon\*.ico` 还没用起来）。
5. **`hover_type = hilite`**：官方悬停即切换选中项，主题接口没有"改选中"这个动作，目前按
   `semi_hilite` 处理（只画半透明块）。
6. **`click_to_capture`**（点候选窗抓图进剪贴板）未实现。
7. **`vertical_auto_reverse`**（翻到上方后从下往上重排候选）未实现。
8. **模式提示窗**是我们自己的面（官方没有这个窗口，它用状态图标），沿用配色与圆角绘制。
9. **序号来源**：快照不带 rime 的 labels，序号按 `page_start + i + 1` 合成再套
   `style/label_format`（默认 `%s.`），与官方默认观感一致。
