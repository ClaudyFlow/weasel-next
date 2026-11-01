# 备忘录

> 记录待办与已决定的设计；**标注「暂时不做」的项不要动**。

## 程序文件夹（`C:\Program Files\weasel-next\`）

程序文件夹里需要额外放两个工具，**一个负责下载、一个负责解压**：

| 文件 | 角色 | 备注 |
|---|---|---|
| `aria2-next.exe` | **下载**压缩包 | 名字**必须是 `aria2-next`**（不是 `aria2c`） |
| `7z.exe` | **解压**压缩包 | 7-Zip 命令行版 |

- 用途：从网络下载方案/词库/细胞词库压缩包并解压到用户文件夹。
- 状态：**暂时不做**（待办，未实现）。

## 用户文件夹分类（`%APPDATA%\Rime-Next\`）

```
theme/<名字>/<名字>.theme.yaml      主题        （YAML，我们自己解析，不转换）
schema/<名字>/<名字>.schema.yaml    方案        （部署时转成 <名字>.schema.yaml 给 librime）
dict/<名字>/<名字>.dict.yaml        词典        （2026-10-28 起词库只在 dict/，身份是头里的 id:）
eassy/<名字>/<名字>.eassy.yaml      细胞词库    （按词归并求和 → essay.txt / <名字>.txt）
model/<名字>.gram                   语言模型    （直接放好，librime 原生）
```

- 五个文件夹（`theme` / `schema` / `dict` / `eassy` / `model`）必须存在，程序启动时自动创建。
- `dict/` 只放词库（`*.dict.yaml`）、`schema/` 只放方案（`*.schema.yaml`）；身份一律用文件头里的
  `id:`（`name:` 是显示名，允许中文，**不能**拿来拼文件名）。词库的目录只要求是 `dict/` +
  id 分段的任意前缀（`dict/cangjie/5/` 合格），不必为片段再建子文件夹；方案仍严格「目录 = id 全部分段」。
- librime **只吃 YAML/txt/gram**，所以 TOML 必须在**部署时转换**；`*.schema.yaml` / `*.dict.yaml` 本身**不能改成 TOML**。
- essay 只影响**打分/调序**（`Grammar::Evaluate`），**不产生候选**；`vocabulary` 默认名就是 `essay`。
- 支持通过 **plum（東風破）** 下载：包 = git 仓库 + `recipe.yaml`，安装动作把文件复制到上面这些子目录。
- 状态：**待做**。

## 已经做完并验证的

- 全量改名：`Weasel-RS` → `Weasel-Next`、`weasel-rs` → `weasel-next`、`weaselrs` → `weaselnext`、`小狼毫RS` → `小狼毫Next`。
- 代码目录改名：`C:\code\weasel-rs` → `C:\code\Weasel-Next`（TIP 注册表已同步指向新路径）。
- 用户目录：`%APPDATA%\Rime-Next\`（大写保留）、日志 `%LOCALAPPDATA%\Rime-Next\Logs\`；`lua\` → `plugin\`。
- `sound` 移出用户目录：声音用**程序文件夹**的 `Rime-Next\sound\*.wav`。
- librime 补丁两个（已归档进 `librime/librime-local-patches.patch`，并在 `C:\code\librime` 提交）：
  1. `level_db.cc` 的 `weasel_userdb_path()`：所有 `*.userdb` 统一进 `<用户目录>/userdb/`；
  2. `resource.cc` 的 `ResourceResolver::ResolvePath`：`*.bin` 编译产物按方案进子目录 `build/<方案>/<方案>.<类型>.bin`（其余资源保持扁平，缺则回退扁平）。
- 统计插件：存储从 txt 改为 LevelDB（`userdb\input_stat.userdb`，旧 txt 首次自动迁移并改名）；报表新增 实际/理论 击键·码长 + 键准，逗号改空格，`UTC+` 前加空格，「误触按键」改「误触」，名言前固定一行「知识从我而来，问题到我为止。」，署名 `脚本：🐐🪶❄️₂₀₂₆1010・A`。
- 主题 `ten`（液态玻璃）：全透面板 + 圆角（高度 1/4）+ 2px 圆角描边 + 逐图元抗锯齿 + 顶部高光/左右淡边/底部暗边 + 选中与悬停为灰色（无蓝色）+ 无磨砂（acrylic 关闭）。
- 便携版已删除，只保留安装版。

## 待办（未完成）

1. **安装版**：`installer\weasel-rs.nsi` 的 `InstallDir` 改小写 `$PROGRAMFILES64\weasel-next`（`PRODUCT_KEY`/`UNINSTALL_KEY` 同步小写）；`asset\`（含 `sound`）打进程序文件夹，否则装完**声音会失效**；`librime\build-librime-mingw.ps1` 里旧的 `artifacts\smoke` 改成 `artifact\smoke`；然后跑 `script\build-installer.ps1` 出安装包。
2. 用户文件夹的四个分类文件夹 + 四种 TOML（见上）。
3. **推送**：本地 `main` 领先远端 35 个提交，但本机 `github.com:443` 被阻断 → 需在能连上时 `git push origin main`。
4. `weasel_tip.dll` 仍是改名前的旧版（被运行中的程序占用）→ 关掉用输入法的程序后覆盖，否则管道名不匹配、输入法连不上。
5. 清理：`artifact\smoke` 里 12 个 `.old-*`/`.previous`（约 100 MB）；`C:\code\librime` 里的 `*.bak-*` 与 `build-mingw/` 未忽略；`script\download_librime.ps1` 是死代码。
6. 测试断言同步（布局 2 条 + capi 1 条 + 误触 2 处，当前 65/67）。
7. **液态玻璃的折射与厚度**（B 方案，真·液态玻璃的最后一块）：绑定补 `CLSID_D2D1DisplacementMap` + `ID2D1Effect`；`BitBlt` 抓窗口背后画面 → 生成位移贴图（RG 编码，128 = 不偏移）→ `DisplacementMap` 效果（`Scale` 对应 thickness ≈ 26px）→ R/G/B 三次绘制做色散 → 叠回高光/描边。
8. 程序文件夹的两个工具（见文首：「暂时不做」）。

## 统计插件的文件归属（已定）

| 内容 | 位置 |
|---|---|
| 插件代码（Rust `plugin/core` + C++ `plugin/librime`） | 编进程序文件夹的 `rime.dll` |
| 名言库 `quote.txt` | `<用户目录>\plugin\quote.txt` |
| 旧统计文本（仅首次迁移用） | `<用户目录>\plugin\input_stat_<方案>.lua` |
| 统计数据库（LevelDB） | `<用户目录>\userdb\input_stat.userdb`（保持与“所有 `*.userdb` 都在 `userdb\` 下”一致） |

## 输入统计窗口（`/stat`「个人生涯」）的数据源（已定 2026-10-17，**22:0x 改二进制**）

* 数据链：**TIP 采集 → `%APPDATA%\Rime-Next\stat\stat.bin`（单数 `stat`，单个二进制文件）
  → `Weasel-Next-Setting.exe /stat` 读并显示**。
  写侧 `Tip/src/stats.rs`（`encode`/`parse_file`），读侧 `Setting/src/stats.rs`（`parse_stat_bytes`）；
  **两边是同一份字节契约，改一处必须同时改另一处**，各自有一组独立手写字节的往返测试看着。
* 用户明确要求（2026-10-17 22:0x）：目录名 `stat` 单数、**单个二进制文件**、进程独占锁**防删**。
  ⇒ 写侧 `CreateFileW(..., dwShareMode = FILE_SHARE_READ)`。
  ⚠️ **共享模式由「先打开的一方」决定**：写侧给 `0` 时，读侧请求 `FILE_SHARE_READ|WRITE` 也一样
  拿 `ERROR_SHARING_VIOLATION`（22:0x 的检查点写过反向结论，已实测推翻）。
  `FILE_SHARE_READ` 同时满足两件事：读得开、`DeleteFile` 仍然失败（删要 `FILE_SHARE_DELETE`）。
* 格式 v3（小端，2026-10-26）：头 16B `"WNST"|version=3|天数|字数`；
  天 36B `date|keys|chars|active_ms|effective_keys|commits`；字 12B `codepoint|count`（全量前 200 个字）；
  尾部 1156B = 256 个虚拟键计数 + 33 档码长直方图（**累计**，不按天分）。
  * `effective_keys`（Σ编辑量 = Σ按键 − 2×退格，负值收 0）与 `commits`（当天上屏轮数）是 v3 新增的
    **当日**数据，专供「今日键准」「今日平均词长」两张卡；灰色历史值仍由尾部直方图算（累计）。
  * 读侧认 **1/2/3**：v2（28B 天记录）读出来 `effective_keys` 按"全有效"补齐、`commits` = 0；
    v1（20B、无时长/字表）照样读。落盘一律写 v3。
  * ⚠️ 写侧遇到**不认识的版本（>3）就拒绝落盘**（计数留在内存下次重试），绝不按空文件覆盖：
    旧版 TIP 把未知版本当空文件，会把 v3 记录直接清空。
  * ⚠️ 换 TIP 后**旧进程里的旧实例还在**（它们只会写 v2，且同一文件只能被"先拿到句柄"的进程写）。
    所以部署完 TIP 尽快重启一次（重启最干净），别让旧实例在 v3 文件上落盘。
* 旧 `stats\YYYY-MM-DD.yaml`（**复数**目录）只作**只读兜底**：二进制里没有的日期才用，
  不迁移、不删除用户文件。
* 为什么不直接读统计插件的库：插件的数据在 librime 进程里（LevelDB / `input_stat_*.lua`），
  目前也还没有数据；`/stat` 是界面侧的"个人生涯"，先用 TIP 这条**绝不阻塞按键**的链路喂。
  等 Lua 版插件真正跑起来，再把更细的指标（码长 / 键准 / 极速）并进来。
* 段位 / 境界两张表照抄 `input_statistics.lua`（20 级 + 7 境界），在 `Setting/src/main.rs`；
  **经验 = 累计上屏字数**。
* 速度（字/分）只用 `active_ms` 算：按键间隔 **>5 秒**视为中断、那段不计时（与 lua 版 `gapThd = 5` 一致）。
  v1 文件 / 旧 toml 没有时长 → 速度显示 0，不除零。


## 程序文件夹与 asset 的结构（已定，待实现）

单一配置文件夹原则：配置只有一份，就是用户目录 `%APPDATA%\Rime-Next\`；
仓库里的 `asset\` 只作**出厂默认**来源，编译时搬进程序文件夹。

```
asset\                     仓库里（出厂默认与程序资源）
  Rime-Next\               ← 出厂默认配置（编译时整个搬到程序文件夹）
      default.eassy.yaml   ← 原 essay.txt（细胞词库/语言模型词表，YAML 格式）
      …（schema / dict / theme / model 的默认内容）
  icon\                    ← 程序图标（11 个 .ico，各组件共用 weasel.ico）
  sound\                   ← 键声/上屏声（typewriter-key.wav / typewriter-bell.wav）
  （历史设计，已废弃：共享数据现在是 Rime-Next\ 本身，按分类树组织）
