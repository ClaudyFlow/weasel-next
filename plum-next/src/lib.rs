//! 東風破 /plum/ 的 Rust 重写 —— PlumNext。
//!
//! 与原版 `rime/plum` 的行为对齐（逐句对照 `scripts/*.sh`）：
//! * `scripts/resolver.sh`  → [`target`]：`<user>/<repo>@<branch>:<recipe>:k=v`
//! * `scripts/fetch-package.sh` → [`fetch`]：`git clone --depth 1`（这里用 libgit2）
//! * `scripts/recipe.sh`    → [`recipe`]：`download_files` / `install_files` / `patch_files`
//! * `scripts/install-packages.sh` → [`install`]：数据文件挑选与"无变化则不覆盖"
//!
//! 原版之外的增量：**镜像自动嗅探与自动切换**（[`mirror`]）。

pub mod fetch;
pub mod install;
pub mod mirror;
pub mod org;
pub mod progress;
pub mod recipe;
pub mod relocate;
pub mod target;
