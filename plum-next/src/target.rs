//! 目标解析 —— 对照 `scripts/resolver.sh`。
//!
//! 一个"配方订单"（Rx）的完整形式是：
//!
//! ```text
//! <github-user>/<repository-name>@<branch>:<recipe>:key=value,...
//! ```
//!
//! `<github-user>/`、`@<branch>`、`:<recipe>...` 都可以省略。
//! 另外支持原版的两类"配置目标"：`:preset`（读 `preset-packages.conf`）
//! 与 `<user>/<repo>@<branch>/xxx-packages.conf`（远端清单）。

use std::path::Path;

/// 解析出的一个配方订单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// GitHub 用户名，默认 `rime`。
    pub user: String,
    /// 仓库全名（`<user>/<repo>`）。
    pub package: String,
    /// 去掉 `rime-` 前缀后的包名（原版 `resolve_package_name`）。
    pub package_name: String,
    /// `@<branch>`，未指定为 `None`。
    pub branch: Option<String>,
    /// `:<recipe>`，未指定为 `None`。
    pub recipe: Option<String>,
    /// `:k=v,k=v` 里的选项串。
    pub options: Vec<String>,
}

impl Target {
    /// 原版展示用的 `Rx` 串：`<package>[:<recipe>]`。
    pub fn rx(&self) -> String {
        match &self.recipe {
            Some(r) => format!("{}:{}", self.package, r),
            None => self.package.clone(),
        }
    }
}

/// 解析目标串。空串返回 `None`。
pub fn parse_target(input: &str) -> Option<Target> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    // `<pkg>[@branch]` 部分：第一个 ':' 之前
    let (pkg_part, rest) = match s.find(':') {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let (user_opt, repo) = match pkg_part.find('/') {
        Some(i) => (Some(pkg_part[..i].to_string()), pkg_part[i + 1..].to_string()),
        None => (None, pkg_part.to_string()),
    };
    let branch = pkg_part
        .find('@')
        .map(|i| pkg_part[i + 1..].to_string())
        .filter(|b| !b.is_empty());
    // 仓库名去掉 `@branch`
    let repo = match repo.find('@') {
        Some(i) => repo[..i].to_string(),
        None => repo,
    };
    // 补成完整 `<user>/<repo>`：原版 `fetch-package.sh` 的规则 ——
    // 没写用户时，仓库名不带 `rime-` 前缀就补成 `rime/rime-<name>`。
    let (user, package) = match user_opt {
        Some(u) => (u.clone(), format!("{u}/{repo}")),
        None => {
            let full = if repo.starts_with("rime-") {
                format!("rime/{repo}")
            } else {
                format!("rime/rime-{repo}")
            };
            ("rime".to_string(), full)
        }
    };
    let package_name = repo.strip_prefix("rime-").unwrap_or(&repo).to_string();

    let mut recipe = None;
    let mut options = Vec::new();
    if let Some(rest) = rest {
        let (r, opt_part) = match rest.find(':') {
            Some(i) => (Some(&rest[..i]), Some(&rest[i + 1..])),
            None => (Some(rest), None),
        };
        recipe = r.filter(|x| !x.is_empty()).map(str::to_string);
        if let Some(o) = opt_part {
            options = o
                .split([',', ':'])
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .collect();
        }
    }
    Some(Target {
        user,
        package,
        package_name,
        branch,
        recipe,
        options,
    })
}

/// 判断一个目标是不是"配置目标"（`:preset` / `*.conf` / 远端 conf URL），
/// 是的话不应该当普通包名处理（原版 `load_package_list_from_target` 的分支顺序）。
pub fn is_configuration_target(input: &str) -> bool {
    let s = input.trim();
    s.starts_with(':')
        || s.ends_with(".conf")
        || s.contains("-packages.conf")
        || s.starts_with("https://")
}

/// 解析 `*-packages.conf`：原版是 `source` 进 bash 的，实质就是一个
/// `package_list=( ... )` 赋值。这里把括号里的 token 全部取出来（跨行、带注释）。
pub fn parse_package_conf(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_list = false;
    for raw_line in text.lines() {
        let line = match raw_line.find('#') {
            Some(i) => &raw_line[..i],
            None => raw_line,
        };
        let line = line.trim();
        if !in_list {
            // 形如 `package_list=(`、`package_list+=(`（上游 preset-packages.conf 用的就是 `+=`）
            if let Some(rest) = line.split_once('=') {
                let lhs = rest.0.trim().trim_end_matches('+');
                if lhs.ends_with("package_list") {
                    in_list = true;
                    let after = rest.1.trim().trim_start_matches('(');
                    push_tokens(after, &mut out, &mut in_list);
                }
            }
            continue;
        }
        push_tokens(line, &mut out, &mut in_list);
    }
    out
}

fn push_tokens(line: &str, out: &mut Vec<String>, in_list: &mut bool) {
    // 去掉行尾可能的 ')'
    let mut line = line;
    if let Some(close) = line.find(')') {
        *in_list = false;
        line = &line[..close];
    }
    for tok in line.split_whitespace() {
        let tok = tok.trim_matches(|c| c == '"' || c == '\'');
        if !tok.is_empty() {
            out.push(tok.to_string());
        }
    }
}

