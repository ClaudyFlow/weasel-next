//! 方案文件的**语法回归保护**。
//!
//! 这些 `.schema.yaml` 是手写或生成器产出的文本，改坏了（缩进、引号、YAML 特殊
//! 字符）在部署时才炸，很难定位。这里用完整 YAML 解析器过一遍，钉住关键结构。

use std::path::{Path, PathBuf};

/// 仓库根（style 的上一级）。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("style 在仓库子目录里")
        .to_path_buf()
}

fn load(rel: &str) -> serde_json::Value {
    let path = repo_root().join(rel);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));
    serde_norway::from_str(&text).unwrap_or_else(|e| panic!("{} 不是合法 YAML: {e}", path.display()))
}

#[test]
fn stroke_schema_parses() {
    let doc = load("Rime-Next/schema/stroke/stroke.schema.yaml");
    assert_eq!(doc["schema"]["id"], "stroke");
    // 新写法：单复数由 `normalize_dialect` 归一化，源里写单数
    assert!(doc["switch"].is_array(), "switch 应是数组");
    assert!(doc["engine"]["processor"].is_array(), "processor 应是数组");
    assert_eq!(doc["translator"]["dict"], "stroke");
    assert!(doc["translator"]["dictionary"].is_null(), "词库键一律写单数 dict");
    // 形码：无变体、无语言模型
    assert!(doc["translator"]["simp"].is_null());
    assert!(doc["translator"]["trad"].is_null());
    assert!(doc["translator"]["model"].is_null());
}

#[test]
fn pinyin_schema_parses() {
    let doc = load("Rime-Next/schema/pinyin/luna/pinyin.luna.schema.yaml");
    assert_eq!(doc["schema"]["id"], "pinyin.luna");
    // 单本词库 + 双变体：dict/prism 提到 translator 顶层共享，变体只写 model
    assert_eq!(doc["translator"]["dict"], "pinyin.luna");
    // 棱镜名 = 方案 id（2026-10-28 定的规矩）；棱镜不能跟词库名混用，否则与双拼撞名
    assert_eq!(doc["translator"]["prism"], "pinyin.luna");
    assert_eq!(doc["translator"]["simp"]["model"], "zh-han.simp");
    assert_eq!(doc["translator"]["trad"]["model"], "zh-han.trad");
    // 正则已内联（不是 import_preset）
    assert!(doc["speller"]["algebra"].is_array());
    assert!(doc["translator"]["preedit_format"].is_array());
}

#[test]
fn shuangpin_schemas_parse() {
    for (rel, id) in [
        (
            "Rime-Next/schema/double_pinyin/flypy/double_pinyin.flypy.schema.yaml",
            "double_pinyin.flypy",
        ),
        (
            "Rime-Next/schema/double_pinyin/ziranma/double_pinyin.ziranma.schema.yaml",
            "double_pinyin.ziranma",
        ),
    ] {
        let doc = load(rel);
        assert_eq!(doc["schema"]["id"], id);
        assert!(doc["__include"].is_string(), "{rel} 应继承基础方案");
        // 单本词库 + 双变体：dict/prism 提到 translator 顶层共享（与 pinyin.luna 同本），
        // 变体只写各自不同的 model；棱镜是**本方案**的拼写产物，名字必须等于方案 id。
        assert_eq!(doc["translator"]["dict"], "pinyin.luna");
        assert_eq!(doc["translator"]["prism"], id, "{rel} 棱镜名应等于方案 id");
        assert!(doc["translator"]["simp"]["dict"].is_null(), "{rel} 变体不该再写 dict");
        assert!(doc["translator"]["trad"]["dict"].is_null(), "{rel} 变体不该再写 dict");
        assert_eq!(doc["translator"]["simp"]["model"], "zh-han.simp");
        assert_eq!(doc["translator"]["trad"]["model"], "zh-han.trad");
        assert!(doc["speller"]["algebra"].is_array());
        // 死键 `parser:` 已删
        assert!(doc["speller"]["parser"].is_null(), "{rel} 不该再有 parser 死键");
    }
}

