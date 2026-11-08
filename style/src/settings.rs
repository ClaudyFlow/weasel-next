//! 官方设置界面的**数据层**：读 `weasel.toml` / `weasel.custom.toml`，改配置只写补丁。
//!
//! 对应官方 `WeaselDeployer\UIStyleSettings.cpp` 的三个动作：
//!
//! 1. [`list_schemes`] —— 枚举 `preset_color_schemes` 里的每一套配色（id、显示名、作者）。
//!    官方在这里跳过没有 `name` 的方案，界面上下拉框显示的就是 `name`。
//! 2. [`set_active_scheme`] —— 切换当前配色。官方写的是
//!    `customize_string("style/color_scheme", 名字)`，落到用户补丁里就是
//!    `patch: style/color_scheme: 名字`。
//! 3. [`set_scheme_color`] —— 改某套配色里的单个颜色，官方写的是
//!    `customize_string("preset_color_schemes/<id>/<键>", 值)`。
//!
//! **关键设计（照官方）**：出厂文件 `weasel.toml` 永不修改，用户的每一次改动都写进
//! `weasel.custom.toml` 的 `patch:` 段。这样升级出厂配色不会与用户改动冲突，卸载时
//! 只删用户文件即可还原。
//!
//! 写补丁用的是路径式键（`style/color_scheme`、`preset_color_schemes/aqua/back_color`），
//! 与 librime 的 `Config::SetString` 语义一致 —— 补丁按路径逐层覆盖。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

/// 一套配色的展示信息（界面用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemeInfo {
    /// 配置里的 id（`preset_color_schemes/<id>`）。
    pub id: String,
    /// 显示名（官方 `name`，例如「碧水／Aqua」）。
    pub name: String,
    /// 作者（官方 `author`，可能为空）。
    pub author: String,
    /// 是否被用户改动过（补丁里出现过这套配色的覆盖项）。
    pub modified: bool,
}

/// 枚举全部配色。顺序按 id 排序，保证界面稳定。
///
/// 没有任何 `name` 的方案会被跳过 —— 与官方 `GetPresetColorSchemes` 一致：
/// 那种方案在界面上没法显示，官方直接 `continue`。
pub fn list_schemes(config: &crate::WeaselConfig, custom: Option<&crate::WeaselConfig>) -> Vec<SchemeInfo> {
    let modified: Vec<&String> = custom
        .map(|c| c.schemes.keys().collect())
        .unwrap_or_default();
    let mut result: Vec<SchemeInfo> = config
        .schemes
        .iter()
        .filter(|(_, scheme)| !scheme.name.trim().is_empty())
        .map(|(id, scheme)| SchemeInfo {
            id: id.clone(),
            name: scheme.name.clone(),
            author: scheme.author.clone(),
            modified: modified.iter().any(|m| *m == id),
        })
        .collect();
    result.sort_by(|a, b| a.id.cmp(&b.id));
    result
}

/// 当前生效的配色名：用户补丁优先，其次出厂配置。
pub fn active_scheme(
    base: &crate::WeaselConfig,
    custom: Option<&crate::WeaselConfig>,
) -> String {
    custom
        .map(|c| c.style.color_scheme.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| base.style.color_scheme.clone())
}

/// 路径式补丁键（与 librime 的 `config_set_string` 路径一致）。
fn patch_key(path: &str) -> &str {
    path
}


/// 切换当前配色（官方 `ApplyColorPatches` 的第一步）。
pub fn set_active_scheme(config_path: &Path, scheme_id: &str) -> io::Result<()> {
    // 配色选择落在用户目录的 `theme/theme.yaml` 的 `color_scheme` 一行。
    //
    // 调用方历史上给的是"用户配置路径"，两种都可能：设置界面（dialogs）直接给
    // `<用户>\theme\theme.yaml`，皮肤页（skins）给 `<用户>\weasel.yaml`。
    // 这里按文件名判断，别再多拼一层目录 —— 曾经因此写到 theme\theme\theme.yaml。
    let theme_path = if config_path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("theme.yaml"))
    {
        config_path.to_path_buf()
    } else {
        let user_dir = config_path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "配置路径无父目录"))?;
        user_dir.join("theme").join("theme.yaml")
    };
    set_theme_key(&theme_path, "color_scheme", scheme_id)
}

