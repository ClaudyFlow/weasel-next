//! 报表排版（规范 §6）。
//!
//! 逐字符对齐 Lua 版：槽位模型 1..21、短款/长款差异、`progressBar` 阈值阶梯、居中分隔线、
//! 最终拼装（折叠连续换行 → 去首尾换行 → trim）。已登记的**有意偏离**：
//! * 每次渲染从空槽位表开始（Lua 的 `strTable` 跨命令残留会污染输出，规范 §6.9 建议如此）；
//! * 码长并列时按“次数降序 + 码长升序”确定性排序（Lua `table.sort` 不稳定，规范 §6.5）；
//! * 生字词的 `verStr` 固定为脚本行（Lua 依赖 `strTable[#strTable-1]` 隐藏状态，规范 §6.4）。

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::state::{char_length_ratio, Bucket, CharLengthRatio, Fastest};
use crate::time::format_datetime;

/// 分隔线长度（Lua `splitorLen`）。
pub const SPLITOR_LEN: usize = 14;
/// 分隔线字符（Lua `splitorChar`）。
pub const SPLITOR_CHAR: char = '─';
/// 脚本版本（Lua `SCRIPT_VERSION`）。
pub const SCRIPT_VERSION: &str = "₂₀₂₆1010・A";
/// 脚本行前缀（Lua 长款槽 20：`'脚本：🐐🪶' .. SCRIPT_VERSION`）。
pub const SCRIPT_LINE_PREFIX: &str = "脚本：🐐🪶❄️";

/// 分隔线：14 个 `─`。
pub fn splitor() -> String {
    std::iter::repeat_n(SPLITOR_CHAR, SPLITOR_LEN).collect()
}

/// 13 个 `─` 的分隔线，前置一个 emoji（长款槽 3/7/11）。
pub fn emoji_splitor(emoji: &str) -> String {
    let mut text = String::from(emoji);
    text.extend(std::iter::repeat_n(SPLITOR_CHAR, SPLITOR_LEN - 1));
    text
}

/// 把文字居中嵌入 14 列分隔线（Lua `embedTextIntoSplitor`）。
///
/// 仅当 `14 > 码点数 + 1` 时补线，否则原样返回。
pub fn embed_text_into_splitor(text: &str) -> String {
    let len = text.chars().count();
    if SPLITOR_LEN <= len + 1 {
        return text.to_owned();
    }
    let half = ((SPLITOR_LEN - len) as f64 * 0.5).floor() as usize;
    let mut out = String::new();
    out.extend(std::iter::repeat_n(SPLITOR_CHAR, half));
    out.push_str(text);
    out.extend(std::iter::repeat_n(SPLITOR_CHAR, SPLITOR_LEN - len - half));
    out
}

/// 进度条皮肤：实心/空心字符对（Lua `skinList`，23 项，顺序必须一致）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Skin {
    pub field: char,
    pub empty: char,
}

/// Lua 版内置的 23 套皮肤。
pub const SKINS: [Skin; 23] = [
    Skin { field: '▉', empty: '▁' },
    Skin { field: '━', empty: '┄' },
    Skin { field: '●', empty: '○' },
    Skin { field: '■', empty: '□' },
    Skin { field: '▲', empty: '△' },
    Skin { field: '◆', empty: '◇' },
    Skin { field: '▶', empty: '▷' },
    Skin { field: '◀', empty: '◁' },
    Skin { field: '▼', empty: '▽' },
    Skin { field: '▶', empty: '▁' },
    Skin { field: '▉', empty: '┄' },
    Skin { field: '━', empty: '▁' },
    Skin { field: '●', empty: '▁' },
    Skin { field: '■', empty: '┄' },
    Skin { field: '▲', empty: '▁' },
    Skin { field: '◆', empty: '┄' },
    Skin { field: '▉', empty: '○' },
    Skin { field: '━', empty: '□' },
    Skin { field: '●', empty: '△' },
    Skin { field: '■', empty: '◇' },
    Skin { field: '★', empty: '☆' },
    Skin { field: '✭', empty: '✩' },
    Skin { field: '█', empty: '░' },
];

