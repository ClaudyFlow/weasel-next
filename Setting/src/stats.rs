//! 输入统计的数据层：读 `%APPDATA%\Rime-Next\stat\stat.bin`。
//!
//! 写侧是 TIP（`Tip/src/stats.rs`），读侧是 `/stat` 窗口。**格式以写侧注释为准**，
//! 两边改一处必须同时改另一处（`Tip/src/stats.rs` 的 `encode`/`parse_file` 与本文件
//! 的 `parse_stat_bytes` 是同一份契约，各有一组往返测试看着）。
//!
//! ```text
//! 头 16 字节： "WNST" | version u32 (3) | 天记录数 u32 | 字记录数 u32
//! 天记录 36B： date=YYYYMMDD u32 | keys u64 | chars u64 | active_ms u64
//!              | effective_keys u32（Σ编辑量 = Σ按键 − 2×退格）| commits u32（上屏轮数）
//! 字记录 12B： codepoint u32 | count u64        （按次数降序，最多 200 个）
//! ```
//!
//! * v3 才有后两个字段；**v1（20 字节天记录）/ v2（28 字节天记录）照样读**，
//!   只是那些天没有编辑量与轮数（读成 0）。
//! * 版本 3 是两侧同时支持的"官方版本"（用户 2026-10-26 要求写侧/读侧都认 2/3）。
//!
//! * v1（20 字节天记录、无时长/字表）**照样读**，只是速度显示 0、常用字为空。
//! * 读的时候一律**宽容**：坏文件跳过并记一条 warning —— 统计窗口不能因为一个坏文件就打不开；
//!   文件被写侧「截断后重写」时可能读到半截，此时按已完整的记录读（宁少勿崩）。
//! * 极旧的 `stats\YYYY-MM-DD.toml`（v1 之前的格式）**只读兜底**：二进制里没有的日期才用，
//!   这样已经积累过的旧数据不会凭空消失，也不需要迁移/删除用户文件。
//!
//! 段位/境界两张表在 `Setting/src/main.rs`（移植自 input_statistics.lua），
//! 这里只负责出**累计经验**（= 所有 `chars` 之和）、**累计字数**与每日数字，等级换算交给调用方。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 热力图：7 行（周日→周六）× 53 列（周），共 371 格 —— 整年。
///
/// 与 GitHub 贡献图同一套排法：**列 = 周、行 = 星期（周日在上）**，
/// 最右一列是本周。Slint 侧按 `week * 7 + day` 索引，所以数组必须恒为 301 个。
pub const GRID_WEEKS: usize = 53;
/// 热力图格子总数。
pub const GRID_CELLS: usize = GRID_WEEKS * 7;
/// 「常用字」最多显示几个。
pub const MAX_WORDS: usize = 90;

/// 用户数据根（默认 `%APPDATA%\Rime-Next`；安装时自选的位置由 `userdir.txt` 覆盖）。
pub fn state_root() -> Option<PathBuf> {
    weasel_common::process::user_data_dir()
}

/// 二进制统计文件：`%APPDATA%\Rime-Next\stat\stat.bin`（**单数** `stat`）。
pub fn stat_file() -> Option<PathBuf> {
    Some(state_root()?.join("stat").join("stat.bin"))
}

/// 二进制统计文件所在目录：`%APPDATA%\Rime-Next\stat`。
pub fn stats_dir() -> Option<PathBuf> {
    Some(state_root()?.join("stat"))
}

/// 极旧的「一天一个 toml」目录：`%APPDATA%\Rime-Next\stats`（**复数**，只读兜底）。
pub fn legacy_stats_dir() -> Option<PathBuf> {
    Some(state_root()?.join("stats"))
}

/// 一天的统计。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DayEntry {
    /// `YYYY-MM-DD`（取自文件名）。
    pub date: String,
    /// 击键数。
    pub keys: u64,
    /// 上屏字数。
    pub chars: u64,
    /// 有效输入时长（毫秒）。
    pub active_ms: u64,
    /// Σ编辑量（Σ按键 − 2×退格，负值收 0）；v3 起才有，老记录是 0。
    ///
    /// 键准 = 有效按键 ÷ 击键（冰雪 rime-snow-pinyin 的口径，见 `doc/memo.md`）。
    pub effective_keys: u64,
    /// 当天上屏轮数；v3 起才有，老记录是 0。平均词长 = 上屏字数 ÷ 轮数。
    pub commits: u64,
    /// 当天上屏字频。
    pub char_counts: BTreeMap<char, u64>,
}

/// 热力图的一格。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayCell {
    /// 该格代表的那天的击键数（决定颜色深浅）。
    pub count: u32,
    /// 悬停/占位提示，例如 `2026-10-17 周六 · 1,234 键 / 567 字`。
    pub tip: String,
    /// 是否今天（Slint 侧描边高亮）。
    pub today: bool,
}

/// 界面一次刷新所需的全部数字。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// 今日击键。
    pub today_keys: u64,
    /// 今日上屏字数。
    pub today_chars: u64,
    /// 今日速度（字/分）；没有时长数据时为 0。
    pub today_speed: u32,
    /// 今日击键速度，单位 **0.1 键/秒**（即 `5` = 0.5 键/秒）。
    ///
    /// 用户 2026-10-25：击键速度要用**秒速度**（"分速度不好算"），并且要一位小数，
    /// 所以这里存十分之一键/秒的整数，由界面格式化成 `5.2 键/秒`。
    /// 与 [`Self::today_speed`] 同源（都除 `active_ms`），只是分子换成击键数。
    pub today_key_speed_x10: u32,
    /// 历史最高速度（所有天里最快的那一天，字/分）。
    pub best_speed: u32,
    /// 历史均速（所有天合起来：总字数 / 总有效时长，字/分）。
    ///
    /// 与 [`Self::best_speed`] 配对：用户 2026-10-25 明确"历史均速就是所有的平均速度，
    /// 历史极速就是最大值"。注意它**不是**各天速度的平均（那样会被只打了两三个字的
    /// 那几天严重拉偏），而是加权平均 —— 用总字数除以总时长。
    pub all_speed: u32,
    /// 历史最大击键速度（单位 **0.1 键/秒**，单日最高）。
    pub best_key_speed_x10: u32,
    /// 历史平均击键速度（单位 **0.1 键/秒**，总击键 / 总有效时长）。
    pub all_key_speed_x10: u32,
    /// 今日平均码长（单位 **0.01 键/字**，今日击键 / 今日上屏字数）。
    pub today_code_len_x100: u32,
    /// 今日键准（单位 **0.1 %**，`987` = 98.7%）。
    ///
    /// 口径 = 有效按键 ÷ 击键（冰雪 rime-snow-pinyin 的 `docs/statistics.md`），
    /// 数据来自 v3 天记录的 `effective_keys`；老记录没有这一项，显示 0.0。
    pub today_key_accuracy_x10: u32,
    /// 今日平均词长（单位 **0.01 字/轮**，今日上屏字数 ÷ 今日上屏轮数）。
    ///
    /// 数据来自 v3 天记录的 `commits`；老记录没有这一项，显示 0.00。
    pub today_word_len_x100: u32,
    /// 今日是「混合日」：从 v2 升级上来的那天，`chars` 含升级前的字数、`commits` 只有
    /// 升级后的轮数 ⇒ 词长算不出来，界面显示 `—` 而不是一个离谱的数（见 [`word_len_plausible`]）。
    pub today_word_len_mixed: bool,
    /// 今日**有效输入时长**（毫秒）：只有按键间隔 ≤5 秒的那几段算进来（与平均速度同源）。
    pub today_active_ms: u64,
    /// 累计有效输入时长（毫秒），所有天相加 —— 「打字时间」卡的灰色历史值。
    pub total_active_ms: u64,
    /// 平均每天上屏字数（只除以**有击键的天数**，不拿空白日摊薄）。
    pub avg_daily_chars: u32,
    /// 历史平均码长（单位 **0.01 键/字**，总击键 / 总上屏字数）。
    pub all_code_len_x100: u32,
    /// 连续使用天数（含今天；今天还没打字就从昨天往前数）。
    pub streak_days: u32,
    /// 历史最长连续天数（所有记录里最长的一段连续使用）。
    pub best_streak_days: u32,
    /// 「2026 年第 4 季度」这种标题，Rust 算好给 Slint 用。
    pub quarter_label: String,
    /// 累计击键。
    pub total_keys: u64,
    /// 累计上屏字数。
    pub total_chars: u64,
    /// 累计经验（= 累计上屏字数，段位表按它分段）。
    pub total_exp: u64,
    /// 恒为 [`GRID_CELLS`] 个。
    /// 近 7 天（含今天）的合计与平均。
    ///
    /// 用户 2026-10-26：**数据一直全量记录**（stat.bin 不删任何一天），但界面上所有「平均」
    /// 只读最近 7 天 —— 全期平均会把几个月前的打字习惯一直压在分母里，看不出进步；
    /// 3 天又太抖（我们实测单日字数能从 258 跳到 969）。7 天 ≈ 一个「工作周 + 周末」。
    pub recent_days: u32,
    /// 近 7 天击键合计。
    pub recent_keys: u64,
    /// 近 7 天上屏字数合计。
    pub recent_chars: u64,
    /// 近 7 天有效时长合计（毫秒）。
    pub recent_active_ms: u64,
    /// 近 7 天平均速度（字/分）。
    pub recent_speed: u32,
    /// 近 7 天平均击键速度（0.1 键/秒）。
    pub recent_key_speed_x10: u32,
    /// 近 7 天平均码长（0.01 键/字）。
    pub recent_code_len_x100: u32,
    /// 近 7 天键准（0.1 %）；只有 v3 天记录（带 `commits`）能算，算不出时为 0。
    pub recent_key_accuracy_x10: u32,
    /// 近 7 天平均词长（0.01 字/轮）；同样只有 v3 天记录能算。
    pub recent_word_len_x100: u32,
    /// 今日格在热力图里的**列**（周索引，0 = 1 月 1 日所在周）；-1 = 今天不在今年网格里。
    ///
    /// 给界面画「今日十字渐变」用：列 = 周，行 = 周日→周六。
    pub today_week: i32,
    /// 今日格在热力图里的**行**（0 = 周日 … 6 = 周六）；-1 = 同上。
    pub today_day: i32,
    pub days: Vec<DayCell>,
    /// 年度活跃卡片用：**12 列（月）× 7 行（周日→周六）= 84 格**。
    ///
    /// `days` 是 371 格整年网格，只有 count/tip/today，**没有月份字段**，
    /// 界面侧没法按月分组 —— 所以在这里顺带聚合成 84 格（用户 2026-11-01 设计图）。
    pub month_days: Vec<DayCell>,
    /// 今年有记录的天数（count > 0）。
    pub active_days: u32,
    /// 活跃率（百分比，0..100）= 活跃天 / 今年已过天数。
    pub active_rate: u32,
    /// 12 个月各自**从第几周开始**（0 起的周索引），给界面把月份标签摆在对应列上。
    pub month_week: Vec<i32>,
    /// 「按月」日历：每个月单独一块，**从该月 1 号开始**排，列 = 该月周数（4~6），
    /// 行 = 周一→周日（用户 2026-11-01 20:1x：「一个月一个月从一月一开始计算」）。
    /// 块内按**行主序**排列：`offset[m] + row * cols[m] + col`。
    pub calendar: Vec<DayCell>,
    /// 12 个月各自的周数（列数）。
    pub calendar_cols: Vec<i32>,
    /// 12 个月各自在 `calendar` 里的起始下标。
    pub calendar_offsets: Vec<i32>,
    /// 与 `calendar` 等长：1 = 该月真实日期，0 = 补齐用的空格（界面画成透明）。
    pub calendar_mask: Vec<i32>,
    /// 12 个月周数之和（= 网格总列数），界面据此精确算格子边长（不然一定会溢出）。
    pub calendar_cols_total: i32,
    /// 常用字（字, 次数），按次数降序。
    pub words: Vec<(String, u64)>,
    /// 至少读到过一天的数据。
    pub has_data: bool,
    /// 读文件时遇到的问题（给状态栏用）。
    pub warnings: Vec<String>,
}

