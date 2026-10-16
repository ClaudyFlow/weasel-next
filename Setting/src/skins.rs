//! 皮肤页的数据接线：把官方 `weasel.yaml` / `weasel.custom.yaml` 读成界面可用的行，
//! 并把用户的选择写回官方的 `patch:` 形式。
//!
//! 分工照官方：读写全部落在 [`weasel_style::settings`]，本模块只做三件事——
//! 找文件、把颜色转成界面类型、把结果组装成 Slint 行模型。

use std::path::{Path, PathBuf};

use weasel_style::WeaselConfig;

use crate::SchemeRow;

/// 皮肤页的状态：出厂配置 + 用户补丁 + 补丁文件位置。
pub struct Skins {
    /// 出厂配置（程序文件夹的 `Rime-Next\weasel.yaml`）。
    base: Option<WeaselConfig>,
    /// 用户补丁（`%APPDATA%\Rime-Next\weasel.custom.yaml`）。
    custom: Option<WeaselConfig>,
    /// 用户补丁文件的路径；写入时用它。
    custom_path: PathBuf,
    /// 出厂文件位置（状态栏显示用）。
    base_path: PathBuf,
}

impl Skins {
    /// 按 librime 的查找顺序定位两份文件。
    ///
    /// `shared_dir` 是程序文件夹里的 `Rime-Next`；用户目录取自 `%APPDATA%\Rime-Next`。
    pub fn load(shared_dir: &Path, user_dir: &Path) -> Self {
        let base_path = shared_dir.join("weasel.yaml");
        let custom_path = user_dir.join("weasel.custom.yaml");
        let mut skins = Self {
            base: None,
            custom: None,
            custom_path,
            base_path,
        };
        skins.reload();
        skins
    }

    /// 重新读两份文件（用户改过之后调用）。
    pub fn reload(&mut self) {
        // 用户文件是 `patch:` 段，必须**叠加**到出厂配置上才是生效结果；
        // 直接把补丁当完整配置解析会读不到 style/color_scheme（补丁里没写的东西都不存在）。
        // 用文本合并（parse_with_patch）而不是 parse_files：前者与 style 里"官方基底 + 用户补丁"
        // 的测试路径完全一致，行为已被验证。
        let base_text = std::fs::read_to_string(&self.base_path).ok();
        let custom_text = std::fs::read_to_string(&self.custom_path).ok();
        self.base = match base_text {
            Some(base) => weasel_style::parse_with_patch(&base, custom_text.as_deref()).ok(),
            None => None,
        };
        // 原始补丁单独保留：界面用它标记"改过哪些配色"，与合并结果无关。
        self.custom = weasel_style::parse_file(&self.custom_path).ok();
    }

    /// 出厂文件是否读到了。读不到时界面应给出提示而不是空白。
    pub fn base_available(&self) -> bool {
        self.base.is_some()
    }

    /// 状态栏文字：文件位置与配色数量。
    pub fn status(&self) -> String {
        match &self.base {
            Some(config) => {
                let count = weasel_style::settings::list_schemes(config, self.custom.as_ref()).len();
                let active = self.active_id();
                format!(
                    "{} 套配色 · 当前 {} · 出厂 {} · 补丁 {}",
                    count,
                    active,
                    self.base_path.display(),
                    if self.custom.is_some() { "已生效" } else { "无" }
                )
            }
            None => format!("读不到出厂皮肤文件：{}", self.base_path.display()),
        }
    }

    /// 当前生效的配色 id。
    pub fn active_id(&self) -> String {
        // 合并结果就在 self.base 里（出厂 + 用户补丁），直接读它的 style/color_scheme。
        // 不能把原始补丁交给 active_scheme —— 单独解析补丁文件时读到的只是默认值。
        match &self.base {
            Some(base) => base.style.color_scheme.clone(),
            None => String::new(),
        }
    }

