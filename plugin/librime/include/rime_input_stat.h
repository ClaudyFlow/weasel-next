/*
 * rime_input_stat.h — librime 原生插件（input_stat）所用 Rust 核心的 C ABI。
 *
 * 与 Rust 侧 `core/src/capi.rs` 一一对应：任何一侧新增/改名都必须同步另一侧。
 * 约定：
 *   * 句柄由 rime_input_stat_new 创建、rime_input_stat_free 释放，不得混用；
 *   * 返回的 char* 都由 Rust 分配（CString），必须用 rime_input_stat_string_free 释放；
 *   * 所有字符串都是 UTF-8；时间一律是 Unix 整数秒（规范 §0/§3.2）；
 *   * 函数不会抛出异常，失败以返回 0 / NULL / -1 表示。
 */

#ifndef RIME_INPUT_STAT_H_
#define RIME_INPUT_STAT_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** 不透明统计句柄。 */
typedef struct StatsHandle RimeInputStats;

/** 创建一个统计会话（内部已是 Lua 初始 schema：logVer = -1、皮肤索引 21）。 */
RimeInputStats* rime_input_stat_new(void);

/** 释放统计会话；传入 NULL 安全。 */
void rime_input_stat_free(RimeInputStats* handle);

/**
 * 设置“自动顶屏码长”集合（对应 Lua `codeLenOfAutoCommit`）。
 * 传入 NULL 或 len == 0 时恢复默认 `{0}`（等价于“总是 +1”）。
 */
void rime_input_stat_set_auto_commit(RimeInputStats* handle,
                                      const int64_t* codes,
                                      size_t len);

/** 设置卡壳门限（秒）；负值恢复默认 3。 */
void rime_input_stat_set_boggle_threshold(RimeInputStats* handle, int64_t seconds);

/**
 * 翻译器前置步骤（Lua `translator` 第 1–3 步）：输入中断结算、击键计数、`lastCode` 维护。
 *
 * @param input       当前编码（UTF-8，可为空串）
 * @param input_len   字节长度
 * @param now_unix    当前时间（Unix 秒）
 * @param tz_offset_seconds 本地时区相对 UTC 的秒数（东八区为 28800）
 * @return 1 表示输入以 '/' 开头，宿主应继续做命令分发；0 表示普通输入。
 */
int rime_input_stat_on_query(RimeInputStats* handle,
                              const char* input,
                              size_t input_len,
                              int64_t now_unix,
                              int32_t tz_offset_seconds);

/** 当前是否值得结算平均分速（Lua `avgAvailable`）：1 是、0 否。 */
int rime_input_stat_avg_available(const RimeInputStats* handle);

/**
 * 记录一次上屏（Lua commit 回调）。
 *
 * @param commit_text 上屏文本（UTF-8）
 * @param text_len    字节长度；为 0 时不做任何累计
 * @param raw_code_len `string.len(ctx.input)`，即**不含**顶屏空格的编码长度
 * @return 1 表示宿主应立即落盘（Lua 语义：第 21/42/63… 次上屏）
 */
int rime_input_stat_on_commit(RimeInputStats* handle,
                               const char* commit_text,
                               size_t text_len,
                               int64_t raw_code_len,
                               int64_t now_unix,
                               int32_t tz_offset_seconds);

/**
 * 用统计文件内容初始化状态，并按 `logVer` 做版本迁移（§4.4，迁移是破坏性的）。
 *
 * @return 1 成功（可能发生迁移）、0 解析失败（状态保持不变）。
 */
int rime_input_stat_load(RimeInputStats* handle, const char* text, size_t text_len);

/**
 * 判定一次上屏是否计入统计（对应 Lua commit 回调开头的四道守卫）。
 *
 * @param input       提交时的编码串（`ctx.input`），可为 NULL
 * @param commit_text 上屏文本，可为 NULL
 * @return `0` = 正常统计；
 *         `1` = 不统计并清零击键计数（空上屏 / 编码与上屏一致 / 命令输入 / 上屏含非汉字）；
 *         `2` = 不统计但保留击键计数（纯标点或数字；Lua 版里这条因判定顺序而不可达，本实现
 *               如实保留）。
 *
 * 必须调用：否则统计报表自身（`/rtj` 产出的长文本）也会被算成一次上屏，
 * 字数、均速与平均码长都会失真。
 */
int rime_input_stat_classify_commit(const char* input,
                                     size_t input_len,
                                     const char* commit_text,
                                     size_t text_len);

