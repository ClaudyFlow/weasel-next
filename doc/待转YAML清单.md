# TOML 退出清单（2026-10-27 完成）

> 上一版叫「还没转 YAML 的 TOML（清单，只列不改）」，那一轮把**宿主配置**（`weasel.*`）
> 转完了。本轮把**剩下的数据文件全部转成 YAML**，代码里 `toml::` 调用归零。

## 结论

- 代码里 `toml::` 调用：**0 处**（`common` / `style` / `Engine` / `Renderer` / `Server` / `Setting`）
- 5 个 crate 的 `toml = "1"` 依赖：已删除
- 仓库内非 `Cargo.toml` / `rustfmt.toml` 的 `.toml` 文件：**0 个**
- 装机目录（`%APPDATA%\Rime-Next`）生效中的 `.toml`：**0 个**（`backup/` 里的历史快照也已清）

## 本轮转完的

| 原文件 | 新文件 | 代码改动 |
|---|---|---|
| `sound\sound.toml` | `sound\sound.yaml` | `Engine/src/engine.rs` 改走 `weasel_common::yaml` |
| `eassy\default\default.eassy.toml`（442,723 条 / 8.1 MB） | `default.eassy.yaml` | `common/src/eassy.rs`；`rime_layout.rs`、`Engine/src/librime/mod.rs` 同步 |
| `theme\theme.toml` + `theme\default\default.theme.toml` | `.theme.yaml` | `style/src/theme.rs`（`parse_toml`→`parse_yaml`）、`style/src/settings.rs`（行编辑改 YAML 语法）、`Setting/src/skins.rs` / `dialogs.rs` / `main.rs`、`theme/src/config.rs` |
| `stats\YYYY-MM-DD.toml` | `.yaml` | `Setting/src/stats.rs` |
| `schema\shuangpin\xiaohe.toml` / `ziranma.toml` | `.yaml` | 另一进程并行完成 |
| 生成器 bin `theme-toml` | `theme-yaml` | `style/src/bin/theme_yaml.rs`（重写成 YAML 输出） |
| 部署清单 `SOURCE_EXTENSIONS` | 去掉 `toml` | `common/src/deploy_manifest.rs` |
| `weasel.toml`（仓库根，已废弃） | — | 直接删除 |

## 引擎侧（librime-next / C++ 补丁）

`.theme.*` 的后缀是 **librime 的路径摆放规则**，不只是数据格式：

| 位置 | 改动 |
|---|---|
| `librime-next/src/lever/paths.rs` 规则 ⑥ | 认 `.theme.toml` **和** `.theme.yaml`（否则会被当未归类资源摆到根目录） |
| `librime-next/src/lever/scan.rs` `classify` | 同上；旧的 `.toml` 仍认，存量文件还能扫到 |
| `librime/weasel-next.patch`（C++ `resource.cc` 规则 ⑥） | 同上；下次重编 `rime.dll` 才生效 |

`schema\schema.toml` 那条链：补丁版 C++ 直读（+259 行 `toml_lite`），LRN 已改读 `schema\schema.yaml`，
装机目录里那份已删（LRN 有测试 `toml_is_not_recognized_anymore` 保证不回退）。

## 语义变更（要记住）

1. **重复键**：TOML 里重复键是语法错误 → 落宽容解析 → 同词**求和**；
   YAML 里重复键合法 → **后者覆盖**。同一份词库只有 5 处重复词，无实际影响。
   跨文件的同词仍然求和。
2. `set_theme_key` 的插入位置：顶层键要写在**嵌套块的起始键之前**，
   只"插到第一个缩进行之前"会把那个块拦腰截断。

## 数据等价性（2026-10-27 逐条核对，与 git HEAD 里的旧 TOML 比）

| 文件 | 旧 TOML | 新 YAML | 结论 |
|---|---|---|---|
| `default.eassy.*` | 442,723 词条（376,198 条频次 > 0） | 442,723 词条（376,198 条频次 > 0） | 键集合一致、**值 0 处不同** |
| `default.theme.*` | 37 套配色 | 37 套配色 | 一致 |
| `shuangpin/xiaohe.*` | schema 7 / initials 3 / finals 33 | 同 | 三段全等 |
| `shuangpin/ziranma.*` | schema 8 / initials 3 / finals 33 | 同 | 三段全等 |

（细胞词库里约 6.6 万条频次为 0 的条目按设计不计入 —— 与 TOML 时代口径相同，不是丢数据。）

## 验证

- `cargo test -p weasel-common -p weasel-style`：**114 passed / 0 failed**
- `common/tests/data_yaml_files.rs`（新增，出厂数据文件冒烟）：**4 passed**，
  用自己的解析器实读 `default.eassy.yaml` / `default.theme.yaml` / 双拼表 / `weasel.yaml`
- `cargo test --workspace`（含 `LIBCLANG_PATH`）：**全绿**
- `librime-next` `cargo test`：**584 passed / 0 failed**
- `librime-next` `cargo test --lib lever`：**36 passed**（含新增 `.theme.yaml` 分类/摆放测试）

## 附：两份 `weasel.yaml` 收进 `Rime-Next\`（2026-10-27 晚）

仓库根原有一份宿主出厂默认 `weasel.yaml`，`Rime-Next\` 另有一份**官方原版皮肤**
`weasel.yaml`（fxliang/weasel 的数据文件）—— 同名不同物，极易混淆。现统一放进 `Rime-Next\`：

| 文件 | 是什么 | 谁读 |
|---|---|---|
| `Rime-Next\weasel.yaml`（2.8 KB） | **宿主出厂默认**（theme / ascii_mode / sound / app_options…） | 7 处 `include_str!` 内嵌；安装脚本复制成 `<程序文件夹>\weasel.yaml` |
| `Rime-Next\weasel.official.yaml`（18 KB） | **官方原版皮肤**（36 套配色 + style） | 仅 `style` 的测试常量与 `theme-yaml` 生成器 |

**为什么不合二为一**：`weasel.richschema.json` 根节点是 `additionalProperties: false`，
只认 11 个键；官方皮肤里的 `config_version` / `show_notifications` / `style` /
`preset_color_schemes` 一旦进到内嵌默认值，`Metadata::from_parts` 校验就会失败，
设置界面直接加载不出来。加上那份皮肤有"与上游逐字节同一份"的约定，所以保持两个文件、
用文件名把职责分开。