/// 原版 `preset-packages.conf` 的内容（网络不可用时用它兜底，
/// 内容取自 `rime/plum` README 的 Essentials + 常用方案）。
pub const FALLBACK_PRESET: &[&str] = &[
    "prelude",
    "essay",
    "luna-pinyin",
    "terra-pinyin",
    "bopomofo",
    "pinyin-simp",
    "double-pinyin",
    "combo-pinyin",
    "stenotype",
    "cantonese",
    "jyutping",
    "wugniu",
    "soutzoe",
    "middle-chinese",
    "stroke",
    "cangjie",
    "quick",
    "wubi",
    "array",
    "scj",
    "emoji",
    "ipa",
];

/// `:preset` / `:extra` / `:all` 对应的清单文件名（原版 `${target#:}-packages.conf`）。
pub fn configuration_file_name(target: &str) -> Option<String> {
    let name = target.trim().trim_start_matches(':');
    if name.is_empty() {
        None
    } else {
        Some(format!("{name}-packages.conf"))
    }
}

/// 展开 `expand_configuration_url`：`<user>/<repo>[@branch]/<file>-packages.conf`
/// → `https://github.com/<user>/<repo>/raw/<branch>/<file>-packages.conf`。
pub fn expand_configuration_url(input: &str) -> Option<String> {
    let s = input.trim();
    if s.starts_with("https://") {
        return Some(s.to_string());
    }
    let (head, file) = s.rsplit_once('/')?;
    if !file.ends_with("-packages.conf") {
        return None;
    }
    let (repo_part, branch) = match head.split_once('@') {
        Some((r, b)) => (r, b.to_string()),
        None => (head, "master".to_string()),
    };
    let (user, repo) = repo_part.split_once('/')?;
    Some(format!(
        "https://github.com/{user}/{repo}/raw/{branch}/{file}"
    ))
}

/// 本地清单文件是否存在于 plum 目录（原版 `source "${root_dir}/${target#:}-packages.conf"`）。
pub fn local_configuration_path(plum_dir: &Path, target: &str) -> Option<std::path::PathBuf> {
    let name = configuration_file_name(target)?;
    let p = plum_dir.join(name);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_package_defaults_to_rime_user() {
        let t = parse_target("luna-pinyin").unwrap();
        assert_eq!(t.user, "rime");
        assert_eq!(t.package, "rime/rime-luna-pinyin");
        assert_eq!(t.package_name, "luna-pinyin");
        assert_eq!(t.branch, None);
        assert_eq!(t.recipe, None);
        assert!(t.options.is_empty());
    }

    #[test]
    fn full_rx_order_is_parsed() {
        let t = parse_target("lotem/rime-zhung@master:my-recipe:a=1,b=2").unwrap();
        assert_eq!(t.user, "lotem");
        assert_eq!(t.package, "lotem/rime-zhung");
        assert_eq!(t.package_name, "zhung");
        assert_eq!(t.branch.as_deref(), Some("master"));
        assert_eq!(t.recipe.as_deref(), Some("my-recipe"));
        assert_eq!(t.options, vec!["a=1", "b=2"]);
        assert_eq!(t.rx(), "lotem/rime-zhung:my-recipe");
    }

    #[test]
    fn branch_without_recipe() {
        let t = parse_target("jyutping@master").unwrap();
        assert_eq!(t.package, "rime/rime-jyutping");
        assert_eq!(t.branch.as_deref(), Some("master"));
        assert_eq!(t.recipe, None);
    }

    #[test]
    fn recipe_without_options() {
        let t = parse_target("emoji:all").unwrap();
        assert_eq!(t.recipe.as_deref(), Some("all"));
        assert!(t.options.is_empty());
    }

    #[test]
    fn configuration_targets_are_recognized() {
        assert!(is_configuration_target(":preset"));
        assert!(is_configuration_target("lotem/rime-forge/lotem-packages.conf"));
        assert!(is_configuration_target(
            "https://github.com/lotem/rime-forge/raw/master/lotem-packages.conf"
        ));
        assert!(!is_configuration_target("luna-pinyin"));
    }

    #[test]
    fn package_conf_handles_append_assignment() {
        // 上游 preset-packages.conf 的真实写法是 `package_list+=(`
        let conf = "\
#!/usr/bin/env bash

package_list+=(
    bopomofo
    essay
    prelude
)
";
        assert_eq!(
            parse_package_conf(conf),
            vec!["bopomofo", "essay", "prelude"]
        );
    }

    #[test]
    fn package_conf_is_parsed_across_lines() {
        let conf = "\
# 预设配方
package_list=(
  prelude
  essay      # 八股文
  luna-pinyin
)
";
        assert_eq!(
            parse_package_conf(conf),
            vec!["prelude", "essay", "luna-pinyin"]
        );
    }

    #[test]
    fn configuration_url_is_expanded() {
        assert_eq!(
            expand_configuration_url("lotem/rime-forge/lotem-packages.conf").as_deref(),
            Some("https://github.com/lotem/rime-forge/raw/master/lotem-packages.conf")
        );
        assert_eq!(
            expand_configuration_url("lotem/rime-forge@dev/lotem-packages.conf").as_deref(),
            Some("https://github.com/lotem/rime-forge/raw/dev/lotem-packages.conf")
        );
    }
}
