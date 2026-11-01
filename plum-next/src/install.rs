//! 文件挑选与安装 —— 对照 `scripts/install-packages.sh`。
//!
//! 两条规则与原版一致：
//! * **挑什么**：`*.yaml`（排除 `*.custom.yaml` / `*.recipe.yaml` / `recipe.yaml`）、
//!   `*.txt`、`*.gram`、`opencc/` 下的 `.json` / `.ocd` / `.txt`
//!   （PlumNext 额外收 `.ocd2`，因为 Weasel-Next 用的是 OpenCC 2.0 二进制）；
//! * **怎么装**：目标已存在且内容相同就**跳过**（原版 `diff -q`），只统计真正变化的文件数。

use std::path::{Path, PathBuf};

/// 按原版规则列出包里的"数据文件"（相对包目录的路径，按名字排序）。
pub fn data_files(package_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect_yaml(package_dir, &mut out);
    collect_by_ext(package_dir, &mut out, &["txt", "gram"]);
    collect_opencc(package_dir, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_yaml(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".yaml") {
            continue;
        }
        if name.ends_with(".custom.yaml") || name.ends_with(".recipe.yaml") || name == "recipe.yaml" {
            continue;
        }
        out.push(name);
    }
}

fn collect_by_ext(dir: &Path, out: &mut Vec<String>, exts: &[&str]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if exts.iter().any(|e| name.ends_with(&format!(".{e}"))) {
            out.push(name);
        }
    }
}

fn collect_opencc(dir: &Path, out: &mut Vec<String>) {
    let opencc = dir.join("opencc");
    let Ok(entries) = std::fs::read_dir(opencc) else {
        return;
    };
    let keep = ["json", "ocd", "ocd2", "txt"];
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if keep.iter().any(|e| name.ends_with(&format!(".{e}"))) {
            out.push(format!("opencc/{name}"));
        }
    }
}

/// 按 `install_files` 的模式从包目录里挑文件（支持 `*`）。
pub fn pick(package_dir: &Path, patterns: &[String]) -> Vec<String> {
    let all = data_files(package_dir);
    let mut out: Vec<String> = Vec::new();
    for pattern in patterns {
        for name in &all {
            if crate::recipe::matches(pattern, name) && !out.contains(name) {
                out.push(name.clone());
            }
        }
    }
    out
}

/// 安装一批文件；返回**实际写入**的文件数（内容相同的会被跳过）。
pub fn install(package_dir: &Path, output_dir: &Path, files: &[String]) -> Result<usize, String> {
    let mut updated = 0usize;
    for file in files {
        let source = package_dir.join(file);
        let target = output_dir.join(file);
        if !source.exists() {
            return Err(format!("包内缺少文件: {file}"));
        }
        let source_bytes = std::fs::read(&source).map_err(|e| e.to_string())?;
        if target.exists() && std::fs::read(&target).map(|b| b == source_bytes).unwrap_or(false) {
            continue; // 无变化，不覆盖（原版 `diff -q` 的效果）
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&target, &source_bytes).map_err(|e| e.to_string())?;
        updated += 1;
    }
    Ok(updated)
}

/// 下载 `download_files` 里的一条（`文件名::URL`），落到 `dest_dir`。
///
/// 返回 `(文件名, 是否真的写了)`；本地已有同名文件时不重复下载（原版行为）。
pub fn download_entry(entry: &str, dest_dir: &Path) -> Result<(String, bool), String> {
    let (name, url) = crate::recipe::split_download_entry(entry);
    let dest: PathBuf = dest_dir.join(&name);
    if dest.exists() {
        return Ok((name, false));
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("下载失败 {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败 {url}: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    Ok((name, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("plum-next-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn data_files_pick_the_right_ones() {
        let pkg = scratch("data");
        for name in [
            "essay.schema.yaml",
            "essay.dict.yaml",
            "essay.custom.yaml", // 排除
            "recipe.yaml",       // 排除
            "zz.recipe.yaml",    // 排除
            "essay.txt",
            "essay.gram",
            "README.md", // 排除
        ] {
            fs::write(pkg.join(name), "x").unwrap();
        }
        fs::create_dir_all(pkg.join("opencc")).unwrap();
        fs::write(pkg.join("opencc/s2t.json"), "x").unwrap();
        fs::write(pkg.join("opencc/STCharacters.ocd2"), "x").unwrap();
        fs::write(pkg.join("opencc/notes.md"), "x").unwrap(); // 排除

        let files = data_files(&pkg);
        assert!(files.contains(&"essay.schema.yaml".to_string()));
        assert!(files.contains(&"essay.dict.yaml".to_string()));
        assert!(files.contains(&"essay.txt".to_string()));
        assert!(files.contains(&"essay.gram".to_string()));
        assert!(files.contains(&"opencc/s2t.json".to_string()));
        assert!(files.contains(&"opencc/STCharacters.ocd2".to_string()));
        assert!(!files.iter().any(|f| f.ends_with(".custom.yaml")));
        assert!(!files.iter().any(|f| f.ends_with(".recipe.yaml")));
        assert!(!files.iter().any(|f| f == "recipe.yaml"));
        assert!(!files.iter().any(|f| f.ends_with(".md")));
    }

    #[test]
    fn install_skips_identical_files() {
        let pkg = scratch("pkg");
        let out = scratch("out");
        fs::write(pkg.join("a.yaml"), "hello").unwrap();
        fs::write(out.join("a.yaml"), "hello").unwrap(); // 相同
        fs::write(pkg.join("b.yaml"), "world").unwrap();

        let n = install(&pkg, &out, &["a.yaml".into(), "b.yaml".into()]).unwrap();
        assert_eq!(n, 1, "只有 b.yaml 应该被写入");
        assert_eq!(fs::read_to_string(out.join("b.yaml")).unwrap(), "world");
    }

    #[test]
    fn install_reports_missing_file() {
        let pkg = scratch("pkg2");
        let out = scratch("out2");
        assert!(install(&pkg, &out, &["nope.yaml".into()]).is_err());
    }

    #[test]
    fn pick_expands_patterns() {
        let pkg = scratch("pick");
        fs::write(pkg.join("emoji.schema.yaml"), "x").unwrap();
        fs::write(pkg.join("emoji.dict.yaml"), "x").unwrap();
        let got = pick(&pkg, &["*.dict.yaml".to_string()]);
        assert_eq!(got, vec!["emoji.dict.yaml".to_string()]);
    }
}