impl Skin {
    /// 10 格进度条：`filled` 个实心 + 其余空心。
    fn bar(self, filled: usize) -> String {
        let mut text = String::with_capacity(10);
        for index in 0..10 {
            text.push(if index < filled { self.field } else { self.empty });
        }
        text
    }
}

/// 按百分比取进度条（规范 §8 的阈值阶梯，**从高到低、`>=` 判定**）。
pub fn progress_bar(percent: f64, skin: Skin) -> String {
    let filled = if percent >= 95.0 {
        10
    } else if percent >= 85.0 {
        9
    } else if percent >= 75.0 {
        8
    } else if percent >= 65.0 {
        7
    } else if percent >= 55.0 {
        6
    } else if percent >= 45.0 {
        5
    } else if percent >= 35.0 {
        4
    } else if percent >= 25.0 {
        3
    } else if percent >= 15.0 {
        2
    } else if percent >= 5.0 {
        1
    } else {
        0
    };
    skin.bar(filled)
}

/// 按索引取皮肤（1-based；越界回退到默认皮肤 21）。
pub fn skin_at(index: i64) -> Skin {
    let clamped = index.clamp(1, SKINS.len() as i64) as usize;
    SKINS[clamped - 1]
}

/// 报表周期。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportKind {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl ReportKind {
    /// 标题里的统计类型（Lua `stat_type`）。
    pub fn stat_type(self) -> &'static str {
        match self {
            Self::Daily => "日统计",
            Self::Weekly => "周统计",
            Self::Monthly => "月统计",
            Self::Yearly => "年统计",
        }
    }

    /// 空桶提示（注意日维度用“今天”）。
    pub fn empty_message(self) -> &'static str {
        match self {
            Self::Daily => "※ 今天没有任何记录。",
            Self::Weekly => "※ 本周没有任何记录。",
            Self::Monthly => "※ 本月没有任何记录。",
            Self::Yearly => "※ 本年没有任何记录。",
        }
    }
}

/// 报表风格（Lua `reportStyle`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Short,
    Long,
}

/// 设备类型（Lua `deviceType`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceType {
    Phone,
    Desktop,
    /// 空串 = 识别失败，报表命令会拒绝执行。
    Unknown,
}

impl DeviceType {
    /// 从缓存的字符串解析（只接受 `📱` 与 `💻`）。
    pub fn parse(text: &str) -> Self {
        match text {
            "📱" => Self::Phone,
            "💻" => Self::Desktop,
            _ => Self::Unknown,
        }
    }

    /// emoji 文本（未知时为空串）。
    pub fn emoji(self) -> &'static str {
        match self {
            Self::Phone => "📱",
            Self::Desktop => "💻",
            Self::Unknown => "",
        }
    }
}

/// 码长分布的一项。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CodeItem {
    pub code_len: i64,
    pub ratio: f64,
}

/// `statisticsCal` 的结果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Statistics {
    /// 报表口径的“极速”：`max(历史最快窗口, 本期均速)`。
    pub fastest: Fastest,
    /// 本期均速（字/分）；时长不足 1 秒时为 0。
    pub avg_spd: f64,
    /// 报表平均码长 = 总码长 / **总字数**。
    pub avg_code_len: f64,
    /// 字长占比（按字数加权）。
    pub ratio: CharLengthRatio,
    /// 码长占比前 3 项 + 其它（第 4 项 `code_len` 恒为 0）。
    pub code_first_n: [CodeItem; 4],
}