```

编译/安装：
- `Rime-Next\` →  `C:\Program Files\Weasel-Next\Rime-Next\`（程序文件夹里也有一份）
- 用户实际用的是 `%APPDATA%\Rime-Next\`（唯一的活动配置文件夹）
- 于是不必再考虑"多个配置文件夹"的问题。

文件格式：
- 细胞词库统一叫 `*.eassy.yaml`（注意拼写是 **eassy**），出厂默认那份叫 **`default.eassy.yaml`**；
- 多份 eassy 由部署流程按词归并（频次求和）后生成 librime 需要的 `essay.txt`（或 `<名字>.txt` + `vocabulary:`）。

安装路径（首字母大写）：`C:\Program Files\Weasel-Next\`；代码目录 `C:\code\Weasel-Next\`。

```
asset\                         仓库里（出厂默认 + 程序资源）
  Rime-Next\                   ← ★ 出厂默认的整棵配置树（编译时整个搬到程序文件夹）
      sound\                   ← 键声 / 上屏声（typewriter-key.wav / typewriter-bell.wav）
      model\                   ← 语言模型（*.gram，如 zh-hans-t-essay-bgw.gram）
      default.eassy.yaml       ← 原 essay.txt（细胞词库词表，YAML 格式）
      （历史设计，已废弃：见上）
      …（schema / dict / theme 的默认内容，均为 TOML）
  icon\                        ← 程序图标（11 个 .ico，各组件共用 weasel.ico；构建脚本要用，留在 asset 根）
```

搬迁后程序文件夹的样子（**只有二进制 + 一个配置文件夹**）：

```
C:\Program Files\Weasel-Next\
  Weasel-Next-Server.exe · Weasel-Next-Engine.exe · Weasel-Next-Renderer.exe · Weasel-Next-Setting.exe
  weasel_tip.dll · rime.dll · libc++.dll · libglog.dll · libopencc.dll · libunwind.dll · WebView2Loader.dll
  theme\                       ← 主题 DLL
  Rime-Next\                   ← ★ 唯一的配置文件夹（= Rime-Next 整棵搬过来）
  7z\                          ← 以后放 7z.exe（解压）      【不急】
  aria2-next\                  ← 以后放 aria2-next.exe（下载）【不急】
```

由此：程序文件夹不再散落配置 ✓，活动配置只有 `%APPDATA%\Rime-Next\` 一处 ✓。
## 安装时直接部署（已定：A 方案 + 异步，不卡向导）

目标：装完就能打字，但安装向导**绝不能被部署卡住**。

做法：
1. 用户在向导里**选好安装目录**（`$INSTDIR`）后即确定所有路径；
2. 文件复制完成后，用 **`Exec`（分离进程）**而不是 `ExecWait` 拉起部署助手：
   - `nsExec::Exec` / `Exec` 启动 `launch-broker.ps1 -Mode PostInstall`，立刻返回；
   - 助手做两件事：
     a) 若 `%APPDATA%\Rime-Next` 不存在 → 从 `<INSTDIR>\Rime-Next` 整棵复制（初始化配置）；
     b) 调 `Weasel-Next-Server.exe --deploy --silent`（现成入口：`broker\src\installer.rs:35`）。
   - 助手是独立进程（broker 本身也是独立受监督进程），因此**向导继续显示"完成/快捷方式/注册"等步骤时不卡**；
3. 完成页显示"正在后台部署…可以开始打字"；
4. 兜底：broker 首次启动再检查一次（`build\` 产物 + `installation.yaml` 是否存在），未部署则补部署；
5. 托盘气泡提示"正在部署 / 部署完成 / 部署失败"（`broker\src\notifications.rs` 现成）。

注意：
- NSIS 本身是单线程、无真正线程原语 → "双线程"用**分离进程**实现，效果等同（互不阻塞）；
- 首次方案编译耗时较长（`pinyin_simp.dict.yaml` 31 MB），所以必须异步；
- 若部署失败，安装仍然算成功，由首次启动兜底重试，避免"装到一半失败"。

## 插件架构（用户澄清的设计 + 落地约束）

用户要的结构：
```
代码文件夹\plugin\
    core\              插件接口 / SDK（Rust，公共 ABI 契约）
    <内置插件>\         内置插件实现（统计插件等）
程序文件夹\
    plugin.dll         插件的接口层（宿主，导出稳定 C ABI）
    rime.dll           librime 本体
用户文件夹\
    plugin\<插件名>\   单独的插件代码（Rust，可扩展；不是数据）
```

落地约束（必须知道的事实）：
1. librime 在 Windows 上**不加载外部 DLL 插件**（plugins\ 目录里的 DLL 不会被加载）→ 需要进入
   librime 翻译器链的插件（如统计插件）**只能静态编进 rime.dll**；rime.dll 就是它的二进制载体。
2. 因此"插件接口 plugin.dll"只能是我们**自己的宿主**：由 Weasel-Next-Engine 加载 plugin.dll（Rust cdylib，
   导出 C ABI），plugin.dll 再去加载/托管各插件。librime 那条链上的插件不受它影响。
3. "用户文件夹放 Rust 源码"要能**被程序编译**，需要用户机上有 Rust 工具链（重，不可行）→ 实际做法二选一：
   (a) 用户文件夹放**编译好的插件 DLL**（plugin\<名字>\<名字>.dll + plugin.yaml 元数据）；
   (b) 放源码仅供开发/查看，真正加载的是 DLL。
   两者都需要先有 (2) 的宿主 plugin.dll。

任务拆分（待做）：
  T1 在 plugin\ 下新增 core 的稳定 C ABI 面（版本号、注册表、生命周期回调）。
  T2 新增宿主 crate（输出 plugin.dll 到程序文件夹），实现：扫描 用户目录\plugin\*\plugin.yaml、
     加载其中的 DLL、按 ABI 调用、做版本校验与隔离（崩溃不影响宿主）。
  T3 Weasel-Next-Engine 启动时加载 plugin.dll（可选，缺失则静默跳过）。
  T4 统计插件保持静态编进 rime.dll（librime 链约束），只在文档里说明它为何不是独立 DLL。
  T5 安装器把 plugin.dll 放进程序文件夹；用户目录建立 plugin\ 空目录 + README（已有）。

## 插件宿主定名与 C++ 动态插件可行性（已定）

- 宿主 DLL 定名 **`weasel-plugin.dll`**（程序文件夹）。
- C++ 完全能做动态插件：Win32 `LoadLibraryW` + `GetProcAddress` + 纯 `extern "C"` 函数表
  （ABI 稳定，Rust 侧用 `#[no_mangle] extern "C"` 同样能实现）。

