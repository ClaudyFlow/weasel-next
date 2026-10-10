//! 统计状态、累积算法与翻译器侧的纯逻辑。
//!
//! 语义**严格对齐** `docs/lua-parity-spec.md`（源自 `rime-lua/lua/input_statistics.lua`）：
//! * 时间一律是 **Unix 整数秒**（Lua 用 `os.time()`；规范 §0/§3.2 特别强调原型里的毫秒注释是错的）；
//! * 极速窗口固定 **10 秒**，**只扫 `yearly` 的三条序列**，结果同时写回四个桶（§3.2）；
//! * 中断门限 `gapThd = 5` 秒、卡壳门限 `boggleThd_s = 3` 秒（§1.2、§3.7）；
//! * 落盘格式版本 `logVer = 4`，低于它就**破坏性重置**且只保留皮肤索引与设备类型（§4.4）。
//!
//! 本模块保持纯计算（无 I/O、不读系统时间），宿主通过 [`BucketBounds`] 与时间戳参数注入环境。

use std::collections::{BTreeMap, BTreeSet};

/// 极速滑动窗口：10 秒（Lua：`latestGapsSum >= 10`）。
pub const FASTEST_WINDOW_SECONDS: f64 = 10.0;
/// 输入中断门限：两次按键间隔 **严格大于** 5 秒即视为中断。
pub const INTERRUPT_GAP_SECONDS: i64 = 5;
/// 卡壳（生字/词）判定门限，单位秒，默认 3；Lua 顶部可配置项 `boggleThd_s`。
pub const DEFAULT_BOGGLE_THRESHOLD_SECONDS: i64 = 3;
/// 当前代码期望的落盘格式版本（Lua `verOfStatsFormat`）。
pub const LOG_VERSION: i64 = 4;
/// 皮肤索引默认值（Lua 初始 schema 为 21）。
pub const DEFAULT_SKIN_INDEX: i64 = 21;

/// 极速记录：某个时间桶内观察到的最大瞬时速度。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fastest {
    /// 字/分
    pub spd: f64,
    /// 键/秒（注意不是键/分）
    pub key_touch_spd: f64,
    /// 键/字
    pub avg_code_len: f64,
}

/// 一个时间桶（日/周/月/年）的累计数据。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bucket {
    /// 上屏次数。
    pub count: i64,
    /// 上屏总字数（UTF-8 码点数）。
    pub length: i64,
    /// 该桶内的极速记录。
    pub fastest: Fastest,
    /// 本桶周期起点（Unix 秒，本地时区）。
    pub ts: i64,
    /// 字长分布：`lengths[j] = j 字上屏的次数`。
    pub lengths: BTreeMap<i64, i64>,
    /// 码长分布：`code_lengths[k] = 码长为 k 的上屏次数`（值是**次数**，不是字数）。
    pub code_lengths: BTreeMap<i64, i64>,
    /// 每段会话时长（秒）序列，无上限增长，周期重置时清空。
    pub avg_gaps: Vec<f64>,
    /// 每段会话的上屏字数序列。
    pub avg_cnts: Vec<f64>,
    /// 每段会话的累计码长序列。
    pub avg_code_len: Vec<f64>,
    /// 累计击键数（含误触与空格）。
    pub key_touch_cnt: i64,
    /// 累计"不含空格编码数"（= Σ `string.len(ctx.input)`）。
    pub total_code_len_without_space: i64,
}

impl Bucket {
    /// 以一个周期起点构造空桶。
    pub fn at(ts: i64) -> Self {
        Self {
            ts,
            ..Self::default()
        }
    }

    /// 桶内总字数（由 `lengths` 推出，供码长报表的第二套分母使用）。
    pub fn total_chars(&self) -> i64 {
        self.lengths.iter().map(|(j, n)| j * n).sum()
    }

