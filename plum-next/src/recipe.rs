//! 配方（`*.recipe.yaml`）解析与执行 —— 对照 `scripts/recipe.sh`。
//!
//! 原版是用 `sed` 从 YAML 里"抠"出三个区块再 eval，这里改成正经解析：
//!
//! | 区块 | 行为 |
//! |---|---|
//! | `recipe:` / `Rx:` | 声明校验（与文件名/订单里的 recipe 名比对） |
//! | `download_files:` | 下载外链文件到包目录（支持 `文件名::URL`） |
//! | `install_files:` | 按模式挑选文件安装到 Rime 用户目录（支持 `*` 通配） |
//! | `patch_files:` | 把一段 YAML 补丁写进目标文件的 `__patch:` 段 |

use std::path::Path;

/// 一个配方文件。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Recipe {
    /// `recipe:` 段里的 `Rx` 声明。
    pub rx: Option<String>,
    /// `recipe:` 段里 `args:` 声明的默认选项（`k=v`，用户传的会覆盖它）。
    pub args: Vec<String>,
    /// `download_files:` 条目（原样，可能是 `文件名::URL`）。
    pub download_files: Vec<String>,
    /// `install_files:` 模式。
    pub install_files: Vec<String>,
    /// `install_files:` 段**是否存在**（存在但为空 = 什么都不装，与原版一致）。
    pub has_install_files: bool,
    /// `patch_files:` 的 (目标文件, 补丁正文)。
    pub patch_files: Vec<(String, String)>,
}

