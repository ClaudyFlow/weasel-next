//! 从 `Rime-Next\shuangpin\<方案>.toml` 生成 rime 认的 `<方案>.schema.yaml`。
//!
//! 用法：
//!   cargo run --release -p weasel-style --bin shuangpin-schema
//!   cargo run --release -p weasel-style --bin shuangpin-schema -- Rime-Next\shuangpin\xiaohe.toml
//!
//! 为什么要有这一步：librime 的 `ResourceResolver({"schema_source_file", "", ".schema.yaml"})`
//! 写死了只找 `.schema.yaml`，它不解析 TOML。所以约定是两层——
//!   * `shuangpin\*.toml`            ← **人编辑的配置（唯一源）**
//!   * `schema\<id>\<id>.schema.yaml` ← 本程序生成的产物，别手改
//!
//! 设计要点：**algebra 的规则形态固定、键位字母从 TOML 取**。
//! 双拼的 xform 顺序是由 rime 的匹配语义决定的（长韵母必须先于短韵母，
//! 否则 `liang` 会被 `ang` 规则先命中变成 `lih`），不是随便排的。
//! 所以这里不"推导"顺序，只把 TOML 里的键位字母填进固定模板 ——
//! 改键位（比如把 an 从 j 挪走）能自动生效，又不会破坏匹配顺序。

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// TOML 顶层结构。
#[derive(Debug, Deserialize)]
struct Spec {
    schema: Meta,
    #[serde(default)]
    initials: BTreeMap<String, String>,
    finals: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct Meta {
    /// 方案 id，决定 `<id>.schema.yaml` 的文件名。
    id: String,
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    dict: Option<String>,
    #[serde(default)]
    prism: Option<String>,
    #[serde(default)]
    simp_option: Option<String>,
    /// 继承哪个基础方案（用 `__include` 拉它的骨架，再 `patch` 覆盖差异）。
    ///
    /// 这样生成的方案里**不重复** engine / switches / segmentors / translators 那一大坨，
    /// 只写双拼真正需要改的东西（algebra、prism、preedit_format）。
    #[serde(default)]
    base: Option<String>,
}

/// (正则片段, 捕获组前缀, 韵母键名)
///
/// 顺序 = 官方 `rime-double-pinyin` 的顺序，**不可乱**：长韵母必须排在短的前面。
/// 有些条目共用一键（`ue`/`ve` → 同一个键、`ong`/`iong` → 同一个键），
/// 所以正则里用字符类（`[uv]e`、`i?ong`）把它们合并，键位从 TOML 取。
const ALGEBRA_RULES: &[(&str, &str, &str)] = &[
    (r"iu$", "", "iu"),
    (r"(.)ei$", "$1", "ei"),
    (r"uan$", "", "uan"),
    (r"[uv]e$", "", "ue"),   // ue / ve 同键
    (r"un$", "", "un"),
    (r"uo$", "", "uo"),
    (r"ie$", "", "ie"),
    (r"i?ong$", "", "ong"),  // ong / iong 同键
    (r"ing$|uai$", "", "ing"), // ing / uai 同键
    (r"(.)ai$", "$1", "ai"),
    (r"(.)en$", "$1", "en"),
    (r"(.)eng$", "$1", "eng"),
    (r"[iu]ang$", "", "iang"), // iang / uang 同键
    (r"(.)ang$", "$1", "ang"),
    (r"ian$", "", "ian"),
    (r"(.)an$", "$1", "an"),
    (r"(.)ou$", "$1", "ou"),
    (r"[iu]a$", "", "ia"),   // ia / ua 同键
    (r"iao$", "", "iao"),
    (r"(.)ao$", "$1", "ao"),
    (r"ui$", "", "ui"),      // ui / ü 同键
    (r"in$", "", "in"),
];

/// 小鹤的 `preedit_format`（把双拼键还原成完整拼音，显示在预编辑行）。
///
/// 这份是**小鹤键位专用**的固定模板：同键多韵母（如 `s` = ong/iong）
/// 靠上下文判断（`([jqx])s` 优先 iong，其余 ong），没法从键位表自动推导。
/// 换成别的键位方案时，这段需要按该方案的规则重写。
const PREEDIT_FLYPY: &[&str] = &[
    r"xform/([bpmfdtnljqx])n/$1iao/",
    r"xform/(\w)g/$1eng/",
    r"xform/(\w)q/$1iu/",
    r"xform/(\w)w/$1ei/",
    r"xform/([dtnlgkhjqxyvuirzcs])r/$1uan/",
    r"xform/(\w)t/$1ve/",
    r"xform/(\w)y/$1un/",
    r"xform/([dtnlgkhvuirzcs])o/$1uo/",
    r"xform/(\w)p/$1ie/",
    r"xform/([jqx])s/$1iong/",
    r"xform/(\w)s/$1ong/",
    r"xform/(\w)d/$1ai/",
    r"xform/(\w)f/$1en/",
    r"xform/(\w)h/$1ang/",
    r"xform/(\w)j/$1an/",
    r"xform/([gkhvuirzcs])k/$1uai/",
    r"xform/(\w)k/$1ing/",
    r"xform/([jqxnl])l/$1iang/",
    r"xform/(\w)l/$1uang/",
    r"xform/(\w)z/$1ou/",
    r"xform/([gkhvuirzcs])x/$1ua/",
    r"xform/(\w)x/$1ia/",
    r"xform/(\w)c/$1ao/",
    r"xform/([dtgkhvuirzcs])v/$1ui/",
    r"xform/(\w)b/$1in/",
    r"xform/(\w)m/$1ian/",
    r"xform/([aoe])\1(\w)/$1$2/",
    r"xform/(^|[ '])v/$1zh/",
    r"xform/(^|[ '])i/$1ch/",
    r"xform/(^|[ '])u/$1sh/",
    r"xform/([jqxy])v/$1u/",
    r"xform/([nl])v/$1ü/",
];

fn build_algebra(spec: &Spec) -> Vec<String> {
    let mut lines = Vec::new();

    // ① jqxy + u → v（官方放最前：jue/juan 要先变 jve/jvan，才走得到韵母规则）
    lines.push(r"derive/^([jqxy])u$/$1v/".to_string());

    // ② 双字母声母改成大写占位，避免被韵母规则误伤
    for initial in ["zh", "ch", "sh"] {
        if let Some(key) = spec.initials.get(initial) {
            if let Some(letter) = key.chars().next() {
                lines.push(format!(
                    "xform/^{initial}/{}/",
                    letter.to_ascii_uppercase()
                ));
            }
        }
    }

    // ③ 零声母（必须早于韵母规则）
    lines.push(r"xform/^([aoe])(ng)?$/$1$1$2/".to_string());
    lines.push(r"derive/^([aoe])([ioun])$/$1$1$2/".to_string());

    // ④ 韵母（顺序固定，长的在前）
    for (pattern, capture, final_key) in ALGEBRA_RULES {
        let Some(key) = spec.finals.get(*final_key) else {
            continue;
        };
        let Some(letter) = key.chars().next() else {
            continue;
        };
        lines.push(format!(
            "xform/{pattern}/{capture}{}/",
            letter.to_ascii_uppercase()
        ));
    }

    // ⑤ 大写中间标记统一转小写
    lines.push("xlit/QWRTYUIOPSDFGHJKLZXCVBNM/qwrtyuiopsdfghjklzxcvbnm/".to_string());

    // ★ 简拼（abbrev）不在双拼里加 —— 2026-10-18 实测结论，别再加回来：
    //   双拼的每个音节正好 2 个字母，所以任意两字母（如 "nh"）本身就是**合法全码**
    //   （n + ang → 囊）。用 `abbrev/^([a-z]).+$/$1/` 派生出的单字母拼写会和全码抢同一个
    //   输入串：打 "nh" 出来的是「囊」而不是「你好」（实测候选前五全是 nang 音的字）。
    //   简拼因此只在**全拼**（luna_pinyin_simp）里成立，那边首字母不冲突。
    //   双拼这边只接语言模型（见下方 __patch），不做简拼。
    lines
}

/// 生成**极简 patch 方案**：用 `__include` 拉基础方案的骨架，再 `patch` 覆盖差异。
///
/// 为什么不用"完整方案"：那样要把 engine / switches / segmentors / translators
/// 那一大坨连同注释整份复制过来，改一处基础方案就得重新生成全部，
/// 而且文件里 90% 的内容和 `luna_pinyin_simp` 一模一样。
/// rime 原生支持 `__include` + `patch`，只写差异才是对的做法。
fn render(spec: &Spec, source: &str) -> String {
    let meta = &spec.schema;
    let dict = meta.dict.as_deref().unwrap_or("luna_pinyin");
    let prism = meta.prism.as_deref().unwrap_or(&meta.id);
    let simp = meta.simp_option.as_deref().unwrap_or("zh_simp");
    let version = meta.version.as_deref().unwrap_or("0.18");
    let author = meta.author.as_deref().unwrap_or("");
    let base = meta.base.as_deref().unwrap_or("luna_pinyin_simp");

    let algebra = build_algebra(spec)
        .iter()
        .map(|line| format!("      - {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let preedit = PREEDIT_FLYPY
        .iter()
        .map(|line| format!("      - {line}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"# Rime schema
# encoding: utf-8
#
# ★ 本文件由 `shuangpin-schema` 从 `Rime-Next\schema\shuangpin\{source}` 生成。
#   **不要手改**：改键位请改那份 TOML 再重新生成。
#
# 用 rime 的 **`__include` + `patch`** 继承 `{base}` 的骨架（engine / switches /
# 用 rime 的 **`__include` 深合并**继承 `{base}` 的骨架（engine / switches /
# segmentors / translators 全部继承过来），下面**直接写要覆盖的字段**即可。
#
# ⚠️ 这里**不能**用 `patch:` —— 那是 `.custom.yaml` 专用语法，
# 写在 `.schema.yaml` 里会被 rime 判为 `invalid schema definition` 而部署失败。
# `__include` 的语义是深合并：映射递归合并、列表整体替换、标量覆盖。

__include: {base}.schema:/

schema:
  schema_id: {id}
  name: {name}
  version: "{version}"
  author:
    - {author}
  description: |
    {name}（明月拼音 + 双拼键位）。

speller:
  algebra:
{algebra}

translator:
  dictionary: {dict}
  prism: {prism}
  preedit_format:
{preedit}

simplifier:
  option_name: {simp}

# 语言模型（grammar.yaml 的 hans 预设）：双拼也是简体，用 hans 模型调序。
# 找不到 grammar.yaml 时 `?` 让它静默跳过，不影响部署。
__patch:
  - grammar:/hans?
"#,
        source = source,
        base = base,
        id = meta.id,
        name = meta.name,
        version = version,
        author = author,
        dict = dict,
        prism = prism,
        simp = simp,
        algebra = algebra,
        preedit = preedit,
    )
}

/// 找到"数据根目录"（含 `Rime-Next\` 的那个）。
///
/// ⚠️ **不能用 `env!("CARGO_MANIFEST_DIR")`**：那是**编译期常量**，装到别的机器上会指向
/// 开发机的 `C:\Code\Weasel-Next\style`（不存在），程序直接失效。
/// 所以这里用运行时路径：从 exe 所在目录往上找，哪个目录下有 `Rime-Next\` 就是根。
///
/// 开发时：`target\release\shuangpin-schema.exe` → 上溯到 `<repo>\` ✓
/// 安装后：`C:\Program Files\Weasel-Next\shuangpin-schema.exe` → 自己就是根 ✓
fn install_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // ancestors 第一个是 exe 自己，从它的父目录开始找
    for dir in exe.ancestors().skip(1) {
        if dir.join("Rime-Next").is_dir() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// 编译期回退（只在开发机上用得到，作为最后兜底）。
fn source_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// 用户数据目录：`%APPDATA%\Rime-Next`。
fn user_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join("Rime-Next"))
}

/// TOML 的搜索目录，**用户目录优先**（用户改的那份要生效）。
///
/// 搜索顺序（先找到的先赢，同名去重）：
///   1. `%APPDATA%\Rime-Next\schema\shuangpin\`  ← 用户把 shuangpin 当"方案的一部分"放这里
///   2. `%APPDATA%\Rime-Next\shuangpin\`          ← 旧位置，兼容
///   3. `<exe目录>\Rime-Next\schema\shuangpin\`   ← 程序文件夹（同样的 schema 内布局）
///   4. `<exe目录>\Rime-Next\shuangpin\`          ← 程序文件夹旧位置
///   5. `<repo>\Rime-Next\shuangpin\`             ← 开发时兜底
///
/// 为什么 `schema\shuangpin\` 在列表里优先：用户明确要求"shuangpin 是方案的一部分"，
/// 所以它和 `schema\<方案名>\` 平级放在 `schema\` 下；这份必须是**可改且优先**的。
fn shuangpin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut push = |dir: PathBuf| {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    };

    if let Some(user) = user_dir() {
        push(user.join("schema").join("shuangpin"));
        push(user.join("shuangpin"));
    }
    if let Some(root) = install_root() {
        let data = root.join("Rime-Next");
        push(data.join("schema").join("shuangpin"));
        push(data.join("shuangpin"));
    }
    push(source_root().join("Rime-Next").join("schema").join("shuangpin"));
    dirs
}

/// 生成物的输出目录：**用户目录优先**（放程序文件夹用户改不了）。
fn output_base() -> PathBuf {
    if let Some(user) = user_dir() {
        return user.join("schema");
    }
    if let Some(root) = install_root() {
        return root.join("Rime-Next").join("schema");
    }
    source_root().join("Rime-Next").join("schema")
}

/// 列出所有待生成的 TOML（同名时用户目录那份优先，用 seen 去重）。
fn collect_targets() -> Vec<PathBuf> {
    let mut seen = std::collections::BTreeSet::new();
    let mut list = Vec::new();
    for dir in shuangpin_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "toml") {
                continue;
            }
            let Some(name) = path.file_name().map(|n| n.to_os_string()) else {
                continue;
            };
            // 用户目录先扫到，所以同名时保留用户那份
            if seen.insert(name) {
                list.push(path);
            }
        }
    }
    list.sort();
    list
}