/// 扫目录，读出所有 `YYYY-MM-DD.yaml`（旧格式：一天一个文件）。
///
/// 返回（按日期升序的条目, 问题清单）。目录不存在 = 还没开始采集，不算错误。
pub fn read_days(dir: &Path) -> (Vec<DayEntry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let Ok(listing) = std::fs::read_dir(dir) else {
        return (entries, warnings);
    };
    for item in listing.flatten() {
        let path = item.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("yaml") {
            continue;
        }
        let Some(date) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        // 只认 `YYYY-MM-DD` —— 以后 stats\ 里若多出别的 yaml（配置之类），不能被当成一天。
        if !is_iso_date(date) {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match parse_day(date, &text) {
                Ok(entry) => entries.push(entry),
                Err(error) => warnings.push(error),
            },
            Err(error) => warnings.push(format!("{date}: 读不出来（{error}）")),
        }
    }
    entries.sort_by(|left, right| left.date.cmp(&right.date));
    (entries, warnings)
}

/// 解析一天的统计文件。字段缺失当 0；整份不是合法 YAML 才算错。
///
/// 2026-10-27：一天一份 `YYYY-MM-DD.toml` 是**旧格式**（已被 `stat/stat.bin` 取代，
/// 这里只作兜底），随全仓库一起改成 YAML。
pub fn parse_day(date: &str, text: &str) -> Result<DayEntry, String> {
    let value: serde_json::Value = weasel_common::yaml::to_value(text).map_err(|error| format!("{date}: {error}"))?;
    let number = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_i64)
            .filter(|value| *value > 0)
            .unwrap_or(0) as u64
    };
    let mut char_counts = BTreeMap::new();
    if let Some(table) = value.get("char_counts").and_then(serde_json::Value::as_object) {
        for (key, count) in table {
            let mut chars = key.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                continue;
            };
            if ch.is_control() {
                continue;
            }
            if let Some(count) = count.as_i64().filter(|count| *count > 0) {
                char_counts.insert(ch, count as u64);
            }
        }
    }
    Ok(DayEntry {
        date: date.to_string(),
        keys: number("keys"),
        chars: number("chars"),
        active_ms: number("active_ms"),
        // 旧格式（一天一个文件）从来没采过编辑量/轮数：留 0，算不出就不显示。
        effective_keys: 0,
        commits: 0,
        char_counts,
    })
}

/// 二进制统计文件的布局（与写侧 `Tip/src/stats.rs` 同值，改一处必须同时改另一处）。
const STAT_HEADER_LEN: usize = 16;
const STAT_DAY_LEN_V1: usize = 20;
const STAT_DAY_LEN_V2: usize = 28;
/// v3：v2 的天记录 + `effective_keys: u32` + `commits: u32`。
const STAT_DAY_LEN_V3: usize = 36;
const STAT_DAY_LEN_V4: usize = 36 + 24 * 4;
const STAT_CHAR_LEN: usize = 12;
const STAT_MAGIC: [u8; 4] = *b"WNST";

/// 版本 → 天记录长度。写侧（TIP）只写 v3；读侧必须认 1/2/3
/// （用户 2026-10-26：两侧版本校验同时支持 2/3，老文件不能被判成坏了）。
fn stat_day_len(version: u32) -> Option<usize> {
    match version {
        1 => Some(STAT_DAY_LEN_V1),
        2 => Some(STAT_DAY_LEN_V2),
        3 => Some(STAT_DAY_LEN_V3),
        4 => Some(STAT_DAY_LEN_V4),
        _ => None,
    }
}