两条链分开对待：
1. librime 链上的插件（统计插件）—— Windows 上 librime 没有插件加载器，必须静态编进 rime.dll。
2. 我们自己的插件体系 —— 由 weasel-plugin.dll 当宿主，动态加载 用户目录\plugin\<名>\<名>.dll。

推荐实现（兼顾两者）：
```
rime.dll  (静态, C++)  ← librime 强制要求静态的薄适配层
   └─ 运行时 LoadLibraryW("weasel-plugin.dll")
        └─ weasel-plugin.dll (宿主, C++ 或 Rust)
             ├─ 统计插件实现（报表/存储，真正的逻辑）
             └─ 动态加载 用户目录\plugin\<名>\<名>.dll（C++/Rust 任意）
```
即：统计逻辑跑在真正的动态 DLL 里，rime.dll 只留几十行胶水。

落地顺序（替换原 T1/T2）：
  S1 定义宿主 ABI：weasel_plugin_host_api（版本、日志、Rime 查询、定时器）+
     weasel_plugin_entry(const host_api*) 入口约定；C 头文件放 plugin\core\include。
  S2 新建 weasel-plugin 宿主 crate/工程，输出 weasel-plugin.dll；实现扫描、加载、版本校验、隔离。
  S3 rime.dll 的 C++ 适配层改为调用 weasel-plugin.dll（找不到宿主时退化为内置实现，保证不崩）。
  S4 Weasel-Next-Engine 启动时确保宿主已加载（可选）。
  S5 安装器把 weasel-plugin.dll 放进程序文件夹；用户目录 plugin\<名>\ 放插件 DLL。

## 插件宿主改名 + "代码不编译直接运行"的结论（已定）

- 宿主 DLL 定名 **`rime-plugin.dll`**（通用命名；替代先前的 weasel-plugin.dll）。

- 为什么"有 ABI"不等于"能跑源码"：ABI 约定的是已编译机器码之间如何互相调用；
  要让**源码**直接运行，宿主里必须有**解释器/JIT**。

- 三条可行路径（宿主按扩展名分派）：
  * `.lua`  → 宿主内嵌 Lua（~200 KB，C ABI 极简）；原版统计插件就是 Lua，用户文件夹放 init.lua 即可直接跑。
     —— 这就是"不编译直接运行"的答案。
  * `.wasm` → 复用本仓库已有的 wasm 运行时（theme\wasm）；仍需一次编译成 wasm，但跨语言且沙箱安全。
  * `.dll`  → 原生插件（C++/Rust），最快，但要编译。

- 目录约定：
  程序文件夹\rime-plugin.dll          宿主（ABI + 内嵌 Lua + 复用 wasm 运行时）
  用户文件夹\plugin\<名字>\init.lua    Lua 插件（免编译）
  用户文件夹\plugin\<名字>\mod.wasm    wasm 插件
  用户文件夹\plugin\<名字>\mod.dll     原生插件
  用户文件夹\plugin\<名字>\plugin.yaml 元数据（名字/版本/入口/权限）

- 边界：解释运行的脚本不能直接进 librime 的翻译器链（librime 只认编进去的 C++），
  因此宿主通过 API 表把 Rime 能力（查询/候选/上屏事件等）暴露给脚本；
  "读事件 + 计算 + 出报表"这类插件（统计插件）完全够用。

- 落地顺序调整：
  S1 定 ABI: rime_plugin_host_api（版本/日志/Rime 查询/事件订阅/定时器）+ rime_plugin_entry。
  S2 新建宿主工程 → rime-plugin.dll（含 Lua 绑定；wasm 复用现有运行时）。
  S3 rime.dll 的 C++ 胶水层 LoadLibraryW("rime-plugin.dll")；找不到则退化为内置实现。
  S4 Weasel-Next-Engine 启动时确保宿主已加载。
  S5 安装器放 rime-plugin.dll；用户目录 plugin\<名>\ 放插件代码；示例 Lua 插件随包提供。

## 统计插件做回 Lua（用户要求，待实现）

素材（已定位）：
- 基础：`C:\code\rime-lua\lua\input_statistics.lua`（53,892 字节，经典全功能版，Rust 端就是由它合并而来）。
- 现状参照：`%APPDATA%\Rime-Next\statistics.lua`（25,534 字节，snow 系；命令 tj/rtj/ztj/ytj/ntj/qtj、tjq、tjs；
  量：词数/字数/用时/按键/有效按键/键准/均速/极速/击键/码长/理论码长/平均词长/多字词占比/词长分布/码长分布）。
- 今天新增（Rust 端已有，要全部搬进 Lua）：
  * 四行式：实际极速 / 实际均速 / 理论极速 / 理论均速，每行「速度 击键 码长」；
  * 理论值 = 实际值 x 键准（有效按键/按键）；键准、码长在标题行；
  * 「上屏 N 次 输入 M 字」；时间戳与时区之间留空格（UTC+08:00）；
  * 「误触」改为「误触」并附 误触率 + 键准；逗号改空格；
  * 名言前固定一行「知识从我而来，问题到我为止。」；
  * 署名行 `脚本：🐐🪶❄️₂₀₂₆1010・A`；
  * 存储改 LevelDB（当前 Rust 端写 userdb/input_stat.userdb）。

放置位置（用户指定）：
```
%APPDATA%\Rime-Next\plugin\input_staticstic\     ← 单独的文件夹（用户指定的拼写 input_staticstic）
      statistics.lua                             ← 合并后的 Lua 插件
      README.txt
```

前置条件（关键）：
- 当前 librime **没有 Lua 运行时**（无 lua_State / 无 rime-lua 插件）→ 必须先静态编入 `C:\code\rime-lua`，
  才能提供 lua_translator / lua_processor 组件。
