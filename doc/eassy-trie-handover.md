# eassy + trie 落地设计（2026-11-05，交给下一棒）

> 一句话：**eassy 不进词库、不生成编码；运行时用字→音表建 trie，打字时按音节前缀直接匹配。**
> 数据管线与速度已实测（见 §2），引擎接入未做（见 §5 的工作流）。

---

## 1. 设计

### 1.1 为什么不能走"生成词库"

eassy（`eassy/default/default.eassy.yaml` 8.1 MB + `essay-zh-hans.txt` 24 万行）**只有词和词频，没有编码**。
`use_preset_vocabulary` 只给**已有词条加权**（`entry_collector.rs:84-95` 调 `get_weight_for_entry`），
不会新增词。要变成能打的词，必须给每个词注音 → 也就是为**每个方案**各生成一份派生词库：

- 拼音/双拼/五笔/仓颉/注音 各一套，每套 15–25 MB（`.table.bin` 级）；
- 10 个方案 ≈ **150–250 MB 落盘**，而且每次 deploy 都要重编；
- 多音字注错就是"怪词进候选"。

⇒ 用运行时匹配替代：**零落盘、一份索引全局共享**。

### 1.2 运行时结构（trie）

```
Node {
    children: HashMap<String, Node>,   // 音节 → 子节点
    words: Vec<(String, f64)>,         // 该前缀下的词，按词频排序，截断到 N 条
}
```

- 构建：
  1. 从方案词库的**单字行**抽「字 → 音节」表（`pinyin.luna.dict.yaml` 91.6 万行，449 ms，7,139 字）；
  2. 逐条读 eassy（24 万行，`词\t词频`），只收 2–4 字、且每个字都有读音的；
  3. 按音节序列插 trie；每个节点的 `words` 按词频排序并截断（建议 N=32）。
- 查询：输入音节串（如 `ni hao`）按空格切，逐层下钻；命中节点的 `words` 就是候选。
  **前缀天然支持**：打一半 `ni` 也能拿到 `ni*` 的所有词。
- 只对拼音系（luna/terra）启用；`WEASEL_EASSY=off` 关闭；构建放后台线程，
  未就绪时查询返回空（输入法照常）。

### 1.3 接入点（下一棒要改的三处）

| 位置 | 改什么 |
|---|---|
| `librime-next/src/dict/eassy_matcher.rs`（新） | `spawn(data_roots, char2syllable)` + `lookup(syllables) -> &[(word,freq)]` + 全局 `OnceLock<Arc<_>>` + 开关 + `catch_unwind` |
| `src/engine/runtime.rs` | 段翻译之后、神经重排/预测之前，把命中的 eassy 词追加成 `Candidate`（`comment` 打 `eassy` 标记），保持原候选顺序不变 |
| `src/engine/gear/registry.rs` | 给 matcher 提供「字→音节」表来源（从 translator 已打开的词典/编译产物取，或启动时读一次 `*.dict.yaml` 的单字行） |

异步补偿：索引建好后，走一次与 `rescore::poll_rescore_landing` 同款的 re-compose，
把 eassy 候选补进菜单（首屏不阻塞）。

---

## 2. 实测数据（Rust，真数据）

`librime-next/example/eassy_match_bench.rs`（已注册进 `Cargo.toml`）：

```text
字→拼音: 7139 个,  449 ms
索引: 213646 词 / 152233 键, 跳过 26313, 构建 679 ms
估算内存: 10.0 MB
查询: 0.071 µs/次（160 万次实测）
```

| 指标 | 值 |
|---|---|
| 构建（一次性，后台） | **1.13 s** |
| 内存 | **10.0 MB**（扁平索引）；**trie 版估 25–30 MB**（30–40 万节点） |
| 单次查询 | **71 ns**（一次按键里查 100 次 ≈ 7 µs，可忽略） |
| 磁盘 | **0**（不落盘；eassy 原文件本来就是安装包/数据根里已有的） |
| 覆盖 | 213,646 个 2–4 字词（24 万行的 89%），跳过 26,313（含生僻字/长度不合） |

对比：一次 DirectWrite 排版/一次按键处理在**几十微秒**量级 ⇒ eassy 查询不是瓶颈。


### 2.1 trie 版实测（2026-11-05 21:40，模块已落地）

`src/dict/eassy_matcher.rs`（5 条单测全绿）+ `example/eassy_match_bench.rs`：

```text
构建 1706 ms | 节点 195091 | 词 213593 | 跳过 26313 | 估算内存 30.5 MB
查询 0.636 µs/次（160 万次）
  ni hao -> 你好(4869), 逆号(1)
  zhong guo -> 中国(337254), 种过(40)
  shi jie -> 世界(121499), 时节(1340), 师姐(1073)
```

