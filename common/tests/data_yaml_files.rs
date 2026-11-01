//! 出厂数据文件（YAML）的**冒烟测试**：用我们自己的解析器实读仓库里随包分发的
//! 那几份 YAML，确保格式没被改坏、也不会悄悄退化成别的格式。
//!
//! 2026-10-27：全仓库退出 TOML 时，这几份文件都是从 TOML 转来的（细胞词库 8.1 MB、
//! 主题 1000 行、双拼键位表）。转换脚本只跑一次、之后没人再看，所以得有测试盯着：
//! 一旦有人改坏了格式，部署出去的就是一份解析失败的产物。

use std::path::PathBuf;

fn data_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("Rime-Next")
}

/// 细胞词库：442,723 条，其中 66,525 条频次为 0（按设计跳过），有效 **376,198** 条。
/// （2026-10-27 逐条比对过旧 TOML 与现 YAML：键集合一致、值 0 处不同。）
///
/// 这里只钉"量级"与"能解析"，不钉精确值 —— 词库内容会随上游更新变动，
/// 精确值一变就红是噪音；但少一半（说明解析中途断了）必须红。
#[test]
fn default_eassy_yaml_parses() {
    let path = data_root().join("eassy").join("default").join("default.eassy.yaml");
    assert!(path.is_file(), "细胞词库缺失：{}", path.display());

    let vocabulary = weasel_common::eassy::read_file(&path).expect("细胞词库必须能解析");
    assert!(
        vocabulary.len() > 370_000,
        "细胞词库条目数异常：{}（应 > 370,000）",
        vocabulary.len()
    );
    // 抽样：常见字必须存在，否则说明键的引号/转义处理退化
    for word in ["的", "一", "我"] {
        assert!(vocabulary.contains_key(word), "缺少常用词「{word}」");
    }
}

/// 出厂主题（由官方皮肤 `weasel.official.yaml` 经 `theme-yaml` 生成）。
#[test]
fn default_theme_yaml_parses() {
    let path = data_root().join("theme").join("default").join("default.theme.yaml");
    assert!(path.is_file(), "出厂主题缺失：{}", path.display());

    let text = std::fs::read_to_string(&path).unwrap();
    let value = weasel_common::yaml::to_value(&text).expect("出厂主题必须能解析");
    let root = value.as_object().expect("根是映射");
    // `theme-yaml` 生成的是**我们约定的形状**：`color_scheme` + `schemes`（不是官方
    // 皮肤那份的 `preset_color_schemes`）。配色数 36 套以上才对得起"出厂皮肤"。
    assert!(root.contains_key("color_scheme"), "缺 color_scheme：{:?}", root.keys());
    let schemes = root
        .get("schemes")
        .and_then(|v| v.as_object())
        .unwrap_or_else(|| panic!("缺 schemes 配色表：{:?}", root.keys()));
    assert!(schemes.len() >= 30, "配色数异常：{}", schemes.len());
    assert!(schemes.contains_key("aqua"), "应有 aqua（碧水）配色");
}

/// 双拼键位表：源在 `parser/shuangpin/*.parser.yaml`（宿主与 `shuangpin-schema` 都读它）。
/// 音节表在 `parser/pinyin/syllables.parser.yaml`。
#[test]
fn shuangpin_tables_parse() {
    let dir = data_root().join("parser").join("shuangpin");
    let mut checked = 0;
    for name in ["xiaohe", "ziranma"] {
        let path = dir.join(format!("{name}.parser.yaml"));
        assert!(path.is_file(), "双拼表缺失：{}", path.display());
        let text = std::fs::read_to_string(&path).unwrap();
        let value = weasel_common::yaml::to_value(&text).unwrap_or_else(|e| panic!("{name} 解析失败：{e}"));
        assert!(!value.as_object().unwrap().is_empty(), "{name} 内容为空");
        checked += 1;
    }
    assert_eq!(checked, 2);

    // 音节表 YAML 也要能解析、且量级正确
    let syll_path = data_root().join("parser").join("pinyin").join("syllables.parser.yaml");
    assert!(syll_path.is_file(), "音节表缺失：{}", syll_path.display());
    let syll_text = std::fs::read_to_string(&syll_path).unwrap();
    let syll_val = weasel_common::yaml::to_value(&syll_text).expect("音节表必须能解析");
    let syllables = syll_val.get("syllables").and_then(|v| v.as_array())
        .expect("缺 syllables 列表");
    assert!(syllables.len() >= 400, "音节数异常：{}", syllables.len());
}

/// 宿主出厂默认配置：现在住在 `Rime-Next/weasel.yaml`（2026-10-27 从仓库根搬进来）。
///
/// 只验「能被我们的 YAML 解析、且有那几个程序级键」，值本身由 Setting 的
/// `native_metadata_and_defaults` 用 richschema 校验。
#[test]
fn host_default_config_parses() {
    let path = data_root().join("weasel.yaml");
    assert!(path.is_file(), "宿主出厂默认缺失：{}", path.display());

    let text = std::fs::read_to_string(&path).unwrap();
    let value = weasel_common::yaml::to_value(&text).expect("宿主出厂默认必须能解析");
    let root = value.as_object().unwrap();
    for key in ["theme", "ascii_mode", "sound", "app_options"] {
        assert!(root.contains_key(key), "缺少程序级键 `{key}`");
    }
    // 官方原版皮肤改名后必须与它分开存放，别再被当成宿主配置
    let official = data_root().join("weasel.official.yaml");
    assert!(official.is_file(), "官方皮肤缺失：{}", official.display());
    let official_text = std::fs::read_to_string(&official).unwrap();
    let official_value = weasel_common::yaml::to_value(&official_text).expect("官方皮肤必须能解析");
    assert!(
        official_value.as_object().unwrap().contains_key("preset_color_schemes"),
        "官方皮肤应带配色表"
    );
}
