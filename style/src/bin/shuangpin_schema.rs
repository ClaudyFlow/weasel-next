//! 从 `Rime-Next\parser\shuangpin\<方案>.parser.yaml` 生成 rime 认的 `<方案>.schema.yaml`。
//!
//! 用法：
//!   cargo run --release -p weasel-style --bin shuangpin-schema
//!   cargo run --release -p weasel-style --bin shuangpin-schema -- Rime-Next\parser\shuangpin\xiaohe.parser.yaml
//!
//! 为什么要有这一步：librime 的 `ResourceResolver({"schema_source_file", "", ".schema.yaml"})`
//! 写死了只找 `.schema.yaml`，它不解析 YAML 以外的格式。所以约定是两层——
//!   * `parser\shuangpin\*.parser.yaml` ← **人编辑的配置（唯一源）**
//!   * `schema\<id>\<id>.schema.yaml` ← 本程序生成的产物，别手改
//!
//! 键位源文件是 YAML（2026-10-26 起不再认 TOML）：顶层 `schema:` 块放元信息，
//! `initials:` / `finals:` 放键位映射，一行一个 `键: "值"`。
//!
//! 设计要点：**algebra 的规则形态固定、键位字母从 YAML 取**。
//! 双拼的 xform 顺序是由 rime 的匹配语义决定的（长韵母必须先于短韵母，
//! 否则 `liang` 会被 `ang` 规则先命中变成 `lih`），不是随便排的。
//! 所以这里不"推导"顺序，只把 YAML 里的键位字母填进固定模板 ——
//! 改键位（比如把 an 从 j 挪走）能自动生效，又不会破坏匹配顺序。

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 键位 YAML 顶层结构。
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
    /// 语言模型（octagram）**前缀**。写 `zh-han.simp` 就自动挂载
    /// `model/zh-han.simp.*.gram` 全部模型（essay-bgc 字级 + essay-bgw 词级）；
    /// 只想用某一本就写全名（如 `zh-han.simp.essay-bgc`）。
    #[serde(default)]
    model_simp: Option<String>,
    #[serde(default)]
    model_trad: Option<String>,
    /// 继承哪个基础方案（用 `__include` 拉它的骨架，再 `patch` 覆盖差异）。
    ///
    /// 这样生成的方案里**不重复** engine / switches / segmentors / translators 那一大坨，
    /// 只写双拼真正需要改的东西（algebra、prism、preedit_format）。
    #[serde(default)]
    base: Option<String>,
    /// 是否开**简拼（首字母）**：开了以后打第 1 个键（声母）就出候选，
    /// 不再要等第 2 个键凑齐音节。默认关，说明见 [`build_algebra`] 末尾。
    #[serde(default)]
    abbrev: Option<bool>,
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

