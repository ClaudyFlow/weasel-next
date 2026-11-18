# 顶功607（xmjd6 / 键道6·仰望星空）适配说明

2026-11-18 完成。源包：`C:/Users/CloudyFlow/Downloads/顶功607.zip`（134 文件，28 MB；上游
`https://github.com/hugh7007/xmjd6-rere`）。方案本体是标准 rime-lua 社区包，但有两处
与锈狼嚎引擎的约定不同，需要转换。

## 目录映射（源包 → 安装目录 data）

| 源包路径 | 安装后位置 | 说明 |
|---|---|---|
| `xmjd6.schema.yaml` 等 6 个方案 | `schema/<id按点分段>/<id>.schema.yaml` | 引擎 `paths.rs` 的恒等式规则；`xmjd6.cx` → `schema/xmjd6/cx/` |
| `*.dict.yaml`（25 个） | `dict/xmjd6/`（pinyin_simp / english / liangfen 各自目录） | 编译器支持逐级退前缀 + 兄弟目录宽容搜索 |
| `lua/xmjd6/*.lua` | `plugin/xmjd6/<名字>/xmjd6.<名字>.lua` | 引擎只认 `plugin/` 恒等式布局，`lua/` 不解析 |
| `opencc/*.json/txt/ocd2`、`opencc/Data/*.lua` | `opencc/` 与 `plugin/opencc/` 各一份 | `lazy_simplifier` 用相对脚本路径 `../../opencc/` |
| `xmjd6.ico` | `icon/xmjd6.ico` | 方案图标 |

安装脚本：`doc/install_xmjd6.py`（只读源、覆盖目标；运行前会备份被覆盖文件到
`C:/code/_repo_cleanup_20261118/xmjd6-install-backup-20261118/`）。

## Lua 引导注册

方案组件写成 `lua_processor@*xmjd6/key_counter` 这种 `*` + 斜杠形式；引擎的全局查找按
`.` / `*` 分段，不认斜杠。因此在 `plugin/rime.lua` 末尾追加 `xmjd6_reg(别名, 模块)`，
把 `*xmjd6/名字` 注册为全局键，模块走恒等式 `require("xmjd6.名字")`。
共 38 个组件（含 `lazy_simplifier@emoji_cn` 这类带 `@tag` 的变体）。
三份引导脚本同步：安装目录 data、用户目录、`Rime-Next/plugin/rime.lua`（仓库源）。

## 引擎侧兼容（librime-next `f68322b`）

社区脚本依赖引擎原本缺失的 Lua API，已在 `src/gear/lua.rs` 补齐：

- **debug 标准库**：`debug.getinfo(1).source` 定位脚本目录（`xmjd6_topup_processor` /
  `lazy_simplifier` 用它读相邻码表）。用 `Lua::unsafe_new_with(StdLib::ALL, ...)`。
- **UserDb**：进程内内存实现 `fetch/update/erase/loaded/query`（`query` 返回带 `iter()`
  的游标），供 `input_statistics` 存统计元数据；不落盘。
- **context 方法**：`get_selected_candidate()`（返回带 text/comment 的候选表或 nil）、
  `commit()`、`pop_input(n)` —— 顶功上屏与空码回退的核心。

## 部署与验证

停任务 → 杀进程 → `librime-next.exe deploy --shared "<安装目录 data>" --user "%APPDATA%/Rime-Next"`
（ok=true，xmjd6 主词库 895,344 条，产物 182 MB）→ 双名 DLL 铺装 → 重启服务。

C-API 探针（临时把 `schema/schema.yaml` 的 `active` 切到 `xmjd6`）：

| 输入 | 结果 |
|---|---|
| `ds` | 候选 `大; 妲; 第三步; 第三次; 第三代;`，空格提交「大」 |
| `nihao` | 顶功拆分：preedit 剩 `hao`，候选「还能」，提交「还能」 |
| `juzhong` | 顶功拆分：preedit 剩 `ng`，候选「宁」，提交「宁」 |

所有 Lua 组件 init 无报错。测试后 `active` 已恢复 `pinyin`。

## 已知限制

- `table_translator@xmjd6_user`（`dictionary: ""` 的纯用户词典翻译器）引擎创建失败，
  影响方案自带的动态自造词入口；主词库输入完全正常，未改引擎的「必须非空主词典」约束。
- `UserDb` 是内存实现，统计面板数据重启后不保留（不影响输入）。
- 方案清单 `<user>/schema/schema.yaml` 由用户勾选权威维护；已追加 `xmjd6`，`active` 仍 `pinyin`。
