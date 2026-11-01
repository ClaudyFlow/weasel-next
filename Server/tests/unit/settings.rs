use super::*;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

fn temporary_directory() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "Weasel-Next-Setting-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ))
}

fn apply_patch(base: &mut Value, patch: &Value) -> Result<(), String> {
    let directory = temporary_directory();
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("weasel.yaml");
    std::fs::write(&path, weasel_common::yaml::to_string_pretty(&patch)).unwrap();
    let result = overlay_file(base, &path);
    std::fs::remove_dir_all(directory).unwrap();
    result
}

#[test]
fn merged_size_limit_is_atomic() {
    let mut base = json!({"theme":"ten", "first":"x".repeat(600_000)});
    let original = base.clone();
    let patch = json!({"second":"y".repeat(600_000)});
    assert!(patch.to_string().len() < MAX_CONFIG_BYTES as usize);
    assert!(
        apply_patch(&mut base, &patch)
            .unwrap_err()
            .contains("RPC size")
    );
    assert_eq!(base, original);
}

#[test]
fn merges_objects_and_replaces_arrays() {
    let mut base = json!({"theme":"eleven", "nested":{"a":1,"b":2}, "list":[1,2]});
    apply_patch(
        &mut base,
        &json!({"theme":"ten","nested":{"b":3},"list":[4]}),
    )
    .unwrap();
    assert_eq!(
        base,
        json!({"theme":"ten","nested":{"a":1,"b":3},"list":[4]})
    );
}

#[test]
fn option_values_are_left_for_consumers_to_validate() {
    let mut base = json!({"theme":"ten", "inline_preedit":true});
    apply_patch(
        &mut base,
        &json!({
            "theme": "future-theme",
            "inline_preedit": "invalid",
            "app_options": {"cmd.exe": {"ascii_mode": "invalid"}}
        }),
    )
    .unwrap();
    assert_eq!(base["theme"], "future-theme");
    assert_eq!(base["inline_preedit"], "invalid");
    assert_eq!(base["app_options"]["cmd.exe"]["ascii_mode"], "invalid");
}

#[test]
fn empty_override_is_supported() {
    let mut base = json!({"theme":"ten"});
    apply_patch(&mut base, &json!({})).unwrap();
    assert_eq!(base["theme"], "ten");
}

#[test]
fn files_follow_precedence_and_missing_files_are_optional() {
    let directory = temporary_directory();
    std::fs::create_dir(&directory).unwrap();
    let paths = RuntimePaths {
        executable_directory: directory.clone(),
        user_data: directory.join("user-data"),
        logs: directory.join("logs"),
    };
    paths.ensure().unwrap();
    let mut warnings = Vec::new();
    // 没有任何配置文件时用内嵌默认值（出厂 `weasel.yaml` 里的 theme = "official"）
    assert_eq!(
        load(&paths, |w| warnings.push(w))
            .required::<String>(".theme")
            .unwrap(),
        "official"
    );
    // 程序文件夹那份是 `weasel.yaml`（2026-10-23 起宿主配置只有 YAML 一种格式）。
    std::fs::write(
        weasel_common::settings::program_config_path(&directory),
        b"theme: ten\n",
    )
    .unwrap();
    assert_eq!(
        load(&paths, |w| warnings.push(w))
            .required::<String>(".theme")
            .unwrap(),
        "ten"
    );
    let custom = weasel_common::settings::user_config_path(&paths.user_data);
    std::fs::create_dir_all(custom.parent().unwrap()).unwrap();
    std::fs::write(&custom, b"\xef\xbb\xbftheme: eleven\n").unwrap();
    assert_eq!(
        load(&paths, |w| warnings.push(w))
            .required::<String>(".theme")
            .unwrap(),
        "eleven"
    );
    assert!(warnings.is_empty());
    std::fs::write(&custom, b"invalid").unwrap();
    assert_eq!(
        load(&paths, |w| warnings.push(w))
            .required::<String>(".theme")
            .unwrap(),
        "ten"
    );
    assert_eq!(warnings.len(), 1);
    std::fs::write(&custom, vec![b' '; MAX_CONFIG_BYTES as usize + 1]).unwrap();
    assert!(overlay_file(&mut json!({"theme":"ten"}), &custom).is_err());
    std::fs::remove_dir_all(&directory).unwrap();
}

/// 验收①：`weasel.yaml` 里的值必须被读出来（程序文件夹 + 用户目录）。
#[test]
fn yaml_program_config_is_read() {
    let directory = temporary_directory();
    std::fs::create_dir_all(&directory).unwrap();
    let paths = RuntimePaths {
        executable_directory: directory.clone(),
        user_data: directory.join("user-data"),
        logs: directory.join("logs"),
    };
    paths.ensure().unwrap();
    std::fs::write(
        weasel_common::settings::program_config_path(&directory),
        b"theme: eleven\ninput_mode_indicator_duration_ms: 1234\nsound:\n  enabled: false\n",
    )
    .unwrap();
    let mut warnings = Vec::new();
    let settings = load(&paths, |w| warnings.push(w));
    assert_eq!(settings.required::<String>(".theme").unwrap(), "eleven");
    assert_eq!(
        settings
            .required::<u64>(".input_mode_indicator_duration_ms")
            .unwrap(),
        1234
    );
    assert_eq!(settings.required::<bool>(".sound.enabled").unwrap(), false);
    assert!(warnings.is_empty(), "{warnings:?}");
    std::fs::remove_dir_all(&directory).unwrap();
}

/// 验收②：程序文件夹里**只有**旧 `weasel.toml`（或更老的 `weasel.json`）时，
/// 读不到任何配置 —— 不报错、不崩，直接走内嵌出厂默认值；且不会生成/改名任何文件。
#[test]
fn legacy_toml_only_program_config_falls_back_to_defaults() {
    let directory = temporary_directory();
    std::fs::create_dir_all(&directory).unwrap();
    let paths = RuntimePaths {
        executable_directory: directory.clone(),
        user_data: directory.join("user-data"),
        logs: directory.join("logs"),
    };
    paths.ensure().unwrap();
    std::fs::write(
        directory.join("weasel.toml"),
        b"theme = \"ten\"\ninput_mode_indicator_duration_ms = 1234\n",
    )
    .unwrap();
    std::fs::write(directory.join("weasel.json"), br#"{"theme":"seven"}"#).unwrap();
    let mut warnings = Vec::new();
    let settings = load(&paths, |w| warnings.push(w));
    assert_eq!(
        settings.required::<String>(".theme").unwrap(),
        "official",
        "旧 TOML/JSON 不该被读：走内嵌出厂默认值"
    );
    assert_eq!(
        settings
            .required::<u64>(".input_mode_indicator_duration_ms")
            .unwrap(),
        800,
        "TOML 里的 1234 不该生效"
    );
    assert!(warnings.is_empty(), "读不到不是错误，不该有告警：{warnings:?}");
    // 不迁移、不生成、不改名：旧文件原样留着，也没有 weasel.yaml。
    assert!(!weasel_common::settings::program_config_path(&directory).exists());
    assert!(directory.join("weasel.toml").is_file());
    assert!(directory.join("weasel.json").is_file());
    // 用户自己写好 YAML 之后立即生效。
    std::fs::write(
        weasel_common::settings::program_config_path(&directory),
        b"theme: eleven
",
    )
    .unwrap();
    assert_eq!(
        load(&paths, |w| warnings.push(w))
            .required::<String>(".theme")
            .unwrap(),
        "eleven"
    );
    std::fs::remove_dir_all(&directory).unwrap();
}
