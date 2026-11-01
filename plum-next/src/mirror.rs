//! 镜像自动嗅探与自动切换 —— 这是 PlumNext 相对原版 /plum/ 的增量部分。
//!
//! 原版只会直连 `https://github.com`（`scripts/fetch-package.sh` 把 URL 写死），
//! 国内环境下经常拉不动、也没有退路。这里做两件事：
//!
//! * **嗅探**：并发探测各镜像的建连耗时，按"可达 + 最快"排序；
//! * **切换**：真正执行 clone/下载时按排序依次尝试，失败就标记该镜像失效并换下一个；
//!   上次成功的镜像记进状态文件，下次优先用它（快路径），它挂了再重新嗅探。

use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 一个镜像源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mirror {
    /// 稳定标识（写进状态文件）。
    pub id: String,
    /// 人类可读名字。
    pub label: String,
    /// git 地址前缀：`{git}<package>.git`，package 形如 `rime/rime-luna-pinyin`。
    pub git: String,
    /// 原始文件前缀：`{raw}<user>/<repo>/<branch>/<file>`。
    pub raw: String,
}

/// 内置镜像表。第一条是上游直连（基准，永远留在候选里）。
pub fn builtin() -> Vec<Mirror> {
    vec![
        Mirror {
            id: "github".to_string(),
            label: "GitHub 直连".to_string(),
            git: "https://github.com/".to_string(),
            raw: "https://raw.githubusercontent.com/".to_string(),
        },
        Mirror {
            id: "ghproxy".to_string(),
            label: "ghproxy.com".to_string(),
            git: "https://ghproxy.com/https://github.com/".to_string(),
            raw: "https://ghproxy.com/https://raw.githubusercontent.com/".to_string(),
        },
        Mirror {
            id: "gh-proxy".to_string(),
            label: "gh-proxy.com".to_string(),
            git: "https://gh-proxy.com/https://github.com/".to_string(),
            raw: "https://gh-proxy.com/https://raw.githubusercontent.com/".to_string(),
        },
        Mirror {
            id: "mirror-ghproxy".to_string(),
            label: "mirror.ghproxy.com".to_string(),
            git: "https://mirror.ghproxy.com/https://github.com/".to_string(),
            raw: "https://mirror.ghproxy.com/https://raw.githubusercontent.com/".to_string(),
        },
        Mirror {
            id: "ghfast".to_string(),
            label: "ghfast.top".to_string(),
            git: "https://ghfast.top/https://github.com/".to_string(),
            raw: "https://ghfast.top/https://raw.githubusercontent.com/".to_string(),
        },
        Mirror {
            id: "gitmirror".to_string(),
            label: "hub.gitmirror.com".to_string(),
            git: "https://hub.gitmirror.com/https://github.com/".to_string(),
            raw: "https://hub.gitmirror.com/https://raw.githubusercontent.com/".to_string(),
        },
        Mirror {
            id: "gitclone".to_string(),
            label: "gitclone.com".to_string(),
            git: "https://gitclone.com/github.com/".to_string(),
            raw: "https://raw.githubusercontent.com/".to_string(),
        },
    ]
}

/// 用户自定义镜像文件：`<plum_dir>/mirrors.txt`，每行 `id|git前缀|raw前缀`，
/// `#` 开头是注释，`raw` 缺省时按 github 规则推导。
pub fn user_mirrors_path(plum_dir: &Path) -> PathBuf {
    plum_dir.join("mirrors.txt")
}

/// 解析自定义镜像文本。坏行跳过（不报错，保证半途断网/手误不会炸）。
pub fn parse_user_mirrors(text: &str) -> Vec<Mirror> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let Some(id) = parts.next() else { continue };
        let Some(git) = parts.next() else { continue };
        let raw = parts.next().map(str::trim).filter(|s| !s.is_empty());
        let (id, git) = (id.trim(), git.trim());
        if id.is_empty() || git.is_empty() {
            continue;
        }
        out.push(Mirror {
            id: id.to_string(),
            label: id.to_string(),
            git: git.to_string(),
            raw: raw.map(str::to_string).unwrap_or_else(|| derive_raw(git)),
        });
    }
    out
}

