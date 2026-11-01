//! 用 libgit2 拉取/更新配方包 —— 替代原版 `scripts/fetch-package.sh` 的
//! `git clone --depth 1 --recurse-submodules`。
//!
//! 这样就不再依赖用户机器上装有 Git for Windows（原版在 Windows 上没装 git 时
//! 只能退回到 ZIP 下载，且没有增量更新）。
//!
//! 进度条与代理参数抄自 `fxliang/rime-cli`（indicatif 进度 + `--proxy`）：
//! 原版 libgit2 往 stderr 打的是滚动文本，这里换成单行进度条。

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use git2::build::{CheckoutBuilder, RepoBuilder};
use git2::{AutotagOption, FetchOptions, Progress, RemoteCallbacks, Repository};
use indicatif::{ProgressBar, ProgressStyle};

/// 一次拉取的参数。
#[derive(Debug, Clone, Default)]
pub struct FetchOpts {
    /// 代理地址（`http://127.0.0.1:7890`），同时作用于 git 与 https 下载。
    pub proxy: Option<String>,
}

/// 拉一个包到本地目录。返回人类可读的动作说明（"下载" / "更新"）。
///
/// `url` 由 [`crate::mirror::git_url`] 生成；镜像切换时同一个目录会换 origin 地址，
/// 这里每次都把 origin 校正成当前 url。
pub fn clone_or_update(
    package_dir: &Path,
    url: &str,
    branch: Option<&str>,
    opts: &FetchOpts,
) -> Result<String, String> {
    if package_dir.join(".git").exists() {
        update_existing(package_dir, url, branch, opts).map(|_| "更新".to_string())
    } else {
        clone_new(package_dir, url, branch, opts).map(|_| "下载".to_string())
    }
}

/// 仅做 clone（不判断「是否已存在」），返回**原始 git2 错误**，
/// 以便调用方用 `e.code()` / `e.class()` / `e.message()` 上报状态与原因。
pub(crate) fn clone_only(
    package_dir: &Path,
    url: &str,
    branch: Option<&str>,
    opts: &FetchOpts,
) -> Result<(), git2::Error> {
    if let Some(parent) = package_dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| git2::Error::from_str(&e.to_string()))?;
    }
    let state = new_state(
        &package_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
    );
    let mut fo = FetchOptions::new();
    fo.depth(1);
    fo.download_tags(AutotagOption::None);
    fo.remote_callbacks(remote_callbacks(&state));
    apply_proxy(&mut fo, opts);

    let mut co = CheckoutBuilder::new();
    co.progress(|path, cur, total| {
        let mut st = state.borrow_mut();
        st.path = path.map(|p| p.to_path_buf());
        st.current = cur;
        st.total = total;
        update_progress_bar(&mut st);
    });

    let mut builder = RepoBuilder::new();
    builder.fetch_options(fo).with_checkout(co);
    if let Some(b) = branch {
        builder.branch(b);
    }
    let result = builder.clone(url, package_dir).map(|_| ());
    state.borrow_mut().pb.finish();
    result
}

/// 克隆一个新仓库（clone 失败返回人类可读的中文错误）。
fn clone_new(
    package_dir: &Path,
    url: &str,
    branch: Option<&str>,
    opts: &FetchOpts,
) -> Result<(), String> {
    clone_only(package_dir, url, branch, opts).map_err(|e| format!("clone 失败: {e}"))
}

