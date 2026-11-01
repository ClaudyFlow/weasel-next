//! 生涯段位 / 境界 / 名著字数对照——**框架无关**，Slint 版与 D2D 外壳共用这一份。
//!
//! 移植自 lua 插件 `input_statistics` 的 `career_levels` / `career_realms`，
//! 两张表必须与插件完全一致，否则同一个用户在两处看到的段位会不一样
//! （原实现曾写在 `Setting/src/main.rs`，2026-10-31 抽到这里）。

/// 100 级段位曲线：`req(n) = 50(n-1) + 50(n-1)²`。
///
/// 前 5 级正好是 100 / 300 / 600 / 1000（与旧手写表一致），100 级约 49.5 万经验
/// （≈ 上屏 49.5 万字）。旧表用 `r*2+100` 会指数爆炸（20 级就到 5200 万）。
pub const CAREER_MAX_LEVEL: u32 = 100;

/// 升到 `level` 级所需的累计经验（1 级为 0）。
pub fn career_level_req(level: u32) -> u64 {
    if level <= 1 {
        return 0;
    }
    let n = u64::from(level - 1);
    50 * n + 50 * n * n
}

/// 每 10 级一个段位称号（等级越高，称号越"仙"）。
pub const CAREER_TITLES: &[&str] = &[
    "初识键盘",
    "摸键学徒",
    "码字新人",
    "指尖练习生",
    "输入学徒",
    "码流行者",
    "飞键使",
    "连击者",
    "码阵构筑师",
    "熟练输入师",
];

pub fn career_title(level: u32) -> &'static str {
    let index = ((level.saturating_sub(1)) / 10) as usize;
    CAREER_TITLES[index.min(CAREER_TITLES.len() - 1)]
}

/// 7 个境界（按累计字数），**降序**排列，取第一个满足的。
pub const CAREER_REALMS: &[(u64, &str)] = &[
    (10_000_000, "码仙境"),
    (5_000_000, "码圣境"),
    (1_000_000, "通玄境"),
    (200_000, "御码境"),
    (50_000, "飞键境"),
    (10_000, "熟键境"),
    (0, "凡键境"),
];

/// 按累计经验算段位。返回 (称号, 等级, 本级已得经验, 本级所需经验)。
pub fn career_level(total_exp: u64) -> (&'static str, u32, u64, u64) {
    let mut lo = 1u32;
    let mut hi = CAREER_MAX_LEVEL;
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if career_level_req(mid) <= total_exp {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let level = lo;
    let start = career_level_req(level);
    let end = career_level_req((level + 1).min(CAREER_MAX_LEVEL + 1));
    let span = if end > start { end - start } else { 1 };
    (career_title(level), level, total_exp.saturating_sub(start), span)
}

/// 按累计字数算境界。
pub fn career_realm(total_chars: u64) -> &'static str {
    for (base, name) in CAREER_REALMS {
        if total_chars >= *base {
            return name;
        }
    }
    "凡键境"
}

/// 名著篇幅对照表（通行版本约数，升序）。
pub const BOOK_LEVELS: &[(u64, &str)] = &[
    (5_000, "《道德经》"),
    (6_000, "《孙子兵法》"),
    (16_000, "《论语》"),
    (35_000, "《孟子》"),
    (80_000, "《庄子》"),
    (196_000, "《左传》"),
    (360_000, "《三国志》"),
    (526_000, "《史记》"),
    (640_000, "《三国演义》"),
    (731_000, "《红楼梦》"),
    (820_000, "《西游记》"),
    (960_000, "《水浒传》"),
    (3_000_000, "《资治通鉴》"),
    (40_000_000, "《二十四史》"),
];

/// 最多列几个"下一个里程碑"。
pub const BOOK_UPCOMING_SHOWN: usize = 3;

/// 累计字数还没超过的下一批名著（升序，最多 [`BOOK_UPCOMING_SHOWN`] 部）。
pub fn upcoming_books(total_chars: u64) -> Vec<(u64, &'static str)> {
    BOOK_LEVELS
        .iter()
        .filter(|(base, _)| *base > total_chars)
        .take(BOOK_UPCOMING_SHOWN)
        .map(|(base, name)| (*base, *name))
        .collect()
}

/// 字数格式化：满一万写成 `X.X 万字`，否则 `X 字`。
pub fn format_chars(n: u64) -> String {
    if n >= 10_000 {
        format!("{:.1} 万字", n as f64 / 10_000.0)
    } else {
        format!("{n} 字")
    }
}

/// 名著字数水平：`已写 6.0 万字；还差 2.0 万字到《庄子》、13.6 万字到《左传》…`
pub fn book_level(total_chars: u64) -> String {
    let upcoming = upcoming_books(total_chars);
    let mut text = format!("已写 {}", format_chars(total_chars));
    if upcoming.is_empty() {
        text.push_str("，已经超过《二十四史》的篇幅");
        return text;
    }
    for (index, (base, name)) in upcoming.iter().enumerate() {
        text.push_str(if index == 0 { "；还差 " } else { "、" });
        text.push_str(&format!("{}到{}", format_chars(base - total_chars), name));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_curve_matches_plugin() {
        assert_eq!(career_level_req(1), 0);
        assert_eq!(career_level_req(2), 100);
        assert_eq!(career_level_req(3), 300);
        assert_eq!(career_level_req(4), 600);
        assert_eq!(career_level_req(5), 1000);
        let (title, level, cur, span) = career_level(150);
        assert_eq!(level, 2);
        assert_eq!(title, "初识键盘");
        assert_eq!(cur, 50);
        assert_eq!(span, 200);
    }

    #[test]
    fn realm_and_book_text() {
        assert_eq!(career_realm(0), "凡键境");
        assert_eq!(career_realm(12_000), "熟键境");
        assert_eq!(career_realm(1_000_000), "通玄境");
        let text = book_level(60_000);
        assert!(text.contains("已写 6.0 万字"), "{text}");
        assert!(text.contains("《庄子》"), "{text}");
    }
}