- rime-lua 默认从 `<user_data>/lua/` 找脚本 → 需要改成 `plugin\input_staticstic\`（或加该目录到搜索路径），
  与"lua 改名 plugin"的既定结构一致。

步骤：
  L1 把 rime-lua 静态链进 librime，确认 lua_translator 组件可用。
  L2 改 rime-lua 的脚本目录为 <user_data>/plugin/input_staticstic（可回退到 <user_data>/lua）。
  L3 合并两个 Lua 源：以 rime-lua/lua/input_statistics.lua 为骨架，吸收 snow 版的设备维度(tjq/tjs)，
     并把今天所有格式更新写进去。
  L4 产出 %APPDATA%\Rime-Next\plugin\input_staticstic\statistics.lua，在方案里挂 lua_translator@statistics。
  L5 Rust/C++ 版保留（rime.dll 内），二者不冲突；文档说明取舍。

## 决定：不自己做插件宿主插件，直接用原版 rime-lua（已定）

用户决定：
- **去掉自建插件宿主 DLL**（原议的 weasel-plugin.dll / rime-plugin.dll 全部作废）。
- **用原版 rime-lua 的机制**：与官方 Weasel 一致 —— librime 内嵌 lua（已完成 ✓：
  lua_translator / lua_processor / lua_segmentor / lua_filter 均已在 rime.dll 中 ✓），
  脚本放用户目录，入口 rime.lua，其余由脚本 require；不引入任何自研宿主或 ABI。

官方 Weasel 0.17.4 实测（本机 C:\Program Files\Rime\weasel-0.17.4）：
- rime.dll 3.57 MB，含 lua_translator / lua_processor / lua_segmentor / lua_filter、rime.lua、LUA_PATH ✓
- 但它**不预置任何 .lua 脚本**（用户目录下既无 lua\ 也无 rime.lua）→ 脚本由用户或 plum 包提供。
- 安装目录 = 7z.exe/7z.dll + curl.exe/curl-ca-bundle.crt + WinSparkle.dll + data\ + 若干 .bat。

下载器：aria2 取代 curl（已定）
- 下载场景 aria2 完全覆盖 curl：-o 输出、-c 续传、--ca-certificate 指定 CA、-i 批量、
  -x/-s 多线程（curl 没有），仅缺少上传/任意动词（本用途不需要）。
- 因此程序文件夹放 aria2-next\（aria2c 改名 aria2-next），不再放 curl。

Lua 插件的落位（2026-10-20 用户改口径：入口也进 plugin\，**根部禁止放**）：
  用户目录\plugin\input_staticstic\statistics.lua   ← 插件脚本（用户指定目录名 input_staticstic）
  用户目录\plugin\rime.lua                          ← 入口：把 plugin\input_staticstic 加入 package.path 再 require
  方案里：lua_translator@statistics（或 lua_processor@…，视插件类型）

  * 能这么放是因为 librime 的 `plugins/lua/src/modules.cc` 有 **Weasel-Next 补丁**，
    搜索顺序：**用户根 → 用户 plugin → 共享根 → 共享 plugin**。
    2026-10-20 实测：把 rime.lua 从根部搬进 `plugin\` 后重启引擎，
    `plugin\_boot.log` 照样出现「=== rime.lua 开始加载 ===」+「虎整句已注册」。
  * **检查点里「放 plugin\ 无效、只能在根部」这条已被推翻**（那是补丁加之前的结论）。
  * 仓库里的出厂副本也在 `Rime-Next\plugin\rime.lua`；`deploy-install.ps1` 只在
    「用户目录 plugin\ 里没有」时才复制，**绝不覆盖用户/插件包改过的那份**，
    并且会把根部的历史遗留搬进 plugin\ 再删掉根部那份。
  * 虎整句的注册块（14 行）属于**插件包**（虎整句 zip 里有 `rime.lua.sample`），
    不该写进出厂 `Rime-Next\plugin\rime.lua`；用户机器上那份已恢复。

## 决策：所有图形界面统一使用 Direct2D（已定）

- 候选窗、状态窗、设置界面、预览窗 —— 全部 GUI 一律 Direct2D/DirectWrite 自绘，
  统一由 Rust + Direct2D 实现，不引入 WebView、不使用 XAML、不再新建 Slint 界面。
- 原因：只有 Direct2D 这条路能做到真·液态玻璃（透明 + DWM 背景模糊 + 圆角高光 + 折射位移 + 厚度色散）。
  Slint 与 Tauri/WebView2 都拿不到窗口背后的像素，也没有 D2D 效果链，做不出折射。
- 现状：候选窗已是 D2D（theme\ten ✓）；设置界面目前是 Slint（Weasel-Next-Setting）→ 计划改为 D2D 自绘。
- 设置界面改造要复用现有非 UI 核心：settings 库里的字段模型、校验、会话读写与框架无关，
  可直接被 D2D 前端调用；只需替换 UI 层。
- 迁移顺序：先做主题化外壳（窗口/圆角/玻璃）→ 再逐页迁移（常规/外观/按键/词典/关于）→ 最后删除 Slint 依赖。
- 性能约定：设置界面同样采用 D2D1_ALPHA_MODE_PREMULTIPLIED + DwmEnableBlurBehindWindow，
  与候选窗共用一份 D2D 绑定（theme\support\build_d2d.rs 生成的绑定）。
- **2026-10-29：外壳 v1 已落地**（`Weasel-Next-Setting-D2D.exe`，见 `PROGRESS.md` 六·十三）：
  Win32 无边框 + DComp 交换链 + D2D/DWrite 自绘 + DWM 亚克力（`#991B1F27`）+ Win11 圆角；
  左侧五页签 + 占位卡片，可点、可拖、可缩放。**默认设置入口仍是 Slint 版**，进包与切换都还没做。
## 安装期两件独立任务（已定：并行、互不牵制）

