//! 输入统计的数据层：读 `%APPDATA%\Rime-Next\stat\stat.bin`。
//!
//! 写侧是 TIP（`Tip/src/stats.rs`），读侧是 `/stat` 窗口。**格式以写侧注释为准**，
//! 两边改一处必须同时改另一处（`Tip/src/stats.rs` 的 `encode`/`parse_file` 与本文件
//! 的 `parse_stat_bytes` 是同一份契约，各有一组往返测试看着）。
//!
//! ```text
//! 头 16 字节： "WNST" | version u32 | 天记录数 u32 | 字记录数 u32
//! 天记录 28B： date=YYYYMMDD u32 | keys u64 | chars u64 | active_ms u64
//! 字记录 12B： codepoint u32 | count u64        （按次数降序，最多 200 个）
//! ```
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

/// 热力图：7 行（周日→周六）× 5 列（周），共 35 格。
///
/// 与 GitHub 贡献图同一套排法：**列 = 周、行 = 星期（周日在上）**，
/// 最右一列是本周。Slint 侧按 `week * 7 + day` 索引，所以数组必须恒为 35 个。
pub const GRID_WEEKS: usize = 5;
/// 热力图格子总数。
pub const GRID_CELLS: usize = GRID_WEEKS * 7;
/// 「常用字」最多显示几个。
pub const MAX_WORDS: usize = 12;

/// 用户数据根：`%APPDATA%\Rime-Next`。
pub fn state_root() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("Rime-Next"))
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
    /// 连续使用天数（含今天；今天还没打字就从昨天往前数）。
    pub streak_days: u32,
    /// 累计击键。
    pub total_keys: u64,
    /// 累计上屏字数。
    pub total_chars: u64,
    /// 累计经验（= 累计上屏字数，段位表按它分段）。
    pub total_exp: u64,
    /// 恒为 [`GRID_CELLS`] 个。
    pub days: Vec<DayCell>,
    /// 常用字（字, 次数），按次数降序。
    pub words: Vec<(String, u64)>,
    /// 至少读到过一天的数据。
    pub has_data: bool,
    /// 读文件时遇到的问题（给状态栏用）。
    pub warnings: Vec<String>,
}

/// 扫目录，读出所有 `YYYY-MM-DD.toml`。
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
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        let Some(date) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        // 只认 `YYYY-MM-DD` —— 以后 stats\ 里若多出别的 toml（配置之类），不能被当成一天。
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

/// 解析一天的统计文件。字段缺失当 0；整份不是合法 TOML 才算错。
pub fn parse_day(date: &str, text: &str) -> Result<DayEntry, String> {
    let value: toml::Value = toml::from_str(text).map_err(|error| format!("{date}: {error}"))?;
    let number = |key: &str| {
        value
            .get(key)
            .and_then(toml::Value::as_integer)
            .filter(|value| *value > 0)
            .unwrap_or(0) as u64
    };
    let mut char_counts = BTreeMap::new();
    if let Some(table) = value.get("char_counts").and_then(toml::Value::as_table) {
        for (key, count) in table {
            let mut chars = key.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                continue;
            };
            if ch.is_control() {
                continue;
            }
            if let Some(count) = count.as_integer().filter(|count| *count > 0) {
                char_counts.insert(ch, count as u64);
            }
        }
    }
    Ok(DayEntry {
        date: date.to_string(),
        keys: number("keys"),
        chars: number("chars"),
        active_ms: number("active_ms"),
        char_counts,
    })
}

