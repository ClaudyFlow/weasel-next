# 虎符 HuFu（hufu-ime-rust）皮肤 / 玻璃化 研究报告

> 研究对象：`https://github.com/LeafHW/hufu-ime-rust.git`（clone 于 `/c/Code/hufu-ime-rust`）
> 关注点：① 玻璃化（毛玻璃）效果怎么做的；② 如何兼容原有 weasel/squirrel 配色
> 结论先行：**两项能力都有完整的代码基础设施**；但玻璃化渲染链在 2026-09-11 被「退役」过一轮，
> 当前（本仓库 HEAD）`Material::Glass/Frosted` 实际上按 `translucent` 渲染、DWM accent 只用来「清除残留」。
> 所以 README 的「毛玻璃」宣传略领先于当前代码——基础设施在，开关没全打开。

---

## 0. 架构位置（皮肤相关模块）

```
engine/crates/hufu-skin/   皮肤模型（Rust 结构体）+ weasel/squirrel 互导   ← 配色兼容在这
engine/crates/hufu-server/ 常驻 daemon：加载 Skin JSON，经命名管道 IPC 下发给前端
platform/windows/hufu-tsf/  Windows TSF 前端（DLL），候选窗渲染在这        ← 玻璃化在这
   └─ src/candwin2.rs        候选窗 v2：D3D11 + DirectComposition + D2D + DWM 材质
   └─ src/candwin.rs         候选窗 v1：GDI UpdateLayeredWindow（兜底/预览）
```

数据流：`hufu-skin::Skin::load(json)` → `hufu-server` 读皮肤 → IPC → `candwin2.rs` 按 `material.kind` 决定材质。
weasel 配色导入发生在 `hufu-server/src/main.rs:1790`（`from_weasel_colors`）。

---

## 1. 配色兼容（hufu-skin，值得直接借鉴）

### 1.1 颜色角色与 weasel 一一对应

`hufu-skin/src/lib.rs` 的 `Colors` 结构体（lib.rs:189）共 21 个角色，**字段名与 weasel `preset_color_schemes` 完全对齐**：

```
back_color  border_color  text_color  preedit_back_color
candidate_text_color  candidate_back_color  candidate_shadow_color
comment_text_color  label_color
hilited_text_color  hilited_back_color
hilited_candidate_text_color  hilited_candidate_back_color
hilited_candidate_label_color  hilited_comment_text_color
hilited_label_color  hilited_mark_color
shadow_color  hilited_shadow_color  hilited_candidate_shadow_color
+ capsule_text_color / capsule_back_color（macOS 状态胶囊扩展）
```

### 1.2 双向互导

- **导入** `Skin::from_weasel_colors(id, scheme)`（lib.rs:535）：直接吃 weasel `preset_color_schemes/<id>` 的 JSON 片段，
  逐字段 `set(field, &mut colors.xxx)` 映射。还做了两件事：
  - 兼容 weasel 社区里 `"0xC0303030"` 这种**字符串形式**的 0x 色值（旧实现 `as_u64()` 返回 None 会整色静默丢弃）；
  - 按背景亮度算 `lum = 0.299R+0.587G+0.114B` 自动判 `dark`，并把 `material.tint = back_color`。
- **导出** `Skin::to_weasel_patch()`（lib.rs:589）：把 `Colors` 转回 weasel 的 `preset_color_schemes` patch 片段，
  写回小狼毫用。有单元测试 `weasel_import`（lib.rs:647）验证往返。

### 1.3 颜色字节序坑（最该记的一笔）

weasel 的颜色整数是 **`0xAABBGGRR`**（BGR 字节序，alpha 在高位），不是 RGB。

- `Color` 反序列化（lib.rs:64）收到整数时：`a = (v>>24)==0 ? 0xFF : (v>>24)`，`[v as u8, (v>>8) as u8, (v>>16) as u8, a]` —— 即按 BGR 拆。
- 字符串 `#RRGGBBAA` 与整数双格式都兼容（`color_serde`，lib.rs:19）。
- 导出时 `conv()`（lib.rs:591）再拼回 `0xAABBGGRR`。