    /// 组装界面行模型。
    pub fn rows(&self) -> Vec<SchemeRow> {
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
                let color = |pick: fn(&weasel_style::ColorScheme) -> u32, fallback: slint::Color| {
                    scheme.map(pick).map(to_slint_color).unwrap_or(fallback)
                };
                SchemeRow {
                    id: info.id.as_str().into(),
                    name: info.name.as_str().into(),
                    author: info.author.as_str().into(),
                    active: info.id == active,
                    modified: info.modified,
                    back_color: color(|s| s.back_color, slint::Color::from_rgb_u8(0xec, 0xee, 0xee)),
                    border_color: color(|s| s.border_color, slint::Color::from_rgb_u8(0xb0, 0xb0, 0xb0)),
                    text_color: color(|s| s.text_color, slint::Color::from_rgb_u8(0, 0, 0)),
                    hilited_back_color: color(
                        |s| s.hilited_candidate_back_color,
                        slint::Color::from_rgb_u8(0xfa, 0x3a, 0x0a),
                    ),
                    hilited_text_color: color(
                        |s| s.hilited_candidate_text_color,
                        slint::Color::from_rgb_u8(0xff, 0xff, 0xff),
                    ),
                    comment_color: color(
                        |s| s.comment_text_color,
                        slint::Color::from_rgb_u8(0x88, 0x88, 0x88),
                    ),
                }
            })
            .collect()
    }

    /// 切换配色：写 `patch: style/color_scheme: <id>`，然后重读。
    ///
    /// 出厂文件不动；失败时返回给界面显示的原因。
    pub fn select(&mut self, id: &str) -> Result<String, String> {
        if self.base.is_none() {
            return Err("读不到出厂皮肤文件，无法切换".into());
        }
        weasel_style::settings::set_active_scheme(&self.custom_path, id)
            .map_err(|error| format!("写入补丁失败：{error}"))?;
        self.reload();
        Ok(format!("已切换到 {}（写入 {}）", id, self.custom_path.display()))
    }

    /// 用户补丁文件的位置（界面显示用）。
    pub fn custom_path(&self) -> &Path {
        &self.custom_path
    }
}

/// 官方颜色是 `0xAARRGGBB`，界面只用 RGB；alpha 为 0 时按不透明处理。
fn to_slint_color(value: u32) -> slint::Color {
    let r = ((value >> 16) & 0xff) as u8;
    let g = ((value >> 8) & 0xff) as u8;
    let b = (value & 0xff) as u8;
    slint::Color::from_rgb_u8(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 出厂文件在仓库里，用户目录用一个临时目录，互不干扰。
    fn repo_shared() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Rime-Next")
    }

    #[test]
    fn rows_come_from_official_skin() {
        let user = std::env::temp_dir().join("Weasel-Next-Setting-skins-test");
        let _ = std::fs::create_dir_all(&user);
        let _ = std::fs::remove_file(user.join("weasel.custom.yaml"));
        let skins = Skins::load(&repo_shared(), &user);
        assert!(skins.base_available(), "应读到出厂 weasel.yaml");
        let rows = skins.rows();
        println!("{} 行，当前 {}", rows.len(), skins.active_id());
        println!("状态：{}", skins.status());
        assert_eq!(rows.len(), 36, "官方 36 套配色");
        assert_eq!(skins.active_id(), "aqua");
        let aqua = rows.iter().find(|r| r.id == "aqua").expect("应有 aqua");
        assert!(aqua.active, "aqua 应是当前生效的一套");
        assert_eq!(aqua.name, "碧水／Aqua");
        assert_eq!(aqua.back_color, slint::Color::from_rgb_u8(0xec, 0xee, 0xee));
        assert_eq!(aqua.hilited_back_color, slint::Color::from_rgb_u8(0xfa, 0x3a, 0x0a));
    }

    #[test]
    fn selecting_writes_patch_and_marks_active() {
        let user = std::env::temp_dir().join("Weasel-Next-Setting-skins-select");
        let _ = std::fs::create_dir_all(&user);
        let _ = std::fs::remove_file(user.join("weasel.custom.yaml"));
        let mut skins = Skins::load(&repo_shared(), &user);
        let message = skins.select("luna").expect("切换应成功");
        println!("{message}");
        let text = std::fs::read_to_string(user.join("weasel.custom.yaml")).unwrap();
        assert!(text.contains("style/color_scheme: luna"), "补丁应写入：{text}");
        assert_eq!(skins.active_id(), "luna", "重读后当前配色应为 luna");
        let rows = skins.rows();
        assert!(rows.iter().find(|r| r.id == "luna").unwrap().active);
        assert!(!rows.iter().find(|r| r.id == "aqua").unwrap().active);
        let _ = std::fs::remove_file(user.join("weasel.custom.yaml"));
    }
}
