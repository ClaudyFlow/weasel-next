# reference/ —— 官方渲染器源码（只读参考，**不参与编译**）

来源：`C:\Code\weasel`（官方 0.17.4 源码树），`WeaselUI\*.cpp|*.h` 与 `include\` 里它依赖的几个头文件。

用途：`theme\official`（我们的 Rust 主题）要**逐句对齐**官方实现；把这些源码放在仓库里，
      移植时可直接对照，不必再去翻外部目录（也避免"读错文件"这类事故 —— 今天发生过 ✓）。

对应关系：

| 官方 | 作用 | 我们 |
|---|---|---|
| `WeaselPanel.cpp` `DoPaint` L423-509 / `_HighlightRect` L799-823 | 绘制总流程与圆角块 | `theme\official\src\mod.rs` 的 `paint()` |
| `WeaselPanel.cpp` `Create` L349-391 | 建窗与扩展样式（要点：`WS_EX_NOREDIRECTIONBITMAP`，**不要** `WS_EX_LAYERED`） | `mod.rs` 的 `CreateWindowExW` |
| `VerticalLayout.cpp` `DoLayout` L5-210 | 竖排布局（我们用的就是这套） | `logic.rs` 的 `Layout::new` |
| `Layout.cpp` 构造 L24-60 | `real_margin` / `offset` / DPI 缩放 | 同上（待接 TOML 的 Metrics） |
| `StandardLayout.cpp` | 尺寸测量、mark、页码 | `logic.rs` |
| `d2d.cpp` / `d2d.h` | D3D/D2D/DComp 设备与绘制原语 | `presenter.rs` |

算法摘要见 `doc\official-layout-reference.md`（根公式、DoLayout 逐句映射、绘制清单、移植骨架）。