> **对 Weasel-Next 的意义**：如果要做 weasel 配色互导/预览，这个 BGR 字节序 + alpha 高位约定是唯一的硬坑，
> HuFu 已经趟平，可直接照搬 `Color` 的 serde 实现。

---

## 2. 玻璃化（Windows，重点研究）

### 2.1 材质模型

`Material` 枚举（lib.rs:78），语义注释写明平台落点：

```rust
enum Material {
    Solid,       // 不透明纯色
    Translucent, // 半透明（自定义不透明度）
    Frosted,     // 毛玻璃磨砂（Acrylic / vibrancy behindWindow）
    Glass,       // 玻璃（Mica / underWindowBackground + 细边框高光）
}
```

`MaterialConfig`（lib.rs:91）是关键参数的**单一真相源**：`tint` / `opacity` / `darken` / `glass_alpha` /
`glass_shadow_alpha` / `glass_shadow_size` / `glass_margin_x/y`（玻璃独立留白，与普通皮肤留白解耦）等。
所有玻璃参数都有详细「版本+用户实测定稿」注释（v3.5 / v3.9 等），说明这套是长期调出来的。

### 2.2 窗口与合成链路（candwin2.rs）

候选窗 v2 的材质由 DWM accent 承担（模块头注释 lib.rs 引自 candwin2.rs:4）：

```
材质 → SetWindowCompositionAttribute accent：
  solid      = 不透明
  translucent= 半透明渐变
  frosted    = Acrylic 磨砂
  glass      = HostBackdrop 玻璃（Win11 22H2+）
```

**窗口创建**（candwin2.rs:2380）：
```rust
WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP
// + WS_POPUP
```
`WS_EX_NOREDIRECTIONBITMAP` 是 DComp 直通窗的关键：窗口不走传统 GDI 重定向位图，逐像素 alpha 由 DWM 合成。

**图形链路**（candwin2.rs:2405-2439）：
```
D3D11CreateDevice(BGRA_SUPPORT)        // 硬件 → WARP 兜底
  → IDXGIDevice
  → D2D1CreateFactory → ID2D1Device → ID2D1DeviceContext   // Direct2D 画文字/圆角
  → DCompositionCreateDevice → CreateTargetForHwnd(hwnd) → CreateVisual
  → CreateSwapChainForComposition                                    // 合成交换链
```

**DWM 未公开 API**（candwin2.rs:1375-1418）—— 这是「真系统毛玻璃」的核心：
```rust
const WCA_ACCENT_POLICY: u32 = 19;
struct AccentPolicy { accent_state, accent_flags, gradient_color: 0xAABBGGRR, animation_id }
#[link(name = "user32")]
extern "system" { fn SetWindowCompositionAttribute(hwnd, *WinCompAttrData) -> BOOL; }

fn apply_accent(hwnd, state, tint) { ... SetWindowCompositionAttribute(hwnd, &data); }
```
`accent_state` 取 `ACCENT_ENABLE_ACRYLICBLURBEHIND` 时，**DWM 合成器直接给窗口底下做系统级真毛玻璃——零抓屏、零模糊算法**
（注释 candwin2.rs:2332，明确参照 window-vibrancy / TranslucentTB 成熟方案）。
染色由 `gradient_color`（0xAABBGGRR）控制；`glass` 对应 `HostBackdrop`（Win11 22H2+ Mica）。

**圆角**：DirectComposition 交换链上 D2D `PushLayer` 几何 mask 实测不生效，改为 **CPU 后处理**——
`present_ulw` / 测试回读时对四角像素做透明化（预乘直清 + 1px 抗锯齿）（candwin2.rs:4548-4553）。

**兜底**：D3D/DComp 初始化失败 → 上层回退 v1 GDI `UpdateLayeredWindow` 分层窗（逐像素 alpha + 软投影 + 圆角）。