    /// 桶内上屏次数（由 `code_lengths` 推出，供码长占比的分母使用）。
    pub fn total_commits(&self) -> i64 {
        self.code_lengths.values().sum()
    }
}

/// 四个周期桶的起点（本地时区的日/周/月/年边界，Unix 秒）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BucketBounds {
    pub day: i64,
    pub week: i64,
    pub month: i64,
    pub year: i64,
}

/// 生字/词本（Lua `input_stat.newWords`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewWords {
    /// 本册起始时间（Unix 秒）；`0` 表示尚未开始。
    pub start_time: i64,
    /// 生字/词集合，键是上屏文本本身。
    pub words: BTreeSet<String>,
}

/// 全局统计（对应 Lua 根表 `input_stat`）。
#[derive(Clone, Debug, PartialEq)]
pub struct InputStats {
    pub daily: Bucket,
    pub weekly: Bucket,
    pub monthly: Bucket,
    pub yearly: Bucket,
    /// 已落盘格式版本；`-1` 表示尚未迁移（Lua 里是 `logVer = -1` / `nil`）。
    pub log_ver: i64,
    /// 当日最长一次上屏的字数（仅日维度）。
    pub daily_max: i64,
    pub new_words: NewWords,
    /// 设备类型：只允许 `"📱"`、`"💻"`、`""`（空串=识别失败）。
    pub device_type: String,
    /// 字词统计条皮肤索引（1..=23）。
    pub skin_idx_word: i64,
    /// 码长统计条皮肤索引（1..=23）。
    pub skin_idx_code: i64,
}

impl Default for InputStats {
    /// Lua 初始 schema（§2.1）：`logVer = -1`、皮肤索引 21、设备类型空串。
    fn default() -> Self {
        Self {
            daily: Bucket::default(),
            weekly: Bucket::default(),
            monthly: Bucket::default(),
            yearly: Bucket::default(),
            log_ver: -1,
            daily_max: 0,
            new_words: NewWords::default(),
            device_type: String::new(),
            skin_idx_word: DEFAULT_SKIN_INDEX,
            skin_idx_code: DEFAULT_SKIN_INDEX,
        }
    }
}

impl InputStats {
    /// 四个桶按固定顺序排列，避免各处重复写同样的分支。
    pub fn buckets_mut(&mut self) -> [&mut Bucket; 4] {
        [
            &mut self.daily,
            &mut self.weekly,
            &mut self.monthly,
            &mut self.yearly,
        ]
    }

    pub fn buckets(&self) -> [&Bucket; 4] {
        [&self.daily, &self.weekly, &self.monthly, &self.yearly]
    }

    /// 版本迁移（§4.4）：`log_ver < expected` 时**破坏性重置**，只保留皮肤索引与设备类型。
    ///
    /// 返回是否真的发生了重置。迁移只改内存、不落盘（与 Lua 版一致）。
    ///
    /// 与 Lua 版的有意偏离（登记于规范附录 B）：Lua 只在
    /// `progressBarSkinIdx_word ~= nil` 时才读取两个索引，导致"词索引存在而码索引为 nil"
    /// 时会把码索引写成 nil；这里对两个索引各自独立回退到 [`DEFAULT_SKIN_INDEX`]。
    pub fn migrate_format(&mut self, expected: i64) -> bool {
        if self.log_ver >= expected {
            return false;
        }
        let skin_word = if self.skin_idx_word >= 1 {
            self.skin_idx_word
        } else {
            DEFAULT_SKIN_INDEX
        };
        let skin_code = if self.skin_idx_code >= 1 {
            self.skin_idx_code
        } else {
            DEFAULT_SKIN_INDEX
        };
        let device_type = std::mem::take(&mut self.device_type);
        *self = Self {
            log_ver: expected,
            skin_idx_word: skin_word,
            skin_idx_code: skin_code,
            device_type,
            ..Self::default()
        };
        true
    }
}

