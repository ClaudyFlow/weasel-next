//! 生成样例报表，便于人工核对文案与排版（也可写入文档）。
//!
//! * 打印：`cargo run --release --example render_reports`
//! * 写文件：`cargo run --release --example render_reports -- docs/sample-reports.md`
//!
//! 样例数据是构造的（一天 1284 次上屏、3512 字），用于展示长款/短款、空桶、生字词本、
//! 皮肤清单、皮肤切换回显与持久化片段的真实输出。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use rime_input_stat_core::{
    format_new_words, format_report, format_skin_list, recognize, run_command, Bucket,
    CommandContext, DeviceType, Fastest, InputStats, NewWords, ReportInput, ReportKind, Skin, Style,
    DEFAULT_SKIN_INDEX, LOG_VERSION, SKINS,
};

/// 构造一份贴近真实的样例统计。
fn sample_bucket() -> Bucket {
    let mut bucket = Bucket::default();
    bucket.ts = 1_777_248_000; // 2026-04-27 08:00:00 +08:00（周一）
    bucket.count = 1_284;
    bucket.length = 3_512;
    bucket.key_touch_cnt = 11_806;
    bucket.total_code_len_without_space = 10_402;
    bucket.lengths = BTreeMap::from([
        (1_i64, 402_i64),
        (2_i64, 386_i64),
        (3_i64, 41_i64),
        (4_i64, 6_i64),
    ]);
    bucket.code_lengths = BTreeMap::from([
        (1_i64, 12_i64),
        (2_i64, 128_i64),
        (3_i64, 402_i64),
        (4_i64, 486_i64),
        (5_i64, 214_i64),
        (6_i64, 42_i64),
    ]);
    bucket.fastest = Fastest {
        spd: 168.4,
        key_touch_spd: 5.07,
        avg_code_len: 1.81,
    };
    bucket.avg_gaps = vec![412.0, 655.0, 138.0, 902.0, 61.0];
    bucket.avg_cnts = vec![1_186.0, 1_902.0, 402.0, 2_641.0, 176.0];
    bucket.avg_code_len = vec![3_520.0, 5_610.0, 1_180.0, 7_780.0, 520.0];
    bucket
}

fn report<'a>(
    bucket: &'a Bucket,
    kind: ReportKind,
    style: Style,
    skin: Skin,
    quote: Option<&'a str>,
) -> ReportInput<'a> {
    ReportInput {
        kind,
        bucket,
        style,
        device_type: DeviceType::Desktop,
        schema_name: "luna_pinyin_simp",
        software_name: "weasel",
        software_version: "0.17.4",
        avg_code_len_desc: "码长：单字码长 + 空格",
        quote,
        timestamp: "2026/05/03 21:14:07",
        timezone: "UTC+08:00",
        skin_word: skin,
        skin_code: skin,
    }
}

fn main() {
    let bucket = sample_bucket();
    let skin: Skin = SKINS[(DEFAULT_SKIN_INDEX - 1) as usize];
    let mut body = String::new();

    writeln!(body, "## 长款日报（`/rtjj`，皮肤 21，带名言）\n").unwrap();
    writeln!(body, "```text").unwrap();
    writeln!(
        body,
        "{}",
        format_report(&report(
            &bucket,
            ReportKind::Daily,
            Style::Long,
            skin,
            Some("键盘上的每一次迟疑，都是思路在拐弯。")
        ))
    )
    .unwrap();
    writeln!(body, "```\n").unwrap();

    writeln!(body, "## 短款周报（`/ztj`，皮肤 21）\n").unwrap();
    writeln!(body, "```text").unwrap();
    writeln!(
        body,
        "{}",
        format_report(&report(&bucket, ReportKind::Weekly, Style::Short, skin, None))
    )
    .unwrap();
    writeln!(body, "```\n").unwrap();

    writeln!(body, "## 空桶提示（本月无记录，走命令层）\n").unwrap();
    let mut empty_stats = InputStats::default();
    empty_stats.device_type = "💻".into();
    let context = CommandContext {
        now_unix: 1_777_824_847, // 2026-05-03 21:14:07 +08:00
        tz_offset_seconds: 8 * 3600,
        schema_name: "luna_pinyin_simp",
        software_name: "weasel",
        software_version: "0.17.4",
        device_type: DeviceType::Desktop,
        style: Style::Short,
        avg_code_len_desc: "",
        quote: None,
    };
    let outcome = run_command(
        &mut empty_stats,
        recognize("//3").expect("//3 应被识别为月统计"),
        &context,
    );
    writeln!(body, "```text").unwrap();
    writeln!(body, "{}", outcome.text).unwrap();
    writeln!(body, "```\n").unwrap();

    writeln!(body, "## 生字/词本（`/sztj`，按字典序）\n").unwrap();
    let words: BTreeSet<String> = ["龘", "饕餮", "腼腆", "迤逦", "踟蹰", "氤氲", "葳蕤"]
        .iter()
        .map(|word| (*word).to_owned())
        .collect();
    writeln!(body, "```text").unwrap();
    writeln!(body, "{}", format_new_words(&words, 1_777_248_000, 8 * 3600)).unwrap();
    writeln!(body, "```\n").unwrap();

    writeln!(body, "## 皮肤清单（`/pf`，字词皮肤 21、码长皮肤 3）\n").unwrap();
    writeln!(body, "```text").unwrap();
    writeln!(body, "{}", format_skin_list(21, 3)).unwrap();
    writeln!(body, "```\n").unwrap();

    writeln!(body, "## 皮肤切换回显（`//6011`）\n").unwrap();
    let mut switch_stats = InputStats::default();
    let switch = run_command(
        &mut switch_stats,
        recognize("//6011").expect("//6011 应被识别为数字皮肤切换"),
        &context,
    );
    writeln!(body, "```text").unwrap();
    writeln!(body, "{}", switch.text).unwrap();
    writeln!(body, "```\n").unwrap();

    let mut stats = InputStats::default();
    stats.log_ver = LOG_VERSION;
    stats.daily = sample_bucket();
    stats.daily_max = 6;
    stats.new_words = NewWords {
        start_time: 1_777_248_000,
        words,
    };
    stats.device_type = "💻".into();
    stats.skin_idx_word = 21;
    stats.skin_idx_code = 3;

    writeln!(body, "## 持久化片段（`input_stat_<schema>.lua`，节选）\n").unwrap();
    writeln!(body, "```lua").unwrap();
    let text = rime_input_stat_core::serialize(&stats);
    for line in text.lines().take(30) {
        writeln!(body, "{line}").unwrap();
    }
    writeln!(body, "\t\t… （其余桶与字段略）").unwrap();
    writeln!(body, "```").unwrap();

    match std::env::args().nth(1) {
        Some(path) => {
            let header = "# input_stat 样例报表\n\n\
                > 由 `cargo run --release --example render_reports -- docs/sample-reports.md` 生成。\n\
                > 皮肤 21 = `★☆`；真实候选中换行会转成 CRLF。\n\n";
            std::fs::write(&path, format!("{header}{body}")).expect("写入样例文档失败");
            println!("已写入 {path}");
        }
        None => print!("{body}"),
    }
}