1. **部署**：`Weasel-Next-Engine.exe --deploy --silent`（由 broker 的 post-install 发起）。
2. **解压语言模型**：用 `7z\7z.exe` 把 `Rime-Next\model\*.gram.7z`（含分卷 .001/.002）解到 `Rime-Next\model\`。

约定：
- 这两件**互相独立**，可同时进行，不依赖用户配置、不依赖方案、不依赖对方先完成。
- 部署本身会在 librime 初始化前由 `server\librime\mod.rs` 的 `generate_essay_txt()` 从
  `eassy\**\*.eassy.yaml` 归并生成 `eassy\essay.txt`，因此不需要预先准备 txt。
- 若解压尚未完成，部署会照常进行（模型缺失只影响语言模型打分，不阻断方案编译）。

## eassy 的位置（已定）

- **代码目录与程序文件夹**各有 `Rime-Next\eassy\<名字>\<名字>.eassy.yaml`（出厂源）。
- **用户目录不建 `eassy\`**；`essay.txt` 是**程序文件夹里的部署产物**
  （`<程序>\Rime-Next\eassy\essay.txt`），由部署/初始化时从 TOML 归并生成。
- 用户若想扩展，可自行在 `<用户目录>\eassy\` 放 `*.eassy.yaml`，会被一并归并（可选）。
- librime 的 `*.txt` 资源规则指向 `<root>/eassy/`。
## 以后可下载"用户词库"（已记录，不急）

设想：
- 将来支持从网络**下载用户词库/细胞词库**，下载后**同样按 eassy 的分类**放好：
  `eassy\<名字>\<名字>.eassy.yaml`（与出厂词库同一套格式与目录约定）。
- 因为归并器已经把 `eassy\**\*.eassy.yaml` 全部按词求和并生成 `essay.txt`，
  **下载来的词库不需要额外接入**：放进 eassy\ 即被并入，重新部署/初始化即生效。
- 下载器用程序文件夹里的 `aria2-next\aria2-next.exe`；压缩包用 `7z\7z.exe` 解压
  （与安装期解压模型同一套工具）。
- 用户目录可选扩展：`<用户目录>\eassy\` 也支持（用户自己放的词库会被一并归并）。

状态：**不急**，本次不做；实现时无需改动归并器与 librime 规则，只需下载 + 落盘两件事。
## 语言模型改由外部获取（已定，仓库不再分发）

- **不再**在仓库放 `*.gram` 或分卷压缩包；**不再**有安装期解压步骤。
- 仓库 `Rime-Next\` 因此不含 `model\`：内容只有出厂配置（eassy/schema/theme/icon/sound/opencc + 扁平配置）。
- 模型由外部获取（下载，或从既有安装/官方目录同步）后直接放进 `<程序文件夹>\Rime-Next\model\`；
  用户目录的 `model\` 保持为指向该目录的 **junction**（一份数据两处可见 ✓ 改程序文件夹即生效 ✓）。
- 缺失模型不阻断部署与打字，只影响语言模型打分（grammar 加权）。
- 以后可用 `aria2-next` 下载（与"下载用户词库"同一套工具与思路）。
## eassy 也用 junction（与 model 一致，避免两份）

- 实体只有两处：**代码目录**（git 源）与**程序文件夹**（部署产物 essay.txt 也写在这里）。
- **用户目录的 `eassy\` 是指向程序文件夹的 junction**（`mklink /J`，免管理员）：
  一份数据、两处可见；改程序文件夹即对用户目录生效。
- 因此不再有"两份 eassy 内容"的问题；用户若想自己加词库，直接往该 junction 里放即可
  （实际落在程序文件夹，会被归并器读到）。
- 同类做法适用于只读出厂内容：`model\`（已做）、`eassy\`（本次）。
  用户数据目录（userdb/build/*.custom.yaml/installation.yaml）**绝不能**做链接。
## 液态玻璃（折射 / 色散）实现要点

调研时间：2026-10-12。结论：**没有"Vulkan 液态玻璃"这种专属教程**，液态玻璃不是某个图形 API 的特性，而是一段片元着色器的数学。Vulkan / D3D12 / Metal / WebGL / WebGPU 上做法完全一致，真正平台相关的只有一步：**如何拿到窗口背后的像素当纹理**。

### 七步配方（照 masakazuimai 的 WebGL 折射实现整理）

1. 铺背景（cover）。
2. 用 **SDF** 定义玻璃形状；圆角矩形用 Inigo Quilez 的 sdRoundRect，函数返回带符号距离（内负外正）。
3. 对距离场做**数值微分求梯度**，归一化后即该点外向法线，也就是"边缘朝向"。
4. **折射**：位移 = 法线 × 边缘权重 curve × 折射强度 uRefraction × 最大位移；再按位移后的坐标采样背景。中心 curve 近 0 几乎不歪，越靠边弯曲越强 —— 这就是真实透镜的行为。
5. **色差**：R / G / B 三通道分别用不同偏移采样，偏移 = 法线 × curve × uAberration × 14。
6. **磨砂**：WebGL 没有 CSS 模糊，改为多点采样求平均（原文用中心 + 12 个固定方向的核）。
7. **边缘高光 + 抗锯齿**：用法线做镜面反射；边界用 smoothstep 配合 SDF 做 1px 平滑。

关键洞察：CSS 的 `backdrop-filter: blur()` 只能对背景像素**就地**处理，无法从别处取像素，所以永远做不出折射，必须写着色器。

默认参数参考：折射 0.7、色差 0.4；模糊半径要乘 DPR。

### 参考资料

- masakazuimai，WebGL 液态玻璃折射七步详解（246 行，最系统）：https://github.com/masakazuimai/zenn-content/blob/main/articles/liquid-glass-webgl-refraction.md
- aaaa-zhen/glsl-playground，单文件零依赖 GLSL demo，源码即教程；液态玻璃可在线玩、可上传截图实时折射：https://github.com/aaaa-zhen/glsl-playground/liquid-glass.html
- 同仓库的 metaball-search：从屏幕顶部拖出液滴凝聚成黑玻璃搜索框，SDF 驱动折射：https://aaaa-zhen.github.io/glsl-playground/metaball-search.html
- Codrops，Three.js + WebGPU + TSL 的三维液态玻璃网格：https://tympanus.net/codrops/2026/09/08/building-an-infinite-liquid-glass-grid-with-three-js-webgpu-and-tsl/
- Spectral Glass 光谱色散 demo：https://saqoosha.github.io/Spectral-Glass/
- Flutter liquid_glass_easy 的参数面（折射强度 / 色散 / 模糊 / 厚度）：https://pub.dev/documentation/liquid_glass_easy/3.5.0/liquid_glass_easy/LiquidGlassRefraction-class.html

### Windows 上取背景的三条路（唯一平台相关部分）

- **DWM 模糊**（DwmEnableBlurBehindWindow / SetWindowCompositionAttribute）：能出磨砂，但拿不到可采样的背景纹理，做不了折射。
- **DirectComposition**（WS_EX_NOREDIRECTIONBITMAP + IDCompositionVisual 效果链）：能合成、能模糊，同样不给原始背景像素。
- **抓屏**（Windows.Graphics.Capture 或 BitBlt）：抓窗口背后的区域当纹理喂给着色器，折射与色散想怎么算就怎么算。Vulkan 在这件事上和 D3D 处境相同，都得靠系统 API，见 Stack Overflow「Can Vulkan interop with DirectComposition for window background blur effects?」https://stackoverflow.com/questions/66628174

### 本项目落地建议

1. 着色器数学一次写成 **WGSL**，将来 wgpu 可全平台复用；也可先在 HLSL 里验证。
2. 候选窗**先不做真折射**：用 D2D 内置高斯模糊（CLSID_D2D1GaussianBlur）+ 顶部高光 + 边缘亮线，观感即可接近官方，成本几乎为零。
3. 真要做液态玻璃，再加背景抓取（Windows.Graphics.Capture 抓候选窗背后那条区域），并且**必须节流**，不要每帧全屏抓。
4. 参数宁保守：候选窗是打字时盯着看的小面板，折射过强直接伤害可读性（苹果自家 Liquid Glass 也因可读性挨过批评）。

## 设置入口与「界面风格 / 方案选单」收进主界面（2026-10-17 23:2x）

* **托盘「设置」= `Weasel-Next-Setting.exe` 无参数 = Slint 主界面**。
  `Server/src/menu_actions.rs::parameters(SETTINGS)` 故意返回 `None`；`Setting/src/main.rs`
  里**绝不能**再把无参数改写成 `/switcher`。2026-10-17 之前正是这样：点「设置」弹的是官方
  方案选单对话框，用户 21:11 报「设置界面没显示，直接是这个」。当时三份检查点都写着
  「无参数 = Slint 主界面」，与代码**相反** —— 别再凭检查点这句话去改。
* **界面风格 / 方案选单**从托盘菜单收进主界面顶部「界面与方案」卡片，两个按钮。
  两者都是官方模板的 Win32 对话框（`DialogBoxParamW`），Slint 事件循环里建不了，
  按钮回调走 `Setting/src/main.rs::open_official()`：`std::process::Command` 再起一个
  自身进程，参数分别是 `/style`、`/switcher`。
* **配色页只该建一次**：`ColorSchemesPage` 原先写在 `for field in visible-fields` 循环里，
  于是每个字段都渲染一份（截图里一段界面能看到两三个白块）。现在移到滚动区末尾的
  「配色方案」区，并给它固定高度 —— 竖排 ScrollView 里不给高度，内部配色列表会塌成一行。
* **`app_options` 的 `inline_preedit` 不能用 `ConfigSnapshot::app_bool`**：它的"全局值"在
  主题文件（`style/inline_preedit`），宿主配置里没有这个键，`app_bool` 会以
  `missing required configuration value inline_preedit` 失败 → `State::show()` 整个失败 →
  主界面停在只读（开关点了不动、保存灰着）。用 `app_option_bool(...).unwrap_or(false)`。
* **验证口径**：Win32 对话框用 `EnumWindows` 按 PID/类名 `#32770` 查，别按标题猜；
  主界面截图用 `PrintWindow(PW_RENDERFULLCONTENT)`，滚动不可靠时先向上滚 60 格到顶再点。
  点按钮前用 `SetForegroundWindow` + `ClientToScreen(0,0)` 换算坐标（图像坐标 ≈ 客户区 + (9,5)）。
  **2026-10-18 补**：`C:\Code\click.ps1` 的 PostMessage 点击对 Slint 设置窗口**无效**（连点两次页签都没切）；
  必须用真鼠标事件：`ClientToScreen` 拿客户区原点 → `SetCursorPos` → `mouse_event(LBUTTONDOWN/UP)`。

## 形码整句 / 顶功（2026-10-18 调研记录）

### 三码整句（参照 Litles/rime-xingma-sentence，MIT）
用户认定「整句要配上这些黑科技才完备」：<https://github.com/Litles/rime-xingma-sentence>
它的四个特性（README 原文）：① 畅打十万字（常用字 2–3 码，生僻字无缝补第 4 码）；
② **整句句中单字选重**——重码字不用移光标，补辅助码 `_`（空格）选重，可选任意重；
③ **整句句中词选重**——词重码用 `/` 选重；④ **恢复防变**——`/` 把已定内容固定，后续延长句子不再改动。
实现形态：**Python 生成器**（`func_lib.py` + 每个形码一个 `schema_*_fluid.py`）把码表/拆分表 + 字词频
（知频 28 MB 词频表等）+ 字符集（通规 8105 / GB18030 / Big5）烤成 Rime 方案。
关键点：空格版里 **`_`（空格）和 `/` 是合法编码字符**，要动 `speller/alphabet`/`delimiter` 和按键处理链，
Rime 原生 `punctuator`/`key_binder` 做不到"句中选择性补码"，**必须自定义 processor/filter（Lua）**。
我们的条件：librime 已内嵌 Lua（`lua_processor/filter/segmentor/translator` 都在）；但仓颉/笔画是 1–5 码
不定长，要做"三码整句"得先按目标码表生成三码码表（可用 `shuangpin_schema` 同类的生成器 + `encoder` 公式）。
落地顺序建议：① 三码化码表 → ② 开整句 + `_`/`/` 进 speller + 最小 processor（先单字选重）→
③ 灌知频字词频 → ④ 词选重 + 恢复防变。

### 大小集合顶功（用户 2026-10-18 提出，**未实现**）
定义（按用户要求记）：只有当当前编码的候选**全部落在「小集合」**（常用字集，如通规 3500 / 8105）时，
才**自动顶屏上屏**（顶功）；一旦可能命中「大集合」（全字集 / 生僻字）就保守等空格，避免误上屏。
现状：**Rime 原生只有「定长顶字上屏」**——`speller/max_code_length`（`cangjie5.schema.yaml` 里还留着一行注释
`#max_code_length: 5  # 五碼頂字上屏`），**没有"集合"概念**。
所以要做得自己加一层：`lua_filter`/`lua_processor` 判断「当前码的全部候选 ⊆ 小集合」→ 自动 commit；
否则不顶。前提是小集合字表（通规 8105 / 常用 3500，仓库与 rime-xingma-sentence 里都有现成的）。
结论：**可做，中等工作量**；建议先开原生的定长顶屏（几乎零成本）验证手感，再加"小集合判定"的 Lua 层。