/// 当前会话的实时速度缓存（Lua `avgSpdInfo`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvgSpdInfo {
    /// 本会话第一次按键的时间（Unix 秒）。
    pub start_time: i64,
    /// 最近一次按键时间（Unix 秒）。
    pub click_time: i64,
    /// 上一次事件（上屏或中断）的时间（Unix 秒）。
    pub commit_time: i64,
    /// 中断门限，单位秒（Lua `gapThd = 5`）。
    pub gap_thd: i64,
    /// 自上次结算以来上屏的字数。
    pub count: i64,
    /// 自上次结算以来按下的按键数。
    pub code_len: i64,
}

impl Default for AvgSpdInfo {
    fn default() -> Self {
        Self {
            start_time: 0,
            click_time: 0,
            commit_time: 0,
            gap_thd: INTERRUPT_GAP_SECONDS,
            count: 0,
            code_len: 0,
        }
    }
}

/// 一次上屏要提交给统计的量。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Commit {
    /// 本次上屏的字数（UTF-8 码点数）。
    pub input_length: i64,
    /// 本次上屏的码长（按键数，含顶屏空格）。
    pub code_len: i64,
    /// 本次上屏的有效码长（`string.len(ctx.input)`，不含顶屏空格）。
    pub code_len_without_space: i64,
    /// 是否值得参与平均分速结算（Lua `avgAvailable`）。
    pub avg_available: bool,
}

/// 会话是否有效（Lua 两处同款判据：`commitTime - startTime >= 1 且 count > 0`）。
pub fn session_available(avg: &AvgSpdInfo) -> bool {
    avg.commit_time - avg.start_time >= 1 && avg.count > 0
}

/// `avgAvailable`：与 [`session_available`] 同判据（§1.2 第 4 步）。
pub fn avg_available(avg: &AvgSpdInfo) -> bool {
    session_available(avg)
}

/// 是否应当判定为"输入中断"：两次按键间隔 **严格大于** `gap_thd` 秒（§1.2 第 1 步）。
pub fn should_interrupt(avg: &AvgSpdInfo, now_unix: i64) -> bool {
    now_unix - avg.click_time > avg.gap_thd
}

/// 一次上屏的码长修正（§3.6）：码长命中"自动顶屏码长集合"时**不** `+1`，否则 `+1`。
///
/// 默认集合 `{0}` 等价于"总是 +1"（真实码长不会是 0）。
pub fn adjusted_code_len(raw_code_len: i64, auto_commit: &[i64]) -> i64 {
    if auto_commit.contains(&raw_code_len) {
        raw_code_len
    } else {
        raw_code_len + 1
    }
}

/// 上屏字数：UTF-8 码点数（Lua `utf8.len(commit_text)`）。
pub fn utf8_char_count(text: &str) -> i64 {
    text.chars().count() as i64
}

/// 击键计数（§1.2 第 2 步）：返回本次编码变化应累加的击键数（0 或 1）。
///
/// `previous_code` 是上一次的编码串，`input` 是当前编码串；两者相等或 `input` 以 `/`
/// 开头时不计数（调用方需自行跳过命令输入）。长度一律按**字节**计算（`string.len`）。
pub fn count_key_press(previous_code: &str, input: &str) -> i64 {
    if input.starts_with('/') || previous_code == input {
        return 0;
    }
    let now = input.len() as i64;
    let previous = previous_code.len() as i64;
    if now > previous {
        return 1;
    }
    if now == 1 && (previous == 1 || previous > 2) {
        return 1;
    }
    if now == 1 && previous == 2 {
        // 前次两码：首字节不同视为 esc 后重打；相同则无法区分退格/esc，做 +0（源码注释如此）。
        return i64::from(previous_code.as_bytes().first() != input.as_bytes().first());
    }
    0
}