fn update_existing(
    package_dir: &Path,
    url: &str,
    branch: Option<&str>,
    opts: &FetchOpts,
) -> Result<(), String> {
    let repo = Repository::open(package_dir).map_err(|e| format!("打开仓库失败: {e}"))?;
    // 镜像可能换了：把 origin 地址校正成当前使用的镜像
    let remote = repo.find_remote("origin").map_err(|e| e.to_string())?;
    if remote.url().unwrap_or_default() != url {
        repo.remote_set_url("origin", url).map_err(|e| e.to_string())?;
        // 重新取一次，拿到改后的 remote
        drop(remote);
    }
    let mut remote = repo.find_remote("origin").map_err(|e| e.to_string())?;

    let state = new_state(
        &package_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
    );
    let mut fo = FetchOptions::new();
    fo.depth(1);
    fo.download_tags(AutotagOption::None);
    fo.remote_callbacks(remote_callbacks(&state));
    apply_proxy(&mut fo, opts);

    let refspecs: Vec<String> = match branch {
        Some(b) => vec![format!("+refs/heads/{b}:refs/remotes/origin/{b}")],
        None => Vec::new(), // 空 = 用 remote 已配置的 refspec
    };
    let result = remote
        .fetch(&refspecs, Some(&mut fo), None)
        .map_err(|e| format!("fetch 失败: {e}"));
    state.borrow_mut().pb.finish();
    result?;

    // 硬重置到拉回来的最新提交（等价于原版 update-package.sh 的效果）
    let target = if let Some(b) = branch {
        repo.find_reference(&format!("refs/remotes/origin/{b}"))
            .map_err(|e| e.to_string())?
    } else {
        repo.find_reference("FETCH_HEAD").map_err(|e| e.to_string())?
    };
    let commit = repo
        .reference_to_annotated_commit(&target)
        .map_err(|e| e.to_string())?;
    let obj = repo.find_object(commit.id(), None).map_err(|e| e.to_string())?;
    repo.reset(&obj, git2::ResetType::Hard, None)
        .map_err(|e| format!("重置失败: {e}"))?;
    Ok(())
}

/// 把 git 拉取进度桥接到 indicatif 进度条（rime-cli 同款逻辑）。
struct State {
    progress: Option<Progress<'static>>,
    total: usize,
    current: usize,
    path: Option<PathBuf>,
    pb: ProgressBar,
}

fn new_state(label: &str) -> RefCell<State> {
    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40}] [eta: {eta}]\n  {msg}")
            .unwrap()
            .progress_chars("█>-"),
    );
    RefCell::new(State {
        progress: None,
        total: 0,
        current: 0,
        path: Some(label.into()),
        pb,
    })
}

fn update_progress_bar(state: &mut State) {
    if let Some(progress) = &state.progress {
        state.pb.set_length(progress.total_objects() as u64);
        state.pb.set_position(progress.received_objects() as u64);
    } else {
        state.pb.set_length(state.total as u64);
        state.pb.set_position(state.current as u64);
    }
    let message = if let Some(stats) = &state.progress {
        let network_pct = (100 * stats.received_objects()) / stats.total_objects();
        let index_pct = (100 * stats.indexed_objects()) / stats.total_objects();
        let co_pct = if state.total > 0 {
            (100 * state.current) / state.total
        } else {
            0
        };
        let kbytes = stats.received_bytes() / 1024;
        if stats.received_objects() == stats.total_objects() {
            format!(
                "Resolving deltas {}/{}",
                stats.indexed_deltas(),
                stats.total_deltas()
            )
        } else {
            format!(
                "net {}% ({} kb, {}/{}) / idx {}% ({}/{}) / chk {}% ({}/{}) {}",
                network_pct,
                kbytes,
                stats.received_objects(),
                stats.total_objects(),
                index_pct,
                stats.indexed_objects(),
                stats.total_objects(),
                co_pct,
                state.current,
                state.total,
                state
                    .path
                    .as_ref()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )
        }
    } else {
        String::new()
    };
    state.pb.set_message(message);
}

fn remote_callbacks(state: &RefCell<State>) -> RemoteCallbacks {
    let mut cb = RemoteCallbacks::new();
    cb.transfer_progress(|stats| {
        let mut st = state.borrow_mut();
        st.progress = Some(stats.to_owned());
        update_progress_bar(&mut st);
        true
    });
    cb
}

/// 给 git 拉取挂上代理（rime-cli 的 `--proxy`）。
fn apply_proxy(fo: &mut FetchOptions, opts: &FetchOpts) {
    if let Some(proxy) = &opts.proxy {
        let mut po = git2::ProxyOptions::new();
        po.url(proxy);
        fo.proxy_options(po);
    }
}

/// 当前检出的分支名（用于日志展示）。
pub fn current_branch(package_dir: &Path) -> Option<String> {
    let repo = Repository::open(package_dir).ok()?;
    let head = repo.head().ok()?;
    Some(head.shorthand().unwrap_or("?").to_string())
}
