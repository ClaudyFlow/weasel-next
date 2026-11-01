//! 配置快照的本地解析、查询和默认值合并。
//!
//! [`ConfigSnapshot`] 持有不可变 JSON 配置，查询结果借用快照中的值；合并主题默认值
//! 会创建新快照，不修改原快照。配置从何处取得由各个消费组件决定。
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;

/// 用户配置（设置界面写的补丁）的路径：`<用户目录>/weasel.yaml`。
///
/// 2026-10-21 用户要求：**去掉 `custom` 后缀、改名 `weasel.yaml`、直接放在用户目录根**
/// （`.weasel.lock` 也留根目录），内容也真的换成 YAML —— `style::parse_file` 本来就只认 YAML，
/// 旧的 TOML 版补丁在皮肤页其实一直解析失败、静默降级成 `None`。
pub fn user_config_path(user_data: &std::path::Path) -> std::path::PathBuf {
    user_data.join("weasel.yaml")
}

/// 2026-10-21 的**中途落点**（迁移用）：`<用户目录>/weasel/weasel.yaml`。
///
/// 用户当天先要求「收进 `weasel\` 子目录」，随后改成「放在用户目录根」；这一份就是那段中间态，
/// 迁移时要能认出来并搬回根部，否则用户会发现设置全丢了。
pub fn interim_user_config_path(user_data: &std::path::Path) -> std::path::PathBuf {
    user_data.join("weasel").join("weasel.yaml")
}

/// 一次性搬家：`<用户目录>\weasel\weasel.yaml`（2026-10-21 的中途落点，**本来就是 YAML**）
/// → `<用户目录>\weasel.yaml`，搬完把空掉的 `weasel\` 子目录删掉。
///
/// 只在「新文件不存在」时做；返回 `Ok(true)` 表示这次真的搬了，调用方可以据此打日志。
///
/// ⚠️ 这不是 TOML→YAML 的格式迁移：**TOML 配置一概不读**（`weasel.custom.toml`
/// 这类旧补丁由用户自己改成 YAML）。这里只搬同一份 YAML 文件的位置。
pub fn migrate_interim_user_config(user_data: &std::path::Path) -> Result<bool, String> {
    let new_path = user_config_path(user_data);
    if new_path.exists() {
        return Ok(false);
    }
    let interim = interim_user_config_path(user_data);
    if !interim.is_file() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(&interim)
        .map_err(|error| format!("{}: {error}", interim.display()))?;
    std::fs::write(&new_path, text).map_err(|error| format!("{}: {error}", new_path.display()))?;
    if let Some(dir) = interim.parent() {
        let _ = std::fs::remove_file(&interim);
        // 子目录空了才删；里面还有别的东西就留着，别替用户决定。
        if std::fs::read_dir(dir).is_ok_and(|mut rest| rest.next().is_none()) {
            let _ = std::fs::remove_dir(dir);
        }
    }
    Ok(true)
}

/// 程序文件夹里宿主配置的**唯一文件名**（2026-10-23 起唯一格式）：`<程序文件夹>/weasel.yaml`。
///
/// # 为什么强制 YAML
///
/// 1. `weasel_style::parse_file`（`style/src/lib.rs`，皮肤/配色读取的唯一入口）**只认 YAML**；
///    它接手的是官方小狼毫那份 `weasel.yaml` 形状的数据，根本不解析 TOML。
/// 2. 配置树与写盘序列化统一在 [`crate::yaml`]（`serde_json::Value` ⇄ YAML 文本）：设置界面
///    早就在往用户目录写 `weasel.yaml`。程序文件夹那份若还是 TOML，同一个配置就有两种格式、
///    两套解析分支，"读到哪份取决于扩展名"的坑会一直存在。
/// 3. 读与写必须同一种格式，否则"改文件即生效"永远说不清该改哪份。
///
/// 因此 TOML/JSON **既不被读也不被写**，也没有 TOML→YAML 的迁移层：旧文件里没有
/// `weasel.yaml` 时读不到配置，宿主就用手上的内嵌出厂默认值（不报错、不崩）。
pub const PROGRAM_CONFIG_FILE: &str = "weasel.yaml";