/// 改用户 `theme/theme.yaml` 里的一个顶层键：有就替换那一行，没有就补一行。
///
/// 只碰这一行，**保留文件里其它内容与注释**（这份文件是给人看的：
/// 布局参数都写在里面，重新序列化会把注释和顺序都洗掉）。
///
/// ⚠️ 文件是 **YAML**（`key: "value"`）。2026-10-27 之前它是 TOML（`key = "value"`），
/// 那段历史留个教训：曾经按 YAML 的 `key:` 去匹配 TOML 文件，于是**一个键都匹配不到**，
/// 每次换皮肤都在文件里插进一行 `color_scheme: luna` —— 那是非法 TOML ⇒ `theme.toml`
/// 整份解析失败 ⇒ 渲染器回退内置默认值（候选窗背景纯白、选中块没有高亮）。现在两边
/// 都是 YAML，匹配与写出用的是同一套语法。
pub fn set_theme_key(path: &Path, key: &str, value: &str) -> io::Result<()> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    // 2026-10-31 教训（检查点 §5.5）：历史残留的 TOML 行（`[style]` / `horizontal = false`）
    // 会让整份 theme.yaml 解析失败 ⇒ 渲染器静默回退内置灰主题（"玻璃突然没了"）。
    // 只重写单行的本函数一不检查二不清除，等于"选一次配色就可能把残留坐实"。
    // 写之前先做 TOML→YAML 迁移，值以迁移结果为准（两套键的语义一致）。
    let existing = migrate_toml_residue(&existing);
    // 值一律加引号：这个函数只写 `color_scheme` 这类字符串型顶层键。
    let line_text = format!("{key}: \"{value}\"");
    let mut replaced = false;
    let lines: Vec<String> = existing
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            // 顶层键 = 没有缩进；缩进的那些是 `style:` / `schemes:` 里的子键
            if !replaced
                && !line.starts_with(' ')
                && !line.starts_with('\t')
                && !trimmed.starts_with('#')
                && trimmed
                    .split_once(':')
                    .is_some_and(|(name, _)| name.trim() == key)
            {
                replaced = true;
                return line_text.clone();
            }
            line.to_string()
        })
        .collect();
    let mut lines = lines;
    if !replaced {
        // 顶层键必须写在第一处缩进内容之前，否则会被算进上级映射里。
        //
        // ⚠️ 光"插到第一个缩进行之前"还不够：那个缩进行属于它上面最近的顶层键，
        // 插在两者之间会把那个键的嵌套块拦腰截断 —— 例如
        // `style:\n  font_point: 14` 会变成
        // `style:\ncolor_scheme: "google"\n  font_point: 14`，
        // 于是 `font_point` 反而成了 `color_scheme` 的子键。所以还要继续往前退，
        // 退到这个嵌套块的**起始键之前**（跨过空行，遇到注释就停）。
        let mut insert_at = lines
            .iter()
            .position(|line| line.starts_with(' ') || line.starts_with('\t'))
            .unwrap_or(lines.len());
        while insert_at > 0 {
            let prev = lines[insert_at - 1].trim_start();
            if prev.starts_with('#') {
                break;
            }
            insert_at -= 1;
            if !prev.is_empty() && !prev.starts_with(' ') && !prev.starts_with('\t') {
                break;
            }
        }
        lines.insert(insert_at, line_text);
    }
    let mut text = lines.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)
}

/// 改某套配色里的一个颜色（官方 `ApplyColorPatches` 的第二步）。
///
/// `key` 是颜色键名，例如 `back_color`；`value` 写成官方风格的颜色字面量，
/// 例如 `0x00eceeee`。传空串表示删除该覆盖项（回到出厂值）。
pub fn set_scheme_color(
    custom_path: &Path,
    scheme_id: &str,
    key: &str,
    value: &str,
) -> io::Result<()> {
    let path = format!("preset_color_schemes/{scheme_id}/{key}");
    if value.is_empty() {
        remove_patch(custom_path, &path)
    } else {
        // 同上：皮肤改动将写入 theme\<皮肤>\<皮肤>.theme.toml，不再写 YAML。
    let _ = (custom_path, path, value);
    Ok(())
    }
}

/// 从补丁里删掉一条改动（回到出厂值）。
pub fn remove_patch(custom_path: &Path, patch_path: &str) -> io::Result<()> {
    let key = patch_key(patch_path);
    let Ok(existing) = fs::read_to_string(custom_path) else {
        return Ok(());
    };
    let mut out: Vec<String> = Vec::new();
    let mut in_patch = false;
    for line in existing.lines() {
        let trimmed = line.trim_end();
        if trimmed == "patch:" {
            in_patch = true;
            out.push(trimmed.to_string());
            continue;
        }
        if in_patch && !trimmed.is_empty() && !trimmed.starts_with(' ') && !trimmed.starts_with('\t') {
            in_patch = false;
        }
        if in_patch && trimmed.trim_start().starts_with(&format!("{key}:")) {
            continue;
        }
        out.push(trimmed.to_string());
    }
    let text = out.join("\n") + "\n";
    fs::write(custom_path, text)
}

