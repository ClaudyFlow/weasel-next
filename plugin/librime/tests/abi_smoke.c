/*
 * abi_smoke.c — Rust 核心 C ABI 的冒烟测试。
 *
 * 用途：在**不依赖 librime/MSVC** 的前提下验证 `plugin/include/rime_input_stat.h`
 * 与 Rust `core/src/capi.rs` 的 ABI 完全一致（编译 + 链接 + 运行）。
 *
 * 构建（本机 llvm-mingw + rust windows-gnu）：
 *   cargo build --release --manifest-path ../core/Cargo.toml --target x86_64-pc-windows-gnu
 *   clang --target=x86_64-w64-windows-gnu -I ../include abi_smoke.c \
 *         ../core/target/x86_64-pc-windows-gnu/release/librime_input_stat_core.a \
 *         -lws2_32 -luserenv -lbcrypt -lntdll -lole32 -loleaut32 -luuid -ladvapi32 -o abi_smoke.exe
 *
 * 退出码 0 = 全部断言通过。
 */

#include <stdio.h>
#include <string.h>

#include "rime_input_stat.h"

static int failures = 0;

static void expect_int(const char* what, int64_t got, int64_t want) {
  if (got != want) {
    printf("FAIL %s: got %lld, want %lld\n", what, (long long)got, (long long)want);
    failures++;
  } else {
    printf("ok   %s = %lld\n", what, (long long)got);
  }
}

static void expect_str(const char* what, const char* got, const char* want) {
  if (got == NULL) {
    printf("FAIL %s: got NULL, want \"%s\"\n", what, want);
    failures++;
    return;
  }
  if (strcmp(got, want) != 0) {
    printf("FAIL %s: got \"%s\", want \"%s\"\n", what, got, want);
    failures++;
  } else {
    printf("ok   %s = \"%s\"\n", what, got);
  }
}