/// 程序文件夹宿主配置的正式路径：`<程序文件夹>/weasel.yaml`。
pub fn program_config_path(executable_directory: &std::path::Path) -> std::path::PathBuf {
    executable_directory.join(PROGRAM_CONFIG_FILE)
}

/// 解析一份宿主配置文本（**只有 YAML**），并要求根是对象。
///
/// 两处读取点（`Server/src/settings.rs`、`Setting/src/config.rs`）共用它，
/// 保证"什么算合法宿主配置"只有一个定义。
pub fn parse_config_text(text: &str) -> Result<Value, String> {
    let value: Value = crate::yaml::to_value(text)?;
    if !value.is_object() {
        return Err("配置必须是对象".into());
    }
    Ok(value)
}

/// 可廉价克隆的不可变配置快照。
///
/// 内部 JSON 通过引用计数共享；查询不会修改配置。调用
/// [`with_theme_defaults`](Self::with_theme_defaults) 会返回独立快照，并保留原快照不变。
#[derive(Clone, Debug)]
pub struct ConfigSnapshot(Arc<Value>);

impl ConfigSnapshot {
    /// 将 JSON 值封装为共享的只读快照。
    pub fn new(value: Value) -> Self {
        Self(Arc::new(value))
    }
    /// 解析 JSON 根对象并构造配置快照。
    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if !value.is_object() {
            return Err("configuration root must be an object".into());
        }
        Ok(Self::new(value))
    }
    /// 按受限的 jq 风格路径查询配置值。
    ///
    /// 返回的引用与快照具有相同生命周期；路径不存在时返回 `Ok(None)`。
    ///
    /// # Errors
    ///
    /// 路径语法无效或超过长度限制时返回描述错误的字符串。
    pub fn query(&self, path: &str) -> Result<Option<&Value>, String> {
        query(&self.0, path)
    }
    /// 查询并反序列化指定配置项。
    ///
    /// 路径不存在时返回 `Ok(None)`；存在但类型不匹配或值无法反序列化时返回错误。
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<Option<T>, String> {
        self.query(path)?
            .map(|v| serde_json::from_value(v.clone()).map_err(|e| e.to_string()))
            .transpose()
    }
    /// 查询并反序列化必需配置项。
    ///
    /// # Errors
    ///
    /// 路径不存在、语法无效或值不能反序列化为目标类型时返回错误。
    pub fn required<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        self.get(path)?
            .ok_or_else(|| format!("missing required configuration value {path}"))
    }

    /// 查询应用布尔选项；应用未指定时继承同名全局选项。
    ///
    /// 应用名比较不区分 ASCII 大小写。配置合并阶段保证全局选项存在，因此缺失或
    /// 类型不正确表示配置快照违反内部不变量。
    pub fn app_bool(&self, executable: &str, option: &str) -> Result<bool, String> {
        let value = self
            .application_value(executable, option)?
            .or_else(|| self.0.get(option))
            .ok_or_else(|| format!("missing required configuration value .{option}"))?;
        value
            .as_bool()
            .ok_or_else(|| format!("configuration value {option} must be a boolean"))
    }

    /// 只查 `app_options.<应用>` 里的布尔选项，不看全局值。
    ///
    /// 给"全局值来自主题文件"的选项用（例如 `inline_preedit` —— 它是
    /// `theme.yaml` 的 `style/inline_preedit`，不是宿主配置的全局键）。
    pub fn app_option_bool(&self, executable: &str, option: &str) -> Result<Option<bool>, String> {
        let Some(value) = self.application_value(executable, option)? else {
            return Ok(None);
        };
        value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("app_options.{executable}.{option} must be a boolean"))
    }

    /// 判断是否有任何应用需要外置 preedit 能力。
    ///
    /// `theme_inline_preedit` 是主题文件（`theme.yaml` 的 `style/inline_preedit`）的值：
    /// 它为 false 时组字串要画进候选窗，必须提前协商外置预编辑能力；此外只要任一应用
    /// 通过 `app_options.<应用>.inline_preedit` 显式关闭，也同样需要协商。
    pub fn needs_external_preedit(&self, theme_inline_preedit: bool) -> Result<bool, String> {
        if !theme_inline_preedit {
            return Ok(true);
        }
        let apps = self
            .0
            .get("app_options")
            .and_then(Value::as_object)
            .ok_or("app_options must be an object")?;
        for (name, options) in apps {
            let options = options
                .as_object()
                .ok_or_else(|| format!("app_options.{name} must be an object"))?;
            let Some(value) = options.get("inline_preedit") else {
                continue;
            };
            let enabled = value
                .as_bool()
                .ok_or_else(|| format!("app_options.{name}.inline_preedit must be a boolean"))?;
            if !enabled {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn application_value(&self, executable: &str, option: &str) -> Result<Option<&Value>, String> {
        let apps = self
            .0
            .get("app_options")
            .and_then(Value::as_object)
            .ok_or("app_options must be an object")?;
        let application = apps.get(executable).or_else(|| {
            apps.iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(executable))
                .map(|(_, value)| value)
        });
        match application {
            Some(value) => value
                .as_object()
                .ok_or_else(|| format!("app_options.{executable} must be an object"))
                .map(|options| options.get(option)),
            None => Ok(None),
        }
    }

    /// 读取指定主题的设置。
    ///
    /// 设置值按 `Deserialize` 反序列化。调用前应已通过
    /// [`with_theme_defaults`](Self::with_theme_defaults) 合并主题默认值。
    ///
    /// # Errors
    ///
    /// 配置路径无效，或现有值无法反序列化为 `T` 时返回错误。
    pub fn theme_settings<T: DeserializeOwned>(&self, name: &str) -> Result<T, String> {
        let key = serde_json::to_string(name).map_err(|e| e.to_string())?;
        self.required(&format!(".themeSettings[{key}]"))
    }

    /// 为指定主题叠加默认设置，并返回新的配置快照。
    ///
    /// 默认对象作为基础，现有 `themeSettings.<name>` 作为覆盖值；递归合并规则由
    /// [`merge`] 定义。输入快照保持不变。
    ///
    /// # Errors
    ///
    /// 默认值、配置根或对应主题设置不是 JSON 对象时返回错误。
    pub fn with_theme_defaults(&self, name: &str, mut defaults: Value) -> Result<Self, String> {
        if !defaults.is_object() {
            return Err(format!("theme {name} defaults must be an object"));
        }
        let mut root = (*self.0).clone();
        let root_object = root
            .as_object_mut()
            .ok_or("configuration root must be an object")?;
        let themes = root_object
            .entry("themeSettings")
            .or_insert_with(|| Value::Object(Default::default()))
            .as_object_mut()
            .ok_or("themeSettings must be an object")?;
        if let Some(overrides) = themes.get(name) {
            if !overrides.is_object() {
                return Err(format!("themeSettings.{name} must be an object"));
            }
            merge(&mut defaults, overrides.clone());
        }
        themes.insert(name.into(), defaults);
        Ok(Self::new(root))
    }
}

/// 将 JSON 覆盖值合并到基础值中。
///
/// 两侧均为对象时递归合并；数组、标量和显式 `null` 均由覆盖值整体替换基础值。
pub fn merge(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            for (key, value) in patch {
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, patch) => *base = patch,
    }
}

