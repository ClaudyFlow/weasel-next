//! 命令识别与副作用（规范 §5）。
//!
//! Rime 侧把以 `/` 开头的输入交给本模块：先[`recognize`]识别命令，再由[`run_command`]
//! 执行副作用并给出回显文本。回显文案逐字照抄 `docs/lua-parity-spec.md` 附录 A。
//!
//! 尚未实现的部分（报表排版 `//1`…`/ntjj`、生字词本 `//5`、皮肤清单 `//60`）会返回
//! [`CommandOutcome::pending`]：命令被识别、无副作用、文本为空，由宿主决定是否产出候选。
//! 这些命令的文本将在报表模块落地后填入。

use crate::report::{self, DeviceType, ReportInput, Style};
use crate::state::{Bucket, Fastest, InputStats, NewWords, DEFAULT_SKIN_INDEX};
use crate::time::{format_datetime, timezone_label};

/// 报表类命令渲染所需的宿主上下文。
#[derive(Clone, Copy, Debug)]
pub struct CommandContext<'a> {
    /// 当前时间（Unix 秒），用于报表时间戳。
    pub now_unix: i64,
    /// 本地时区偏移（秒）。
    pub tz_offset_seconds: i32,
    /// 方案名（长款槽 17 / 短款槽 17）。
    pub schema_name: &'a str,
    /// 发行版名（长款槽 18）。
    pub software_name: &'a str,
    /// 发行版版本（长款槽 18）。
    pub software_version: &'a str,
    /// 设备类型；`Unknown` 时报表命令拒绝执行（§6.8）。
    pub device_type: DeviceType,
    /// 报表风格（由命令串末两位是否相同决定，§5）。
    pub style: Style,
    /// 平均码长说明（长款追加到槽 6）。
    pub avg_code_len_desc: &'a str,
    /// 已按 Lua 语义随机选好的名言；短款忽略。
    pub quote: Option<&'a str>,
}

/// 皮肤总数（Lua `#skinList`）。
pub const SKIN_COUNT: i64 = 23;

/// 皮肤作用对象。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkinKind {
    /// 字词统计条（Lua `progressBarSkinIdx_word`）。
    Word,
    /// 码长统计条（Lua `progressBarSkinIdx_code`）。
    Code,
}

/// 报表周期。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportPeriod {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// 已识别的命令。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// 日/周/月/年报表（§5.1）。
    Report(ReportPeriod),
    /// 生字/词本（§5.2）。
    NewWordsReport,
    /// `//07` / `/qcjs`：清除极速（§5.3）。
    ClearFastest,
    /// `//08` / `/qcsz`：清除生字/词（§5.4）。
    ClearNewWords,
    /// `//09` / `/qctj`：清除所有统计（§5.5）。
    ClearAll,
    /// `//60` / `/pf`：皮肤清单（§5.6）。
    SkinList,
    /// `//6NN1` / `//6NN2` / `/[hp]fw<xy>` / `/[hp]fc<xy>`：切换皮肤（§5.7、§5.8）。
    SetSkin {
        kind: SkinKind,
        index: i64,
        /// 数字形式（`//6NN1`）还是字母形式（`/[hp]fw<xy>`），决定回显模板。
        numeric: bool,
        /// 回显里原样使用的编号：数字形式为两位十进制数字，字母形式为两个小写字母。
        label: [char; 2],
    },
    /// 进入了命令分支但没有匹配任何具体命令：Lua 版此时 `summary` 为空，不产出候选。
    Silent,
}

/// 命令执行结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutcome {
    /// 回显文本；空串表示"命令已识别但暂无输出"。
    pub text: String,
    /// 是否需要宿主立即落盘。
    pub save: bool,
}

impl CommandOutcome {
    /// 已识别但本期尚未实现文本输出的命令。
    pub fn pending() -> Self {
        Self {
            text: String::new(),
            save: false,
        }
    }
}

