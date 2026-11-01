//! 皮肤页的 **Slint 适配层**。
//!
//! 真正的读写逻辑在 `weasel_settings::themes`（框架无关，D2D 外壳也用那一份）。
//! 这里只做两件事：把 [`ThemeRow`] 搬成 Slint 生成的 `SchemeRow`，并把
//! `select`/`status`/`active_id` 原样转发。
//!
//! 历史：这份文件原先自己实现了全部逻辑，拿不出给 D2D 用（返回值是 Slint 类型），
//! 2026-10-31 抽出 `themes.rs` 后变成薄壳——**别再把逻辑写回来**。

use std::path::{Path, PathBuf};

pub use weasel_settings::themes::resolve_shared_dir;
pub use weasel_settings::themes::SKIN;

use weasel_settings::themes::{ThemeRow, Themes};

use crate::SchemeRow;

/// 配色页状态：出厂配置 + 用户补丁 + 补丁文件位置。
pub struct Skins {
    inner: Themes,
}

impl Skins {
    /// `shared_dir` 是程序文件夹里的数据目录；`user_dir` 是用户文件夹。
    pub fn load(shared_dir: &Path, user_dir: &Path) -> Self {
        Self {
            inner: Themes::load(shared_dir, user_dir),
        }
    }

    /// 重新读两份文件（用户改过之后调用）。
    pub fn reload(&mut self) {
        self.inner.reload();
    }

    /// 出厂文件是否读到了。读不到时界面应给出提示而不是空白。
    pub fn base_available(&self) -> bool {
        self.inner.base_available()
    }

    /// 状态栏文字：文件位置与配色数量。
    pub fn status(&self) -> String {
        self.inner.status()
    }

    /// 当前生效的配色 id。
    pub fn active_id(&self) -> String {
        self.inner.active_id()
    }

    /// 组装界面行模型。
    pub fn rows(&self) -> Vec<SchemeRow> {
        self.inner.rows().into_iter().map(to_row).collect()
    }

    /// 切换配色：写 `theme/theme.yaml` 的 `color_scheme`，然后重读。
    pub fn select(&mut self, id: &str) -> Result<String, String> {
        self.inner.select(id)
    }

    /// 用户补丁文件的位置（界面显示用）。
    pub fn custom_path(&self) -> &Path {
        self.inner.custom_path()
    }

    /// 出厂皮肤文件的位置（界面显示 / 排查用）。
    pub fn base_path(&self) -> &Path {
        self.inner.base_path()
    }
}

fn to_row(row: ThemeRow) -> SchemeRow {
    SchemeRow {
        id: row.id.as_str().into(),
        name: row.name.as_str().into(),
        author: row.author.as_str().into(),
        active: row.active,
        modified: row.modified,
        back_color: to_slint_color(row.back),
        border_color: to_slint_color(row.border),
        text_color: to_slint_color(row.text),
        hilited_back_color: to_slint_color(row.hilited_back),
        hilited_text_color: to_slint_color(row.hilited_text),
        comment_color: to_slint_color(row.comment),
    }
}

/// 配色值 → 界面色。
///
/// ⚠️ 输入是**内部 ARGB**（`0xAARRGGBB`），不是官方文本里的 abgr 字形 ——
/// `style::color::parse` 已经把官方文本（默认 `abgr`，字形 `AABBGGRR`）**归一化**成
/// `(a<<24)|(r<<16)|(g<<8)|b` 了（见 `style/src/color.rs:96-101`）。
/// 渲染器就是这么解它的（`theme/src/paint.rs::d2d_color` 按 ARGB 取通道），
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
        let _ = std::fs::remove_file(user.join("weasel.yaml"));
        let skins = Skins::load(&repo_shared(), &user);
        assert!(skins.base_available(), "应读到出厂皮肤 theme/default/default.theme.yaml");
        assert_eq!(skins.base_path(), &repo_shared().join("theme/default/default.theme.yaml"));
        let rows = skins.rows();
        println!("{} 行，当前 {}", rows.len(), skins.active_id());
        println!("状态：{}", skins.status());
        // 官方 36 套 + 后来加的「星露谷／Stardew Valley」= 37（2026-10-24）。
        // 写成"至少 36"避免每加一套配色就红一次。
        assert!(rows.len() >= 36, "官方 36 套配色，实际 {}", rows.len());
        let aqua = rows.iter().find(|r| r.id == "aqua").expect("应有 aqua");
        assert!(aqua.name.as_str() == "碧水／Aqua");
        // 界面色必须等于"同一份配置走渲染器那条解码路"的结果，否则色块/预览会与
        // 候选窗实际画出来的颜色不一致。
        //
        // 这里**从工厂文件独立解析一遍**再按 ARGB 拆通道（= `paint::d2d_color` 的做法），
        // 而不是照抄这里的实现，免得两边一起错还互相"验证"通过。
        let factory = weasel_style::theme::parse_file(
            repo_shared().join("theme/default/default.theme.yaml"),
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
        let bogus_base = bogus.join("theme").join(SKIN).join(format!("{SKIN}.theme.yaml"));
        let resolved = resolve_shared_dir(bogus, &bogus_base);
        println!("解析出的 shared 目录：{}", resolved.display());
        assert!(resolved != bogus, "本机已安装，应退回到安装目录的数据目录");
        assert!(resolved.join("theme").join(SKIN).join(format!("{SKIN}.theme.yaml")).is_file());
    }

    #[test]
    fn selecting_writes_patch_and_marks_active() {
        let user = std::env::temp_dir().join("Weasel-Next-Setting-skins-select");
        let _ = std::fs::create_dir_all(&user);
        let _ = std::fs::remove_file(user.join("weasel.yaml"));
        let mut skins = Skins::load(&repo_shared(), &user);
        let message = skins.select("luna").expect("切换应成功");
        println!("{message}");
        // 配色选择写在用户目录的 theme/theme.yaml（`color_scheme` 一行），不写 weasel.yaml
        let text = std::fs::read_to_string(user.join("theme").join("theme.yaml")).unwrap();
        // 文件现在是 **YAML**（`key: "value"`）。2026-10-27 之前它是 TOML，而写回逻辑
        // 按 YAML 的 `key:` 去匹配 ⇒ 一个键都匹配不到、每次换皮肤都插一行非法 TOML
        // ⇒ 整份文件解析失败 ⇒ 主题回退内置默认值（候选窗背景纯白、无高亮）。
        assert!(text.contains("color_scheme: \"luna\""), "主题文件应写入 YAML：{text}");
        assert!(!text.contains("color_scheme = "), "不该再是 TOML 形式：{text}");
        assert_eq!(skins.active_id(), "luna", "重读后当前配色应为 luna");
        let rows = skins.rows();
        assert!(rows.iter().find(|r| r.id == "luna").unwrap().active);
        assert!(!rows.iter().find(|r| r.id == "aqua").unwrap().active);
        let _ = std::fs::remove_file(user.join("weasel.yaml"));
    }
}
