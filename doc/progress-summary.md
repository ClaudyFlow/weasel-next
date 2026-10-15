# Weasel-Next 进度总结（2026-10-15）

给下一次接续用。凡是标了「实测」的都是跑出来的结果，标了「未验证」的不要当成已完成。

---

## 一、约束（必须遵守）

1. **只允许 GNU 工具链**。llvm-mingw，宿主 `x86_64-pc-windows-gnu`，不用 MSVC、不带 vcredist、不产出 pdb。
2. **只在程序文件夹运行**。代码在 `C:\code\Weasel-Next` 编译，NSIS 打包，装到 `C:\Program Files\Weasel-Next\` 后只从那里运行。
3. **只保留 64 位**，不生成 x86/x64 子目录。
4. **禁止链接**（junction / hardlink）——用户目录里只能有真实文件。
5. 排版与文案照官方（fxliang/weasel 在 `C:\Code\weasel`）。

---

## 二、当前可分发状态

安装包：`artifact\installer\Weasel-Next-0.18.0-x64-setup.exe`，23.17 MB，10-15 11:39
SHA256：`0ae29a29c8722f8d8dfa36f734d74850b5385f23204da0abb46c00e2a2902ee4`

装完实测（退出码 3010 = 成功、需重启）：

- 程序文件夹 104 个文件：四个 exe、`Weasel-Next.dll` 5.08 MB、`rime.dll` 9.62 MB、
  `weasel_theme_official.dll`、六个 MinGW 运行时、`Uninstall.exe`
- 数据目录 `Rime-Next\`：eassy / icon / opencc / plugin / schema / sound / theme 七个分类，75 个文件
- TIP 注册：`HKLM\SOFTWARE\Classes\CLSID\{16A7AEA9-9EE6-4540-8020-E384C0489BB1}\InprocServer32`
  → `C:\Program Files\Weasel-Next\Weasel-Next.dll`，CTF TIP 项也在
- 卸载项：小狼毫Next · 0.18.0
- 三个设置对话框都能稳定打开

**注意**：改了 broker 可执行文件名的尝试**已回退**（原因见第五节），所以包和源码都还是 `Weasel-Next-Server.exe`。

---

## 三、已经能用的

**打字链路通了**（用户实测）。TIP 已从 `weasel_tip.dll` 改名为 `Weasel-Next.dll` 并生效。

**`/style` 界面风格设定**——唯一数据完整的功能。36 套配色（碧水／Aqua、安卓／Android、
青天／Azure…）、自绘颜色列表（带色块）、切换即写
`%APPDATA%\Rime-Next\weasel.custom.yaml` 的 `patch: style/color_scheme`。
数据层在 `style\src\settings.rs`，22 个单元测试全过。

**`/switcher` 方案选单设定**——**本轮修好**（提交 `8f7fb30`）。12 个方案能进列表、
前 8 个带勾。根因见第四节第 1 条。

**`/dict` 用户词典管理**——只有官方模板，四个按钮（输出/合入词典快照、导出/导入文本码表）都没接逻辑。

**部署与分类**——`--deploy` 跑通（9 success 0 failure），产物按方案归入
`build\<方案>\`，读取端与写入端规则一致。librime 有两处补丁，导出在
`librime\weasel-next.patch`。

---

## 四、关键教训（这几条最值钱，别再重新踩）

### 1. Rust 的 `match` 分支里，未定义的常量会被当成绑定变量

`switcher_dialog.rs` 里 `WM_INITDIALOG_` 这个常量**没定义**，于是

```rust
match message {
    WM_INITDIALOG_ => { /* 填充列表 */ }
    WM_NOTIFY_ => { ... }
}
```

第一个分支变成**匹配一切的模式**，填充代码在**任意消息**下都会执行——包括 ListView 正在处理
`WM_NOTIFY` 的时候，控件于是拒绝插行，`LVM_INSERTITEMW` 恒返回 -1。

**线索一直在编译警告里**：`variable 'WM_CLOSE_' should have a snake case name`——Rust 在说
"这是变量不是常量"。当时把它当无关警告忽略了，白查了很久。

修法：补齐常量 + 用 `PostMessageW` 把填充推迟到对话框就绪之后（补了 `PostMessageW` 绑定）。

### 2. `embed_resource` 的返回值不能丢

之前写 `let _ = embed_resource::compile(...)`，把 `.rc` 编译错误**吞掉了**：构建显示成功，
运行时三个对话框全部创建失败（`DialogBoxParamW` 返回 -1）。改成 `expect` 后立刻暴露
`windres failed to compile`（`.rc` 里缺 `#define IDC_SCHEMA_LIST` 等）。