/// 形码（五笔 / 仓颉）与注音也走新写法：**单数方言键** + `translator/dict` +
/// `...` 收尾。这些是手写文件，最容易改一半。
#[test]
fn shape_and_zhuyin_schemas_use_singular_dialect() {
    for rel in [
        "Rime-Next/schema/wubi/86/wubi.86.schema.yaml",
        "Rime-Next/schema/cangjie/5/cangjie.5.schema.yaml",
        "Rime-Next/schema/zhuyin/zhuyin.schema.yaml",
    ] {
        let path = repo_root().join(rel);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains('\r'), "{rel} 含 CR，应为纯 LF");
        assert!(text.trim_end().ends_with("..."), "{rel} 应以 `...` 收尾");
        let doc = load(rel);
        // 单数的 `dependency`（不是 `dependencies`）
        assert_eq!(doc["schema"]["dependencies"], serde_json::Value::Null, "{rel}");
        // 形码 / 注音都不做繁简双变体，也不挂语言模型（注音例外：见下）
        assert!(doc["translator"]["simp"].is_null(), "{rel} 不该有 simp 变体");
        assert!(doc["translator"]["trad"].is_null(), "{rel} 不该有 trad 变体");
    }

    // 五笔：反查指向**存在**的方案（原来写的 `luna_pinyin_simp` 数据里没有）
    let wubi = load("Rime-Next/schema/wubi/86/wubi.86.schema.yaml");
    assert_eq!(wubi["schema"]["dependency"][0], "pinyin.luna");
    assert_eq!(wubi["translator"]["dict"], "wubi.86");
    assert!(wubi["switch"].is_array());
    assert!(wubi["engine"]["processor"].is_array());
    assert!(wubi["engine"]["segmentor"].is_array());
    assert!(wubi["engine"]["translator"].is_array());
    assert_eq!(wubi["reverse_lookup"]["schema"], "pinyin.luna");

    // 仓颉：反查也一律走 `schema:`（词库/棱镜由目标方案自己决定）
    let rel = "Rime-Next/schema/cangjie/5/cangjie.5.schema.yaml";
    let doc = load(rel);
    assert_eq!(doc["translator"]["dict"], "cangjie.5");
    assert_eq!(doc["reverse_lookup"]["schema"], "pinyin.luna");
    assert!(doc["reverse_lookup"]["dictionary"].is_null(), "{rel} 不该再写 dictionary");
    assert!(doc["engine"]["filter"].is_array(), "{rel} filter 应是数组");

    // 注音：表音方案，挂繁体语言模型；反查笔画用单数 `schema:`
    let zhuyin = load("Rime-Next/schema/zhuyin/zhuyin.schema.yaml");
    assert_eq!(zhuyin["translator"]["dict"], "pinyin.terra");
    assert_eq!(zhuyin["translator"]["model"], "zh-han.trad");
    assert_eq!(zhuyin["reverse_lookup"]["schema"], "stroke");
    assert!(zhuyin["speller"]["parser"].is_null(), "parser 是死键，已删");
}

/// 方案文件一律 LF、以换行收尾（`# newline: lf` 是自己声明的）。
#[test]
fn schema_files_use_lf_and_end_with_newline() {
    for rel in [
        "Rime-Next/schema/stroke/stroke.schema.yaml",
        "Rime-Next/schema/pinyin/luna/pinyin.luna.schema.yaml",
        "Rime-Next/schema/double_pinyin/flypy/double_pinyin.flypy.schema.yaml",
        "Rime-Next/schema/double_pinyin/ziranma/double_pinyin.ziranma.schema.yaml",
    ] {
        let path = repo_root().join(rel);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains('\r'), "{rel} 含 CR，应为纯 LF");
        assert!(text.ends_with('\n'), "{rel} 结尾缺换行");
    }
}

/// **标准**（2026-10-28）：`schema/description` **必须**是块标量 `|`。
///
/// 全仓 44 份方案一起查，不只是我们手写的那几份 —— 生成器
/// （`shuangpin-schema` / `pinyin-schema`）也走同一套
/// `weasel_style::schema_standard`，所以这里挂了就说明生成器或手写文件破了标准。
#[test]
fn every_schema_description_is_block_scalar() {
    let root = repo_root().join("Rime-Next").join("schema");
    let checked = collect_schemas(&root);
    assert!(
        !checked.is_empty(),
        "{} 下没找到 .schema.yaml，路径不对？",
        root.display()
    );
    for path in &checked {
        let text = std::fs::read_to_string(path).unwrap();
        let rel = path
            .strip_prefix(&repo_root())
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if let Err(e) = weasel_style::schema_standard::check(&text) {
            panic!("{rel} 不合标准: {e}");
        }
        // 顺带确认解析后确实拿到了字符串（块没写歪）
        let doc: serde_json::Value = serde_norway::from_str(&text)
            .unwrap_or_else(|e| panic!("{rel} 不是合法 YAML: {e}"));
        assert!(
            doc["schema"]["description"].is_string(),
            "{rel} 的 schema/description 不是字符串"
        );
    }
}

/// 递归收集目录下所有 `*.schema.yaml`（按路径排序，失败信息稳定）。
fn collect_schemas(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".schema.yaml"))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}
