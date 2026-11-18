# rime/home 项目收录 issue（2026-11-18 已提交，均无标签）

> 参考样本：`rime/home` #2238（librime-evo）、#2239（万象拼音）、#2240（万象词库与语法模型），
> 均为 2026-11-18 由 amzxyz 新建的「项目收录」型 issue；标签口径依据维护者 lotem 的
> [#2237](https://github.com/rime/home/issues/2237)「通過 issue label 彙總 Rime 下游項目介紹頁」。
> 本文记录写法与已提交的三份正文；按用户要求，三个 issue 均**不带任何 GitHub 标签**（labels 数组为空）。
> 已提交：[#2241 Rime-Next 引擎](https://github.com/rime/home/issues/2241)、[#2242 锈狼嚎](https://github.com/rime/home/issues/2242)、[#2243 PlumNext](https://github.com/rime/home/issues/2243)。

## 一、三个样本共同写法（可复制的模板）

1. **标题**：`项目中文名（仓库名）` 或 `项目名 — 一句话定位`。例：
   - `librime-evo — Rime 输入法引擎增强分支`
   - `万象拼音（rime-wanxiang）`
   - `万象词库与语法模型（rime-wanxiang-data）`
2. **正文开头**：与标题完全相同的 `##` 二级标题。
3. **元数据用加粗行逐条列出**（每条独立成行、行间空行），字段按需要取用：
   - `**项目主页：** <仓库 URL>`
   - `**使用文档：** <文档 URL>`（可选）
   - `**上游项目：** <上游 URL>`（衍生项目必写）
   - `**项目类型：** Rime 核心引擎衍生项目（`hall of fame`）` / `Rime Windows 输入法前端（`hall of fame`、`frontend`）` / `Rime 配方/配置管理工具（`hall of fame`、`tools`）` / `Rime 拼音输入方案（`data project`）` / `Rime 词库、语法模型与数据构建工具（`data project`）`
   - `hall of fame` 收录 Rime 衍生项目；下分 `frontend`（输入法前端）、`plugin`（插件）、`tools`（工具软件）；`data project` 单列输入方案/词库/配置/文档类数据项目（#2237 的正式定义）。**没有 `engine` 这个标签**：引擎衍生项目按 #2238 的写法归入 `hall of fame`。
   - `**开发语言：** C++`（可选，样本 #2238 有）
   - `**安装与下载：**` / `**数据及模型下载：**`（releases URL，可选）
   - `**源码及版本发布：**`（#2238 用）
4. **正文 = 2～4 段短说明**：定位一句话 → 分点列主要改进/能力（3～6 条，每条一句）→ 说明是否独立前端/适用范围/版本形态 → 授权与「非官方衍生分支」声明（#2238）。
5. **结尾固定两句**：`**问题反馈：** <issues URL>` +「有关……的问题，请前往项目自身的 Issues 反馈，勿在 rime/home 或项目收录页面讨论。」（样本三篇措辞一致）。
6. 无标签、无 assignee、无 comment；正文只用 Markdown 标题/粗体/列表/链接，不放截图。

## 二、草稿 A：Rime-Next 引擎（librime-next）

> 已提交为 https://github.com/rime/home/issues/2241

````text
## Rime-Next 引擎（librime-next）

**项目主页：** https://github.com/ClaudyFlow/librime-next

**上游项目：** https://github.com/rime/librime

**项目类型：** Rime 核心引擎衍生项目（`hall of fame`）

**开发语言：** Rust

Rime-Next 是 [librime](https://github.com/rime/librime) 的 Rust 移植版，目标是产出与原版
`rime.dll` ABI 兼容的替代动态库，让现有 Rime 前端无需修改头文件即可替换引擎。

项目的主要改进方向包括：

- **ABI 兼容层**：`rime_get_api()` 导出的 `RimeApi` 字段逐字对齐上游 `rime_api.h`，并用 ABI 校验工具对照原版头文件。
- **配置与词典层**：重写 YAML 配置装载/`__include`/`__merge`/`__patch`，以及 prism、table、reverse、字符串表等词典数据结构。
- **引擎与处理器**：Session / Context / Composition / Menu 与 speller、selector、script translator 等 gear 按上游语义实现。
- **部署层**：按上游 deploy 任务模型重写资源分类与增量编译，支持多方案并行部署。
- **可测试性**：保留真实 Rime 数据快照与端到端用例，便于与上游行为对拍。

项目不是独立输入法前端，也不绑定某一套输入方案；它替换的是 Rime 引擎核心，可配合
Weasel-Next 等前端使用。

Rime-Next 为社区独立维护的衍生项目，**并非 Rime 官方版本**。项目保留上游版权声明，遵循
BSD 3-Clause 开源许可证，并持续跟进上游变更。

**源码及版本发布：** https://github.com/ClaudyFlow/librime-next/releases

**问题反馈：** https://github.com/ClaudyFlow/librime-next/issues

有关引擎实现、ABI 兼容与部署行为的问题，请前往项目自身的 Issues 反馈，勿在 rime/home
或本收录页面讨论。上游 librime 的通用问题请优先向上游项目反馈。
````

## 三、草稿 B：锈狼嚎（Weasel-Next）

````text
## 锈狼嚎（Weasel-Next）

**项目主页：** https://github.com/ClaudyFlow/Weasel-Next

**上游项目：** https://github.com/rime/weasel

**项目类型：** Rime Windows 输入法前端（`hall of fame`、`frontend`）

**开发语言：** C++ / Rust

锈狼嚎（Weasel-Next）是基于 Rime 输入法引擎的 Windows 前端，面向 Windows 10 1809
及以上版本。它保留小狼毫的使用方式，同时把部署、候选窗渲染与设置界面逐步迁移到 Rust
组件，并配套自研的 Rime-Next 引擎（librime-next）与配方管理器 PlumNext。

项目的主要改进方向包括：

- **候选窗渲染**：基于 DirectWrite / Direct2D 重写绘制路径，支持主题配色、材质档位与多套皮肤。
- **设置界面**：提供 Slint 与 Win32+D2D 两套前端，覆盖方案、皮肤、常规、外观、统计、部署等页面。
- **部署链路**：方案与词典的选择性编译、增量台账与进度反馈；用户目录与共享数据目录分离。
- **输入辅助**：两级方案菜单、辅助码、符号面板、剪贴板面板与用户词库同步入口。
- **配置管理**：内置 PlumNext 配方管理器，可直接在 Windows 上安装/更新 Rime 配方包。

项目是完整的输入法前端，需要配合 Rime 引擎与输入方案数据使用；不兼容 C++ 版小狼毫的
`weasel.toml` / `weasel.custom.toml` 配置格式。

锈狼嚎 为社区独立维护的衍生项目，**并非 Rime 官方版本**。项目使用 BSD 3-Clause
开源许可证，第三方组件许可清单见仓库 `THIRD-PARTY-LICENSE.txt`。

**源码及版本发布：** https://github.com/ClaudyFlow/Weasel-Next/releases

**问题反馈：** https://github.com/ClaudyFlow/Weasel-Next/issues

有关安装、界面、配置及与 Rime-Next 引擎配合的问题，请前往项目自身的 Issues 反馈，
勿在 rime/home 或本收录页面讨论。上游小狼毫的通用问题请优先向上游项目反馈。
````

## 四、草稿 C：PlumNext（plum-next）

````text
## PlumNext（plum-next）

**项目主页：** https://github.com/ClaudyFlow/plum-next

**上游项目：** https://github.com/rime/plum

**项目类型：** Rime 配方/配置管理工具（`hall of fame`、`tools`）

**开发语言：** Rust

PlumNext 是 [rime/plum（東風破）](https://github.com/rime/plum) 的 Rust 重写版：一个用于安装、更新 Rime 输入方案与词库配方包的命令行工具。原版是一组 bash 脚本，本项目在保持配方订单（Rx）语义兼容的同时，针对 Windows 上的两个常见痛点做了重做。

项目的主要改进方向包括：

- **不依赖 Git for Windows**：用 libgit2 直接克隆/拉取配方包；已存在的包走 `fetch + 硬重置` 增量更新，不再退化成无增量能力的 ZIP 下载。
- **镜像自动嗅探与切换**：内置 7 个镜像，按建连延迟排序，失败立即切换下一个；上次成功的镜像会被记住，换源不需要重新 clone。
- **配方订单兼容**：保留 `<用户>/<仓库>@<分支>:<配方>:key=value` 的 Rx 语法与 `:preset` / `:extra` / `:all` 预设清单，以及交互式选包（`--select`）。
- **用户目录一致性**：默认 Rime 用户目录为 `%APPDATA%\Rime-Next`，与锈狼嚎（Weasel-Next）保持一致；也可作为其设置界面里的配方管理器使用。
- **配方参数补丁**：`patch_files` 把参数写进方案的 `__patch:` 段，并对同一配方订单幂等替换。

项目是独立的命令行配方管理器，不包含输入方案或词库数据本身；配方包由各自仓库发布，许可证请以各包自己的说明为准。原版脚本的部分能力（git 子模块递归、`eval` 选项变量）尚未实现。

PlumNext 为社区独立维护的衍生项目，**并非 Rime 官方版本**。代码沿用上游 rime/plum 的授权，遵循 LGPL-3.0-or-later 开源许可证，全文见仓库 `LICENSE`。

**源码及版本发布：** https://github.com/ClaudyFlow/plum-next/releases

**问题反馈：** https://github.com/ClaudyFlow/plum-next/issues

有关配方安装、镜像与更新行为的问题，请前往项目自身的 Issues 反馈，勿在 rime/home 或本收录页面讨论。上游東風破的通用问题请优先向上游项目反馈。
````

## 五、两处待定项已经按 rime/home 口径定稿

1. **仓库名大小写**：`ClaudyFlow/Weasel-Next` 与 `ClaudyFlow/weasel-next` 是同一仓库的大小写别名；草稿统一用大写 `Weasel-Next`，与仓库 README、GitHub API 返回的 `full_name` 一致。
2. **项目类型标签**：#2237 定义了 `hall of fame` / `frontend` / `plugin` / `tools` / `data project`，没有 `engine`。按此定稿：引擎写 `hall of fame`（与样本 #2238 一致），前端写 `hall of fame`、`frontend`，配方管理器写 `hall of fame`、`tools`。注意正文里的反引号标签是**写法说明/搜索入口**，三个样本本体都没加 GitHub issue label（labels 数组为空），提交时不需要（也无法自行）添加标签。

以上三份正文已于 2026-11-18 原文提交为 #2241 / #2242 / #2243，三者的 labels 数组均为空（不带任何标签）。后续修改正文请直接编辑对应 issue，不必重开。