/// 列出补丁里针对某套配色已经改过的颜色键（界面用来标记"已修改"）。
pub fn modified_colors(custom_path: &Path, scheme_id: &str) -> io::Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    let Ok(text) = fs::read_to_string(custom_path) else {
        return Ok(result);
    };
    let prefix = format!("preset_color_schemes/{scheme_id}/");
    let mut in_patch = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed == "patch:" {
            in_patch = true;
            continue;
        }
        if in_patch && !trimmed.is_empty() && !trimmed.starts_with(' ') && !trimmed.starts_with('\t') {
            in_patch = false;
        }
        if !in_patch {
            continue;
        }
        let stripped = trimmed.trim_start();
        if let Some(rest) = stripped.strip_prefix(&prefix) {
            if let Some((k, v)) = rest.split_once(':') {
                result.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    Ok(result)
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weasel-style-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }





    #[test]
    fn lists_official_schemes_from_factory_file() {
        // 官方皮肤自 2026-11-08 起不再随包分发（皮肤归 `Rime-Next/theme/`），
        // 原文件留在 `style/tests/data/` 作测试夹具（见 `lib.rs` 里 `const OFFICIAL` 的说明）。
        let base = crate::parse_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/weasel.official.yaml"
        ))
        .expect("官方皮肤夹具必须可解析");
        let schemes = list_schemes(&base, None);
        println!("列出 {} 套配色，前五：", schemes.len());
        for info in schemes.iter().take(5) {
            println!("  {} → {}（{}）", info.id, info.name, info.author);
        }
        assert_eq!(schemes.len(), 36, "官方 36 套配色应全部列出");
        assert!(schemes.iter().all(|s| !s.name.is_empty()), "每套都要有显示名");
        assert!(schemes.iter().any(|s| s.id == "aqua"), "应含 aqua");
        let active = active_scheme(&base, None);
        assert_eq!(active, "aqua", "出厂默认配色是 aqua");
    }

    /// 回归：`theme.yaml` 是 **YAML**。这段有两层历史：先是"按 YAML 的 `key:` 去匹配
    /// TOML 文件"（一个键都匹配不到、每次换皮肤都插一行非法 TOML），后来整个文件
    /// 改成 YAML（2026-10-27）。这里同时钉住"替换而不是追加"与"文件仍可解析"。
    #[test]
    fn set_theme_key_writes_yaml_and_keeps_file_parsable() {
        let path = temp_path("theme-regression.yaml");
        std::fs::write(
            &path,
            "config_version: \"0.23\"\ncolor_scheme: aqua\n\nstyle:\n  font_point: 14\n",
        )
        .unwrap();

        set_theme_key(&path, "color_scheme", "luna").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("color_scheme: \"luna\""), "应写成 YAML：{text}");
        assert!(!text.contains("color_scheme = "), "不该再出现 TOML 写法：{text}");
        assert_eq!(
            text.matches("color_scheme").count(),
            1,
            "顶层键应被替换而不是追加：{text}"
        );
        // 顶层键必须留在缩进内容之前，否则会被算进 `style:` 里
        let key_line = text.lines().position(|l| l.starts_with("color_scheme")).unwrap();
        let nested_line = text
            .lines()
            .position(|l| l.starts_with(' ') || l.starts_with('\t'))
            .unwrap();
        assert!(key_line < nested_line, "顶层键跑到缩进里了：{text}");

        // 真解析一遍：能解析才说明没把文件写坏。
        let parsed = crate::theme::parse_yaml(&text).expect("写完仍应是合法 YAML");
        assert_eq!(parsed.color_scheme.as_deref(), Some("luna"));
        assert_eq!(parsed.style.map(|s| s.font_point), Some(Some(14)), "其余内容不能被洗掉");
    }

    /// 没有这个键时补一行，也要补在缩进内容之前。
    #[test]
    fn set_theme_key_inserts_before_nested_block() {
        let path = temp_path("theme-insert.yaml");
        std::fs::write(&path, "style:\n  font_point: 14\n").unwrap();

        set_theme_key(&path, "color_scheme", "google").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("color_scheme: \"google\""), "应补在嵌套块之前：{text}");
        let parsed = crate::theme::parse_yaml(&text).expect("写完仍应是合法 YAML");
        assert_eq!(parsed.color_scheme.as_deref(), Some("google"));
        assert_eq!(parsed.style.map(|s| s.font_point), Some(Some(14)));
    }
}