int main(void) {
  printf("core version: %s\n", rime_input_stat_version());

  RimeInputStats* stats = rime_input_stat_new();
  if (stats == NULL) {
    printf("FAIL rime_input_stat_new returned NULL\n");
    return 1;
  }

  /* 初始 schema：logVer = -1（触发迁移）、皮肤索引 21 */
  expect_int("initial logVer", rime_input_stat_field(stats, RIME_INPUT_STAT_FIELD_LOG_VERSION), -1);
  expect_int("initial skin(word)", rime_input_stat_field(stats, RIME_INPUT_STAT_FIELD_SKIN_WORD), 21);

  /* 命令输入：不计数、返回 1 */
  expect_int("query /rtj returns command",
             rime_input_stat_on_query(stats, "/rtj", 4, 1000, 28800), 1);

  /* 三次编码变化 = 3 次击键；上屏 2 字，原始码长 3 → 码长 4（默认集合 {0} 总是 +1） */
  expect_int("query n", rime_input_stat_on_query(stats, "n", 1, 1001, 28800), 0);
  expect_int("query ni", rime_input_stat_on_query(stats, "ni", 2, 1002, 28800), 0);
  expect_int("query nih", rime_input_stat_on_query(stats, "nih", 3, 1003, 28800), 0);
  expect_int("commit returns no-save",
             rime_input_stat_on_commit(stats, "\xe4\xbd\xa0\xe5\xa5\xbd", 6, 3, 1004, 28800), 0);

  expect_int("daily count", rime_input_stat_field(stats, RIME_INPUT_STAT_FIELD_DAILY_COUNT), 1);
  expect_int("daily length", rime_input_stat_field(stats, RIME_INPUT_STAT_FIELD_DAILY_LENGTH), 2);
  expect_int("daily keyTouch", rime_input_stat_field(stats, RIME_INPUT_STAT_FIELD_DAILY_KEY_TOUCH), 3);
  expect_int("daily max", rime_input_stat_field(stats, RIME_INPUT_STAT_FIELD_DAILY_MAX), 2);

  /* 序列化 → 解析（含迁移） */
  char* text = rime_input_stat_serialize(stats);
  if (text == NULL) {
    printf("FAIL serialize returned NULL\n");
    return 1;
  }
  printf("serialized head: %.40s...\n", text);
  expect_int("load returns ok", rime_input_stat_load(stats, text, strlen(text)), 1);
  expect_int("logVer after load", rime_input_stat_field(stats, RIME_INPUT_STAT_FIELD_LOG_VERSION), 4);
  expect_int("history reset by migration",
             rime_input_stat_field(stats, RIME_INPUT_STAT_FIELD_DAILY_COUNT), 0);
  rime_input_stat_string_free(text);

  /* 报表上下文：方案名/发行版/设备类型/风格 */
  rime_input_stat_set_report_context(stats, "luna_pinyin", 11, "Weasel", 6, "1.0", 3,
                                      "\xf0\x9f\x92\xbb", 4, NULL, 0);
  rime_input_stat_add_quote(stats, "\xe5\x90\x8d\xe8\xa8\x80", 6);
  expect_int("quote count", rime_input_stat_quote_count(stats), 1);

  /* 报表命令：无记录时给出空桶提示（短款、需要已知设备类型） */
  int save = 0;
  char* echo = rime_input_stat_run_command(stats, "/rtj", 4, 1004, 28800, &save);
  expect_str("empty daily report", echo,
             "\xe2\x80\xbb \xe4\xbb\x8a\xe5\xa4\xa9\xe6\xb2\xa1\xe6\x9c\x89\xe4\xbb\xbb\xe4\xbd\x95\xe8\xae\xb0\xe5\xbd\x95\xe3\x80\x82");
  expect_int("report does not save", save, 0);
  rime_input_stat_string_free(echo);

  /* 风格判定：末两位相同 → 长款（规范 §5.0） */
  rime_input_stat_note_report_style(stats, "/rtjj", 5);

  /* 皮肤清单 */
  echo = rime_input_stat_run_command(stats, "/pf", 3, 1004, 28800, &save);
  if (echo == NULL || strncmp(echo, "\xe2\x80\xbb \xe5\x8f\xaf\xe7\x94\xa8\xe7\x9a\xae\xe8\x82\xa4", 12) != 0) {
    printf("FAIL skin list head: %s\n", echo ? echo : "(null)");
    failures++;
  } else {
    printf("ok   skin list rendered\n");
  }
  if (echo != NULL) {
    rime_input_stat_string_free(echo);
  }

  /* 皮肤切换回显（附录 A 精确文案） */
  echo = rime_input_stat_run_command(stats, "//6011", 6, 1004, 28800, &save);
  expect_str("skin switch echo", echo, "\xe2\x80\xbb \xe5\xad\x97\xe8\xaf\x8d\xe7\xbb\x9f\xe8\xae\xa1\xe7\x9a\xae\xe8\x82\xa4\xe5\xb7\xb2\xe5\x88\x87\xe6\x8d\xa2\xe8\x87\xb3\xef\xbc\x9a//6011 \xe2\x96\x89\xe2\x96\x81");
  expect_int("skin switch wants save", save, 1);
  expect_int("skin index recorded", rime_input_stat_field(stats, RIME_INPUT_STAT_FIELD_SKIN_WORD), 1);
  rime_input_stat_string_free(echo);

  /* 清空命令 */
  echo = rime_input_stat_run_command(stats, "//09", 4, 1004, 28800, &save);
  expect_str("clear all echo", echo, "\xe2\x80\xbb \xe6\x89\x80\xe6\x9c\x89\xe7\xbb\x9f\xe8\xae\xa1\xe6\x95\xb0\xe6\x8d\xae\xe5\xb7\xb2\xe6\xb8\x85\xe7\xa9\xba\xe3\x80\x82");
  expect_int("clear all wants save", save, 1);
  rime_input_stat_string_free(echo);

  /* 未识别输入 → NULL */
  echo = rime_input_stat_run_command(stats, "nihao", 5, 1004, 28800, &save);
  expect_int("unknown command returns NULL", echo == NULL ? 1 : 0, 1);
  if (echo != NULL) {
    rime_input_stat_string_free(echo);
  }

  /* 报表命令判定 + 会话结算 + 版本迁移（本轮新增的 ABI） */
  expect_int("is_report_command(/rtj)",
             rime_input_stat_is_report_command("/rtj", 4), 1);
  expect_int("is_report_command(/qcjs)",
             rime_input_stat_is_report_command("/qcjs", 5), 0);
  rime_input_stat_settle_session(stats, 2000, 28800, 0);
  expect_int("ensure_current is idempotent",
             rime_input_stat_ensure_current(stats), 0);

  rime_input_stat_free(stats);
  rime_input_stat_free(NULL); /* 必须安全 */

  printf(failures == 0 ? "\nALL OK\n" : "\n%d FAILURES\n", failures);
  return failures == 0 ? 0 : 1;
}
