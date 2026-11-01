# 小狼毫Next —— 工作进度（PROGRESS.md）

更新：2026-10-28 19:0x · 会话区间：2026-10-27 深夜 ~ 10-28
代码库：`C:\Code\Weasel-Next` · 相关运行时：`C:\Program Files\Weasel-Next` · 用户数据：`%APPDATA%\Rime-Next`

> 本文件记录**本轮会话**的进度、已定标准、实机诊断与卡点；项目长期待办见 `todo.md`。

---

## 一、本轮目标

把 `pinyin` 方案做成**标准样板**：识别合规 → 部署生成合规 → 打字可用 → 实机（记事本）验证。

---

## 二、已完成（代码 / 测试）

1. **四步识别流程**（`librime-next/src/lever/admit.rs`）
   - 顺序：识别 `.schema.yaml` → 检查路径（文件名主干反推路径，与实际比对）→ 检查头/格式 → 检查 `schema.id`
   - 单测 **14/14 通过**
2. **部署产物按标准生成**（`deployment_tasks.rs::write_compiled_schema`）
   - 对 `.schema.yaml` 强制写标准三行头，并把编译期生成的 `schema_id` 反写回单数 `id`
   - 已验证 `build/pinyin/pinyin.schema.yaml` 合规（头 + `id: pinyin`）
3. **`deploy` 命令**（`main.rs`）：`librime-next deploy [--shared <dir>] [--user <dir>]`
4. **pinyin 打字测试**（`test/pinyin_typing.rs`）：`nihao` → 首候选「你好」，空格上屏成功 ✅
5. **修引擎 bug**：`confirm` 动作 `confirm_current_selection() || commit()` 的 `||` 短路导致空格只确认不上屏 → 改为先标记选中再显式 `commit()`（`gear/editor.rs`）
6. **重新部署** `ok=true`，产物合规，打字测试复跑通过

---

## 三、已定标准（钉死，勿回退）

- **标准文件头三行**（所有方案源文件 + 生成产物统一）：
  ```
  # Rime Schema
  # encode: utf-8
  # newline: lf
  ```
  （`Rime`、`Schema` 首字母都大写；后两行是 admit 实际校验的标记）
- **`schema.id`**：强制单数 `id:`，拒 `schema_id:` 与混写；内容合法（字母数字 + `._-`，点表层级）；与**文件名 / 文件夹名 / 路径**三处对齐
- **四步识别**：识别 → 路径 → 头/格式 → `schema/id`；不合适即**抛弃**（不进清单、不删文件）
- **识别范围**：只认两个根下 `schema/` 的**直接子文件夹**；Rime-Next 根其他目录、`schema/` 根文件（含 `schema.yaml` 清单本身）不识别；资源池（shuangpin / yuhao）排除
- **`schema.yaml` 唯一权威**：用户文件夹 `schema/schema.yaml`（active + schemas）
- **不过 default**：`default.yaml` 不读当预设、不读当兜底；方案缺失 → 重新生成；default 只留全局配置，删掉 `schema_list`
- **首装流程**：识别程序文件夹全量池 → 向导展示 → **强制选 ≥1** → 生成用户 `schema.yaml` → 部署编译
- **出厂唯一必须**：`luna_pinyin_simp`（明月简体）；其余全可选，源文件留在 `schema/<名>/` 但不进出厂清单

---

## 四、实机状态（诊断结果）

**已就绪 ✅**
- Weasel-Next 后端运行：`Weasel-Next-Server` / `Engine` / `Renderer` 三进程
- IPC 管道已建并监听：`weasel-next-server-…` / `renderer-…` / `broker-…`
- TIP 已注册：`{16A7AEA9-9EE6-4540-8020-E384C0489BB1}` → `C:\Program Files\Weasel-Next\Weasel-Next.dll`；`LanguageProfile` 含 `0x0804`（简中）等
- 用户输入法列表含 Weasel-Next：`0804:{16A7AEA9-…}{3D02CAB6-…}`
- `active` 已改回 `pinyin`，重新部署 `ok=true`

**问题 / 未决 ⚠️**
1. **旧版小狼毫 0.17.4 并存冲突**：`C:\Program Files\Rime\weasel-0.17.4\WeaselServer.exe` 开机自启；其 TIP `{A3F4CDED-…}` → `C:\WINDOWS\system32\weasel.dll`。两个 Weasel 系同时在列表 → 抢后端、混淆
2. **语言栏看不到 Weasel-Next**：列表里有但 UI 未刷新/被旧版挤占 → 待用户**注销重登**
3. **打字上屏字母而非中文**：疑似未激活到 Weasel-Next 中文态，或误触 `Shift`（`Shift_L: inline_ascii`）进临时英文
4. **候选窗残留白块**：深色记事本上出现白底黑边空框（Renderer 未隐藏 / 空候选仍渲染）—— 待复测定位
5. **运行时 `rime.dll` 仍是旧版**：新 cdylib 已编出（`target/release/librime_next.dll`，已暂存到 `%TEMP%\rime_stage\rime.dll`），换入 Program Files 需 UAC

---

## 五、卡点（需用户操作 / 提权，我做不了）

1. **换 `rime.dll` 入 Program Files**：管理员运行 `%TEMP%\rime_stage\do-copy.ps1`（自动备份旧 dll 后换入）
2. **注销再登录**（刷新输入法列表 / TIP 重载）
3. **卸载旧版小狼毫 0.17.4**（消除并存冲突）
4. 程序目录源文件 `schema/pinyin/pinyin.schema.yaml` 首行仍是小写 `# Rime schema`（Program Files 无权限改）；**不影响产物**（产物已硬编码标准大写头）
5. **程序目录 40 个方案未标准化**：管理员 PowerShell 运行
   `& 'C:\Users\CloudyFlow\AppData\Local\Temp\rime_stage\do-normalize.ps1'`
   （自动备份 + 改头 + `schema_id:`→`id:` + LF；跑完告知我，我复扫确认 40/40）

---

## 六、待办（代码侧）

- **部署时生成 `schema.yaml`**：首装/缺失时从全量池生成（目前 `schema.yaml` 还是 13 项测试残留，非出厂该有）
- **其余 39 个装机方案改造为标准写法**（`schema_id:` → `id:` + 补标准头）
  - ✅ 已完成 **62 个**：代码仓 44/44、用户目录 18/18（2026-10-28 01:11）
  - ⏳ 程序目录 40 个待管理员执行（见"五、卡点"第 5 条）
- **`default.yaml` 收口**：删 `schema_list`；两份不一致（用户数据那份多 `pinyin`）
- **Renderer 候选窗残留白块**修复
- 非阻塞告警：`custom_phrase` translator、`simplifier` filter 创建失败（自定义短语 / 繁→简）

---

## 六·补、方案标准化改造记录（2026-10-28 01:11）

**动作**：三处 `schema/` 下所有 `.schema.yaml`（排除清单 `schema.yaml` 自身）批量改造为
① 标准头三行（首行 `# Rime Schema`、`encoding:`→`encode:`、补 `# newline: lf`）
② `schema:` 块内复数 `schema_id:` → 单数 `id:`
③ 去 BOM、CRLF→LF、末尾补换行。幂等，改前自动备份。

**脚本**
- `%TEMP%\rime_stage\normalize_schema.py`（Python，CODE / PROGRAM / USER 三根；`--dry-run` 只报告）
- `%TEMP%\rime_stage\do-normalize.ps1`（PowerShell 纯实现，**管理员运行**，专治 Program Files ACL）

**结果**

| 位置 | 方案数 | 改造成功 | 符合四步标准 |
|---|---|---|---|
| CODE `C:\Code\Weasel-Next\Rime-Next\schema` | 44 | 44 | **44 / 44** |
| USER `%APPDATA%\Rime-Next\schema` | 18 | 18 | **18 / 18** |
| PROGRAM `Program Files\Weasel-Next\Rime-Next\schema` | 40 | 0（ACL 拒） | 1 / 40（仅 pinyin） |

- 原始备份：`%TEMP%\schema_backup_20261028_011107`
- 残留 `schema_id` 在 CODE / USER 已清零（grep 无命中）

**改造后验证**
- `admit --dry-run` 抽查 `luna_pinyin_simp` / `wubi86` / `double_pinyin_flypy` / `yuling` → 全部 **结论：通过**（头标记、实际 UTF-8 无 BOM、实际 LF、`schema.id` 单数合规、与文件名/系列摆放一致）
- 重新部署 `ok=true`（double_pinyin_flypy、luna_pinyin_simp、pinyin 均重编译）
- 测试：`pinyin_typing` 1 passed、`lever::admit` 14 passed、`lever_deploy` 1 passed

---

## 六·二、程序文件夹布局大改（2026-10-28 01:40，02:30 修订）

**新布局（钉死）**

```text
PROGRAM_DIR/Rime-Next/          ← 程序文件夹（安装目录，exe 在这层）
    Weasel-Next-<*>.exe …
    data/                       ← 内置数据目录 = librime 的 shared_data_dir
        default.yaml  schema/  opencc/  parser/  eassy/  theme/ ...
```

- `PROGRAM_DIR` = 安装根（`Program Files`）
- 产品目录 `Rime-Next` —— **程序文件夹到这一层为止**（安装路径 + 产品名即可）
- 数据目录 **`data`**
- **用户文件夹一律不动**：`%APPDATA%\Rime-Next`

**★ 版本不进目录层级（02:30 修订）**：`Weasel-Next-<版本>` 是**安装包名 / 可执行文件名**
（`Weasel-Next-0.18.4-Setup.exe`、`Weasel-Next-Engine.exe`），不是目录名；
多版本并存的排布属于**外壳（Weasel）安装器**的决定，与 Rime 无关。
（01:40 版曾把版本当目录，已撤销；`DIST_PREFIX` / `DIST_FOLDER` 一并删除。）

**★ 分层职责（02:30 明确）**

| 层 | 负责什么 |
|---|---|
| Weasel 外壳（`common::rime_layout`，Windows） | 拼 `PROGRAM_DIR/Rime-Next` → 取 `data` → 得到绝对路径 |
| Rime（`librime-next`） | 只认**四个被注入**的目录：`shared_data_dir` / `user_data_dir` / `prebuilt_data_dir` / `staging_dir`，不知道 `Program Files`、`Rime-Next`、`data` 这些名字 |

`RimeTraits`（`c_api.rs`）里就是这四个目录字段 + `distribution_name` / `distribution_code_name` /
`distribution_version` 三个发行信息字段；`program_dir` 是**我们自己的扩展**（非 librime 标准字段），
仅当宿主不注入时由 `shared_data_dir.parent()` 兜底推出 —— 新布局下推出值正好是 `…/Rime-Next`，正确。

**常量收口**（`common/src/rime_layout.rs`）

| 常量 / 函数 | 值 / 作用 |
|---|---|
| `PROGRAM_DIR_ENV` | `"ProgramFiles"` |
| `PRODUCT_FOLDER` | `"Rime-Next"` |
| `VERSION` | crate 版本，**只用于安装包/exe 命名**，不参与目录拼装 |
| `DATA_FOLDER` | `"data"`（由 `shared` 改名而来） |
| `LEGACY_DATA_FOLDER` | `"Rime-Next"`（仅旧装机回退） |
| `USER_DATA_FOLDER` | `"Rime-Next"`（用户侧，不动） |
| `program_dir()` | `PROGRAM_DIR/Rime-Next` |
| `data_dir(install_dir)` | `<程序文件夹>/data`，旧装机回退 `<程序文件夹>/Rime-Next` |

**改动落点（20 处调用）**：Engine（`librime/mod.rs` 9、`main.rs` 2、`engine.rs` 1）、Server/operations.rs 2、Setting/skins.rs 2、style/shuangpin_schema.rs 1、theme（config.rs 2、paint.rs 1）。

**有意保留 `shared_data_dir` 名字的地方**：`Engine/src/librime/mod.rs` 的 `RimeTraits` 字段与 `librime-next/src/c_api.rs` —— 这是 librime **对外 C ABI** 的名字，改了会破坏兼容性，所以只在目录常量/helper 层改名。

**验证**
- `cargo check --workspace` 全通过（Engine 需 `LIBCLANG_PATH="C:\Program Files\llvm-mingw\bin"`，顺带解决了既有 bindgen 环境问题）
- 测试：`weasel-common` 74 项、`pinyin_typing` 1、`lever::admit` 14、`weasel-style` 45 全绿

**待办**
- 安装器要按新布局铺：`InstallDir` 由 `$PROGRAMFILES64\Weasel-Next` 改成 `$PROGRAMFILES64\Rime-Next`，
  内置数据落到 `data`（现在实际装机仍是 `Program Files\Weasel-Next\Rime-Next`，靠 `data_dir` 回退兼容）
- 代码仓源目录 `C:\Code\Weasel-Next\Rime-Next` 是否物理改名 `data`（暂未动）

---

## 六·三、部署时生成方案清单（2026-10-28 03:00）

**要解决的问题**：清单 `<user>/schema/schema.yaml` 是部署唯一权威，但首装时它不存在，
部署又必须知道编译哪些方案。以前靠 `default.yaml` 的 `schema_list` 兜底 —— 用户拍板
**不过 default**，直接在部署时识别程序文件夹生成。

**落点**：新模块 `librime-next/src/lever/schema_list.rs`

| API | 作用 |
|---|---|
| `discover(data_root)` | 扫 `<数据根>/schema/**/*.schema.yaml`，逐个过 admit 四关，返回 `accepted` + `rejected`（带原因） |
| `discover_roots(&[shared, user])` | 多个根合并、id 去重（shared 优先） |
| `ensure_manifest(user, shared)` | **清单非空 → 原样不动**；缺失/为空 → 识别生成并落盘 |

**规则**
- 只认 `schema/` 下的方案：根目录散文件、根目录文件夹、其它分类目录一概不扫
- 四关全过才收（头标记 / 实际格式 / `schema.id` 单数 / 文件名·摆放一致），不合格**抛弃**不进清单
- `schemas` 按 id 字典序；`active` 优先 `pinyin`，没有取第一个
- 一个合规的都没有 → **不写空清单**（写了等于清空用户选择）
- `run_workspace_update` 第 3 步调用；`default.yaml` 回退代码已移除（`schema_list_from_json` 只留给第 5 步写回 staging）

**CLI**：`librime-next manifest [--shared <dir>] [--user <dir>] [--rebuild]` —— 打印收下/抛弃 + 清单现状

**验证**
- 单测 `lever::schema_list` 7 项、集成测试 `test/schema_manifest_generate.rs` 3 项全绿
- `librime-next --lib` 517 项全过；`lever_deploy` 集成测试仍过（249s）
- 真实数据：`--shared C:\Code\Weasel-Next\Rime-Next` → **收下 44 / 抛弃 0**
- 旧装机 `--shared "C:\Program Files\Weasel-Next\Rime-Next"` → 收下 37 / 抛弃 6
  （全是 `schema/yuhao/` 下的嵌套副本：`yuming*` ×5 + `yuhao_pinyin`，摆放关
  `expected yuming but found yuhao/yuming`；根下已有正牌副本，故**无需白名单也无需删除**，
  自动进不了清单 —— 之前挂着的那个待办就此关闭）

**待办**
- 首装向导"强制至少选一个"属于 UI（Setting 方案选单），清单侧已保证 active 非空
- 用户文件夹现有 13 项清单是历史选择，未被覆盖（符合"用户选择是权威"）

---

## 六·四、`dict/` 与 `schema/` 拆家：词库只认 `dict/`、身份用 `id`（2026-10-28 18:5x）

**用户 10-28 18:4x 定的数据布局**：`dict/` 只放词库（`*.dict.yaml`）、`schema/` 只放方案
（`*.schema.yaml`）；词库身份是**头里的 `id:`**，不是 `name:`（`name` 是显示名，允许中文）。

**代码落点**

| 文件 | 改动 |
|---|---|
| `lever/paths.rs` | 新增 `pub const DICT_DIR: &str = "dict";` |
| `lever/scan.rs` | `ResourceKind::Dict => Some(DICT_DIR)`（原归 `SCHEMA_DIR`） |
| `dict/dict_settings.rs` | 新增 `dict_id()` / `identity()`（有 `id` 用 `id`，老文件回退 `name`）；`get_tables()` 改用 `identity()`；**新增词典头方言归一化 `normalize_dict_dialect()`：`column` → `columns`（只补缺，复数优先）** |
| `lever/admit.rs` | `id_from_dict_file_name()` / `find_dict_data_root()` / `check_dict_identity()`（第三关）/ `check_dict_file()` / `first_failure()` |
| `lever/schema_list.rs` | `collect_dict_files()` + `discover_dicts(root)` + `discover_roots_dicts(roots)`（与 `discover` / `discover_roots` 完全同构） |
| `dict/dict_compiler.rs` | `classified_dict_source()` 两处 `"schema"` → `DICT_DIR`（`:456` 系列路径、`:471` 宽容搜索） |
| `main.rs` | 新增 `dicts [--shared <目录>] [--user <目录>]`：列词库的收下/抛弃名单 |

**验证（本棒）**

- `cargo check -p librime-next` 通过；`cargo check --workspace` 通过（0 error）
- `cargo test -p librime-next --lib` → **525 passed / 0 failed**（含改到 `dict/` 的 5 项 dict_compiler 单测 + `lever::admit` 14 项）
- `dicts --shared C:\Code\Weasel-Next\Rime-Next` → **收下 3 / 抛弃 5**
  （收：`pinyin.luna`、`pinyin.terra`、`stroke`；弃：cangjie 5 份缺 `id:`，`cangjie.5.stem` 的 `id: cangjie5.stem` 与文件名不符，`wubi.86` 缺 `id:`）
- 临时用户目录真机 deploy（`--shared Rime-Next --user C:\Code\_tmp_dict_deploy2`）：
  **6 方案过准入、4 个词库编译成功**（pinyin.luna / pinyin.terra / stroke / zhuyin）
  → 词库确实从 `dict/` 按 id 取到了（日志：`collecting entries from …\dict\pinyin\luna\pinyin.luna.dict.yaml`）
  → `ok=false`：`cangjie.5` 报 `source file 'cangjie5' does not exist`、`wubi.86` 报 `'wubi86'`（**数据侧引用还是旧 id**）

**数据侧遗留（需用户拍板，代码不擅自改）**

1. 6 份词库头缺/错 `id:`：`cangjie.5.base`、`cangjie.5`、`cangjie.5.extended`、`cangjie.5.stem`（写成 `cangjie5.stem`）、`cangjie.5_char`、`wubi.86`
2. 3 份方案 `dict:` 仍写旧 id：`cangjie.5` / `cangjie.5_express` 写 `cangjie5`、`wubi.86` 写 `wubi86`；
   `cangjie.5` 反查写 `dictionary: luna_pinyin` + `prism: luna_quanpin`（词库现名 `pinyin.luna`，`luna_quanpin` 无词库）