/// 生字/词判定（§3.7）：与**上一次上屏**的间隔 `>= boggle_threshold_s` 时记入，否则移除。
pub fn observe_new_word(
    stats: &mut InputStats,
    commit_text: &str,
    now_unix: i64,
    boggle_threshold_s: i64,
    last_commit_time: i64,
) {
    if now_unix - last_commit_time >= boggle_threshold_s {
        if stats.new_words.start_time < 1 || stats.new_words.words.is_empty() {
            stats.new_words.start_time = now_unix;
        }
        stats.new_words.words.insert(commit_text.to_owned());
    } else {
        stats.new_words.words.remove(commit_text);
    }
}

/// 累积一次上屏；等价于 Lua 版 `update_stats(input_length, codeLen, codeLenWithoutSpace, avgAvailable)`。
///
/// `key_touch_cnt` 是自上次上屏以来累计的按键数，处理完本次上屏后清零（§3.4）。
/// 每次调用都会先做跨周期刷新，即便 `input_length == 0`（§3.1）。
pub fn update_stats(
    stats: &mut InputStats,
    avg: &mut AvgSpdInfo,
    key_touch_cnt: &mut i64,
    bounds: BucketBounds,
    commit: Commit,
) {
    // 1) 跨周期刷新：时间戳不相等即整桶重建（时间回拨同样触发，§3.1）。
    if stats.daily.ts != bounds.day {
        stats.daily = Bucket::at(bounds.day);
        stats.daily_max = 0;
    }
    if stats.weekly.ts != bounds.week {
        stats.weekly = Bucket::at(bounds.week);
    }
    if stats.monthly.ts != bounds.month {
        stats.monthly = Bucket::at(bounds.month);
    }
    if stats.yearly.ts != bounds.year {
        stats.yearly = Bucket::at(bounds.year);
    }

    // 2) 会话结算与极速窗口（§3.2）。
    if commit.avg_available {
        let delta = (avg.commit_time - avg.start_time) as f64;
        for bucket in stats.buckets_mut() {
            bucket.avg_gaps.push(delta);
            bucket.avg_code_len.push(avg.code_len as f64);
            bucket.avg_cnts.push(avg.count as f64);
        }

        // 窗口只扫 yearly 的三条序列，从最新往回累加，先加后判。
        let len = stats.yearly.avg_gaps.len();
        let mut gaps_sum = 0.0_f64;
        let mut cnts_sum = 0.0_f64;
        let mut code_len_sum = 0.0_f64;
        for index in 0..len {
            gaps_sum += stats.yearly.avg_gaps[len - 1 - index];
            cnts_sum += stats.yearly.avg_cnts[len - 1 - index];
            code_len_sum += stats.yearly.avg_code_len[len - 1 - index];
            if gaps_sum >= FASTEST_WINDOW_SECONDS {
                break;
            }
        }
        if gaps_sum >= FASTEST_WINDOW_SECONDS {
            let spd = cnts_sum / gaps_sum * 60.0;
            let key_touch_spd = code_len_sum / gaps_sum;
            let avg_code_len = if cnts_sum > 0.0 {
                code_len_sum / cnts_sum
            } else {
                0.0
            };
            for bucket in stats.buckets_mut() {
                // 三项同生同死，判据是严格大于。
                if spd > bucket.fastest.spd {
                    bucket.fastest = Fastest {
                        spd,
                        key_touch_spd,
                        avg_code_len,
                    };
                }
            }
        }

        avg.count = 0;
        avg.code_len = 0;
    }

    // 3) 空上屏到此为止（§3.3）。
    if commit.input_length < 1 {
        return;
    }

    // 4) 击键、码长、次数/字数/极值与分布（§3.4、§3.5）。
    for bucket in stats.buckets_mut() {
        bucket.key_touch_cnt += *key_touch_cnt;
        bucket.total_code_len_without_space += commit.code_len_without_space;
        bucket.count += 1;
        bucket.length += commit.input_length;
        *bucket.lengths.entry(commit.input_length).or_insert(0) += 1;
        *bucket.code_lengths.entry(commit.code_len).or_insert(0) += 1;
    }
    *key_touch_cnt = 0;

    if commit.input_length > stats.daily_max {
        stats.daily_max = commit.input_length;
    }
}

