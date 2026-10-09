# weasel-settings-tauri

小狼毫 RS 的 Tauri 2 + Vue 3 设置界面。它与 Slint 版（`../settings/`）**并存**，两套界面
共用同一份框架无关核心，因此对配置的行为完全一致。

## 现在的功能

界面不再是骨架，已经能真正读写配置：

* **通用表单**：按 `weasel.richschema.json` 生成字段，支持布尔/枚举选择、数字（含"自定义"）、
  字符串、颜色（含"跟随系统"）、JSON 对象（多行）；每项都有"覆盖/继承"开关。
* **分区**：常规、各内置主题（abc / eleven）、WASM 模块选择与路径。
* **按应用覆盖**：列出 `app_options`，逐项设置 `ascii_mode` / `inline_preedit`（继承/开/关），
  可新增（`notepad.exe` 这类文件名）与删除。
* **保存语义**：改动先进内存补丁，点"保存"才写 `<用户数据>/weasel.custom.json`；
  写盘前用 rich schema 校验每个分区，保存采用临时文件 + 原子替换，并在替换前检测
  "文件已被其他程序修改"。另有"丢弃改动"（重新读盘）与"恢复默认"（清空补丁）。
* 界面显示配置文件路径、是否已存在、运行时目录、注意事项与未保存标记。

## 架构

```
vue/src/App.vue            渲染 + 交互（不含配置逻辑）
src-tauri/src/lib.rs       Tauri commands：加锁取出会话 → 调用 → 返回 View
                           （全部实现在 weasel-settings 库里）
../settings/src/lib.rs     框架无关核心：config / catalog / metadata / form / app_options / session
```

`weasel-settings` 以 `default-features = false` 依赖，**不会**把 Slint 与 GPU 后端拖进 Tauri 程序。

命令一览（`invoke` 名）：

| 命令 | 作用 |
|---|---|
| `settings_load` | 读取磁盘配置，返回完整视图 |
| `settings_view` | 只重建视图（不读盘） |
| `settings_apply(section, field, selected, text)` | 应用字段改动（`selected = 0` 恢复继承） |
| `settings_save` | 校验 + 原子保存 |
| `settings_discard` | 丢弃内存改动，重新读盘 |
| `settings_reset` | 清空用户补丁（恢复默认，仍需保存） |
| `settings_app_add` / `settings_app_remove` / `settings_app_set` | 按应用覆盖项增删改 |
| `weasel_ping` | 保留的握手命令 |

## 开发与构建

```bash
cd settings-tauri
pnpm install
pnpm build          # vue-tsc 类型检查 + vite 构建到 dist/
cd ..
cargo build -p weasel-settings-tauri --release
```

注意：Tauri 在**编译期**把 `frontendDist`（`../dist`）嵌进可执行文件，所以改完前端必须
先 `pnpm build`，再重新 `cargo build`，否则界面还是旧的。

开发模式：`pnpm tauri:dev`（vite dev server 在 127.0.0.1:1420）。

## 测试

会话逻辑的单元测试在库里（`../settings/src/session.rs`），因为它们不依赖 Tauri：

```bash
cargo test -p weasel-settings --lib --no-default-features
```

本 crate 自身不写单元测试：它链接 Tauri/tao/WebView2，在缺少应用清单的测试宿主里会在
加载期失败（`STATUS_ENTRYPOINT_NOT_FOUND`），这是宿主环境问题而非逻辑问题。
