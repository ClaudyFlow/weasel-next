# XSIME（犀术 IME）学习笔记

> 来源：`https://github.com/scholay/xsime`（Apache-2.0，1100★，跨平台 IME，底层同为 Rime）
> 学习时间：2026-10-19。本文只记录**能搬到 Weasel-Next 的东西**，不搬运代码。

## 规模对照

| 模块 | 文件 | 体积 |
|---|---|---|
| `platforms/windows/native/workbench` | 60 | 837 KB |
| `platforms/windows/native/peripherals` | 17 | 363 KB |
| `platforms/windows/native/tsf`（TSF DLL） | 32 | 279 KB |
| `platforms/windows/native/broker` | 23 | 168 KB |
| `platforms/windows/native/engine` | 8 | 99 KB |
| `Sources/RimeBuffer`（macOS Swift） | 20+ | — |

结构与 Weasel-Next 一致：**TSF DLL + broker + engine + UI**，只是它用 C++，我们用 Rust。

## 一、文档写法：铁律 + 实测修正

`ARCHITECTURE.md` 骨架值得整段照搬：

```
## 0. 一页速览
## 3. 三条铁律（违反即架构事故）
## 4. ⚠️ 实测修正（v2 核心，接手者必读）
## 7. 仓库地图
## 8. 构建 · 安装 · 调试手册
## 10. 风险登记簿（活文档，解决即划掉）
## 11. 未决问题（留给用户/后续拍板）
```

它的三条铁律：

1. **内部单进程，零轮询式组件 IPC。** 前身把候选 UI 拆到另一进程、经 `state.json` 轮询同步，
   造成「UI 进程没起来 → 打不出字」的脆弱链，永久禁止。
2. **C 桥是 ground truth。** RimeApi vtable 字段顺序是 load-bearing（声明顺序 = 内存布局），
   只允许追加包装函数，不许重排/删减。
3. **上屏唯一出口 + 永活兜底。** 所有文字只经 `Delivery.insert`；引擎不健康或 session==0 时
   必须落到裸 `insertText`，**不存在「返回 false 且丢掉一个可打印字符」的路径**。

> Weasel-Next 的等价铁律候选：
> ① 根部 `*.dict.yaml` 永不删（已踩两次）；
> ② `build\<id>.schema.yaml` 必须扁平（Rime 加载器写死）；
> ③ 部署校验必须覆盖全部产物（漏了 `Setting.exe` 就假报「一致」）。

## 二、组字协议：可能踩了同一个坑

它记录的三个真机 bug（2026-07-04）：

| 现象 | 根因 |
|---|---|
| 候选窗死在屏幕下方 | 无 marked-text 会话时，光标矩形 API 返回零矩形 → 永远走兜底锚点 |
| 微信里「zuoye作业」原始字母残留 | 从不 `setMarkedText`，客户端不知道自己在组字，自行回显原始按键 |
| 切不到并击方案 | 键位表没映射 F4/grave，热键根本没进 Rime |

结论（**组字协议 v2**）：**marked-text 会话必须常驻**。

- `inline` 模式：`setMarkedText(带下划线的 preedit, caret 位置)`；
- `placeholder` 模式（对付 marked text 处理糟糕的 App）：组字期 `setMarkedText("　")`
  —— **全角空格**，半角会让中文基线跳动（Squirrel 同款做法）；
- 提交：有活动 marked 会话时按协议**原子替换** marked 区。

> 对照 Weasel-Next：Tip 侧 `DoEditSession` 用 `GetTextExt` 逐级兜底拿矩形（已修），
> 但**没有常驻 composition 会话**。若某宿主出现「原始字母 + 上屏文字一起出现」，
> 根因大概率就在这里。

## 三、候选矩阵契约（`CANDIDATE-MATRIX.md`）

- 外观设置只影响横/竖紧凑候选，**不改写界面方案或用户词库**；
- `Down` 在引擎有多页候选时打开矩阵，视口最多三页（一页一行），继续按需取页；
- `Left/Right` 在整行内移动，**超宽时滚动露出选中列，不压缩每格**；
- `Esc` 第一下回到打开时的页（保留组字），第二下才走普通取消路径；
- `Space` 提交高亮项；`1–9` 只选当前行可见项；鼠标选择保留**引擎真实页码与候选下标**；
- 失焦/提交/新输入/安全输入框/上下文退役 → 旧候选投影与点击回调立即失效。

> Weasel-Next 的候选窗没有矩阵形态，也没有「页码+下标」的稳定映射。
> 以后做多行候选，这份契约可直接当验收清单。

## 四、安装器：所有权 + fail-closed + 回滚

脚本：`Install.ps1`(7.3KB) `Package.Common.ps1`(36.6KB) `Rollback.ps1` `Verify.ps1`
`Uninstall.ps1` `Restore-Legacy.ps1`；测试：`Test-InstallLifecycle` / `Test-UninstallApp`(20.7KB) /
`Test-RegistrarRecovery` / `Test-SetupSigning` / `Test-BrokerMaintenance` / `Invoke-RimesImeE2E`。

