# weasel-settings（Slint 界面）

小狼毫 RS 的两套设置界面之一，**功能完整**的那一套（另一套见 `../settings-tauri/`）。
界面用 Slint 描述（`ui/*.slint`），配置读写、schema 校验、表单模型在 **`weasel_settings` 库**里，
与 Tauri 版共用。

## 构建

```bash
# 默认：Slint 界面 + femtovg 渲染后端（纯 Rust + 系统 OpenGL，构建期不需要联网）
cargo build -p weasel-settings --release

# 只构建框架无关的库（不牵入 Slint，Tauri 版就是这样依赖它的）
cargo build -p weasel-settings --lib --no-default-features

# 库测试（配置读写、schema 表单、会话逻辑）
cargo test -p weasel-settings --lib --no-default-features
```

产物：`target/release/weasel-settings.exe`（窗口标题"小狼毫Next：设置"）。

## 渲染后端

| 特性 | 说明 |
|---|---|
| `renderer-femtovg`（默认） | 纯 Rust + 系统 OpenGL。**离线可构建**，本仓库当前默认值。 |
| `renderer-skia` | 原先后端。`skia-bindings` 会在构建期访问 `chromium.googlesource.com` / `skia.googlesource.com` 拉源码或预编译包；网络受限或离线环境会直接构建失败（本机即如此）。需要它时：`cargo build -p weasel-settings --release --no-default-features --features gui,renderer-skia` |
| `renderer-software` | 纯 CPU 渲染，兼容性最好、性能最低。 |

## 模块划分

* `src/main.rs`：Slint 窗口、事件绑定、模型同步（需要 `gui` 特性）。
* `../src/lib.rs`（`weasel_settings` 库，框架无关）：`config`（配置文档与原子保存）、
  `catalog`（主题与 WASM 描述发现）、`metadata`（rich schema 校验）、`form`（表单字段）、
  `app_options`（按应用覆盖）、`session`（加载→改→校验→保存的会话，供两个前端复用）。
* 只在 bin 里的模块：`backdrop`（Mica/无边框）、`bindings`、`broker`、`colors`、`frame`、`updater`、`wasm_modules`。

`CONFIGURATION.md` 说明配置描述文件（`*.json` / `*.richschema.json` / `*.schema.json`）的来源与生成流程。
