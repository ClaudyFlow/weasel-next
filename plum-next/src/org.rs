//! GitHub 组织仓库浏览 —— 供设置面板「下载」页列出 `Rime-Next` 组织下的仓库。
//!
//! 只做只读的列表拉取（GitHub REST API）；具体克隆与搬运复用 [`relocate`]。
//! 组织名写死，镜像自动嗅探由 plum 的 `fetch` 在克隆时负责，这里不涉及。

use serde::Deserialize;

/// 写死的 GitHub 组织（下载页只遍历它下面的仓库）。
pub const ORG: &str = "Rime-Next";

/// GitHub REST 返回的仓库摘要（只取需要的字段）。
#[derive(Clone, Debug, Deserialize)]
pub struct Repo {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub clone_url: String,
    #[serde(default)]
    pub default_branch: Option<String>,
    #[serde(default)]
    pub stargazers_count: Option<u32>,
    #[serde(default)]
    pub language: Option<String>,
    /// 仓库分类标签（GitHub topics）。
    #[serde(default)]
    pub topics: Vec<String>,
    /// 最近一次 push 的时间（ISO8601）。设置页拿它和「上次拉取时记下的 pushed_at」比对，
    /// 判断有没有新版本可拉（2026-10-30 加，用于「更新拉取」）。
    #[serde(default)]
    pub pushed_at: Option<String>,
}

/// 列出组织的公开仓库。
///
/// 返回 `Ok(仓库列表)`；失败时返回 `Err((错误文案, 是否网络问题))`。
/// 网络波动/断联时第二个分量为 true，其它（404 / 限流 / 解析失败）为 false。
/// 调用方据此决定是否弹网络通知横幅。
/// 列表拉取的候选基址:GitHub 直连优先,其后全部内置镜像的 **API 形式**。
///
/// gh-proxy 系镜像的通用规则:`<镜像>https://github.com/` 能代理,则
/// `<镜像>https://api.github.com/` 同样能代理 API——由镜像表自带的前缀推导,
/// 不在镜像表里另设字段。直连实测可用时第一个就命中;断连时逐个镜像兜底。
fn api_bases() -> Vec<String> {
    let mut out = vec!["https://api.github.com".to_string()];
    for mirror in crate::mirror::builtin() {
        if let Some(base) = mirror.git.strip_suffix("https://github.com/") {
            out.push(format!("{base}https://api.github.com"));
        }
    }
    out
}

/// 逐候选拉取组织仓库列表:直连优先,断连时按镜像逐个兜底。
///
/// 每个候选短超时(8 秒)——断连的候选死得快,不能拖死整条链。
/// 全部失败才返回最后一个错误(调用方据此弹网络横幅)。
pub fn list_repos(org: &str) -> Result<Vec<Repo>, (String, bool)> {
    let path = format!("/orgs/{org}/repos?per_page=100&type=public");
    let client = reqwest::blocking::Client::builder()
        .user_agent("weasel-marketplace")
        .timeout(std::time::Duration::from_secs(8))
        // 镜像链+直连自足,不碰系统代理——系统代理指向的本地端口经常没开
        // (真机 2026-10-28:WinINET 127.0.0.1:7890 已死,reqwest 默认检测
        // 系统代理导致所有请求 EOF,而 curl 直连同秒 200)。
        .no_proxy()
        .build()
        .map_err(|e| (format!("网络请求失败: {e}"), true))?;

    let mut last_err: Option<(String, bool)> = None;
    for base in api_bases() {
        let url = format!("{base}{path}");
        // 每个候选两次尝试:GitHub 直连在国内是**分钟级通断**摆动,
        // 短歇重试能抓住恢复窗口(实测:20 分钟内 200↔断连摆了三回)。
        let mut resp = None;
        for attempt in 0..2 {
            match client
                .get(&url)
                .header("Accept", "application/vnd.github+json")
                .send()
            {
                Ok(r) => {
                    resp = Some(r);
                    break;
                }
                Err(e) if attempt == 0 => {
                    let network = e.is_connect() || e.is_timeout() || e.is_request();
                    let text = if e.is_connect() {
                        "网络连接已断开，无法访问 GitHub".to_owned()
                    } else if e.is_timeout() {
                        "网络波动，连接超时".to_owned()
                    } else {
                        format!("网络请求失败: {e}")
                    };
                    last_err = Some((text, network));
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                }
                Err(e) => {
                    let network = e.is_connect() || e.is_timeout() || e.is_request();
                    let text = if e.is_connect() {
                        "网络连接已断开，无法访问 GitHub".to_owned()
                    } else if e.is_timeout() {
                        "网络波动，连接超时".to_owned()
                    } else {
                        format!("网络请求失败: {e}")
                    };
                    last_err = Some((text, network));
                }
            }
        }
        let resp = match resp {
            Some(resp) => resp,
            None => continue, // 换下一个候选
        };
        if !resp.status().is_success() {
            // 4xx/5xx 属于服务侧/镜像侧,不算网络波动,但同样换下一个候选
            last_err = Some((
                format!("GitHub 返回 {}: {}", resp.status(), resp.text().unwrap_or_default()),
                false,
            ));
            continue;
        }
        return resp
            .json()
            .map_err(|e| (format!("解析响应失败: {e}"), false));
    }
    Err(last_err.unwrap_or_else(|| ("无可用候选".to_owned(), true)))
}