/// 二进制统计文件的布局（与写侧 `Tip/src/stats.rs` 同值，改一处必须同时改另一处）。
const STAT_HEADER_LEN: usize = 16;
const STAT_DAY_LEN_V1: usize = 20;
const STAT_DAY_LEN_V2: usize = 28;
const STAT_CHAR_LEN: usize = 12;
const STAT_MAGIC: [u8; 4] = *b"WNST";

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
    if version != 1 && version != 2 {
        return Err(format!("统计文件版本 {version} 不认识（本程序认 1/2）"));
    }
    let day_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let char_count = if version >= 2 {
        u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize
    } else {
        0
    };
    let day_len = if version >= 2 { STAT_DAY_LEN_V2 } else { STAT_DAY_LEN_V1 };

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
    }
    snapshot.total_exp = snapshot.total_chars;

    if let Some(today_entry) = entries.iter().find(|entry| entry.date == today_text) {
        snapshot.today_keys = today_entry.keys;
        snapshot.today_chars = today_entry.chars;
        if today_entry.active_ms > 0 {
            snapshot.today_speed = ((today_entry.chars * 60_000 + today_entry.active_ms / 2)
                / today_entry.active_ms) as u32;
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

    // GitHub 排法：列 = 周，行 = 周日→周六，最右一列是本周。
    let today_weekday = sunday_index(today_days);
    let this_sunday = today_days - today_weekday;
    let today_cell = (GRID_WEEKS as i64 - 1) * 7 + today_weekday;
    let mut cells: Vec<DayCell> = (0..GRID_CELLS)
        .map(|index| {
            let week = (index / 7) as i64;
            let day = (index % 7) as i64;
            let date_days = this_sunday - (GRID_WEEKS as i64 - 1 - week) * 7 + day;
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
        let weeks_ago = (this_sunday - (days - sunday_index(days))) / 7;
        if !(0..GRID_WEEKS as i64).contains(&weeks_ago) {
            continue;
        }
        let cell = (GRID_WEEKS as i64 - 1 - weeks_ago) * 7 + sunday_index(days);
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
    snapshot.days = cells;

    let mut words: Vec<(char, u64)> = word_counts.into_iter().collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(date: &str, keys: u64, chars: u64, active_ms: u64) -> DayEntry {
        DayEntry {
            date: date.to_string(),
            keys,
            chars,
            active_ms,
            char_counts: BTreeMap::new(),
        }
    }

    #[test]
    fn parses_all_fields_and_ignores_unknown_ones() {
        let text = "date = \"2026-10-17\"\nkeys = 12\nchars = 7\nactive_ms = 65000\n\
                    version = 9\n\n[char_counts]\n\"的\" = 3\n\"ab\" = 4\n\"\\\"\" = 1\n";
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
        // 这是 `Tip/src/stats.rs::render` 的**逐字节**输出形状（含注释、空行、引号键）。
        // 两边键名/格式一旦分叉，这个测试就该红 —— 别只改一边。
        let text = "# 由 Weasel-Next 自动写入：每天的按键数、上屏字数、有效输入时长与常用字。\n\
                    # 采集在 TIP 侧，攒够 20 次上屏落一次盘；失败静默。\n\
                    date = \"2026-10-17\"\n\
                    keys = 1234\n\
                    chars = 567\n\
                    active_ms = 69000\n\
                    \n\
                    [char_counts]\n\
                    \"的\" = 12\n\
                    \"\\\"\" = 3\n\
                    \"\\\\\" = 2\n";
        let day = parse_day("2026-10-17", text).unwrap();
        assert_eq!((day.keys, day.chars, day.active_ms), (1234, 567, 69_000));
        assert_eq!(day.char_counts.get(&'的'), Some(&12));
        assert_eq!(day.char_counts.get(&'"'), Some(&3));
        assert_eq!(day.char_counts.get(&'\\'), Some(&2));
    }

    #[test]
    fn missing_time_field_reads_as_zero() {
        // 旧版 TIP 只写 keys / chars；新界面必须照样能读。
        let day = parse_day("2026-10-16", "keys = 1\nchars = 2\n").unwrap();
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
        std::fs::write(dir.join("2026-10-16.toml"), "keys = 5\nchars = 2\n").unwrap();
        std::fs::write(dir.join("2026-10-17.toml"), "keys = 7\nchars = 3\n").unwrap();
        std::fs::write(dir.join("settings.toml"), "keys = 999\n").unwrap();
        std::fs::write(dir.join("2026-13-40.toml"), "keys = 999\n").unwrap();
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
        let cell = &snapshot.days[34];
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
        assert_eq!(snapshot.days[34].count, 10);
        assert_eq!(snapshot.days[27].count, 20, "上周六 = 第 28 格");
        assert_eq!(snapshot.days[6].count, 40, "四周前的周六 = 第 7 格");
        assert_eq!(snapshot.days.iter().map(|cell| cell.count).sum::<u32>(), 70);
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
        // 120 字 / 60 秒 = 120 字/分。
        let fast = snapshot(&[entry("2026-10-17", 300, 120, 60_000)], today);
        assert_eq!(fast.today_speed, 120);
        // 没有时长数据（旧文件）不能除零，显示 0。
        assert_eq!(snapshot(&[entry("2026-10-17", 300, 120, 0)], today).today_speed, 0);
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

    // ── 二进制 stat.bin（写侧 `Tip/src/stats.rs`）────────────────────────

    /// 手写一份 `.bin` 字节：测试必须**独立于写侧代码**，否则两边一起写错也测不出来。
    fn stat_bin(version: u32, days: &[(u32, u64, u64, u64)], chars: &[(char, u64)]) -> Vec<u8> {
        let day_len = if version >= 2 { 28 } else { 20 };
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"WNST");
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&(days.len() as u32).to_le_bytes());
        let char_len = if version >= 2 { chars.len() as u32 } else { 0 };
        bytes.extend_from_slice(&char_len.to_le_bytes());
        for (date, keys, chars_count, active) in days {
            bytes.extend_from_slice(&date.to_le_bytes());
            bytes.extend_from_slice(&keys.to_le_bytes());
            bytes.extend_from_slice(&chars_count.to_le_bytes());
            if day_len >= 28 {
                bytes.extend_from_slice(&active.to_le_bytes());
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
            &[(20261016, 5, 2, 1_000), (20261017, 1234, 567, 180_000)],
            &[('的', 12), ('一', 3)],
        );
        let (days, chars) = parse_stat_bytes(&bytes).unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!(days[1].date, "2026-10-17");
        assert_eq!((days[1].keys, days[1].chars, days[1].active_ms), (1234, 567, 180_000));
        assert_eq!(chars.get(&'的'), Some(&12));
        assert_eq!(chars.get(&'一'), Some(&3));
        // 时钟字段进得了速度计算：567 字 / 180 秒 = 189 字/分。
        let snapshot = snapshot(&days, (2026, 10, 17));
        assert_eq!(snapshot.today_speed, 189);
        assert_eq!(snapshot.total_exp, 569);
    }

    #[test]
    fn reads_stat_bin_v1_without_time_or_chars() {
        let bytes = stat_bin(1, &[(20261017, 251, 39, 0)], &[]);
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
            &[(20261016, 5, 2, 1_000), (20261017, 1234, 567, 180_000)],
            &[('的', 12)],
        );
        for cut in [0usize, 3, 15, 17, 44, 45, bytes.len() - 1] {
            let _ = parse_stat_bytes(&bytes[..cut]);
        }
        let (days, _) = parse_stat_bytes(&bytes[..44]).unwrap();
        assert_eq!(days.len(), 1, "只该读到完整的那一条");
        // 头都不完整 = 明确报错（写侧的「截断后重写」窗口，调用方会重试一次）。
        assert!(parse_stat_bytes(&bytes[..8]).is_err());
    }

    #[test]
    fn bogus_stat_bin_is_an_error_not_a_panic() {
        let mut bytes = stat_bin(2, &[(20261017, 1, 1, 0)], &[]);
        bytes[0] = b'X';
        assert!(parse_stat_bytes(&bytes).is_err());
        let mut bytes = stat_bin(2, &[(20261017, 1, 1, 0)], &[]);
        bytes[4..8].copy_from_slice(&7u32.to_le_bytes());
        assert!(parse_stat_bytes(&bytes).is_err());
        // 坏日期记录丢掉，但文件本身能用。
        let bytes = stat_bin(2, &[(20261340, 9, 9, 0), (20261017, 1, 1, 0)], &[]);
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
            stat_bin(2, &[(20261017, 100, 20, 60_000)], &[('的', 7)]),
        )
        .unwrap();
        // 旧格式：10-16 只在旧文件里；10-17 与二进制同一天（换 TIP 当天的两段量，要相加）。
        std::fs::write(root.join("stats").join("2026-10-16.toml"), "keys = 5
chars = 2
").unwrap();
        std::fs::write(
            root.join("stats").join("2026-10-17.toml"),
            "keys = 9
chars = 3
active_ms = 1000

[char_counts]
\"不\" = 2
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

    /// 换 TIP 之前的状态：只有旧的 stats\*.toml，也要能正常显示（不白屏、不报错）。
    #[test]
    fn load_works_with_only_legacy_toml() {
        let root = std::env::temp_dir().join(format!("weasel-stat-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("stats")).unwrap();
        std::fs::write(root.join("stats").join("2026-10-17.toml"), "keys = 251
chars = 39
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