### 3. 批量注释代码会留下悬空判断

把 x86 的 `regsvr32` 那行注释掉后，**紧跟的判断没删**：`StrCmp $0 0 0 install_failed`
检查的 `$0` 是上一条命令的残留值，于是必然跳进失败分支，弹出"安装或 TIP 注册失败"
（`weasel-rs.nsi:531`）。卸载段有同样的 bug，会导致卸载也失败。

### 4. 本机自测通不代表外网通

起 HTTP 服务分享文件时，本机 curl 拿环回地址一律 200，但外部（Linux 机器）连不上：
0 字节、0 速度、**curl 不报错**——典型的被上游静默丢包。校园网很可能封了入站。

### 5. 两个输入法并存

机器上同时装着官方小狼毫 0.17.4（`C:\Program Files\Rime\weasel-0.17.4\WeaselServer.exe`，
托盘图标一直在）和我们的。**右键很容易点成官方的**——官方菜单也有设置/重新部署/重启/退出，
打开的是官方 WeaselDeployer，与我们代码无关。用户的"右键还是旧的黑色界面"就是这个原因。

区分办法：看进程路径，或从程序文件夹直接跑
`Weasel-Next-Setting.exe /style`（`/switcher`、`/dict` 同理）。

### 6. DPI 不感知的进程截图会抓错区域

`GetWindowRect` 返回逻辑坐标、`CopyFromScreen` 用物理像素，两者对不上，抓到的是桌面。
要在捕获进程里先设置 DPI 感知。

---

## 五、没做完的（按优先级）

### 1. 候选窗看不见（最影响使用）

`Weasel-Next-Renderer.exe` 启动后约 0.3 秒打印 `UI event channel closed` 然后**退出码 1** 退出，
broker 的监控每 16～30 秒重新拉起一次，一直循环。所以窗口没机会画出来。

日志在 `%LOCALAPPDATA%\Rime-Next\Logs\renderer.*.log` 与 `broker.*.log`。

下一步：查通道是谁先关的。那句话是"给 TIP 传 UI 事件的通道被关闭了"，**多半是 TIP 在一次
按键结束后就把通道收了**，不是渲染器自己挂。要读 `renderer\src\ui_runtime.rs` 和 TIP 侧的通道管理。

### 2. broker 改名成 `Weasel-Next-Server.exe`（用户要求，未完成，已回退）

为了和官方 `WeaselServer.exe` 区分。做到一半卡在编译：

```
error: invalid instruction `cargo:rustc-link-arg-bin` from build script of `Weasel-Next-Server`
```

该指令由 `build_support\manifest.rs` 发出（`cargo:rustc-link-arg-bin={binary}=/MANIFEST:EMBED`
等三行），`{binary}` 由 `broker\build.rs` 传入。已经把传入名改成新名字仍被拒绝；
`cargo clean -p Weasel-Next-Server` 报 `Removed 0 files`（bin 目标换了名，包名匹配不到）。

下一步：先 `cargo metadata` 确认 `[[bin]] name = "Weasel-Next-Server"` 之后 cargo 认的
target 名到底是什么（连字符可能被规范化），再改 `manifest.rs` 的调用。

改动清单（回退前的做法，参考）：`broker\Cargo.toml` 加 `[[bin]]`（**包名保持
`Weasel-Next-Server` 不动**，免得破坏 `build_support\version.rs` 里按包名查版本信息的映射）；
全局替换 `Weasel-Next-Server.exe` 27 处；按进程名杀进程的 6 处；日志标签 14 处。

### 3. `/dict` 的四个功能

官方走 levers API，我们没有。四件事：输出词典快照、合入词典快照、导出文本码表、导入文本码表。

### 4. 语言栏右键菜单

官方是 `LanguageBar.cpp` 432 行实现的 `ITfLangBarItemButton`（`InitMenu`/`OnMenuSelect`/
`OnClick`）。**我们的 TIP 里这个实现是零**（`tip\src\*.rs` 搜 `LangBar`、`TF_LBI` 命中 0 处），
所以不是菜单坏了，是根本没有语言栏按钮对象。

官方菜单 12 项及命令 ID：Settings 40008、Dictionary manager 40010、User data sync 40012、
User folder 40007、Program folder 40006、Log folder 40016、Help 40009、Forum 40004、
Check for updates 40003、Deploy 40002、Restart 40015、Exit 40001。

