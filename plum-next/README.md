# PlumNext —— 東風破 /plum/ 的 Rust 重写

上游 [`rime/plum`](https://github.com/rime/plum)（東風破）是 Rime 的配置/配方管理器，
原版是一组 bash 脚本。PlumNext 用 Rust 重写，解决原版在 Windows 上的两个痛点：

| 原版痛点 | PlumNext |
|---|---|
| 依赖 Git for Windows；没装就退化成 ZIP 下载，且**没有增量更新** | 用 **libgit2** 直接拉包，用户机器上不需要装 git；已存在的包走 `fetch + 硬重置` 增量更新 |
| 直连 `https://github.com`，国内经常拉不动且**没有退路** | **镜像自动嗅探 + 自动切换**：内置 7 个镜像，按延迟排序，失败立即换下一个 |

## 用法

```bat
plum-next                       :: 等价于 :preset
plum-next :preset               :: 预设配方清单（也可 :extra / :all）
plum-next prelude emoji         :: 安装指定包
plum-next lotem/rime-zhung@master:my-recipe:a=1   :: 完整"配方订单"
plum-next --select :preset      :: 交互式挑选
plum-next --sniff               :: 只嗅探镜像并打印排序
plum-next --list-mirrors        :: 列出内置镜像
```

常用选项：

| 选项 | 说明 |
|---|---|
| `--rime-dir <目录>` | Rime 用户目录，默认 `%APPDATA%\Rime-Next`（与 Weasel-Next 一致） |
| `--plum-dir <目录>` | 配方包缓存目录，默认 `%LOCALAPPDATA%\Rime-Next\plum` |
| `--mirror <id>` | 强制指定镜像，跳过嗅探 |
| `--no-update` | 已存在的包不拉更新 |
| `--select` | 交互式选包 |

### 配方订单（Rx）的完整形式

```text
<github-user>/<repository-name>@<branch>:<recipe>:key=value,...
```

`<github-user>/`、`@<branch>`、`:<recipe>...` 都可以省略。
没写用户名时，仓库名按原版规则补全：不带 `rime-` 前缀就补成 `rime/rime-<name>`。

## 与原版脚本的对应

| 原版 | 本仓库 |
|---|---|
| `scripts/resolver.sh` | `src/target.rs` |
| `scripts/fetch-package.sh` | `src/fetch.rs`（git2） |
| `scripts/recipe.sh` | `src/recipe.rs` |
| `scripts/install-packages.sh` | `src/install.rs` |
| （无，原版写死 github） | `src/mirror.rs` |

## 镜像机制

* **嗅探**：并发 TCP 建连（443，超时 1500 ms），按"可达优先 + 延迟升序"排序。
  只测建连不发包，排序够快；真正的可用性交给后续 clone/下载判定（失败就切换）。
* **切换**：`Picker::attempt()` 按排序依次尝试，失败即前进到下一个镜像；
  全部失败时错误里带 `[镜像id]`，便于定位是哪个源挂了。
* **记忆**：上次成功的镜像写进 `<plum_dir>/mirror.txt`，下次提到队首（快路径），它挂了再重新嗅探。
* 切镜像**不需要重新 clone**：`fetch.rs` 每次都把 origin 地址校正成当前镜像。

内置镜像：`github`、`ghproxy`、`gh-proxy`、`mirror-ghproxy`、`ghfast`、`gitclone`、`gitmirror`。

## 已知边界

1. **子模块**（原版 `--recurse-submodules`）未实现。
2. `patch_files` 只做"写进 `__patch:` 段 + 同 Rx 幂等替换"，没实现原版 `eval` 选项变量那套。
3. `:preset` 清单优先读本地 `<plum_dir>/preset-packages.conf` → 联网拉 `rime/plum/master/…`
   → 最后用内置兜底清单（README 里的 22 个包）。

设计记录另见仓库 `doc/memo.md` 的「PlumNext」一节。
