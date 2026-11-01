# YUME / yumete 学习笔记（可抄清单）

> 2026-10-22 读 `C:\Code\yumete`（宇夢編輯器开源前端）。**未完成**：灵妙整句交互那部分在另一路深读中。
> 先读这份再看代码，能省两个小时。

## 0. 最重要的一条：引擎源码不在

`yumete` 只是**前端**，它的输入法层依赖同级另一个仓库：

```toml
# crates/yumete-ime/Cargo.toml
yume-core = { path = "../../../yume/crates/yume-core" }
```

本地 `C:\Code\yume` 不存在 ⇒ **这份连编译都编不了**。网上只有
[forfudan/yume-release](https://github.com/forfudan/yume-release/releases)（发行包）与
[forfudan/yuhao-ime-release](https://github.com/forfudan/yuhao-ime-release)，
**YUME 引擎本体没有公开源码仓库**。

⇒ 能学的只有「前端工程约束 + 数据格式」；**解码器、四种模式（顶字/自动/整句/顶功）的真实行为、码表内存结构，一概学不到**，别在这上面耗时间。

## 1. 这 3,890 行长什么样

`crates/yumete-ime/`（`lib.rs` 2,735 行，其中 615 行是测试；生产代码约 2,120 行）：

- `ImeSession` 只持有 `Engine` + 一批前端状态（方案、可用性、engaged、panel_room），**没有 buffer/候选字段**（`lib.rs:451-508`）。
- 一次按键：`tui:4171 ime_handle` → 六个单行转发（`lib.rs:896-924`）→ `Engine::*`（外部）→ 回读
  `display_mode/page_candidates/display_buffer` → `take_committed`（`lib.rs:1089`）→ 编辑器插入。
- **本层没有任何"编码 → 候选"的查表函数**，只有取当前页 `page_candidates`（1135-1152）和取高亮 `top_candidate`（1409-1417）。

## 2. 三条容易误读的地方（我踩过）

| 看着像 | 实际 |
|---|---|
| `segment.rs` 是输入码切分 | ❌ 切的是**已上屏的汉字**（给编辑器按词移动光标 / 分词着色），`segment.rs:1-19` |
| 「空格键分割」在这层实现 | ❌ 这层没有 split；`ImeSession::space()` 只是**上屏**（`lib.rs:900-903`） |
| 四种模式在这层分派 | ❌ 顶字/自动/整句 = `CommitStrategy` 一个枚举的三个值；顶功是 `code_space`，另一回事 |

## 3. 值得抄的（都落到我们文件上）

### 3.1 数据故障分类：把"版本不匹配"和"文件不存在"分开

他踩过的坑写在注释里（`lib.rs:1656-1658`，一字不改地值得看）：

> `.ydiv` 变了 magic（2026-09-04）—— 老数据目录**每个文件都还在原地、却悄悄不再应答**。
> 编辑器上看不出任何问题；只是拆分注解不再出现了。

他的答案（`lib.rs:1659-1678`）：

```rust
pub enum DataFault {
    Missing,                                   // 不在搜索目录里：可选数据的常态，不必打扰人
    Unreadable(String),                        // 在，但操作系统不给
    Rejected { reason: String, magic: Option<MagicMismatch> }, // 在且能读，但引擎不收
    UnknownKind,                               // 这个 kind 本版本没编译进来：按设计跳过，只记一笔
}
```

关键两条：
- `is_loud()` 只让 `Rejected` / `UnknownKind` 出声（1707-1712）—— **缺失是常态，别报警**。
- `magic` 只读**文件头**比对（1847-1865），报的是"版本不匹配"而不是"症状"。

**抄到哪**：`common/src/deploy_manifest.rs:43-65` + `Engine/src/librime/loader.rs:32-83`，部署前预检。
代价：分类**小**，各格式 magic **中**。

> 这类故障我们这边一模一样：部署日志假报"全部与本次构建一致"、32 位 DLL 被 robocopy
> ERROR 32 静默跳过、`rime.lua` 根部副本遮掉 plugin 那份。全都属于"文件在、但不生效"。

### 3.2 「请求值」与「有效值」分开

`commit_override`（`lib.rs:1332-1334`）与 `commit_strategy`（`lib.rs:1317-1326`）是两个 API：
前者是用户选了什么，后者是**此刻真正生效的**（拼音会强制整句，但用户的选择仍答得出）。
**抄到哪**：`common/src/message.rs:16-55`（`ascii_mode: Option<bool>` 已经是这个形状）。代价小。**只抄 API 分层，别抄行为**。

### 3.3 可用性阶梯：别用一个 bool 决定要不要吞键

`available`（855-857）/ `engaged`（828-830）/ `holding_the_keys`（841-843），
外加 installed→builtin→unavailable 的 `stand_in`（1586-1592、1608-1624）。
**抄到哪**：`Engine/src/librime/loader.rs:32-83`（首次部署/部署失败路径）。代价小-中。

### 3.4 可见性闸：上屏了却没画出来

`panel_is_full`（1397-1402）、`inline_text`（1429-1437）、`panel_shows_the_code`（1443-1445）、
`panel_room`（1365-1373）、`page_has`（999-1001）；数字键在 TUI 过**两道闸**（`tui:4245-4252`）。
**抄到哪**：`common/src/message.rs:28-42`（`page_start/candidates/can_page_*`）+ Renderer 的"这一帧画得下吗"。代价小。

### 3.5 单一上屏出口

`take_committed` 取走并清空（`lib.rs:1089-1091`），每个键处理完**统一 drain**（`tui:4337-4340`）。
我们 `Engine/src/engine.rs:2584-2590` 已有同一形状，只需保证每 response 只 drain 一次。代价小。

## 4. 别抄的

1. **进程级 `OnceLock` + `Box::leak` 注册表**（`lib.rs:71,102,324,380`；二次 discover 直接忽略 288-290）。
   测试只能拆成独立二进制（`tests/found_schemes.rs:3-8`）。我们是多客户端 + 可热部署 + 可切方案，
   必须实例状态（已有 per-client `ClientSession`，`engine.rs:122-153`）。
2. **构建期联网下载 + `include_bytes!` 内嵌数据**（`build.rs:224-273` 缺文件就 curl 抓 GitHub release；
   `62-65` 直接嵌进二进制，连 3.2 MB 拆分表也嵌）。构建不可复现、离线失败、供应链风险。
3. **2,700 行单文件 + 直接摸引擎字段**（`engine.page_size` 1351、`engine.unigram` 1291、`engine.lexicon` 1292）。
   这使它只能配 `yume-core`；换成 librime 是重写不是移植。我们应保持 `RimeSession` 不透明，策略放独立模块。

## 5. 一条对我们可能有用的数据格式

磁盘码表是**前缀压缩的紧凑行**（`lib.rs:409-414`）：

```
sharedLen: u8 | suffixLen: u8 | suffix | textLen: u8 | text
```

即"和上一条共享几个字节 + 差异后缀 + 文本"，有序键；超 255 字节的行**在交给引擎之前**就剔除
（432-448、558-565，注释说是 #344：先验再交，不是后发现）。`load_text` 还会**自动识别 RIME YAML**（542-548）。

另：`code_space`（`lib.rs:2102-2110`）是「段界的裁判」——由最大码长 + 自分段字母推导，
**码表编译时学到的答案优先**。也就是说顶功的"段界"是**预计算数据**，不是运行时推断。
（若哪天要在 Rime 上做顶功，这是它唯一的可抄点：把段界编译进方案，而不是临时算。）

## 6. 待补

- [x] 灵妙整句的交互实现 —— **结论：本地这份根本没有**。全仓 `灵妙`/`靈妙` **0 命中**；
      组字中 `Tab` 只是查字典（`crates/yumete-tui/src/lib.rs:4321-4327`），没有格子模型；
      `v`/`u` 只是编码字母（同文件 `:4334`），`x` 没有子模式；线索里的 `segment.rs` 是 `w/b/e` 词切分、
      `chrome.rs` 是浮框几何。⇒ 这套交互在 **YUME 自己的前端/引擎**里，开源部分没有。
      **要抄只能照官网规格自己设计**（官网把键位、规则、甚至"为什么用 v/u 不用 ,/."都写了：够设计，不够抄实现）。
- [ ] 已确认的硬约束：librime 的 C API **没有 segmentation / spelling 接口**
      （核过 `target/.../rime_bindings.rs`，只有 `get_context/get_commit/get_input/...`），
      所以 `v`/`u` 拨切分**在 Rime 上做不了**，除非自己实现解码器、绕开 librime。
- [x] 我们 Engine 里**一行整句逻辑都没有**（grep `sentence` 无命中），整句完全靠方案（`tiger_sentence`）。

## 7. 灵妙整句：在 librime 上重做要多少代价（2026-10-22 评估）

**结论：这不是「移植」，是「在 librime 上重做」。** 原型 **25–42 人日**；产品化（异常/跨方案/测试/IME 兼容）**45–70 人日**。
按熟悉 librime 的一个人、20 人日/月算 ≈ **1–2 人月原型，2–3.5 人月产品**。

### 7.1 必须新增

| # | 内容 | 人日 | 说明 |
|---|---|---|---|
| 1 | 影子格子模型 `{code_range, text, candidates, selected}` + 进/出/上屏/撤销生命周期 | 4–6 | **无参照实现**（yumete 里没有） |
| 2 | 每格候选重算（固定左右邻居、排除当前词） | **8–15** | **最大不确定性**。公开 API 只能整串出候选；`set_input` 重放一格会改变整句切分 |
| 3 | 边界缩/增（拼音按音节、形码按码元） | 5–8 | librime 内部有 `SyllableGraph`/`Spell`，公开 API 不给；要自建边界表 + 重切后偏移 |
| 4 | 键位与 UI（a/b/c 标签、数字替换、v/u/x、格子覆盖层） | 5–8 | 状态机可搬，ratatui 浮框不能搬；加鼠标点格 +3–5 |
| 5 | 上屏/撤销与组字串同步 | 3–5 | 一次上屏一条 undo，格内多次改动要合并 |

### 7.2 唯一省力路径：写 **C++ rime 模块**，不走 C ABI

librime 内部结构与「格子」几乎一一对应：

- `Composition : Segmentation`（`C:\Code\librime\src\rime\composition.h:21`）
- `Segment{status, start, end, length, menu, selected_index}`（`src\rime\segmentation.h:19-31`）
- `AddSegment / Trim / Close / Reopen`（`segmentation.h:47-48`、`66-69`）
- `Menu`（`src\rime\menu.h:27-36`）
- `ScriptTranslator::Query(input, segment)`（`src\rime\gear\script_translator.h:33-34`）

代价：这些是 **C++ 内部类型、非 ABI、跨版本无保证** ⇒ 只能模块化 + 锁死 librime 版本（而我们已经因为 `resource.cc` 残缺恢复版而**不敢重编 librime**，见检查点坑 2）。

### 7.3 我们现在缺的绑定

`Engine\src\librime\api.rs:41-62` 只到 `get_commit` / `select_candidate_on_current_page`，
连 `set_input`（`rime_api.h:503`）和 `get_candidate_preview`（`:520`）都没接。
**影子模型的第一步就是补这两个绑定**。

### 7.4 明确放弃

- **已上屏句子的格子编辑**：两端都只有纯文本（`yumete-core/src/buffer.rs:97-105`；`RimeCommit` 只有 `text`，`rime_api.h:146-150`）。要做就得给文档模型加"字→码"，那是另一个项目。
- **把内部 `Segment`/`Composition` 当稳定 API**：见 7.2。
- **>26 格字母标签**：无既有实现，也没必要。26 封顶或直接改用数字。

### 7.5 建议（如果真要做）

1. **先补 `set_input` + `get_candidate_preview` 两个绑定（1–2 人日）**，做个原型验证"整串重设能不能骗过切分"。
   **这一步无论最后走哪条路都要做**，而且它直接回答第 2 项那个最大不确定性。
2. 若原型证明"整串重设"可用 → 走**影子模型**路线（不改 librime），只做「跳格 + 换词 + 上屏」，**放弃 v/u 拨切分**，
   约 **8–12 人日**，风险低。
3. 若必须要有 v/u 对 → 只能写 C++ 模块，且**要重新面对"敢不敢重编 librime"**（我们现在是不敢的）。