fn generate(toml_path: &Path) -> Result<PathBuf, String> {
    let text = std::fs::read_to_string(toml_path)
        .map_err(|error| format!("读取 {} 失败: {error}", toml_path.display()))?;
    let spec: Spec = toml::from_str(&text)
        .map_err(|error| format!("解析 {} 失败: {error}", toml_path.display()))?;

    let source = toml_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();

    // 把实际读到的内容打出来 —— 免得"到底读没读 TOML"这种事只能靠猜。
    println!("  读取 {}", toml_path.display());
    println!(
        "    [initials] {}",
        spec.initials
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!(
        "    [finals] {} 项，其中 an={} iang={} iu={}",
        spec.finals.len(),
        spec.finals.get("an").map(String::as_str).unwrap_or("-"),
        spec.finals.get("iang").map(String::as_str).unwrap_or("-"),
        spec.finals.get("iu").map(String::as_str).unwrap_or("-"),
    );
    let rendered = render(&spec, &source);
    let out_dir = output_base().join(&spec.schema.id);
    std::fs::create_dir_all(&out_dir)
        .map_err(|error| format!("创建 {} 失败: {error}", out_dir.display()))?;
    let out_path = out_dir.join(format!("{}.schema.yaml", spec.schema.id));
    std::fs::write(&out_path, rendered)
        .map_err(|error| format!("写入 {} 失败: {error}", out_path.display()))?;
    Ok(out_path)
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let targets: Vec<PathBuf> = if args.is_empty() {
        collect_targets()
    } else {
        args.iter().map(PathBuf::from).collect()
    };

    if targets.is_empty() {
        eprintln!("没有找到任何 shuangpin\\*.toml，搜索过：");
        for dir in shuangpin_dirs() {
            eprintln!("  {}", dir.display());
        }
        return std::process::ExitCode::FAILURE;
    }

    let mut failed = false;
    for toml_path in targets {
        match generate(&toml_path) {
            Ok(out) => println!("✓ {} → {}", toml_path.display(), out.display()),
            Err(error) => {
                eprintln!("✗ {error}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}



