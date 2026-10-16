# 小狼毫Next —— 待办事项（todo.md）

更新：2026-10-16  ·  代码库：C:\code\Weasel-Next  ·  提交数 376

## 当前状态（可用）

- 候选窗**可用**：能显示、能打字、皮肤读官方 aqua（`theme.log`: 皮肤已加载 配色 aqua 底色 #eceeee）
- 布局已是**官方纵排**：每候选一行、序号列分开、选中只盖一行、无 Aero 玻璃图层、四周 border 留白
- 官方渲染器源码已入库：`theme\official\reference\`（24 文件 230 KB + README 对应表）
- 两份对照文档：`doc\official-layout-reference.md`、`doc\official-style-as-toml.md`
- librime 改动已归档：`librime\weasel-next.patch`（9 文件）
- `installation.yaml` → `installation.toml`（installation_id 未变）
- 分层约定：出厂只读在**程序文件夹**、用户设置在**用户文件夹**（`theme.toml` / `schema.toml`）

## P0 —— 渲染（照官方 `WeaselPanel.cpp`）

1. **字体接线未完成（当前卡点）** ✗
   - 已写 `logic::font_style_from_official()`（读 `style/font_face`、`font_point`、`label_font_point`、`comment_font_point`）
   - `Graphics::new()` 已改成用它建 number/text/comment/mode 四个格式
   - **编译报错**：`PCWSTR` 未导入 → 改成 `windows_core::PCWSTR(face.as_ptr())` 即可
   - 症状：注音方案下候选画成 `Y / ȶ / ɿ`（`ㄚ` 缺字形被回退）
2. **预编辑行未画** ✗ —— `Layout::new_with_preedit(widths, preedit_w)` 已就绪；调用处仍传 0，`paint` 也没画。照官方 `_DrawPreedit`（`WeaselPanel.cpp:511`，尺寸 `L539-541`）：白底块 + 文字 + 右端小三角
3. **布局值仍写死** ✗ —— `logic.rs` 里是 `MARGIN 12 / ROW_GAP 5 / LABEL_GAP 4 / MIN_WIDTH 160`（= 官方 aqua 值 ✓）；应接 `theme.toml` 的 `Metrics`（骨架见参照文档第六节）
4. 未做：`align_type` 垂直居中（center/bottom）· `comment` 列 · `mark` 与页码箭头（aqua 全透明，官方也不画）

## P1 —— 数据/引擎

5. `user.yaml` → `user.toml`：配置层加 `.toml` 分支（`config_component.cc:166` 后缀 + `config_data.cc` 解析分支，需重建 rime.dll）
6. 程序文件夹根目录 7 个 yaml 迁进分类目录；`weasel.yaml` / `default.yaml` 内容已被 `theme.toml` / `schema.toml` 接管，可退休
7. 引擎侧 `eassy.txt` 写出位置改到用户目录（现在每次启动报 `拒绝访问 (os error 5)`）
8. 引擎启动前设 `GLOG_minloglevel=1`，压掉 librime 200+ 条 INFO（WARNING/ERROR 保留）

## P2 —— 部署

9. 部署清单 `common\src\deploy_manifest.rs`：`SOURCE_EXTENSIONS` 加 `"toml"`；扫描范围加**程序文件夹**的七个分类（现只扫用户目录，改动不触发部署）
10. 增量做法接线：`common\src\deploy_watch.rs` 已就绪（`write_stamps` / `detect_changes` / `write_watch` / `clear_after_deploy`，3 个测试过）
    - 在 `Server`（`weasel-next-server`）的 `deploy()` 里：算各文件夹哈希 → 变了才更新时间并写根目录 `user.toml`；部署成功后 `clear_after_deploy`
    - 去掉部署前 `is_unchanged` 那道闸（改成"触发就部署"）
11. 无事可做时报「**无需部署**」（现在已经是 ✓，接线时不要退化成"部署完成"）

## P3 —— 其它

12. `/dict` 四个功能 · 语言栏右键菜单（需要 `ITfLangBarItemButton`，重启才能测）
13. 稳定后删掉临时诊断（`mod.rs` 的调色板/高亮日志、`rpc.rs`/`ui_runtime.rs` 的退出诊断、`theme.log` 通道可保留）
14. 安装包：确认 `script\build-installer.ps1` 递归打包 `Rime-Next\`（已确认 ✓ 仓库有啥装啥）

## 每次改动的固定动作（今天验证有效）

1. 改**一处** → `cargo build --release -p weasel-theme-official`
2. 提权复制到 `C:\Program Files\Weasel-Next\weasel_theme_official.dll`
3. **比对 SHA256**（源与目标一致才算装上 —— 只看字节数会被骗，今天上过一次当）
4. 重启 Server → 读 `%LOCALAPPDATA%\Rime-Next\Logs\theme.log` 与 `renderer.*.log`
5. 打字验收 → `git commit`

## 常见坑（今天踩过的）

- PowerShell：`.Replace()` 后不能直接接 `.PadRight()`，要加括号
- PowerShell：**变量名大小写不敏感**（`$w` 与 `$W` 是同一个变量）
- PowerShell：解析失败 = **整段没执行**（不会有半途状态）
- Rust：局部变量名不要取 `rect`（会遮蔽同名函数）
- Rust：改结构体时注意它**上一行**的 `#[derive(...)]` 是否还在
- 日志：跨模块日志要确认真的写出来了（主题曾用 `eprintln`，而渲染器是 `windows_subsystem="windows"`，**没有 stderr** → 原因全被丢掉）
- 官方参照：`C:\Code\weasel`（`WeaselUI\*.cpp`）—— 注意别把 `C:\Code\Weasel-Next` 当成官方（今天错过一次）