/// 识别一条命令输入（输入必须以 `/` 开头）。未识别时返回 `None`。
pub fn recognize(input: &str) -> Option<Command> {
    // 报表命令（Lua `reportCommands`，§5.0）
    let period = match input {
        "//1" | "//11" | "/rtj" | "/rtjj" => Some(ReportPeriod::Daily),
        "//2" | "//22" | "/ztj" | "/ztjj" => Some(ReportPeriod::Weekly),
        "//3" | "//33" | "/ytj" | "/ytjj" => Some(ReportPeriod::Monthly),
        "//4" | "//44" | "/ntj" | "/ntjj" => Some(ReportPeriod::Yearly),
        _ => None,
    };
    if let Some(period) = period {
        return Some(Command::Report(period));
    }

    match input {
        "//5" | "/sztj" => return Some(Command::NewWordsReport),
        "//07" | "/qcjs" => return Some(Command::ClearFastest),
        "//08" | "/qcsz" => return Some(Command::ClearNewWords),
        "//09" | "/qctj" => return Some(Command::ClearAll),
        _ => {}
    }

    // 皮肤分支（Lua L1113）：`sub(1,3) ∈ {"//6","/pf"}` 或 `sub(1,4) ∈ {"/pfw","/pfc","/hfw","/hfc"}`。
    // 进入后按顺序匹配：清单、数字字词、数字码长、字母字词、字母码长；都不匹配时静默。
    if input.len() >= 3 {
        let head3: String = input.chars().take(3).collect();
        let head4: String = input.chars().take(4).collect();
        let enters = matches!(head3.as_str(), "//6" | "/pf")
            || matches!(head4.as_str(), "/pfw" | "/pfc" | "/hfw" | "/hfc");
        if enters {
            if input == "//60" || input == "/pf" {
                return Some(Command::SkinList);
            }
            if let Some(command) = parse_numeric_skin(input) {
                return Some(command);
            }
            if let Some(command) = parse_letter_skin(input) {
                return Some(command);
            }
            return Some(Command::Silent);
        }
    }
    None
}

/// `^//6(\d\d)1$` / `^//6(\d\d)2$`（§5.7）。
fn parse_numeric_skin(input: &str) -> Option<Command> {
    let rest = input.strip_prefix("//6")?;
    let digits: String = rest.chars().take(2).collect();
    if digits.len() != 2 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let kind = match rest.chars().nth(2) {
        Some('1') if rest.chars().count() == 3 => SkinKind::Word,
        Some('2') if rest.chars().count() == 3 => SkinKind::Code,
        _ => return None,
    };
    let index = digits.parse::<i64>().ok()?;
    let mut chars = digits.chars();
    Some(Command::SetSkin {
        kind,
        index,
        numeric: true,
        label: [chars.next()?, chars.next()?],
    })
}

/// `^/[hp]fw[a-z][a-z]$` / `^/[hp]fc[a-z][a-z]$`（§5.8）。
fn parse_letter_skin(input: &str) -> Option<Command> {
    let bytes = input.as_bytes();
    if bytes.len() != 6 {
        return None;
    }
    if bytes[0] != b'/' || !matches!(bytes[1], b'h' | b'p') {
        return None;
    }
    // 模式顺序是 `/` `[hp]` `f` `w|c` `<xy>`
    if bytes[2] != b'f' {
        return None;
    }
    let kind = match bytes[3] {
        b'w' => SkinKind::Word,
        b'c' => SkinKind::Code,
        _ => return None,
    };
    if !bytes[4].is_ascii_lowercase() || !bytes[5].is_ascii_lowercase() {
        return None;
    }
    // 索引公式：(第一字母 - 97) * 10 + (第二字母 - 97)
    let index = (bytes[4] as i64 - 97) * 10 + (bytes[5] as i64 - 97);
    Some(Command::SetSkin {
        kind,
        index,
        numeric: false,
        // 回显原样使用输入里的两个小写字母（Lua `input:sub(5,6)`）。
        label: [bytes[4] as char, bytes[5] as char],
    })
}