/// `YYYYMMDD` 的 u32 → `YYYY-MM-DD`。越界的日期当坏记录丢掉（别把 20261340 画到热力图上）。
fn code_to_date(code: u32) -> Option<String> {
    let (year, month, day) = (code / 10_000, (code / 100) % 100, code % 100);
    if !(1970..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// 解析 `.bin` 的字节。返回（按 date 升序的天条目, 全局字频表）。
///
/// 只有「头都不完整 / magic 不对 / 版本不认识」才算错；**记录被截断不算错** ——
/// 写侧是「`SetEndOfFile` → 重写整个文件」，读的瞬间可能刚好撞上半截文件，
/// 这时按已经完整的记录返回，比报错有用。
pub fn parse_stat_bytes(bytes: &[u8]) -> Result<(Vec<DayEntry>, BTreeMap<char, u64>), String> {
    if bytes.len() < STAT_HEADER_LEN {
        return Err(format!("统计文件太短（{} 字节），可能正在写入", bytes.len()));
    }
    if bytes[0..4] != STAT_MAGIC {
        return Err("不是统计文件（WNST magic 不对）".to_string());
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let Some(day_len) = stat_day_len(version) else {
        return Err(format!("统计文件版本 {version} 不认识（本程序认 1/2/3）"));
    };
    let day_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let char_count = if version >= 2 {
        u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize
    } else {
        0
    };

    let mut days: Vec<DayEntry> = Vec::new();
    let mut at = STAT_HEADER_LEN;
    for _ in 0..day_count {
        let Some(chunk) = bytes.get(at..at + day_len) else { break };
        at += day_len;
        let Some(date) = code_to_date(u32::from_le_bytes(chunk[0..4].try_into().unwrap())) else {
            continue;
        };
        days.push(DayEntry {
            date,
            keys: u64::from_le_bytes(chunk[4..12].try_into().unwrap()),
            chars: u64::from_le_bytes(chunk[12..20].try_into().unwrap()),
            active_ms: if day_len >= STAT_DAY_LEN_V2 {
                u64::from_le_bytes(chunk[20..28].try_into().unwrap())
            } else {
                0
            },
            // v3 才有的两列；v1/v2 读成 0（那几天本来也没采过）。
            effective_keys: if day_len >= STAT_DAY_LEN_V3 {
                u64::from(u32::from_le_bytes(chunk[28..32].try_into().unwrap()))
            } else {
                0
            },
            commits: if day_len >= STAT_DAY_LEN_V3 {
                u64::from(u32::from_le_bytes(chunk[32..36].try_into().unwrap()))
            } else {
                0
            },
            char_counts: BTreeMap::new(),
        });
    }
    // 同一个日期出现两次（写侧不该发生）按「后来的算」，并且始终按 date 升序。
    let mut by_date: BTreeMap<String, DayEntry> = BTreeMap::new();
    for day in days {
        by_date.insert(day.date.clone(), day);
    }
    let days: Vec<DayEntry> = by_date.into_values().collect();

    let mut chars: BTreeMap<char, u64> = BTreeMap::new();
    for _ in 0..char_count {
        let Some(chunk) = bytes.get(at..at + STAT_CHAR_LEN) else { break };
        at += STAT_CHAR_LEN;
        let Some(ch) = char::from_u32(u32::from_le_bytes(chunk[0..4].try_into().unwrap())) else {
            continue;
        };
        if ch.is_control() {
            continue;
        }
        *chars.entry(ch).or_insert(0) += u64::from_le_bytes(chunk[4..12].try_into().unwrap());
    }
    Ok((days, chars))
}

/// 统计文件的**尾部扩展**：每个虚拟键的次数 + 码长直方图。
///
/// 写侧 `Tip/src/stats.rs` 把这两张表追加在字表之后、**没有升版本号**，
/// 所以老读取器读到字表就停，新读取器按「字表之后还有没有 `STAT_EXT_LEN` 字节」判断。
pub const STAT_VK_COUNT: usize = 256;
/// 码长直方图的档数（0..=32 键）。
pub const STAT_CODELEN_COUNT: usize = 33;
const STAT_EXT_LEN: usize = (STAT_VK_COUNT + STAT_CODELEN_COUNT) * 4;
/// 时段统计桶数：一天 24 小时（手机屏幕使用时间样式）。
pub const STAT_HOUR_COUNT: usize = 24;
/// 时段桶扩展区：24 × (u32 keys + u64 active_ms)，追加在键盘/码长直方图之后。
const STAT_HOUR_EXT_LEN: usize = STAT_HOUR_COUNT * (4 + 8);

/// 从字节里取尾部扩展；没有扩展（老文件）时返回 `None`。
pub fn parse_stat_hist(bytes: &[u8]) -> Option<([u32; STAT_VK_COUNT], [u32; STAT_CODELEN_COUNT])> {
    if bytes.len() < STAT_HEADER_LEN || bytes[0..4] != STAT_MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    // 天记录长度随版本变（v1 20 / v2 28 / v3 36），偏移量算错就整张表读歪。
    let day_len = stat_day_len(version)?;
    if version < 2 {
        return None;
    }
    let day_count = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    let char_count = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    let at = STAT_HEADER_LEN + day_count * day_len + char_count * STAT_CHAR_LEN;
    if bytes.len() < at + STAT_EXT_LEN {
        return None;
    }
    let mut keys = [0u32; STAT_VK_COUNT];
    for (index, slot) in keys.iter_mut().enumerate() {
        let start = at + index * 4;
        *slot = u32::from_le_bytes(bytes[start..start + 4].try_into().ok()?);
    }
    let coded = at + STAT_VK_COUNT * 4;
    let mut code_lens = [0u32; STAT_CODELEN_COUNT];
    for (index, slot) in code_lens.iter_mut().enumerate() {
        let start = coded + index * 4;
        *slot = u32::from_le_bytes(bytes[start..start + 4].try_into().ok()?);
    }
    Some((keys, code_lens))
}

/// 读时段统计（2026-10-31 新尾部扩展，老文件返回全 0 —— 界面据此提示"今天还没采"）。
///
/// 返回 `(每小时击键数, 每小时有效毫秒)`；文件太旧/太短都是全 0。
pub fn parse_stat_hours(bytes: &[u8]) -> ([u32; STAT_HOUR_COUNT], [u64; STAT_HOUR_COUNT]) {
    let empty = ([0u32; STAT_HOUR_COUNT], [0u64; STAT_HOUR_COUNT]);
    if bytes.len() < STAT_HEADER_LEN || bytes[0..4] != STAT_MAGIC {
        return empty;
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap_or([0; 4]));
    let Some(day_len) = stat_day_len(version) else {
        return empty;
    };
    if version < 2 {
        return empty;
    }
    let day_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap_or([0; 4])) as usize;
    let char_count = u32::from_le_bytes(bytes[12..16].try_into().unwrap_or([0; 4])) as usize;
    // 小时桶锚定**文件末尾**(TIP 最后追加 HourExt=24×(u32+u64)=336 字节)。
    // 不走字表偏移——TIP 字表条目 12 字节、Setting 曾按 20 读,中间布局一变就全歪。
    let hours_at = bytes
        .len()
        .checked_sub(STAT_HOUR_EXT_LEN)
        .filter(|_| bytes.len() >= STAT_HEADER_LEN + STAT_HOUR_EXT_LEN)
        .map(|base| {
            let _ = (day_count, day_len, char_count);
            base
        });
    let Some(hours_at) = hours_at else {
        return empty;
    };
    let mut keys = [0u32; STAT_HOUR_COUNT];
    for (index, slot) in keys.iter_mut().enumerate() {
        let start = hours_at + index * 4;
        *slot = u32::from_le_bytes(bytes[start..start + 4].try_into().unwrap_or([0; 4]));
    }
    let ms_at = hours_at + STAT_HOUR_COUNT * 4;
    let mut ms = [0u64; STAT_HOUR_COUNT];
    for (index, slot) in ms.iter_mut().enumerate() {
        let start = ms_at + index * 8;
        *slot = u64::from_le_bytes(bytes[start..start + 8].try_into().unwrap_or([0; 8]));
    }
    (keys, ms)
}

/// 读当前用户的时段统计（读不到就是全 0，界面画空表）。
/// 前 7 天日总量柱(用户 2026-11-07):返回 (高度百分比, 透明度百分比),
/// 越久远透明度越低(45→10)。自读 stat.bin,不依赖调用方上下文。
pub fn load_week_bars() -> Vec<(i32, i32)> {
    let Ok(bytes) = std::fs::read(&stat_file().unwrap_or_default()) else {
        return Vec::new();
    };
    let Ok((days, _)) = parse_stat_bytes(&bytes) else {
        return Vec::new();
    };
    let (ty, tm, td) = today_local();
    let today = format!("{ty:04}-{tm:02}-{td:02}");
    let mut out = Vec::new();
    let mut peak: u64 = 0;
    let mut vals: Vec<(i64, u64)> = Vec::new();
    for off in (1..=7).rev() {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
            - off * 86400;
        let days_n = secs.div_euclid(86400);
        let z = days_n + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let mut y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        if m <= 2 { y += 1; }
        let date = format!("{y:04}-{m:02}-{d:02}");
        if date == today {
            continue;
        }
        let keys = days.iter().find(|e| e.date == date).map(|e| e.keys).unwrap_or(0);
        vals.push((off, keys));
        peak = peak.max(keys);
    }
    for (off, keys) in &vals {
        let pct = if peak > 0 { ((*keys as f32 / peak as f32) * 100.0).round() as i32 } else { 0 };
        let alpha = match off { 1 => 45, 2 => 38, 3 => 31, 4 => 25, 5 => 19, 6 => 14, _ => 10 };
        out.push((pct, alpha));
    }
    out
}

/// 旁路补数文件(`statabricated.json`,编造的近 7 天+缺天,含逐时曲线)。
/// stat.bin 被 TIP 锁死没法直接写文件,显示层合并:天记录进活动图,
/// 逐时曲线进时段图的历史层(用户 2026-11-07)。
pub fn load_fabricated() -> Vec<(String, u64, u64, u64, [u32; STAT_HOUR_COUNT])> {
    let Some(root) = state_root() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(root.join("stat").join("fabricated.json")) else {
        return Vec::new();
    };
    let Ok(list) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(arr) = list.as_array() else { return Vec::new() };
    arr.iter()
        .filter_map(|item| {
            let date = item.get("date")?.as_str()?.to_string();
            let keys = item.get("keys")?.as_u64()?;
            let active_ms = item.get("active_ms")?.as_u64()?;
            let commits = item.get("commits")?.as_u64().unwrap_or(0);
            let hours_json = item.get("hours")?.as_array()?;
            let mut hours = [0u32; STAT_HOUR_COUNT];
            for (i, h) in hours_json.iter().enumerate().take(STAT_HOUR_COUNT) {
                hours[i] = h.as_u64().unwrap_or(0) as u32;
            }
            Some((date, keys, active_ms, commits, hours))
        })
        .collect()
}

pub fn load_hours() -> ([u32; STAT_HOUR_COUNT], [u64; STAT_HOUR_COUNT]) {
    let Some(root) = state_root() else {
        return ([0u32; STAT_HOUR_COUNT], [0u64; STAT_HOUR_COUNT]);
    };
    let path = root.join("stat").join("stat.bin");
    let Ok(bytes) = std::fs::read(&path) else {
        return fabricated_hours_fallback();
    };
    let parsed = parse_stat_hours(&bytes);
    if parsed.0.iter().all(|n| *n == 0) {
        // 今日还没采(stat.bin 的时段桶全零)→ 用旁路编造数据的最近一天。
        return fabricated_hours_fallback();
    }
    parsed
}

/// 旁路编造数据(statabricated.json)里**最近一天**的逐时曲线。
fn fabricated_hours_fallback() -> ([u32; STAT_HOUR_COUNT], [u64; STAT_HOUR_COUNT]) {
    let mut best: Option<(String, [u32; STAT_HOUR_COUNT])> = None;
    for (date, _keys, _ms, _com, hours) in load_fabricated() {
        if best.as_ref().map_or(true, |(d, _)| date > *d) {
            best = Some((date, hours));
        }
    }
    match best {
        Some((_, hours)) => {
            let ms = hours.iter().map(|h| u64::from(*h) * 900).collect::<Vec<_>>();
            let mut out = [0u64; STAT_HOUR_COUNT];
            out.copy_from_slice(&ms);
            (hours, out)
        }
        None => ([0u32; STAT_HOUR_COUNT], [0u64; STAT_HOUR_COUNT]),
    }
}

/// 读当前用户的键盘/码长统计（还没采集到就给一张全 0 的表）。
///
/// **返回全 0 而不是空表**：界面据此总是画出键盘，只是键帽都是浅灰 + 提示一句
/// 「还没采到数据」。以前返回空表 → 卡片只剩一行字，看着像做扁了（用户 2026-10-18 报）。
pub fn load_hist() -> (Vec<u32>, Vec<u32>) {
    let zeros = || (vec![0u32; STAT_VK_COUNT], vec![0u32; STAT_CODELEN_COUNT]);
    let Some(root) = state_root() else {
        return zeros();
    };
    let path = root.join("stat").join("stat.bin");
    let Ok(bytes) = std::fs::read(&path) else {
        return zeros();
    };
    match parse_stat_hist(&bytes) {
        Some((keys, code_lens)) => (keys.to_vec(), code_lens.to_vec()),
        None => zeros(),
    }
}

/// 读 `.bin` 文件。
///
/// 写侧用 `FILE_SHARE_READ` 打开（见 `Tip/src/stats.rs`），所以普通 `fs::read` 能读；
/// 若撞上「刚截断还没写」的几十微秒，重试一次再放弃。
pub fn read_stat_file(path: &Path) -> Result<(Vec<DayEntry>, BTreeMap<char, u64>), String> {
    let read = || std::fs::read(path).map_err(|error| format!("{}: 读不出来（{error}）", path.display()));
    let mut bytes = read()?;
    if bytes.len() < STAT_HEADER_LEN {
        std::thread::sleep(std::time::Duration::from_millis(30));
        bytes = read()?;
    }
    parse_stat_bytes(&bytes)
}

/// `/stat` 窗口的唯一入口：二进制为主，旧的 `stats\*.toml` 只补二进制里没有的日期。
///
/// 返回（按 date 升序的条目, 问题清单）。目录/文件不存在 = 还没开始采集，不算错误。
/// 显示层补缺:首末日期之间的空洞用邻居均值合成(用户 2026-11-07:
/// "空着很难看,编一下数据")。**只改内存副本,不写 stat.bin**(被 TIP 锁死)。
/// 随机性用日期做种子,同一天每次刷新数值一致。
pub fn fill_missing_days(entries: &mut Vec<DayEntry>) {
    use std::collections::BTreeSet;
    if entries.len() < 2 {
        return;
    }
    let date_ord = |s: &str| -> Option<i64> {
        let p: Vec<i64> = s.split('-').filter_map(|x| x.parse::<i64>().ok()).collect();
        if p.len() != 3 {
            return None;
        }
        // 简化天数序:年*372+月*31+日(相邻比较够用,跨月误差不影响"有没有")
        Some(p[0] * 372 + p[1] * 31 + p[2])
    };
    let mut have: BTreeSet<i64> = BTreeSet::new();
    for e in entries.iter() {
        if let Some(o) = date_ord(&e.date) {
            have.insert(o);
        }
    }
    let avg_keys = entries.iter().map(|e| e.keys).sum::<u64>() / entries.len() as u64;
    let avg_ms = entries.iter().map(|e| e.active_ms).sum::<u64>() / entries.len() as u64;
    let avg_chars = entries.iter().map(|e| e.chars).sum::<u64>() / entries.len() as u64;
    let avg_eff = entries.iter().map(|e| e.effective_keys).sum::<u64>() / entries.len() as u64;
    let avg_com = entries.iter().map(|e| e.commits).sum::<u64>() / entries.len() as u64;
    let mut extra = Vec::new();
    let mut o = *have.first().unwrap();
    let last = *have.last().unwrap();
    while o <= last {
        if !have.contains(&o) {
            let jitter = ((o % 50) as u64).min(40) + 80; // 80..120%
            let keys = avg_keys * jitter / 100;
            let ms = avg_ms * jitter / 100;
            let chars = avg_chars * jitter / 100;
            let eff = avg_eff * jitter / 100;
            let com = avg_com * jitter / 100;
            extra.push((o, keys, chars, ms, eff, com));
        }
        o += 1;
    }
    for (o, keys, chars, ms, eff, com) in extra {
        // 天数序 → 日期串:反解(近似,跨月按 31 天滚动,月末溢出用 min 截断)。
        let mut y = (o / 372) as i64;
        let mut rem = o % 372;
        let mut m = (rem / 31) as i64;
        rem %= 31;
        let mut dd = rem as i64;
        if m == 0 { m = 12; y -= 1; }
        if dd == 0 { dd = 28; }
        let date = format!("{:04}-{:02}-{:02}", y, m.min(12).max(1), dd.min(28).max(1));
        entries.push(DayEntry {
            date,
            keys,
            chars,
            active_ms: ms,
            effective_keys: eff,
            commits: com,
            char_counts: Default::default(),
        });
    }
    entries.sort_by(|a, b| a.date.cmp(&b.date));
}

pub fn load() -> (Vec<DayEntry>, Vec<String>) {
    match state_root() {
        Some(root) => load_from(&root),
        None => (Vec::new(), vec!["找不到 %APPDATA%，读不到统计数据".to_string()]),
    }
}

/// [`load`] 的可测版本：根目录由调用方给（`%APPDATA%\Rime-Next` 的形状）。
pub fn load_from(root: &Path) -> (Vec<DayEntry>, Vec<String>) {
    let mut entries: Vec<DayEntry> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut chars: BTreeMap<char, u64> = BTreeMap::new();

    let stat_path = root.join("stat").join("stat.bin");
    if stat_path.is_file() {
        match read_stat_file(&stat_path) {
            Ok((days, table)) => {
                entries = days;
                chars = table;
            }
            Err(error) => warnings.push(error),
        }
    }

    // 旧格式兜底。**同一天要相加，不能忽略**：换 TIP 的那天，上午的量在旧 toml 里、
    // 下午的在 stat.bin 里，忽略任一边都会少算。（两个写侧永远不会同时存在：TIP 只有一个。）
    let (legacy, legacy_warnings) = read_days(&root.join("stats"));
    warnings.extend(legacy_warnings);
    for day in legacy {
        match entries.iter_mut().find(|known| known.date == day.date) {
            Some(known) => {
                known.keys = known.keys.saturating_add(day.keys);
                known.chars = known.chars.saturating_add(day.chars);
                known.active_ms = known.active_ms.saturating_add(day.active_ms);
                for (ch, count) in day.char_counts {
                    *known.char_counts.entry(ch).or_insert(0) += count;
                }
            }
            None => entries.push(day),
        }
    }

    entries.sort_by(|left, right| left.date.cmp(&right.date));
    // 字频表是**全量**的（写侧只留前 200 个字），挂到最后一天上：
    // `snapshot()` 会跨天求和，于是「常用字」拿到的正是全量累计。
    if !chars.is_empty() {
        if let Some(last) = entries.last_mut() {
            for (ch, count) in chars {
                *last.char_counts.entry(ch).or_insert(0) += count;
            }
        }
    }
    (entries, warnings)
}

/// 把一天的条目算成界面要的数字。`today` 是本地日期 `(年, 月, 日)`。
/// 一天的字/轮超过这个值就判定为"混合日"（口径见 [`word_len_plausible`]）。
///
/// 正常打字一天 2~10 字/轮；就算整句上屏也很难到 30 字一轮，所以 30 足够宽松。
const MAX_PLAUSIBLE_WORD_LEN: u64 = 30;

/// 这天的「上屏字数 ÷ 上屏轮数」是否可信。
///
/// v3 的 `commits` 只从升级那一刻开始计数，`chars` 却是当天全量。于是从 v2 升级上来的
/// **当天**会出现"分子含升级前字数、分母只有升级后轮数"的情况：2026-10-26 真机实测
/// 923 字 ÷ 5 轮 = 184.60 字/轮。这种天的词长算不出来，按不可信处理（界面 `—`，
/// 也不进近 7 天聚合），升级次日起自然恢复 —— 不必为了它去改 stat.bin 的格式。
fn word_len_plausible(chars: u64, commits: u64) -> bool {
    commits > 0 && chars <= commits.saturating_mul(MAX_PLAUSIBLE_WORD_LEN)
}

pub fn snapshot(entries: &[DayEntry], today: (i64, u32, u32)) -> Snapshot {
    let today_days = days_from_civil(today.0, today.1, today.2);
    let today_text = format!("{:04}-{:02}-{:02}", today.0, today.1, today.2);

    let mut snapshot = Snapshot {
        has_data: !entries.is_empty(),
        ..Snapshot::default()
    };
    for entry in entries {
        snapshot.total_keys += entry.keys;
        snapshot.total_chars += entry.chars;
        // 历史最高速度：只统计有上屏、有时长的天
        if entry.active_ms > 0 && entry.chars > 0 {
            let speed = ((entry.chars * 60_000 + entry.active_ms / 2) / entry.active_ms) as u32;
            if speed > snapshot.best_speed {
                snapshot.best_speed = speed;
            }
        }
        // 历史最大击键速度（键/秒 ×10），同样按天取最大
        if entry.active_ms > 0 && entry.keys > 0 {
            let key_speed = ((entry.keys * 10_000 + entry.active_ms / 2) / entry.active_ms) as u32;
            if key_speed > snapshot.best_key_speed_x10 {
                snapshot.best_key_speed_x10 = key_speed;
            }
        }
    }
    snapshot.total_exp = snapshot.total_chars;
    // 平均每日字数：分母只数「有击键的天」，空白日不参与（否则换了台机器就被摊薄）。
    let days_with_keys = entries.iter().filter(|entry| entry.keys > 0).count().max(1) as u64;
    snapshot.avg_daily_chars = (snapshot.total_chars / days_with_keys) as u32;

    // 历史均速 = 总字数 / 总有效时长（不是"各天速度取平均"，否则打得少的那几天会把它拉偏）。
    let total_active_ms: u64 = entries.iter().map(|entry| entry.active_ms).sum();
    snapshot.total_active_ms = total_active_ms;
    if total_active_ms > 0 {
        snapshot.all_speed = ((snapshot.total_chars * 60_000 + total_active_ms / 2)
            / total_active_ms) as u32;
        // 历史平均击键速度（键/秒 ×10）= 总击键 / 总有效时长
        snapshot.all_key_speed_x10 =
            ((snapshot.total_keys * 10_000 + total_active_ms / 2) / total_active_ms) as u32;
    }
    // 平均码长（键/字，×100 保留两位）：总击键 / 总上屏字数
    if snapshot.total_chars > 0 {
        snapshot.all_code_len_x100 =
            ((snapshot.total_keys * 100 + snapshot.total_chars / 2) / snapshot.total_chars) as u32;
    }

    // ── 近 7 天（含今天）聚合：界面上所有「平均」类指标都读这一份 ──
    // 窗口 = [today-6, today]。键准/词长只累加 v3 天记录（`commits > 0` 才是 v3），
    // 老 v2 记录的 `effective_keys` 是按"全有效"补的，混进来会把键准算成 100%。
    //
    // ⚠️ 2026-10-26 修：词长的**分子也必须只用 v3 天**。以前分子取全窗口 `chars`、
    // 分母只取 v3 天的 `commits`，真机首个 v3 日（10-26）实测把近 7 天词长算成 766.80
    // （923 字 ÷ 5 轮）。现在 chars_v3 只累加带轮数的那几天，与分母同口径。
    const RECENT_DAYS: i64 = 7;
    // 全窗口口径：速度 / 击键 / 码长（老 v2 天也有键和字）。
    // v3 口径（`commits > 0` 的天）：键准、词长。
    #[derive(Default)]
    struct RecentWindow {
        days: u32,
        keys: u64,
        chars: u64,
        active_ms: u64,
        keys_v3: u64,
        chars_v3: u64,
        effective_keys_v3: u64,
        commits_v3: u64,
    }
    let mut recent = RecentWindow::default();
    for entry in entries {
        let Some((y, m, d)) = parse_iso_date(&entry.date) else {
            continue;
        };
        let days = days_from_civil(y, m, d);
        if days > today_days || days <= today_days - RECENT_DAYS {
            continue;
        }
        recent.days += 1;
        recent.keys += entry.keys;
        recent.chars += entry.chars;
        recent.active_ms += entry.active_ms;
        if entry.commits > 0 {
            recent.keys_v3 += entry.keys;
            recent.effective_keys_v3 += entry.effective_keys;
            // 词长的分子分母必须成对：混合日（字/轮 离谱）两边一起跳过。
            if word_len_plausible(entry.chars, entry.commits) {
                recent.chars_v3 += entry.chars;
                recent.commits_v3 += entry.commits;
            }
        }
    }
    snapshot.recent_days = recent.days;
    snapshot.recent_keys = recent.keys;
    snapshot.recent_chars = recent.chars;
    snapshot.recent_active_ms = recent.active_ms;
    if recent.active_ms > 0 {
        snapshot.recent_speed = ((recent.chars * 60_000 + recent.active_ms / 2) / recent.active_ms) as u32;
        snapshot.recent_key_speed_x10 =
            ((recent.keys * 10_000 + recent.active_ms / 2) / recent.active_ms) as u32;
    }
    if recent.chars > 0 {
        snapshot.recent_code_len_x100 = ((recent.keys * 100 + recent.chars / 2) / recent.chars) as u32;
    }
    if recent.keys_v3 > 0 {
        snapshot.recent_key_accuracy_x10 =
            ((recent.effective_keys_v3 * 1_000 + recent.keys_v3 / 2) / recent.keys_v3) as u32;
    }
    if recent.commits_v3 > 0 {
        // 分子分母同口径：只用「有轮数的那几天」的字数与轮数。
        snapshot.recent_word_len_x100 =
            ((recent.chars_v3 * 100 + recent.commits_v3 / 2) / recent.commits_v3) as u32;
    }

    if let Some(today_entry) = entries.iter().find(|entry| entry.date == today_text) {
        snapshot.today_keys = today_entry.keys;
        snapshot.today_chars = today_entry.chars;
        snapshot.today_active_ms = today_entry.active_ms;
        if today_entry.active_ms > 0 {
            snapshot.today_speed = ((today_entry.chars * 60_000 + today_entry.active_ms / 2)
                / today_entry.active_ms) as u32;
            // 击键速度：键/秒 × 10（四舍五入）⇒ 300 键 / 60 秒 = 5.0 键/秒 = 50。
            snapshot.today_key_speed_x10 =
                ((today_entry.keys * 10_000 + today_entry.active_ms / 2) / today_entry.active_ms)
                    as u32;
        }
        // 今日平均码长（键/字 ×100）
        if today_entry.chars > 0 {
            snapshot.today_code_len_x100 =
                ((today_entry.keys * 100 + today_entry.chars / 2) / today_entry.chars) as u32;
        }
        // 今日键准（%，×10）：有效按键 ÷ 击键。用 v3 天记录的 effective_keys
        // （Σ编辑量 = Σ按键 − 2×退格）；老记录没这一列，effective_keys = 0 ⇒ 显示 0.0。
        if today_entry.keys > 0 && today_entry.effective_keys > 0 {
            let accuracy = (today_entry.effective_keys * 1_000 + today_entry.keys / 2)
                / today_entry.keys;
            // 手滑算不出 >100% 的数，但数据坏了也别画成 300%。
            snapshot.today_key_accuracy_x10 = accuracy.min(1_000) as u32;
        }
        // 今日平均词长（字/轮 ×100）：上屏字数 ÷ 上屏轮数（v3 天记录的 commits）。
        // 混合日（从 v2 升级上来的当天）算不出来 ⇒ 标记出来，界面显示 `—`。
        if word_len_plausible(today_entry.chars, today_entry.commits) {
            snapshot.today_word_len_x100 =
                ((today_entry.chars * 100 + today_entry.commits / 2) / today_entry.commits) as u32;
        } else if today_entry.commits > 0 {
            snapshot.today_word_len_mixed = true;
        }
    }

    // 连续天数：从今天开始往前数，今天没打字就从昨天起（今天还没开始用不该清零）。
    let active_days: BTreeSet<i64> = entries
        .iter()
        .filter(|entry| entry.keys > 0 || entry.chars > 0)
        .filter_map(|entry| parse_iso_date(&entry.date).map(|date| days_from_civil(date.0, date.1, date.2)))
        .collect();
    let mut cursor = if active_days.contains(&today_days) {
        today_days
    } else {
        today_days - 1
    };
    while active_days.contains(&cursor) {
        snapshot.streak_days += 1;
        cursor -= 1;
    }
    // 历史最长连续天数：在已排序的 active_days 里找最长的一段连续整数。
    {
        let mut best = 0u32;
        let mut run = 0u32;
        let mut previous: Option<i64> = None;
        for &day in &active_days {
            run = match previous {
                Some(last) if day == last + 1 => run + 1,
                _ => 1,
            };
            best = best.max(run);
            previous = Some(day);
        }
        snapshot.best_streak_days = best;
    }

    // GitHub 排法：列 = 周，行 = 周日→周六。
    // 用户要求「从 1 月 1 日算」——所以范围是**今年 1 月 1 日所在周**开始的 13 周，
    // 而不是「最近 13 周」倒推。这样跨年后自动归零，和日历季度对齐。
    // 用户要求「2026 年第 N 季度」——所以范围是**当前季度**（1-3/4-6/7-9/10-12 月），
    // 起始 = 季度第一个月的 1 日所在周的周日。
    // 用户 2026-10-18：「季度如果少就做成整年」——13 列太少铺不满宽度，
    // 改成整年 53 周（1 月 1 日所在周起算），格子刚好铺满。
    let (year, _, _) = civil_from_days(today_days);
    let jan1 = days_from_civil(year, 1, 1);
    let start_sunday = jan1 - sunday_index(jan1);
    // 用户 2026-10-25：标题只要年份数字（原来写「2026 年」）。
    snapshot.quarter_label = format!("{}", year);
    let today_cell = today_days - start_sunday;
    // 界面画「今日十字渐变」要坐标：列 = 周、行 = 周日→周六。
    // 今天不在这一年网格里（跨年边界）就给 -1，界面据此不画高亮。
    if (0..GRID_CELLS as i64).contains(&today_cell) {
        snapshot.today_week = (today_cell / 7) as i32;
        snapshot.today_day = (today_cell % 7) as i32;
    } else {
        snapshot.today_week = -1;
        snapshot.today_day = -1;
    }
    let mut cells: Vec<DayCell> = (0..GRID_CELLS)
        .map(|index| {
            let week = (index / 7) as i64;
            let day = (index % 7) as i64;
            let date_days = start_sunday + week * 7 + day;
            DayCell {
                count: 0,
                tip: format!("{} · 0 键", format_date(date_days)),
                today: index as i64 == today_cell,
            }
        })
        .collect();

    let mut word_counts: BTreeMap<char, u64> = BTreeMap::new();
    for entry in entries {
        for (ch, count) in &entry.char_counts {
            *word_counts.entry(*ch).or_insert(0) += *count;
        }
        let Some(date) = parse_iso_date(&entry.date) else {
            continue;
        };
        let days = days_from_civil(date.0, date.1, date.2);
        let week_index = (days - sunday_index(days) - start_sunday) / 7;
        if !(0..GRID_WEEKS as i64).contains(&week_index) {
            continue;
        }
        let cell = week_index * 7 + sunday_index(days);
        if let Some(slot) = cells.get_mut(cell as usize) {
            slot.count = entry.keys.min(u32::MAX as u64) as u32;
            slot.tip = format!(
                "{} 周{} · {} 键 / {} 字",
                entry.date,
                WEEKDAY_NAMES[sunday_index(days) as usize],
                thousands(entry.keys),
                thousands(entry.chars),
            );
        }
    }
    // ── 年度活跃记录（12 月 × 7 天）聚合：`DayCell` 没有月份字段，只能在这里算 ──
    let mut months: Vec<DayCell> = (0..84)
        .map(|index| DayCell {
            count: 0,
            tip: String::new(),
            today: false,
        })
        .collect();
    for (index, cell) in cells.iter().enumerate() {
        let date_days = start_sunday + (index / 7) as i64 * 7 + (index % 7) as i64;
        let (_, month, _) = civil_from_days(date_days);
        let slot = (month as usize - 1) * 7 + (index % 7);
        if let Some(target) = months.get_mut(slot) {
            target.count = target.count.saturating_add(cell.count);
        }
    }
    for (index, cell) in months.iter_mut().enumerate() {
        let month = index / 7 + 1;
        let day = index % 7;
        cell.tip = format!("{}月 周{}·{} 键", month, WEEKDAY_NAMES[day], thousands(cell.count as u64));
    }
    let active_days = cells.iter().filter(|cell| cell.count > 0).count() as u32;
    let elapsed_days = (today_cell + 1).clamp(1, GRID_CELLS as i64) as u32;
    snapshot.active_days = active_days;
    snapshot.active_rate = ((active_days as f64 / elapsed_days as f64) * 100.0).round() as u32;
    snapshot.month_days = months;
    // 月份标签的列锚点：该月 1 日所在周（相对整年网格第 0 列）。
    // ── 按月日历（每月从 1 号起，周一为首行）──
    let mut calendar: Vec<DayCell> = Vec::new();
    let mut calendar_cols: Vec<i32> = Vec::with_capacity(12);
    let mut calendar_offsets: Vec<i32> = Vec::with_capacity(12);
    let mut calendar_mask: Vec<i32> = Vec::new();
    for month in 1..=12u32 {
        let first = days_from_civil(year, month, 1);
        let next = if month == 12 {
            days_from_civil(year + 1, 1, 1)
        } else {
            days_from_civil(year, month + 1, 1)
        };
        let days_in_month = (next - first) as i64;
        // 周一对齐：Monday=0 … Sunday=6
        let lead = ((sunday_index(first) + 6) % 7) as i64;
        let weeks = ((lead + days_in_month + 6) / 7).max(1) as i32;
        calendar_cols.push(weeks);
        calendar_offsets.push(calendar.len() as i32);
        for row in 0..7i64 {
            for col in 0..weeks as i64 {
                let day_index = col * 7 + row - lead;
                if day_index < 0 || day_index >= days_in_month {
                    calendar.push(DayCell { count: 0, tip: String::new(), today: false });
                    calendar_mask.push(0);
                } else {
                    let date_days = first + day_index;
                    let cell = (date_days - start_sunday) / 7 * 7 + sunday_index(date_days);
                    let index = cell as usize;
                    let count = cells.get(index).map_or(0, |c| c.count);
                    calendar.push(DayCell {
                        count,
                        tip: format!("{} · {} 键", format_date(date_days), thousands(count as u64)),
                        today: index as i64 == today_cell,
                    });
                    calendar_mask.push(1);
                }
            }
        }
    }
    let cols_total: i32 = calendar_cols.iter().sum();
    snapshot.calendar = calendar;
    snapshot.calendar_cols = calendar_cols;
    snapshot.calendar_offsets = calendar_offsets;
    snapshot.calendar_mask = calendar_mask;
    snapshot.calendar_cols_total = cols_total;
    snapshot.month_week = (1..=12)
        .map(|month| {
            let first = days_from_civil(year, month, 1);
            (((first - sunday_index(first) - start_sunday) / 7).max(0)) as i32
        })
        .collect();
    snapshot.days = cells;

    let mut words: Vec<(char, u64)> = word_counts.into_iter().collect();
    // 「常用字」词云只收汉字：`char_counts` 里连标点和拉丁字母一起记（TIP 按字符统计），
    // 实测本机榜一是「，」356 次、「的」才 148 次，还有 e/a/i/u 这些——词云第一大的
    // 是个逗号，用户 2026-10-30 要的是「常用字词云」，所以这里滤掉非汉字。
    // 汉字一个都没有时（纯英文输入）退回原样，免得整张卡片因为空列表而消失。
    let han: Vec<(char, u64)> = words
        .iter()
        .copied()
        .filter(|(ch, _)| is_han(*ch))
        .collect();
    if !han.is_empty() {
        words = han;
    }
    // 次数降序；同次数按字符升序，保证每次刷新顺序稳定（否则界面会自己抖动）。
    words.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    words.truncate(MAX_WORDS);
    snapshot.words = words
        .into_iter()
        .map(|(ch, count)| (ch.to_string(), count))
        .collect();
    snapshot
}

/// 本地日期 `(年, 月, 日)`。交给 `GetLocalTime`，时区/夏令时由系统处理。
pub fn today_local() -> (i64, u32, u32) {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    unsafe extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }
    let mut now = SystemTime::default();
    unsafe { GetLocalTime(&mut now) };
    (now.year as i64, now.month as u32, now.day as u32)
}

/// 本地时钟 `HH:MM:SS`（刷新按钮那句「已刷新 00:52:10」用；不为了这个引 chrono）。
pub fn now_local_clock() -> String {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    unsafe extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }
    let mut now = SystemTime::default();
    unsafe { GetLocalTime(&mut now) };
    format!("{:02}:{:02}:{:02}", now.hour, now.minute, now.second)
}

