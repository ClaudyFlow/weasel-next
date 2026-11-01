# librime 有什么可写的（librime-next 的功能清单）

> 2026-10-23。来源：这一轮实际踩到 / 实际读源码确认的问题。**每条都带证据**，不是愿望清单。
> 排序原则：**我们先受害过的排在前面**。

## 1. ascii 切换没有策略（社群公认的缺失）

**现状**：`ascii_mode` 只有开/关两种状态。`ascii_composer/switch_key` 里的
`inline_ascii` / `commit_code` / `commit_text` **只作用于列在那张 map 里的按键**；
`key_binder` 的 `toggle: ascii_mode` 只翻 option、带不了 style（`gear/key_binder.cc:88-117`）。

之所以"看起来像 inline_ascii"，是两条路径叠加的副作用：
- `engine.cc:130-138` `OnOptionUpdate` → `RefreshNonConfirmedComposition()` 按新模式重建；
- `ascii_composer.cc:128-139` 在 ascii 下对可打印 ASCII 直接 `ctx->PushInput(ch)`。

**可写**：把"切换时怎么处理未确认的编码"做成**一等配置**（`inline_ascii` / `commit_code` /
`commit_text`），对**所有**触发路径生效（switch_key、key_binder、托盘、`Ctrl+Space`、API）。

**两种落法**：
- **A（现在就能做，不用重编）**：我们在 Rust 的 `Engine` 层接管 —— 检测 `ascii_mode` 变化，
  按配置调 `commit_composition()` / `clear_composition()` / 不动。
  落点：`Engine/src/engine.rs:2647-2680`（那里已经在比较切换前后），配置进 `weasel.yaml`。
- **B（长远）**：`librime-next` 移植 `gear/ascii_composer` 时原生实现。

## 2. `__build_info` 用 mtime 判断"要不要重编"—— 本机时钟会回跳

**现状**：`build_info_plugin.cc:25-41` 把每个源文件的 `last_write_time` 记进 timestamps；
`deployment_tasks.cc:444-480` 拿它跟当前 mtime 比对。`RIME_NO_TIMESTAMP` 可关掉。

**为什么可写**：这台机器的时钟**回跳过**（本项目检查点第一条就是"时间戳不可靠，判断新旧用哈希"）。
mtime 判新旧在这个环境下会误判。

**可写**：用**内容哈希**（或哈希 + mtime 双条件）代替 mtime；结构上仍叫 `__build_info`，
但 `timestamps` 变成 `sources{digest, mtime}`。

## 3. `rime deploy` 是复合动作，没法只做一半

**现状**：一条 `--deploy` 同时干三件事：编译 config、编译词典（`.bin`）、写输出。
所以我们"用 Rust 替换 config 层"时不能把 `--deploy` 换掉 —— 换掉就没人编词典了（今天实测确认）。

**可写**：拆成独立任务（`config` / `dict` / `output`），可单独跑、可 dry-run、可列出"会改哪些文件"。

## 4. 资源搜索规则写死在 C++ 里

**现状**：`resource.cc` 的 `ResolvePath` 把"哪类资源进哪个子目录"写死。我们为此打了
三个补丁（`f32b367` / `a8ce2f4` / `40416ff`），还是靠"扁平回退"兜着。

**可写**：把搜索规则做成**数据**（一张表：后缀 → 子目录 → 回退顺序），而不是代码分支。

## 5. Lua 组件加载失败只 `LOG(ERROR)` 然后静默丢弃

**现状**：`plugins/lua/src/lua_gears.cc:93-108`，`*名字.函数` 会被当成
`require(整个点分名)`；失败只打一条 ERROR，组件就没了。方案照样能加载，行为悄悄变。

**这就是 2026-10-23「宇浩自分割没了」的根因** —— 插件文件没进安装包，`require` 失败，
`lua_processor@*yuhao.yuhao_auto_select_postponed` 被丢掉，而界面上没有任何提示。

**可写**：组件加载失败 → **显式错误**（至少让方案标记"降级运行"，并在候选窗/日志里可见）。
配合"数据清单校验"（见 8）。

## 6. `__include` 循环依赖只记 `LOG(INFO)`

**现状**：`config_compiler.cc:583-586`，`circular dependencies detected` 是 INFO 级，
返回 false 后上层往往继续用"过时数据"（`:424-430` 的 CAVEAT 注释自己承认）。

**可写**：升级为可诊断错误（带依赖链），并在链接阶段 fail-closed。

## 7. punctuator 有**三种**值形态并存

**现状**：`symbols.yaml` 里同一个键可以是
`"＃"`（纯字符串）/ `{commit: ￥}` / `[ ＃, ⌘ ]`（数组，多候选）。
`gear/punctuator.cc:180/211/347` 分别处理。今天对拍时正是这个差异让我们误以为"有转换代码"。

**可写**：编译期**规范化**成一种内部表示（数组），对外仍接受三种写法；
这样 `build/*.schema.yaml` 的形态稳定，比对/迁移都简单。

## 8. 没有"方案声明它需要什么"

**现状**：方案靠 `lua_processor@*yuhao.xxx` 这种名字隐式依赖插件；缺了不报错（见 5）。
对照 YUME 的 `data_manifest` 思路（见 `doc/yume-学习笔记.md`）：**数据集由清单定义**，
缺件要能说出来。

**可写**：`schema.yaml` 里可选的 `requires:` 段（组件/数据文件），加载时校验并报缺。

## 9. 用户词典格式锁死 LevelDB

**现状**：`userdb/*.userdb` 是 LevelDB，C++ 侧只读写不导出（`lever/deployment_tasks.cc` 里有
`userdb` 归置逻辑，但我们为它打了补丁 `weasel_userdb_path`）。

**可写**：提供一个可读写/可导出的格式（或至少导出工具），让用户词典能被备份、diff、迁移。

---

## 优先级建议

| 序 | 条目 | 理由 |
|---|---|---|
| 1 | **1（ascii 策略）** | 社群明确抱怨、我们前端一层就能做、当天可验 |
| 2 | **5（组件加载失败要可见）** | 今天真实事故的根因，静默失败最贵 |
| 3 | **2（哈希代替 mtime）** | 本机时钟不可靠，这个坑已经踩过 |
| 4 | **3（拆 deploy）** | 是"替换 config 层"能否上线的关键前提 |
| 5 | 4 / 6 / 7 / 8 / 9 | 结构性改进，等前三件落地后再排 |