/// 执行已识别的命令，返回回显文本与是否需要落盘（§5.3–§5.8、§6）。
pub fn run_command(
    stats: &mut InputStats,
    command: Command,
    context: &CommandContext<'_>,
) -> CommandOutcome {
    match command {
        Command::Report(kind) => {
            if context.device_type == DeviceType::Unknown {
                return CommandOutcome {
                    text: "deviceType识别失败，请设置deviceType后重新部署".into(),
                    save: false,
                };
            }
            let (report_kind, bucket) = match kind {
                ReportPeriod::Daily => (report::ReportKind::Daily, &stats.daily),
                ReportPeriod::Weekly => (report::ReportKind::Weekly, &stats.weekly),
                ReportPeriod::Monthly => (report::ReportKind::Monthly, &stats.monthly),
                ReportPeriod::Yearly => (report::ReportKind::Yearly, &stats.yearly),
            };
            let text = if bucket.count == 0 {
                report_kind.empty_message().to_owned()
            } else {
                report::format_report(&ReportInput {
                    kind: report_kind,
                    bucket,
                    style: context.style,
                    device_type: context.device_type,
                    schema_name: context.schema_name,
                    software_name: context.software_name,
                    software_version: context.software_version,
                    avg_code_len_desc: context.avg_code_len_desc,
                    quote: context.quote,
                    timestamp: &format_datetime(context.now_unix, context.tz_offset_seconds),
                    timezone: &timezone_label(context.tz_offset_seconds),
                    skin_word: report::skin_at(stats.skin_idx_word),
                    skin_code: report::skin_at(stats.skin_idx_code),
                })
            };
            CommandOutcome { text, save: false }
        }
        Command::NewWordsReport => CommandOutcome {
            text: report::format_new_words(
                &stats.new_words.words,
                stats.new_words.start_time,
                context.tz_offset_seconds,
            ),
            save: false,
        },
        Command::SkinList => CommandOutcome {
            text: report::format_skin_list(stats.skin_idx_word, stats.skin_idx_code),
            save: false,
        },
        Command::ClearFastest => {
            for bucket in stats.buckets_mut() {
                bucket.fastest = Fastest::default();
            }
            CommandOutcome {
                text: "※ 极速数据已清空。".into(),
                save: true,
            }
        }
        Command::ClearNewWords => {
            stats.new_words = NewWords::default();
            CommandOutcome {
                text: "※ 生字词已清空。".into(),
                save: true,
            }
        }
        Command::ClearAll => {
            // 保留 logVer、deviceType 与两个皮肤索引（§5.5 第 1/5 步）。
            let log_ver = stats.log_ver;
            let skin_word = stats.skin_idx_word;
            let skin_code = stats.skin_idx_code;
            let device_type = std::mem::take(&mut stats.device_type);
            *stats = InputStats {
                daily: Bucket::default(),
                weekly: Bucket::default(),
                monthly: Bucket::default(),
                yearly: Bucket::default(),
                log_ver,
                daily_max: 0,
                new_words: NewWords::default(),
                device_type,
                skin_idx_word: skin_word,
                skin_idx_code: skin_code,
            };
            CommandOutcome {
                text: "※ 所有统计数据已清空。".into(),
                save: true,
            }
        }
        Command::SetSkin {
            kind,
            index,
            numeric,
            label,
        } => {
            if !(1..=SKIN_COUNT).contains(&index) {
                return CommandOutcome {
                    text: format!("※ 无效的皮肤编号〔{}{}〕", label[0], label[1]),
                    save: false,
                };
            }
            match kind {
                SkinKind::Word => {
                    stats.skin_idx_word = index;
                    CommandOutcome {
                        text: if numeric {
                            format!(
                                "※ 字词统计皮肤已切换至：//6{index:02}1 {}{}",
                                skin_field(index),
                                skin_empty(index)
                            )
                        } else {
                            format!(
                                "※ 字词统计皮肤已切换至：/[ph]fw{}{} {}{}",
                                label[0],
                                label[1],
                                skin_field(index),
                                skin_empty(index)
                            )
                        },
                        save: true,
                    }
                }
                SkinKind::Code => {
                    stats.skin_idx_code = index;
                    CommandOutcome {
                        text: if numeric {
                            format!(
                                "※ 码长皮肤已切换至：//6{index:02}2 {}{}",
                                skin_field(index),
                                skin_empty(index)
                            )
                        } else {
                            format!(
                                "※ 码长皮肤已切换至：/[ph]fc{}{} {}{}",
                                label[0],
                                label[1],
                                skin_field(index),
                                skin_empty(index)
                            )
                        },
                        save: true,
                    }
                }
            }
        }
        // 进入命令分支但未匹配具体命令：Lua 版不产出候选。
        Command::Silent => CommandOutcome::pending(),
    }
}