/// 自然码的 `preedit_format`（用户 2026-10-19：以前错用小鹤模板，显示不对）。
///
/// 自然码键位：ai=l ei=z ui=v ao=k ou=b iu=q ie=x ue=t ve=t
///            an=j en=f in=n un=p ang=h eng=g ing=y ong=s
///            ia=w iao=c ian=m iang=d iong=s ua=w uo=o uai=y uan=r uang=d
/// 同键多韵母按声母上下文拆分（s=iong/ong、d=iang/uang、w=ia/ua、y=ing/uai、o=uo/o、t=ue/ve）。
const PREEDIT_ZIRANMA: &[&str] = &[
    r"xform/([jqx])s/$1iong/",
    r"xform/(\w)s/$1ong/",
    r"xform/([jqxnl])d/$1iang/",
    r"xform/(\w)d/$1uang/",
    r"xform/([gkhzcsr])w/$1ua/",
    r"xform/(\w)w/$1ia/",
    r"xform/([gkhzcsr])y/$1uai/",
    r"xform/(\w)y/$1ing/",
    r"xform/([dtnlgkhvuirzcs])o/$1uo/",
    r"xform/([jqxy])t/$1ue/",
    r"xform/(\w)t/$1ve/",
    r"xform/(\w)q/$1iu/",
    r"xform/(\w)r/$1uan/",
    r"xform/(\w)p/$1un/",
    r"xform/(\w)n/$1in/",
    r"xform/(\w)m/$1ian/",
    r"xform/(\w)j/$1an/",
    r"xform/(\w)f/$1en/",
    r"xform/(\w)h/$1ang/",
    r"xform/(\w)g/$1eng/",
    r"xform/(\w)k/$1ao/",
    r"xform/(\w)l/$1ai/",
    r"xform/(\w)z/$1ei/",
    r"xform/(\w)x/$1ie/",
    r"xform/(\w)c/$1iao/",
    r"xform/(\w)b/$1ou/",
    r"xform/([dtgkhvuirzcs])v/$1ui/",
    r"xform/([aoe])\1(\w)/$1$2/",
    r"xform/(^|[ '])v/$1zh/",
    r"xform/(^|[ '])i/$1ch/",
    r"xform/(^|[ '])u/$1sh/",
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

    // ⑥ 简拼（首字母）—— 默认**不开**，要开请在 parser YAML 的 `schema:` 里写 `abbrev: true`。
    //
    //   · 为什么默认关：双拼的每个音节正好 2 个字母，所以任意两字母（如 "nh"）
    //     本身就是**合法全码**（n + ang → 囊）。简拼派生出的单字母拼写遇到这种输入串时，
    //     整段会被判成一个音节，多字的简拼切分要让位（打 "nh" 出来的是「囊」不是「你好」）。
    //   · 为什么用 `abbrev` 而不是 `derive`：`abbrev` 在棱镜里被标成简码，只在**没有全码
    //     匹配**时兜底，不会跟正常的两键全码抢 —— 打 "n" 出简拼、打 "ni" 出全码，各走各的。
    //   · 开了之后的真实收益：双拼第 1 个键（声母）不再"什么都不出"，
    //     与全拼 `pinyin.luna` 的手感对齐（`abbrev/^([a-z]).+$/$1/` 同款规则）。
    if spec.schema.abbrev.unwrap_or(false) {
        lines.push("abbrev/^([a-z]).+$/$1/".to_string());
    }
    lines
}