### 2.3 当前实际状态（重要，务必知道）

代码里有**多处「毛玻璃退役 2026-09-11」注释**，现状是：

- `candwin2.rs:3583`：材质 `glass/毛玻璃` 整链已删，`kind` 值不再分支，旧 glass 皮肤按 `translucent` 渲染；
- `candwin2.rs:3589`：`apply_accent` 现在**只调用 `ACCENT_DISABLED`** 清残留（热切换过毛玻璃的窗恢复普通合成）；
- `candwin2.rs:4566`：自绘毛玻璃（抓屏 + D2D 高斯模糊）路径整块移除。

也就是说：**模型层 `Material::Frosted/Glass` 仍在、DWM `apply_accent` 管线仍在，但渲染侧开关没打开**——
当前构建里玻璃/毛玻璃会退化成半透明。README 的「毛玻璃等材质」宣传领先于代码。这是该项目的「在 flux」特性，
报告时不能当成「现在就能开真毛玻璃」。

---

## 3. 对 Weasel-Next 的借鉴清单

1. **weasel 配色 1:1 互导**：`hufu-skin` 的 `Colors` 字段集 + `Color` 的 `0xAABBGGRR` BGR 字节序 serde +
   `from_weasel_colors` / `to_weasel_patch` 是现成的模板。Weasel-Next 若要做「导入小狼毫主题 / 导出回小狼毫」，
   直接复用这套字段映射即可，不必重新设计。

2. **真·Windows 玻璃化的最简路径**：Weasel-Next 当前 Windows 前端是 Weasel 的 C++ UI（GDI/Direct2D）。
   要加真毛玻璃，**最省事的不是重写 DComp 交换链**，而是：保留现有候选窗 HWND，直接调用
   `SetWindowCompositionAttribute(hwnd, ACCENT_ENABLE_ACRYLICBLURBEHIND)`——accent 对任意顶层 HWND 都生效，
   与是否用重定向位图无关。HuFu 因为还要画 D2D 圆角/高亮才上了完整 DComp 链路；纯「要毛玻璃背景」用 accent 一条调用就够。
   （window-vibrancy / TranslucentTB 都是这个思路。）

3. **皮肤模型与渲染解耦**：`hufu-skin`（纯数据，平台无关）与 `platform/*/candwin*.rs`（平台渲染）分离。
   Weasel-Next 若引入 JSON 皮肤，建议同样把「材质/配色/布局」建模成独立 crate，前端只消费。

4. **玻璃参数独立集**：HuFu 把 `glass_margin_*`、`glass_shadow_*`、`glass_alpha` 与普通皮肤的
   `margin_*`/`shadow_*` 完全解耦（两套独立、不联动），避免「毛玻璃放大时留白不跟着变」这类坑。
   设计玻璃主题时这点是经验。

---

## 4. 关键文件索引

| 文件 | 内容 |
|---|---|
| `engine/crates/hufu-skin/src/lib.rs` | `Skin` / `Colors`(21角色) / `Material` / `MaterialConfig`；`from_weasel_colors` / `to_weasel_patch`；`Color` BGR serde |
| `engine/crates/hufu-skin/src/ssf/` | .ssf（搜狗/虎娘）皮肤容器解析（QPYS/QQ 皮肤模块已摘除留档） |
| `engine/crates/hufu-server/src/main.rs:1790` | weasel 配色导入调用点 |
| `engine/crates/hufu-server/src/candwin.rs` | v1 GDI 分层窗（规格 1:1 复刻 candwin2，用于预览） |
| `platform/windows/hufu-tsf/src/candwin2.rs` | v2 候选窗：D3D11+DirectComposition+D2D+DWM；`apply_accent`（candwin2.rs:1398）；材质分支（candwin2.rs:3583） |

> 注：本报告基于 HEAD（2026-10-28 拉取的 main 浅克隆）。玻璃化相关代码注释密集且带大量「N 修」修订记录，
> 属快速演进特性，具体行为以实机构建为准。