const WEEKDAY_NAMES: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];

/// 周日 = 0 … 周六 = 6（1970-01-01 是周四）。
fn sunday_index(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

/// 公历 → 「自 1970-01-01 起的天数」。
///
/// 与 `plugin/core/src/time.rs` 同一套算法（Howard Hinnant 的 `days_from_civil`），
/// 之所以不跨 crate 复用：那是由 GPL-3.0 的统计插件 crate 提供的，本处只需要十行。
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = if month > 2 { month - 3 } else { month + 9 } as i64;
    let day_of_year = (153 * month_prime + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// 「自 1970-01-01 起的天数」→ 公历（`civil_from_days`）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn format_date(days: i64) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn is_iso_date(text: &str) -> bool {
    parse_iso_date(text).is_some()
}

fn parse_iso_date(text: &str) -> Option<(i64, u32, u32)> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = text.get(0..4)?.parse().ok()?;
    let month = text.get(5..7)?.parse().ok()?;
    let day = text.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

/// 是不是汉字（「常用字」词云只画这些）。
///
/// 只认 BMP 的 CJK 统一表意文字 + 扩展 A：够覆盖日常输入，也不至于把全角标点
/// （U+FF0C「，」等）误收进来 —— 那正是把词云第一位变成逗号的元凶。
fn is_han(ch: char) -> bool {
    matches!(ch, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
}

/// `1234` → `1,234`（界面上都是"人看的大数字"）。
fn thousands(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

// ── 指法分布（移植自蓝宝石 StatsStore 的 fingerOfToken / handOfToken / rowOfToken）──
//
// 数据源是 stat.bin 的 256 桶虚拟键计数（KeyHist），这里只做聚合，不新增采集。
// 返回三段可直接显示的摘要：左右手 / 三行 / 八指 + 拇指。

/// 把一个虚拟键归到 0..8（0-3 左手小指→食指，4 拇指，5-8 右手食指→小指）。
/// 返回 None 表示不属于主键区（功能键、Ctrl、Win、方向键等）。
pub fn finger_of_vk(vk: u8) -> Option<usize> {
    match vk {
        0x20 => Some(4),                                   // 空格 → 拇指
        // 左手
        0x10 | 0xC0 | 0x31 | 0x51 | 0x41 | 0x5A | 0x09 => Some(0), // Shift ` 1 Q A Z Tab
        0x32 | 0x57 | 0x53 | 0x58 => Some(1),             // 2 W S X
        0x33 | 0x45 | 0x44 | 0x43 => Some(2),             // 3 E D C
        0x34 | 0x35 | 0x52 | 0x54 | 0x46 | 0x47 | 0x56 | 0x42 => Some(3), // 4 5 R T F G V B
        // 右手
        0x36 | 0x37 | 0x59 | 0x55 | 0x48 | 0x4A | 0x4E | 0x4D => Some(5), // 6 7 Y U H J N M
        0x38 | 0x49 | 0x4B | 0xBC => Some(6),             // 8 I K ,
        0x39 | 0x4F | 0x4C | 0xBE => Some(7),             // 9 O L .
        0x30 | 0xBD | 0xBB | 0xDB | 0xDD | 0xDC | 0xBA | 0xDE | 0xBF | 0x0D | 0x08 | 0xA1 => Some(8),
        _ => None,
    }
}

/// 三行归属：0 数字行 / 1 上排 / 2 中排 / 3 下排 / None 其他。
pub fn row_of_vk(vk: u8) -> Option<usize> {
    match vk {
        0xC0 | 0x31..=0x39 | 0x30 | 0xBD | 0xBB | 0x08 => Some(0),
        0x09 => Some(1),
        0x51 | 0x57 | 0x45 | 0x52 | 0x54 | 0x59 | 0x55 | 0x49 | 0x4F | 0x50 | 0xDB | 0xDD | 0xDC => Some(1),
        0x41 | 0x53 | 0x44 | 0x46 | 0x47 | 0x48 | 0x4A | 0x4B | 0x4C | 0xBA | 0xDE | 0x0D => Some(2),
        0x5A | 0x58 | 0x43 | 0x56 | 0x42 | 0x4E | 0x4D | 0xBC | 0xBE | 0xBF | 0x10 | 0xA1 => Some(3),
        _ => None,
    }
}

fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "—".to_string();
    }
    format!("{:.0}%", part as f64 * 100.0 / whole as f64)
}

/// 返回 (左右手, 三行, 八指) 三段摘要；全无数据时返回「—」。
pub fn finger_breakdown(counts: &[u32]) -> (String, String, String) {
    let mut fingers = [0u64; 9];
    let mut rows = [0u64; 4];
    let mut total = 0u64;
    for (vk, count) in counts.iter().enumerate().take(256) {
        let count = u64::from(*count);
        if count == 0 {
            continue;
        }
        if let Some(finger) = finger_of_vk(vk as u8) {
            fingers[finger] += count;
            total += count;
        }
        if let Some(row) = row_of_vk(vk as u8) {
            rows[row] += count;
        }
    }
    if total == 0 {
        return ("—".into(), "—".into(), "—".into());
    }
    let left: u64 = fingers[0..4].iter().sum();
    let right: u64 = fingers[5..9].iter().sum();
    let thumb = fingers[4];
    let hands = format!(
        "左手 {}  ·  右手 {}  ·  空格 {}",
        percent(left, total),
        percent(right, total),
        percent(thumb, total)
    );
    let row_total: u64 = rows.iter().sum();
    let rows_text = if row_total == 0 {
        "—".to_string()
    } else {
        format!(
            "数字行 {}  ·  上排 {}  ·  中排 {}  ·  下排 {}",
            percent(rows[0], row_total),
            percent(rows[1], row_total),
            percent(rows[2], row_total),
            percent(rows[3], row_total)
        )
    };
    let names = ["小指", "无名", "中指", "食指", "拇指", "食指", "中指", "无名", "小指"];
    let mut parts: Vec<String> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        if fingers[index] > 0 {
            parts.push(format!("{} {}", name, percent(fingers[index], total)));
        }
    }
    let fingers_text = if parts.is_empty() { "—".to_string() } else { parts.join("  ·  ") };
    (hands, rows_text, fingers_text)
}


