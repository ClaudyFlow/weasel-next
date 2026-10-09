//! Rime 输入统计核心（Rust）。
//!
//! 由 `rime-lua/lua/input_statistics.lua`（GPL-3.0）的全功能合并而来，供 librime 原生插件使用：
//! C++ 模块壳通过 [`capi`] 暴露的 C ABI 调用本 crate，编译产物为 `staticlib`（链入模块）
//! 或 `cdylib`（直接做动态库）。
//!
//! 分层：
//! * [`state`]：纯计算的状态模型与累积算法（无 I/O、无系统时间，便于测试）；
//! * [`commit_gate`]：上屏准入判定（Lua commit 回调开头的四道守卫）；
//! * [`persist`]：与 Lua 版同语法的统计文件读写；
//! * [`time`]：本地时区的日/周/月/年边界；
//! * [`report`]：日报/周报/月报/年报与生字词报表的文本排版；
//! * [`capi`]：给 C++ 模块壳用的 C ABI。

pub mod capi;
pub mod command;
pub mod commit_gate;
pub mod persist;
pub mod report;
pub mod state;
pub mod time;

pub use command::{
    recognize, run_command, Command, CommandContext, CommandOutcome, ReportPeriod, SkinKind,
    SKIN_COUNT,
};
pub use commit_gate::{
    classify_commit, is_all_chinese, is_chinese_char, CommitDisposition,
};
pub use persist::{file_name, parse, serialize, ParseError};
pub use report::{
    embed_text_into_splitor, format_new_words, format_report, format_skin_list, progress_bar,
    skin_at, splitor, statistics_cal, DeviceType, ReportInput, ReportKind, Skin, Statistics, Style,
    SCRIPT_LINE_PREFIX, SCRIPT_VERSION, SKINS, SPLITOR_CHAR, SPLITOR_LEN,
};
pub use state::{
    adjusted_code_len, average_code_len_by_commits, average_speed, avg_available,
    char_length_ratio, code_length_weighted_sum, count_key_press, observe_new_word,
    report_avg_code_len, session_available, should_interrupt, update_stats, utf8_char_count,
    AvgSpdInfo, Bucket, BucketBounds, CharLengthRatio, Commit, Fastest, InputStats, NewWords,
    DEFAULT_BOGGLE_THRESHOLD_SECONDS, DEFAULT_SKIN_INDEX, FASTEST_WINDOW_SECONDS,
    INTERRUPT_GAP_SECONDS, LOG_VERSION,
};
pub use time::{bucket_bounds, timezone_label};