我们 broker 的托盘菜单（`common\src\command_menu.rs` 的 `ITEMS`）目前只有 12 项，缺词典管理与方案选单。
**做完必须重启才能验证**（TIP 被 explorer 加载）。

### 5. 一次重启

`PendingFileRenameOperations` 里还有旧 TIP 的退休任务与 `.old.nsh*.tmp` 残留
（那两个文件被 explorer 占用，删不掉）。重启后才会清理干净。

---

## 六、待提交

工作树**干净**（最后一次提交 `afd6efd`，共 306 个提交）。本次会话的改动都已落盘：

- `8f7fb30` 修 `/switcher` 列表插不进行 + 版本号 0.18.0 + TIP 改名 Weasel-Next.dll
- `afd6efd` 托盘菜单的设置项带 `/style` 参数（打开官方界面而非旧界面）
- `0fd6d5a` 只保留 64 位，清掉 installer 里全部 x86/x64 残留
- `bea2655` 修资源编译静默失败（根因）
- 其它：`32e4980` 部署产物按方案分类、`c3ff2a1` librime `relocate_target` 归位、
  `7958d38`/`aee16c2` 发行测试说明等

未跟踪：`tool\aria2-next\`（aria2-next 3.0.3 Windows x64，18,225,664 字节，
SHA256 `b2f2f7a9d6f120a1dccd2d632df60e4d2d54b3c67ac822bd3fbfeb98d2be9465`）。
要不要进仓库未定；按 `tool\7z\` 的先例应当作构建输入、由安装包负责分发到程序文件夹。

---

## 七、实用命令

```powershell
# 编译全部
cd C:\code\Weasel-Next; $env:LIBCLANG_PATH='C:\Program Files\llvm-mingw\bin'; cargo build --release

# 只编设置程序（约 2.5 分钟）
cargo build --release -p Weasel-Next-Setting

# 打包
$env:Path = "C:\Program Files (x86)\NSIS;" + $env:Path
powershell -File script\build-installer.ps1

# 提权运行（sudo 是强制新窗口模式，输出要写文件再读）
Start-Process 'C:\WINDOWS\system32\sudo.exe' -ArgumentList '"C:\path\to\script.cmd"' -Wait

# 三个官方对话框
& 'C:\Program Files\Weasel-Next\Weasel-Next-Setting.exe' /style     # 界面风格设定
& 'C:\Program Files\Weasel-Next\Weasel-Next-Setting.exe' /switcher  # 方案选单设定
& 'C:\Program Files\Weasel-Next\Weasel-Next-Setting.exe' /dict      # 用户词典管理

# librime 增量重编（约 1 分钟）
powershell -File librime\build-librime-mingw.ps1
# 产物 C:\Code\librime\build-mingw\bin\librime.dll → 复制到程序文件夹（栈要先停）

# 日志
%LOCALAPPDATA%\Rime-Next\Logs\{broker,server,renderer,deploy}.<日期>.log
```

---

## 八、改名尝试的追加发现（2026-10-15 第二次尝试后）

第二次尝试从「清空 broker 构建脚本缓存」入手，仍然失败：

```
error: invalid instruction `cargo:rustc-link-arg-bin` from build script of `Weasel-Next-Server`
```

**关键发现**：`build_support\manifest.rs` 第 21 行是 `if toolchain::is_msvc_target()`——
那三条 `cargo:rustc-link-arg-bin={binary}=...` **只在 MSVC 目标下发出**；
GNU 分支编译 `RT_MANIFEST` 资源（第 33 行起），发完就 `return`（第 30 行）。

也就是说 **GNU 构建根本不该产生这个指令**。既然 cargo 还在报它，说明**另有一个辅助模块**
（多半是 `build_support\icon.rs` 或 `build_support\version.rs`，两者都按 bin 名发资源指令）
在发同样的指令，而我一直改错了地方（只在 `manifest.rs` 的调用点改名字）。

**下次接手的第一步**：

```powershell
Get-ChildItem build_support,broker -Recurse -File -Filter *.rs |
  Select-String -Pattern 'rustc-link-arg-bin'
