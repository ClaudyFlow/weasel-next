//! 本地时区的时间桶边界。
//!
//! 对齐 Lua 版的 `start_of_day` / `start_of_week` / `start_of_month` / `start_of_year`：
//! 都取**本地时区**的零点，且 **周起点是周一**（Lua 里 `wday == 1` 表示周日，代码把它回退 6 天）。
//!
//! 时区偏移由宿主提供（Lua 版用 `os.date`/`os.time` 自己算），因此本模块保持纯函数、可测试。
//! 注意：偏移在整个时间桶内按常量处理，跨夏令时切换的极端情况与 Lua 版行为不完全一致，
//! 宿主应在每次调用时传入当前偏移。

use crate::state::BucketBounds;

const SECONDS_PER_DAY: i64 = 86_400;

/// 把「自 1970-01-01 起的天数」转换为公历日期（Howard Hinnant 的 civil_from_days）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 把公历日期转换为「自 1970-01-01 起的天数」（Howard Hinnant 的 days_from_civil）。
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// 计算 `now_unix` 所在本地日/周/月/年的起点（Unix 秒）。
///
/// `tz_offset_seconds` 是本地时间相对 UTC 的偏移（东八区为 `8 * 3600`）。
pub fn bucket_bounds(now_unix: i64, tz_offset_seconds: i32) -> BucketBounds {
    let offset = tz_offset_seconds as i64;
    let local_seconds = now_unix + offset;
    let local_days = local_seconds.div_euclid(SECONDS_PER_DAY);
    let (year, month, _) = civil_from_days(local_days);

    // 0 = 周日, 1 = 周一 … 6 = 周六（1970-01-01 是周四）。
    let weekday = (local_days + 4).rem_euclid(7);
    let monday_back = if weekday == 0 { 6 } else { weekday - 1 };

    let to_unix = |days: i64| days * SECONDS_PER_DAY - offset;

    BucketBounds {
        day: to_unix(local_days),
        week: to_unix(local_days - monday_back),
        month: to_unix(days_from_civil(year, month, 1)),
        year: to_unix(days_from_civil(year, 1, 1)),
    }
}

/// 生成 Lua 版同款的时区标签，例如 `UTC+08:00`。
pub fn timezone_label(tz_offset_seconds: i32) -> String {
    let total_minutes = tz_offset_seconds / 60;
    let hours = total_minutes / 60;
    let minutes = (total_minutes % 60).abs();
    format!("UTC{hours:+03}:{minutes:02}")
}

/// 本地时间字符串 `YYYY/MM/DD HH:MM:SS`（Lua `os.date("%Y/%m/%d %H:%M:%S")`）。
pub fn format_datetime(unix_seconds: i64, tz_offset_seconds: i32) -> String {
    let local = unix_seconds + tz_offset_seconds as i64;
    let days = local.div_euclid(SECONDS_PER_DAY);
    let seconds_of_day = local.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}/{month:02}/{day:02} {hour:02}:{minute:02}:{second:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monday_is_the_week_start() {
        // 2024-01-01 是周一；东八区 05:00 对应 UTC 2023-12-31 21:00。
        let now = 1_704_056_400;
        let bounds = bucket_bounds(now, 8 * 3600);
        assert_eq!(bounds.day, 1_704_038_400, "本地 2024-01-01 00:00");
        assert_eq!(bounds.week, bounds.day, "周一即周起点");
        assert_eq!(bounds.month, bounds.day);
        assert_eq!(bounds.year, bounds.day);
    }

    #[test]
    fn sunday_belongs_to_the_previous_monday() {
        // 2024-01-07 是周日，东八区 12:00。
        let now = 1_704_600_000;
        let bounds = bucket_bounds(now, 8 * 3600);
        let (year, month, day) = civil_from_days((bounds.day + 8 * 3600) / SECONDS_PER_DAY);
        assert_eq!((year, month, day), (2024, 1, 7));
        assert_eq!(
            (bounds.day - bounds.week) / SECONDS_PER_DAY,
            6,
            "周日归上一个周一，相差 6 天"
        );
    }

    #[test]
    fn month_and_year_starts_are_the_first_day() {
        let now = 1_704_056_400; // 2024-01-01 05:00 +08:00
        let bounds = bucket_bounds(now, 8 * 3600);
        let month_local_days = (bounds.month + 8 * 3600) / SECONDS_PER_DAY;
        assert_eq!(civil_from_days(month_local_days), (2024, 1, 1));
        let year_local_days = (bounds.year + 8 * 3600) / SECONDS_PER_DAY;
        assert_eq!(civil_from_days(year_local_days), (2024, 1, 1));
    }

    #[test]
    fn negative_offsets_round_down_correctly() {
        // UTC-05:00 的本地 2023-12-31 20:30 对应 UTC 2024-01-01 01:30。
        let now = 1_704_072_600;
        let bounds = bucket_bounds(now, -5 * 3600);
        let local_days = (bounds.day - 5 * 3600) / SECONDS_PER_DAY;
        assert_eq!(civil_from_days(local_days), (2023, 12, 31));
    }

    #[test]
    fn timezone_label_formats_like_lua() {
        assert_eq!(timezone_label(8 * 3600), "UTC+08:00");
        assert_eq!(timezone_label(-5 * 3600 - 1800), "UTC-05:30");
        assert_eq!(timezone_label(0), "UTC+00:00");
    }

    #[test]
    fn datetime_matches_lua_os_date_format() {
        // 2024-01-01 05:30:07 +08:00
        let unix = 1_704_059_407;
        assert_eq!(format_datetime(unix, 8 * 3600), "2024/01/01 05:50:07");
        // 同一瞬间在 UTC 下
        assert_eq!(format_datetime(unix, 0), "2023/12/31 21:50:07");
        // 跨日边界
        assert_eq!(format_datetime(0, 0), "1970/01/01 00:00:00");
        assert_eq!(format_datetime(86_399, 0), "1970/01/01 23:59:59");
        assert_eq!(format_datetime(86_400, 0), "1970/01/02 00:00:00");
    }
}