/// 从 git 前缀推导 raw 前缀：`https://gh.example/https://github.com/` →
/// `https://gh.example/https://raw.githubusercontent.com/`；推不出就原样返回。
fn derive_raw(git: &str) -> String {
    match git.find("https://github.com/") {
        Some(pos) => format!("{}https://raw.githubusercontent.com/", &git[..pos]),
        None => git.to_string(),
    }
}

/// 载入用户自定义镜像（文件不存在则为空）。
pub fn load_user_mirrors(plum_dir: &Path) -> Vec<Mirror> {
    match std::fs::read_to_string(user_mirrors_path(plum_dir)) {
        Ok(text) => parse_user_mirrors(&text),
        Err(_) => Vec::new(),
    }
}

/// 内置 + 自定义合并；同 id 时自定义覆盖内置（保留其位置），新 id 追加在后。
pub fn all_mirrors(plum_dir: &Path) -> Vec<Mirror> {
    let mut out = builtin();
    for m in load_user_mirrors(plum_dir) {
        match out.iter_mut().find(|x| x.id == m.id) {
            Some(slot) => *slot = m,
            None => out.push(m),
        }
    }
    out
}

/// 追加/更新一条自定义镜像到镜像文件（持久化，下次运行自动纳入探测）。
pub fn add_user_mirror(
    plum_dir: &Path,
    id: &str,
    git: &str,
    raw: Option<&str>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(plum_dir)?;
    let path = user_mirrors_path(plum_dir);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    // 同 id 旧行剔除（用 id 段比对，注释行原样保留）
    let kept: Vec<String> = existing
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
        .filter(|l| l.split('|').next().map(str::trim) != Some(id))
        .map(str::to_string)
        .collect();
    let line = match raw {
        Some(r) if !r.trim().is_empty() => format!("{id}|{git}|{}", r.trim()),
        _ => format!("{id}|{git}"),
    };
    let mut text = String::from("# 用户自定义镜像：每行 id|git前缀|raw前缀（raw 缺省按 github 规则推导）\n");
    for l in kept {
        text.push_str(&l);
        text.push('\n');
    }
    text.push_str(&line);
    text.push('\n');
    std::fs::write(&path, text)
}

/// 按 id 查内置镜像。
pub fn by_id(id: &str) -> Option<Mirror> {
    builtin().into_iter().find(|m| m.id == id)
}

/// git 地址：`{git}<package>.git`。
pub fn git_url(mirror: &Mirror, package: &str) -> String {
    format!("{}{}.git", mirror.git, package)
}

/// 原始文件地址：`{raw}<user>/<repo>/<branch>/<file>`。
pub fn raw_url(mirror: &Mirror, repo_path: &str) -> String {
    format!("{}{}", mirror.raw, repo_path.trim_start_matches('/'))
}

/// 从 URL 取主机名（用于探测）。
pub(crate) fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or("");
    // 去掉可能的 user:pass@ 与端口
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// 单个镜像的探测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub mirror: Mirror,
    /// 建连耗时（`None` = 不可达 / 超时）。
    pub latency: Option<Duration>,
}

/// 并发探测所有镜像的可达性与延迟；返回按"可达优先、延迟升序"排好的结果。
///
/// 探测只做 TCP 建连（443），不发请求：目的是排序，真正的可用性由后续
/// clone/下载决定（失败会自动切换），这样探测本身又轻又快。
pub fn sniff(mirrors: &[Mirror], timeout: Duration) -> Vec<Probe> {
    let handles: Vec<_> = mirrors
        .iter()
        .map(|m| {
            let url = git_url(m, "rime/rime-prelude");
            std::thread::spawn(move || {
                let host = match host_of(&url) {
                    Some(h) => h,
                    None => return None,
                };
                let started = Instant::now();
                let addr = format!("{host}:443");
                let ok = addr
                    .to_socket_addrs()
                    .ok()
                    .map(|addrs| {
                        addrs.into_iter().any(|a| {
                            std::net::TcpStream::connect_timeout(&a, timeout).is_ok()
                        })
                    })
                    .unwrap_or(false);
                Some(if ok {
                    started.elapsed()
                } else {
                    return None;
                })
            })
        })
        .collect();

    let mut out = Vec::with_capacity(mirrors.len());
    for (mirror, handle) in mirrors.iter().zip(handles) {
        let latency = handle.join().ok().flatten();
        out.push(Probe {
            mirror: mirror.clone(),
            latency,
        });
    }
    out.sort_by_key(|p| (p.latency.is_none(), p.latency.unwrap_or(Duration::MAX)));
    out
}