/// 生成**极简继承方案**：用 `__include` 拉 `pinyin.luna` 的骨架，只写双拼真正要改的东西
/// （speller 键位、translator 的预编辑映射与变体 model）。
///
/// 布局与 `schema/pinyin/luna/pinyin.luna.schema.yaml` 对齐（2026-10-28）：`translator`
/// 下挂 `simp:` / `trad:` 两个**变体子模块**，引擎自动识别并随繁简开关切换，
/// `engine/translators` 里不用写 `script_translator@simp/@trad`。
/// 词库**单文件**：simp/trad 共用一本 `pinyin.luna`（纯简体），字形差异交给 opencc
/// （继承 pinyin.luna 的 filter：zh_hant 关→t2s 保底简体、开→s2t 出繁体），
/// `dict` / `prism` 写在父 `translator` 层由两个变体共用。
///
/// 为什么不用"完整方案"：那样要把 engine / switches / segmentors / translators
/// 那一大坨连同注释整份复制过来，改一处基础方案就得重新生成全部，
/// 而且文件里 90% 的内容和 `pinyin.luna` 一模一样。
/// rime 原生支持 `__include`，只写差异才是对的做法。
fn render(spec: &Spec, source: &str) -> String {
    let meta = &spec.schema;
    let dict = meta.dict.as_deref().unwrap_or("pinyin.luna");
    let prism = meta.prism.as_deref().unwrap_or(&meta.id);
    let model_simp = meta
        .model_simp
        .as_deref()
        .unwrap_or("zh-han.simp");
    let model_trad = meta
        .model_trad
        .as_deref()
        .unwrap_or("zh-han.trad");
    let version = meta.version.as_deref().unwrap_or("0.18");
    let author = meta.author.as_deref().unwrap_or("");
    let base = meta.base.as_deref().unwrap_or("pinyin.luna");

    let algebra = build_algebra(spec)
        .iter()
        .map(|line| format!("    - {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    // 预编辑行（双拼键 -> 完整拼音）必须按**该方案自己的键位**还原。
    // 用户 2026-10-19 报「双拼编码转全拼显示也不对」：以前不管哪个方案都用小鹤模板。
    // id 允许带点（`double_pinyin.ziranma`，点表目录层级），按末段判断键位，
    // 不能写死 2026-10-28 之前的无点 id `double_pinyin_ziranma`。
    let preedit_rules: &[&str] = if spec.schema.id.ends_with("ziranma") {
        PREEDIT_ZIRANMA
    } else {
        PREEDIT_FLYPY
    };
    let preedit = preedit_rules
        .iter()
        .map(|line| format!("    - {line}"))
        .collect::<Vec<_>>()
        .join("\n");

    // 标准（2026-10-28）：`schema/description` **一律**块标量 `|`（保留换行、免转义）。
    // 走 `schema_standard` 渲染，和 `style/tests/schema_syntax.rs` 的强制检查同源。
    let description = weasel_style::schema_standard::render_description(&format!(
        "{}（明月拼音 + 双拼键位）。",
        meta.name
    ));
    let description = description.trim_end_matches('\n');

    format!(
        r#"# Rime Schema
# encode: utf-8
# newline: lf
#
# ★ 本文件由 `shuangpin-schema` 从 `Rime-Next\parser\shuangpin\{source}` 生成。
#   **不要手改**：改键位请改那份 YAML 再重新生成。
#
# 用 rime 的 **`__include` 深合并**继承 `{base}` 的骨架（engine / switches /
# segmentors / translators 全部继承过来），下面**直接写要覆盖的字段**即可。
#
# ⚠️ 这里**不能**用 `patch:` —— 那是 `.custom.yaml` 专用语法，
# 写在 `.schema.yaml` 里会被 rime 判为 `invalid schema definition` 而部署失败。
# `__include` 的语义是深合并：映射递归合并、列表整体替换、标量覆盖。
---
__include: {base}.schema:/
###
schema:
  id: {id}
  name: {name}
  version: "{version}"
  author:
    - {author}
{description}
###
speller:
  algebra:
{algebra}
###
# 繁简双变体：与 pinyin.luna 同布局 —— simp/trad 嵌在 translator 下，
# 引擎自动识别并随 `zh_hant` 开关切换；`preedit_format` 写在父层，两个变体继承。
# 词库**单文件**：simp/trad 共用一本 `pinyin.luna`（纯简体），字形差异交给
# opencc（继承 pinyin.luna 的 filter：zh_hant 关→t2s 保底简体、开→s2t 出繁体）。
# `model:` = 语言模型（octagram）前缀：写 `zh-han.simp` 就自动挂载
# model/zh-han.simp.*.gram 全部（essay-bgc 字级 + essay-bgw 词级，取最高分）；
# 只想用某一本就写全名，如 zh-han.simp.essay-bgc。变体各带一套。
translator:
  preedit_format:
{preedit}
  # dict/prism 两变体共用（与 pinyin.luna 同本词库），提到这一层共享；
  # 引擎把本层配置深合并进各变体，变体只写各自不同的 model（见下）。
  dict: {dict}
  prism: {prism}
  simp:
    model: {model_simp}
  trad:
    model: {model_trad}
###
# 繁简开关名从 include 的 pinyin.luna 的 `switch: zh_hant` 直接推断，
# 引擎自动绑定变体（@simp/@trad）与 filter（simplifier@zh_hans/@zh_hant）。
###
# 用家自選配置
__patch:
  - {id}.custom:/patch?
...
"#,
        source = source,
        base = base,
        id = meta.id,
        name = meta.name,
        description = description,
        version = version,
        author = author,
        dict = dict,
        prism = prism,
        model_simp = model_simp,
        model_trad = model_trad,
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
        if dir.join("data").is_dir() || dir.join("Rime-Next").is_dir() {
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

/// 用户数据目录：默认 `%APPDATA%\Rime-Next`，安装时自选的位置优先（`userdir.txt`）。
fn user_dir() -> Option<PathBuf> {
    weasel_common::process::user_data_dir()
}

/// 键位 YAML 的搜索目录，**程序文件夹优先**（parser 是出厂数据，2026-10-27 起
/// 住在 `Rime-Next\parser\`，随安装包分发；用户目录只留兼容兜底）。
///
/// 搜索顺序（先找到的先赢，同名去重）：
///   1. `<exe目录>\Rime-Next\parser\shuangpin\`    ← 程序文件夹（parser 出厂位置）
///   2. `%APPDATA%\Rime-Next\parser\shuangpin\`   ← 用户目录（历史部署残留，兼容）
///   3. `%APPDATA%\Rime-Next\schema\shuangpin\`   ← 旧位置，兼容
///   4. `%APPDATA%\Rime-Next\shuangpin\`          ← 更旧位置，兼容
///   5. `<exe目录>\Rime-Next\schema\shuangpin\`    ← 程序文件夹旧位置
///   6. `<exe目录>\Rime-Next\shuangpin\`           ← 程序文件夹更旧位置
///   7. `<repo>\Rime-Next\parser\shuangpin\`       ← 开发时兜底
///   8. `<repo>\Rime-Next\schema\shuangpin\`       ← 开发时旧位置兼容
///
/// 为什么 `parser\shuangpin\` 在列表里：shuangpin 是解析器数据，不是方案定义，
/// 与 `schema\` 平级放在 `parser\` 下；出厂那份在程序文件夹，**优先**。
fn shuangpin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut push = |dir: PathBuf| {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    };

    if let Some(root) = install_root() {
        let data = weasel_common::rime_layout::data_dir(&root);
        push(data.join("parser").join("shuangpin"));
        push(data.join("schema").join("shuangpin"));
        push(data.join("shuangpin"));
    }
    if let Some(user) = user_dir() {
        push(user.join("parser").join("shuangpin"));
        push(user.join("schema").join("shuangpin"));
        push(user.join("shuangpin"));
    }
    push(source_root().join("Rime-Next").join("parser").join("shuangpin"));
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

/// 列出所有待生成的键位 YAML（同名时用户目录那份优先，用 seen 去重）。
fn collect_targets() -> Vec<PathBuf> {
    let mut seen = std::collections::BTreeSet::new();
    let mut list = Vec::new();
    for dir in shuangpin_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "yaml") {
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

fn generate(yaml_path: &Path) -> Result<PathBuf, String> {
    let text = std::fs::read_to_string(yaml_path)
        .map_err(|error| format!("读取 {} 失败: {error}", yaml_path.display()))?;
    let spec: Spec = weasel_common::yaml::from_str(&text)
        .map_err(|error| format!("解析 {} 失败: {error}", yaml_path.display()))?;

    let source = yaml_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();

    // 把实际读到的内容打出来 —— 免得"到底读没读 YAML"这种事只能靠猜。
    println!("  读取 {}", yaml_path.display());
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
    // id 里的点 = 目录层级，与 `librime-next/src/lever/scan.rs::classify` 的摆放规则一致：
    // `double_pinyin.flypy` → `schema/double_pinyin/flypy/double_pinyin.flypy.schema.yaml`。
    let out_dir = spec
        .schema
        .id
        .split('.')
        .fold(output_base(), |dir, segment| dir.join(segment));
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
        eprintln!("没有找到任何 shuangpin\\*.yaml，搜索过：");
        for dir in shuangpin_dirs() {
            eprintln!("  {}", dir.display());
        }
        return std::process::ExitCode::FAILURE;
    }

    let mut failed = false;
    for yaml_path in targets {
        match generate(&yaml_path) {
            Ok(out) => println!("✓ {} → {}", yaml_path.display(), out.display()),
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