```

把**所有**发出点找齐（不是只看 manifest.rs），确认每个点传的 bin 名 —— 只要有一处传了
`Weasel-Next-Server` 而实际 bin 叫 `Weasel-Next-Engine`，cargo 就会拒绝整个构建脚本的输出。

**另外两个本次踩到的坑**：

1. 全局替换 `"Weasel-Next-Server` → `"Weasel-Next-Engine` **会误伤 `Cargo.toml` 的
   `name = "Weasel-Next-Server"`**（包名），导致 cargo 报
   `package ID specification 'Weasel-Next-Server' did not match any packages`。
   包名必须保持不动，替换时要把 `[package] name` 排除在外。
2. 往 `broker/Cargo.toml` 插 `[[bin]]` 时若用 `Replace("[dependencies]", ...)`，
   会把后面的 `serde_json = "1"` 挤进 `[[bin]]` 段里、破坏清单结构。
   `[[bin]]` 要追加在**文件末尾**。

**当前状态**：改名已再次回退，全量编译通过（`Finished release profile`），
工作树干净，安装包仍是 `Weasel-Next-Server.exe` 那一版。
---

## 九、改名方案定稿（第三次，正确姿势）

### 根因（终于查清）

`build_support\toolchain.rs` 只有一行：`CARGO_CFG_TARGET_ENV == "msvc"`。
我们的目标是 `x86_64-pc-windows-gnu`，该值为 `"gnu"`，所以
**`manifest.rs` 里那三条 `rustc-link-arg-bin` 在我们的构建里根本不发出**。

全仓库只有两处发出点：`manifest.rs`（MSVC 专用分支）与 `setting\build.rs`
（硬编码 `Weasel-Next-Setting`，与它自己的 bin 名一致，所以它一直没事）。

**结论：`Weasel-Next-Server` 报的无效指令来自 `embed-resource`**——它是唯一的外部构建依赖，
发的 `cargo:rustc-link-arg-bin=` 用**包名**而非 `[[bin]]` 的名字。所以只改 bin 名、
保留包名就会不一致，cargo 拒绝整个构建脚本的输出。前两次失败都是这个原因。

### 正确做法：**构建后改名，不碰 Cargo**

TIP 改名（`weasel_tip.dll` → `Weasel-Next.dll`）用的就是这个办法，一次就成。
四个 exe 照做：

1. **不动任何 `Cargo.toml`**（包名、bin 名全保持）——`embed-resource` 与
   `setting\build.rs` 的硬编码名字都继续有效，坑不存在。
2. `script\build-installer.ps1` 里加复制步骤（照 TIP 那句 `Copy-Item` 的写法）：
   `target\release\Weasel-Next-Server.exe` → `Weasel-Next-Engine.exe`，其余同理。
3. 必需文件清单改用新名字。
4. `.nsi` 里所有 `ManagedFile` / 快捷方式 / `Delete` / `DisplayIcon` 改用新名字。
5. **运行时按名字找进程与拉子进程的地方**（这是最容易漏的）：
   `broker\src\service_supervisor.rs`、`operations.rs`、`installer.rs`、
   `installer\processes.nsh`、`script\deploy-install.ps1`、`deploy-and-normalize.ps1`
   里那些 `Get-Process -Name 'weasel-...'` 与 spwan 用的文件名。

### 名字（待用户最终确认）

| 现在 | 改成 |
| --- | --- |
| `Weasel-Next-Server.exe` | `Weasel-Next-Engine.exe`（用户已选） |
| `Weasel-Next-Engine.exe` | `Weasel-Next-Engine.exe`（待确认；它是 librime 引擎宿主） |
| `Weasel-Next-Renderer.exe` | `Weasel-Next-Renderer.exe` |
| `Weasel-Next-Setting.exe` | `Weasel-Next-Settings.exe` |
| `weasel_tip.dll` | `Weasel-Next.dll` ✓ 已完成 |

注意官方 `WeaselServer.exe` 是**服务宿主**，职责上对应我们的 broker；名字怎么分由用户定，
但**不能出现两个同义的名字**（否则又和官方混淆）。

### 验证顺序

编译（必须过）→ 打包（确认包内是新名字、旧名字 0 条）→ 提权静默安装 → 检查程序文件夹
与注册表 → 三个对话框各开一次 → 打字确认候选窗与上屏。
---

## 十、改名失败的根因（探针实验，已定性）

用探针实验一次问出了答案：给 `server\Cargo.toml` 临时加一段

```toml
[[bin]]
name = "Probe"
path = "src/main.rs"
```

编译后 cargo 给出**完整**错误（之前一直只看到第一行）：

```
error: invalid instruction `cargo:rustc-link-arg-bin` from build script of `Weasel-Next-Engine`
The package Weasel-Next-Engine v0.18.0 does not have a bin target with the name `Weasel-Next-Engine`.
```

**结论**：某个构建期模块按**包名**拼 `cargo:rustc-link-arg-bin=<名>=...`。
而**一旦写了显式 `[[bin]]`，cargo 就不再按包名自动生成隐式 bin 目标**——
于是指令指向一个不存在的目标，cargo 拒绝整个构建脚本的输出。

这解释了三次失败的全部现象：只改 bin 名（包名不动）→ 不匹配；
加 `[[bin]]` 改名（包名不动）→ 包名那个隐式目标消失 → 不匹配；
而 `setting` 一直没事，因为它的包名与 bin 名相同、且它自己在 `build.rs` 里
硬编码的 `rustc-link-arg-bin=Weasel-Next-Setting` 也是对的。

**正确解法（二选一，都不需要"打包时复制"）**：

1. **只改包名、不写 `[[bin]]` 段**——cargo 的隐式 bin 自动跟着包名，
   指令自然匹配，产物直接叫新名字。最干净（代价：包名若含大写会有命名警告）。
2. **包名与 `[[bin]] name` 写成同一个新名字**——等效，多写一段。

配套必须一起改（漏一处就编译不过或运行不起来）：

- `build_support\version.rs` 的**键**＝包名（值放新文件名）
- 各 `build.rs` 里 `manifest::embed("<包名>", ...)` 的参数
- `setting\build.rs` 里硬编码的 `rustc-link-arg-bin=Weasel-Next-Setting`
- `.nsi` 的 `ManagedFile` 源与目标、快捷方式、`DisplayIcon`
- 按进程名杀进程处：`installer\processes.nsh`、`script\deploy-*.ps1` 的 `Get-Process -Name`
- 运行时拉子进程/找进程的名字：`broker\src\service_supervisor.rs`、`operations.rs`、`installer.rs`

**命名已定**（用户确认，含职责纠正）：

| 现在 | 改成 | 说明 |
| --- | --- | --- |
| `Weasel-Next-Server` | `Weasel-Next-Server` | 名副其实的服务端：托盘、监管子进程、设置下发、自启动、优雅退出 |
| `Weasel-Next-Engine` | `Weasel-Next-Engine` | librime 引擎宿主（叫 server 是误导） |
| `Weasel-Next-Renderer` | `Weasel-Next-Renderer` | 候选窗渲染 |
| `Weasel-Next-Setting` | `Weasel-Next-Setting` | 设置界面（去复数） |
| `weasel_tip` | `Weasel-Next.dll` | ✓ 已完成 |

**下一步执行顺序**：改包名（方案 1）→ 同步上面六类引用 → 编译（必须过）→
打包 → 确认包内是新名字且旧名字 0 条 → 提权静默安装 → 三个对话框各开一次 → 打字验证。
---

## 十一、当前三个未修缺陷（2026-10-15 晚）

### 1. 方案勾选只保存一个（用户报"还是不记录方案"）

`%APPDATA%\Rime-Next\default.custom.yaml` 实际内容只有一项：

```yaml
patch:
  schema_list:
    - schema: luna_pinyin