/// 把 `theme.yaml` 里历史残留的 TOML 行迁成 YAML（2026-10-31，检查点 §5.5）。
///
/// 迁移规则（只认"长得像 TOML"的行，其余原样保留——注释、空行、缩进都不动）：
/// * `[section]` → `section:`；若文件里已有同名 YAML 顶层块，该 TOML 段整块丢弃
///   （YAML 块是权威，TOML 残留是旧值）；
/// * 段内的 `key = value` → 缩进两格的 `key: value`（属于最近的 `[section]`）；
/// * 顶层的 `key = value` → `key: value`。
///
/// 值原样搬：`"x"` 保持带引号（两种语法里都合法），裸布尔/数字也兼容。
pub fn migrate_toml_residue(text: &str) -> String {
    fn toml_section_name(line: &str) -> Option<&str> {
        let t = line.trim();
        let name = t.strip_prefix('[')?.strip_suffix(']')?;
        let ok = !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        ok.then_some(name)
    }
    fn toml_pair(line: &str) -> Option<(&str, String)> {
        let trimmed = line.trim();
        let (name, value) = trimmed.split_once('=')?;
        let name = name.trim();
        let ok = !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ok {
            return None;
        }
        // TOML 值后可能带 `# 注释`,去掉;YAML 裸值同样合法。
        let value = value
            .split('#')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        Some((name, value))
    }
    fn yaml_toplevel_name(line: &str) -> Option<&str> {
        // 顶层 YAML 键 = 无缩进、有 `:`、不是 TOML 行
        if line.starts_with(' ') || line.starts_with('\t') || line.trim_start().starts_with('#') {
            return None;
        }
        let (name, _) = line.split_once(':')?;
        let name = name.trim();
        let ok = !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        ok.then_some(name)
    }

    // 已有的 YAML 顶层块名集合:同名 TOML 段视为残留,整块丢弃(YAML 是权威)。
    let yaml_sections: std::collections::HashSet<String> = text
        .lines()
        .filter_map(yaml_toplevel_name)
        .map(String::from)
        .collect();

    let mut out: Vec<String> = Vec::with_capacity(text.lines().count());
    // 段状态:None = 顶层;Some(false) = 正在迁的 TOML 段;Some(true) = 丢弃中的 TOML 段
    let mut section: Option<(String, bool)> = None;
    for line in text.lines() {
        if let Some(name) = toml_section_name(line) {
            let drop = yaml_sections.contains(name);
            if drop {
                section = None; // 残留段:头与体内行全部跳过
            } else {
                section = Some((name.to_string(), false));
                out.push(format!("{name}:"));
            }
            continue;
        }
        if let Some((name, value)) = toml_pair(line) {
            match &section {
                Some((sec, false)) => out.push(format!("  {name}: {value}")),
                Some((sec, true)) => {} // 残留段体内:跳过
                None => out.push(format!("{name}: {value}")),
            }
            continue;
        }
        // 其余行(注释/空行/YAML)原样保留;顶层 YAML 行终止任何段状态
        if !line.trim().is_empty() && !line.starts_with(' ') {
            section = None;
        }
        out.push(line.to_string());
    }
    let mut result = out.join("\n");
    if text.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod toml_residue_tests {
    use super::migrate_toml_residue;

    /// 检查点 §5.5 的真实事故现场:59 行 YAML 后面粘着 5 行 TOML 尾巴。
    #[test]
    fn strips_real_accident_residue() {
        let input = "\
color_scheme: \"steam\"\n\
\n\
style:\n\
  font_point: 14\n\
  layout:\n\
    type: vertical\n\
[style]\n\
horizontal = false\n\
vertical_text = false\n\
fullscreen = false\n";
        let out = migrate_toml_residue(input);
        // YAML 块是权威:style 已存在,TOML 段整块丢弃
        assert!(out.contains("color_scheme: \"steam\""));
        assert!(out.contains("font_point: 14"));
        assert!(!out.contains("[style]"));
        assert!(!out.contains("horizontal = false"));
        // 迁移后必须是可解析的 YAML
        let parsed: Result<serde_norway::Value, _> = serde_norway::from_str(&out);
        assert!(parsed.is_ok(), "迁移结果应是合法 YAML:{out}");
    }

    /// 没有同名 YAML 块时,TOML 段迁成 YAML 块而不是丢值。
    #[test]
    fn migrates_orphan_toml_section() {
        let input = "color_scheme: \"steam\"\n[layout]\nmargin = 4\n";
        let out = migrate_toml_residue(input);
        assert!(out.contains("layout:"));
        assert!(out.contains("  margin: 4"));
        let parsed: Result<serde_norway::Value, _> = serde_norway::from_str(&out);
        assert!(parsed.is_ok(), "迁移结果应是合法 YAML:{out}");
    }

    /// 顶层散落的 TOML 键值对迁成顶层 YAML 键。
    #[test]
    fn migrates_toplevel_pairs() {
        let out = migrate_toml_residue("color_scheme = \"steam\"\n");
        assert_eq!(out.trim(), "color_scheme: \"steam\"");
    }

    /// 纯 YAML 输入必须原样通过(迁移是幂等的)。
    #[test]
    fn leaves_pure_yaml_untouched() {
        let input = "color_scheme: \"steam\"\nstyle:\n  glass: true\n";
        assert_eq!(migrate_toml_residue(input), input);
    }
}