### 顶功的现成实现（读过 `sbsrf.zip` 里的 lua，2026-10-18）
素材：用户给的 `C:\Users\CloudyFlow\Downloads\sbsrf.zip`（73 MB；`lua/sbxlm/*.lua` + 几十个 `*.dict.yaml`；
本棒解压到 `C:\Code\_tmp_sbsrf`，可删）。
* **顶功核心 = `lua/sbxlm/popping.lua`（245 行）**：挂在 speller 前面的 processor。每次按键用
  「当前输入串 + 本键」匹配 `speller/popping` 规则，命中就 `confirm_current_selection()` + `commit()`
  把首选顶上屏，再把多余的码 `push_input` 回去继续打 —— 这就是"下一键顶上一段"。
* 规则字段：`when`（选项名）/ `match`（当前输入正则）/ `accept`（本键正则）/ `prefix`（截断位数，
  `>0` 保留前 N 位、`==0` 找最后一个声母位置）/ `strategy`。
* 策略：`append` 不顶（继续加码）、`conditional` 条件顶（先试着加一键，能出候选就不顶）、
  默认直接顶、`ignore` 顶完直接接受。
* 硬编码特例：`;` 两段式上屏、数字键 `2/3/7/8/9` → 笔画 `a/e/u/i/o`、`mm` 方案 3/4 码拆分、
  `delayed_pop`（元音不顶、辅音顶）、Rime "句号后隐藏 `.`" 的 bug 补丁；**大写键顶完转小写**
  （所以大写也能当顶功键）。
* ⚠️ **它没有"大小集合"概念**：顶不顶只看输入形态 + 下一键，**不查候选属于哪个字集**。
  所谓"小集合顶功"在这些方案里是用规则形态近似的（只在 2/3 码顶、只在 `mixed`/`pro_word` 选项下顶）。
  要做真·小集合顶功，挂点就是 `popping.lua` 里 `if context:has_menu() then` 那一行：加一层
  「候选列表 ⊆ 小集合」判断，小集合字表由通规 3500 / 8105 生成 Lua set。
* 依赖：`lua/lib.lua`（Rime Lua 封装，24 KB）+ `sbxlm/core.lua`（`core.fm/ft/mm/jp/py` 方案判断）；
  同包还有 `auto_length.lua`(41 KB)、`postpone.lua`（延迟上屏）、`editor.lua`/`sp_editor.lua`
  （顶功模式下的编辑行为）、`selector.lua`、`hint.lua` —— **顶功是一整套，不止 popping**。
* 许可：zip 里没看到 LICENSE 文件（对照：`rime-xingma-sentence` 是 MIT）⇒ 抄之前先确认来源许可。

## 简词 / 语言模型 / 自定义双拼（2026-10-18 核过一遍，记结论）

### 词库没词也能出词 —— 不需要额外实现
* `luna_pinyin.dict.yaml`（70,771 行）里既有词、也有**单字条目**。实测「没做」**不在词典**，但「没」「做」在；
  输入 `mei zuo` 照样上屏「没做」——这是 `script_translator` 的**跨字组合**（整串查不到词就切段用单字组合）。
  ⇒ 用户问的"识别不同字的组合"是 Rime 的既有行为，不用写代码。
* `eassy`（细胞词库）只影响**排序**：`Rime-Next\eassy\default\default.eassy.yaml`（`"〇": 981` 这种 词→频次）
  部署时归并成 `eassy\essay.txt`（2026-10-18 实测 5,093,882 字节），作为 `vocabulary` 参与 Grammar 打分，**不产生候选**。

### 简词 = 简拼（用户 2026-10-18 明确要的就是这个）
* 全拼（`luna_pinyin` / `luna_pinyin_simp`）：`speller/algebra.__patch` 引 `pinyin:/abbreviation`，
  落到 `Rime-Next\pinyin.yaml` 的 `abbrev/^([a-z]).+$/$1/`（首字母）+ `abbrev/^([zcs]h).+$/$1/`（zh/ch/sh）
  ⇒ 规则**是通的**（`nh` 应出「你好」）。⚠️ 2026-10-18 尚未实测成功（自动测试切输入法没落到我们的 IME，
  用 `Win+Space` 时 Win 键本身会被 TIP 记成按键，产生假阳性；判据要用引擎日志里的 `composition=xx`）。
* 双拼（`double_pinyin_flypy` / `double_pinyin_ziranma`）：生成的 schema 里 `speller/algebra` 是**普通列表**
  （`__include` 深合并时列表整体替换），只有键位映射，**没有 abbrev ⇒ 双拼没有简拼**。
  要加得改 `style/src/bin/shuangpin_schema.rs` 的模板，并注意与键位映射的先后顺序冲突（双拼两键 = 一个音节）。

