//
// input_stat_translator.h — librime 原生输入统计 translator。
//
// 职责（全部业务逻辑在 Rust 核心，见 core/src/capi.rs）：
//   * 构造：创建核心句柄、读取统计文件、连接会话的 commit 通知；
//   * Query：把每次编码变化交给核心做前置处理（中断结算/击键计数），命令输入再走命令分发；
//   * OnCommit：把上屏文本与码长交给核心累计，必要时落盘；
//   * 析构：断开通知并释放核心句柄。
//
// 时间一律使用 Unix 整数秒（`time(nullptr)`），时区偏移按当前瞬间计算（含夏令时），
// 与 Lua 版 `os.date`/`os.time` 的本地语义一致；跨夏令时切换瞬间的极端情况见 README。
//

#ifndef RIME_INPUT_STAT_TRANSLATOR_H_
#define RIME_INPUT_STAT_TRANSLATOR_H_

#include <cstdint>
#include <string>

#include <rime/common.h>
#include <rime/context.h>
#include <rime/translation.h>
#include <rime/translator.h>

#include "rime_input_stat.h"

namespace input_stat {

class InputStatsTranslator : public rime::Translator {
 public:
  explicit InputStatsTranslator(const rime::Ticket& ticket);
  ~InputStatsTranslator() override;

  rime::an<rime::Translation> Query(const std::string& input,
                                    const rime::Segment& segment) override;

 private:
  /// 上屏回调：累计统计并在需要时落盘。
  void OnCommit(rime::Context* context);
  /// 读取本方案的统计文件（不存在时按初始 schema 处理）。
  void Load();
  /// 版本迁移（统计文件缺失时也必须调用）。
  void EnsureCurrent();
  /// 写回本方案的统计文件。
  void Save();
  /// 把方案名/发行版/设备类型/配置项推给核心（报表渲染需要）。
  void SetReportContext();
  /// 读取 `<user_data>/plugin/quote.txt` 并把非空行注入核心（名言尾部用）。
  void LoadQuotes();
  /// 返回统计文件路径（`<user_data>/plugin/input_stat_<schema>.lua`）。
  std::string StatsFilePath() const;
  /// 当前本地时区偏移（秒），取当前瞬间的偏移。
  static int LocalOffsetSeconds(int64_t now_unix);

  /// Rust 核心句柄；构造失败时为空指针，此后所有操作退化为空操作。
  RimeInputStats* core_ = nullptr;
  /// 本方案 id（来自 `engine->schema()->schema_id()`）。
  std::string schema_id_;
  /// 最近一次编码串；commit 回调里若 `context->input()` 已清空则回退到它。
  std::string last_input_;
  /// commit 通知连接；析构时断开。
  rime::connection commit_connection_;
};

}  // namespace input_stat

#endif  // RIME_INPUT_STAT_TRANSLATOR_H_