关键做法：

1. **版本化目录**：`<InstallRoot>\versions\<version>-<commit12>-<hash12>`，
   安装状态写 `state.json`（含 `active` 版本、`requiresSignOut`）；
2. **包清单**：`PACKAGE.json` + `Read-VerifiedPackage` 校验每个文件哈希，
   包不完整直接拒绝安装（`-AllowMissingFiles` 才放宽）；
3. **所有权断言**：`Assert-OwnedVersion` / `Assert-OwnedRegisteredPath` /
   `Assert-OwnedBrokerAutostart` —— 只碰自己写的东西；
4. **broker 互斥按用户+会话**：`Get-BrokerMutexName(UserSid, Session)`，
   维护前 `Stop-OwnedBroker` 并建 reservation；
5. **注册表恢复**：读写 `Uninstall\RIMES` 键、开机自启、快捷方式，卸载时按快照还原；
6. **fail-closed**：目标文件内容不同 → 一个都不改，先列冲突；`-PlanOnly` 只报告。

> Weasel-Next 的 `deploy-install.ps1` 现在只比对 7 个二进制的哈希（漏了 `Setting.exe`
> 就假报一致），没有清单、没有版本目录、没有回滚。这是最该补的一块。

## 五、统计口径（`TYPING-METRICS.md`）

分两套数据：**日常统计**（不含正文）与**文章测速**（明确开始/结束）。

```
有效速度 = 最终正确字符数 × 60 / 总秒数          中文 字/分；英文 ÷5 显示 WPM
原始速度 = 成文字符数 × 60 / 总秒数
过程正确率 = 新提交字符中的正确尝试 / 全部尝试   （删旧错字不抹错误）
最终字准 = 正确 / (正确 + 替换 + 漏字 + 多字)
物理键数 = 只计 keyDown；长按重复不算；纯修饰键/导航/快捷键不算
码长 = 物理键数 / 当前成文字数
```

两条**诚实性规则**：

- 无样本的正确率、拿不到的拍数显示「—」，**缺失数据不能在趋势里画成 0**；
- 不从下游文字或按键间隔去猜批次，**拿不到就显示不可用**。

> Weasel-Next 统计页没数据时显示 `0 字/分`，按此规范应为 `—`。

## 六、插件配置 schema 化（`PLUGIN-CONFIGURATION.md`）

```markdown
插件设置必须通过 PluginConfigurationSchema 声明，并由宿主渲染；
不得为单个插件在 SettingsWindow 中继续增加一次性的输入框或持久化分支。
字段类型仅 text / secureText / toggle / choice / number；
每字段必须声明默认值、边界和面向用户的说明；
交叉约束放进 schema validator；错误信息只描述修复方法，不回显被拒绝的值。
```

> `weasel.richschema.json` 方向一致（`ui.fields` + 指针 + 枚举），
> 缺的是「错误信息不回显被拒值」与「交叉字段校验」。

## 七、其它可借鉴

- **Buffer 电音演奏**（`BUFFER-MUSIC.md`）：四弦贝斯指板，
  `1234567890 / QWERTYUIOP / ASDFGHJKL; / ZXCVBNM,./` 四排对应 G2–E3 … E1–C♯2；
  **按物理位置识别**；多键同时发声、同音不同弦各自持权；移调/八度、两轨 Loop、
  BPM 40–240、3/4·4/4·6/8。→ 若以后做「打字当乐器」，映射表可照抄。
- **共存处理**：它的 Windows 数据包 README 明确处理「本机已装官方 Weasel」，
  并**尊重 `HKCU\Software\Rime\Weasel\RimeUserDir` 自定义用户目录**。
  → 我们的部署脚本目前硬编码 `%APPDATA%\Rime-Next`，应读这个注册表值兜底。
- **测试矩阵**：安装/卸载/注册表恢复/签名/broker 维护/E2E 各有独立 PowerShell 测试，
  Weasel-Next 一个都没有（全靠手工部署 + 看日志）。

## 八、落地清单（按性价比）

| 优先 | 做什么 | 收益 |
|---|---|---|
| 1 | 部署校验清单覆盖全部产物（含 `Setting.exe`） | 杜绝「假报一致」 |
| 2 | 统计页缺数据显示 `—`，写 `TYPING-METRICS.md` 定口径 | 数据诚实 |
| 3 | 安装器加 `-PlanOnly` + 清单 + 回滚 | 可审计、可恢复 |
| 4 | 仓库根写 `ARCHITECTURE.md`（含铁律 + 风险登记簿） | 接手成本骤降 |
| 5 | 检查 Tip 是否需要常驻 composition 会话 | 防「原始字母残留」类 bug |