| 指标 | 扁平 HashMap | **trie** |
|---|---|---|
| 构建 | 1.13 s | **1.71 s** |
| 内存 | 10.0 MB | **30.5 MB** |
| 查询 | 0.071 µs | **0.636 µs** |
| 前缀命中 | ✗ | ✓ |

结论：trie 用 3 倍内存、9 倍查询时间换"打一半就能出候选"，仍然远低于一次按键的开销
（几百 µs 级）。**推荐 trie**；如果只要完整音节命中，扁平版更省。

---

## 3. 磁盘空间对比（"生成词库" vs "运行时匹配"）

| 方案 | 磁盘 | 内存 | 每次 deploy |
|---|---|---|---|
| 每方案生成 eassy 派生词库（10 个方案） | **+150–250 MB**（每个 `.table.bin` 15–25 MB） | 0 额外 | 重编，分钟级 |
| **运行时 trie（推荐）** | **0** | +10–30 MB（常驻，全局一份） | 无 |

补充：现在 luna 的 `pinyin.luna.table.bin` 已是 **36.4 MB**（开 `use_preset_vocabulary` 后重建，
只改权重不改词数）；如果真去"生成词库"，这个数字还会再涨。

---

## 4. 用户词库：现在是什么状态

- **格式：最新五字段**（librime-evo），落盘形如 `21,3.50902,3402,2,6fn3k`，旧三字段
  `c=21 d=3.50902 t=3402` 仍可读（缺的字段按 0）。
- **五个字段的含义**（`src/dict/user_db.rs:52-72`）：

| 字段 | 名字 | 含义 |
|---|---|---|
| `c` | commits | 累计选词次数；**负数 = 用户删除了该词** |
| `d` | dee | 衰减后的词频权重（`algo::dynamics::formula_d` 迭代，上限 10000） |
| `t` | tick | 最后一次改动的 tick（算衰减用） |
| `p` | position | **手排位置**（1 基；0 = 自动排序） |
| `m` | manual_stamp | 手排名次戳（36 进制短码；两处手排冲突时"新的覆盖旧的"） |

- **现状（实测）**：`%APPDATA%\Rime-Next\userdb\` 里的条目**仍是旧三字段**（读兼容）；
  新写入才是五字段。库名 = 方案 `translator/dict:` 的第一段（`pinyin.luna/terra/ziranma`
  共用 `pinyin.userdb`），**不是方案 id**。
- **没做的**：① 旧条目重写成五字段（纯数据迁移，`user_dict_upgrade` 现在是 no-op，
  因为没注册 legacy_userdb）；② 手排功能的宿主入口（`set_manual_position` 等三个方法
  **全仓零调用者**，C API 没导出），所以 `user_dict_mode: manual` 目前是摆设；
  ③ 092 方案 `enable_user_dict: false`，**一个字都不学**。

---

## 5. 下一棒工作流（按顺序，每步可单独验证）

1. **先跑基线**：`cd C:\Code\Weasel-Next\librime-next && cargo run --release -p librime-next --example eassy_match_bench`
   记下三行输出（构建/内存/查询），作为回归基线。
2. **写 `eassy_matcher.rs`**（trie 版，§1.2），带单测：命中、前缀、未知字跳过、开关关闭。
3. **接线**（§1.3 三处），候选追加时保持原顺序、带 `eassy` 标记；`WEASEL_EASSY=off` 行为与现在完全一致。
4. **测四件事**：
   - `cargo test -p librime-next --lib`（当前基线 626+ 条，必须全绿）；
   - 按键延迟：用现有 `example/neural_bench` 或临时计时，对比开/关 eassy；
   - 内存增量：任务管理器看 `Weasel-Next-Engine.exe` 的提交大小（预期 +10–30 MB）；
   - 索引未就绪时输入法照常（首屏只出词典候选）。
5. **部署**：`deploy-librime.ps1 -InstallDir "C:\Program Files\Weasel-Next"`（gsudo），
   然后真机打 20 个字，确认不卡、候选正常；出问题就把 `.bak` 的 dll 换回来。
6. **记录**：把实测的「打字速度/内存/磁盘」三行补回本文件 §2，并更新
   `C:\Code\checkpoint-5bc98a52.md`。

### 红线

- 索引构建/查询**绝不 panic**：外层 `catch_unwind`，失败就当作"没有 eassy"。
- 查询在按键路径上只允许一次 trie 下钻（几十 ns）；构建/加载必须在后台线程。
- 不改任何 `*.schema.yaml`；eassy 是全局词源，不是方案选项。
- 部署必须能一键回滚（`weasel-next-theme.dll.bak-*` / `rime-next.dll.bak-*` 模式）。