### 语言模型（用户放了 4 个，**一个都没被用上**）
* 文件在 `%APPDATA%\Rime-Next\model\`（普通目录，**不是**上面说的 junction）：
  `zh-hans-t-essay-bgw.gram`(206 MB)、`zh-hant-t-essay-bgw.gram`(208 MB)、`zh-hans-t-essay-bgc.gram`(29 MB)、`zh-hant-t-essay-bgc.gram`(30 MB)。
* 程序文件夹 `Rime-Next\model\` 为空；仓库 `Rime-Next\model\` 不存在。
* 方案里只有 `__patch: - grammar:/hant?`（**可选**引用），而 **`grammar.yaml` 不存在**（程序文件夹/仓库/官方 data 都没有）
  ⇒ patch 被跳过 ⇒ `filters` 只有 `simplifier`/`uniquifier`，**没有 grammar** ⇒ 4 个模型完全不参与打分。
* 接上要做的：新建 `Rime-Next\grammar.yaml`（`hans`/`hant` 两个预设，`language:` 指向上面的名字），
  并确认 librime 的 grammar 组件怎么定位 `<language>.gram`（搜索路径顺序 / 是否要 `model\` 前缀）、认哪些配置键、filter 注册名。
  简体方案（`luna_pinyin_simp`）应指向 `hans`（现在从 `luna_pinyin` 继承的是 `hant`）。

### 自定义双拼是怎么做的（已实现，2026-10-18 复核）
* **唯一手改源**：`Rime-Next\parser\shuangpin\<名字>.parser.yaml`（现有 `xiaohe.parser.yaml` 小鹤、
  `ziranma.parser.yaml` 自然码；2026-10-27 从 `schema\shuangpin\*.toml` 迁到 parser 目录 + `.parser.yaml` 后缀）。
  结构：顶层 `schema:`（`id` / `name` / `version` / `author` / `dict` / `prism` / `simp_option` / `base`）
  + `initials:`（只有 `zh` `ch` `sh` 这类两字母声母要写）+ `finals:`（韵母→键位，**顺序随便写**）。
  音节表在 `parser\pinyin\syllables.parser.yaml`（424 条），注音键位在 `parser\zhuyin\zhuyin.parser.yaml`。
* **生成器**：`style/src/bin/shuangpin_schema.rs` → `shuangpin-schema.exe`（随安装包分发）。
  它把 parser YAML 的键位填进**固定的 algebra 模板**：规则顺序 = 官方 `rime-double-pinyin` 的顺序，**长韵母必须先于短韵母**
  （否则 `liang` 会被 `ang` 规则先命中，变成 `lih`）。产物是 `<用户目录>\Rime-Next\schema\<id>\<id>.schema.yaml`。
  * 找键位表的搜索顺序（2026-10-27 起 **程序文件夹优先**，parser 是出厂数据随安装包分发）：
    程序文件夹 `Rime-Next\parser\shuangpin\` → 用户目录 `parser\shuangpin\`（兼容兜底）→
    用户目录旧位置 `schema\shuangpin\`、`shuangpin\`（兼容）→ 仓库（开发时兜底）。
  * 生成的 schema 用 `__include: <base>.schema:/` **深合并**继承 `luna_pinyin_simp` 骨架，
    只覆盖 `algebra` / `translator` / `simplifier`。
    ⚠️ 里面**不能**写 `patch:` —— 那是 `.custom.yaml` 专用语法，写进 `.schema.yaml` 会 `invalid schema definition` 导致部署失败。
* **生效路径**：`script\deploy-install.ps1` 一开头就跑生成器（必须早于部署/编译），
  然后把 `Rime-Next\schema\double_pinyin*` 目录和 `parser\shuangpin\*.parser.yaml` 拷到**用户目录**
  （parser 数据只在目标不存在时拷，避免覆盖用户改过的键位），再跑一次生成器写到用户目录。
  librime 只认 `.schema.yaml`；同名时**用户目录那份优先**。
* **改键位的正确姿势**（2026-10-27 起）：改**程序文件夹** `%ProgramFiles%\Weasel-Next\Rime-Next\parser\shuangpin\<名字>.parser.yaml`
  → 设置界面「保存并部署」（或手动跑 `shuangpin-schema.exe`）→ 新键位生效。
  （parser 已从用户目录挪到程序文件夹：出厂数据随包分发、整包升级时一并更新；用户目录不再部署。）
* 原 `script\gen-shuangpin.py`（Python 版生成器）已于 2026-10-27 删除，路径已指向不存在的旧目录；
  生成器以 Rust 版 `shuangpin-schema`（weasel-style bin）为准，数据在 `Rime-Next\parser\shuangpin\*.parser.yaml`。

## 输入法链路的两条硬约定（2026-10-18 实测定下来）

* **候选窗不能因为拿不到插入点就"吃键不显示"**：引擎在新组合开始时清锚点并等 TIP 送几何（`waiting_for_layout`），
  等不到就只发"隐藏 + 空列表"帧；而 TIP 以前 `GetTextExt` 一失败就静默不发 ⇒ 某些宿主里打字完全没反应。
  现在 TIP 按「窄组合范围 → 整个组合范围 → 当前选区 → 视图矩形」逐级兜底，必须一直保留这条退让链。
* **TIP 的 DLL 是加载进每个宿主进程的**：换了 `Weasel-Next.dll` 后，**打字的那个程序必须关掉重开**才用上新代码
  （explorer / QQ / 浏览器 / 聊天客户端都算）。
* **设置界面写的是用户补丁**（2026-10-21 起 `%APPDATA%\Rime-Next\weasel.yaml`，真 YAML，就在用户目录根；
  旧名 `weasel.custom.toml` 已迁走），引擎读配置时必须**叠加**它，
  否则界面上关掉的东西不生效（2026-10-18 的「启用声音」就是这么假的：引擎只读程序文件夹的 `weasel.toml`）。
* **路径只有一处定义**：`common/src/settings.rs` 的 `user_config_path()` / `legacy_user_config_path()`，
  各组件都调它，别再手写 `join("weasel.yaml")`；迁移函数也在这里，服务端启动和设置界面加载都会先跑。

## 上游 PR #1232：`rewriter` 滤镜（2026-10-28 用户指定记录，**待实现**）

> https://github.com/rime/librime/pull/1232 —— amzxyz 的 fork，**未合并（open）**；
> 3 个 commit（`c4cdc74` 主体 / `6e31439` 前缀递进 / `8530499` 长句性能），13 files，+5235 行。
> 本次只记录设计与落地要点，代码一行没搬。

**是什么**：原生「KV 匹配」候选加工滤镜，替掉大量本来要写 Lua 的场景。
`engine/filters` 里写 `rewriter@s2t`，对应配置顶层节点 `s2t:`；只写 `rewriter` 就用 `rewriter:` 节点。
部署期把用户维护的 TXT 编译成**所有方案共用的一个** `build/rewriter.rwp`，
运行期按需只 mmap 用到的 Stage，直接查 darts Trie（不读源 TXT）。

**典型用途**：简繁/地区字形整句转换（替代 `simplifier`）、原候选+派生候选、emoji/翻译/拆字注释、
简码与自定义短语（替代 `custom_phrase` translator）、九键等编码投影（部署期转写 key）。

**配置参数**

| 参数 | 取值 | 默认 | 说明 |
|---|---|---|---|
| `option` | `true`/`false`/字符串/列表 | 未启用 | `true` 常开；字符串=受该 Context option 控制；列表=OR |
| `tags` | 字符串/列表 | TagMatching 默认 | 限定在哪些 segment 生效 |
| `mode` | `derive`/`replace`/`comment`/`abbrev` | `derive` | 见下 |
| `enable_sentence` | bool | `false` | 对整条候选从左到右做**最长匹配**（不是分词） |
| `comment_source` | `none`/`text`/`inherit` | `none` | 派生注释来源（仅 derive/replace） |
| `comment_template` | 字符串 | `〔%s〕` | 只替换第一个 `%s`；无 `%s` 用原文 |
| `candidate_type` | `preserve`/任意串 | `preserve` | 派生候选 type；`abbrev` 未指定时是 `abbrev` |
| `excluded_types` | 字符串/列表 | 空 | 跳过这些 type 的上游候选（abbrev 不查） |
| `insert_position` / `insert_count` | 正整数 | `1`/`1` | abbrev 插入位置/最多插几个 |
| `promote_on_types` | 字符串/列表 | 空 | 首候选 type 命中时，简码全部提前 |
| `fallback_on_empty` | bool | `true` | 上游无候选时释放全部简码兜底 |
| `key_xlit` | `"from>to"` | 空 | **部署期**字符转写 key，原 key 存成 preedit |
| `preset` / `files` | 二选一（必须且只能一个） | — | preset 引用共享 `<共享>/opencc/presets.yaml` |

**四种 mode**

* `derive`：保留原候选，再追加改写结果（`零`→`〇`，`二零二六年` → 第二条 `二〇二六年`）。
* `replace`：用改写结果替换原候选；无结果时保留原候选。
* `comment`：候选正文不动，只把 comment 换成改写结果（经 `comment_template`）。
* `abbrev`：不加工上游候选，直接拿**当前输入编码**查表注入（`jys`→`静夜思`）；不用 `enable_sentence`。

**数据格式**：一行一个 `key<TAB>value`；同 key 多值=重复 key（按行序），同 key 同 value 去重保首现；
多个 `files` 按「文件配置顺序 + 文件内行顺序」合并成一个 Trie。

**产物 / 部署语义**（这是最费工的部分）

* 文件：`build/rewriter.rwp`，magic `RIMERWP1`、format version 1。
* 结构：两段 superblock（A/B，generation 大者胜，用于原子提交）→ 若干 Stage object
  （header 带 payload hash + StageId）→ 末尾 Index（`StageIndex`: StageId→offset/size；
  `SchemaIndex`: schema→section→有序 StageId）。
* StageId = 格式/编译版本 + 该 Stage 源文件顺序与内容 + `key_xlit`；`mode`/`option` 等运行期参数**不参与**。
  ⇒ 同 Stage 跨方案/跨 preset 自动复用。
* 增量：先算 StageId，store 里已有就复用，没有才编译；所有方案处理完后一次生成紧凑 store
  （未变的 Stage 流式复制，写新 Index 后**原子替换**旧文件）。
* 紧凑化阈值：死 Stage ≥16 个，或垃圾 ≥4 MB 且 ≥当前文件 1/4。
* 失败回滚：workspace 提交前旧 Index 始终有效；失败就截断新增尾部，没旧文件就删掉本次创建的空文件。
* 运行期：option 关闭时**不映射**；第一次启用才读 Index、只 mmap 对应 Stage 范围
  （Windows `MapViewOfFile` 要按 `dwAllocationGranularity` 对齐），Stage 映射后关闭 option 也不卸载。

**C++ 实现文件**（PR 里能确认的 4 个，共 3235 行）：
`src/rime/dict/rewrite_internal.h`(197)、`src/rime/dict/rewrite_pack.h`(144)、
`src/rime/dict/rewrite_pack.cc`(1490)、`src/rime/dict/rewrite_store.cc`(1404)。
其余 9 个文件是滤镜装配 / 部署任务 / CMake / 测试（PR 共 13 files）。

**LRN（Rust）落地要点**

1. `engine/gear/` 加 `rewriter` 滤镜组件，按 `engine/filters` 的 `rewriter@section` 装配；section 名=配置顶层节点。
2. 部署任务：TXT → Stage（Trie + value pool + char directory），写共享 `build/rewriter.rwp`；
   需要 workspace 事务、双 superblock 原子提交、StageId 复用、失败截断回滚。
3. 运行期：store 索引解析 + 单 Stage range mmap；Trie 要用 darts double-array（LRN 现在只有 `marisa`，
   要对齐 `.rwp` 格式就得自己实现/移植 Darts，否则只能自定义格式、互不兼容）。
4. 语义：四种 mode、`tags`/`candidate_type`/`excluded_types`、最长匹配、insert/promote/fallback、`key_xlit`。
5. 落地后先在「s2t/s2hk/s2tw、emoji、简码」三处替换现有 `simplifier`/`custom_phrase`/Lua，再决定旧组件去留；
   完成后在 `librime-next/doc/PARITY.md` 的 filter 清单加一行（现在 6 项）。

**风险 / 注意**

* **PR 未合并**，配置键和 `.rwp` 二进制都可能变；先按文档实现语义，**别把格式当稳定契约**（除非要和 C++ 补丁版互读）。
* `preset` 依赖 `<共享>/opencc/presets.yaml` + 相关 TXT；作者明说该数据的获取/安装/打包「另行提交」，
  在那之前 preset 缺文件就不可用。
* 部署的原子性要求高（双 superblock、payload hash、临时文件 rename）；LRN 现在是直接写 `build/`，
  照搬要先做 staging/事务层。

---

## ARM64（Windows on ARM）安装包 —— 用户 2026-10-28 定下的矩阵

**口径**：两种包都出，**架构写进文件名**（`…-x64-setup.exe` / `…-arm64-setup.exe`）。

| 包 | 64 位 TIP | 32 位 TIP | 覆盖的宿主 |
|---|---|---|---|
| x64 | `Weasel-Next.dll`（AMD64） | `Weasel-Next-32.dll`（i386） | 64 位宿主 + 32 位宿主（WPS 等） |
| arm64 | `Weasel-Next.dll`（**arm64**） | `Weasel-Next-32.dll`（i386，与 x64 包是同一个文件） | ARM64 原生宿主 + 32 位仿真宿主 |

* 引擎：x64 包同时放 C++ `rime.dll` 与 `rime-next.dll`；**arm64 包只放 arm64 的 `rime-next.dll`**
  （`Engine` 优先加载这个名字，`rime.dll` 槽位空着即可）。arm64 产物只额外依赖一个非系统 DLL：`libunwind.dll`。
* 版本：**0.18.5**（安装器版本号取自 `Tip\Cargo.toml`，10 个 crate 一起升）。
  产物：`artifact\installer\Weasel-Next-0.18.5-x64-setup.exe`（47,519,492 B）与
  `Weasel-Next-0.18.5-arm64-setup.exe`（34,416,255 B）。
* 一条命令出 arm64 包：`script\build-installer-arm64.ps1`（`-SkipBuild` 跳过重编、`-SkipX64` 跳过顺带打 x64 包）。
* **仍未解**：**有没有骁龙真机可测**（2026-10-26 13:0x 起一直挂着）。所以 ARM64 只验到「打包层面」——
  包内每个 PE 的机器类型、版本信息、数据齐全都核过；**安装 + 部署 + 打字没在 ARM 上跑过**。
  详见 `PROGRESS.md` 六·十一。

---

## PlumNext：東風破 /plum/ 的 Rust 重写（2026-10-29 新建 `plum-next/`）

> 上游：`https://github.com/rime/plum`（bash 实现，LGPLv3）。
> 动机：原版在 Windows 上要么依赖 Git for Windows（没装就退化成 ZIP 下载、且**没有增量更新**），
> 要么直连 `https://github.com`（国内经常拉不动，**没有退路**）。
> PlumNext 用 libgit2 拉包 + 镜像自动嗅探/切换，两头都解决。