3. `double_pinyin_flypy` / `double_pinyin_ziranma` 引 `pinyin.simp` / `pinyin.trad`：`dict/` 下**没有**这两份词库
   （旧位置在用户目录 `schema/pinyin/`）；且两个方案本身未过准入（系列摆放 `double_pinyin/flypy` ≠ id）
4. `cangjie.5_express` 的 `id:` 仍是 `cangjie5_express`（文件名 `cangjie.5_express`）→ 准入拒
5. **USER 根 `%APPDATA%\Rime-Next` 没有 `dict/`**：18 份词库仍在 `schema/` 下，严格只认 `dict/` 后取不到源
6. `doc/memo.md` 的用户目录表仍写 `dict/<名字>/<名字>.eassy.yaml`（细胞词库），**与现状（`dict/` = 词库、`eassy/` = 细胞词库）相反**，应更正
7. 生成清单的 `active` 会落到 `cangjie.5`（因 `pinyin` 已改名 `pinyin.luna`，`ensure_manifest` 的首选 `pinyin` 匹配不上）——要不要把首选改成 `pinyin.*`，待定

---

## 六·五、反查 = 用方案：`schema:` 升为正式键（2026-10-28 19:3x）

**发现的问题**：`stroke` / `wubi.86` 的 `reverse_lookup: schema: pinyin.luna` 被归一化改写成
`dictionary: pinyin.luna`，棱镜名按 C++ 缺省推成词典名 → 找 `build/pinyin/luna/pinyin.luna.prism.bin`，
而 luna 的棱镜叫 `pinyin`（`pinyin.luna.schema.yaml:85`）→ `Dictionary::open` 返回 `None` →
反查翻译器**静默建不出来**（打 `` ` `` + 拼音零候选、不报错）。
实测（同一份部署产物，把修复临时关掉）：`stroke` 打 `` `nihao ``、`wubi.86` 打 `znihao` 都是 **0 候选**；
修复后都出「你好」。

**定论（用户 19:2x 拍板）**：反查就是**用方案**，`schema:` 不该被悄悄降级成词典名。
棱镜名归目标方案自己（`pinyin.luna` 的棱镜叫 `pinyin`、`zhuyin` 的叫 `bopomofo`）。

**改法（代码，数据 0 改动）**

| 文件 | 改动 |
|---|---|
| `config/load.rs` | **删掉** `reverse_lookup: schema → dictionary` 的归一化；`schema:` 原样保留（编译产物里也是） |
| `engine/gear/reverse_lookup_translator.rs` | 新增 `resolve_target()`：`schema:` → `Schema::open(build)` 取目标方案自己的 `translator/dictionary` + `translator/prism`；没写 `schema:` 才走原生 `dictionary`(+`prism`)；写了但没部署 → 把 id 当词典名兜底 |
| `test/rime_next_dialect.rs` | 断言改成 `reverse_lookup/schema` 原样保留、不再出现 `dictionary` |
| `test/reverse_lookup_scheme.rs`（新） | 真机产物端到端，老布局自动跳过 |

**验证**：`--lib` **526/526**、`rime_next_dialect` 1/1、`component_assembly` 5/5、
`schema_manifest_generate` 3/3、`pinyin_typing` 1/1；
新部署产物 `build/stroke/stroke.schema.yaml` 里 `reverse_lookup: schema: pinyin.luna` 保留，
`stroke` `` `nihao `` 与 `wubi.86` `znihao` 都出「你好」。

**cangjie 反查已修（19:10 / 19:4x 验证）**：两份 cangjie 方案的 `reverse_lookup` 已把旧 id
`luna_pinyin` 换成 `dictionary: pinyin.luna` + `prism: pinyin`（全仓库 `luna_pinyin` 归零），
临时目录部署后实测 `cangjie.5` 打 `` `nihao `` 出「你好」。
⚠️ 这两处上方还留着注释「没法用单数的 `schema:` 一带而过」——**已过时**：`schema:` 现在能带棱镜，
写成 `schema: pinyin.luna` 等价且更短（两种写法都能跑，未改）。
剩下的 `dict: cangjie5` 仍缺（`dict/cangjie/5/cangjie.5.dict.yaml` 头里没有 `id:`），
所以 cangjie 主翻译器仍建不起来；这归"数据待改"第 1 条。

**仓库位置提醒**：`librime-next/` 是**独立 git 仓库**（父仓 `.gitignore` 第 16 行忽略了它），
所以父仓 `git status` 看不到它；它的改动要在 `cd librime-next && git status` 里看。

---

## 六·六、词库路径放宽到「id 分段前缀」+ 词典头方言（2026-10-28 19:5x）

**用户 19:4x 定的规则**：词库**不为片段建一层层文件夹**；只要它在 `dict/` 下、且目录是
**id 分段的某个前缀**就算摆放合适。引号一律不动。词典头补齐
`id / name / version / sort / use_preset_vocabulary`（后两项对片段是形式上的，写上）。

**代码落点**

| 文件 | 改动 |
|---|---|
| `lever/admit.rs` | 新增 `dict_placement()`：目录必须是 `dict/` + id 分段的**任意前缀**（`dict/`、`dict/cangjie/`、`dict/cangjie/5/`、…全合格；`dict/wubi/` 不合格）；`check_dict_identity` 的摆放关改用它（方案仍走 `scan::classify` 严格规则） |
| `dict/dict_compiler.rs` | `classified_dict_source()` 改成**精确优先、逐级退前缀**：`dict/cangjie/5/base/` → `dict/cangjie/5/` → `dict/cangjie/` → `dict/`；兄弟目录宽容搜索保留（仍只扫 `dict/` 一层） |

**验证（本次）**

- `dicts --shared Rime-Next` → **收下 9 / 抛弃 0**（cangjie 5 份平铺在 `dict/cangjie/5/` 全部通过）
- `--lib` **529/529**（新增 `dict_placement_accepts_id_prefixes`、`dict_outside_dict_dir_fails`）
- 副本上只把 `cangjie.5.schema.yaml` 的 `dict: cangjie5` 改成 `cangjie.5` → deploy **ok=true**，
  cangjie 主表把 4 个平铺源文件合成一张表（`[4 file(s)]`）