/// 计算报表所需的全部数值（规范 §6.5）。
pub fn statistics_cal(bucket: &Bucket) -> Statistics {
    let mut fastest = bucket.fastest;
    let ratio = char_length_ratio(&bucket.lengths);

    let total_chars: i64 = bucket.lengths.iter().map(|(j, n)| j * n).sum();
    let total_code_len: i64 = bucket.code_lengths.iter().map(|(k, n)| k * n).sum();
    let total_code_cnt: i64 = bucket.code_lengths.values().sum();
    let avg_code_len = if total_chars == 0 {
        0.0
    } else {
        total_code_len as f64 / total_chars as f64
    };

    // 按出现次数降序、次数相同时码长升序（确定性；Lua 的 table.sort 不保证稳定）。
    let mut kinds: Vec<(i64, i64)> = bucket
        .code_lengths
        .iter()
        .map(|(code_len, count)| (*code_len, *count))
        .collect();
    kinds.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    let denominator = if total_code_cnt == 0 { 1 } else { total_code_cnt };
    let mut code_first_n = [CodeItem::default(); 4];
    let mut ratio_sum = 0.0;
    for index in 0..3 {
        if let Some((code_len, count)) = kinds.get(index) {
            code_first_n[index] = CodeItem {
                code_len: *code_len,
                ratio: *count as f64 / denominator as f64 * 100.0,
            };
        }
        ratio_sum += code_first_n[index].ratio;
    }
    code_first_n[3] = CodeItem {
        code_len: 0,
        ratio: 100.0 - ratio_sum,
    };

    let avg_delt: f64 = bucket.avg_gaps.iter().sum();
    let avg_cnts: f64 = bucket.avg_cnts.iter().sum();
    let mut avg_spd = 0.0;
    if avg_delt > 1.0 {
        avg_spd = avg_cnts / avg_delt * 60.0;
        // 本期均速高于历史极速时覆盖“极速”三项（三项同生同死）。
        if avg_spd > fastest.spd {
            fastest.spd = avg_spd;
            fastest.key_touch_spd = avg_spd * avg_code_len / 60.0;
            fastest.avg_code_len = avg_code_len;
        }
    }

    Statistics {
        fastest,
        avg_spd,
        avg_code_len,
        ratio,
        code_first_n,
    }
}

/// 报表渲染所需的上下文（宿主提供）。
#[derive(Clone, Copy, Debug)]
pub struct ReportInput<'a> {
    pub kind: ReportKind,
    pub bucket: &'a Bucket,
    pub style: Style,
    pub device_type: DeviceType,
    pub schema_name: &'a str,
    pub software_name: &'a str,
    pub software_version: &'a str,
    pub avg_code_len_desc: &'a str,
    /// 已选好的名言；短款忽略。
    pub quote: Option<&'a str>,
    /// 本地时间字符串 `YYYY/MM/DD HH:MM:SS`。
    pub timestamp: &'a str,
    /// 时区标签 `UTC+08:00`。
    pub timezone: &'a str,
    pub skin_word: Skin,
    pub skin_code: Skin,
}

/// 拼装槽位：以 `\n` 连接 1..21 → 折叠连续换行 → 去首尾换行 → trim。
fn assemble(slots: &[String; 22]) -> String {
    let mut joined = String::new();
    for index in 1..=21 {
        if index > 1 {
            joined.push('\n');
        }
        joined.push_str(&slots[index]);
    }
    let mut collapsed = String::with_capacity(joined.len());
    let mut last_was_newline = false;
    for ch in joined.chars() {
        if ch == '\n' {
            if last_was_newline {
                continue;
            }
            last_was_newline = true;
        } else {
            last_was_newline = false;
        }
        collapsed.push(ch);
    }
    collapsed
        .trim_start_matches('\n')
        .trim_end_matches('\n')
        .trim()
        .to_owned()
}