/// 码长加权和 `Σ(k × n_k)`（`code_lengths` 的值是上屏次数）。
pub fn code_length_weighted_sum(code_lengths: &BTreeMap<i64, i64>) -> i64 {
    code_lengths.iter().map(|(k, n)| k * n).sum()
}

/// 平均码长（**按上屏次数加权**）：`Σ(k × n_k) / Σ n_k`。
///
/// 这是"码长占比"口径；报表里的平均码长用的是 [`report_avg_code_len`]（分母是总字数）。
pub fn average_code_len_by_commits(code_lengths: &BTreeMap<i64, i64>) -> f64 {
    let total = code_lengths.values().sum::<i64>();
    if total == 0 {
        0.0
    } else {
        code_length_weighted_sum(code_lengths) as f64 / total as f64
    }
}

/// 报表平均码长（§6.5）：`Σ(k × n_k) / 总字数`，分母来自 `lengths`。
pub fn report_avg_code_len(code_lengths: &BTreeMap<i64, i64>, lengths: &BTreeMap<i64, i64>) -> f64 {
    let total_chars: i64 = lengths.iter().map(|(j, n)| j * n).sum();
    if total_chars == 0 {
        0.0
    } else {
        code_length_weighted_sum(code_lengths) as f64 / total_chars as f64
    }
}

/// 字长占比：1 字词 / 2 字词 / 其他，按字数加权。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CharLengthRatio {
    pub one_char: f64,
    pub two_char: f64,
    pub other: f64,
}

/// `r_j = (j × n_j) / Σ(i × n_i) × 100%`。
pub fn char_length_ratio(lengths: &BTreeMap<i64, i64>) -> CharLengthRatio {
    let total: i64 = lengths.iter().map(|(j, n)| j * n).sum();
    let total = if total == 0 { 1 } else { total };
    let v1 = lengths.get(&1).copied().unwrap_or(0);
    let v2 = lengths.get(&2).copied().unwrap_or(0) * 2;
    let v3 = total - v1 - v2;
    CharLengthRatio {
        one_char: v1 as f64 / total as f64 * 100.0,
        two_char: v2 as f64 / total as f64 * 100.0,
        other: v3 as f64 / total as f64 * 100.0,
    }
}