```

出厂 `default.yaml` 的 `schema_list` 有 8 项。**文件写成功了，是读勾选状态只读到一个。**

根因：`switcher_dialog.rs` 在 `IDOK` 时才用 `LVM_GETITEMSTATE` 逐个问控件要勾选状态，
那是**把结果编码在返回值里的跨消息读取**，12 行里只有第一行取到有效值，其余被判为"未勾选"。

**修法**：不要在最后一刻去问控件，而在**列表内容变化时就维护一份内存状态**
（插入时初始化、`LVN_ITEMCHANGED` 里更新），`IDOK` 时直接写这份内存数据。

### 2. `/style` 右侧的「样式设置」列表是空的（用户报"皮肤设置不全"）

官方 `IDD_STYLE_SETTING` 有**两个列表**：

- 左 `IDC_COLOR_LIST`（7,43,190,160）——当前配色的颜色键，配 `IDC_COLOR_VALUE` + 「修改」
- 右 `IDC_STYLE_LIST`（205,20,285,183）——**样式键**（字体、字号、layout/... 等），
  配 `IDC_STYLE_VALUE_COMBO` / `IDC_STYLE_VALUE_EDIT` + 「设置」

我们的 `dialogs.rs` **只填了左边**（`COLOR_KEYS`），右侧 `IDC_STYLE_LIST` 从未填过任何内容，
所以打开是空白。官方在 `UIStyleSettings.h` 里有一张样式键表，要照着搬过来、
按当前 `weasel.yaml` 的 `style:` 段取值，并用 `WM_MEASUREITEM`/`WM_DRAWITEM` 自绘。

### 3. 候选窗/打字（最影响使用）

`Weasel-Next-Renderer.exe` 启动约 1 秒后打印 `UI event channel closed`，**退出码 1** 退出，
broker 的监控每 20 秒重新拉起一次，一直循环：

```
13:05:35.85  starting (mode=Live)
13:05:35.85  using renderer theme official
13:05:36.86  UI event channel closed     ← 退出
13:05:52.92  starting (mode=Live)        ← 被重新拉起
```

"通道断了"是**另一端关的**，不是自己崩。多半是 **TIP 在一次按键结束后就把通道收了**。
这是"打不了字"和"看不见候选窗"的共同根因。

排查入口：`renderer\src\ui_runtime.rs` 与 TIP 侧的通道管理。

### 4. 附带缺陷

引擎往**程序文件夹**写生成物被拒（普通用户对 `Program Files` 没有写权限）：

```
WARN Weasel-Next-Engine::librime: eassy: 写出 C:\Program Files\Weasel-Next\Rime-Next\eassy\essay.txt 失败: 拒绝访问。(os error 5)
```

`essay.txt` 是部署时由 `eassy.toml` 归并生成的运行期产物，应改写到用户目录。

### 状态

- 设置程序默认入口：`/switcher`（先选方案），按「中」后串联打开 `/style`（已实现并安装）
- 「中」按钮位置：已挪到右下角（实测相对左=527、右距=23，对话框 650 宽）✓
- 设置快捷方式：开始菜单「小狼毫Next」下 5 条 + 算法服务 + 卸载 ✓
- `/legacy` 进旧 Slint 界面（备用）✓
### 1b. 修正：根因不在写端（2026-10-15 深夜读码后）

读了 `set_schema_list`（`style\src\switcher.rs:206`）——**写端是正确的**：
它遍历 `ids` 逐条写出（L239-241），没有任何截断。

问题在 `switcher_dialog.rs` 的 IDOK 分支（L473-483）：

```rust
let selected: Vec<String> = state
    .rows
    .iter()
    .zip(checked.iter())        // ← zip 以短的一侧为准
    .filter(|(_, on)| **on)
    .map(|(row, _)| row.id.clone())
    .collect();