/// 渲染一份周期报表（规范 §6.1–§6.3、§6.7、§6.8）。
pub fn format_report(input: &ReportInput<'_>) -> String {
    let stats = statistics_cal(input.bucket);
    let long = input.style == Style::Long;
    let mut slots: [String; 22] = Default::default();

    // §6.1 头部
    slots[1] = embed_text_into_splitor(&format!("🚩 {} 🚩", input.kind.stat_type()));
    let _ = write!(slots[2], "{}{}", input.timestamp, input.timezone);
    let _ = write!(
        slots[4],
        "上屏 {} 次，输入 {} 字",
        input.bucket.count, input.bucket.length
    );
    // 键准 = 有效按键 / 按键（有效按键即不含空格的码长合计）。
    // 理论值 = 实际值 × 键准：因为 击键 = 按键/用时、码长 = 按键/字数，
    // 把“按键”换成“有效按键”就得到理论击键 / 理论码长。
    let accuracy = if input.bucket.key_touch_cnt > 0 {
        input.bucket.total_code_len_without_space as f64 / input.bucket.key_touch_cnt as f64
    } else {
        1.0
    };
    let _ = write!(
        slots[5],
        "实际极速 {:.1}，击键{:.1}，码长{:.1}",
        stats.fastest.spd,
        stats.fastest.key_touch_spd,
        stats.fastest.avg_code_len
    );
    let _ = write!(
        slots[6],
        "实际均速 {:.1}，击键{:.1}，码长{:.1}",
        stats.avg_spd,
        stats.avg_spd * stats.avg_code_len / 60.0,
        stats.avg_code_len
    );
    // 理论值 = 实际值 × 键准（有效按键 / 按键）。
    let _ = write!(
        slots[6],
        "\n理论极速 {:.1}，击键{:.1}，码长{:.1}",
        stats.fastest.spd,
        stats.fastest.key_touch_spd * accuracy,
        stats.fastest.avg_code_len * accuracy
    );
    let _ = write!(
        slots[6],
        "\n理论均速 {:.1}，击键{:.1}，码长{:.1}",
        stats.avg_spd,
        stats.avg_spd * stats.avg_code_len / 60.0 * accuracy,
        stats.avg_code_len * accuracy
    );
    let err_count = (input.bucket.key_touch_cnt - input.bucket.total_code_len_without_space).max(0);
    let err_ratio = if input.bucket.key_touch_cnt > 0 {
        100.0 * err_count as f64 / input.bucket.key_touch_cnt as f64
    } else {
        0.0
    };
    // 键准 = 有效按键 / 按键（有效按键即不含空格的码长合计）。
    // 由它可以直接推出「理论」值：理论击键/理论码长 = 实际值 × 键准
    // （因为 击键 = 按键/用时、码长 = 按键/字数，把“按键”换成“有效按键”即可）。
    let accuracy = if input.bucket.key_touch_cnt > 0 {
        input.bucket.total_code_len_without_space as f64 / input.bucket.key_touch_cnt as f64
    } else {
        1.0
    };
    let avg_touch = stats.avg_spd * stats.avg_code_len / 60.0;
    let _ = write!(
        slots[6],
        "\n误触按键 {} 次，误触率 {:.0}%，键准 {:.0}%",
        err_count,
        err_ratio,
        accuracy * 100.0
    );
    let _ = write!(
        slots[6],
        "\n实际击键 {:.1}，理论击键 {:.1}，实际码长 {:.1}，理论码长 {:.1}",
        avg_touch,
        avg_touch * accuracy,
        stats.avg_code_len,
        stats.avg_code_len * accuracy
    );
    if long && !input.avg_code_len_desc.is_empty() {
        slots[6].push('\n');
        slots[6].push_str(input.avg_code_len_desc);
    }
    slots[7] = if long {
        emoji_splitor("📊")
    } else {
        splitor()
    };
    slots[16] = splitor();
    if !long {
        slots[1].clear();
        slots[2].clear();
    }

    // §6.2 字长分布
    let ratio = stats.ratio;
    if long {
        if ratio.one_char > 0.0 {
            let _ = write!(
                slots[8],
                "{}单字{:>3.0}%",
                progress_bar(ratio.one_char, input.skin_word),
                ratio.one_char
            );
        }
        if ratio.two_char > 0.0 {
            let _ = write!(
                slots[9],
                "{} 2字{:>3.0}%",
                progress_bar(ratio.two_char, input.skin_word),
                ratio.two_char
            );
        }
        if ratio.other > 0.0 {
            let _ = write!(
                slots[10],
                "{}>2字{:>3.0}%",
                progress_bar(ratio.other, input.skin_word),
                ratio.other
            );
        }
    } else {
        if ratio.one_char > 0.0 {
            let _ = write!(slots[8], "单字{:.0}%", ratio.one_char);
        }
        if ratio.two_char > 0.0 {
            if slots[8].is_empty() {
                let _ = write!(slots[8], "; 2字{:.0}%", ratio.two_char);
            } else {
                let _ = write!(slots[8], "; 2字{:.0}%", ratio.two_char);
            }
        }
        if ratio.other > 0.0 {
            if slots[8].is_empty() {
                // 忠实复现 Lua L636 的兜底文案（写成 "2字" 而非 ">2字"，规范 §6.2 标注为疑似 bug）。
                let _ = write!(slots[8], "; 2字{:.0}%", ratio.other);
            } else {
                let _ = write!(slots[8], "; >2字{:.0}%", ratio.other);
            }
        }
        slots[9].clear();
        slots[10].clear();
    }

    // §6.3 码长分布
    if long {
        slots[3] = emoji_splitor("📈");
        slots[11] = emoji_splitor("📊");
        for index in 0..3 {
            let item = stats.code_first_n[index];
            if item.ratio > 0.0 {
                let code_len = if item.code_len < 10 {
                    format!(" {}", item.code_len)
                } else {
                    item.code_len.to_string()
                };
                let _ = write!(
                    slots[12 + index],
                    "{}{}码{:>3.0}%",
                    progress_bar(item.ratio, input.skin_code),
                    code_len,
                    item.ratio
                );
            }
        }
        if stats.code_first_n[3].ratio > 0.0 {
            let _ = write!(
                slots[15],
                "{}其它{:>3.0}%",
                progress_bar(stats.code_first_n[3].ratio, input.skin_code),
                stats.code_first_n[3].ratio
            );
        }
    } else {
        for index in 0..4 {
            let item = stats.code_first_n[index];
            if item.ratio <= 0.0 {
                continue;
            }
            let body = if index == 3 {
                format!("其它{:.0}%", item.ratio)
            } else {
                format!("{}码{:.0}%", item.code_len, item.ratio)
            };
            if slots[12].is_empty() {
                slots[12] = body;
            } else {
                slots[12].push_str("; ");
                slots[12].push_str(&body);
            }
        }
        slots[13].clear();
        slots[14].clear();
        slots[15].clear();
    }

    // §6.0 槽 17..21：方案/平台/脚本/名言
    if long {
        let _ = write!(slots[17], "◉ 方案：{}", input.schema_name);
        let _ = write!(
            slots[18],
            "◉ 平台：{}{} {}",
            input.device_type.emoji(),
            input.software_name,
            input.software_version
        );
        slots[19] = splitor();
        let _ = write!(
            slots[20],
            "{}{}",
            SCRIPT_LINE_PREFIX, SCRIPT_VERSION
        );
        if let Some(quote) = input.quote {
            let _ = write!(slots[21], "{}\n{}", splitor(), quote);
        }
    } else {
        let _ = write!(
            slots[17],
            "{}・{}",
            input.device_type.emoji(),
            input.schema_name
        );
    }

    assemble(&slots)
}

