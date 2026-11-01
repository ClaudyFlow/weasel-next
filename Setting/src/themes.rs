//! 配色（皮肤）页的**框架无关**数据层。
//!
//! 从 `skins.rs` 抽出来：那份只服务 Slint（返回值是 Slint 生成的 `SchemeRow`），
//! D2D 外壳用不了。现在两套界面共用这一份逻辑：
//!
//! * 读：出厂主题链 `theme/<皮肤>/<皮肤>.theme.yaml` + 用户 `theme/theme.yaml`，
//!   合成结果与渲染器**同一份**（`weasel_style::theme::load_theme`）；
//! * 写：切配色落 `theme/theme.yaml` 的 `color_scheme` 一行（`weasel_style::settings::set_active_scheme`）；
//! * 颜色一律是内部 ARGB `0xAARRGGBB`（与 `theme/src/paint.rs::d2d_color` 一致）。
//!
//! 这里**不允许**出现任何 Slint / Direct2D 类型。

use std::path::{Path, PathBuf};

use weasel_style::WeaselConfig;

/// 出厂皮肤名。与渲染器一致（`theme/official/src/config.rs::SKIN`）：
/// 皮肤文件是 `theme/default/default.theme.yaml`，里面是**全部配色**。
pub const SKIN: &str = "default";

/// 一套配色的界面行。
#[derive(Clone, Debug)]
pub struct ThemeRow {
    pub id: String,
    pub name: String,
    pub author: String,
    /// 是否当前生效。
    pub active: bool,
    /// 用户补丁里有没有改过这套（界面标"已改"用）。
    pub modified: bool,
    pub back: u32,
    pub border: u32,
    pub text: u32,
    pub hilited_back: u32,
    pub hilited_text: u32,
    pub comment: u32,
}

/// 配色页状态：出厂配置 + 用户补丁 + 补丁文件位置。
pub struct Themes {
    base: Option<WeaselConfig>,
    custom: Option<WeaselConfig>,
    custom_path: PathBuf,
    user_dir: PathBuf,
    base_path: PathBuf,
}

impl Themes {
    /// `shared_dir` 是程序文件夹里的数据目录（装机后是 `<版本>\data`，仓库里是 `Rime-Next`）；
    /// `user_dir` 是用户文件夹（`%APPDATA%\Rime-Next` 或安装时自选的那个）。
    pub fn load(shared_dir: &Path, user_dir: &Path) -> Self {
        let base_path = shared_dir
            .join("theme")
            .join(SKIN)
            .join(format!("{SKIN}.theme.yaml"));
        let custom_path = weasel_common::settings::user_config_path(user_dir);
        let mut themes = Self {
            base: None,
            custom: None,
            custom_path,
            user_dir: user_dir.to_path_buf(),
            base_path,
        };
        themes.reload();
        themes
    }

    /// 重新读两份文件（用户改过配置之后调用）。
    pub fn reload(&mut self) {
        // 程序文件夹由 `base_path` 反推（`<shared>/theme/<皮肤>/<皮肤>.theme.yaml`）。
        let script_dir = self
            .base_path
            .ancestors()
            .nth(3)
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let shared_dir = resolve_shared_dir(&script_dir, &self.base_path);
        let user_dir = self.user_dir.clone();
        self.base = weasel_style::theme::load_theme(&shared_dir, &user_dir, SKIN)
            .ok()
            .flatten();
        // 原始补丁单独保留：界面用它标记"改过哪些配色"，与合并结果无关。
        self.custom = weasel_style::parse_file(&self.custom_path).ok();
    }

    pub fn base_available(&self) -> bool {
        self.base.is_some()
    }

    /// 状态栏文字：配色数量、当前配色、文件位置。
    pub fn status(&self) -> String {
        match &self.base {
            Some(config) => {
                let count = weasel_style::settings::list_schemes(config, self.custom.as_ref()).len();
                format!(
                    "{} 套配色 · 当前 {} · 补丁{}",
                    count,
                    self.active_id(),
                    if self.custom.is_some() { "已生效" } else { "无" }
                )
            }
            None => format!("读不到出厂皮肤文件：{}", self.base_path.display()),
        }
    }

    /// 当前生效的配色 id：用户 `theme/theme.yaml` 的 `color_scheme` 优先，
    /// 回落到出厂配置的 `style/color_scheme`。
    pub fn active_id(&self) -> String {
        if let Some(id) = self.theme_color_scheme() {
            return id;
        }
        match &self.base {
            Some(base) => base.style.color_scheme.clone(),
            None => String::new(),
        }
    }

    fn theme_color_scheme(&self) -> Option<String> {
        let path = self
            .custom_path
            .parent()?
            .join("theme")
            .join("theme.yaml");
        let text = std::fs::read_to_string(path).ok()?;
        let table = weasel_common::yaml::to_value(&text).ok()?;
        table
            .get("color_scheme")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    }

    /// 组装配色行。出厂配置读不到时返回空表（界面应显示 `status()` 的原因）。
    pub fn rows(&self) -> Vec<ThemeRow> {
        let Some(base) = &self.base else {
            return Vec::new();
        };
        let active = self.active_id();
        weasel_style::settings::list_schemes(base, self.custom.as_ref())
            .into_iter()
            .map(|info| {
                // 用户补丁优先，其次出厂；与 librime 的补丁语义一致。
                let scheme = self
                    .custom
                    .as_ref()
                    .and_then(|c| c.schemes.get(&info.id))
                    .or_else(|| base.schemes.get(&info.id));
                let pick = |f: fn(&weasel_style::ColorScheme) -> u32, fallback: u32| {
                    scheme.map(f).filter(|v| *v != 0).unwrap_or(fallback)
                };
                ThemeRow {
                    id: info.id.clone(),
                    name: info.name.clone(),
                    author: info.author.clone(),
                    active: info.id == active,
                    modified: info.modified,
                    // 兜底色与 Slint 侧一致（aqua 实测：底 #eeeeec、选中块 #0a3afa）。
                    back: pick(|s| s.back_color, 0xff_ee_ee_ec),
                    border: pick(|s| s.border_color, 0xff_b0_b0_b0),
                    text: pick(|s| s.text_color, 0xff_00_00_00),
                    hilited_back: pick(|s| s.hilited_candidate_back_color, 0xff_0a_3a_fa),
                    hilited_text: pick(|s| s.hilited_candidate_text_color, 0xff_ff_ff_ff),
                    comment: pick(|s| s.comment_text_color, 0xff_88_88_88),
                }
            })
            .collect()
    }