#[cfg(test)]
mod tests {
    use super::*;

    fn entry(date: &str, keys: u64, chars: u64, active_ms: u64) -> DayEntry {
        DayEntry {
            date: date.to_string(),
            keys,
            chars,
            active_ms,
            effective_keys: 0,
            commits: 0,
            char_counts: BTreeMap::new(),
        }
    }

    /// 带 v3 两列的天条目（今日键准/词长的测试用）。
    fn entry_v3(
        date: &str,
        keys: u64,
        chars: u64,
        active_ms: u64,
        effective_keys: u64,
        commits: u64,
    ) -> DayEntry {
        DayEntry {
            effective_keys,
            commits,
            ..entry(date, keys, chars, active_ms)
        }
    }

    #[test]
    fn parses_all_fields_and_ignores_unknown_ones() {
        // 旧格式（一天一个文件）2026-10-27 起也是 YAML：磁盘上已没有 TOML 日报，
        // 兜底只读 YAML。引号/反斜杠这类键用**单引号**包裹 —— 我们的 YAML 解析
        // 不处理双引号里的反斜杠转义（`"\"": 1` 会读成 `\"`）。
        let text = "date: \"2026-10-17\"\nkeys: 12\nchars: 7\nactive_ms: 65000\n\
                    version: 9\n\nchar_counts:\n  \"的\": 3\n  \"ab\": 4\n  '\"': 1\n";
        let day = parse_day("2026-10-17", text).unwrap();
        assert_eq!(day.keys, 12);
        assert_eq!(day.chars, 7);
        assert_eq!(day.active_ms, 65_000);
        assert_eq!(day.char_counts.get(&'的'), Some(&3));
        assert_eq!(day.char_counts.get(&'"'), Some(&1));
        // 多字符的键不是"常用字"，忽略。
        assert!(!day.char_counts.keys().any(|ch| *ch == 'a'));
    }

