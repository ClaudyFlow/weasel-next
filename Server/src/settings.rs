//! 加载并合并由 broker 管理的启动配置。
//!
//! **宿主配置的格式只有 YAML 一种**（2026-10-23 起）：`style::parse_file` 只认 YAML，
//! 配置树与写盘序列化也统一在 `weasel_common::yaml`，所以 TOML/JSON **既不读也不写** ——
//! 没有兼容层，也没有 TOML→YAML 的迁移：程序文件夹里没有 `weasel.yaml` 时，就用手上的
//! 内嵌出厂默认值（旧 `weasel.toml` 由用户自己改成 YAML）。
//!
//! 读取顺序（后者覆盖前者）：程序文件夹 `weasel.yaml` → 用户补丁
//! `%APPDATA%\Rime-Next\weasel.yaml`。
//!
//! 对象递归合并，数组和标量替换已有值；单个文件无效时保留该文件应用前的配置。
//! 这里只检查文件、格式和传输大小；具体选项由消费组件校验。
use serde_json::Value;
use std::{
    io::{self, Read},
    path::Path,
};
use weasel_common::{
    process::RuntimePaths,
    settings::{ConfigSnapshot, merge},
};

/// 单个覆盖配置文件允许读取的最大字节数（1 MiB）。
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// 从打包默认值开始，依次应用可选的程序目录配置和用户自定义配置。
///
/// 后加载的文件优先级更高。缺失文件会被忽略；读取或解析失败时调用 `warn` 报告问题，
/// 并继续使用此前有效的值。
pub fn load(paths: &RuntimePaths, mut warn: impl FnMut(String)) -> ConfigSnapshot {
    // Also keep packaged defaults available to standalone cargo builds.
    let mut value: Value =
        weasel_common::settings::parse_config_text(include_str!("../../Rime-Next/setting.yaml"))
            .expect("packaged settings must be valid YAML");
    // 只有这两份 YAML：程序文件夹一份、用户目录一份。旧 `weasel.toml` / `weasel.json`
    // 不在候选里 —— 读到它们的唯一结果就是"读不到"，然后继续用内嵌默认值。
    for path in [
        weasel_common::settings::program_config_path(&paths.executable_directory),
        weasel_common::settings::user_config_path(&paths.user_data),
    ] {
        if let Err(error) = overlay_file(&mut value, &path) {
            warn(format!(
                "settings {}: {error}; keeping previous settings",
                path.display()
            ));
        }
    }
    ConfigSnapshot::new(value)
}

/// 将单个可选配置文件叠加到基值；缺失文件不改变基值。
///
/// 单文件最多读取 1 MiB。文件读取、字节解码、YAML 解析（唯一格式）、根对象检查和
/// 配置合并均在此处完成。新配置只有在全部检查通过后才会原子地替换基值。
fn overlay_file(base: &mut Value, path: &Path) -> Result<(), String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err("configuration exceeds 1 MiB".into());
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let patch: Value = weasel_common::settings::parse_config_text(text)?;
    let mut next = base.clone();
    merge(&mut next, patch);

    // 此处不理解 `theme`、`inline_preedit` 等业务选项。消费配置的组件负责验证
    // 自己使用的字段、报告错误并选择回退值。这里只保留 RPC 帧大小这一传输约束。
    // Leave room for protobuf fields when two individually valid files merge.
    if next.to_string().len() > weasel_common::data_frame::MAX_FRAME_SIZE - 4096 {
        return Err("merged configuration exceeds RPC size limit".into());
    }
    *base = next;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/settings.rs"]
mod tests;
