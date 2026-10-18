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

/// 把 TOML 文本（文件名以 `.json` 结尾的按 JSON 解析，兼容旧文件）解析为对象。
fn object(text: &str, name: &str) -> Result<Value, String> {
    let value: Value = if name.ends_with(".json") {
        serde_json::from_str(text).map_err(|e| e.to_string())?
    } else {
        toml::from_str(text).map_err(|e| e.to_string())?
    };
    if !value.is_object() {
        return Err("配置必须为对象".into());
    }
    Ok(value)
}

/// 内嵌出厂默认值（TOML）→ 对象。
fn toml_object(text: &str) -> Result<Value, String> {
    object(text, "weasel.toml")
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
    /// 合并顺序为内嵌默认值、程序目录 `weasel.toml`、用户补丁 `weasel.custom.toml`。
    /// 缺少用户文件时补丁为空对象且原始快照为 `None`；任何存在但无效的配置或读取错误都会失败。
    pub fn load(paths: &RuntimePaths) -> Result<Self, String> {
        let mut base = toml_object(include_str!("../../weasel.toml"))?;
        // 兼容期：程序目录里可能还留着上一版的 weasel.json
        for name in ["weasel.toml", "weasel.json"] {
            if let Some(text) = read(&paths.executable_directory.join(name))? {
                merge(&mut base, object(&text, name)?);
            }
        }
        let path = paths.user_data.join("weasel.custom.toml");
        let original = read(&path)?;
        let patch = match &original {
            Some(text) => object(&text, "weasel.custom.toml")?,
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
        let mut text = toml::to_string_pretty(&self.patch).map_err(|e| e.to_string())?;
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
        let path = paths.user_data.join("weasel.custom.toml");
        // 夹具必须是 TOML：这份用户补丁在 TOML 化改造后就不读 YAML 了
        // （原来这里是 `ascii_mode: true` + 缩进嵌套，解析直接报错）。
        fs::write(
            &path,
            b"ascii_mode = true\n\n[themeSettings.future]\nvalue = 42\n",
        )
        .unwrap();
        let mut doc = Document::load(&paths).unwrap();
        doc.patch.as_object_mut().unwrap().remove("ascii_mode");
        doc.save().unwrap();
        let saved: Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["themeSettings"]["future"]["value"], 42);
        assert!(saved.get("ascii_mode").is_none());
        fs::write(&path, b"").unwrap();
        assert!(doc.save().is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }
}
