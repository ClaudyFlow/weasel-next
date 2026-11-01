//! 加载、合并并保存设置 JSON，同时保留用户覆盖中的未知字段。
//!
//! 文档将程序默认值、可选的程序目录覆盖和用户自定义补丁分开保存；写回前
//! 比较文件原始字节以发现外部修改，并通过同目录临时文件完成替换。
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use weasel_common::{process::RuntimePaths, settings::merge};
/// 设置配置的基底、用户补丁及用于冲突检测的文件快照。
pub struct Document {
    /// 内嵌默认配置与程序目录覆盖合并后的基底。
    pub base: Value,
    /// 用户可编辑的覆盖对象；未知字段也会原样保留在该补丁中。
    pub patch: Value,
    /// 用户自定义配置文件位置。
    path: PathBuf,
    /// 加载时文件的原始文本；`None` 表示当时文件不存在。
    original: Option<String>,
}
/// 读取配置文件，最多接受 1 MiB；文件不存在返回 `Ok(None)`。
///
/// 超大文件或其他 I/O 错误返回 `Err`。保留原始文本（含 BOM 已去除），供保存时
/// 精确检测外部写入。
fn read(path: &Path) -> Result<Option<String>, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(1048577)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1048576 {
        return Err("配置超过 1 MiB".into());
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes).to_vec();
    String::from_utf8(bytes).map(Some).map_err(|e| e.to_string())
}

/// 把配置文本解析为对象。
///
/// **宿主配置只有 YAML 一种格式**（`weasel.yaml`，2026-10-23 起）：`style::parse_file`
/// 只认 YAML，写盘序列化也统一在 `weasel_common::yaml`，所以这里不按扩展名分派
/// TOML/JSON，也没有 TOML→YAML 的迁移层（旧文件由用户自己改成 YAML）。
fn object(text: &str) -> Result<Value, String> {
    weasel_common::settings::parse_config_text(text)
}