/// 整段周期的均速（字/分）：`Σ字数 / Σ时长 × 60`；时长不足 1 秒时不计算（§6.5）。
pub fn average_speed(bucket: &Bucket) -> f64 {
    let gaps: f64 = bucket.avg_gaps.iter().sum();
    let cnts: f64 = bucket.avg_cnts.iter().sum();
    if gaps <= 1.0 {
        0.0
    } else {
        cnts / gaps * 60.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(day: i64) -> BucketBounds {
        BucketBounds {
            day,
            week: day,
            month: day,
            year: day,
        }
    }

    fn commit(input_length: i64, code_len: i64, avg_available: bool) -> Commit {
        Commit {
            input_length,
            code_len,
            code_len_without_space: code_len.saturating_sub(1),
            avg_available,
        }
    }

    #[test]
    fn default_matches_lua_initial_schema() {
        let stats = InputStats::default();
        assert_eq!(stats.log_ver, -1, "初始 logVer = -1，触发迁移");
        assert_eq!(stats.skin_idx_word, DEFAULT_SKIN_INDEX);
        assert_eq!(stats.skin_idx_code, DEFAULT_SKIN_INDEX);
        assert_eq!(stats.device_type, "");
        assert!(stats.new_words.words.is_empty());
        for bucket in stats.buckets() {
            assert_eq!(bucket.count, 0);
            assert!(bucket.avg_code_len.is_empty(), "avgCodeLen 字段必须始终存在");
        }
    }

    #[test]
    fn commit_accumulates_keys_and_distributions() {
        let mut stats = InputStats::default();
        let mut avg = AvgSpdInfo::default();
        let mut keys = 9_i64;
        update_stats(&mut stats, &mut avg, &mut keys, bounds(100), commit(3, 4, false));
        assert_eq!(stats.daily.count, 1);
        assert_eq!(stats.daily.length, 3);
        assert_eq!(stats.daily.lengths.get(&3), Some(&1));
        assert_eq!(stats.daily.code_lengths.get(&4), Some(&1));
        assert_eq!(stats.daily.key_touch_cnt, 9);
        assert_eq!(stats.daily.total_code_len_without_space, 3);
        assert_eq!(keys, 0, "按键计数结算后清零");
        assert_eq!(stats.daily_max, 3);
        for bucket in stats.buckets() {
            assert_eq!(bucket.count, 1);
            assert_eq!(bucket.length, 3);
        }
    }

    #[test]
    fn empty_commit_only_settles_average_window() {
        let mut stats = InputStats::default();
        let mut avg = AvgSpdInfo {
            start_time: 0,
            commit_time: 5,
            count: 6,
            code_len: 18,
            ..Default::default()
        };
        let mut keys = 7_i64;
        update_stats(&mut stats, &mut avg, &mut keys, bounds(100), commit(0, 0, true));
        assert_eq!(stats.daily.avg_gaps, vec![5.0]);
        assert_eq!(stats.daily.avg_cnts, vec![6.0]);
        assert_eq!(stats.daily.avg_code_len, vec![18.0]);
        assert_eq!(stats.daily.fastest, Fastest::default(), "不足 10 秒不记极速");
        assert_eq!(stats.daily.count, 0, "空上屏不计次数");
        assert_eq!(keys, 7, "空上屏不清零按键计数");
        assert_eq!(avg.count, 0);
        assert_eq!(avg.code_len, 0);
    }

    #[test]
    fn fastest_scans_yearly_and_writes_all_buckets() {
        let mut stats = InputStats::default();
        let mut avg = AvgSpdInfo::default();
        let mut keys = 0_i64;
        for (gap, cnt, code) in [(4_i64, 6_i64, 18_i64), (3, 5, 17), (4, 6, 18)] {
            avg.start_time = 0;
            avg.commit_time = gap;
            avg.count = cnt;
            avg.code_len = code;
            update_stats(
                &mut stats,
                &mut avg,
                &mut keys,
                bounds(100),
                commit(cnt, code, true),
            );
        }
        // 窗口 4+3+4 = 11 秒（先加后判，>=10 才停），C=17，K=53
        let spd = 17.0 / 11.0 * 60.0;
        for bucket in stats.buckets() {
            assert!((bucket.fastest.spd - spd).abs() < 1e-9);
            assert!((bucket.fastest.key_touch_spd - 53.0 / 11.0).abs() < 1e-9);
            assert!((bucket.fastest.avg_code_len - 53.0 / 17.0).abs() < 1e-9);
        }
    }

    #[test]
    fn fastest_keeps_strictly_greater_only() {
        let mut stats = InputStats::default();
        stats.daily.fastest = Fastest {
            spd: 100.0,
            key_touch_spd: 1.0,
            avg_code_len: 1.0,
        };
        let mut avg = AvgSpdInfo {
            start_time: 0,
            commit_time: 10,
            count: 10,
            code_len: 40,
            ..Default::default()
        };
        let mut keys = 0_i64;
        // bounds 与桶的 ts 相同（都是 0），避免跨周期重置掩盖本次断言。
        update_stats(&mut stats, &mut avg, &mut keys, bounds(0), commit(0, 0, true));
        assert_eq!(stats.daily.fastest.spd, 100.0, "平局/更低不覆盖");
        assert_eq!(stats.daily.fastest.key_touch_spd, 1.0, "三项同生同死");
        assert_eq!(stats.daily.fastest.avg_code_len, 1.0);
    }

    #[test]
    fn window_reset_clears_daily_bucket_and_max() {
        let mut stats = InputStats::default();
        let mut avg = AvgSpdInfo::default();
        let mut keys = 3_i64;
        update_stats(&mut stats, &mut avg, &mut keys, bounds(100), commit(5, 5, false));
        let mut keys = 2_i64;
        let mut next = bounds(100);
        next.day = 86_500;
        update_stats(&mut stats, &mut avg, &mut keys, next, commit(2, 2, false));
        assert_eq!(stats.daily.count, 1, "跨日重置后重新计数");
        assert_eq!(stats.daily.length, 2);
        assert_eq!(stats.daily_max, 2, "daily_max 随日桶重置");
        assert_eq!(stats.yearly.count, 2, "年桶不重置");
    }

    #[test]
    fn clock_rollback_also_resets_bucket() {
        let mut stats = InputStats::default();
        let mut avg = AvgSpdInfo::default();
        let mut keys = 1_i64;
        update_stats(&mut stats, &mut avg, &mut keys, bounds(200), commit(2, 2, false));
        let mut keys = 1_i64;
        update_stats(&mut stats, &mut avg, &mut keys, bounds(100), commit(1, 1, false));
        assert_eq!(stats.daily.count, 1, "时间回拨按不等式触发重建");
        assert_eq!(stats.daily.ts, 100);
    }

    #[test]
    fn session_availability_and_interrupt_follow_spec() {
        let avg = AvgSpdInfo {
            start_time: 10,
            commit_time: 11,
            count: 1,
            ..Default::default()
        };
        assert!(session_available(&avg));
        assert!(avg_available(&avg));
        let short = AvgSpdInfo {
            start_time: 10,
            commit_time: 10,
            count: 5,
            ..Default::default()
        };
        assert!(!session_available(&short), "时长不足 1 秒");
        let empty = AvgSpdInfo {
            start_time: 10,
            commit_time: 20,
            count: 0,
            ..Default::default()
        };
        assert!(!session_available(&empty), "无字数");

        let mut avg = AvgSpdInfo {
            click_time: 100,
            gap_thd: 5,
            ..Default::default()
        };
        assert!(!should_interrupt(&avg, 105), "恰好 5 秒不算中断（严格大于）");
        assert!(should_interrupt(&avg, 106));
        avg.gap_thd = 5;
        assert!(should_interrupt(&avg, 200));
    }

    #[test]
    fn key_press_count_matches_spec_table() {
        assert_eq!(count_key_press("", "a"), 1, "变长：按了新键");
        assert_eq!(count_key_press("n", "ni"), 1, "变长：按了新键");
        assert_eq!(count_key_press("n", "n"), 0, "编码未变");
        assert_eq!(count_key_press("ni", "n"), 0, "两码→单码且首字节相同：无法区分，做 +0");
        assert_eq!(count_key_press("ni", "m"), 1, "两码→单码且首字节不同：esc 后重打");
        assert_eq!(count_key_press("a", "b"), 1, "单码→单码");
        assert_eq!(count_key_press("abc", "x"), 1, "多码→单码：判定为 esc 清空");
        assert_eq!(count_key_press("nh", "nm"), 0, "同长度非单码：视为退格");
        assert_eq!(count_key_press("abc", "ab"), 0, "退格");
        assert_eq!(count_key_press("abc", "/rtj"), 0, "命令输入不计数");
        assert_eq!(count_key_press("", "/pf"), 0, "命令输入不计数");
    }

    #[test]
    fn code_len_adjustment_uses_auto_commit_set() {
        assert_eq!(adjusted_code_len(4, &[0]), 5, "默认集合等价于总是 +1");
        assert_eq!(adjusted_code_len(5, &[5, 6]), 5, "5 码自动顶屏不加空格");
        assert_eq!(adjusted_code_len(4, &[5, 6]), 5, "非顶屏码长补上空格");
    }

    #[test]
    fn new_word_is_recorded_and_removed_on_smooth_typing() {
        let mut stats = InputStats::default();
        observe_new_word(&mut stats, "你好", 100, 3, 90);
        assert!(stats.new_words.words.contains("你好"), "间隔 10 秒记为生词");
        assert_eq!(stats.new_words.start_time, 100);
        observe_new_word(&mut stats, "你好", 102, 3, 101);
        assert!(!stats.new_words.words.contains("你好"), "顺畅输入会移除该词");
        assert_eq!(stats.new_words.start_time, 100, "有词后不再更新起始时间");
        // 词集清空后再次卡壳会重新开始计时
        stats.new_words.words.clear();
        observe_new_word(&mut stats, "世界", 200, 3, 150);
        assert_eq!(stats.new_words.start_time, 200);
    }

    #[test]
    fn migration_is_destructive_but_keeps_skins_and_device() {
        let mut stats = InputStats::default();
        stats.daily.count = 9;
        stats.device_type = "💻".into();
        stats.skin_idx_word = 5;
        stats.skin_idx_code = 7;
        assert!(stats.migrate_format(LOG_VERSION));
        assert_eq!(stats.log_ver, LOG_VERSION);
        assert_eq!(stats.daily.count, 0, "历史被丢弃");
        assert_eq!(stats.device_type, "💻", "设备类型保留");
        assert_eq!(stats.skin_idx_word, 5);
        assert_eq!(stats.skin_idx_code, 7);
        assert!(!stats.migrate_format(LOG_VERSION), "版本已达标不再迁移");
    }

    #[test]
    fn derived_metrics_use_the_three_denominators() {
        // 码长分布：3 码 3 次、4 码 23 次；字长分布给出总字数 26
        let mut codes = BTreeMap::new();
        codes.insert(3_i64, 3_i64);
        codes.insert(4_i64, 23_i64);
        let mut lengths = BTreeMap::new();
        lengths.insert(1_i64, 2_i64);
        lengths.insert(2_i64, 12_i64);
        assert_eq!(lengths.iter().map(|(j, n)| j * n).sum::<i64>(), 26);
        // 按次数加权：Σkn/Σn = (3*3+4*23)/26 = 101/26 = 3.8846…
        assert!((average_code_len_by_commits(&codes) - 101.0 / 26.0).abs() < 1e-12);
        // 报表口径：分母是总字数（这里恰好也是 26）
        assert!((report_avg_code_len(&codes, &lengths) - 101.0 / 26.0).abs() < 1e-12);

        let ratio = char_length_ratio(&lengths);
        // 1 字词 2 个 → 2/26；2 字词 12 个 → 24/26；其他 0
        assert!((ratio.one_char - 2.0 / 26.0 * 100.0).abs() < 1e-9);
        assert!((ratio.two_char - 24.0 / 26.0 * 100.0).abs() < 1e-9);
        assert!(ratio.other.abs() < 1e-9);
    }

    #[test]
    fn average_speed_needs_more_than_one_second() {
        let mut bucket = Bucket::default();
        bucket.avg_gaps = vec![1.0];
        bucket.avg_cnts = vec![5.0];
        assert_eq!(average_speed(&bucket), 0.0, "不足 1 秒不计算");
        bucket.avg_gaps = vec![2.0];
        assert!((average_speed(&bucket) - 150.0).abs() < 1e-9);
    }

    #[test]
    fn utf8_char_count_counts_code_points() {
        assert_eq!(utf8_char_count("你好"), 2);
        assert_eq!(utf8_char_count("hello"), 5);
        assert_eq!(utf8_char_count(""), 0);
        assert_eq!(utf8_char_count("a你b"), 3);
    }
}