/// 皮肤清单（规范 §6.6）。
pub fn format_skin_list(skin_idx_word: i64, skin_idx_code: i64) -> String {
    let mut lines = vec!["※ 可用皮肤列表：".to_owned()];
    for (offset, skin) in SKINS.iter().enumerate() {
        let index = offset as i64 + 1;
        let prefix = format!(
            "{}{}",
            if skin_idx_word == index { "-w-" } else { "--- " },
            if skin_idx_code == index { "-c-" } else { "---" }
        );
        let first = (b'a' + (index / 10) as u8) as char;
        let second = (b'a' + (index % 10) as u8) as char;
        let field: String = std::iter::repeat_n(skin.field, 4).collect();
        let empty: String = std::iter::repeat_n(skin.empty, 4).collect();
        lines.push(format!(
            "{prefix}{index:02} {first}{second} {field}{empty}"
        ));
    }
    lines.push("w 当前字词统计皮肤 / c 当前码长统计皮肤".to_owned());
    lines.push("输入 //60 或 /pf 查看皮肤列表".to_owned());
    lines.push("输入 /6xx1 或 /hfwyy 或 /pfwyy 切换字词统计皮肤(xx为数字，yy为字母)".to_owned());
    lines.push("输入 /6xx2 或 /hfcyy 或 /pfcyy 切换码长统计皮肤(xx为数字，yy为字母)".to_owned());
    let joined = lines.join("\n");
    let mut collapsed = String::with_capacity(joined.len());
    let mut last_was_newline = false;
    for ch in joined.chars() {
        if ch == '\n' {
            if last_was_newline {
                continue;
            }
            last_was_newline = true;
        } else {
            last_was_newline = false;
        }
        collapsed.push(ch);
    }
    collapsed
        .trim_start_matches('\n')
        .trim_end_matches('\n')
        .to_owned()
}