/// 镜像调度器：持有一份按优先级排好的镜像列表，逐个尝试、失败即切换。
pub struct Picker {
    order: Vec<Mirror>,
    index: usize,
    /// 上次实际成功用到的镜像（用于回写状态文件）。
    chosen: Option<Mirror>,
}

impl Picker {
    /// 按嗅探结果排序构造；`preferred` 若给出会被提到队首（快路径）。
    pub fn new(mut order: Vec<Mirror>, preferred: Option<&str>) -> Self {
        if let Some(id) = preferred {
            if let Some(pos) = order.iter().position(|m| m.id == id) {
                let m = order.remove(pos);
                order.insert(0, m);
            }
        }
        Self {
            order,
            index: 0,
            chosen: None,
        }
    }

    /// 当前候选（已耗尽则返回 `None`）。
    pub fn current(&self) -> Option<&Mirror> {
        self.order.get(self.index)
    }

    /// 可尝试的镜像数。
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// 依次尝试直到成功；全部失败则返回最后一次的错误。
    ///
    /// 每失败一次就前进到下一个镜像（"自动切换"），成功时记下用的是哪个。
    pub fn attempt<T, F>(&mut self, mut op: F) -> Result<(T, Mirror), String>
    where
        F: FnMut(&Mirror) -> Result<T, String>,
    {
        let mut last_err = String::from("没有可用镜像");
        while let Some(mirror) = self.order.get(self.index).cloned() {
            match op(&mirror) {
                Ok(value) => {
                    self.chosen = Some(mirror.clone());
                    return Ok((value, mirror));
                }
                Err(e) => {
                    last_err = format!("[{}] {}", mirror.id, e);
                    self.index += 1;
                }
            }
        }
        Err(last_err)
    }

    /// 本次实际成功的镜像（用于回写状态文件）。
    pub fn chosen(&self) -> Option<&Mirror> {
        self.chosen.as_ref()
    }
}

/// 状态文件：记住上次成功的镜像 id。
pub fn state_path(plum_dir: &Path) -> PathBuf {
    plum_dir.join("mirror.txt")
}

