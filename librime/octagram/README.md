# rime-octagram（内置到 rime.dll）

语言模型插件，让 `script_translator` 组句时用 N-gram 语言模型给候选加权。

- 上游：<https://github.com/lotem/librime-octagram>（master，2026-10-18 抓取，GPL-3.0，见 `LICENSE`）
- 只取了 `src/`，**去掉了 `tools/`**（建模型用，本项目不需要；构建时用 `-DBUILD_TOOLS=OFF` 也印证）

## 它认的配置（来自 `src/octagram.cc`）

```yaml
grammar:
  language: zh-hans-t-essay-bgw     # 必填；对应 <数据目录>/model/zh-hans-t-essay-bgw.gram
  collocation_max_length: 4         # 以下都有默认值，一般不用写
  collocation_min_length: 3
  collocation_penalty: -12
  non_collocation_penalty: -12
  weak_collocation_penalty: -24
  rear_penalty: -18
```

- 找不到 `grammar/language` 时插件**什么都不做**（不报错、不影响打字）。
- `.gram` 路径由 librime 的资源解析器给出；本项目 `resource.cc` 的补丁把 `*.gram`
  固定解析到 `<root>/model/<名字>.gram`（用户目录优先，其次程序文件夹）。
- 组件名就是 `grammar`；`engine/filters` 里**不需要**写它 —— 是 Poet 内部查的。

## 怎么接上（本仓库）

1. `librime/build-librime-mingw.ps1 -Step librime` 会把本目录同步到
   `<librime>/plugins/octagram`，随 `rime.dll` 一起编（BUILD_MERGED_PLUGINS）。
2. 数据侧：`Rime-Next/grammar.yaml` 提供 `hans` / `hant` 两个 preset，
   方案里已有 `__patch: - grammar:/hant?`（`?` = 文件不存在就跳过）。
3. 换 `rime.dll` 后重启 Server/Engine 才生效（TIP 在宿主进程里另有副本，但
   librime 只在引擎进程里跑，所以不用关打字程序）。