### 与原版脚本的对应关系（逐句对照 `scripts/*.sh`）

| 原版 | PlumNext |
|---|---|
| `scripts/resolver.sh` | `src/target.rs`：`<user>/<repo>@<branch>:<recipe>:k=v`；`package_list=(...)` 清单解析；`*-packages.conf` URL 展开 |
| `scripts/fetch-package.sh` | `src/fetch.rs`（git2）：`clone --depth 1` / 已存在则 fetch + 硬重置；**每次都把 origin 地址校正成当前镜像**（切镜像不需要重 clone） |
| `scripts/recipe.sh` | `src/recipe.rs`：正经 YAML 解析（`Rx` 声明校验 / `download_files` / `install_files` / `patch_files`），替掉原版用 `sed` 抠区块 + eval |
| `scripts/install-packages.sh` | `src/install.rs`：数据文件挑选（`*.yaml` 排除 `*.custom.*`/`*.recipe.*`/`recipe.yaml`、`*.txt`、`*.gram`、`opencc/*.json|ocd|ocd2|txt`）+ **内容相同则跳过**（原版 `diff -q`） |
| 无（原版写死 github） | `src/mirror.rs`：内置 7 个镜像 + 并发嗅探 + 失败自动切换 + 状态记忆 |

### 镜像表（`src/mirror.rs::builtin()`）

`github`（基准直连）、`ghproxy`、`gh-proxy`、`mirror-ghproxy`、`ghfast`、`gitclone`、`gitmirror`。

* **嗅探**：并发 TCP 建连（443，默认超时 1500 ms），按"可达优先 + 延迟升序"排序。
  只测建连不发包 —— 排序要快，真正的可用性由后续 clone/下载决定（失败就切换）。
* **切换**：`Picker::attempt()` 按排序依次尝试，失败即前进到下一个；全部失败时错误里带 `[镜像id]`，便于定位。
* **记忆**：上次成功的镜像 id 写进 `<plum_dir>/mirror.txt`，下次提到队首（快路径），它挂了再重新嗅探。
* `--mirror <id>` 可强制指定（跳过嗅探），`--sniff` 只打印排序，`--list-mirrors` 列内置表。

### 实测（2026-10-29 00:5x，本机）

```
--sniff:      gh-proxy 96ms < gitclone 133ms < github 254ms < ghfast 283ms
              ghproxy / mirror-ghproxy / gitmirror 不可达
prelude:      下载 配方包 rime/rime-prelude [gh-proxy.com] → 装出 4 个 yaml
ipa 二次运行: 走"更新"路径 → 没有文件更新（幂等）→ mirror.txt = gh-proxy
--mirror ghproxy（坏）: 失败 [ghproxy] clone 失败…（错误带镜像 id）
```

### 待办 / 已知边界

1. `--select` 交互式选包、**子模块**（原版 `--recurse-submodules`）未做。
2. `patch_files` 只实现了"写进 `__patch:` 段 + 同 Rx 幂等替换"，没实现原版 `eval` 选项变量（`key=value`）那套。
3. `:preset` 清单优先读本地 `<plum_dir>/preset-packages.conf`，取不到才联网拉 `rime/plum/master/…`，
   再不行用 `FALLBACK_PRESET`（README 里的 22 个包）兜底。
4. 还没接进设置界面（将来可在"获取更多输入方案"处直接调 `plum-next` 二进制）。

### 与设置界面的接线（同一天补上）

* 设置界面「**获取更多输入方案...**」按钮（`Setting/src/switcher_dialog.rs` 的 `IDC_GET_SCHEMATA = 1211`）
  原先**只定义了常量、点了没反应**。现在接到 PlumNext：拉起同目录的 `plum-next.exe --select :preset`
  （控制台程序，`--select` 要读 stdin，所以**不能**隐藏窗口）。
* 安装要带 `plum-next.exe`：已加进 `script/build-installer.ps1` 的 `$requiredFiles`、
  `script/build-installer-arm64.ps1` 的复制清单、`installer/weasel-next.nsi` 的 `ManagedFile` 与卸载段。
  （x64/arm64 安装脚本都是 `cargo build --release --workspace`，所以产物会自动存在。）
* 装完新方案**不会自动部署** —— 提示用户回对话框点「确定」，由 broker 走正常部署流程。

### 补记（同一天晚些）

* **快路径**：`<plum_dir>/mirror.txt` 里记着上次成功的镜像，下次**跳过嗅探**直接用
  （实测：第二次运行不再打印那 7 行探测）。它挂了照样往下切 ——
  实测把 `mirror.txt` 改成不可达的 `ghproxy` 再跑，自动落到 `gh-proxy.com` 完成克隆。
* **配方语义补了两处**（用上游 `rime-emoji:customize` 真跑出来的差异）：
  1. `install_files:` **段存在但内容为注释**（上游就这样写）⇒ 等价于"什么都不装"，
     **不能**回退成"装包里所有数据文件"。用 `has_install_files` 区分"段不存在"与"段为空"。
  2. `patch_files:` 的键/值要做**变量替换**：`${name}` 与 `${name:-默认值}`，
     变量来自配方 `recipe.args:` 的默认值 + 命令行 `:recipe:k=v`（后者优先）。
     实测 `emoji:customize` 生成 `luna_pinyin.custom.yaml`，内容带 `__patch:` 与 `# Rx: … {}` 块，与上游一致。
  3. `patch_files` 的值可能是**序列**（`- patch/+:`），不能只收字符串；现在会序列化回 YAML 文本再写入。