/// 取皮肤实心字符（1-based 索引，越界回退默认皮肤）。
pub fn skin_field(index: i64) -> char {
    crate::report::SKINS
        .get((index.max(1) - 1) as usize)
        .map(|skin| skin.field)
        .unwrap_or_else(|| crate::report::SKINS[(DEFAULT_SKIN_INDEX - 1) as usize].field)
}

/// 取皮肤空心字符。
pub fn skin_empty(index: i64) -> char {
    crate::report::SKINS
        .get((index.max(1) - 1) as usize)
        .map(|skin| skin.empty)
        .unwrap_or_else(|| crate::report::SKINS[(DEFAULT_SKIN_INDEX - 1) as usize].empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用上下文：桌面、短款、固定时间。
    fn ctx() -> CommandContext<'static> {
        CommandContext {
            now_unix: 1_704_056_400,
            tz_offset_seconds: 8 * 3600,
            schema_name: "luna_pinyin",
            software_name: "Weasel",
            software_version: "1.0",
            device_type: DeviceType::Desktop,
            style: Style::Short,
            avg_code_len_desc: "",
            quote: None,
        }
    }

    #[test]
    fn recognizes_report_commands() {
        for (input, period) in [
            ("//1", ReportPeriod::Daily),
            ("//11", ReportPeriod::Daily),
            ("/rtj", ReportPeriod::Daily),
            ("/rtjj", ReportPeriod::Daily),
            ("//2", ReportPeriod::Weekly),
            ("/ztjj", ReportPeriod::Weekly),
            ("//3", ReportPeriod::Monthly),
            ("/ytj", ReportPeriod::Monthly),
            ("//4", ReportPeriod::Yearly),
            ("/ntjj", ReportPeriod::Yearly),
        ] {
            assert_eq!(recognize(input), Some(Command::Report(period)), "{input}");
        }
        assert_eq!(recognize("//5"), Some(Command::NewWordsReport));
        assert_eq!(recognize("/sztj"), Some(Command::NewWordsReport));
    }

    #[test]
    fn recognizes_clear_commands() {
        assert_eq!(recognize("//07"), Some(Command::ClearFastest));
        assert_eq!(recognize("/qcjs"), Some(Command::ClearFastest));
        assert_eq!(recognize("//08"), Some(Command::ClearNewWords));
        assert_eq!(recognize("/qcsz"), Some(Command::ClearNewWords));
        assert_eq!(recognize("//09"), Some(Command::ClearAll));
        assert_eq!(recognize("/qctj"), Some(Command::ClearAll));
    }

    #[test]
    fn recognizes_skin_commands() {
        assert_eq!(recognize("//60"), Some(Command::SkinList));
        assert_eq!(recognize("/pf"), Some(Command::SkinList));
        assert!(matches!(
            recognize("//6011"),
            Some(Command::SetSkin {
                kind: SkinKind::Word,
                index: 1,
                numeric: true,
                ..
            })
        ));
        assert!(matches!(
            recognize("//6232"),
            Some(Command::SetSkin {
                kind: SkinKind::Code,
                index: 23,
                numeric: true,
                ..
            })
        ));
        // `/pfwab` 与 `/pf` 命中同一个 Lua 分支，但走字母形式（源码 L1113 起的内层 if/elseif）
        assert!(matches!(
            recognize("/pfwab"),
            Some(Command::SetSkin {
                kind: SkinKind::Word,
                index: 1,
                numeric: false,
                ..
            })
        ));
        assert!(matches!(
            recognize("/hfcbn"),
            Some(Command::SetSkin {
                kind: SkinKind::Code,
                index: 23,
                numeric: false,
                ..
            })
        ));
        // 进入分支但都不匹配 → 静默
        assert_eq!(recognize("//6aa"), Some(Command::Silent));
        assert_eq!(recognize("/pfzz"), Some(Command::Silent));
        // `aa` 索引 0：命令形式合法但编号非法
        assert!(matches!(
            recognize("/pfwaa"),
            Some(Command::SetSkin { index: 0, .. })
        ));
    }

    #[test]
    fn unknown_input_is_not_a_command() {
        assert_eq!(recognize("nihao"), None);
        assert_eq!(recognize("/xyz"), None);
        assert_eq!(recognize("//999"), None);
        assert_eq!(recognize("//5x"), None);
    }

    #[test]
    fn clear_fastest_only_touches_fastest() {
        let mut stats = InputStats::default();
        stats.daily.fastest.spd = 99.0;
        stats.daily.avg_gaps = vec![3.0, 4.0];
        stats.daily.count = 5;
        let outcome = run_command(&mut stats, Command::ClearFastest, &ctx());
        assert_eq!(outcome.text, "※ 极速数据已清空。");
        assert!(outcome.save);
        assert_eq!(stats.daily.fastest, Fastest::default());
        assert_eq!(stats.daily.count, 5, "其它累计不动");
        assert_eq!(stats.daily.avg_gaps, vec![3.0, 4.0], "avgGaps 不动");
    }

    #[test]
    fn clear_all_keeps_skins_device_and_version() {
        let mut stats = InputStats::default();
        stats.log_ver = 4;
        stats.device_type = "💻".into();
        stats.skin_idx_word = 3;
        stats.skin_idx_code = 9;
        stats.daily.count = 7;
        stats.new_words.words.insert("你好".into());
        let outcome = run_command(&mut stats, Command::ClearAll, &ctx());
        assert_eq!(outcome.text, "※ 所有统计数据已清空。");
        assert!(outcome.save);
        assert_eq!(stats.daily.count, 0);
        assert!(stats.new_words.words.is_empty());
        assert_eq!(stats.log_ver, 4, "logVer 保留");
        assert_eq!(stats.device_type, "💻", "deviceType 保留");
        assert_eq!(stats.skin_idx_word, 3, "皮肤索引保留");
        assert_eq!(stats.skin_idx_code, 9);
    }

    #[test]
    fn numeric_skin_switch_reports_exact_text() {
        let mut stats = InputStats::default();
        let outcome = run_command(
            &mut stats,
            recognize("//6011").expect("识别 //6011"),
            &ctx(),
        );
        assert_eq!(stats.skin_idx_word, 1);
        assert!(outcome.save);
        assert_eq!(outcome.text, "※ 字词统计皮肤已切换至：//6011 ▉▁");

        let outcome = run_command(&mut stats, recognize("//6232").expect("识别 //6232"), &ctx());
        assert_eq!(stats.skin_idx_code, 23);
        assert_eq!(outcome.text, "※ 码长皮肤已切换至：//6232 █░");
    }

    #[test]
    fn letter_skin_switch_reports_exact_text_and_rejects_aa() {
        let mut stats = InputStats::default();
        let outcome = run_command(&mut stats, recognize("/pfwab").expect("识别 /pfwab"), &ctx());
        assert_eq!(stats.skin_idx_word, 1);
        assert_eq!(outcome.text, "※ 字词统计皮肤已切换至：/[ph]fwab ▉▁");

        // aa → 索引 0，被判为非法；回显里的编号是原样两个字母
        let command = Command::SetSkin {
            kind: SkinKind::Word,
            index: 0,
            numeric: false,
            label: ['A', 'A'],
        };
        let outcome = run_command(&mut stats, command, &ctx());
        assert!(!outcome.save);
        assert_eq!(outcome.text, "※ 无效的皮肤编号〔AA〕");
    }

    #[test]
    fn out_of_range_skin_index_is_rejected() {
        let mut stats = InputStats::default();
        let command = Command::SetSkin {
            kind: SkinKind::Code,
            index: 24,
            numeric: true,
            label: ['2', '4'],
        };
        let outcome = run_command(&mut stats, command, &ctx());
        assert_eq!(outcome.text, "※ 无效的皮肤编号〔24〕");
        assert!(!outcome.save);
        assert_eq!(stats.skin_idx_code, DEFAULT_SKIN_INDEX, "状态未变");
    }
}