- 反查复测：`stroke` `` `nihao `` / `wubi.86` `znihao` → 「你好」

**历史状态**：真实数据现在**只差** `schema/cangjie/5/cangjie.5.schema.yaml:65` 的 `dict: cangjie5`
（旧 id，没有这份词库）；其余 8 份方案 + 9 份词库全部合规。
（2026-10-28 20:0x 已改成 `dict: cangjie.5`，见六·七；全仓 deploy 现在 `ok=true`。）

---

## 六·七、棱镜名 = 方案 id：双拼必须自己的棱镜（2026-10-28 20:1x）

**用户 20:0x 的规矩**：① 方案内部用自己的词库/棱镜就写方案名；② 反查一律写 `schema:`。
据此把 `prism:` 全面改成**方案 id**。

**先有实测（回答"双拼能不能借 luna 的棱镜、省一份产物"）：不能。**

`prism:` 名 = 棱镜**输出文件名**（`dict_compiler.rs::prism_target()` →
`<staging>/<prism_name>.prism.bin`，点分段即目录），部署时每个方案都拿**自己的
`speller/algebra` 重建那个文件** ⇒ 同名不是共享，是**后编译的覆盖前者**。

| 实验 | `pinyin.luna` 打 `nihao` | `flypy` 打 `nihc` | `ziranma` 打 `nihk` |
|---|---|---|---|
| 改前：flypy/ziranma 都写 `prism: pinyin.luna` | 「你好」✓ | 「你好」✓ | **「你坏」✗** |
| 借 luna：flypy/ziranma 写 `prism: pinyin` | **「你哈偶」✗** | 「你好」✓ | **「你坏」✗** |
| 改后：各自写自己的方案 id | 「你好」✓ | 「你好」✓ | 「你好」✓ |

⇒ 三份方案 = 三份棱镜，省不掉（同`algebra` 的才可能共用）。

**代码/数据落点**

| 位置 | 改动 |
|---|---|
| `schema/pinyin/luna/pinyin.luna.schema.yaml` | `prism: pinyin` → `prism: pinyin.luna` |
| `schema/zhuyin/zhuyin.schema.yaml` | `prism: bopomofo` → `prism: zhuyin`（词库是 terra，必须显式写） |
| `schema/double_pinyin/{flypy,ziranma}/*.schema.yaml` | 生成物，`prism:` = 各自方案 id |
| `Rime-Next/parser/shuangpin/{xiaohe,ziranma}.parser.yaml` | `id:` 改成带点（`double_pinyin.flypy`）、`prism:` = 同 id、`version: "2026.10.28"` |
| `style/src/bin/shuangpin_schema.rs` | **id 里的点 = 目录层级**（`schema/double_pinyin/flypy/`），原来按 id 原样建目录（带点就只有一个文件夹，与 `scan::classify` 摆放规则冲突）；预编辑模板按 id **末段**判断（`ends_with("ziranma")`），不再写死无点 id |
| `schema/pinyin/terra/pinyin.terra.schema.yaml` | 反查 `dictionary: stroke` → `schema: stroke` |
| `schema/cangjie/5/cangjie.5.schema.yaml` | 反查已是 `schema: pinyin.luna`，删掉"只能写 dictionary+prism"的过时注释 |
| `style/tests/schema_syntax.rs` | 旧无点 id/旧路径/`prism: pinyin`/`dictionary:` 断言全部跟着改 |
| `doc/memo.md` | 用户文件夹表更正：`dict/`=词库、`eassy/`=细胞词库（原写反了） |
| `engine/gear/reverse_lookup_translator.rs` | 注释里旧棱镜名（`pinyin`/`bopomofo`）订正 |

**验证（本次实跑）**

- `deploy --shared Rime-Next --user <临时目录>` → **`ok=true`**（六·六 记的那 1 行
  `dict: cangjie5` 在上一棒收尾时已改成 `cangjie.5`，本棒开工复核时已是 `cangjie.5`）
- 棱镜产物各归各位：`pinyin/luna/pinyin.luna.prism.bin`(43964)、`double_pinyin/flypy/double_pinyin.flypy.prism.bin`(23932)、
  `double_pinyin/ziranma/double_pinyin.ziranma.prism.bin`(23960)、`zhuyin/zhuyin.prism.bin`(397156)
- 三方案实测：luna `nihao` / flypy `nihc` / ziranma `nihk` 全部首候选「你好」（改前 ziranma 出「你坏」）
- 反查回归：`stroke` `` `nihao ``、`wubi.86` `znihao`、`cangjie.5` `` `nihao `` 都出「你好」；
  `pinyin.terra` `` `hspnz ``（笔画）出 109 个候选 → `schema: stroke` 生效
- `cargo test -p librime-next --lib` → **529 passed / 0 failed**；
  `cargo test -p weasel-style --test schema_syntax` → **6/6**；`reverse_lookup_scheme` → 2/2
- `manifest` → 收下 8 / 抛弃 0；`dicts` → 收下 9 / 抛弃 0
- 生成器幂等：用 fake `APPDATA` 重跑 `shuangpin-schema.exe`，产物与仓库里的两份**逐字节相同**

**待办 / 注意**

- `%APPDATA%\Rime-Next\schema\*` 与 `C:\Program Files\Weasel-Next\...` 还是旧棱镜名（本轮只动仓库数据）；
  真正装机要重新部署一次才生效（要用户操作）。
- 老 `.custom.yaml` 若按无点 id 命名（`double_pinyin_flypy.custom.yaml`），方案 id 改点后不再匹配；
  用户目录里如有这类补丁要改名成 `double_pinyin.flypy.custom.yaml`。

---

## 六·八、词库键统一成单数 `dict`（2026-10-28 20:5x）

**用户 20:5x 定**：`translator:` 里的词库键一律写 **`dict:`**（与 `id` / `column` 同一套单数方言），
并追加一句「**去掉兼容，只要 dict**」⇒ 全链路只认 `dict`，**没有** `dictionary:` 兼容层。

**数据**：`stroke`、`pinyin.terra` 两份还写全名 `dictionary:`，改成 `dict:`；其余 6 份本来就是 `dict:`。
⇒ 8/8 方案源文件都是单数键。

**代码（只认 `dict`）**

| 文件 | 改动 |
|---|---|
| `config/load.rs` | **删掉** `translator:` 那条约化规则（既不再 `dict`→`dictionary`，也不补 `dictionary`→`dict`）；`dict` 原样透传到编译产物 |
| `engine/gear/registry.rs` | `translator_dict_name()` 只读 `dict` |
| `lever/deployment_tasks.rs` | 要不要编词典 / 词典清单 / 变体词库：只读 `/translator/dict`、`node["dict"]`；报错文案 `translator/dict not specified` |
| `lever/admit.rs` | 「词典声明」提示只读 `/translator/dict` |
| `engine/schema.rs` | `first_translator_dict()` 只读 `{ns}/dict` |
| `engine/gear/reverse_lookup_translator.rs` | 反查表源改读 `{target}/dict`（默认 `translator/dict`）；自建的测试夹具也改 `dict:` |

**不属于这次方言的两处 `dictionary`（保持原样）**：
`custom_phrase:` 块的 `dictionary:` 是自定义短语组件的 C++ 键；`reverse_lookup:` 的原生写法
`reverse_lookup/dictionary` 是"反查表源"的键（我们的方案一律用 `schema:`，只有原生 rime 方案才走它）。

**测试跟着改**
- `rime_next_dialect`：断言编译产物是 `translator/dict`、且不出现 `dictionary`（不再有"归一化"这回事）。
- `style/tests/schema_syntax.rs`：stroke 断言 `dict`、并断言没有 `dictionary`。
- `all_schemas_e2e.rs` / `lever_deploy.rs`：读 staged 产物只读 `dict`。
- `test/e2e/build/*.schema.yaml`（C++ 产物夹具）：**把 `translator:` / `zaoci:` 块里的 `dictionary:` 迁成 `dict:`**
  （`custom_phrase:` / `reverse_lookup:` 两处不动），否则 engine_e2e / component_assembly 会读不到词库。
- `engine_e2e::repro_real_luna_pinyin`、`c_api::host_smoke`：这两条打的是**真机部署产物**；
  旧产物还是 `dictionary:`，现在读不出词库 ⇒ 加"真实 build 里没有 `dict:` 就跳过"的守卫，
  免得把"还没重新部署"报成代码回归。

**验证（本次实跑）**

- `deploy --shared Rime-Next --user <临时目录>` → **`ok=true`**
- 编译产物 8/8 都是 `dict:`（`custom_phrase` 的 `dictionary: ""` 是另一组件的键）
- 运行期实测：luna `nihao` / flypy `nihc` / ziranma `nihk` 出「你好」；cangjie `` `nihao ``、stroke `` `nihao `` 出「你好」；
  terra `` `hspnz `` 出 109 候选
- `cargo test -p librime-next --lib` → **529 passed**；`component_assembly` **5/5**；
  `engine_e2e` **69 passed / 1 failed**（唯一红的是已知的 `wubi86_table_translator_end_to_end`：
  写死 `Rime-Next/schema/wubi86/wubi86.schema.yaml`，该目录已不存在，**不是本次改坏的**）；
  `rime_next_dialect` 1/1；`reverse_lookup_scheme` 2/2；`weasel-style schema_syntax` 6/6；`manifest` 8/0；`dicts` 9/0

**注意（parity / 旧数据）**

- staging 产物里词库键是 `translator/dict`，与 C++ 原版 dump **不再逐字一致**
  （`librime-next/doc/STATUS.md` 里"translator 字段逐字一致"那条要按这个读）。
- `librime-next/fixture/**` 是 C++ 历史快照（仍是 `dictionary:`），**没有任何测试在读它**，未迁移；
  真要拿它当运行输入，得先按 `test/e2e/build` 同样迁一遍。
- **旧部署产物（%APPDATA% / Program Files）在新引擎下会读不出词库** —— 必须重新部署。
- `.custom.yaml` 里写 `translator/dictionary:` 的补丁不生效（键已不存在；patch 也绕过归一化）。

**六·八 收尾时做的全量引用审计（2026-10-28 21:0x）**

脚本 `C:\Code\_audit_refs.py`：8 方案 + 9 词库的 id↔路径、dict/prism、dependency、`__include`、
`reverse_lookup/schema`、`__patch`、`import_tables`、model 预设、opencc 配置，逐条核对（49 条引用）。
结果 **0 错误**，另修两处：

| # | 问题 | 修法 |
|---|---|---|
| 14 | `zhuyin.schema.yaml` 的 `__patch: - bopomofo.custom:/patch?` 还是**旧 id** → 用户的 `zhuyin.custom.yaml` 永远不生效 | 改成 `zhuyin.custom:/patch?` |
| 15 | `pinyin.terra.schema.yaml` 写的是复数 `dependencies:`（其它 5 份都是单数 `dependency:`） | 改单数 |

其余：棱镜 8 个名字**一名一主**；依赖环 `pinyin.luna ↔ stroke`（互为反查目标，部署按 visited 去重，**合法**）；
`dict/cangjie/5/cangjie.5.char.dict.yaml` 没有任何方案/词库引用（**未使用的词库**，留着无害）。

---

## 七、常用命令

---

## 七、常用命令

```bash
# 部署（重新生成 staging 产物）
cd /c/Code/Weasel-Next && ./target/debug/librime-next.exe deploy

# 打字测试（nihao → 你好）
cd librime-next && cargo test --test pinyin_typing

# 识别标准单测
cargo test --lib lever::admit

# 词库识别（收下/抛弃名单，不写任何文件）
./target/debug/librime-next.exe dicts --shared C:/Code/Weasel-Next/Rime-Next

# 反查端到端（`nihao / znihao → 你好；老布局自动跳过）
cargo test -p librime-next --test reverse_lookup_scheme

# 编 rime.dll 替代品（cdylib）
cargo build --release -p librime-next --lib   # → target/release/librime_next.dll

# 重启后端（改配置/换 dll 后）
# 停 Weasel-Next-* 进程 → 启动 Weasel-Next-Server.exe
```

---

## 八、下一步

1. 用户注销重登 + 卸载旧版小狼毫 → 复测实机打字（是否出「你好」、白块是否还在）
2. 按复测结果定位：候选窗残留（Renderer）/ 未激活（输入法状态）/ 方案加载
3. 推进代码侧待办：清单生成 → 其余方案标准化 → default 收口

---

## 六·九、新数据真正装机（2026-10-28 21:2x～21:3x）

**目标**：把仓库里的 8 个标准方案（`dict/` + 带点 id + 自己的棱镜）装到真实输入法，
而不是只在临时目录里 `ok=true`。

**做了什么**

| # | 动作 | 结果 |
|---|---|---|
| 1 | `cargo build --release -p librime-next --lib` → `target/release/librime_next.dll`（8,583,168 B，md5 `81f36e901955a0f1a3ed9513a7d8f407`） | 复制成 **`Program Files\Weasel-Next\rime-next.dll`**（新槽位；旧 `rime.dll` 原样保留 = 回滚开关） |
| 2 | 仓库 `Rime-Next\` 合并进程序目录 `Program Files\Weasel-Next\Rime-Next\`（`schema` 8 份、`dict/` 9 份、`parser`、`model.yaml`、`default.yaml` 等） | 旧 40 个方案目录**保留未删**（只新增，不 purge） |
| 3 | 用户清单切到 8 个新 id（`active: pinyin.luna`），`schema.yaml` 与 `schema.toml` 同步 | 旧清单备份在 `C:\Code\_bak_user_20261028\` |
| 4 | `librime-next deploy --shared "C:/Program Files/Weasel-Next/Rime-Next" --user "%APPDATA%\Rime-Next"` | **`ok=true`**；8 个棱镜产物就位（`cangjie/5`、`double_pinyin/{flypy,ziranma}`、`pinyin/{luna,terra}`、`stroke`、`wubi/86`、`zhuyin`） |
| 5 | 重启 `Weasel-Next-Server`；引擎模块确认加载 `rime-next.dll` | 日志 `loaded schema preference ... active="pinyin.luna" selected=8` |

**实测**

- 记事本实打：luna `nihao` + 空格 → 上屏「你好」✅（真实 TIP → Server → Engine 全链路）
- 引擎级（`Session::create_with_roots(real build, [用户目录, 程序目录])`）：
  `pinyin.luna` `nihao` / `double_pinyin.flypy` `nihc` / `double_pinyin.ziranma` `nihk`
  **首候选都是「你好」** ✅；语法模型 4 本按 `zh-han.{simp,trad}` 前缀全部挂上（用户 `model/` 已是新命名）
- 反查（对真实 build）：`stroke` `` `nihao ``、`wubi.86` `znihao` → 「你好」2/2 ✅

**注意 / 坑**

- **合成按键会被 TIP 锚点握手干扰**：用 Python `keybd_event` 连续打多键时，
  engine 每次「开始等锚点」后若布局未及时更新，会丢掉前面的组合、只剩最后一两键
  （实测出现「亟」「你和出」这类残字）。真人手打不受影响；**自动化复测要放慢/重试，
  不要据此判代码回归**（引擎级 Session 测试才是稳的判据）。
- `schema.toml` 必须与 `schema.yaml` 一起改：装机的 Engine 仍读 toml，
  不改会每次启动刷 `WARN schema.toml 里的方案当前不可用`。
- 旧 `build/` 产物没删（`luna_pinyin`、`yuling*` 等目录还在），只是不在清单里、不会被加载。

**回滚（一条命令级）**

1. 删 `Program Files\Weasel-Next\rime-next.dll`（引擎回落到旧 `rime.dll`）
2. 还原 `C:\Code\_bak_user_20261028\schema.yaml` / `schema.toml`
3. 重启 `Weasel-Next-Server`
（程序目录的旧数据没删，所以旧方案仍可编译。）

**仍未做**：旧 26/40 个方案（yuhao/yuling/tiger/yustar…）没迁到新布局，
现在装机里可选方案 = 这 8 个；要全部保留得按 `dict/` + 带点 id 逐个标准化。

### 六·九·补、装机后候选窗变纯白 → 主题 DLL 是 TOML 旧版（2026-10-28 21:35）

**现象**：装机后打字，候选窗是白底黑字的默认外观，`color_scheme: steam` 没生效。

**根因**：两处装机缺口里的第二处 —— **程序文件夹里的 `weasel_theme_official.dll` 是 10-24 的旧构建**。
用二进制串核对：旧 DLL 里 `theme.toml` ×8、`theme.yaml` ×0；仓库 `target/release` 那份正好相反
（`theme.yaml` ×8、`theme.toml` ×0）。而主题数据 10-27 起已全部改成 YAML
（`theme/theme.yaml` + `theme/default/default.theme.yaml`）⇒ 旧 DLL 读不到主题，
静默回落到内置默认配色（白底），**不报错**。

**修法**：`target/release/weasel_theme_official.dll`（8,055,808 B，sha256 `953EC5F4…62CC8`）
复制进 `Program Files\Weasel-Next\`，备份旧文件在 `C:\Code\_bak_pf_theme_20261028\`，重启 Server。
候选窗恢复深色（steam）。

**坑**：主题 DLL 被 Renderer 独占加载，**必须先停 `Weasel-Next-*` 进程再复制**，
否则 `copy` 报「being used by another process」。

**装机缺口清单（下次装包别漏）**：① 数据（`schema/` + `dict/` + `parser/` + `model.yaml` …）；
② **`weasel_theme_official.dll` 等运行期组件要跟着一起更新**；
③ 用户 `schema.yaml` / `schema.toml` 清单；④ `rime-next.dll`。

---

## 六·十、设置界面唯一化（2026-10-28 21:5x，用户要求「不许同时存在多个设置界面」）

**做法**

- 新增 `Setting/src/single_instance.rs`：
  第二个实例拿到 `AlreadyRunning` 后不建窗口，用 `EnumWindows` 按**进程 exe 全路径**找到已在运行的那个窗口，
  `ShowWindow(SW_RESTORE)` + `BringWindowToTop` + `SetForegroundWindow` 拉到前台，然后退出。
  *不用 `FindWindowW` 按标题/类名*：Slint 的窗口类名不是常量，标题还随语言/方案变；
  按 exe 路径匹配在沙箱/不同完整性级别下也更稳（与 `switcher_dialog.rs` 找 broker 窗口的结论一致）。
- `Setting/src/main.rs` 的 `main()` 开头接锁：`SingleInstance::acquire("settings-ui")`
  （复用 `common/src/process.rs`，与 server/broker/renderer 同一套）。
- **只锁「无参数」的主界面启动**：带 `/style`、`/switcher`、`/dict`、`/stat` 的对话框不占这把锁 ——
  否则设置窗开着时，托盘里的「方案选单 / 界面风格」会全部打不开（那些入口是另起进程带参数）。

**验证**：`cargo check -p Weasel-Next-Setting` 通过（0 error）；双开实测见下。

---

## 六·十一、ARM64（Windows on ARM）安装包（2026-10-28 21:4x～22:1x）

**结论**：0.18.5 出了两个包，**架构写进文件名**：

| 包 | 大小 | sha256 |
|---|---|---|
| `artifact\installer\Weasel-Next-0.18.5-x64-setup.exe` | 47,519,492 B | `a80bcb19c9c4e83b060fcefc6c62785a9aaca7c904661c30a950156b8843066a` |
| `artifact\installer\Weasel-Next-0.18.5-arm64-setup.exe` | 34,416,255 B | `85c693fa48772e6c38c49967ee05f3edfdbf56d61ebbbb0d5446c7611b0db8ef` |

**前因（上一棒留下的第一步）**：`cargo check --workspace --target aarch64-pc-windows-gnullvm`
这次重跑 **exit 0**（只有 `Setting` 的 `unused BOOL` 警告）；`cargo build --release --target aarch64-pc-windows-gnullvm --workspace`
**5m54s 全绿**，8 个产物全 `coff-arm64`。⇒ ARM64 没有可移植性阻塞，直接进打包。

**改了什么**

| 文件 | 改动 |
|---|---|
| `installer/weasel-next.nsi` | ① `RELEASE_DIR` 改 `!ifndef` 可覆盖（否则 `/D` 打不进去，会**静默打成 x64 包**）；② `CheckPlatform` 加 `TARGET_ARM64`：ARM64 包只认 `${IsNativeARM64}`，x64 包反过来明确拒绝 ARM64；③ `Section librime` 的 ARM64 分支装 `rime-next.dll` + arm64 运行库，**不装** `rime.dll` / `libglog` / `libopencc`；④ ARM64 注册 TIP 若失败，用 `$WINDIR\Sysnative\regsvr32.exe` 兜底重试一次 |
| `script/build-installer-arm64.ps1` | 新脚本：ARM64 release → x86 TIP → x64 包（顺带刷新 `payload-files.nsh`）→ 组 `target\arm64-release` → `makensis /DTARGET_ARM64 /DRELEASE_DIR=…` |
| `script/build-installer.ps1` | `$requiredFiles` 加 `target\release\rime-next.dll` |
| 10 个 `Cargo.toml` + `Cargo.lock` | 版本 0.18.4 → **0.18.5**（含 Tip，安装器版本号取自 `Tip\Cargo.toml`） |

**包内架构矩阵（7z 解包后逐个 `llvm-objdump -f` 核过）**

| 包 | 64 位 TIP | 32 位 TIP | 引擎 |
|---|---|---|---|
| x64 | `Weasel-Next.dll` = AMD64 | `Weasel-Next-32.dll` = i386 | `rime.dll`(C++) + `rime-next.dll` |
| arm64 | `Weasel-Next.dll` = **coff-arm64** | `Weasel-Next-32.dll` = i386 | **只有** `rime-next.dll`(arm64) |

arm64 包里 `Server/Engine/Renderer/Setting/shuangpin-schema/weasel_theme_official/libc++/libunwind/libwinpthread/rime-next`
全部 `coff-arm64`；`Uninstall.exe` 是 NSIS 的 x86（仿真跑，正常）。两包的 `Weasel-Next-32.dll` 是**同一个文件**
（md5 `ae4d320d4a378a792b098463e5cc0afb`，由 `build-tip32.ps1` 产出，i686）。数据侧两包一致：8 份 `*.schema.yaml`、9 份 `*.dict.yaml`、`weasel.yaml`/`default.yaml`/`model.yaml` 齐全。

**关键技术事实（下次别再摸一遍）**

- **ARM64 产物只依赖一个非系统 DLL：`libunwind.dll`**。逐个 `llvm-objdump -p` 查导入表：没有 `libc++.dll`、
  没有 `libwinpthread-1.dll`、没有 `libgcc_s_seh-1.dll`。包里多装的那两个只是保底；
  `libglog` / `libopencc` 是 x64 C++ `rime.dll` 的依赖，**ARM 包完全不需要**。
- `${IsNativeARM64}`（NSIS 3.08 `x64.nsh`）读 KUSER_SHARED_DATA 的 `NativeMachine`，
  **跟安装器自身是 x86 进程无关** ⇒ 32 位 NSIS 安装器在 ARM64 上也能判对平台。
- ARM64 的 `weasel_theme_official.dll` 是 **YAML 代**（二进制里 `theme.yaml`×4 / `theme.toml`×0，
  1,005,056 B）。**坑 18 对 ARM 同样成立**：别拿 x64 那份 8 MB 的顶替。
- `Engine` 优先加载 `rime-next.dll`（`Engine/src/librime/mod.rs:164-174`）⇒ ARM 包只放这一个就够，
  `rime.dll` 槽位可以空着。
- **x64 包的 `target\release\rime-next.dll` 必须现拷**：仓库里原来那份是 10-27 的旧构建
  （md5 `a2ca1a178e727f12e3ec772aa3ee92a6`），0.18.4 那次差点把它打进 x64 包。
  0.18.5 已改成构建后从 `target\release\librime_next.dll` 现拷（现 md5 `efb606ef4ff4f74bc23db452815ef997`，8,584,704 B）。
- 重新编出来的 `librime_next.dll` 与装机在用的那份（md5 `81f36e90…`，21:17 构建）**字节不同但源码相同**
  （`librime-next/src` 自 21:00 后无改动；差异来自 `-p librime-next --lib` 与 `--workspace` 的特性归一化），
  所以引擎语义没变、`cargo test -p librime-next --lib` 仍是判据。

**没验证的（说清楚，别当成已验）**

- **没有骁龙真机**（10-26 起一直挂着这个问号），ARM64 包只做到**「打包层面验证」**：
  `makensis` 成功、包内逐个 PE 机器类型正确、版本信息 0.18.5、数据齐全。
  **安装、部署、实际打字在 ARM64 上都没跑过。**
- ARM64 的 TIP 注册依赖「关掉 WOW64 重定向后的 `System32\regsvr32.exe`（arm64）」这一条，
  **x64 机器无法验证**（所以加了 `Sysnative` 兜底）。真机上若仍失败，先看
  `%TEMP%\Weasel-Next-install.log` 里的 `WARN: … 改用 Sysnative 重试` 这行。
- ARM64 只做了 `check`/`build`，**没跑测试二进制**（没有 ARM 环境）。
- 旧 26/40 个方案仍未迁到新布局（同六·九），ARM 包里的可选方案也只有这 8 个。

---

## 六·十二、版本化安装（2026-10-28 23:5x～2026-10-29 00:2x）

**用户报的问题**：点安装会「重复安装」；要求**有版本号就装到
`C:\Program Files\Rime-Next\Weasel-Next-<版本>`**，**只有同版本才算重复安装**（提示覆盖修复），
不同版本必须并存、旧目录保留可回滚。

**规则**

| 情形 | 行为 |
|---|---|
| 全新安装 | 装到 `${PROGRAMFILES64}\Rime-Next\Weasel-Next-${PRODUCT_VERSION}`，写 `PRODUCT_KEY\InstallDir` / `CurrentVersion`、`UNINSTALL_KEY\DisplayVersion` |
| **同版本目录里已有** `Weasel-Next-Server.exe` | `$IsUpgrade=1`，提示「已安装 ${PRODUCT_VERSION}…继续将**覆盖修复**这个版本，用户数据不受影响」；取消则 `SetErrorLevel 1` 退出 |
| 已安装**别的版本**（版本化目录） | 提示「**并存安装**到新目录，旧版本目录保留、可回滚；输入法与开机自启动会切到新版本」 |
| 已安装**旧布局**（`Program Files\Weasel-Next`） | 单独文案：按版本化目录装到新目录，**旧目录不会自动删除**，确认新版可用后可手动删 |
| 卸载 | **只删自己这个版本目录**；TIP 注销 / 快捷方式 / `PRODUCT_KEY` / `UNINSTALL_KEY` / Run 键都先比对「是否指向本目录」才动 |

**实现位置**

| 文件 | 改动 |
|---|---|
| `installer/weasel-next.nsi:46-47` | 新增 `PRODUCT_ROOT` = `$PROGRAMFILES64\Rime-Next`、`PRODUCT_DIR` = `…\Weasel-Next-${PRODUCT_VERSION}`；`:51` `InstallDir "${PRODUCT_DIR}"` |
| `installer/weasel-next.nsi:147` | `.onInit` 把 `$INSTDIR` 定成 `PRODUCT_DIR`（**不再沿用上次安装记录里的旧目录**） |
| `installer/weasel-next.nsi:151-182` | 同版本判断用 `FileExists "$INSTDIR\Weasel-Next-Server.exe"`；否则若登记里有旧安装 ⇒ 并存提示。旧布局用 `StrLen`+`StrCpy` 取前缀判断（**NSIS 没有 `${StrStr}`**） |
| `installer/weasel-next.nsi:146,153-156,234-236` | `$ScanExtraDir` = 旧 `InstallDir`；进程扫描覆盖它，并再发一次 `--shutdown "$ScanExtraDir"`（否则旧 broker 占着命名管道，新 broker 起不来） |
| `installer/processes.nsh:8,80-81` | `Var ScanExtraDir`；扫描/强杀时按 `$ScanExtraDir\$ProcessName` 也匹配（仍然逐路径核对，不按名字乱杀） |
| `Server/src/installer.rs:34-43,56` | 新增 `is_own_broker_command()`：Run 值指向**任意目录**的 `Weasel-Next-Server.exe` 就改指当前目录；用户手改的其它命令保留（原来「已存在就不改」⇒ 升级后自启动仍拉旧版） |
| `installer/weasel-next.nsi:629-648` | `un.onInit` 不再要求 `$INSTDIR` 等于产品级 `InstallDir`（卸载器里的 `$INSTDIR` 是**构建时烘进去**的），只校验形状：`== ${PRODUCT_DIR}` 且以 `${PRODUCT_ROOT}\` 开头 |
| `installer/weasel-next.nsi:690-745` | 卸载守卫：64 位 TIP 先读 `HKCR\CLSID\{16A7AEA9-…}\InprocServer32`、32 位先 `SetRegView 32` 读 WOW6432Node，**只有等于 `$INSTDIR\…` 才 `regsvr32 /u`**；快捷方式 / Run 键 / 两个登记键同理只在 `PRODUCT_KEY\InstallDir == $INSTDIR` 时才动 |

**验证（两轮实装，均 `/S` 静默）**

- A 全新装：落到 `C:\Program Files\Rime-Next\Weasel-Next-0.18.5`；`HKLM\Software\Weasel-Next`
  `InstallDir`/`CurrentVersion=0.18.5`；`DisplayVersion=0.18.5`；
  `HKCR\CLSID\{16A7AEA9-…}\InprocServer32` → 新目录的 `Weasel-Next.dll`；
  HKCU Run `小狼毫Next算法服务` = `"…\Weasel-Next-0.18.5\Weasel-Next-Server.exe"`（带引号）；
  目录 24 个文件 + `Rime-Next\`；开始菜单 7 个快捷方式齐全。
- B 模拟旧版本升级：0.18.5 目录改名 `.hold`，登记与 Run 指回 `0.18.4`，再 `/S` 安装 ⇒
  **两个版本目录并存**（0.18.4 原样保留），登记/TIP/Run 全部切到 0.18.5。

**坑（写进检查点，别再摸）**

1. `Start-Process -Verb RunAs -Wait` 跑安装包会**一直挂着**（安装早已完成、setup 进程已退）；
   验证要**不带 `-Wait`** + 轮询目录/进程。
2. `un.onInit` 里 `$INSTDIR` 是编译期烘进去的，旧版本目录里的卸载器值不同 —— 不能拿它跟产品级 `InstallDir` 比。
3. 卸载旧版本时，TIP 注册看起来「指向本产品」但其实是**新版本**的；无条件 `regsvr32 /u` 会把新版本 CLSID 删掉。
4. `PROGRAMFILES64` 是运行期变量，`!define PRODUCT_DIR` 里能用，但**别在编译期 `!echo` 当路径**。

**ARM64 包已按新逻辑重打（2026-10-29 00:24，00:33 随 UI 补充重打）**

`pwsh -File script\build-installer-arm64.ps1 -SkipBuild -SkipX64`（先核过
`target\aarch64-pc-windows-gnullvm\release` 产物齐全）⇒
`artifact\installer\Weasel-Next-0.18.5-arm64-setup.exe`，**34,416,646 B**，
sha256 `69DD9031DFE2B8620D1662BDB5384D5311750A329EDE0985B7102399367078DB`，makensis 仅剩 2 个旧警告
（`UseUiAccess`、`uninstall_failed`）。打包层面复核：包内 133 个文件、8 `*.schema.yaml` / 9 `*.dict.yaml`、
`weasel.yaml`/`default.yaml`/`model.yaml` 齐全；8 个产品二进制逐个 `llvm-objdump -f`：
Server/Engine/Renderer/Setting/`Weasel-Next.dll`/`rime-next.dll`/`shuangpin-schema`/`weasel_theme_official` 全
`coff-arm64`，`Weasel-Next-32.dll` 是 `coff-i386`，**无 `rime.dll`**。

- 该包含的是 **00:18 改过的 NSI**（makensis 日志确认处理的是同一份 `installer\weasel-next.nsi`），
  所以版本化目录/并存/TIP-Run-卸载守卫全部继承。**但 ARM64 的版本化路径没在真机跑过**（没有骁龙机器）。
- 顺带重建了 x86 TIP（`target\release\Weasel-Next-32.dll`，5,410,304 B，sha256 `0F9C7403D52C…`）。
  它和 x64 包里那份（md5 `ae4d320d…`）**字节不同**：`Tip/`、`common/` 源码自 10-28 08:29 起没改，
  差异来自构建环境（PATH 里的 llvm-mingw 会换掉链接器），**不是代码回退**；两包从此各带一份自己编的 x86 TIP。

**安装目录可见化 + x64 包重打（2026-10-29 00:32）**

用户报「安装时显示的安装路径还是 `Weasel-Next-0.18.4`」。查下来：包/登记/快捷方式/Run 全是 0.18.5，
只有 `C:\Program Files\Rime-Next\Weasel-Next-0.18.4` 目录还留着（里面只有 10-28 08:33 的 `weasel_tip.dll`）；
安装器原来**任何界面都不显示目标目录**，用户看到的 0.18.4 只可能来自「检测到已安装版本 $InstalledVersion（$0）」这类
提示里的**旧路径**，或者跑的是旧包 —— 无从分辨。所以直接把目标目录写出来：

- 结束页：`MUI_FINISHPAGE_TEXT` / `_REBOOT` → 「小狼毫Next 已安装到：$INSTDIR」（运行期展开）。
- 详情面板 + 安装日志：`Section "Weasel-Next"` 开头 `DetailPrint "安装到：$INSTDIR"` +
  `InstallLog "INFO: 安装目录：$INSTDIR"`。

`Weasel-Next-0.18.5-x64-setup.exe` **47,512,835 B**，sha256
`A80273FA7E6BB00C7C9DA5C724EB0370EB67E272144F00FEA48879561BCBFF6A`（00:32；全量 `cargo build --release --workspace`
重编后打包，含 00:18 那次 `Setting/src/main.rs` 改动）。

**未做（说清楚）**

- 同版本「覆盖修复」的**交互提示没有真人点过**（只用 `/S` 验证了并存路径与全新装）。
- **卸载旧版本没实测**（`Weasel-Next-0.18.4` 目录里现在只有 `weasel_tip.dll`，没有卸载器）。
- ARM64 包仍只到「打包层面验证」：安装、TIP 注册、部署、打字都没在 ARM 上跑过。

---

## 六·十三、Direct2D 设置界面外壳 v1（2026-10-29 00:4x）

用户 2026-10-28 否掉 Slint 假玻璃后选的路线：**所有 GUI 统一 Direct2D，先做外壳、再逐页迁移**。
本棒只做外壳，**不接管默认设置入口**——无参数启动仍是 Slint 版 `Weasel-Next-Setting.exe`。

**新增**

| 文件 | 内容 |
|---|---|
| `Setting/src/d2d_shell/main.rs` | 窗口、消息循环、布局与绘制（标题栏/页签/占位卡片）、鼠标命中、DPI |
| `Setting/src/d2d_shell/present.rs` | D3D11 + DXGI 组合交换链 + DirectComposition + D2D 设备上下文（照 `theme/src/presenter.rs`，去掉换主题相关） |
| `Setting/src/d2d_shell/acrylic.rs` | `SetWindowCompositionAttribute` + `WCA_ACCENT_POLICY`（照 `theme/src/accent.rs`） |
| `Setting/Cargo.toml` | 新增 bin `Weasel-Next-Setting-D2D`；`windows-core`/`windows-strings` 从可选改为必选（两个 bin 都要用）；新增依赖 `weasel-theme-support`（复用候选窗那套已核对的 D2D/DWrite/D3D11/DComp 绑定）；`gui` 特性里的 `dep:windows-*` 同步去掉 |

产物：`target\release\Weasel-Next-Setting-D2D.exe`（1.7 MB，未进安装包——`build-installer.ps1` 的文件表是显式的）。

**技术要点**

- 窗口：`WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU`
  + `WS_EX_NOREDIRECTIONBITMAP | WS_EX_APPWINDOW`；`WM_NCCALCSIZE` 返回 0 让客户区铺满（自绘标题栏），
  `WM_NCHITTEST` 自己算 6px 缩放边与 `HTCAPTION`（三个标题按钮和页签必须回 `HTCLIENT`，否则收不到点击）。
- 玻璃：DWM 亚克力染色 `#991B1F27` + `DWMWA_USE_IMMERSIVE_DARK_MODE` + `DWMWA_WINDOW_CORNER_PREFERENCE=DWMWCP_ROUND`；
  面板本身画 47% 半透明圆角矩形，圆角外逐像素透明 ⇒ 透出背后的模糊。
- 绘制：`ID2D1DeviceContext1` 上 `Clear(transparent)` → 圆角面板/页签/卡片 → `DrawText`（DWrite，微软雅黑 UI）→
  `EndDraw` + `Present(1,0)`。

**踩过的坑（都验证过，别再摸）**

1. **`weasel_theme_support::d2d_bindings` 里已经有全部窗口函数**（`RegisterClassW`/`CreateWindowExW`/`GetMessageW`/
   `LoadCursorW`/`SetCapture`/`GetCursorPos`/`InvalidateRect`/`DwmSetWindowAttribute`…），不需要另生成绑定。
   但它的 `HWND` 与 `bindings` 里的 `HWND` **不是同一个 Rust 类型**，混用编译不过 —— 全程只用 `d2d_bindings` 那一套。
2. **`PCWSTR` 必须自己补结尾 0**：`text.encode_utf16().collect::<Vec<u16>>()` 不带 NUL，
   `CreateTextFormat` 会失败（现象是启动即「文本格式创建失败」、窗口空白）。
3. **初始窗口尺寸要按 `GetDpiForWindow` 折算**：本机 125%（dpi=120），1000 DIP 必须建成 1250 像素，
   否则内容被裁一半（第一版就是这个 bug）。
4. **不能用 `ID2D1HwndRenderTarget`**：它拿不到逐像素 alpha，圆角外是黑的。必须 DComp；
   `ResizeBuffers` 前要 `SetTarget(None)` **并丢掉旧位图**，否则 `DXGI_ERROR_INVALID_CALL`。
5. **DComp 一个 HWND 只能 `CreateTargetForHwnd` 一次**，`Presenter::drop` 里必须 `SetRoot(None)`。
6. **截图**：DComp 窗口用 `CopyFromScreen` 抓，窗口被别的窗口挡住会抓到别人 ——
   `script/capture-window.ps1` 新增 **`-BringToFront`**（`SetWindowPos(HWND_TOPMOST)` + `SetForegroundWindow` 再抓）。

**验证（2026-10-29 00:48～00:52）**

- 启动即出窗：1250x825 @ (374,127)（125% 缩放，按工作区居中，不压任务栏）。
- 截图 `C:\Code\_d2d_page1.png`（默认页「常规」）、`_d2d_page2.png`（合成点击「外观」后标题/选中态切换）✅
- 合成点击右上「✕」→ 进程退出 ✅（`C:\Code\_d2d_test.ps1`）
- 玻璃实证：用**白底**与**棋盘格底** WinForms 窗口垫在后面（`C:\Code\_white_backdrop.ps1` + `_probe_glass2.ps1`）——
  面板内棋盘被糊成均匀灰、面板外棋盘清晰；四角透出底色（圆角）⇒ DWM 模糊 + 逐像素 alpha 都生效 ✅
  （对比图 `C:\Code\_probe_checker.png`、`_d2d_glass_pad.png`）
- **Slint 版仍照常启动**（`小狼毫Next：设置`），默认入口未被改动 ✅

**v1 没做的（下一棒接力点）**

- 页面内容全是占位，**没接设置核心**（`weasel_settings` 的配置/表单/会话读写一行没接）。
- 无键盘导航、无焦点环、无无障碍、无拖拽以外的手势；页签只有五个固定项。
- **没进安装包、没接托盘「设置」菜单**（要不要切换默认入口由用户定）。
- 玻璃目前是 **DWM 亚克力**（模糊+染色），还没做 memo 里记的「折射/色散」真·液态玻璃。

---

## 六·十四、方案列表读不到「系列文件夹」里的方案（2026-10-29 09:2x，用户报）

**症状**：设置里的方案列表看不到 `double_pinyin.*`，用户原话「不能识别系列文件夹了，识别不到 double pinyin」。

**根因**：`style/src/switcher.rs::list_schemas` 的「系列布局」分支还是 **10-19 的旧约定**——
找 `schema\<系列>\<叶>\<叶>.schema.yaml`、id 取叶名。而 2026-10-28 起布局改成
**「目录 = id 全部分段」**：`schema\double_pinyin\flypy\double_pinyin.flypy.schema.yaml`，
文件名是 `<系列>.<叶>.schema.yaml`。于是 `flypy/flypy.schema.yaml` 永远不存在 ⇒
系列分支全部 `continue` ⇒ **double_pinyin / pinyin / cangjie / wubi 四个系列在列表里整体消失**
（平铺的 `stroke`、`zhuyin` 还能显示）。**数据没丢，是列表扫不到**。

**引擎侧不受影响**：`librime-next` 的 `find_schema_file()` 是**递归**按 `<id>.schema.yaml`
找文件（`librime-next/src/engine/schema.rs:173`），所以打字/部署一直是好的，只有设置界面瞎了。

**修法**（`style/src/switcher.rs`）：系列分支先试新布局
`schema\<系列>\<叶>\<系列>.<叶>.schema.yaml`（id = `系列.叶`），再回退旧布局
`schema\<系列>\<叶>\<叶>.schema.yaml`（id = 叶名）；两者都没有才跳过。

**验证**

- 新增回归测试 `switcher::tests::lists_series_layout_with_dotted_ids`：临时目录里造
  `schema/double_pinyin/flypy/double_pinyin.flypy.schema.yaml` + 平铺 `stroke`，断言 id 是
  `double_pinyin.flypy`、`series = Some("double_pinyin")`、清单里勾过的 `selected = true`。
  `cargo test -p weasel-style --lib switcher` → **8 passed**。
- 实机：`Weasel-Next-Setting.exe /switcher` 截图 `C:\Code\_switcher.png`，
  列表**8 个方案全在**（含「小鹤双拼·简体」「自然码双拼·简体」「地球拼音」「86五笔」「仓颉五代」）。
- 已换进 live 安装：`C:\Program Files\Rime-Next\Weasel-Next-0.18.5\Weasel-Next-Setting.exe`
  sha256 `D96A5726827F…`，旧文件备份在 `C:\Code\_bak_pf_setting_20261029\`（提权脚本 `C:\Code\_deploy_setting_fix.ps1`）。

**顺带发现（没动，等用户定）**：`%APPDATA%\Rime-Next\schema\` 里还留着一批 10-25 的
**旧平铺方案目录**（`bopomofo`/`yuling*`/`yuhao*`/`tiger_sentence`…），它们会以未勾选项出现在列表里
（截图里的「注音」「注音·快打模式」就是）。删不删要用户点头——那是用户目录，不是出厂数据。


**同时重打了 x64 包**（让修一起进包，09:33）：`artifact\installer\Weasel-Next-0.18.5-x64-setup.exe`
**50,344,416 B**，sha256 `A52B0C796215330188FE8ECA89B7F189920B581A094082D2E72C6603FB5DEDEE`
（makensis 仍只剩 `UseUiAccess`、`uninstall_failed` 两个旧警告；包内 145 个文件）。

## 六·十五、用户报「只有 stroke 和 pinyin 可以用」的真因：菜单读错文件 + `select_schema` 从没装上（2026-10-29 09:3x～09:5x）

用户原话：「现在只有 stroke 和 pinyin 可以用」。上一棒已经排除数据和部署（`deploy` → `ok=true`，
8 个 schema 全编译过），但没定根因，还留了个**错方向**的怀疑（`scheme_parser_ref` 的平铺路径）。

**先证伪那个怀疑**：写 `librime-next/example/probe_all_schemas.rs`，对 8 个方案逐个
`Session::create_with_roots(user/build, id, [user, shared])` 喂键序并数候选 ——
**8 个全部 `ENGINE ok` 且都有候选**（`nihao`：pinyin.luna 34、pinyin.terra 2、zhuyin 106、
wubi.86 9、stroke 17、cangjie.5 5、双拼 22）。所以"方案文件/词库坏了"和"parser 取不到"
都不是原因：`rewrite_double_pinyin_preedit` 只覆盖**预编辑显示**，与能不能出候选无关。

**真因一：方案菜单读的是用户目录那份过期的 `default.yaml`，不是部署产物**

- `librime-next/src/engine/switcher.rs::load_default_config()` 原来只在 `data_roots`
  里找 `default.yaml`（用户目录优先），**从不读 `build/default.yaml`**。
- 而部署第 5 步（`lever/deployment_tasks.rs`）才刚刚把 `schema/schema.yaml` 的勾选清单写进
  `staging/default.yaml`，也就是 `<用户>/build/default.yaml` —— 那才是"用户勾了什么"。
  真机上这两份是**打架**的：`build/default.yaml` 8 项正确；`<用户>/default.yaml` 还是 10-27 的
  3 个旧 id（`luna_pinyin_simp` / `pinyin` / `wubi86`）。
- 于是 F4/`` ` `` 菜单只有 4 个方案（明月拼音 / 86五笔 / 朙月拼音·简化字 / 全拼），
  后两个 id **候选数实测为 0**（切过去能打字但永不出候选）—— 用户感知就是"只有两项能用"。
- 修法：`load_default_config(build_dir, data_roots)` **部署产物优先**，没部署过才回退数据根源文件
  （这正是 C++ `Switcher` 的语义：它读的是部署目录里的 default）。调用点
  `engine/runtime.rs::create_with_roots` 传 `build_dir`。
- 回归测试：`switcher::tests::deployed_default_yaml_wins_over_stale_user_source` +
  `falls_back_to_data_roots_before_first_deploy`（`cargo test -p librime-next --lib engine::switcher` → 5 passed）。
- 引擎级验证：`probe_menu` 打 F4 打印菜单 —— 修前 4 个方案，修后
  **明月拼音 / 倉頡五代 / 小鹤双拼·简体 / 自然码双拼·简体 / 地球拼音 / 五筆畫 / 86五笔 / 注音**（8 个 + 开关行）。

**真因二：`Engine/src/librime/api.rs` 漏装 `select_schema`，设置里选的"当前方案"从来没生效**

- API 表按 `data_size` 逐字段拷（`field!(...)` 宏），列表里有 `get_current_schema`、
  `select_candidate_on_current_page`、`change_page`，**独独漏了 `select_schema`** ⇒ 快照里是 `None`
  ⇒ `Session::select_schema()` 恒 `false` ⇒ 每个新会话都记一条
  `WARN schema.toml 里的方案当前不可用，保留 librime 的选择 schema=pinyin.luna`（真机日志 09:47 可查）。
  也就是说：设置里改"当前方案"、`schema.yaml` 的 `active`，**从来没落实到引擎会话**。
- 修法：`field!(select_schema);`（保持 optional，老 DLL 缺这项时仍安全回退）。
- 顺带修 `librime-next/src/c_api.rs::extract_first_schema_id`：原来只取清单**第一条**，
  而 `schema.yaml` 的清单按 id 排（第一条 `cangjie.5`）⇒ 新会话会先落在**仓颉**上，靠上面那个
  （当时坏掉的）`select_schema` 纠正。现在 **`active:` 优先**，没有才退回清单第一条。
  回归测试 `c_api::host_smoke::active_schema_wins_for_initial_session`。
- `cargo test -p Weasel-Next-Engine --bins` → 50 passed。

**已换进 live**（提权脚本 `C:\Code\_deploy_engine_dll.ps1`，日志 `C:\Code\_logs\deploy-engine-dll.log`，
旧文件备份在 `C:\Code\_bak_pf_engine_20261029\`）：

- `rime-next.dll` sha256 `DC078A7C25364377444F6ECE9D028F9C130D5ADBE2428E853A6E7A76EAA269B0`
- `Weasel-Next-Engine.exe` sha256 `B6EFC7EA869ABC2CA75BBB884E47B7F1ED4D849F6A32815B6E0845D846D47E5A`
- server 已重启，日志 `loaded schema preference from schema.yaml active="pinyin.luna" selected=8`。

**留给下一棒的**
1. 出厂 `Rime-Next/default.yaml` 的 `schema_list` 只有 2 项（`pinyin.luna` / `wubi.86`），
   而数据里带了 8 个方案；**全新用户目录**由它播种 → 首次只有 2 个勾选。要不要改成 8 项是产品决定。
2. `<用户>/default.yaml` 仍是 10-27 的旧文件（3 个死 id）。修一之后它已经不被菜单读了，
   但 `build/default.yaml` 里的 `switcher/hotkeys`、`page_size` 等来自部署时的合并，
   想彻底干净应当删掉用户目录这份（**用户数据，要点头**）。
3. `%APPDATA%\Rime-Next\schema\schema.toml` 是 10-26 之前的遗物（内容 `schemas=["stroke"]`、
   `active="pinyin.luna"`，正好能对上用户那句"只有 stroke 和 pinyin"）。全仓库已不读 TOML，
   但 `script/deploy-install.ps1:328` 仍按它做校验 —— 那份脚本已不参与打包，可考虑删文件/删脚本。

**六·十五·补（同一天 09:5x，装上 `select_schema` 之后暴露的两个隐患）**

`select_schema` 一旦真的能用，原来那些"反正它返回 false"的写法就变成了**每输入框都切一次方案**：

1. `Engine/src/engine.rs` 的 OpenInput：补上"`current_schema()` 已经等于 `active` 就跳过"，
   否则每次焦点进入输入框都会重建会话（丢组合、大表方案明显卡顿）。三条约束写进注释。
2. `librime-next/src/c_api.rs::rime_select_schema`：**成功才换**。原来无条件把会话替换成
   `Session::create_with_roots(...)`，方案加载失败时新会话 `engine = None` ⇒ 该会话从此
   "什么键都吃不下、没有候选"，用户看到的是**输入法突然哑掉**。现在 `fresh.engine.is_none()`
   直接返回 0，保留原会话。
3. 新增**对 live DLL 的 ABI 回归测试** `Engine/src/librime/mod.rs::live_dll_tests::
   live_dll_exposes_select_schema_and_can_switch`：直接 `Librime::load("C:\Program Files\
   Rime-Next\Weasel-Next-0.18.5", %APPDATA%\Rime-Next)`，断言
   ① API 表里 `select_schema` 不是 None（专防 `field!` 列表又漏项）；
   ② `select_schema("pinyin.luna")` 为真且 `current_schema` 跟着变；
   ③ 切完 `nihao` 仍有候选。实测输出：`候选数 5（首项 你好）`。没装 live 时自动跳过。
   跑法：`cargo test -p Weasel-Next-Engine --bins live_dll -- --nocapture`。

⚠️ **操作教训（别再犯）**：本轮为了换 DLL 连停两次 `Weasel-Next-Server/Engine/Renderer`，
停完再手工 `Start-Process` 拉起，**TIP 与 server 的连接没恢复** —— 用户报"打不了字"，
日志里 09:54 之后**没有任何客户端连接**（broker 只记了 `ignored internal … while idle`）。
正确姿势：要换引擎文件就**只停进程、不手工拉起**，让 TIP 在下次输入焦点时按需拉起新的；
或者换完直接让用户重启。别在用户正在用的时候手工 `Start-Process` server。

## 六·十六、安装器「自选程序目录 + 自选用户文件夹」出包（2026-10-29 10:2x～10:4x）

**代码（上一棒已改完，本棒只编/打）**

- 运行期：`common/src/process.rs` 的 `resolve_user_data_dir` 定优先级
  `WEASEL_NEXT_USER_DIR` > `<程序目录>\userdir.txt`（首行绝对路径）> `%APPDATA%\Rime-Next`；
  Setting 5 处、`Setting/src/stats.rs`、`Tip/src/stats.rs`、`theme/src/config.rs`（2 处）、
  `style/src/bin/shuangpin_schema.rs` 都改走它，不再各自拼 `APPDATA`。
- 安装器：`installer/weasel-next.nsi` 加 `MUI_PAGE_DIRECTORY` + 自定义 `UserDirPageCreate/Leave`
  （`nsDialogs::SelectFolderDialog`）；`.onInit` 删掉 `StrCpy $INSTDIR`（保 `/D=`）、解析 `/USERDIR=`；
  `CheckTargetDirectory` 改在 `SEC_MAIN` 第一行对**最终** `$INSTDIR` 判断；非默认用户文件夹才写
  `$INSTDIR\userdir.txt`，卸载宏 `Delete /REBOOTDIR` 它（用户数据不删）。
  两个包都是 **6 页**安装器（欢迎/许可/目录/用户文件夹/安装/结束）。
  既有警告仍只有 `UseUiAccess` 未知常量、`uninstall_failed` 标签未用。

**本棒重编 + 双包结果**

- `cargo build --release --workspace` 3m13s；`Weasel-Next-Setting.exe` 10:25（含 F4 名著文案）。
- 坑 #6 照做：`cp -f target/release/librime_next.dll target/release/rime-next.dll`（10:26）。
- x64：`artifact\installer\Weasel-Next-0.18.5-x64-setup.exe` **50,470,141 B**，
  sha256 `0eec4f0659ff19a3ff8ac0a8543c2e419bf9ce48d1b1bf2bedd26ef6b20a2af1`
  （内含 Setting 10:49、TIP 10:47、32 位 TIP 10:53、rime-next.dll 10:46）。
- ARM64：`Weasel-Next-0.18.5-arm64-setup.exe` **35,812,765 B**，
  sha256 `872ac48fca3a5caeda4c62207b36773e36883a508d1bb65d16b5c1a1a586fd68`
  （aarch64 Setting 10:52、arm64 TIP 10:50、arm64 rime-next 10:29）。

**出包前又修了三个「自选用户文件夹」的缺陷（10:4x）**

1. **默认用户文件夹算成 `C:\ProgramData\Rime-Next`**（严重）：`.onInit` 里
   `!insertmacro CheckPlatform` 会先执行 `SetShellVarContext all`，把 `$APPDATA` 从
   `C:\Users\X\AppData\Roaming` 变成 `C:\ProgramData`（**实测**：写了一个最小 NSIS 探针
   `C:\Code\_nsi_appdata_probe.nsi` 打印前后值确认）。修法：在 `CheckPlatform` **之前**
   `StrCpy $UserDirDefault "$APPDATA\Rime-Next"`。顺带：MUI 结束页文本引用 `$UserDir`
   必须把 `Var UserDir` 挪到 `MUI_PAGE_FINISH` 之前，否则 warning 6000、文本退化成字面量。
2. **开始菜单「用户文件夹」快捷方式指向 `$INSTDIR`**（改了位置也打不开对的地方）→ 改 `$UserDir`；
   结束页/安装日志也一并显示用户文件夹（默认位置也记一条日志）。
3. **TIP（DLL 形态）识别不到 `userdir.txt`**：`common/src/process.rs::executable_directory()`
   原来用 `std::env::current_exe()`，而 TIP 跑在宿主进程（记事本/explorer/WPS）里，
   拿到的是**宿主** exe 的目录 ⇒ 永远找不到安装目录的 `userdir.txt` ⇒ 用户自选位置在 TIP 侧
   静默回退 `%APPDATA%`（打字统计写错地方）。改成 `GetModuleHandleExW(FROM_ADDRESS)` 取
   **本代码所在模块**：链进 exe 是 exe、链进 DLL 是 DLL；`user_data_dir()`/`RuntimePaths` 全走它。
   验证：单元测试 `module_path_matches_test_executable`（5 passed）+ **真 DLL 实测**——
   临时探针 crate `C:\Code\_probe_userdir`（cdylib 调 `user_data_dir()`，宿主 exe 在别的目录）：
   DLL 旁边放 `userdir.txt`=`D:\自定义 用户目录` → 输出该路径；删掉 → `%APPDATA%\Rime-Next`。

⚠️ 编译时发现 `plum-next/src/fetch.rs`（10:31，untracked）**被外部改坏**（3 个 E0308：git2 API/
生命周期），`cargo build --workspace` 整体失败。**没碰它**，改用 `-p` 单包编译 + 10:06 的
`plum-next.exe` 打包。下次整体构建前先确认那个文件是否还在别人手里。

- 跑法：`pwsh -File script\build-installer.ps1`（x64，~90s）、`pwsh -File script\build-installer-arm64.ps1`
  （**别加 -SkipBuild**；会先编 aarch64、再编 x86 TIP、再顺带重打 x64、最后 `-DTARGET_ARM64` 组装）。

**⚠️ 打完后发现的现场变化（2026-10-29 10:3x）**

`C:\Program Files\Rime-Next\Weasel-Next-0.18.5\` **整个目录已不存在**，卸载登记项也没了；
本机现在装的是**旧版小狼毫 0.17.4**（`C:\Program Files\Rime\weasel-0.17.4\`，WeaselServer 10:15 起在跑），
`HKLM\...\CTF\TIP` 里只有 `{A3F4CDED-…}` 的「小狼毫」档案，我们那份 `{170462E1-…}` 只剩 Enable=0 的残留。
用户数据 `%APPDATA%\Rime-Next` 还在（`build`/`schema`/`userdb` 等）。
⇒ 本棒的包**没在 live 上验过**；上一棒部署的 live DLL（`4D33D149…`）已随卸载消失，备份仍在
`C:\Code\_bak_pf_engine_20261029\`。装不装、什么时候装要用户点头（会改 TIP 注册、动正在用的输入法）。

**「zhi」仍未定位**：`Tip/src/service/language_bar.rs` 的 `ModeButton::text()` 只可能返回
`中/英/密/!/…`，`GetText` 直接返回它；主题绘制也只画「中/英」；全仓库无 `zhi` 字面量。
下一步必须拿到用户重启后的截图（或确认是不是旧版小狼毫/系统输入指示器在显示）。

## 六·十七、第一次把「打字卡」这串修复编出来 + 出双包（2026-10-30 11:36～11:54）

> 本棒**只做编/打/记录**：源码是 10-29 12:2x 与 10-30 02:2x～03:1x 两段改的（工作区一直没提交）。
> 第 4 节是本棒唯一发现的「与检查点不符」，装包前必须先看那一节。

### 1. 编译

- `cargo build --release --workspace`：**5m56s，exit 0**。此前卡住的 `weasel-common` E0308（`accept` 的 `Err(error)` 要 `.into()`）已修好，这次一次过。
- 输出只有 warning（`Weasel-Next-Setting` 51 条、`Weasel-Next-Engine` 12 条、`librime-next` 若干 unsafe-in-unsafe-fn），**无 error**。
- `cp -f target/release/librime_next.dll target/release/rime-next.dll`（11:42，8,600,064 B）。

### 2. 双包

| 包 | 时间 | 字节 | sha256 |
|---|---|---|---|
| `artifact\installer\Weasel-Next-0.18.5-x64-setup.exe` | 11:52 | 55,574,968 | `110b8746a4f8dbfee48d9e276ecb2140e68602df2bfe6b564d98ca633f6ba57b` |
| `artifact\installer\Weasel-Next-0.18.5-arm64-setup.exe` | 11:54 | 39,051,830 | `205eb9b25da4cb165c3b6321df30b4e1e65a71dec4adebe7bfd2f9dd8c08c079` |

payload 抽查（`7z l`）：
- x64：`Weasel-Next-Engine.exe` 11:38、`Weasel-Next.dll`（TIP）11:38、`Weasel-Next-32.dll` 11:50、`rime-next.dll` 11:42、`092.schema.yaml` 43,507 B。
- arm64：Engine / TIP / `rime-next.dll` 全 11:47（aarch64），`Weasel-Next-32.dll` 11:50（x86，两包共用）。

⚠️ **x64 包被打过两次**：11:45 单独跑 `build-installer.ps1` 得 `44cde6ae…`；11:52 `build-installer-arm64.ps1` 内部又重编 x86 TIP + 重打了一次 x64。**以第二次为准，`44cde6ae…` 作废。**

### 3. 验证

- `cargo test --release -p weasel-common --lib process` → **5 passed**
- `cargo test --release -p librime-next --lib schema_list` → **17 passed**
- `cargo test --release -p Weasel-Next-Engine --bins` → **51 passed**（含 `live_dll`：对**已装**的 `rime-next.dll` 实测 `select_schema` 能切）
- 安装包 payload：`Rime-Next\` 整棵树随包（安装数据 214.5 MB → 压缩后 55.5 MB）。

### 4. ⚠️ 与检查点不一致：空闲自退已经不存在了（装包前必读）

`checkpoint-53e92703.md` 的坑 #5 / B5 写「`IDLE_TIMEOUT_MS` 已改 30 分钟，仍是自退」。**代码里已经没有这个常量**（10-30 02:53 的改动）：

- `Engine/src/main.rs`：删掉 `IDLE_TIMEOUT_MS` 与空闲看门狗的自退分支；改成**连接池**——进程与命名管道**常驻**，每 30 s 发一次 `Work::ReclaimIdle`，
  由 `Engine/src/engine.rs::reclaim_idle_sessions()` 回收「超过 `SESSION_IDLE_RECLAIM`(=300 s) 没打字」的**原生会话**（只放会话/词库句柄，不重启进程）。
- `Engine/src/engine.rs`：新增 `ClientSession` + `clients: HashMap<u64, ClientSession>`（一连接一会话 + 路由），`Work::ReclaimIdle` 分发。
- `Server/src/service_supervisor.rs`：`ENGINE_IDLE_EXIT_CODE`(=3) 的「空闲自退 → idle 态」分支**还在**，但引擎已不会返回它 ⇒ 现在是**死路径**；
  文案里的 `ENGINE_IDLE_MINUTES`(=30) 也会误导排查。
- 这跟用户 10-27 定的「消灭常驻进程」**相反**，换来的是「停一会儿再打第一个字不再丢」。⇒ **装不装、要不要保留自退，需用户拍板**。

同包里的其余修复：broker `accept` 退避重试（50→1000 ms）+ 失败走 `diagnostic_warn`、语法模型 `shared_dbs` 进程内缓存、
TIP 唤醒重试（`WAKE_RETRY_INTERVAL` 120 ms × `WAKE_RETRIES` 8 + `WAKE_COOLDOWN` 1500 ms）、
热路径日志默认关（`WEASEL_THEME_LOG` / `WEASEL_RENDERER_LOG` / `WEASEL_ENGINE_VERBOSE`）+ `theme.log` 8 MB 轮转、`Tip/src/key_latency.rs` 每键耗时插桩。

### 5. 还没做的

- **没装到 live**（`C:\Program Files\Rime-Next\Weasel-Next-0.18.5`）：装包会改 TIP 注册、动正在用的输入法，要用户点头。
- 实机验收（首字丢 / 打字卡 / 切方案）没做；`theme.log` 那个 72 MB 旧文件还在（改后不再增长）。
- 092 的 `dependencies` 改指 `pinyin.luna`（`C:\Code\_dep092.py`）仍未执行。

## 六·十八、设置页「常用字词云 / 下载页」目视验收 + 两处布局修复（2026-10-30 20:1x～20:3x）

上一棒（`checkpoint-da9ffca2.md`）留下的「下一步第一件事」是看下载页截图，`C:\Code\_download_page.png` 实际拍的是**主页**（点页签没生效）。
本棒重拍 + 修好两处布局，并把「统计页内容宽度 ≈1960px 不随窗口变」这条**结案**（结论：不可复现，别照它改）。

### 1. 统计页宽度 1960 —— 不是 bug，是截图脚本的假象（重要，别再改）

- 实测（本棒，离屏 `PrintWindow(hwnd, hdc, 2)`，窗口 `SetWindowPos` 到 0,0）：
  | 窗口 | 客户区 | 12 张卡每张宽 | 判定 |
  |---|---|---|---|
  | 1600×900 | 1600×891 | 506 px | 正常（`(1600−64)/3 ≈ 506`，含滚动条） |
  | 1600×2000 | 1582×1991 | 506 px | 正常 |
  | 1600×3000 | 1582×2991 | 506 px | 正常 |
  | 2400 宽 → 缩回 1600 | — | 缩回后仍是 506 px | 正常（不会卡在旧宽度） |
- 上一棒那张 `_download_page.png`（1600×1000）里卡宽 **633 px** ⇒ 反推页面宽 ≈1958。用**物理客户区宽 1582 当逻辑宽**才会得到 633（`(1582−64)/3 = 506` 逻辑 ×1.25 DPI = 632.5 物理）。⇒ 是**外部 `SetWindowPos` 改窗口尺寸时的一过性 DPI 比例错位**（把物理像素当成逻辑像素算了一帧），窗口稳定后自愈；用户自己拖窗口（winit 走 `WM_DPICHANGED`）碰不到。
- ⇒ **不要**按上一棒检查点写的「改成 `horizontal-stretch: 1`」去修：`checkpoint-20261026-setting-ui.md` 实测过 —— Slint 的 stretch 只分**剩余**空间，卡片首选宽度来自文字，文字长的卡更宽，四列/三列会不齐。现在的 `width: (root.width - 64px) / 3` 是对的。

### 2. 下载页两处布局（已修、已截图为证）

- **分类页签吃满宽度**：`DownloadPage` 里原来用 `HorizontalBox` + 裸 `Rectangle`，两个页签各吃一半 ⇒ 「全部」是一条 760px 的蓝条、「方案」飘在右半边。改成 `HorizontalLayout { alignment: start }`，每个页签内套一层带 padding 的布局按文字撑开。
- **列表区固定 380px**：只有一个仓库时下面一大片白。改成 `vertical-stretch: 1; min-height: 240px`。
  ⚠️ 配套坑：`DownloadPage` 根布局原来写着 `alignment: start`，**只要写了它，`vertical-stretch` 就不生效**（实测列表停在 min-height 240、下面全空）。把 `alignment: start` 删掉、需要不拉伸的行显式 `vertical-stretch: 0`，列表才吃满。
- 截图：`C:\Code\_shots\final_dl_900b.png`（1600×900，列表贴到窗口底部）、`C:\Code\_shots\_badge_update.png`、`_badge_installed.png`。

### 3.「常用字」词云只收汉字（行为变更）

- 现象（修前截图 `_shots/static_before`…）：词云第一位是「，」356 次、「的」才 148；第二/三行混进 `e a i u m |` 这些拉丁字母和符号。
- 根因：`Snapshot.words` 直接来自 `char_counts`，TIP 按**字符**统计，标点、字母一样进。
- 修法：`Setting/src/stats.rs::snapshot()` 里只留汉字（U+3400–4DBF / 4E00–9FFF / F900–FAFF，`is_han()`）；**一个汉字都没有时退回原样**，免得纯英文输入时整张卡片消失。
- 新单测 2 条：`words_drop_punctuation_and_latin`、`words_keep_latin_when_no_han_at_all`。`cargo test --release -p Weasel-Next-Setting --lib` → **48 passed**。
- 修后截图（`_shots/final_stats_full.png` 2600 高）：`是 148 / 的 142 / 不 103`，三行 3/7/10 全是汉字。

### 4. 徽标两态目视验证（没碰用户真数据）

- 用 `WEASEL_NEXT_USER_DIR=C:\Code\_shots\fakeuser` 起一个假用户目录，写 `.plum-next\installed.json`：
  - `pushed_at: 2000-01-01` ⇒ 远端更新 ⇒ 显示橙色 **「可更新」** + 按钮变 **「更新」** ✓（`_shots/badge_update.png`）
  - `pushed_at: 2099-01-01` ⇒ 显示绿色 **「已安装」** + 按钮仍是「安装」✓（`_shots/badge_installed.png`）
- 验证完删掉假目录，**没有往 `%APPDATA%\Rime-Next` 写任何东西**。

### 5. 还没做（装包前必读）

- **实机安装仍未跑通**（未决 #2）：真装一次会往 `%APPDATA%\Rime-Next` 写 `schema.092` 并触发 broker 部署，会动用户正在用的数据 ⇒ 等点头。
- **双包（11:52/11:54）里没有这些 UI 改动**：包里的 `Weasel-Next-Setting.exe` 是 11:38–11:47 的，词云/下载页都是 20:0x 才编的。要让「装的包」带上这套 UI，得重跑：build → `cp -f target/release/librime_next.dll target/release/rime-next.dll` → `build-installer.ps1` → `build-installer-arm64.ps1`（arm64 脚本内部会重打 x64，hash 以第二次为准），并把新 hash 补进下一节。
- 单测/编译：`cargo build --release -p Weasel-Next-Setting` 3m24s exit 0；Setting lib 48 passed。**没有重跑全 workspace 构建**（上次 5m56s 那次之后引擎侧没再动过）。

## 六·十九、重打双包（把「词云 + 下载页」打进包里）（2026-10-30 20:4x～20:5x）

上一节（六·十八）改的是设置页 UI，但 11:52/11:54 那两个包里的 `Weasel-Next-Setting.exe` 是 11:38–11:47 编的，**不含**词云与下载页修复。用户点头后重打了一轮。

### 1. 步骤与耗时

| 步骤 | 耗时 | 结果 |
|---|---|---|
| `cargo build --release --workspace` | **2m48s** | exit 0（大部分命中缓存；上次全量 5m56s） |
| `Copy-Item target\release\librime_next.dll target\release\rime-next.dll -Force` | — | 20:43:05 |
| `pwsh -File script\build-installer.ps1`（x64） | 2m17s | 55,577,782 B（**中间产物，作废**） |
| `pwsh -File script\build-installer-arm64.ps1` | **6m39s** | 内部又重打了 x64 + 出 arm64 |

### 2. 双包（以 20:55～20:56 这次为准）

| 包 | 时间 | 字节 | sha256 |
|---|---|---|---|
| `artifact\installer\Weasel-Next-0.18.5-x64-setup.exe` | 20:55:18 | 55,591,029 | `aa98bab30b1b7cba0036309772abcffe2345c69237b2917eadb26630c8e68ec8` |
| `artifact\installer\Weasel-Next-0.18.5-arm64-setup.exe` | 20:56:41 | 39,067,883 | `fe15e513ad959681f9fd84d09185223549c8c250f1551dd63c43cb2d57a13b8e` |

上一轮的 `110b8746…` / `205eb9b2…`（11:52 / 11:54）**作废**（里面没有这套 UI）。

payload 抽查（`7z l`）：

| 文件 | x64 包 | arm64 包 |
|---|---|---|
| `Weasel-Next-Setting.exe` | 20:45:08 · 58,110,464 B | 20:53:00 · 32,247,808 B |
| `Weasel-Next.dll`（TIP） | 20:42:48 · 7,973,888 B | 20:50:26 · 2,225,664 B |
| `rime-next.dll` | 20:43:04 · 8,600,064 B | 11:47:08 · 5,797,376 B（aarch64，本次没改 librime-next） |
| `Weasel-Next-Engine.exe` | 11:38:44 | 11:47:30（本次没改引擎） |
| `092.schema.yaml` | 10-29 11:48 · 43,507 B | 同 |

### 3. ⚠️ 打包时的坑（新踩，记下来）

- **在 pwsh 里 `cp -f` 是错的**：`cp` 是 `Copy-Item` 的别名，`-f` 会报 `Parameter cannot be processed because the parameter name 'f' is ambiguous`，而外层脚本若没设 `ErrorActionPreference='Stop'`，**它继续往下跑**——第一个 x64 包因此打进了 **11:42 的旧 `rime-next.dll`**（55,577,782 B，已作废重打）。要用 `Copy-Item ... -Force`，并且**核对 `rime-next.dll` 的 mtime/size 再打包**。
- arm64 脚本确实会重打一次 x64（这次 x64 的 mtime 20:55:18 落在 arm64 那轮里），**x64 的 hash 以第二次为准**。
- 两个包都没有警告级问题，NSIS 只报 `UseUiAccess` 未知宏 + `uninstall_failed` 标签未用（历史就有）。

### 4. 还没做的

- **仍未装 live**：装包会改 TIP 注册、动正在用的输入法 ⇒ 等用户点头。装完要验收：首字丢不丢、设置页词云/下载页是否是新样子、`%APPDATA%\Rime-Next` 数据是否还在。
- 下载页「安装（真装 schema.092 → 自动部署）」仍未实机验证。

## 六·二十、下载页进度条重做 + 第二次重打双包（2026-10-30 21:0x～21:3x）

用户一句「下载进度条真难看，都不是从左面开始的，中间突然多一块」，查出**两个**真问题：

1. **百分比那段是 Slint 源码被当成文字印出来了**：`text: "{round(root.download-frac * 100)}%"` —— Slint 的字符串插值必须写 `\{…}`（带反斜杠），少了反斜杠它**不做插值**，直接把 `{round(root.do` 这截源码画到界面上。
2. **进度条本身是"一整行染成一块蓝"**：旧写法把 60px 高的整行涂成实心蓝（`width = frac × 整行宽`、方角、无轨道），看着像凭空冒出一截色块，而不是从左往右长；百分比还用 `x: parent.width - 14px`，文本溢出卡片右边。

改法（`Setting/ui/download.slint` 的 `RepoCard` 安装态）：
- 一行文字：名称（左，`overflow: elide`）+ 百分比（右，定宽 88px、右对齐、蓝色 13px 粗体）。
- 下面一条 **8px 高的圆角轨道**（`x:14px`、`width: parent.width-28px`、`#D8E0EC`），填充从**轨道左端**开始长：`width: max(parent.width * frac, 8px)`、两端 4px 圆角 —— 0% 时也留 8px 起点，永远不反向。
- 验证方法：临时把 `is-installing` 强制成 `true`、`download-frac` 钉 0.34，截图确认后再还原（截图 `C:\Code\_shots\bar_fixed_crop.png`：`schema.092 … 34%` + 从左侧生长的圆角条）。

### 第二版双包（21:31 / 21:33，取代 20:55 那对）

| 包 | 时间 | 字节 | sha256 |
|---|---|---|---|
| x64 | 21:31 | 55,601,919 | `b4ea35b89b1ead2e530cf21107224b166516d341d6c82af3283bfcdd8af2c967` |
| arm64 | 21:33 | 39,070,963 | `72d2d69de695f95767fea42999fc0bb9878a985d45540e1e2966f7dfffe58f27` |

payload 抽查（`7z l`）：`Weasel-Next-Setting.exe` **21:22:32**（x64，58,118,656 B）/ **21:28:38**（arm64，32,245,760 B）、TIP 20:42:48、`weasel_theme_official.dll` 20:42:40（玻璃化）、`rime-next.dll` 21:22:34、`key_bindings.yaml` 18:40:44（逗号/句号翻页 `when: has_menu`）。

⇒ **另一条线那套"热修 dll 要管理员手动替换"其实不必做**：那份 `key_bindings.yaml` 逗号修复、TIP、theme 玻璃化都已经在这两个包里（源码就在同一个工作区），直接跑安装器即可，不用碰 Program Files。

⚠️ 打包脚本的耗时：workspace build 3m25s + x64 2m45s + arm64 7m45s ≈ 14 分钟。**UI 还会改的话，先别急着再打**——每改一次 UI 都得重走一遍。

## 六·二十一、下载页进度条改成「就地替换安装键」（2026-10-30 21:5x～22:2x）

用户第二次反馈：「不是，为什么现在百分比没有在安装键的位置，为什么进度条占了安装键的位置」。

上一版（六·二十）把安装态做成**整行另起一套布局**：名称在左、百分比在右上、进度条铺满整行——于是百分比离开了按钮的位置，进度条横穿到按钮那一列下面。

改法（`Setting/ui/download.slint` 的 `RepoCard`）——**不再有两套布局，只有一套**：

| 位置 | 平时 | 安装中 |
|---|---|---|
| 左 | 头像 | 头像（不动） |
| 中（吃满宽度） | 名称 + 说明 | 名称 + **6px 细进度条**（说明让位，避免换行把行撑高抖动） |
| 右 | 「安装 / 更新」按钮 | 同尺寸 **76×30 的「34%」胶囊**，就在按钮那一格 |

- 进度条只长在文字那一列里（起点与名称文字对齐、终点在按钮列之前），**不跨进按钮格**。
- 两处仍是硬约束：插值写 `\{…}`（不然印源码）、填充 `max(parent.width * frac, 6px)` 且 `x: 0`（永远从轨道左端长）。
- 验证：临时 `is-installing: true` + `download-frac: 0.34` 截图 → `_shots\bar_v2_crop.png`（`schema.092` + 名字下面一条 34% 的细条 + 右边 34% 胶囊），确认后还原再编一次；平时的行也复查过（`_shots\dl_normal_v2_crop.png`，按钮还在原位）。

### 第三版双包（见下一行 hashes）

第三次重打（workspace + x64 + arm64 ≈14 分钟）。教训：**UI 没定稿前每改一个字都要重打 14 分钟**，以后先攒、再打。

| 包 | 时间 | 字节 | sha256 |
|---|---|---|---|
| x64 | 21:53 | 55,599,871 | `4f0ace49e8f6d6c62c6ce73f451e6b389203c479b5bd72a30ea08be6b84e0467` |
| arm64 | 21:55 | 39,088,427 | `4e389c6998e2e6a16e69a8740a05d2a6cb795b5cbc2226b92952f61d1198d27e` |

payload 抽查：`Weasel-Next-Setting.exe` **21:44:26**（x64，58,173,952 B）/ **21:50:16**（arm64，32,336,896 B）；`key_bindings.yaml` 18:40:44；`weasel_theme_official.dll` 20:42:40。更早的 `b4ea35b8…/72d2d69d…`（21:31/21:33）与 `aa98bab3…/fe15e513…`（20:55）**都作废**。

## 六·二十二、进度条定形：**整个条就是进度条**（2026-10-30 22:0x）

用户第三次纠正：「我不是要整个条作为进度条吗，不是要你现在的进度条」。
⇒ 六·二十一那版「文字下面一条 6px 细轨道」是理解错了：他要的就是**整行当进度条**。

定形（`Setting/ui/download.slint` 的 `RepoCard`，安装态）：

```
if root.is-installing : Rectangle {
    width: parent.width; height: parent.height;
    background: #EDF1F7;          // 轨道 = 整行
    border-radius: 8px;
    clip: true;                    // 填充跟着卡片圆角裁
    Rectangle {                     // 整条填充
        x: 0px; y: 0px; height: parent.height;
        width: max(parent.width * root.download-frac, 0px);
        background: #BFD6F0;       // 浅蓝：名字/百分比压在上面还看得清
    }
    HorizontalBox { 头像 + 名称(吃满) + 「34%」胶囊(76×30，就在安装键那一格，白 60% 底) }
}
```

- 填充 `x` 恒为 0 ⇒ **永远从最左边开始长**（这正是第一轮说「不是从左面开始」的根子）。
- 颜色用浅蓝而不是最早那种 `#185FA5` 深蓝：深蓝整行会把压在上面的名字吞掉（白字又会在 0% 时看不见）。
  想要更"重"的观感可以改深蓝填充 + 白字，但名字/百分比得跟着反色，得用户点头。
- 截图：`C:\Code\_shots\bar_v3_crop.png`（34%：左边 34% 浅蓝 + 右边浅灰轨道 + 右侧 34% 胶囊）。

⚠️ **双包没重打**：`artifact\installer\` 里还是 21:53/21:55 那版（细轨道形态）。等这个形态被认可再打第四版（≈14 分钟），免得再来回烧时间。

## 六·二十三、真正的垂直居中（推翻六·二十二那条「实测已居中」）（2026-10-30 22:4x～23:0x）

用户看图后只回一句：**「对，但是你没有全部垂直居中啊，这不是都靠上」**。
回头**用像素量**（PIL 逐行扫颜色，物理像素，DPI 125%），六·二十二 的「实测已居中：行内容 285..339，按钮 293..331」**是错的** —— 那是拿「头像顶」当「行顶」量出来的自我印证（又一次「检查器必须被检查」）。

真实几何（同一窗口 1600×900 抓图，逻辑行高 60px = 物理 75px）：

| 位置 | 行范围（物理） | 中线 | 实测 | 结论 |
|---|---|---|---|---|
| 列表卡片上边框 | 265 | — | — | 行 = 边框 +10 … +85 ⇒ **275..349** |
| 头像（安装中，旧） | 285..321 | 312 | 303 | **高 9px** |
| 34% 胶囊（安装中，旧） | 290..317 | 312 | 303.5 | **高 8.5px** |
| 头像（平常） | 294..330 | 312 | 312 | 居中 ✓（所以只有安装态明显「靠上」） |
| 名字（平常） | 294..307 | 312 | 300.5 | **高 11.5px** |

两个**不同**的根因：

1. **安装中分支的内容不填满行**：该分支的 `Rectangle` 里有**两个孩子**（整条填充 + 内容 `HorizontalBox`），Slint「唯一 layout 子元素自动填满父级」的规则只在单孩子时生效 ⇒ `HorizontalBox` 保持自然高度 46px 贴顶，里面的居中容器再准也没用。
   修法：给这一层 `HorizontalBox` 显式 `y/width/height`（`y:0; width:parent.width; height:parent.height`）。
2. **平常分支的名字被「空说明行」顶高**：名字和说明装在同一个 `VerticalLayout{alignment:center}` 里，`schema.092` 没有说明但 `Text` 仍占一行 ⇒ 整块居中、名字就高出半行。
   修法：`if root.repo.description != "" : Text { … }`，空说明不占位。
   顺带把名字、徽标也套进 `VerticalLayout{alignment:center; vertical-stretch:1}` —— `Text` 的 `vertical-alignment` 在横向布局里实测靠不住。

**修后实测（两张图都量了，偏差 ≤1 物理像素 = 0.8 逻辑 px）**：

| 图 | 头像 | 名字/百分比 |
|---|---|---|
| 平常 `_shots/final_dl.png` | 312.0（行中线 312.5） | 312.5 |
| 安装中 `_shots/probe_dl.png` | 312.0（行中线 312.0） | 313.0 |

验证手法（可复用，别再靠眼睛）：`_shots/verify_row.py <png> [row_top]`、`_shots/verify_installing.py <png>`（从 `#BFD6F0` 填充反推行范围）、`_shots/annotate_row.py <png> <out> <top> <bot>`（画中线放大 2 倍）。临时强制安装态的办法照旧：把 `is-installing` 改 `true`、`download-frac` 改 `0.34`，编一版截图后**从 `_shots/download.slint.pre-v5` 还原**。

编译：两次 `cargo build --release -p Weasel-Next-Setting` 各 3m05s / 3m11s（exit 0），最终 exe **22:57**，57,874,944 B。

## 六·二十四、安装按钮改成自绘 + 补上悬停反馈（2026-10-30 23:0x）

用户：「安装按钮的 hover 效果呢」。

查下来**不是没写 hover，是写了看不见**：这个按钮用的是 `std-widgets` 的 `Button`（fluent 皮肤），它的 hover 态只是把底色从 `FluentPalette.control-background` `#FFFFFFB3` 换到 `control-secondary` `#F9F9F980` —— 压在纯白卡片上算出来只差 **3/255**（255 → 252），肉眼等于没有。`main.slint:264` 早就记过同类教训（「std Button 在白底上白底白字…自绘就得自己给悬停/按下反馈」），下载页这个按钮当时漏了。

改法（`Setting/ui/download.slint` 的 `RepoCard` 平时态）：
- 自绘 `Rectangle` 48×30、圆角 6px，配色跟顶栏「保存并部署 / 关闭」同一套：
  平时 白底 + `#c9d2de` 描边；**悬停 `#f1f5f9`**；**按下 `#d7dfe9` + 描边 `#8fa0b4`**；安装中（`is-busy`）底 `#f7f9fb` + 描边 `#e2e8ef` + 字 `#c0c4cc`。
- `TouchArea { mouse-cursor: pointer }` —— 之前连手型光标都没有。
- 顺带：分类页签「全部 / 方案」之前**既没 hover 也没手型**（别的页的页签在 `main.slint` / `color-schemes.slint` 里都有），一并补上：悬停 `#eef2f6` + `pointer`。

实测（窗口 1600×900、DPI 125%，物理像素）：按钮内部 悬停前后 `(255,255,255)` → `(241,245,249)`，差异只落在按钮范围 (1492,295)-(1550,330)；「方案」页签 (94,220)-(157,253) 变浅灰。按钮 y 294..330、中线 312 = 行中线，**居中没被这次改动破坏**。

⚠️ 验 hover 的坑：**PostMessage 的 `WM_MOUSEMOVE` 只在窗口是活动窗口时才生效**。先 `PostMessage` 点击切页签 ≠ 激活窗口；要先真点一下（`click3.ps1`）把窗口激活，再 `PostMessage` 移动，截图才有 hover 态（`_shots/post_click.ps1`）。

还没换的：网络错误提示里那个「✕」仍是 `std Button`（只在拉取失败时出现，同样看不见 hover）。

## 六·二十五、方案选单按键：TIP 二次投递修好（代码层）+ 一次按键被投递两次的实证（2026-10-31 11:0x～11:2x）

用户报 ⓪：`Ctrl+\`` 选单里「空格不确认、按数字选完数字还会上屏」。

### 1. 改了什么（只改 `Tip/src/service/key_event.rs`，未打包）

`TestedKey` 原来用**整份 `ContextToken`**（含 `generation`）配对 `OnTestKey*` 与随后的 `OnKey*`。
测试回调被引擎接受后，上下文可能因组合取消 / 切方案等失效操作推高 generation，配对失败 ⇒
**同一个物理键被投递两次**。改成：

- 缓存里加 `keycode: u32`（`OnKey*` 的 wParam = 虚拟键码），`reuse`/`processed` 只比
  `context_id + connection_epoch + key_up + keycode`，**不比 generation**；
- 单测重写：新增 `test_and_actual_key_pair_across_generation_change`（测试回调 gen=1 → 实际回调
  gen=2 仍复用）与 `different_keycode_does_not_reuse`；原「generation 变化丢弃缓存」改用例删掉。
- `cargo test --release -p weasel-tip --lib` → **74 passed, 0 failed**。
- 编译产物 `target\release\weasel_tip.dll`（5,299,200 B，11:03）。

### 2. 实测：一次按键确实被投递两次（旧 TIP，Notepad）

新加工具 `_shots/switcher_probe2.ps1`（强制前台 → `Ctrl+\`` → 抓图 → 按键 → 再抓图）。
旧 TIP（live 10:12 那份）实测（截图 `_shots/switcher_down-open.png` / `switcher_down-after-key.png`）：

- 开菜单时高亮在第 1 项；**按一次 `↓`，高亮跳到第 3 项** ⇒ 同一个键被引擎处理了两次。
  这是「TIP 二次投递」的直接证据（引擎 `move_selection(engine, 1)` 只移一格）。
- 数字键正常：按 `2` 切到「倉頡五代」（`user.yaml` `previously_selected_schema` 跟着变），文档里**没有**落 `2`。
- **空格 / 回车都不确认**（菜单保持打开，图 `switcher_space-after-key.png`、`switcher_enter-after-key.png`）；
  `ESC` 能关掉（图 `switcher_esc-after-key.png`）。所以「空格不确认」能独立复现，但**根因还没定位**：
  引擎源码 `switcher.rs` ⑤ 明明把 `XK_SPACE | XK_RETURN` 映射到 `confirm()`，而数字/ESC 走同一条链都正常。
  下一步要么在 TIP 里打日志确认空格有没有送进引擎，要么在 live DLL 上跑 `cargo test ... live_dll` 复现。

### 3. 部署被卡住：`Program Files` 下的 TIP DLL 换不了

`C:\Program Files\Rime-Next\Weasel-Next-0.18.5\Weasel-Next.dll` **Copy-Item -Force 和 Rename-Item 都报
`Access to the path ... is denied`**（ACL 要管理员；不是共享冲突，改名也不行）。
⇒ 新 TIP 只能走安装包部署（安装器是静默提权的）。备份：`C:\Code\_bak_tip_live_20261031_1115.dll`（旧 live，7,973,888 B）。
测试时把用户方案切到了 `cangjie.5`，**已用菜单切回 `double_pinyin.flypy`**（`user.yaml` 已确认）。

### 4. 设置界面玻璃：别再往 Slint 里钻（重要更正）

本棒核对更早结论：`六·十三` + `checkpoint-1a4c5462.md` 坑 2 已经定过 —— **Slint 设置窗做不出真玻璃**
（10-28 实测：半透明根底在深色主题下被 DWM 合成成黑/灰，用户当场否掉，已回退）；
真玻璃走 **Direct2D 外壳**，v1 早在 10-29 就验证过（棋盘格垫背、逐像素 alpha + DWM 模糊）。
本棒复验：`target\release\Weasel-Next-Setting-D2D.exe` 11:2x 启动抓图 `_shots/d2d_shell_glass.png`，
窗口顶部透出并模糊了后面的浏览器标签栏 ⇒ **玻璃是现成的，缺的是「把五页从 Slint 迁到 D2D」**。
⇒ 上一棒「先确认实际生效的是哪个渲染器」（`Setting/Cargo.toml` 默认 femtovg、`renderer_name("skia")` 回退）
是条死路：就算换成 skia 能透明，也要面对 10-28 那个「黑/灰」的老问题。要做玻璃就做页面迁移，不要再调 Slint 背景色。

### 5. 新增工具

`_shots/tip_menu_test2.ps1`（自建 WinForms 文本框，直接读 TextBox —— 注意：CUAS 宿主里 `Ctrl+\`` 打不开选单，
得用真 TSF 应用）、`_shots/tip_menu_test3.ps1`（Notepad + 剪贴板读文）、`_shots/switcher_probe2.ps1`、
`_shots/switcher_space_leak.ps1`、`_shots/d2d_glass_shot.ps1`。
焦点坑：`AppActivate`/`SetForegroundWindow` 都会失败，必须 `AttachThreadInput` 到当前前台线程再 SetForegroundWindow，
并且**按键前核对 `GetForegroundWindow() == 目标 HWND`**（第一版没核对，把键打进了 Firefox）。

## 六·二十六、整句/分词质量：线上 dll 是真坏的，工作区这套已修好一半（2026-10-31 11:1x～11:3x）

用户原话：「现在打字的识别和分词很差」。本棒建了**对照工具**，把「线上 dll」与「工作区源码编出来的库」
用同一批输入喂一遍，结论很明确：**线上 rime-next.dll（10:15 那份）的整句排序坏了；工作区这套代码已修好其中 3/5**。

### 对照工具（可复用）

- `librime-next/example/probe_candidates.rs`（已登记到 `librime-next/Cargo.toml` 的 `[[example]]`）：
  直接跑**工作区源码**，打印每段切分、候选、空格提交。
  `cargo run --release --example probe_candidates -- "$APPDATA/Rime-Next/build" "$APPDATA/Rime-Next" "C:/Program Files/Rime-Next/Weasel-Next-0.18.5/data" pinyin.luna nihao jintiantianqihenhao`
- `Engine/src/librime/mod.rs` 新增 `#[ignore]` 探针 `live_dll_probe_candidates`：跑**线上 dll**，同一批输入。
  `RIME_PROBE_SCHEMA=pinyin.luna RIME_PROBE_INPUTS="nihao,jintiantianqihenhao" cargo test --release -p Weasel-Next-Engine --bins live_dll_probe_candidates -- --ignored --nocapture`
  （要 `LIBCLANG_PATH=C:\Program Files\llvm-mingw\bin` 与同目录 PATH）

### 实测对照（pinyin.luna，取「空格提交」= 第一候选）

| 输入 | 期望 | 线上 dll | 工作区 |
|---|---|---|---|
| nihao | 你好 | 你好 | 你好 |
| nihaoshijie | 你好世界 | 你好世界 | 你好世界 |
| wozaixuexizhongwen | 我在学习中文 | **我在学校啊**（截断） | **我在学习中文** ✅ |
| jintiantianqihenhao | 今天天气很好 | **今天天气**（截断） | **今天天气很好** ✅ |
| shurufabunengzheyang | 输入法不能这样 | **输入法软件yang**（半截+原文漏出） | 输入法不能遮阳（差在选词） |
| womenmingtianqugongyuan | 我们明天去公园 | 我们明天去公元 | 我们明天去公元（两边都没选 公园） |

⇒ 线上那版的毛病就是用户说的「整句不整 / 分词碎」：**只覆盖前半段的候选排在第一**，
空格把半句上屏，剩下的原文（`yang`、`i`）留在输入区。

### 本棒修的一个必崩 bug（工作区）

`librime-next/src/engine/menu.rs` 的 `Menu::add_filter`：
`if let Some(result) = self.result.borrow_mut().take()` 里那个 `RefMut` 临时值会活到整个 if-let 结束，
函数内部第 3 行再 `borrow_mut` 就 panic `RefCell already borrowed`。
**工作区这套代码一 compose 就崩**（`double_pinyin.flypy` 打 `nihao` 直接 panic），所以从来没能编出来用。
已改成先 `let taken = { self.result.borrow_mut().take() };` 再处理。修完 74→（TIP 侧 74 条仍绿）与 `probe_candidates` 全部能跑。

⚠️ 状态更正：`librime-next/` 是**独立 git 仓库**，HEAD=`e41e937`（急切菜单，dll 由它编），
工作树有 **25 个未提交文件**（懒化菜单 v3 半成品）。上面「工作区」一列 = 这份半成品 + 本棒的 menu 修复。

### 剩余差距（按性价比）

1. **`公园` 输给 `公元`**：词表里 `公园 gong yuan 3888`、`公元 gong yuan 424`（`data/dict/pinyin/luna/pinyin.luna.dict.yaml:19767/19768`），
   桶也按 weight 降序排（`dict/dictionary.rs:273/284`），但整句 DP 仍然选 公元 ⇒ 问题在**句级打分**（grammar 罚分盖过词频差），不在词表。
   这正是虎符那套「beam + ngram ln P + 名次罚分」要解决的。
2. **`这样` 输给 `遮阳`**：同上，句级打分。
3. 工作区相对 HEAD 的回归：`gear/translator.rs:392` 把 `blacklist/predict_word` 写死、`:389` 用绝对 `segment.start`
   配段内相对音节图（非首段查不到词）、`menu.rs` 的 `prepare()` 只 `peek` 不 `next`（会重复压同一条）。
4. 全库没有 `max_homophones`（C++ 默认 1，`script_translator.h:58`）。

### 原版 librime 的权威结论（本棒核对，供后面照抄）

- `abc_segmentor` **不切音节**：整串字母 = 一个 segment（`src/rime/gear/abc_segmentor.cc:39-69`），
  音节图在 translator 内部建（`algo/syllabifier.cc:108-222`）。
- 整句在段内做 DP/beam（`gear/poet.cc:191-253`；有 grammar 才 beam，无 grammar 单状态 DP，每词常数 -13.8155 罚分 `gear/grammar.h:18-27`）。
- 排序 `Candidate::compare`：start 小 → **end 大** → quality 大（`candidate.cc:34-49`）⇒ 长覆盖必须在前，这正是线上那版搞错的。

### 虎符（hufu）可借鉴

- 它有真整句：`hufu-sentence` 是独立 crate，字级 beam（按位置分桶、同文本 logsumexp），
  打分 = 出字奖励 2.0 + 逐字 `ln P(c|hist)` + `0.03·ln(rank)`（`engine/crates/hufu-sentence/src/lib.rs:815-836`）；
  无音节层，直接按码表 trie 在每个位置枚举可成码前缀建图（`hufu-dict/src/dict.rs:135-165`）。
- 我们的 `poet` 已有 beam 骨架，缺的是「把 4 本 `.gram`（已部署在 `%APPDATA%\Rime-Next\model\`）真正用进句级打分」这条链路。

### 六·二十六·补：第一轮实修（2026-10-31 11:4x）

**修掉两个必现 bug（都在工作区的懒菜单 v3 里）**

| # | 症状 | 根因 | 修法 | 证据 |
|---|---|---|---|---|
| M1 | 一 compose 就 panic `RefCell already borrowed` | `menu.rs::add_filter` 的 `if let Some(x)=self.result.borrow_mut().take()`，`RefMut` 临时值活到 if-let 结束，函数内再 borrow_mut | 先 `let taken = { …take() };` 再处理 | 修前 `double_pinyin.flypy` 打 nihao 直接 panic；修后正常 |
| M2 | 候选列表全是同一条（打 nihao 出 8 条「你好」） | `menu.rs::prepare` 只 `peek` 不 `next`，循环把第一条反复压入 | 压入后调 `result.next()` | 修前 `你好×8`；修后 `你好/拟好/你/拟/尼/呢/泥/妮` |

**A/B 基线工具**：`bash _shots/ab_candidates.sh`（12 条拼音样例，分别跑线上 dll 与工作区源码，比第一候选）。
可用 `BUILD_DIR=` / `USER_ROOT=` 换根（`C:/Code/_tmp_nomodel` 是**没有 grammar 模型**的副本，用来做「有/无语言模型」对照）。

**基线结果（pinyin.luna，2026-10-31 11:4x）**

- 线上 rime-next.dll：**7/12**
- 工作区（懒菜单 v3 + M1/M2）：**10/12**
- 工作区但去掉 grammar 模型：**9/12**（失败项换了一批：你好时节、我再学习中文、贡院）

失败明细（有 grammar）：
- `womenmingtianqugongyuan` → 我们明天去**公元**（期望 公园）
- `shurufabunengzheyang` → 输入法不能**遮阳**（期望 这样）

**定位到两个不同的根因，下一轮做**

1. **词频数据**：`data/dict/pinyin/luna/pinyin.luna.dict.yaml` 里 `贡院 gong yuan 4300` > `公园 gong yuan 3888`（`:217181` / `:19768`）。
   去掉 grammar 时整句直接选「贡院」⇒ **不是解码问题，是词表权重反了**（公园比贡院常见得多）。
2. **grammar 模型会翻盘**：`这样 131084` vs `遮阳 164`（差 6.7 nats），有 grammar 时却选「遮阳」；
   同一批样例去掉 grammar 反而对（`不能这样` ✓），但另一些样例变差（`你好世界`→`你好时节`）。
   ⇒ octagram 打分链路要单独查：是罚分尺度、还是 n-gram 命中位置（`is_rear`/`collocate`）算错了。

**还没做**：`cargo test -p librime-next --lib` 仍有 56 个编译错（测试还在用急切菜单的写法），本轮没动。

### 六·二十六·补二：第二轮（2026-10-31 11:5x）——语法分实验与词频来源排查

**样例扩到 20 条**（`_shots/ab_candidates.sh`），工作区基线：

| 设置 | 命中 | 失败项 |
|---|---|---|
| grammar 系数 1.0 | **15/20** | 公元×2、网上嘛、学系 |
| 0.7 / 0.6 | **16/20** | 公元×2、网上嘛、学系 |
| 0.5 | 15/20 | 再加「我在学习中文→我再学习中文」 |

**语法分系数**：`gear/poet.rs` 新增 `WEASEL_GRAMMAR_SCALE`（默认 1.0，进程内只读一次），
用来验证「模型语料与日常用语不匹配时，语法分压过词频」的假设。结论：只救回 1 条（这样/遮阳），
不改默认值（20 条样本不足以改全局默认）。

**语法分确实会翻盘（追踪证据，`WEASEL_POET_TRACE=1` 打开）**：

- `这样` entry=-6.64 / grammar=**-12.00** → 总 -18.64
- `遮阳` entry=-13.32 / grammar=**-4.87** → 总 -18.19 ⇒ 选错
- `吗` entry=-6.81 / grammar=**-12.00**  vs  `嘛` entry=-7.95 / grammar=**-2.93** ⇒「你在网上嘛」
- `公园` entry≈-8.4 vs `公元`…同样被语法分翻掉

⇒ 三个失败样本**同一个机制**：语法模型给现代词 -12（没有命中），给一个偏门词 -2.9 ~ -4.9（命中）。
这不是解码 bug，是**模型偏好**；要改只能换模型/调罚分，不能靠调 TIP。

**词频来源排查（重要负面结论，别重做）**：

- 词库 `Rime-Next/dict/pinyin/luna/pinyin.luna.dict.yaml` 的 weight 与随包词频表
  `Rime-Next/eassy/default/essay-zh-hans.txt` **同向但不同源**：多数词约为 essay 的 0.2~1.0 倍，
  但 `贡院 4300`（essay 72）、`学系 36430`（essay 488）是明显污染项。
- **不能**直接拿 essay 重算权重：essay 里有 4 万多个常见词的计数是 **1**（占位值），
  如「性的 / 过程中 / 是一个」全是 1；整表重算会把 99.2% 的词权重改坏（`_shots/fix_dict_weights.py` 已实测）。
  ⇒ 要修只能做**定向修补**（只动被污染且 essay 有可信计数的词条），或换一份干净的现代词频表。

**下一步**：① 定向修 贡院/学系 这类污染词条（列名单 → 只改这些）；② `cargo test -p librime-next --lib` 的 56 个编译错；
③ 语法模型：要么换现代语料模型，要么把「未命中 -12」改成与词频同量级的软罚分。

## 六·二十七、lib 单测迁到懒菜单/流式 API：57 个编译错 → 0，530 全绿（2026-10-31 11:2x～11:3x）

**为什么值得做**：`librime-next` 自己的「硬闸门」要求全量测试绿（`work-13.md`、`STATUS.md`），
但懒菜单 v3 落地后测试还按**急切菜单**写（`Menu::add`、`menu.candidates` 是 `Vec`、`filter.apply(2 参)`），
`cargo test --lib` 直接 57 个编译错 ⇒ 这条闸门一整轮是**哑的**：改坏懒化管线没人拦。

**结果（可复现）**

```bash
export LIBCLANG_PATH='C:\Program Files\llvm-mingw\bin'; export PATH="/c/Program Files/llvm-mingw/bin:$PATH"
cargo test --release -p librime-next --lib     # 530 passed; 0 failed; 0 ignored（6.33s）
cargo build --release -p librime-next --lib    # 非测试代码不受影响（14.84s，仅 warning）
```

**没动生产代码**：只在 `gear/translation.rs` 加了两个 `#[cfg(test)]` 助手
`drain(Box<dyn Translation>) -> Vec<Candidate>` / `drain_opt(Option<…>)`（物化 = `Menu::prepare(usize::MAX)`），
其余全是各文件 `#[cfg(test)]` 段。**没加任何生产侧兼容方法**（不加 `Menu::add` 之类的桥，避免懒化名存实亡）。

**57 个错分四类，按类改法**

| 类 | 数量 | 旧写法 | 新写法 |
|---|---|---|---|
| 急切菜单装候选 | 4（`commit_history.rs`） | `menu.add(Candidate)` | `menu.add_translation(Box::new(VecTranslation::new(vec![cand])))`（测试内 `menu_with()` 助手） |
| 翻译器返回流 | 26（`echo_/history_/punctuator/reverse_lookup_translator`） | `t.query(..)` 当 `Vec` 用（`.len()/[0]/.is_empty()`） | `drain_opt(t.query(..))` 后再断言 |
| 滤镜改成包装流 | 11（`charset_filter/simplifier/reverse_lookup_filter`） | `filter.apply(&ctx, &mut list)` 原地改列表 | `filter.apply(ctx, stream, &mut mat)` 返回新流，`drain()` 物化再断言 |
| 私有/字段 | 其余（`menu.candidates`/`len`） | 直接摸内部字段 | 同上，走公开 API |

**语义没被稀释**：滤镜测试仍是"超纲字就丢 / 开关关着不转换 / append 注释加空格 / 覆写注释取代"这四条断言，
只是断言对象从"被就地改过的列表"换成"包装后的流物化结果"——这正是 runtime 真实走的路。

**验收边界**：本轮只跑 `--lib`。完整 `cargo test -p librime-next` 里的 `lever_deploy` 集成测试会对
**真实数据目录**做一次全量 WorkspaceUpdate（231s，且历史上污染过 build 目录），要跑得单独决定。
`example/probe_candidates.rs` 与本轮无关（没动非测试代码）。

**剩余**（原样保留）：① 定向修污染词条；③ 语法模型/罚分。

---

## 六·二十八、设置界面玻璃化开工 + 大厂功能对照（2026-10-31 11:5x～12:1x）

**用户问的三件事，实况**：① 玻璃化设置界面 —— **没做**（只有 D2D 外壳 v1 + 五页占位）；
② 智能流式输出 —— 用户确认指**候选流式渲染（零等待）**，此前**没学过**（全仓 `大厂|流式输出` 0 命中）；
③ 学大厂功能 —— **没做过**（`1030日志.md` 只有 weasel/虎符/msime 三家开源参考对比）。
本轮两件并行开工。

### 1. 配色数据层解耦（为第二套界面让路）

| 文件 | 变化 |
|---|---|
| `Setting/src/themes.rs` | **新增**：框架无关的配色层（读出厂主题链 + 用户 `theme/theme.yaml`、`rows()`、`select()`、`resolve_shared_dir`、`shared_dir_for_exe`、`user_dir`）。颜色一律内部 ARGB |
| `Setting/src/skins.rs` | 从 369 行缩成薄壳：只做 `ThemeRow → SchemeRow` 搬运 + 转发。**逻辑别写回来**（会漂移） |
| `Setting/src/lib.rs` | `pub mod themes;` |

验收：`cargo test --release -p Weasel-Next-Setting --lib` → **50 passed / 0 failed**（含 `themes::rows_come_from_official_skin`、
`themes::selecting_writes_theme_yaml`）；`cargo check --release -p Weasel-Next-Setting --bin Weasel-Next-Setting` 通过（Slint 侧只报警告）。
aqua 的 ARGB 断言（底 `#eeeeec`、选中块 `#0a3afa`）原样保留 —— 它防的是"界面色块与渲染器解码不一致"。

### 2. D2D 外壳 v2：7 页签 + 第 1 页真实内容

- 页签从自造的 5 页（常规/外观/按键/词典/关于）改成**与 Slint 版一致的 7 页**：
  `主页 / 界面风格 / 方案选单 / 常规 / 更新 / 词库 / 下载`（`Setting/ui/main.slint:89-99`）。
- **「界面风格」已接真数据**：37 套配色、当前 `steam`、色块 = 迷你候选窗（底/高亮背/文字/注释四条）；
  点一下 → `themes.select()` → 写 `%APPDATA%\Rime-Next\theme\theme.yaml` 的 `color_scheme` → 重读刷新；
  滚轮滚动（`WM_MOUSEWHEEL`）、悬停、当前行高亮、"已改"标记。
- 其余 6 页仍是占位卡片（写清"本页尚未迁移"）。
- 新增 `--page=N` 启动参数：截图/验收直接开在指定页，不用抢鼠标去点。
- 证据：`_shots/d2d_page_theme.png`（深色背景）、`_shots/d2d_page_theme_glass.png`
  （**亮渐变垫背**：左侧暖橙、右侧蓝紫透过面板并被模糊 ⇒ 亚克力在真工作，不是纯色板）。
- 新脚本：`_shots/d2d_page_shot.ps1`、`_shots/d2d_glass_backdrop_shot.ps1`（都带 idle 守卫，<10s 直接放弃抓图）。
- 构建命令（**不走 Slint，~40s**）：
  `cargo build --release -p Weasel-Next-Setting --bin Weasel-Next-Setting-D2D --no-default-features`

**没做的**：其余 6 页未迁；D2D 外壳仍是**独立 exe**，没接管默认设置入口，也没进安装包。

### 3. `doc/大厂输入法功能对照.md`（233 行，新增）

7 家（搜狗/微信/百度/讯飞/豆包/微软拼音/Rime 生态）× 14 类；我们逐项现状带证据；候选流式渲染专章；
差距/工作量/优先级表。P0 三项：**① 跑 M1–M6 端到端基线（S）② 分段后先发一帧、放宽可见性（M）③ `Translator::query` 拆层懒算（L）**。

三条关键结论（都核过代码）：

1. 现有"流式"**只到发射层**：`Menu::page` 每键只物化一页（5 条 + 1 条判末页，`menu.rs:140-153`），
   c_api 永远只渲染第 0 页；但 `Translator::query` 返回前已算完整句 DP、用户词、`lookup_from_graph` 收集器
   （`translator.rs:390-403`）⇒ **首候选出现时间没变**。懒化省的是内存/滤镜/排序。
2. 端到端七段里六段在等全量；真卡点只有"引擎整段算完才返回"和"回包之后才发下一帧"。通道本身支持一请求多事件。
3. 两条硬约束：`renderer_bridge.rs:189-196` 的 `visible` 只认"有候选"或"外置 preedit"（inline 模式下首帧前移会看不见）；
   watch 容量 1、覆盖语义 ⇒ 中间帧会被丢。
4. 顺带更正：`1030日志/03-延迟采样怎么用.md` 写的 CSV 简称与实际文件名不一致 ——
   实际以**进程 exe 名**为前缀（`common/src/key_timing.rs:163-171`），即 `Weasel-Next-Engine-key-latency.csv` 等。

**边界**：文档里的毫秒数只有 10-30 那份**急切引擎**实测（`translate_segments` 0.36–32.4 ms/键、内存 583 MB）；
懒化之后的一切标"未验证"，**没有任何新测的数字**（M1–M6 还没跑）。

**下一步**：① 其余六页按 `主页 → 常规 → 方案选单` 顺序迁；② 跑 M1–M6 出基线；③ 玻璃化定稿后再决定是否把 D2D 外壳设为默认入口（要重打包，一轮 14 分钟）。

### 4. 第 2 页：主页（统计）已迁（12:0x）

数据源**不另写**：`weasel_settings::stats::{load, snapshot, today_local}` 就是 Slint 版那一套聚合，
D2D 外壳直接调（`Shell::refresh_stats`，启动时 + 点进主页时各刷一次）。

页面内容：6 张数字卡（今日击键/上屏/速度、连续使用、今日键准、今日击键速度，注脚给累计或近 7 天）、
常用字 13 块（字 + 次数，顶到右边就停）、近 30 天柱状图（今天亮蓝）。

**踩到的坑（记下来）**：`Snapshot.days` 是**整年网格**（`GRID_CELLS` = 53 周 × 7），
直接 `days.iter().rev().take(30)` 拿到的是**12 月还没到的空日子** ⇒ 柱子全 0，看着像画不出来。
必须按 `today_week * 7 + today_day` 截到今天为止再取末 30 格。图上因此左边大片空 = 那几天确实没打字，
标题里直接写明「空白 = 那天没打字」，别让人误判成 bug。

截图：`_shots/d2d_page_home.png`（1250×825，2x DPI）。实测数据：12 天记录、今日 217 键 / 13 字 / 18 字每分、
累计 35,810 键 / 5,870 字、连续 1 天（最长 11）、键准 57.6%（近 7 天 98.3%）、击键速度 5.1 键/秒。

### 5. 第 3 页：常规（表单字段）已迁（12:2x，**只读预览**）

数据链路照抄 Slint 版：`RuntimePaths::discover` → `config::Document::load` → `catalog::load` →
`form::fields(section, &doc)`。**取第 0 个分区**这一点也对齐 Slint：`page_shows(3)` = `row.category == 0 && row.key != "app_options"`。

页面内容：13 个字段一行一个 —— 左「字段名 + 说明（截 34 字）」，右「当前值」（继承值前缀"继承"、覆盖过的用蓝色）。
布尔值的 `true/false` 显示成 **开 / 关**（直接印 true 太像调试输出）。滚轮 + 滚动条与配色页共用 `paint_scrollbar`。

**状态**：**只读**。编辑控件（choice 下拉、颜色、字符串输入）与保存（`Document::save` + 部署）**还没做**，
副标题里直接写「只读预览（编辑控件下一轮）」，别让人以为点不动是坏了。

截图：`_shots/d2d_page_settings.png`（实测：数据文件夹名 = 继承 `Rime-Next`、启用声音 = 开、声音模式 = 音乐模式（放歌）、
按键音 `sound/typewriter-key.wav`、上屏音 `sound/typewriter-bell.wav`、主题 = 继承 `"official"`）。

### 6. 第 4 页：方案选单已迁（12:3x，**可勾选**）

数据链路与 Slint 版完全一致：`weasel_style::switcher::{seed_schema_table, list_schemas, write_schema_ids}`。
首次运行照官方对话框先 `seed_schema_table`（漏了这步界面会"一个都没勾"）。

页面：34 个方案一行一个（勾选块 + 名称 + `id · 系列`），点行进内存态、右下「保存并部署」才落盘
（与官方"确定才生效"一致）；写 `%APPDATA%\Rime-Next\schema\schema.yaml` 后按窗口类名
`weasel-next-broker` 找 broker、`WM_COMMAND` 命令号 1001 请它重新部署 —— 引擎单实例，
设置程序自己拉第二个只会 `AlreadyRunning`（与 `switcher_dialog::request_deploy` 同一做法；
用 `EnumWindows` 不用 `FindWindowW`，后者在部分完整性级别下返回 0 就会"勾了不生效"）。

**实测**：点第一行「倉頡五代」→ 勾消失、状态行 `已选 8 → 7`；再点一次复原 → `7 → 8`。
截图 `_shots/d2d_click_1.png` / `_shots/d2d_click_2.png`（探针 `_shots/d2d_click_test.ps1`，
带 idle 守卫、点完复位光标、只动内存不写盘）。

**未实点**：「保存并部署」按钮会真的写盘并触发一次部署（会短暂重载输入法），留给用户自己点。
代码路径与官方对话框同一套函数，但**直到有人点过之前，这条不算实机验证过**。

### 7. 第 5 页：更新已迁（12:4x）

- 版本 / 安装目录 / 用户数据 / 日志目录 四行 + 「自动检查更新」卡片与开关。
- 数据：`updater::{read, write}`（与 Slint bin 共用同一份 `Setting/src/updater.rs`，用 `#[path]` 挂进 D2D bin）。
  查找顺序：exe 旁边 → 装机目录（`shared_dir` 的父目录）。从 `target\release` 跑时前者没有 dll，会落到后者。
- **实测发现（重要）**：**这个产品根本没随包带 `WinSparkle.dll`** ——
  `C:\Program Files\Rime-Next\Weasel-Next-0.18.5\` 里没有，`installer/weasel-next.nsi` 与
  `script/build-installer.ps1` 里也没有任何 `WinSparkle` 字样。所以 Slint 版的「自动检查更新」开关
  一直是**禁用态**（`ui.set_update_available(auto_update_saved.is_some())` 恒 false），只是被界面藏住了。
  D2D 版照实呈现：开关**置灰**、状态行写明「本产品没随包带 WinSparkle.dll ⇒ 不可用」，不再假装能开。
- 截图：`_shots/d2d_page_update.png`。

### 8. 用户 12:5x 决定：自动更新先不做

已把「自动检查更新」开关从 D2D 更新页**整块删掉**（`mod updater`、`Hover::Switch`、`toggle_update`、`update_switch` 一并移除），
那一行改成 `自动更新 | 暂不做（本产品没随包带 WinSparkle；用户 10-31 定）`。Slint 版那行仍是禁用态，**没动** —— 要不要一起撤等用户说。

### 9. 候选窗「边上多一个方块」的定位（12:5x，用户报）

复现：`_shots/live_cand_square.png`，放大 `_zoom_cand.png`。现象是候选窗左侧多出一块**方角浅灰**，压住圆角面板的左边。

定位：**窗口矩形比圆角面板大**，多出来的那一圈被 DWM 亚克力填了。
- 面板 = `layout.content` = 窗口尺寸 `deflate(offset_x, offset_y)`（`theme/src/layout.rs:897-910`）；
  offset 来自主题阴影：`Metrics::offset()` = `shadow_radius*2 + |shadow_offset| + border*2`（`theme/src/config.rs:566-578`）。
- 亚克力对**整个窗口矩形**开：`accent::sync_acrylic(hwnd, glass, tint)`（`theme/src/mod.rs:481`、`:702`）。
  DWM 的模糊没法裁成圆角或内缩 ⇒ 面板外那圈"什么都没有"的地方实际是一片**被染色的模糊**，看上去就是方块。
- 交换链本身清的是透明（`presenter.rs:275-276`）⇒ 问题不在 D2D，在窗口级亚克力。

三条修法（**未改代码，等拍板**）：
1. **窗口 = 面板**（glass 开时 offset 归零）+ Win11 `DWMWA_WINDOW_CORNER_PREFERENCE` 圆角 → 干净、保玻璃；**代价：候选窗投影没了**。
2. **候选窗关亚克力**（只对候选窗 `glass: false`）→ 方角消失、面板仍半透明；代价：**没有模糊**。
3. 只给窗口加 DWM 圆角、不改尺寸 → 方块变圆角块，**仍有一圈**（不推荐）。

---

## 六·二十九、候选流式渲染端到端基线实测 + 「输入法根本没切过来」的根因（2026-10-31 12:1x～12:3x）

### 0. 三句结论

1. 上一棒「M1–M6 跑不成」**不是仪器问题**：本机默认输入法一直是**老版小狼毫 0.17.4**
   （`C:\WINDOWS\system32\weasel.dll`，类 `{A3F4CDED-…}`，profile 描述就叫「小狼毫」）。按键全进了它，我们的 CSV 一行都不会有。
2. 切到我们之后**一次跑通**，两档 ≥200 键样本：**一键一帧（1.0 帧/按下键）⇒ 没有中间帧，就是没流式**；
   长组合（23 字母）按下 P95 **27.2 ms** 已超 16.7 ms 判据，短组合（7 字母）P95 8.1 ms 合格。
3. 中间只有 **1.0–1.3 ms** 是 RPC＋序列化＋队列，其余全在引擎 `process_key` ⇒
   P0 里「先拆 `Translator::query`」比「改协议」更对症（与 `doc/大厂输入法功能对照.md` §3.6 数字一致）。

### 1. 根因：`{A3F4CDED}` 不是微软拼音，是老版小狼毫（更正上一棒结论）

| 项 | 事实 |
|---|---|
| `HKLM\SOFTWARE\Classes\CLSID\{A3F4CDED-B1E9-41EE-9CA6-7B4D0DE6CB0A}\InprocServer32` | `C:\WINDOWS\system32\weasel.dll`（1,071,104 B，2026-08-14），Wow6432 = `SysWow64\weasel.dll` |
| 该 profile 的 `Description` | **小狼毫**（`LanguageProfile\0x00000804\{3D02CAB6-…}`，`Enable=1`） |
| `{16A7AEA9-…}` 的 `InprocServer32` | `C:\Program Files\Rime-Next\Weasel-Next-0.18.5\Weasel-Next.dll`（= 我们） |
| `HKCU\Control Panel\International\User Profile\zh-Hans-CN` | 两条：`0804:{A3F4CDED-…}=1`（老版）、`0804:{16A7AEA9-…}=2`（我们） |
| Notepad 实载模块（切换前 / 后） | `weasel.dll` 只有它 / 之后 `weasel.dll` + **`Weasel-Next.dll`** 都在 |

- 上一棒写「Notepad 当前用的是**微软拼音**或英文」→ **错**（语言列表里根本没有微软拼音，只有这两条）。
  症状却能对上：按键有反应、候选窗有别家的、我们 CSV 全空。
- **判定法（以后就这么判）**：① 目标进程模块里有没有 `Weasel-Next.dll`（`_shots/ime_probe_check.ps1`）；
  ② `key-latency.csv` 有没有行。**别用「候选窗出现了」当判据** —— 老版也有候选窗。
- **切法**：前台对该窗口发一次 **Win+Space**（列表只有两条，切一次就到我们）。`_shots/ime_switch_test.ps1` 做了这件事并顺带验证
  （切完核模块 + 15 键采样，得 72 行 ⇒ 通了）。**HKL 切前切后都是 `0x08040804`，靠 HKL 判不出来。**
- 现状：采样后仍停在我们这一档；要回老版再按一次 Win+Space。

### 2. 实测数据（安装位 11:34 包，2026-10-31 12:15–12:19）

被测文件（`C:\Program Files\Rime-Next\Weasel-Next-0.18.5\`，sha256 前 16 位）：

| 文件 | 字节 | mtime | sha256 |
|---|---|---|---|
| `rime-next.dll` | 8,630,784 | 11:34:10 | `001B4272EC00BD90…` |
| `Weasel-Next.dll`（TIP） | 7,973,888 | 11:33:52 | `02189673FBFEEB92…` |
| `Weasel-Next-Engine.exe` | 8,822,272 | 11:34:26 | `489E50835DBA1C25…` |
| `Weasel-Next-Renderer.exe` | 8,084,480 | **10:13:20**（本轮没重编） | `ECED6CFEB78639AE…` |
| `Weasel-Next-Server.exe` | 7,585,792 | 10:12:22 | `1F9163A0A3448209…` |

方法：真 Notepad（新进程）+ `SendKeys` 45 ms/字符；开关 `%APPDATA%\Rime-Next\key-latency.on`；
驱动 `_shots/stream_probe.ps1`，统计 `_shots/key_latency_stats.py`；原始 CSV 与 `stats.txt` 留档
`_shots/latency/20261031-1217-long23/`、`_shots/latency/20261031-1219-short7/`（带 sha256）。

| 配置 | 键入 | Tip/Engine/Renderer 行数 | Tip 按下 P50/P95 (ms) | Tip 抬起 P50 | timeout | Engine 按下 P50/P95 | Engine >30ms | 帧÷键事件 | 帧÷Tip ok |
|---|---|---|---|---|---|---|---|---|---|
| 7 字母/轮 × 32 | 256 键 | 1024 / 512 / 512 | **1.89 / 8.14** | 1.40 | 0 | 1.84 / 20.87 | 1/256 | 2.00 | 1.00 |
| 23 字母/轮 × 16 | 384 键（含回车） | 1600 / 768 / 768 | **14.39 / 27.22** | 1.84 | 0 | 13.14 / 25.94 | 6/384 | 2.00 | 0.96※ |

※ 长组合 Engine/Renderer 各 768 行 = 3×256 **无截断**；0.96 是 Tip 把关窗前的 Alt+F4/`n` 也算进分母（400 次按下 vs 384 次键事件）。

**逐键位置（长组合，Engine 按下 P50）**：第 1 字 5.2 ms → 第 6 字 15.0 ms → 第 14 字 20.8 ms → 第 19 字后回落 5–8 ms
（疑似段内搜索被截断/换段）。这就是 `Translator::query` 先把整段算完的形状。

**判定**：① `publish` 仍在 `reply` 之后、一键一帧（按下/抬起各一帧）⇒ **没有中间帧**；
② M1 只在短词合格；③ 主因在引擎 `process_key`；④ 与 `doc/大厂输入法功能对照.md` §3.6 互相印证。

### 3. 本轮新增/改动的工具

| 文件 | 说明 |
|---|---|
| `_shots/ime_probe_check.ps1` | **新**：15 键 + 优雅关窗（触发 Tip `flush_pending`）判定「按键到底进没进我们」。 |
| `_shots/ime_switch_test.ps1` | **新**：等空闲 → Win+Space 切一档 → 核 Notepad 模块 → 少量键验证。 |
| `_shots/stream_probe.ps1` | **改**：等真正空闲（≥12 s，最多等 3 分钟）；不再杀别人的 Notepad（只关自己开的 PID）；抢到前台先核 `Weasel-Next.dll`，没有就退出（省 20 秒白跑）；报被测 dll 的 size/mtime/sha256。 |
| `_shots/key_latency_stats.py` | **新**：P50/P95/P99/max、按下/抬起分列、轮内位置曲线、M5 比值。 |
| `_shots/latency/…` | **新**：两档原始 CSV + `stats.txt` 留档（可复查）。 |

### 4. 坑（新，记下来）

1. **Engine/Renderer 只有「攒够 256 条才落盘」，没有退出 flush** ⇒ 键事件数 × 2 要凑成 256 的整数倍，否则尾部丢样本
   （288 键那轮被截成 512 行，尾 88 条丢；384 键 = 768 行正好）。
2. **Tip 每键 4 行**（按下/抬起 × `ok`/`total`），**Engine/Renderer 每键 2 行**（按下、抬起各 1）⇒ 算 M5 必须说清分母，
   否则会得出「帧比键多」的怪结论。文档里 M5 的原始定义（Renderer 行数 ÷ Tip ok 行数）恰好在 1.0 附近。
3. **新 Notepad 会恢复上次会话的标签内容**：12:0x 那次失败采样留在文档里的乱码（`tianqibucuo…`）会被自动恢复 ——
   别当成用户文档，也别据此判断"输入法没生效"。Notepad 是**单进程多标签**，`Stop-Process Notepad` 会连用户标签一起关
   ⇒ 探针改成只关自己开的 PID。
4. `Win+Space` 切换后 **HKL 不变**（0x08040804），不能靠 HKL 判断；只能看模块与 CSV。
5. 想省时间可以先跑 15 键校准（`ime_probe_check.ps1`），别直接上 288 键——不切输入法时那 20 秒是白打的。

### 5. 下一步

- **P0 第二项「首屏优先」**：`calculate_segmentation` 之后、`translate_segments` 之前发一帧，并放宽
  `Engine/src/renderer_bridge.rs:189-196` 的 `visible`；验收就用本轮基线（长组合按下 P95 27.2 ms → 目标 ≤16.7 ms）。
- 或 **P0 第三项**：`Translator::query` 拆层懒算（数字最支持这条：13.1/14.4 ms 里约 12 ms 在引擎）。
- 补测 M3（要逐键配对：需让探针记录键序，或改成「只打字符不按回车」的配置）、M4、M6（抓图看首候选时的 preedit 长度）。