/// 读取并解析配方文件。
pub fn load(path: &Path) -> Result<Recipe, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: serde_norway::Value =
        serde_norway::from_str(&text).map_err(|e| format!("配方 YAML 解析失败: {e}"))?;
    let install_files = strings_of(value.get("install_files"));
    Ok(Recipe {
        rx: value
            .get("recipe")
            .and_then(|r| r.get("Rx"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        args: value.get("recipe").map(strings_of_nested).unwrap_or_default(),
        download_files: strings_of(value.get("download_files")),
        has_install_files: value.get("install_files").is_some(),
        install_files,
        patch_files: mapping_of(value.get("patch_files")),
    })
}

/// 取字符串列表。原版是用 `sed` 抠区块再按空白切分的，所以**块里的注释行会被丢掉**；
/// 这里同样把纯注释行和空行去掉（上游 `rime-emoji` 的 `install_files` 就是折叠标量里
/// 全是注释，等价于"空"）。
fn strings_of(value: Option<&serde_norway::Value>) -> Vec<String> {
    let Some(v) = value else { return Vec::new() };
    match v {
        serde_norway::Value::Sequence(items) => items
            .iter()
            .filter_map(|i| i.as_str())
            .flat_map(|s| lines_of(s))
            .collect(),
        serde_norway::Value::String(s) => lines_of(s),
        _ => Vec::new(),
    }
}

fn lines_of(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// `recipe:` 段里的 `args:`（`k=v` 形式的默认值）。
fn strings_of_nested(value: &serde_norway::Value) -> Vec<String> {
    strings_of(value.get("args"))
}

fn mapping_of(value: Option<&serde_norway::Value>) -> Vec<(String, String)> {
    let Some(serde_norway::Value::Mapping(map)) = value else {
        return Vec::new();
    };
    map.iter()
        .filter_map(|(k, v)| {
            let key = k.as_str()?.to_string();
            // 值通常是块标量（字符串），但也可能是序列/映射（上游 emoji 配方就是
            // `- patch/+:` 这种列表）—— 后者序列化回 YAML 文本再写进补丁。
            let value = match v.as_str() {
                Some(s) => s.to_string(),
                None => serde_norway::to_string(v).ok()?.trim_end().to_string(),
            };
            Some((key, value))
        })
        .collect()
}

/// 拆分 `download_files` 的一条：`文件名::URL`，没写 `::` 就用 URL 的最后一段。
pub fn split_download_entry(entry: &str) -> (String, String) {
    match entry.split_once("::") {
        Some((name, url)) => (name.to_string(), url.to_string()),
        None => (
            entry.rsplit('/').next().unwrap_or(entry).to_string(),
            entry.to_string(),
        ),
    }
}

/// `install_files` 用的通配匹配：只支持 `*`（原版是直接丢给 shell 展开的）。
pub fn matches(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*').peekable();
    let mut rest = name;
    if let Some(first) = parts.peek() {
        // 首段必须出现在开头
        if !rest.starts_with(first) {
            return false;
        }
        rest = &rest[first.len()..];
        parts.next();
    }
    for part in parts {
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
}

/// 变量替换：`${name}` 与 `${name:-默认值}`（原版是 `eval` 掉 `recipe_options`）。
///
/// `vars` 是 `(名字, 值)` 列表，通常 = 配方 `args:` 的默认值 + 命令行传进来的选项。
pub fn substitute(text: &str, vars: &[(String, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == '$' && i + 1 < bytes.len() && bytes[i + 1] == '{' {
            // 找匹配的 '}'
            if let Some(end) = bytes[i + 2..].iter().position(|c| *c == '}') {
                let body: String = bytes[i + 2..i + 2 + end].iter().collect();
                let (name, fallback) = match body.split_once(":-") {
                    Some((n, f)) => (n.trim(), Some(f.trim())),
                    None => (body.trim(), None),
                };
                let replaced = vars
                    .iter()
                    .rev()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.as_str())
                    .or(fallback)
                    .unwrap_or("");
                out.push_str(replaced);
                i += 2 + end + 1;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// 合并配方 `args:` 的默认值与命令行选项（命令行优先）。
pub fn merge_options(args: &[String], options: &[String]) -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = Vec::new();
    for entry in args.iter().chain(options.iter()) {
        if let Some((k, v)) = entry.split_once('=') {
            let k = k.trim().to_string();
            let v = v.trim().to_string();
            if let Some(slot) = vars.iter_mut().find(|(name, _)| *name == k) {
                slot.1 = v;
            } else {
                vars.push((k, v));
            }
        }
    }
    vars
}

/// 把补丁写进目标文件的 `__patch:` 段（幂等：同 Rx 的旧块先删）。
pub fn apply_patch(output_dir: &Path, file_name: &str, rx: &str, contents: &str) -> std::io::Result<()> {
    let target = output_dir.join(file_name);
    let original = if target.exists() {
        std::fs::read_to_string(&target)?
    } else {
        String::new()
    };

    // 1) 去掉同 Rx 的旧块（`# Rx: {rx} {` … `# }`）
    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    let mut cleaned: Vec<String> = Vec::with_capacity(lines.len());
    let mut skipping = false;
    for line in lines.drain(..) {
        if skipping {
            if line.trim_end() == "# }" {
                skipping = false;
            }
            continue;
        }
        if line.trim_end() == format!("# Rx: {rx} {{") {
            skipping = true;
            continue;
        }
        cleaned.push(line);
    }

    // 2) 保证有 `__patch:` 段
    if !cleaned.iter().any(|l| l.trim_end() == "__patch:") {
        if let Some(last) = cleaned.last() {
            if !last.is_empty() {
                cleaned.push(String::new());
            }
        }
        cleaned.push("__patch:".to_string());
    }

    // 3) 新块插到 `__patch:` 之后（缩进两格，符合 YAML 段内缩进）
    let pos = cleaned
        .iter()
        .position(|l| l.trim_end() == "__patch:")
        .unwrap_or(cleaned.len())
        + 1;
    let mut block: Vec<String> = vec![format!("# Rx: {rx} {{")];
    for l in contents.lines() {
        block.push(format!("  {l}"));
    }
    block.push("# }".to_string());
    cleaned.splice(pos..pos, block);

    let mut out = cleaned.join("\n");
    out.push('\n');
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
recipe:
  Rx: emoji:all
  description: 繪文字

download_files:
  - emoji.dict.yaml::https://example.com/emoji.txt

install_files:
  - emoji.schema.yaml
  - '*.dict.yaml'

patch_files:
  luna_pinyin.schema.yaml: |
    switches/+:
      - name: emoji_suggestion
"#;

    #[test]
    fn recipe_is_parsed() {
        let dir = std::env::temp_dir().join(format!("plum-next-recipe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("emoji.recipe.yaml");
        std::fs::write(&p, SAMPLE).unwrap();
        let r = load(&p).unwrap();
        assert_eq!(r.rx.as_deref(), Some("emoji:all"));
        assert_eq!(r.download_files, vec!["emoji.dict.yaml::https://example.com/emoji.txt"]);
        assert_eq!(r.install_files, vec!["emoji.schema.yaml", "*.dict.yaml"]);
        assert_eq!(r.patch_files.len(), 1);
        assert_eq!(r.patch_files[0].0, "luna_pinyin.schema.yaml");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_entry_splits_filename() {
        assert_eq!(
            split_download_entry("emoji.dict.yaml::https://x/y.txt"),
            ("emoji.dict.yaml".to_string(), "https://x/y.txt".to_string())
        );
        assert_eq!(
            split_download_entry("https://x/y/essay.txt"),
            ("essay.txt".to_string(), "https://x/y/essay.txt".to_string())
        );
    }

    #[test]
    fn glob_matches() {
        assert!(matches("*.dict.yaml", "emoji.dict.yaml"));
        assert!(!matches("*.dict.yaml", "emoji.schema.yaml"));
        assert!(matches("emoji.schema.yaml", "emoji.schema.yaml"));
        assert!(matches("*", "anything"));
        assert!(matches("lz*.yaml", "lzrl.schema.yaml"));
    }

    /// 幂等：连打两次补丁，块不会被重复追加。
    #[test]
    fn patch_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("plum-next-patch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let body = "switches/+:\n  - name: x\n";
        apply_patch(&dir, "a.schema.yaml", "emoji:all", body).unwrap();
        apply_patch(&dir, "a.schema.yaml", "emoji:all", body).unwrap();
        let text = std::fs::read_to_string(dir.join("a.schema.yaml")).unwrap();
        assert_eq!(text.matches("# Rx: emoji:all {").count(), 1, "{text}");
        assert!(text.contains("__patch:"), "{text}");
        assert!(text.contains("  switches/+:"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 不同 Rx 的补丁应当共存。
    #[test]
    fn different_rx_patches_coexist() {
        let dir = std::env::temp_dir().join(format!("plum-next-patch2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        apply_patch(&dir, "a.schema.yaml", "emoji:all", "x: 1\n").unwrap();
        apply_patch(&dir, "a.schema.yaml", "ipa:xsampa", "y: 2\n").unwrap();
        let text = std::fs::read_to_string(dir.join("a.schema.yaml")).unwrap();
        assert!(text.contains("# Rx: emoji:all {"));
        assert!(text.contains("# Rx: ipa:xsampa {"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    /// 上游 `rime-emoji/customize.recipe.yaml` 的真实形态：
    /// `install_files` 是折叠标量、内容全是注释 —— 等价于"段存在但为空 ⇒ 什么都不装"，
    /// 且 `recipe.args` 声明了 `schema=luna_pinyin` 默认值。
    #[test]
    fn emoji_customize_recipe_is_parsed_like_upstream() {
        let text = "\
# encoding: utf-8
---
recipe:
  Rx: customize
  args:
    - schema=luna_pinyin
  description: >-
    Customize input schema to show emoji suggestion with OpenCC

install_files: >-
  # emoji_suggestion.yaml
  # opencc/*.*

patch_files:
  ${schema:-luna_pinyin}.custom.yaml:
    - patch/+:
        __include: emoji_suggestion:/patch
";
        let dir = std::env::temp_dir().join(format!("plum-next-emoji-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("customize.recipe.yaml");
        std::fs::write(&path, text).unwrap();

        let r = load(&path).unwrap();
        assert_eq!(r.rx.as_deref(), Some("customize"));
        assert_eq!(r.args, vec!["schema=luna_pinyin".to_string()]);
        // 段存在 ⇒ 不该回退到"包里所有数据文件"
        assert!(r.has_install_files, "install_files 段必须被识别为存在");
        assert!(r.install_files.is_empty(), "全注释 ⇒ 空清单");
        assert_eq!(r.patch_files.len(), 1);
        assert_eq!(r.patch_files[0].0, "${schema:-luna_pinyin}.custom.yaml");

        // 变量替换后的目标文件名
        let vars = merge_options(&r.args, &[]);
        assert_eq!(
            substitute(&r.patch_files[0].0, &vars),
            "luna_pinyin.custom.yaml"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn substitute_simple_and_default() {
        let vars = vec![("schema".to_string(), "luna_pinyin".to_string())];
        assert_eq!(
            substitute("${schema}.custom.yaml", &vars),
            "luna_pinyin.custom.yaml"
        );
        // 有值就用值，不用默认值
        assert_eq!(substitute("${schema:-xyz}.yaml", &vars), "luna_pinyin.yaml");
        // 没给值就用默认值
        assert_eq!(substitute("${other:-abc}.yaml", &vars), "abc.yaml");
        // 既没值也没默认值 → 空
        assert_eq!(substitute("${missing}.yaml", &vars), ".yaml");
    }

    #[test]
    fn merge_options_cli_wins() {
        let args = vec!["schema=luna_pinyin".to_string()];
        let opts = vec!["schema=wubi".to_string(), "extra=1".to_string()];
        let vars = merge_options(&args, &opts);
        assert_eq!(vars.len(), 2);
        assert!(vars.contains(&("schema".to_string(), "wubi".to_string())));
        assert!(vars.contains(&("extra".to_string(), "1".to_string())));
    }
}