/// 读上次成功的镜像 id（文件不存在/损坏则 `None`）。
pub fn load_state(plum_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(state_path(plum_dir)).ok()?;
    let id = text.lines().next().unwrap_or("").trim().to_string();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

/// 写回上次成功的镜像 id。
pub fn save_state(plum_dir: &Path, mirror_id: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(plum_dir)?;
    std::fs::write(state_path(plum_dir), format!("{mirror_id}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_built_from_prefixes() {
        let m = by_id("github").unwrap();
        assert_eq!(git_url(&m, "rime/rime-essay"), "https://github.com/rime/rime-essay.git");
        assert_eq!(
            raw_url(&m, "rime/plum/master/preset-packages.conf"),
            "https://raw.githubusercontent.com/rime/plum/master/preset-packages.conf"
        );
    }

    #[test]
    fn host_is_extracted_from_mirror_url() {
        assert_eq!(host_of("https://github.com/a.git").as_deref(), Some("github.com"));
        assert_eq!(
            host_of("https://ghproxy.com/https://github.com/a.git").as_deref(),
            Some("ghproxy.com")
        );
    }

    #[test]
    fn derived_raw_replaces_github_with_raw_githubusercontent() {
        assert_eq!(
            derive_raw("https://gh.example.com/https://github.com/"),
            "https://gh.example.com/https://raw.githubusercontent.com/"
        );
        // 推不出 github 前缀时原样返回
        assert_eq!(derive_raw("https://git.example.com/"), "https://git.example.com/");
    }

    /// 自定义镜像文本解析：注释跳过、缺 raw 自动推导、坏行跳过。
    #[test]
    fn parse_user_mirrors_handles_comments_and_bad_lines() {
        let text = "\
# 这是注释
mymirror|https://gh.example.com/https://github.com/
bad-no-prefix|
|https://a.b/
explicit|https://a.b/|https://c.d/
";
        let mirrors = parse_user_mirrors(text);
        assert_eq!(mirrors.len(), 2);
        assert_eq!(mirrors[0].id, "mymirror");
        assert_eq!(
            mirrors[0].raw,
            "https://gh.example.com/https://raw.githubusercontent.com/"
        );
        assert_eq!(mirrors[1].id, "explicit");
        assert_eq!(mirrors[1].raw, "https://c.d/");
    }

    /// `add_user_mirror` 先清同 id 旧行再追加，重复添加不产生重复条目。
    #[test]
    fn add_user_mirror_replaces_same_id() {
        let dir = std::env::temp_dir().join(format!("plum-mirror-add-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        add_user_mirror(&dir, "m1", "https://a.b/https://github.com/", None).unwrap();
        add_user_mirror(&dir, "m2", "https://c.d/https://github.com/", Some("https://e.f/")).unwrap();
        // 同 id 重写
        add_user_mirror(&dir, "m1", "https://g.h/https://github.com/", None).unwrap();
        let mirrors = load_user_mirrors(&dir);
        assert_eq!(mirrors.len(), 2);
        assert_eq!(mirrors[0].id, "m2");
        assert_eq!(mirrors[1].id, "m1");
        assert_eq!(mirrors[1].git, "https://g.h/https://github.com/");
        assert_eq!(mirrors[0].raw, "https://e.f/");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 内置 + 自定义合并：同 id 覆盖、新 id 追加，不丢内置镜像。
    #[test]
    fn all_mirrors_merges_custom_over_builtin() {
        let dir = std::env::temp_dir().join(format!("plum-mirror-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        add_user_mirror(&dir, "ghfast", "https://my.fast/https://github.com/", None).unwrap();
        add_user_mirror(&dir, "custom", "https://a.b/https://github.com/", None).unwrap();
        let all = all_mirrors(&dir);
        assert_eq!(all.len(), builtin().len() + 1);
        // 内置 ghfast 被自定义覆盖
        let fast = all.iter().find(|m| m.id == "ghfast").unwrap();
        assert_eq!(fast.git, "https://my.fast/https://github.com/");
        // 自定义新 id 在列表里
        assert!(all.iter().any(|m| m.id == "custom"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preferred_mirror_is_moved_to_front() {
        let order = builtin();
        let picker = Picker::new(order.clone(), Some("gitclone"));
        assert_eq!(picker.current().unwrap().id, "gitclone");
        // 队首之后仍是完整列表，不能丢镜像
        assert_eq!(picker.len(), order.len());
    }

    /// 自动切换的核心行为：前一个失败就换下一个，成功则记住用的是谁。
    #[test]
    fn picker_switches_on_failure_and_remembers_success() {
        let order = builtin();
        let mut picker = Picker::new(order, None);
        let tried: Vec<String> = Vec::new();
        let (value, used) = picker
            .attempt(|m| {
                // 前两个镜像假装失败
                if m.id == "github" || m.id == "ghproxy" {
                    Err("连接超时".to_string())
                } else {
                    Ok(m.id.to_string())
                }
            })
            .unwrap();
        assert_eq!(value, "gh-proxy");
        assert_eq!(used.id, "gh-proxy");
        assert_eq!(picker.chosen().unwrap().id, "gh-proxy");
        assert!(tried.is_empty());
    }

    /// 全部失败时返回最后一个错误，且错误里带上镜像 id，便于定位。
    #[test]
    fn picker_reports_last_error_when_all_fail() {
        let mut picker = Picker::new(builtin(), None);
        let err = picker.attempt(|_| Err::<(), _>("boom".to_string())).unwrap_err();
        assert!(err.contains("boom"), "{err}");
        assert!(err.contains('['), "{err}");
    }

    /// 状态文件往返。
    #[test]
    fn state_roundtrip() {
        let dir = std::env::temp_dir().join(format!("plum-next-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_state(&dir).is_none());
        save_state(&dir, "ghfast").unwrap();
        assert_eq!(load_state(&dir).as_deref(), Some("ghfast"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