    /// 切换配色：写 `theme/theme.yaml` 的 `color_scheme`，然后重读。
    pub fn select(&mut self, id: &str) -> Result<String, String> {
        if self.base.is_none() {
            return Err("读不到出厂皮肤文件，无法切换".into());
        }
        weasel_style::settings::set_active_scheme(&self.custom_path, id)
            .map_err(|error| format!("写入补丁失败：{error}"))?;
        self.reload();
        Ok(format!("已切换到 {id}"))
    }

    pub fn custom_path(&self) -> &Path {
        &self.custom_path
    }

    pub fn base_path(&self) -> &Path {
        &self.base_path
    }
}

/// 出厂皮肤文件所在的 `<shared>` 目录（数据目录）。
///
/// 依次尝试：
/// 1. 可执行文件旁边的 `Rime-Next` / `data`（**安装后的正常布局**）；
/// 2. 从 exe 目录逐级上溯找到的数据目录（**从 `target\release` 跑**的场景）；
/// 3. `%ProgramFiles%\Rime-Next\<版本>\data`（装了就能兜住）。
///
/// 只看 exe 旁边的话，从构建目录跑会全部落空，界面上就是"配色列表全空 / 方案 0 行"——
/// 与真正的故障长得一模一样（2026-10-17 踩过）。
pub fn resolve_shared_dir(script_dir: &Path, base_path: &Path) -> PathBuf {
    let probe = |dir: &Path| {
        dir.join("theme")
            .join(SKIN)
            .join(format!("{SKIN}.theme.yaml"))
            .is_file()
    };
    if probe(script_dir) {
        return script_dir.to_path_buf();
    }
    for ancestor in script_dir.ancestors().skip(1) {
        let candidate = weasel_common::rime_layout::data_dir(ancestor);
        if probe(&candidate) {
            return candidate;
        }
    }
    // 装机布局（2026-10-28 定）：`C:\Program Files\Rime-Next\Weasel-Next-<版本>\data`。
    if let Some(program_files) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
        let brand = program_files.join("Rime-Next");
        let mut roots = vec![brand.clone()];
        if let Ok(entries) = std::fs::read_dir(&brand) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    roots.push(entry.path());
                }
            }
        }
        // 旧装机品牌根是 `Weasel-Next`，过渡期两套都能找到。
        roots.push(program_files.join("Weasel-Next"));
        for root in roots {
            let candidate = weasel_common::rime_layout::data_dir(&root);
            if probe(&candidate) {
                return candidate;
            }
        }
    }
    script_dir.to_path_buf()
}

/// 从当前 exe 出发找数据目录（两个界面共用）。
pub fn shared_dir_for_exe() -> PathBuf {
    let script_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let probe = script_dir
        .join("theme")
        .join(SKIN)
        .join(format!("{SKIN}.theme.yaml"));
    resolve_shared_dir(&script_dir, &probe)
}

/// 用户文件夹：`userdir.txt` / `WEASEL_NEXT_USER_DIR` / `%APPDATA%\Rime-Next`，
/// 统一走 `weasel_common` 的解析（别在界面里自己拼 APPDATA）。
pub fn user_dir() -> PathBuf {
    weasel_common::process::user_data_dir().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_shared() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Rime-Next")
    }

    #[test]
    fn rows_come_from_official_skin() {
        let user = std::env::temp_dir().join("Weasel-Next-Setting-themes-test");
        let _ = std::fs::create_dir_all(&user);
        let _ = std::fs::remove_file(user.join("theme").join("theme.yaml"));
        let themes = Themes::load(&repo_shared(), &user);
        assert!(themes.base_available(), "应读到出厂皮肤 default.theme.yaml");
        let rows = themes.rows();
        assert!(rows.len() >= 36, "官方 36 套配色，实际 {}", rows.len());
        let aqua = rows.iter().find(|r| r.id == "aqua").expect("应有 aqua");
        // 颜色是内部 ARGB：aqua 实测底色 #eeeeec、选中块 #0a3afa。
        assert_eq!(aqua.back & 0x00ff_ffff, 0x00_ee_ee_ec, "aqua 底色");
        assert_eq!(
            aqua.hilited_back & 0x00ff_ffff,
            0x00_0a_3a_fa,
            "aqua 选中块（蓝，不是橙红）"
        );
    }

    #[test]
    fn selecting_writes_theme_yaml() {
        let user = std::env::temp_dir().join("Weasel-Next-Setting-themes-select");
        let _ = std::fs::create_dir_all(&user);
        let _ = std::fs::remove_file(user.join("theme").join("theme.yaml"));
        let mut themes = Themes::load(&repo_shared(), &user);
        themes.select("luna").expect("切换应成功");
        let text = std::fs::read_to_string(user.join("theme").join("theme.yaml")).unwrap();
        assert!(text.contains("color_scheme: \"luna\""), "应写 YAML：{text}");
        assert_eq!(themes.active_id(), "luna");
    }
}