/** 清零击键计数（Lua `keyTouchCnt = 0`），在放弃统计本次上屏时调用。 */
void rime_input_stat_reset_key_touch(RimeInputStats* handle);

/**
 * 设置报表上下文（方案名、发行版名/版本、设备类型、平均码长说明）。
 *
 * 设备类型只接受 `📱` / `💻` / 空串；其余取值按识别失败处理（报表命令会拒绝执行）。
 * 报表风格不在这里设置，由 rime_input_stat_note_report_style 按命令串推导。
 * `avg_code_len_desc` 可为 NULL。
 */
void rime_input_stat_set_report_context(RimeInputStats* handle,
                                         const char* schema_name,
                                         size_t schema_len,
                                         const char* software_name,
                                         size_t software_len,
                                         const char* software_version,
                                         size_t version_len,
                                         const char* device_type,
                                         size_t device_len,
                                         const char* avg_code_len_desc,
                                         size_t desc_len);

/** 追加一条名言（宿主读取 quote.txt 后逐行注入，空行应跳过）。 */
void rime_input_stat_add_quote(RimeInputStats* handle,
                                const char* text,
                                size_t len);

/** 已加载的名言条数。 */
int64_t rime_input_stat_quote_count(const RimeInputStats* handle);

/**
 * 按 Lua 语义记录报表风格（规范 §5.0）：输入长度 > 1 时末两位相同 → 长款。
 *
 * 宿主应在每次命令输入时调用；该状态跨命令保持、不落盘。
 */
void rime_input_stat_note_report_style(RimeInputStats* handle,
                                        const char* input,
                                        size_t input_len);

/**
 * 执行一条以 '/' 开头的命令（规范 §5、§6）。
 *
 * @param now_unix           当前时间（Unix 秒），用于报表时间戳与名言随机种子
 * @param tz_offset_seconds  本地时区偏移（秒）
 * @param out_save 可空；非空时写入 1 表示需要立即落盘
 * @return 回显文本（需 free），或 NULL 表示未识别（宿主当普通输入处理）。
 */
char* rime_input_stat_run_command(RimeInputStats* handle,
                                   const char* input,
                                   size_t input_len,
                                   int64_t now_unix,
                                   int32_t tz_offset_seconds,
                                   int* out_save);

/** 序列化当前统计（与 Lua 版同语法，含结尾换行）；需用 string_free 释放。 */
char* rime_input_stat_serialize(const RimeInputStats* handle);

/** 释放由本库返回的字符串；传入 NULL 安全。 */
void rime_input_stat_string_free(char* text);

/**
 * 结算当前会话（Lua `update_stats(0, 0, 0, avgAvailable)`）。
 *
 * @param settle 非 0 时推进 avgGaps/avgCnts/avgCodeLen；为 0 时只做跨周期刷新。
 */
void rime_input_stat_settle_session(RimeInputStats* handle,
                                     int64_t now_unix,
                                     int32_t tz_offset_seconds,
                                     int settle);

/**
 * 按当前落盘版本做迁移（§4.4，破坏性重置，保留皮肤索引与设备类型）。
 *
 * 统计文件不存在时宿主必须调用它，否则 logVer 会一直停在 -1 并被写回文件。
 *
 * @return 1 表示发生了重置，0 表示版本已达标。
 */
int rime_input_stat_ensure_current(RimeInputStats* handle);

/** 判定输入是否为报表类命令（需要先结算会话）：1 是、0 否。 */
int rime_input_stat_is_report_command(const char* input, size_t input_len);

/** `rime_input_stat_field` 的字段编号。 */
enum RimeInputStatsField {
  RIME_INPUT_STAT_FIELD_DAILY_COUNT = 0,
  RIME_INPUT_STAT_FIELD_DAILY_LENGTH = 1,
  RIME_INPUT_STAT_FIELD_DAILY_MAX = 2,
  RIME_INPUT_STAT_FIELD_DAILY_KEY_TOUCH = 3,
  RIME_INPUT_STAT_FIELD_NEW_WORDS = 4,
  RIME_INPUT_STAT_FIELD_SKIN_WORD = 5,
  RIME_INPUT_STAT_FIELD_SKIN_CODE = 6,
  RIME_INPUT_STAT_FIELD_LOG_VERSION = 7
};

/** 读取汇总字段；未知字段返回 -1。 */
int64_t rime_input_stat_field(const RimeInputStats* handle, int field);

/** 返回静态版本字符串（无需释放）。 */
const char* rime_input_stat_version(void);

#ifdef __cplusplus
}  /* extern "C" */
#endif

#endif /* RIME_INPUT_STAT_H_ */
