# 小狼毫Next —— 待办事项（todo.md）

更新：2026-10-16  ·  代码库：C:\code\Weasel-Next  ·  提交数 376

## 当前状态（可用）

- 候选窗**可用**：能显示、能打字、皮肤读官方 aqua（`theme.log`: 皮肤已加载 配色 aqua 底色 #eceeee）
- 布局已是**官方纵排**：每候选一行、序号列分开、选中只盖一行、无 Aero 玻璃图层、四周 border 留白
- 官方渲染器源码已入库：`theme\official\reference\`（24 文件 230 KB + README 对应表）
- 两份对照文档：`doc\official-layout-reference.md`、`doc\official-style-as-toml.md`
- librime 改动已归档：`librime\weasel-next.patch`（9 文件）
- `installation.yaml` → `installation.yaml`（installation_id 未变）
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
6. 程序文件夹根目录 7 个 yaml 迁进分类目录；`weasel.toml` / `default.yaml` 内容已被 `theme.toml` / `schema.toml` 接管，可退休
7. 引擎侧 `eassy.txt` 写出位置改到用户目录（现在每次启动报 `拒绝访问 (os error 5)`）
8. 引擎启动前设 `GLOG_minloglevel=1`，压掉 librime 200+ 条 INFO（WARNING/ERROR 保留）

## P2 —— 部署

9. 部署清单 `common\src\deploy_manifest.rs`：`SOURCE_EXTENSIONS` 加 `"toml"`；扫描范围加**程序文件夹**的七个分类（现只扫用户目录，改动不触发部署）
10. 增量做法接线：⚠️ **`common\src\deploy_watch.rs` 已不存在**（2026-10-17 核：它在提交 `3c2371a`「保存渲染器重写与配套工作(93 文件)」里被一并删掉，258 行）；
    下面那几个函数名字是它被删之前的样子 —— **要做得重新写**，别按"已就绪"去接线。
    （旧的 3 个测试也随之消失。）
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
---

## P2 详则 —— 部署记录 `user.toml`（按需求逐条）

### 需求原文（照记）

1. **两份** `user.toml`：**用户文件夹根目录**一份、**build 文件夹**一份
2. 内容是：**上次部署的时间** + **各个文件夹的部署时间**
3. **没有新变动就不改时间**
4. 排列顺序：**文件夹名 → 时间戳 → 哈希值**
5. 外面那份（用户文件夹根目录）要**检测变化**，一旦更改就**记录**：**更新时间 + 更新哈希**；
   写法是"**旧哈希 → 新时间 新哈希**"（保留上一次的哈希，便于回溯）
6. **有变化但没部署 → 记着**（攒 pending）；**有部署 → 把记录清掉，只留最新的**
7. 做成**增量编译的做法**：**不要**每十分钟定时校验；哈希比对发生在**触发部署**的那一刻
8. **部署直接部署，不再让部署校验**（去掉部署前那道"没变就跳过"的闸）
9. 无事可做时提示「**无需部署**」，**不要**显示"部署完成"

### 两份文件的格式（示例）

`<程序文件夹>\..` 与用户目录结构相同，此处以 `%APPDATA%\Rime-Next\` 为例。

**① `build\user.toml`** —— 部署时写入。一段一个文件夹，含 `time` 与 `hash`；
**哈希没变就不动 time**（沿用上次那份的时间）：

```toml
# user.toml —— 各文件夹上次部署的时间与哈希
last_deploy = "2026-10-16 14:30:00"

[schema]
time = "2026-10-16 14:30:00"     # 这次有变动 → 用新时间
hash = "3f9a1c…"

[theme]
time = "2026-10-15 20:08:00"     # 这次没变 → 保持旧时间（要求 ③）
hash = "b71e02…"
```

**② `%APPDATA%\Rime-Next\user.toml`**（用户文件夹**根目录**）—— 一行一个文件夹，
记「旧哈希 → 时间 新哈希」；部署成功后**清掉箭头、只留最新哈希**（要求 ⑥）：

```toml
# 部署前（攒着"还没部署的变化"）
schema = "1a2b3c… -> 2026-10-16 14:30:00 3f9a1c…"
theme  = "b71e02… -> 2026-10-16 14:30:00 c93d77…"

# 部署成功后（清账，只留已部署的最新哈希；注释与空行保留）
schema = "3f9a1c…"
theme  = "c93d77…"
```

### 已就绪的代码（`common\src\deploy_watch.rs`，3 个测试全过 ✓）

> ⚠️ 2026-10-17 核正：**这个文件已经不存在了**（提交 `3c2371a` 把它删了）。下表只是**当初的设计**，
> 拿来当规格书用可以，**不要**以为代码还在。落地时要重新写这个模块 + 测试。

| 函数 | 作用 | 对应需求 |
|---|---|---|
| `build_stamp_path(user_dir)` | `build\user.toml` 路径 | ① |
| `watch_stamp_path(user_dir)` | 用户根目录 `user.toml` 路径 | ① |
| `parse_stamps(text)` | 解析 `build\user.toml`（`[名字]` 段 + time/hash） | ④ |
| `write_stamps(path, current, previous, now)` | 写出；**哈希没变就不动 time** | ③ ④ |
| `detect_changes(previous, current, now)` | 挑出哈希变了的文件夹 | ⑤ |
| `write_watch(path, previous_text, changes)` | 把变化**追加**成 `旧哈希 -> 时间 新哈希`；同哈希不重复追加 | ⑤ ⑥ |
| `clear_after_deploy(path, previous_text, deployed)` | 部署后**清账**，只留最新哈希（注释/空行保留） | ⑥ |

### 还差的三处接线（在 `Server` / `weasel-next-server`）

1. **`deploy()` 触发时**：算七个分类文件夹的哈希 → `detect_changes` 与上次比对
   - 有变化 → `write_stamps`（更新时间 ✓）+ `write_watch`（往根目录那份追加 ✓）
   - 没变化 → 原样跳过（时间不动 ✓）
2. **部署成功后** → `clear_after_deploy`（清掉 pending，只留最新 ✓）
3. **去掉部署前的 `is_unchanged` 闸**（要求 ⑧）：改成"触发就部署"；
   判定"要不要部署"由上面那次比对承担 ✓
4. 跳过分支的文案保持「**无需部署**」（`Server\src\operations.rs:204` 已经是 ✓）

### 判定标准（做完怎么验）

- 改一个方案文件 → 触发部署 → `build\user.toml` 里**只有该文件夹**的 `time` 变了 ✓（其余保持）
- 同一次改动再触发部署 → **无需部署**提示 + `time` 不变 ✓
- 用户根目录 `user.toml`：改之前有变化时出现 `旧哈希 -> 时间 新哈希` ✓；部署成功后箭头消失 ✓
- 部署清单（`build\.deploy-manifest.sha256`）与 `user.toml` 不再是"变了就跳过部署"的依据 ✓