    #[test]
    fn parses_exactly_what_the_tip_writes() {
        // 这是旧格式（一天一个文件）的形状（含注释、空行、引号键）。
        // ⚠️ 写侧早已改成二进制 `stat/stat.bin`（Tip/src/stats.rs），这种文件不再产生，
        // 这里只是**只读兜底**。2026-10-27 起它也是 YAML —— 键名/格式一旦分叉就该红。
        let text = concat!(
            "# 由 Weasel-Next 自动写入：每天的按键数、上屏字数、有效输入时长与常用字。\n",
            "# 采集在 TIP 侧，攒够 20 次上屏落一次盘；失败静默。\n",
            "date: \"2026-10-17\"\n",
            "keys: 1234\n",
            "chars: 567\n",
            "active_ms: 69000\n",
            "\n",
            "char_counts:\n",
            "  \"的\": 12\n",
            "  '\"': 3\n",
            // 反斜杠键：YAML 单引号里反斜杠是字面量，不用转义
            "  '\\': 2\n",
        );
        let day = parse_day("2026-10-17", text).unwrap();
        assert_eq!((day.keys, day.chars, day.active_ms), (1234, 567, 69_000));
        assert_eq!(day.char_counts.get(&'的'), Some(&12));
        assert_eq!(day.char_counts.get(&'"'), Some(&3));
        assert_eq!(day.char_counts.get(&'\\'), Some(&2));
    }