impl Document {
    /// 返回基底与用户补丁合并后的新配置值，不修改文档自身。
    pub fn effective(&self) -> Value {
        let mut result = self.base.clone();
        merge(&mut result, self.patch.clone());
        result
    }
    /// 加载内嵌默认配置、可选程序目录覆盖和用户自定义补丁。
    ///
    /// 合并顺序为内嵌默认值、程序目录 `weasel.yaml`、用户补丁 `weasel.yaml`（用户目录根）。
    /// 缺少用户文件时补丁为空对象且原始快照为 `None`；任何存在但无效的配置或读取错误都会失败。
    pub fn load(paths: &RuntimePaths) -> Result<Self, String> {
        let mut base = object(include_str!("../../Rime-Next/weasel.yaml"))?;
        // 程序文件夹只有这一份：weasel.yaml。旧 weasel.toml / weasel.json 既不读也不迁移
        // （用户自己把旧文件改成 YAML）；没有它就用内嵌出厂默认值，不算错误。
        if let Some(text) = read(&weasel_common::settings::program_config_path(
            &paths.executable_directory,
        ))? {
            merge(&mut base, object(&text)?);
        }
        let path = weasel_common::settings::user_config_path(&paths.user_data);
        // 只搬一次位置（`weasel\weasel.yaml` → 用户目录根），不涉及任何格式转换。
        weasel_common::settings::migrate_interim_user_config(&paths.user_data)?;
        let original = read(&path)?;
        let patch = match &original {
            Some(text) => object(text)?,
            None => serde_json::json!({}),
        };
        Ok(Self {
            base,
            patch,
            path,
            original,
        })
    }
    /// 原子保存用户补丁，并在替换前检测文件是否已被其他程序修改。
    ///
    /// 序列化结果最多 1 MiB，先写入目标目录中的临时文件并同步，再与加载/上次
    /// 成功保存时的原始字节比较。快照不一致时拒绝覆盖；只有替换成功后才更新
    /// `original`，因此失败后可重试且不会被误认为已保存。所有文件系统错误均返回 `Err`。
    pub fn save(&mut self) -> Result<(), String> {
        let parent = self.path.parent().ok_or("配置路径无父目录")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        // 用户补丁现在是 YAML（用户目录根 `weasel.yaml`），写盘也用 YAML 序列化
        let mut text = weasel_common::yaml::to_string_pretty(&self.patch);
        if !text.ends_with('\n') {
            text.push('\n');
        }
        if text.len() > 1048576 {
            return Err("配置超过 1 MiB".into());
        }
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        if read(&self.path)? != self.original {
            return Err("文件已被其他程序修改，请关闭后重新打开设置。".into());
        }
        file.persist(&self.path).map_err(|e| e.to_string())?;
        self.original = Some(text);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_unknown_fields_and_rejects_external_changes() {
        let directory = tempfile::tempdir().unwrap();
        let paths = RuntimePaths {
            executable_directory: directory.path().to_path_buf(),
            user_data: directory.path().join("user"),
            logs: directory.path().join("logs"),
        };
        fs::create_dir_all(&paths.user_data).unwrap();
        let path = weasel_common::settings::user_config_path(&paths.user_data);
        // 2026-10-21 起这份补丁是 YAML，且就在用户目录根（`weasel.yaml`）
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            b"ascii_mode: true\nthemeSettings:\n  future:\n    value: 42\n",
        )
        .unwrap();
        let mut doc = Document::load(&paths).unwrap();
        doc.patch.as_object_mut().unwrap().remove("ascii_mode");
        doc.save().unwrap();
        let saved: Value =
            weasel_common::yaml::to_value(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["themeSettings"]["future"]["value"], 42);
        assert!(saved.get("ascii_mode").is_none());
        fs::write(&path, b"").unwrap();
        assert!(doc.save().is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    /// 老机器（程序文件夹只有 `weasel.toml`）：**读不到**旧 TOML，也不迁移，
    /// 直接用内嵌出厂默认值 —— 不报错、不崩。
    #[test]
    fn legacy_toml_program_config_is_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let paths = RuntimePaths {
            executable_directory: directory.path().to_path_buf(),
            user_data: directory.path().join("user"),
            logs: directory.path().join("logs"),
        };
        fs::create_dir_all(&paths.user_data).unwrap();
        fs::write(
            directory.path().join("weasel.toml"),
            b"theme = \"ten\"\ninput_mode_indicator_duration_ms = 1234\n",
        )
        .unwrap();

        let doc = Document::load(&paths).unwrap();
        assert_eq!(doc.base["theme"], "official", "旧 TOML 不该被读，用出厂默认值");
        assert_eq!(
            doc.base["input_mode_indicator_duration_ms"], 800,
            "TOML 里的 1234 不该生效"
        );
        // 不迁移：不生成 weasel.yaml，也不动旧文件。
        assert!(!weasel_common::settings::program_config_path(&paths.executable_directory).exists());
        assert!(directory.path().join("weasel.toml").is_file());
        // 手工改成 YAML 后立即生效。
        fs::write(
            weasel_common::settings::program_config_path(&paths.executable_directory),
            b"theme: ten\n",
        )
        .unwrap();
        let doc = Document::load(&paths).unwrap();
        assert_eq!(doc.base["theme"], "ten");
    }

    /// 2026-10-21 的**中途落点** `weasel\weasel.yaml` 也要能搬回用户目录根（同一份 YAML，原样搬）。
    #[test]
    fn migrates_interim_subdirectory_patch_to_root() {
        let directory = tempfile::tempdir().unwrap();
        let paths = RuntimePaths {
            executable_directory: directory.path().to_path_buf(),
            user_data: directory.path().join("user"),
            logs: directory.path().join("logs"),
        };
        let interim = weasel_common::settings::interim_user_config_path(&paths.user_data);
        fs::create_dir_all(interim.parent().unwrap()).unwrap();
        fs::write(&interim, b"sound:\n  mode: music\n").unwrap();

        let doc = Document::load(&paths).unwrap();
        assert_eq!(doc.patch["sound"]["mode"], "music");
        let root_path = weasel_common::settings::user_config_path(&paths.user_data);
        assert!(root_path.is_file(), "要搬到用户目录根：{}", root_path.display());
        assert_eq!(root_path.parent().unwrap(), paths.user_data);
        assert!(!interim.exists(), "中途落点那份要清掉");
        assert!(!paths.user_data.join("weasel").exists(), "空掉的 weasel\\ 子目录要删");
    }
}