/// 使用受限的 jq 风格路径查询 JSON 值。
///
/// 支持对象属性（`.name`）、带引号的对象键（`["a.b"]`）和数组下标（`[0]`），不支持
/// 过滤器或其他表达式。即使中途遇到缺失属性，也会继续验证路径语法。
///
/// `.` 表示根值；空路径无效。返回引用借用 `root`。
///
/// # Errors
///
/// 路径超过 4096 字节或语法不合法时返回错误。
pub fn query<'a>(root: &'a Value, path: &str) -> Result<Option<&'a Value>, String> {
    if path.len() > 4096 {
        return Err("configuration path exceeds 4096 bytes".into());
    }
    if path == "." {
        return Ok(Some(root));
    }
    let mut rest = path;
    let mut current = Some(root);
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix('.') {
            let end = tail.find(['.', '[']).unwrap_or(tail.len());
            let key = &tail[..end];
            if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err("expected a property name after '.'".into());
            }
            current = current.and_then(|v| v.as_object()?.get(key));
            rest = &tail[end..];
        } else if let Some(tail) = rest.strip_prefix('[') {
            if tail.starts_with('"') {
                let mut stream = serde_json::Deserializer::from_str(tail).into_iter::<String>();
                let key = stream
                    .next()
                    .ok_or("missing quoted property")?
                    .map_err(|e| e.to_string())?;
                rest = tail[stream.byte_offset()..]
                    .strip_prefix(']')
                    .ok_or("expected ']' after property")?;
                current = current.and_then(|v| v.as_object()?.get(&key));
            } else {
                let end = tail.find(']').ok_or("expected ']' after index")?;
                let text = &tail[..end];
                if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                    return Err("invalid array index".into());
                }
                let index: usize = text.parse().map_err(|_| "array index too large")?;
                current = current.and_then(|v| v.as_array()?.get(index));
                rest = &tail[end + 1..];
            }
        } else {
            return Err("expected '.' or '[' in configuration path".into());
        }
    }
    if path.is_empty() {
        return Err("empty configuration path; use '.' for root".into());
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    #[test]
    fn application_preedit_overrides_theme_default_and_enables_early_negotiation() {
        // 全局值来自主题（theme.yaml 的 style/inline_preedit），json 里只留逐应用覆盖
        let config = super::ConfigSnapshot::new(serde_json::json!({
            "app_options": {
                "cmd.exe": {"inline_preedit": false},
                "editor.exe": {"inline_preedit": true},
                "other.exe": {"ascii_mode": true}
            }
        }));
        assert_eq!(
            config.app_option_bool("CMD.EXE", "inline_preedit").unwrap(),
            Some(false)
        );
        assert_eq!(
            config.app_option_bool("editor.exe", "inline_preedit").unwrap(),
            Some(true)
        );
        assert_eq!(
            config.app_option_bool("other.exe", "inline_preedit").unwrap(),
            None
        );
        assert_eq!(
            config.app_option_bool("unknown.exe", "inline_preedit").unwrap(),
            None
        );
        // 主题要求画在窗里 → 需要外置预编辑
        assert!(config.needs_external_preedit(false).unwrap());
        // 主题说画在应用里，但有应用把它关掉 → 仍然要协商
        assert!(config.needs_external_preedit(true).unwrap());

        let defaults = super::ConfigSnapshot::new(serde_json::json!({"app_options": {}}));
        assert_eq!(
            defaults.app_option_bool("cmd.exe", "inline_preedit").unwrap(),
            None
        );
        assert!(!defaults.needs_external_preedit(true).unwrap());
    }
    #[test]
    fn application_defaults_distinguish_false_from_missing() {
        let config = super::ConfigSnapshot::new(serde_json::json!({
            "app_options": {"cmd.exe": {"ascii_mode": true}, "editor.exe": {"ascii_mode": false}}
        }));
        assert!(config.app_bool("CMD.EXE", "ascii_mode").unwrap());
        assert!(!config.app_bool("editor.exe", "ascii_mode").unwrap());
        assert!(config.app_bool("unknown.exe", "ascii_mode").is_err());
        let config = super::ConfigSnapshot::new(serde_json::json!({
            "ascii_mode": true,
            "app_options": {"editor.exe": {"ascii_mode": false}}
        }));
        assert!(config.app_bool("unknown.exe", "ascii_mode").unwrap());
        assert!(!config.app_bool("EDITOR.EXE", "ascii_mode").unwrap());
        for global in [false, true] {
            let config = super::ConfigSnapshot::new(serde_json::json!({
                "ascii_mode": global,
                "app_options": {"empty.exe": {}, "explicit.exe": {
                    "ascii_mode": false, "inline_preedit": false
                }}
            }));
            assert_eq!(config.app_bool("EMPTY.EXE", "ascii_mode").unwrap(), global);
            // inline_preedit 没有全局键：没覆盖就是 None（由主题决定）
            assert_eq!(
                config.app_option_bool("EMPTY.EXE", "inline_preedit").unwrap(),
                None
            );
            assert!(!config.app_bool("explicit.exe", "ascii_mode").unwrap());
            assert_eq!(
                config
                    .app_option_bool("explicit.exe", "inline_preedit")
                    .unwrap(),
                Some(false)
            );
        }
    }
    use super::*;
    #[test]
    fn theme_defaults_are_overlaid_without_changing_the_source() {
        let source = ConfigSnapshot::new(serde_json::json!({
            "ascii_mode": true,
            "themeSettings": {"abc": {"antialiasing": false, "nested": {"b": 3}}}
        }));
        let merged = source
            .with_theme_defaults(
                "abc",
                serde_json::json!({
                    "antialiasing": true, "nested": {"a": 1, "b": 2}
                }),
            )
            .unwrap();
        assert_eq!(
            merged
                .get::<bool>(".themeSettings.abc.antialiasing")
                .unwrap(),
            Some(false)
        );
        assert_eq!(
            merged.get::<i32>(".themeSettings.abc.nested.a").unwrap(),
            Some(1)
        );
        assert_eq!(
            merged.get::<i32>(".themeSettings.abc.nested.b").unwrap(),
            Some(3)
        );
        assert!(
            source
                .query(".themeSettings.abc.nested.a")
                .unwrap()
                .is_none()
        );
        assert_eq!(merged.get::<bool>(".ascii_mode").unwrap(), Some(true));
        let fallback = source
            .with_theme_defaults("ten", serde_json::json!({"size": 12}))
            .unwrap();
        assert_eq!(
            fallback.get::<i32>(".themeSettings.ten.size").unwrap(),
            Some(12)
        );
    }
    #[test]
    fn paths_preserve_objects_null_and_missing() {
        let root = serde_json::json!({"a.b": [null, {"color":"red"}]});
        assert_eq!(
            query(&root, "[\"a.b\"][1].color").unwrap(),
            Some(&serde_json::json!("red"))
        );
        assert_eq!(query(&root, "[\"a.b\"][0]").unwrap(), Some(&Value::Null));
        assert!(query(&root, ".missing").unwrap().is_none());
        assert!(query(&root, ".missing | anything").is_err());
        assert_eq!(query(&root, ".").unwrap(), Some(&root));
    }

    /// 手工临时目录（common 没有 tempfile dev-dependency）。
    fn temporary_directory(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "weasel-common-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// ① YAML 能读：`weasel.yaml` 里的值必须原样变成配置对象。
    #[test]
    fn yaml_program_config_is_read() {
        let directory = temporary_directory("yaml-read");
        let text = r#"theme: ten
sound:
  enabled: false
app_options:
  cmd.exe:
    ascii_mode: true
"#;
        let path = program_config_path(&directory);
        std::fs::write(&path, text).unwrap();
        let value = parse_config_text(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["theme"], "ten");
        assert_eq!(value["sound"]["enabled"], false);
        assert_eq!(value["app_options"]["cmd.exe"]["ascii_mode"], true);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// ② 只有 TOML/JSON 时**读不到配置**：读取点只查 `weasel.yaml`，
    /// 旧文件既不解析也不迁移，调用方拿到"没有文件"后走内嵌默认值（不报错、不崩）。
    #[test]
    fn legacy_toml_only_program_config_is_not_read() {
        let directory = temporary_directory("toml-ignored");
        std::fs::write(directory.join("weasel.toml"), b"theme = \"ten\"\n").unwrap();
        std::fs::write(directory.join("weasel.json"), br#"{"theme":"seven"}"#).unwrap();
        // 唯一的读取路径没文件 —— 这就是"读不到"，不是错误。
        assert!(!program_config_path(&directory).exists());
        assert!(
            std::fs::read_to_string(program_config_path(&directory)).is_err(),
            "weasel.toml 不许被当成 weasel.yaml 读"
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }
}

