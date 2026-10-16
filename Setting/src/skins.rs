//! 皮肤页的数据接线：把官方 `weasel.toml` / `weasel.custom.toml` 读成界面可用的行，
//! 并把用户的选择写回官方的 `patch:` 形式。
//!
//! 分工照官方：读写全部落在 [`weasel_style::settings`]，本模块只做三件事——
//! 找文件、把颜色转成界面类型、把结果组装成 Slint 行模型。

use std::path::{Path, PathBuf};

use weasel_style::WeaselConfig;

use crate::SchemeRow;

/// 出厂皮肤名。与渲染器一致（`theme\official\src\config.rs::SKIN`）：
/// 皮肤文件是 `Rime-Next\theme\default\default.theme.toml`，里面是**全部 36 套配色**。
const SKIN: &str = "default";

/// 皮肤页的状态：出厂配置 + 用户补丁 + 补丁文件位置。
pub struct Skins {
    /// 出厂配置（程序文件夹的 `Rime-Next\weasel.toml`）。
    base: Option<WeaselConfig>,
    /// 用户补丁（`%APPDATA%\Rime-Next\weasel.custom.toml`）。
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
    ///
    /// ⚠️ 出厂皮肤现在是**主题链**（TOML），不再是 YAML 时代那个 `Rime-Next\weasel.toml` ——
    /// 后者在 TOML 化改造中已经不在这个位置了。继续读它会让 `base` 恒为 `None`，
    /// 界面上就是**配色下拉框全空**（皮肤识别不了）。现在与渲染器走**同一份**数据：
    /// `theme\theme.toml`（用户优先）+ `theme\<皮肤名>\<皮肤名>.theme.toml`。
    pub fn load(shared_dir: &Path, user_dir: &Path) -> Self {
        let base_path = shared_dir
            .join("theme")
            .join(SKIN)
            .join(format!("{SKIN}.theme.toml"));
        let custom_path = user_dir.join("weasel.custom.toml");
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
        // 出厂皮肤 + 用户布局合成一份配置：直接复用主题链的读取与合并逻辑
        // （`theme::load_theme` = `theme.toml` 叠加 `<皮肤>\<皮肤>.theme.toml`），
        // 保证**设置界面看到的配色就是渲染器实际会画的配色**，不再是第二套数据源。
        //
        // 程序文件夹由 `base_path` 反推（`<shared>\theme\<皮肤>\<皮肤>.theme.toml`）。
        let script_dir = self
            .base_path
            .ancestors()
            .nth(3)
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let shared_dir = resolve_shared_dir(&script_dir, &self.base_path);
        let user_dir = self
            .custom_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        self.base = weasel_style::theme::load_theme(&shared_dir, &user_dir, SKIN)
            .ok()
            .flatten();
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
    ///
    /// 先说用户目录 `theme/theme.toml` 的 `color_scheme`（现在的唯一落点），
    /// 没有再回落到官方 YAML（出厂 `weasel.toml` + `weasel.custom.toml` 补丁）的
    /// `style/color_scheme`。不能把原始补丁交给 active_scheme —— 单独解析补丁文件时
    /// 读到的只是默认值。
    pub fn active_id(&self) -> String {
        if let Some(id) = self.theme_color_scheme() {
            return id;
        }
        match &self.base {
            Some(base) => base.style.color_scheme.clone(),
            None => String::new(),
        }
    }

    /// 用户 `theme/theme.toml` 里的 `color_scheme`（没有文件或没有这个键时返回 `None`）。
    fn theme_color_scheme(&self) -> Option<String> {
        let path = self
            .custom_path
            .parent()?
            .join("theme")
            .join("theme.toml");
        let text = std::fs::read_to_string(path).ok()?;
        let table: toml::Value = toml::from_str(&text).ok()?;
        table
            .get("color_scheme")
            .and_then(toml::Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
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
                // 兜底色 = 对应方案值被 `to_slint_color`（ARGB）解出来的结果，
                // 所以这里直接写 RGB 分量，别再按 abgr 折算一遍。
                SchemeRow {
                    id: info.id.as_str().into(),
                    name: info.name.as_str().into(),
                    author: info.author.as_str().into(),
                    active: info.id == active,
                    modified: info.modified,
                    // aqua back_color `0xeceeee` → RGB(ee,ee,ec)
                    back_color: color(|s| s.back_color, slint::Color::from_rgb_u8(0xee, 0xee, 0xec)),
                    border_color: color(|s| s.border_color, slint::Color::from_rgb_u8(0xb0, 0xb0, 0xb0)),
                    text_color: color(|s| s.text_color, slint::Color::from_rgb_u8(0x00, 0x00, 0x00)),
                    // aqua hilited_candidate_back_color `0xfa3a0a` → RGB(fa,3a,0a)
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

    /// 出厂皮肤文件的位置（界面显示 / 排查用）。
    pub fn base_path(&self) -> &Path {
        &self.base_path
    }
}

/// 出厂皮肤文件所在的 `<shared>` 目录。
///
/// 依次尝试：
/// 1. 可执行文件旁边的 `Rime-Next`（**安装后的正常布局**，exe 与数据同级）；
/// 2. 从 exe 目录逐级上溯找到的 `Rime-Next`（**从构建目录跑的场景**：
///    `target\release` → 仓库根的 `Rime-Next`）；
/// 3. `%ProgramFiles%\Weasel-Next\Rime-Next`（装了就能兜住）。
///
/// 为什么要上溯：只看 exe 旁边时，从 `target\release` 跑会读到 `None`，
/// 界面上就是"配色列表全空 / 方案列表 0 行"——与真正的故障长得一模一样，极易误判
/// （2026-10-17 实际踩到：误以为是数据库/配置坏了，其实是找错目录）。
pub fn resolve_shared_dir(script_dir: &Path, base_path: &Path) -> PathBuf {
    let probe = |dir: &Path| dir.join("theme").join(SKIN).join(format!("{SKIN}.theme.toml")).is_file();
    if probe(script_dir) {
        return script_dir.to_path_buf();
    }
    for ancestor in script_dir.ancestors().skip(1) {
        let candidate = ancestor.join("Rime-Next");
        if probe(&candidate) {
            return candidate;
        }
    }
    if let Some(installed) = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .map(|base| base.join("Weasel-Next").join("Rime-Next"))
    {
        if probe(&installed) {
            return installed;
        }
    }
    script_dir.to_path_buf()
}

/// 配色值 → 界面色。
///
/// ⚠️ 输入是**内部 ARGB**（`0xAARRGGBB`），不是官方文本里的 abgr 字形 ——
/// `style::color::parse` 已经把官方文本（默认 `abgr`，字形 `AABBGGRR`）**归一化**成
/// `(a<<24)|(r<<16)|(g<<8)|b` 了（见 `style/src/color.rs:96-101`）。
/// 渲染器就是这么解它的（`theme/official/src/paint.rs::d2d_color` 按 ARGB 取通道），
/// 所以这里**必须同样是 ARGB**，两边才画得一个颜色。
///
/// 反例（2026-10-17 踩过）：若在这里再按 abgr 解一遍，等于**多翻一次通道**，
/// 界面色块会把 R 和 B 弄反（aqua 的蓝 `#0a3afa` 显示成橙红 `#fa3a0a`）。
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
        let _ = std::fs::remove_file(user.join("weasel.custom.toml"));
        let skins = Skins::load(&repo_shared(), &user);
        assert!(skins.base_available(), "应读到出厂皮肤 theme/default/default.theme.toml");
        assert_eq!(skins.base_path(), &repo_shared().join("theme/default/default.theme.toml"));
        let rows = skins.rows();
        println!("{} 行，当前 {}", rows.len(), skins.active_id());
        println!("状态：{}", skins.status());
        assert_eq!(rows.len(), 36, "官方 36 套配色");
        assert_eq!(skins.active_id(), "aqua");
        let aqua = rows.iter().find(|r| r.id == "aqua").expect("应有 aqua");
        assert!(aqua.active, "aqua 应是当前生效的一套");
        assert_eq!(aqua.name, "碧水／Aqua");
        // 界面色必须等于"同一份配置走渲染器那条解码路"的结果，否则色块/预览会与
        // 候选窗实际画出来的颜色不一致。
        //
        // 这里**从工厂文件独立解析一遍**再按 ARGB 拆通道（= `paint::d2d_color` 的做法），
        // 而不是照抄这里的实现，免得两边一起错还互相"验证"通过。
        //
        // 实测锚点（2026-10-17，截候选窗采样）：aqua 面板底色 = `#eeeeec`、
        // 选中块 = `#0a3afa`。所以 aqua 的 back_color 解出来必须是 (ee,ee,ec)，
        // hilited_candidate_back_color 必须是 (0a,3a,fa)（**蓝**，不是橙红 —— 官方文本
        // 里写的是 abgr 字形 `0xfa3a0a`，归一化之后才是这个）。
        let factory = weasel_style::theme::parse_file(
            repo_shared().join("theme/default/default.theme.toml"),
        )
        .expect("出厂皮肤必须可解析");
        let raw = factory
            .schemes
            .get("aqua")
            .expect("应有 aqua 段")
            .back_color
            .clone()
            .expect("aqua 应有 back_color");
        let parsed = weasel_style::color::parse(&raw, weasel_style::ColorFormat::Abgr)
            .expect("back_color 应能解析");
        let independent = slint::Color::from_rgb_u8(
            ((parsed >> 16) & 0xff) as u8,
            ((parsed >> 8) & 0xff) as u8,
            (parsed & 0xff) as u8,
        );
        assert_eq!(aqua.back_color, independent, "界面底色应与渲染器解码一致");
        assert_eq!(aqua.back_color, slint::Color::from_rgb_u8(0xee, 0xee, 0xec), "= 实测 #eeeeec");
        assert_eq!(
            aqua.hilited_back_color,
            slint::Color::from_rgb_u8(0x0a, 0x3a, 0xfa),
            "= 实测选中块 #0a3afa（蓝）"
        );
    }

    /// 回归：从构建目录跑时 exe 旁边没有 `Rime-Next`，必须能退回安装目录，
    /// 否则 `base` 恒为 `None`、配色列表全空（与真正的故障长得一样，极易误判）。
    #[test]
    fn falls_back_to_program_files_when_exe_dir_has_no_rime_next() {
        let bogus = std::path::Path::new(r"C:\__no_such_dir__\release");
        let bogus_base = bogus.join("theme").join(SKIN).join(format!("{SKIN}.theme.toml"));
        let resolved = resolve_shared_dir(bogus, &bogus_base);
        // 本机装了 Weasel-Next 时应退回安装目录；没装则原样返回（不 panic）
        println!("解析出的 shared 目录：{}", resolved.display());
        assert!(resolved != bogus, "本机已安装，应退回到安装目录的 Rime-Next");
        assert!(resolved.join("theme").join(SKIN).join(format!("{SKIN}.theme.toml")).is_file());
    }

    #[test]
    fn selecting_writes_patch_and_marks_active() {
        let user = std::env::temp_dir().join("Weasel-Next-Setting-skins-select");
        let _ = std::fs::create_dir_all(&user);
        let _ = std::fs::remove_file(user.join("weasel.custom.toml"));
        let mut skins = Skins::load(&repo_shared(), &user);
        let message = skins.select("luna").expect("切换应成功");
        println!("{message}");
        // 配色选择写在用户目录的 theme/theme.toml（`color_scheme` 一行），不再写 YAML
        let text = std::fs::read_to_string(user.join("theme").join("theme.toml")).unwrap();
        // 文件是 **TOML**（`key = "value"`）：曾经按 YAML 的 `key:` 匹配，写进去的
        // `color_scheme: luna` 是非法 TOML ⇒ 整份 theme.toml 解析失败 ⇒ 主题回退
        // 内置默认值（候选窗背景纯白、无高亮）。
        assert!(text.contains("color_scheme = \"luna\""), "主题文件应写入：{text}");
        assert!(!text.contains("color_scheme: luna"), "不该写入 YAML 形式：{text}");
        assert_eq!(skins.active_id(), "luna", "重读后当前配色应为 luna");
        let rows = skins.rows();
        assert!(rows.iter().find(|r| r.id == "luna").unwrap().active);
        assert!(!rows.iter().find(|r| r.id == "aqua").unwrap().active);
        let _ = std::fs::remove_file(user.join("weasel.custom.toml"));
    }
}