/// 生字/词本（规范 §6.4）。
///
/// `ver_str` 由于 Lua 依赖隐藏状态，这里固定传脚本行（见模块文档的有意偏离）。
pub fn format_new_words(
    words: &BTreeSet<String>,
    start_time: i64,
    tz_offset_seconds: i32,
) -> String {
    if words.is_empty() {
        return "※ 未发现生字/词记录。".to_owned();
    }
    let mut slots: [String; 8] = Default::default();
    slots[1] = embed_text_into_splitor("🚩 生字/词本 🚩");
    if start_time > 0 {
        slots[2] = format!(
            "自 {} 以来，",
            format_datetime(start_time, tz_offset_seconds)
        );
    }
    slots[3] = format!("共记录到生字/词 {} 个", words.len());
    slots[4] = splitor();
    slots[5] = words.iter().cloned().collect::<Vec<_>>().join("，");
    slots[6] = splitor();
    slots[7] = format!("{}{}", SCRIPT_LINE_PREFIX, SCRIPT_VERSION);

    let joined = (1..=7)
        .map(|index| slots[index].clone())
        .collect::<Vec<_>>()
        .join("\n");
    let mut collapsed = String::with_capacity(joined.len());
    let mut last_was_newline = false;
    for ch in joined.chars() {
        if ch == '\n' {
            if last_was_newline {
                continue;
            }
            last_was_newline = true;
        } else {
            last_was_newline = false;
        }
        collapsed.push(ch);
    }
    collapsed
        .trim_start_matches('\n')
        .trim_end_matches('\n')
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Bucket;
    use std::collections::BTreeMap;

    fn bucket_with(count: i64, length: i64) -> Bucket {
        let mut bucket = Bucket::default();
        bucket.count = count;
        bucket.length = length;
        bucket.lengths = BTreeMap::from([(1_i64, 8_i64), (2_i64, 6_i64)]);
        bucket.code_lengths = BTreeMap::from([(3_i64, 4_i64), (4_i64, 10_i64)]);
        bucket.key_touch_cnt = 100;
        bucket.total_code_len_without_space = 88;
        bucket.avg_gaps = vec![4.0, 7.0];
        bucket.avg_cnts = vec![6.0, 8.0];
        bucket.avg_code_len = vec![18.0, 26.0];
        bucket.fastest = Fastest {
            spd: 90.0,
            key_touch_spd: 5.0,
            avg_code_len: 3.0,
        };
        bucket
    }

    fn input<'a>(kind: ReportKind, bucket: &'a Bucket, style: Style) -> ReportInput<'a> {
        ReportInput {
            kind,
            bucket,
            style,
            device_type: DeviceType::Desktop,
            schema_name: "luna_pinyin",
            software_name: "Weasel",
            software_version: "1.0",
            avg_code_len_desc: "",
            quote: None,
            timestamp: "2026/10/02 13:05:00",
            timezone: "UTC+08:00",
            skin_word: SKINS[20],
            skin_code: SKINS[20],
        }
    }

    #[test]
    fn progress_bar_uses_threshold_ladder() {
        let skin = SKINS[0];
        assert_eq!(progress_bar(0.0, skin), "▁▁▁▁▁▁▁▁▁▁");
        assert_eq!(progress_bar(5.0, skin), "▉▁▁▁▁▁▁▁▁▁");
        assert_eq!(progress_bar(14.9, skin), "▉▁▁▁▁▁▁▁▁▁", "不足 15 仍是 1 格");
        assert_eq!(progress_bar(15.0, skin), "▉▉▁▁▁▁▁▁▁▁");
        assert_eq!(progress_bar(50.0, skin), "▉▉▉▉▉▁▁▁▁▁", "50 落在 >=45 档");
        assert_eq!(progress_bar(95.0, skin), "▉▉▉▉▉▉▉▉▉▉");
        assert_eq!(progress_bar(100.0, skin), "▉▉▉▉▉▉▉▉▉▉");
    }

    #[test]
    fn embed_text_centers_into_fourteen_columns() {
        assert_eq!(embed_text_into_splitor("🚩 日统计 🚩").chars().count(), 14);
        assert!(embed_text_into_splitor("🚩 日统计 🚩").starts_with('─'));
        // 文字过长时原样返回
        let long = "一二三四五六七八九十一二三四五六七八";
        assert_eq!(embed_text_into_splitor(long), long);
    }

    #[test]
    fn statistics_cal_prefers_current_average_when_higher() {
        let bucket = bucket_with(9, 20);
        let stats = statistics_cal(&bucket);
        // 均速 = (6+8)/(4+7)*60 = 76.36 < 90 → 保留历史极速
        assert!((stats.avg_spd - 14.0 / 11.0 * 60.0).abs() < 1e-9);
        assert_eq!(stats.fastest.spd, 90.0);

        let mut slow = bucket_with(9, 20);
        slow.fastest.spd = 10.0;
        let stats = statistics_cal(&slow);
        // 均速 76.36 > 10 → 覆盖极速，且 keyTouchSpd = avgSpd * avgCodeLen / 60
        assert!((stats.fastest.spd - stats.avg_spd).abs() < 1e-9);
        assert!((stats.fastest.key_touch_spd - stats.avg_spd * stats.avg_code_len / 60.0).abs() < 1e-9);
        assert!((stats.fastest.avg_code_len - stats.avg_code_len).abs() < 1e-9);
    }

    #[test]
    fn statistics_cal_uses_chars_as_code_length_denominator() {
        let bucket = bucket_with(9, 20);
        let stats = statistics_cal(&bucket);
        // 总码长 = 3*4 + 4*10 = 52；总字数 = 1*8 + 2*6 = 20
        assert!((stats.avg_code_len - 52.0 / 20.0).abs() < 1e-12);
        // 码长占比分母是上屏次数 = 14：4 码 10/14、3 码 4/14
        assert_eq!(stats.code_first_n[0].code_len, 4);
        assert!((stats.code_first_n[0].ratio - 10.0 / 14.0 * 100.0).abs() < 1e-9);
        assert_eq!(stats.code_first_n[1].code_len, 3);
        assert!((stats.code_first_n[3].ratio - 0.0).abs() < 1e-9, "前三项已达 100%");
    }

    #[test]
    fn empty_bucket_messages_match_lua() {
        assert_eq!(ReportKind::Daily.empty_message(), "※ 今天没有任何记录。");
        assert_eq!(ReportKind::Weekly.empty_message(), "※ 本周没有任何记录。");
        assert_eq!(ReportKind::Monthly.empty_message(), "※ 本月没有任何记录。");
        assert_eq!(ReportKind::Yearly.empty_message(), "※ 本年没有任何记录。");
    }

    #[test]
    fn short_report_layout_is_exact() {
        let bucket = bucket_with(9, 20);
        let text = format_report(&input(ReportKind::Daily, &bucket, Style::Short));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "上屏 9 次，输入 20 字");
        assert_eq!(lines[1], "极速 90.0，击键5.0，码长3.0");
        assert_eq!(lines[2], "均速 76.4，击键3.3，码长2.6");
        assert_eq!(lines[3], "误触按键 12 次，误触率 12%");
        assert_eq!(lines[4], "──────────────");
        assert_eq!(lines[5], "单字40%; 2字60%", "短款单行拼接，; 分隔");
        assert_eq!(lines[6], "4码71%; 3码29%");
        assert_eq!(lines[7], "──────────────");
        assert_eq!(lines[8], "💻・luna_pinyin");
        assert!(!text.contains('━'), "短款不出现进度条");
        assert!(!text.contains("脚本："), "短款不显示脚本行");
    }

    #[test]
    fn long_report_layout_is_exact() {
        let bucket = bucket_with(9, 20);
        let mut report = input(ReportKind::Weekly, &bucket, Style::Long);
        report.avg_code_len_desc = "（说明）";
        report.quote = Some("名言一句");
        let text = format_report(&report);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 21, "长款共 21 行（空槽位在拼装时被折叠）");
        assert_eq!(lines[0], "───🚩 周统计 🚩────", "标题居中到 14 列");
        assert_eq!(lines[1], "2026/10/02 13:05:00UTC+08:00");
        assert_eq!(lines[2], "📈─────────────");
        assert_eq!(lines[3], "上屏 9 次，输入 20 字");
        assert_eq!(lines[4], "极速 90.0，击键5.0，码长3.0");
        assert_eq!(lines[5], "均速 76.4，击键3.3，码长2.6");
        assert_eq!(lines[6], "误触按键 12 次，误触率 12%");
        assert_eq!(lines[7], "（说明）", "长款追加 avgCodeLenDesc");
        assert_eq!(lines[8], "📊─────────────");
        assert_eq!(lines[9], "★★★★☆☆☆☆☆☆单字 40%", "字词进度条用皮肤 21");
        assert_eq!(lines[10], "★★★★★★☆☆☆☆ 2字 60%");
        assert_eq!(lines[11], "📊─────────────");
        assert_eq!(lines[12], "★★★★★★★☆☆☆ 4码 71%");
        assert_eq!(lines[13], "★★★☆☆☆☆☆☆☆ 3码 29%");
        assert_eq!(lines[14], "──────────────");
        assert_eq!(lines[15], "◉ 方案：luna_pinyin");
        assert_eq!(lines[16], "◉ 平台：💻Weasel 1.0");
        assert_eq!(lines[17], "──────────────");
        assert_eq!(lines[18], format!("脚本：🐐🪶❄️{}", SCRIPT_VERSION));
        assert_eq!(lines[19], "──────────────");
        assert_eq!(lines[20], "名言一句");
    }

    #[test]
    fn two_char_fallback_message_is_reproduced() {
        // 只有 >2 字占比（无单字/双字）时，Lua 的兜底文案写成 "2字"（疑似 bug，需复现）
        let mut bucket = bucket_with(1, 1);
        bucket.lengths = BTreeMap::from([(3_i64, 1_i64)]);
        let text = format_report(&input(ReportKind::Daily, &bucket, Style::Short));
        assert!(
            text.contains("; 2字100%"),
            "复现 Lua 兜底文案，实际：{text}"
        );
    }

    #[test]
    fn skin_list_matches_lua_template() {
        let text = format_skin_list(21, 1);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 28, "1 行表头 + 23 行皮肤 + 4 行说明");
        assert_eq!(lines[0], "※ 可用皮肤列表：");
        assert_eq!(lines[1], "--- -c-01 ab ▉▉▉▉▁▁▁▁");
        assert_eq!(lines[21], "-w----21 cb ★★★★☆☆☆☆", "字词命中为 -w-、码长未命中为 ---，拼接后无空格");
        assert_eq!(lines[23], "--- ---23 cd ████░░░░");
        assert_eq!(lines[24], "w 当前字词统计皮肤 / c 当前码长统计皮肤");
        assert_eq!(lines[25], "输入 //60 或 /pf 查看皮肤列表");
        assert!(lines[26].starts_with("输入 /6xx1 或 /hfwyy 或 /pfwyy"));
        assert!(lines[27].starts_with("输入 /6xx2 或 /hfcyy 或 /pfcyy"));
    }

    #[test]
    fn new_words_report_layout() {
        let words: BTreeSet<String> = ["你好".to_owned(), "世界".to_owned()].into_iter().collect();
        let text = format_new_words(&words, 1_700_000_000, 8 * 3600);
        assert!(text.contains("🚩 生字/词本 🚩"));
        assert!(text.contains("自 2023/11/15"), "本地时间行：{text}");
        assert!(text.contains("共记录到生字/词 2 个"));
        assert!(text.contains("世界，你好"), "集合按字典序（有意偏离 Lua 的哈希序）：{text}");
        assert!(text.contains(&format!("脚本：🐐🪶❄️{SCRIPT_VERSION}")));

        assert_eq!(
            format_new_words(&BTreeSet::new(), 0, 8 * 3600),
            "※ 未发现生字/词记录。"
        );
    }

    #[test]
    fn device_type_parsing_and_emoji() {
        assert_eq!(DeviceType::parse("📱"), DeviceType::Phone);
        assert_eq!(DeviceType::parse("💻"), DeviceType::Desktop);
        assert_eq!(DeviceType::parse(""), DeviceType::Unknown);
        assert_eq!(DeviceType::parse("x"), DeviceType::Unknown);
        assert_eq!(DeviceType::Phone.emoji(), "📱");
        assert_eq!(DeviceType::Unknown.emoji(), "");
    }
}