```

`checked` 由 `for index in 0..state.rows.len()` 逐行读 `LVM_GETITEMSTATE` 得到。
文件里恰好只有 `luna_pinyin` 一项，**反推出那一刻 `checked` 只有 1 个元素**，
再反推出 **`state.rows.len()` 在当时是 1**（尽管控件里确有 12 行，实测 `LVM_GETITEMCOUNT=12`）。

`zip` 以短侧为准，于是 12 行被静默截断成 1 项 —— 数据就是这样丢的。

**三层修法（缺一不可）**：

1. IDOK 时按**控件的行数**读，不依赖 `state.rows`：
   `let count = SendMessageW(list, LVM_GETITEMCOUNT, 0, 0).0 as usize;` 再 `for index in 0..count`。
2. 别用 `zip` 配对：`state.rows.iter().enumerate().filter(|(i, _)| checked.get(*i).copied().unwrap_or(false))`，
   越界当未勾选 —— 一侧短了也不会静默丢数据。
3. **根治**：勾选状态在**插入时**就记进内存（`state.rows` 每行加 `selected: bool`，
   `LVN_ITEMCHANGED` 里同步更新），IDOK 直接写内存那份，**完全不问控件**。
   控件的勾选状态不该在最后一刻去读。

### 1c. 下一步的顺序（建议）

1. 修 `state.rows` 为什么只有 1 行（这是 1b 反推出的结论，值得单独确认：
   查插入循环是否真的把 12 行都 push 进 `state.rows`，以及填充是否只跑了一轮、
   `initialized` 标志与 `WM_APP_FILL` 的交互是否让第二轮跳过了 push）。
2. 按 1b 的三层修法改勾选保存。
3. `/style` 右侧样式键列表（照官方 `UIStyleSettings` 的样式键表搬）。
4. 渲染器通道生命周期（打字与候选窗的共同根因）。