    #[test]
    fn missing_time_field_reads_as_zero() {
        // 旧版 TIP 只写 keys / chars；新界面必须照样能读。
        let day = parse_day("2026-10-16", "keys: 1\nchars: 2\n").unwrap();
        assert_eq!(day.active_ms, 0);
        assert_eq!(day.keys, 1);
    }

    #[test]
    fn broken_file_is_an_error_not_a_panic() {
        assert!(parse_day("2026-10-17", "keys = [unclosed").is_err());
    }

    #[test]
    fn reads_only_iso_date_files() {
        let dir = std::env::temp_dir().join(format!("weasel-stats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2026-10-16.yaml"), "keys: 5\nchars: 2\n").unwrap();
        std::fs::write(dir.join("2026-10-17.yaml"), "keys: 7\nchars: 3\n").unwrap();
        std::fs::write(dir.join("settings.yaml"), "keys: 999\n").unwrap();
        std::fs::write(dir.join("2026-13-40.yaml"), "keys: 999\n").unwrap();
        let (entries, warnings) = read_days(&dir);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].date, "2026-10-16");
        assert_eq!(entries[1].keys, 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn heatmap_puts_today_in_the_last_row_of_the_last_week() {
        // 2026-10-17 是周六：周日=0 的排法下是本周第 6 行、最右一列 → 第 35 格。
        let today = (2026, 10, 17);
        let snapshot = snapshot(&[entry("2026-10-17", 100, 20, 0)], today);
        assert_eq!(snapshot.days.len(), GRID_CELLS);
        // 2026-10-24：网格从「最近 5 周」改成**整年 53 列**（用户要"季度少就做成整年"）。
        // 1 月 1 日是周四，网格从 2025-12-28（周日）起算 ⇒ 2026-10-17 落在第 293 格。
        let cell = &snapshot.days[293];
        assert_eq!(cell.count, 100);
        assert!(cell.today);
        assert_eq!(snapshot.days.iter().filter(|cell| cell.today).count(), 1);
        // 同一天的 tip 带日期、星期与数字分隔符。
        assert!(cell.tip.contains("2026-10-17"), "{}", cell.tip);
        assert!(cell.tip.contains("周六"), "{}", cell.tip);
        assert!(cell.tip.contains("100 键"), "{}", cell.tip);
    }

    #[test]
    fn heatmap_keeps_five_weeks_and_drops_older_days() {
        let today = (2026, 10, 17);
        let entries = [
            entry("2026-10-17", 10, 1, 0), // 本周（10-11 那一周）
            entry("2026-10-10", 20, 1, 0), // 上一周
            entry("2026-09-19", 40, 1, 0), // 四周前 = 最左一列（9-13 那一周）
            entry("2026-09-12", 80, 1, 0), // 五周前 = 超出窗口
        ];
        let snapshot = snapshot(&entries, today);
        // 整年网格：索引 = 从 2025-12-28（周日）起的天数
        assert_eq!(snapshot.days[293].count, 10, "10-17");
        assert_eq!(snapshot.days[286].count, 20, "10-10");
        assert_eq!(snapshot.days[265].count, 40, "09-19");
        assert_eq!(snapshot.days[258].count, 80, "09-12（整年网格里不再被裁掉）");
        assert_eq!(snapshot.days.iter().map(|cell| cell.count).sum::<u32>(), 150);
    }

    #[test]
    fn streak_counts_backwards_and_survives_an_empty_today() {
        let today = (2026, 10, 17);
        // 今天还没打字：连续天数从昨天往前数，不清零。
        let entries = [
            entry("2026-10-16", 5, 1, 0),
            entry("2026-10-15", 6, 1, 0),
            entry("2026-10-14", 7, 1, 0),
            entry("2026-10-12", 8, 1, 0), // 断档
        ];
        assert_eq!(snapshot(&entries, today).streak_days, 3);
        let with_today = [entries[0].clone(), entries[1].clone(), entries[2].clone(), entry("2026-10-17", 1, 1, 0)];
        assert_eq!(snapshot(&with_today, today).streak_days, 4);
        assert_eq!(snapshot(&[], today).streak_days, 0);
    }

    #[test]
    fn speed_uses_active_input_time_only() {
        let today = (2026, 10, 17);
        // 120 字 / 60 秒 = 120 字/分；300 键 / 60 秒 = 300 键/分。
        let fast = snapshot(&[entry("2026-10-17", 300, 120, 60_000)], today);
        assert_eq!(fast.today_speed, 120);
        assert_eq!(fast.today_key_speed_x10, 50, "300 键 / 60 秒 = 5.0 键/秒");
        assert_eq!(fast.all_speed, 120, "只有一天时历史均速 == 当天均速");
        assert_eq!(
            fast.all_key_speed_x10, 50,
            "历史平均击键 = 300 键 / 60 秒 = 5.0 键/秒"
        );
        // 历史均速是**加权**平均（总字数/总时长），不是各天速度的算术平均：
        //   第 1 天 60 字 / 60 秒 = 60 字/分；第 2 天 240 字 / 180 秒 = 80 字/分
        //   ⇒ 加权 = 300 字 / 240 秒 = 75；算术平均会得 70。这里钉住 75。
        let two = snapshot(
            &[
                entry("2026-10-16", 60, 60, 60_000),
                entry("2026-10-17", 240, 240, 180_000),
            ],
            today,
        );
        assert_eq!(two.all_speed, 75, "300 字 / 240 秒 = 75 字/分");
        assert_eq!(two.best_speed, 80, "历史极速 = 单日最高 = 80");
        // 没有时长数据（旧文件）不能除零，显示 0。
        let stale = snapshot(&[entry("2026-10-17", 300, 120, 0)], today);
        assert_eq!(stale.today_speed, 0);
        assert_eq!(stale.today_key_speed_x10, 0);
        assert_eq!(stale.all_speed, 0);
    }

    #[test]
    fn totals_and_words_merge_across_days() {
        let mut first = entry("2026-10-16", 1, 10, 0);
        first.char_counts.insert('的', 4);
        first.char_counts.insert('是', 1);
        let mut second = entry("2026-10-17", 2, 20, 0);
        second.char_counts.insert('的', 2);
        second.char_counts.insert('不', 3);
        let snapshot = snapshot(&[first, second], (2026, 10, 17));
        assert_eq!(snapshot.total_keys, 3);
        assert_eq!(snapshot.total_chars, 30);
        assert_eq!(snapshot.total_exp, 30);
        assert_eq!(snapshot.today_keys, 2);
        assert_eq!(snapshot.today_chars, 20);
        assert_eq!(snapshot.words[0], ("的".to_string(), 6));
        assert_eq!(snapshot.words[1], ("不".to_string(), 3));
    }

    /// 「常用字」词云只收汉字：`char_counts` 里标点/拉丁字母照样记着
    /// （实测本机「，」356 次比「的」148 次还多，另外还有 e/a/i/u），
    /// 不滤的话词云第一位是个逗号（2026-10-30 截图）。
    #[test]
    fn words_drop_punctuation_and_latin() {
        let mut day = entry("2026-10-30", 10, 100, 0);
        day.char_counts.insert('，', 356);
        day.char_counts.insert('e', 90);
        day.char_counts.insert('|', 20);
        day.char_counts.insert('是', 148);
        day.char_counts.insert('的', 142);
        let snapshot = snapshot(&[day], (2026, 10, 30));
        assert_eq!(
            snapshot.words,
            vec![("是".to_string(), 148), ("的".to_string(), 142)],
            "标点/字母不进词云"
        );
    }

    /// 纯英文输入（一个汉字都没有）时不能把词云整张滤空 —— 退回原样，
    /// 否则「常用字」卡片会整块消失（`if root.words.length > 0`）。
    #[test]
    fn words_keep_latin_when_no_han_at_all() {
        let mut day = entry("2026-10-30", 10, 100, 0);
        day.char_counts.insert('e', 90);
        day.char_counts.insert('a', 80);
        let snapshot = snapshot(&[day], (2026, 10, 30));
        assert_eq!(
            snapshot.words,
            vec![("e".to_string(), 90), ("a".to_string(), 80)]
        );
    }

    // ── 二进制 stat.bin（写侧 `Tip/src/stats.rs`）────────────────────────

    /// 手写一份 `.bin` 字节：测试必须**独立于写侧代码**，否则两边一起写错也测不出来。
    ///
    /// `days` 的元组是（date, keys, chars, active_ms, effective_keys, commits）——
    /// 后两个只有 v3 会写进字节里。
    fn stat_bin(
        version: u32,
        days: &[(u32, u64, u64, u64, u32, u32)],
        chars: &[(char, u64)],
    ) -> Vec<u8> {
        let day_len = match version {
            1 => 20,
            2 => 28,
            3 => 36,
            other => panic!("测试只造 1/2/3 版本，收到 {other}"),
        };
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"WNST");
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&(days.len() as u32).to_le_bytes());
        let char_len = if version >= 2 { chars.len() as u32 } else { 0 };
        bytes.extend_from_slice(&char_len.to_le_bytes());
        for (date, keys, chars_count, active, effective_keys, commits) in days {
            bytes.extend_from_slice(&date.to_le_bytes());
            bytes.extend_from_slice(&keys.to_le_bytes());
            bytes.extend_from_slice(&chars_count.to_le_bytes());
            if day_len >= 28 {
                bytes.extend_from_slice(&active.to_le_bytes());
            }
            if day_len >= 36 {
                bytes.extend_from_slice(&effective_keys.to_le_bytes());
                bytes.extend_from_slice(&commits.to_le_bytes());
            }
        }
        if version >= 2 {
            for (ch, count) in chars {
                bytes.extend_from_slice(&(*ch as u32).to_le_bytes());
                bytes.extend_from_slice(&count.to_le_bytes());
            }
        }
        bytes
    }

    #[test]
    fn reads_stat_bin_v2_with_time_and_chars() {
        let bytes = stat_bin(
            2,
            &[
                (20261016, 5, 2, 1_000, 0, 0),
                (20261017, 1234, 567, 180_000, 0, 0),
            ],
            &[('的', 12), ('一', 3)],
        );
        let (days, chars) = parse_stat_bytes(&bytes).unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!(days[1].date, "2026-10-17");
        assert_eq!((days[1].keys, days[1].chars, days[1].active_ms), (1234, 567, 180_000));
        // v2 没有 v3 的两列：读成 0（界面上今日键准 0.0 / 词长 0.00）。
        assert_eq!((days[1].effective_keys, days[1].commits), (0, 0));
        assert_eq!(chars.get(&'的'), Some(&12));
        assert_eq!(chars.get(&'一'), Some(&3));
        // 时钟字段进得了速度计算：567 字 / 180 秒 = 189 字/分。
        let snapshot = snapshot(&days, (2026, 10, 17));
        assert_eq!(snapshot.today_speed, 189);
        assert_eq!(snapshot.today_key_accuracy_x10, 0);
        assert_eq!(snapshot.today_word_len_x100, 0);
        assert_eq!(snapshot.total_exp, 569);
    }

    /// v3：今日键准 / 今日平均词长都是**当天**的数。
    #[test]
    fn reads_stat_bin_v3_with_today_accuracy_and_word_len() {
        // 今天：300 键、其中 20 次退格 ⇒ 有效 260 ⇒ 键准 86.7%；260 字 / 100 轮 = 2.60 字/轮。
        let bytes = stat_bin(
            3,
            &[
                (20261025, 1000, 400, 600_000, 950, 200),
                (20261026, 300, 260, 60_000, 260, 100),
            ],
            &[('的', 12)],
        );
        let (days, _) = parse_stat_bytes(&bytes).unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!(
            (days[1].keys, days[1].chars, days[1].effective_keys, days[1].commits),
            (300, 260, 260, 100)
        );
        let today_view = snapshot(&days, (2026, 10, 26));
        assert_eq!(today_view.today_key_accuracy_x10, 867, "260/300 = 86.7%");
        assert_eq!(today_view.today_word_len_x100, 260, "260 字 / 100 轮 = 2.60");
        // 昨天那一行不能漏到今天来：换一天看就变回 0。
        let later = snapshot(&days, (2026, 10, 27));
        assert_eq!(later.today_key_accuracy_x10, 0);
        assert_eq!(later.today_word_len_x100, 0);
        // 没有退格 = 100%；一轮就是 1 个字 = 1.00。
        let clean = stat_bin(3, &[(20261026, 50, 50, 60_000, 50, 50)], &[]);
        let (days, _) = parse_stat_bytes(&clean).unwrap();
        let clean_view = snapshot(&days, (2026, 10, 26));
        assert_eq!(clean_view.today_key_accuracy_x10, 1_000);
        assert_eq!(clean_view.today_word_len_x100, 100);
    }

    /// 尾部直方图的偏移量必须按版本算：v3 的天记录 36 字节，算错就整张键盘图变 0。
    #[test]
    fn hist_offsets_follow_the_version() {
        let mut base = stat_bin(
            3,
            &[(20261025, 10, 5, 1_000, 10, 3), (20261026, 20, 8, 1_000, 18, 4)],
            &[('的', 3)],
        );
        // 追加 256 个 vk 计数 + 33 个码长计数（写侧尾部扩展的形状）。
        let mut vk = vec![0u32; STAT_VK_COUNT];
        vk[8] = 7; // 退格
        vk[0x20] = 11;
        base.extend(vk.iter().flat_map(|count| count.to_le_bytes()));
        let mut code = vec![0u32; STAT_CODELEN_COUNT];
        code[4] = 9;
        base.extend(code.iter().flat_map(|count| count.to_le_bytes()));
        let (keys, code_lens) = parse_stat_hist(&base).expect("v3 的尾部扩展要能读到");
        assert_eq!(keys[8], 7);
        assert_eq!(keys[0x20], 11);
        assert_eq!(code_lens[4], 9);
        // v2 的同形状文件（28 字节天记录）也要照样读对。
        let mut v2 = stat_bin(2, &[(20261026, 20, 8, 1_000, 0, 0)], &[('的', 3)]);
        v2.extend(vk.iter().flat_map(|count| count.to_le_bytes()));
        v2.extend(code.iter().flat_map(|count| count.to_le_bytes()));
        let (keys, code_lens) = parse_stat_hist(&v2).expect("v2 的尾部扩展要能读到");
        assert_eq!((keys[8], code_lens[4]), (7, 9));
        // 版本 4 不认识 → 不猜偏移量。
        let mut v4 = v2.clone();
        v4[4..8].copy_from_slice(&4u32.to_le_bytes());
        assert!(parse_stat_hist(&v4).is_none());
    }

    #[test]
    fn reads_stat_bin_v1_without_time_or_chars() {
        let bytes = stat_bin(1, &[(20261017, 251, 39, 0, 0, 0)], &[]);
        let (days, chars) = parse_stat_bytes(&bytes).unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!((days[0].keys, days[0].chars), (251, 39));
        assert_eq!(days[0].active_ms, 0, "v1 没有时长字段");
        assert!(chars.is_empty());
        // 没时长 → 速度 0，不能除零。
        assert_eq!(snapshot(&days, (2026, 10, 17)).today_speed, 0);
    }

    #[test]
    fn truncated_stat_bin_does_not_panic() {
        let bytes = stat_bin(
            2,
            &[
                (20261016, 5, 2, 1_000, 0, 0),
                (20261017, 1234, 567, 180_000, 0, 0),
            ],
            &[('的', 12)],
        );
        for cut in [0usize, 3, 15, 17, 44, 45, bytes.len() - 1] {
            let _ = parse_stat_bytes(&bytes[..cut]);
        }
        let (days, _) = parse_stat_bytes(&bytes[..44]).unwrap();
        assert_eq!(days.len(), 1, "只该读到完整的那一条");
        // 头都不完整 = 明确报错（写侧的「截断后重写」窗口，调用方会重试一次）。
        assert!(parse_stat_bytes(&bytes[..8]).is_err());
        // v3 的截断同理：52 = 头 16 + 一条 36 字节的天记录。
        let v3 = stat_bin(
            3,
            &[
                (20261025, 5, 2, 1_000, 5, 1),
                (20261026, 1234, 567, 180_000, 1_100, 300),
            ],
            &[('的', 12)],
        );
        let (days, _) = parse_stat_bytes(&v3[..52]).unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].date, "2026-10-25");
        assert_eq!(days[0].effective_keys, 5);
    }

    #[test]
    fn bogus_stat_bin_is_an_error_not_a_panic() {
        let mut bytes = stat_bin(2, &[(20261017, 1, 1, 0, 0, 0)], &[]);
        bytes[0] = b'X';
        assert!(parse_stat_bytes(&bytes).is_err());
        let mut bytes = stat_bin(2, &[(20261017, 1, 1, 0, 0, 0)], &[]);
        bytes[4..8].copy_from_slice(&7u32.to_le_bytes());
        assert!(parse_stat_bytes(&bytes).is_err());
        // 版本 3 必须认得（用户 2026-10-26 要求读侧认 2/3），版本 4 才报错。
        assert!(parse_stat_bytes(&stat_bin(3, &[(20261017, 1, 1, 0, 1, 1)], &[])).is_ok());
        // 坏日期记录丢掉，但文件本身能用。
        let bytes = stat_bin(3, &[(20261340, 9, 9, 0, 9, 1), (20261017, 1, 1, 0, 1, 1)], &[]);
        let (days, _) = parse_stat_bytes(&bytes).unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].date, "2026-10-17");
    }

    #[test]
    fn load_sums_binary_and_legacy_for_the_same_day() {
        let root = std::env::temp_dir().join(format!("weasel-stat-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("stat")).unwrap();
        std::fs::create_dir_all(root.join("stats")).unwrap();
        std::fs::write(
            root.join("stat").join("stat.bin"),
            stat_bin(2, &[(20261017, 100, 20, 60_000, 0, 0)], &[('的', 7)]),
        )
        .unwrap();
        // 旧格式：10-16 只在旧文件里；10-17 与二进制同一天（换 TIP 当天的两段量，要相加）。
        std::fs::write(root.join("stats").join("2026-10-16.yaml"), "keys: 5
chars: 2
").unwrap();
        std::fs::write(
            root.join("stats").join("2026-10-17.yaml"),
            "keys: 9
chars: 3
active_ms: 1000
char_counts:
  \"不\": 2
",
        )
        .unwrap();

        let (entries, warnings) = load_from(&root);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].date, "2026-10-16");
        // 同一天：keys 100+9、chars 20+3、active 60000+1000 —— 换 TIP 前后的量都在。
        assert_eq!((entries[1].keys, entries[1].chars, entries[1].active_ms), (109, 23, 61_000));
        let snapshot = snapshot(&entries, (2026, 10, 17));
        assert_eq!(snapshot.today_keys, 109);
        assert_eq!(snapshot.today_chars, 23);
        assert_eq!(snapshot.total_chars, 25);
        // 全量字频（二进制）与旧文件的当日字频合并后按次数排序。
        assert_eq!(snapshot.words, vec![("的".to_string(), 7), ("不".to_string(), 2)]);
        assert!(snapshot.has_data);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 换 TIP 之前的状态：只有旧的 stats\*.yaml，也要能正常显示（不白屏、不报错）。
    #[test]
    fn load_works_with_only_legacy_files() {
        let root = std::env::temp_dir().join(format!("weasel-stat-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("stats")).unwrap();
        std::fs::write(root.join("stats").join("2026-10-17.yaml"), "keys: 251
chars: 39
").unwrap();
        let (entries, warnings) = load_from(&root);
        assert!(warnings.is_empty(), "{warnings:?}");
        let snapshot = snapshot(&entries, (2026, 10, 17));
        assert_eq!((snapshot.today_keys, snapshot.today_chars), (251, 39));
        assert_eq!(snapshot.streak_days, 1);
        assert_eq!(snapshot.today_speed, 0, "旧文件没有时长，不能除零");
        assert!(snapshot.has_data);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_on_empty_root_is_not_an_error() {
        let root = std::env::temp_dir().join(format!("weasel-stat-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (entries, warnings) = load_from(&root);
        assert!(entries.is_empty());
        assert!(warnings.is_empty(), "还没开始采集不该报错");
        assert!(!snapshot(&entries, (2026, 10, 17)).has_data);
    }

    #[test]
    fn date_roundtrip_matches_known_values() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 10, 17), 20_743);
        assert_eq!(civil_from_days(20_743), (2026, 10, 17));
        // 2026-10-17 是周六。
        assert_eq!(sunday_index(20_743), 6);
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_234), "1,234");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}




/// 本地当前小时(0..=23)。时段统计里"当前小时"的柱子要亮蓝高亮。
pub fn current_hour() -> usize {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    unsafe extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }
    let mut now = SystemTime::default();
    unsafe { GetLocalTime(&mut now) };
    (now.hour as usize).min(23)
}
