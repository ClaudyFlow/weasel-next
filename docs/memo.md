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
theme/<名字>/<名字>.theme.toml      主题        （TOML，我们自己解析，不转换）
schema/<名字>/<名字>.schema.toml    方案        （部署时转成 <名字>.schema.yaml 给 librime）
schema/<名字>/<名字>.dict.toml      词典        （部署时转成 <名字>.dict.yaml 给 librime）
dict/<名字>/<名字>.eassy.toml       细胞词库    （按词归并求和 → essay.txt / <名字>.txt）
model/<名字>.gram                   语言模型    （直接放好，librime 原生）
```

- 四个文件夹（`theme` / `schema` / `dict` / `model`）必须存在，程序启动时自动创建。
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

## 程序文件夹与 asset 的结构（已定，待实现）

单一配置文件夹原则：配置只有一份，就是用户目录 `%APPDATA%\Rime-Next\`；
仓库里的 `asset\` 只作**出厂默认**来源，编译时搬进程序文件夹。

```
asset\                     仓库里（出厂默认与程序资源）
  Rime-Next\               ← 出厂默认配置（编译时整个搬到程序文件夹）
      default.eassy.toml   ← 原 essay.txt（细胞词库/语言模型词表，TOML 格式）
      …（schema / dict / theme / model 的默认内容）
  icon\                    ← 程序图标（11 个 .ico，各组件共用 weasel.ico）
  sound\                   ← 键声/上屏声（typewriter-key.wav / typewriter-bell.wav）
  rime-data\               ← 官方 Rime 数据（方案/词典/opencc，供部署时同步）
```

编译/安装：
- `Rime-Next\` →  `C:\Program Files\Weasel-Next\Rime-Next\`（程序文件夹里也有一份）
- 用户实际用的是 `%APPDATA%\Rime-Next\`（唯一的活动配置文件夹）
- 于是不必再考虑"多个配置文件夹"的问题。

文件格式：
- 细胞词库统一叫 `*.eassy.toml`（注意拼写是 **eassy**），出厂默认那份叫 **`default.eassy.toml`**；
- 多份 eassy 由部署流程按词归并（频次求和）后生成 librime 需要的 `essay.txt`（或 `<名字>.txt` + `vocabulary:`）。

安装路径（首字母大写）：`C:\Program Files\Weasel-Next\`；代码目录 `C:\code\Weasel-Next\`。

```
asset\                         仓库里（出厂默认 + 程序资源）
  Rime-Next\                   ← ★ 出厂默认的整棵配置树（编译时整个搬到程序文件夹）
      sound\                   ← 键声 / 上屏声（typewriter-key.wav / typewriter-bell.wav）
      model\                   ← 语言模型（*.gram，如 zh-hans-t-essay-bgw.gram）
      default.eassy.toml       ← 原 essay.txt（细胞词库词表，TOML 格式）
      rime-data\               ← 官方 Rime 数据（方案 / 词典 / opencc）
      …（schema / dict / theme 的默认内容，均为 TOML）
  icon\                        ← 程序图标（11 个 .ico，各组件共用 weasel.ico；构建脚本要用，留在 asset 根）
```

搬迁后程序文件夹的样子（**只有二进制 + 一个配置文件夹**）：

```
C:\Program Files\Weasel-Next\
  weasel-broker.exe · weasel-server.exe · weasel-renderer.exe · weasel-settings.exe
  weasel_tip.dll · rime.dll · libc++.dll · libglog.dll · libopencc.dll · libunwind.dll · WebView2Loader.dll
  themes\                      ← 主题 DLL
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
     b) 调 `weasel-broker.exe --deploy --silent`（现成入口：`broker\src\installer.rs:35`）。
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
2. 因此"插件接口 plugin.dll"只能是我们**自己的宿主**：由 weasel-server 加载 plugin.dll（Rust cdylib，
   导出 C ABI），plugin.dll 再去加载/托管各插件。librime 那条链上的插件不受它影响。
3. "用户文件夹放 Rust 源码"要能**被程序编译**，需要用户机上有 Rust 工具链（重，不可行）→ 实际做法二选一：
   (a) 用户文件夹放**编译好的插件 DLL**（plugin\<名字>\<名字>.dll + plugin.toml 元数据）；
   (b) 放源码仅供开发/查看，真正加载的是 DLL。
   两者都需要先有 (2) 的宿主 plugin.dll。

任务拆分（待做）：
  T1 在 plugin\ 下新增 core 的稳定 C ABI 面（版本号、注册表、生命周期回调）。
  T2 新增宿主 crate（输出 plugin.dll 到程序文件夹），实现：扫描 用户目录\plugin\*\plugin.toml、
     加载其中的 DLL、按 ABI 调用、做版本校验与隔离（崩溃不影响宿主）。
  T3 weasel-server 启动时加载 plugin.dll（可选，缺失则静默跳过）。
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
  S4 weasel-server 启动时确保宿主已加载（可选）。
  S5 安装器把 weasel-plugin.dll 放进程序文件夹；用户目录 plugin\<名>\ 放插件 DLL。

## 插件宿主改名 + "代码不编译直接运行"的结论（已定）

- 宿主 DLL 定名 **`rime-plugin.dll`**（通用命名；替代先前的 weasel-plugin.dll）。

- 为什么"有 ABI"不等于"能跑源码"：ABI 约定的是已编译机器码之间如何互相调用；
  要让**源码**直接运行，宿主里必须有**解释器/JIT**。

- 三条可行路径（宿主按扩展名分派）：
  * `.lua`  → 宿主内嵌 Lua（~200 KB，C ABI 极简）；原版统计插件就是 Lua，用户文件夹放 init.lua 即可直接跑。
     —— 这就是"不编译直接运行"的答案。
  * `.wasm` → 复用本仓库已有的 wasm 运行时（themes\wasm）；仍需一次编译成 wasm，但跨语言且沙箱安全。
  * `.dll`  → 原生插件（C++/Rust），最快，但要编译。

- 目录约定：
  程序文件夹\rime-plugin.dll          宿主（ABI + 内嵌 Lua + 复用 wasm 运行时）
  用户文件夹\plugin\<名字>\init.lua    Lua 插件（免编译）
  用户文件夹\plugin\<名字>\mod.wasm    wasm 插件
  用户文件夹\plugin\<名字>\mod.dll     原生插件
  用户文件夹\plugin\<名字>\plugin.toml 元数据（名字/版本/入口/权限）

- 边界：解释运行的脚本不能直接进 librime 的翻译器链（librime 只认编进去的 C++），
  因此宿主通过 API 表把 Rime 能力（查询/候选/上屏事件等）暴露给脚本；
  "读事件 + 计算 + 出报表"这类插件（统计插件）完全够用。

- 落地顺序调整：
  S1 定 ABI: rime_plugin_host_api（版本/日志/Rime 查询/事件订阅/定时器）+ rime_plugin_entry。
  S2 新建宿主工程 → rime-plugin.dll（含 Lua 绑定；wasm 复用现有运行时）。
  S3 rime.dll 的 C++ 胶水层 LoadLibraryW("rime-plugin.dll")；找不到则退化为内置实现。
  S4 weasel-server 启动时确保宿主已加载。
  S5 安装器放 rime-plugin.dll；用户目录 plugin\<名>\ 放插件代码；示例 Lua 插件随包提供。
