//
// input_stat_translator.cc — 见头文件说明。
//

#include "input_stat_translator.h"

#include <cstdio>
#include <ctime>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <vector>

#include <rime/candidate.h>
#include <rime/config.h>
#include <rime/context.h>
#include <rime/dict/level_db.h>
#include <rime/engine.h>
#include <rime/schema.h>
#include <rime/segmentation.h>
#include <rime_api.h>

#if defined(__APPLE__)
#include <TargetConditionals.h>
#endif

namespace input_stat {

namespace {

int64_t NowSeconds() {
  return static_cast<int64_t>(::time(nullptr));
}

/// 可移植的 `timegm`：Windows（MSVC 与 mingw）用 `_mkgmtime`，其它平台用 `timegm`。
std::time_t UtcFromTm(std::tm* tm) {
#if defined(_WIN32)
  return ::_mkgmtime(tm);
#else
  return ::timegm(tm);
#endif
}

/// 换行统一为 CRLF（Lua 版 `summary:gsub("\n", "\r\n")`）。
std::string ToCrLf(const std::string& text) {
  std::string out;
  out.reserve(text.size() + 8);
  for (char ch : text) {
    if (ch == '\n') {
      out += "\r\n";
    } else {
      out += ch;
    }
  }
  return out;
}

/// 读取用户数据目录（librime C API；取不到时返回空串）。
std::string UserDataDir() {
  RimeApi* api = rime_get_api();
  if (api == nullptr || api->get_user_data_dir_s == nullptr) {
    return std::string();
  }
  char buffer[2048] = {0};
  api->get_user_data_dir_s(buffer, sizeof(buffer));
  return std::string(buffer);
}

/// 编译期版本号（librime 在 CMake 里定义 RIME_VERSION）。
const char* DistributionVersion() {
#if defined(RIME_VERSION)
  return RIME_VERSION;
#else
  return "unknown";
#endif
}

/// 平台 → 设备类型（规范 §7.3 的映射：📱 = android/ios，💻 = mac/windows/linux）。
///
/// Lua 版只能在运行期探测（脚本加载时不知道平台）；原生插件在**编译期**就能确定，
/// 因此这里用编译期分支，结果与探测等价且无误判空间。
const char* DefaultDeviceType() {
#if defined(__ANDROID__)
  return "\xf0\x9f\x93\xb1";  // 📱
#elif defined(__APPLE__) && TARGET_OS_IPHONE
  return "\xf0\x9f\x93\xb1";  // 📱
#else
  return "\xf0\x9f\x92\xbb";  // 💻
#endif
}

}  // namespace

// ── 进程级共享的统计状态 ──────────────────────────────────────────────────────
// librime 为每个会话/引擎各创建一个 translator 实例；统计状态必须是**进程级共享**的，
// 否则换一个窗口（新会话）就会从头计数。Lua 版依赖 librime-lua 的单一 Lua 状态天然共享，
// 原生插件必须自己共享。
//
// 只有单例首次创建时才从统计文件加载：之后的新会话不得用磁盘内容覆盖内存里的累积
// （否则每开一个窗口都会把这段时间的统计冲掉）。用递归锁是因为 Load/EnsureCurrent/
// SetReportContext 这些方法自身也会加锁，需要支持同线程重入。
#include <mutex>

namespace {

std::recursive_mutex g_stats_mutex;
RimeInputStats* g_stats_core = nullptr;
int g_stats_refs = 0;

}  // namespace

InputStatsTranslator::InputStatsTranslator(const rime::Ticket& ticket)
    : Translator(ticket) {
  {
    std::lock_guard<std::recursive_mutex> lock(g_stats_mutex);
    const bool first_instance = (g_stats_core == nullptr);
    if (first_instance) {
      g_stats_core = rime_input_stat_new();
    }
    core_ = g_stats_core;  // 先绑定：Load/EnsureCurrent 依赖它
    if (first_instance && core_ != nullptr) {
      Load();
      EnsureCurrent();
    }
    if (core_ != nullptr) {
      ++g_stats_refs;
    }
  }
  if (engine_ != nullptr && engine_->schema() != nullptr) {
    schema_id_ = engine_->schema()->schema_id();
  }
  SetReportContext();
  LoadQuotes();
  if (engine_ != nullptr && engine_->context() != nullptr) {
    commit_connection_ = engine_->context()->commit_notifier().connect(
        [this](rime::Context* context) { OnCommit(context); });
  }
  LOG(INFO) << "input_stat translator ready for schema '" << schema_id_ << "'.";
}

void InputStatsTranslator::SetReportContext() {
  std::lock_guard<std::recursive_mutex> lock(g_stats_mutex);
  if (core_ == nullptr) {
    return;
  }
  std::string software = "librime";
  std::string description;
  // 设备类型：默认按编译期平台（规范 §7.3），可由配置覆盖（Lua 版模块级 `deviceType`，§2.2）。
  std::string device = DefaultDeviceType();
  int boggle = -1;
  std::vector<int64_t> auto_commit;

  rime::Schema* schema = (engine_ != nullptr) ? engine_->schema() : nullptr;
  rime::Config* config = (schema != nullptr) ? schema->config() : nullptr;
  if (config != nullptr && !name_space_.empty()) {
    std::string value;
    if (config->GetString(name_space_ + "/software_name", &value)) {
      software = value;
    }
    value.clear();
    if (config->GetString(name_space_ + "/avg_code_len_desc", &value)) {
      description = value;
    }
    value.clear();
    if (config->GetString(name_space_ + "/device_type", &value) &&
        (value == "\xf0\x9f\x93\xb1" || value == "\xf0\x9f\x92\xbb")) {
      device = value;
    }
    int number = 0;
    if (config->GetInt(name_space_ + "/boggle_thd_s", &number) && number >= 0) {
      boggle = number;
    }
    // 自动顶屏码长集合（Lua `codeLenOfAutoCommit`，规范 §3.6）：缺省为 {0}。
    rime::an<rime::ConfigList> codes =
        config->GetList(name_space_ + "/code_len_of_auto_commit");
    if (codes) {
      for (size_t index = 0; index < codes->size(); ++index) {
        rime::an<rime::ConfigValue> item = codes->GetValueAt(index);
        int code = 0;
        if (item && item->GetInt(&code)) {
          auto_commit.push_back(code);
        }
      }
    }
  }
  if (boggle >= 0) {
    rime_input_stat_set_boggle_threshold(core_, boggle);
  }
  if (!auto_commit.empty()) {
    rime_input_stat_set_auto_commit(core_, auto_commit.data(), auto_commit.size());
  }

  const std::string version = DistributionVersion();
  rime_input_stat_set_report_context(
      core_, schema_id_.data(), schema_id_.size(), software.data(),
      software.size(), version.data(), version.size(), device.data(),
      device.size(), description.data(), description.size());
}

void InputStatsTranslator::LoadQuotes() {
  std::lock_guard<std::recursive_mutex> lock(g_stats_mutex);
  if (core_ == nullptr) {
    return;
  }
  std::string dir = UserDataDir();
  if (dir.empty()) {
    return;
  }
  std::filesystem::path path(dir);
  path /= "plugin";
  path /= "quote.txt";
  std::ifstream file(path, std::ios::binary);
  if (!file) {
    return;
  }
  // Lua 版：逐行读取、跳过空行、行首尾空白保留（规范 §7.6/§8 files_to_lines）。
  std::string line;
  while (std::getline(file, line)) {
    if (line.empty()) {
      continue;
    }
    if (!line.empty() && line.back() == '\r') {
      line.pop_back();
      if (line.empty()) {
        continue;
      }
    }
    rime_input_stat_add_quote(core_, line.data(), line.size());
  }
  LOG(INFO) << "input_stat: loaded " << rime_input_stat_quote_count(core_)
            << " quotes.";
}

InputStatsTranslator::~InputStatsTranslator() {
  if (commit_connection_.connected()) {
    commit_connection_.disconnect();
  }
  {
    std::lock_guard<std::recursive_mutex> lock(g_stats_mutex);
    if (core_ != nullptr) {
      core_ = nullptr;
      if (--g_stats_refs <= 0) {
        // 最后一个实例销毁时才释放进程级状态。
        g_stats_refs = 0;
        if (g_stats_core != nullptr) {
          rime_input_stat_free(g_stats_core);
          g_stats_core = nullptr;
        }
      }
    }
  }
}

std::string InputStatsTranslator::StatsFilePath() const {
  std::string dir = UserDataDir();
  if (dir.empty()) {
    return std::string();
  }
  std::filesystem::path path(dir);
  path /= "plugin";
  path /= std::string("input_stat_") + schema_id_ + ".lua";
  return path.string();
}

namespace {

/// 统计快照的 LevelDb 库目录：`<user_data>/userdb/input_stat.userdb`。
/// 与 userdb 归置补丁一致 —— 所有 `*.userdb` 都放在 `Rime\userdb\` 下。
std::string StatsDbPath(const std::string& stats_file_path) {
  std::filesystem::path p(stats_file_path);
  if (p.empty()) {
    return std::string();
  }
  std::filesystem::path dir = p.parent_path();  // StatsFilePath 所在目录 = <user_data>/plugin/
  std::error_code error;
  std::filesystem::create_directories(dir, error);
  return (dir / "input_stat.userdb").string();
}

/// 快照键：整个统计文本作为一条记录存进 LevelDb（不再使用 txt 文件）。
const char* const kSnapshotKey = "snapshot";

bool ReadSnapshot(const std::string& db_path, std::string* text) {
  if (db_path.empty()) {
    return false;
  }
  rime::LevelDb db(rime::path(db_path), "input_stat", "userdb");
  if (!db.Open()) {
    return false;
  }
  return db.Fetch(kSnapshotKey, text) && !text->empty();
}

bool WriteSnapshot(const std::string& db_path, const std::string& text) {
  if (db_path.empty()) {
    return false;
  }
  rime::LevelDb db(rime::path(db_path), "input_stat", "userdb");
  if (!db.Open()) {
    return false;
  }
  return db.Update(kSnapshotKey, text);
}

}  // namespace

void InputStatsTranslator::Load() {
  std::lock_guard<std::recursive_mutex> lock(g_stats_mutex);
  if (core_ == nullptr) {
    return;
  }
  const std::string path = StatsFilePath();
  if (path.empty()) {
    LOG(WARNING) << "input_stat: user data dir unavailable; statistics disabled.";
    return;
  }
  const std::string db_path = StatsDbPath(path);
  // 首选 LevelDb；不再读写 txt 文件。
  std::string text;
  if (ReadSnapshot(db_path, &text)) {
    if (rime_input_stat_load(core_, text.data(), text.size()) == 0) {
      LOG(WARNING) << "input_stat: failed to parse statistics snapshot from "
                   << db_path;
    }
    return;
  }
  // 迁移：旧版本的 txt 还在 → 读入 → 立刻写进 LevelDb → 旧文件改名留档（不删）。
  std::ifstream file(path, std::ios::binary);
  if (!file) {
    // 首次使用：与 Lua 版一致，保持初始 schema，由 EnsureCurrent() 完成迁移。
    DLOG(INFO) << "input_stat: no statistics snapshot yet (" << db_path << ")";
    return;
  }
  std::ostringstream buffer;
  buffer << file.rdbuf();
  const std::string legacy = buffer.str();
  if (legacy.empty()) {
    return;
  }
  if (rime_input_stat_load(core_, legacy.data(), legacy.size()) == 0) {
    LOG(WARNING) << "input_stat: failed to parse legacy " << path
                 << "; keeping in-memory state.";
    return;
  }
  char* snapshot = rime_input_stat_serialize(core_);
  if (snapshot != nullptr) {
    WriteSnapshot(db_path, std::string(snapshot));
    rime_input_stat_string_free(snapshot);
  }
  std::error_code error;
  std::filesystem::rename(path, path + ".migrated-leveldb", error);
  LOG(INFO) << "input_stat: migrated legacy statistics into " << db_path;
}

void InputStatsTranslator::EnsureCurrent() {
  std::lock_guard<std::recursive_mutex> lock(g_stats_mutex);
  if (core_ == nullptr) {
    return;
  }
  if (rime_input_stat_ensure_current(core_) != 0) {
    LOG(INFO) << "input_stat: statistics migrated to the current format version.";
  }
}

void InputStatsTranslator::Save() {
  std::lock_guard<std::recursive_mutex> lock(g_stats_mutex);
  if (core_ == nullptr) {
    return;
  }
  const std::string path = StatsFilePath();
  if (path.empty()) {
    return;
  }
  char* text = rime_input_stat_serialize(core_);
  if (text == nullptr) {
    return;
  }
  const std::string db_path = StatsDbPath(path);
  if (!WriteSnapshot(db_path, std::string(text))) {
    LOG(WARNING) << "input_stat: cannot write statistics snapshot to " << db_path;
  }
  rime_input_stat_string_free(text);
}

int InputStatsTranslator::LocalOffsetSeconds(int64_t now_unix) {
  const std::time_t now = static_cast<std::time_t>(now_unix);
  std::tm local = *::localtime(&now);
  std::tm utc_candidate = local;
  // `UtcFromTm(local 墙钟)` 得到的时刻，其 UTC 墙钟正好等于本地墙钟，即 `now + offset`；
  // 因此偏移必须用 `as_utc - now`。原先写成 `now - as_utc` 会把符号弄反（东八区得到
  // -08:00），既让报表的时间/时区标签错位，也会让日/周/月/年的分桶边界切错。
  const std::time_t as_utc = UtcFromTm(&utc_candidate);
  return static_cast<int>(static_cast<int64_t>(as_utc) - now_unix);
}

rime::an<rime::Translation> InputStatsTranslator::Query(
    const std::string& input, const rime::Segment& segment) {
  std::lock_guard<std::recursive_mutex> lock(g_stats_mutex);
  if (core_ == nullptr) {
    return nullptr;
  }
  const int64_t now = NowSeconds();
  const int offset = LocalOffsetSeconds(now);

  // 前置步骤：输入中断结算、击键计数、lastCode 维护。
  const int is_command =
      rime_input_stat_on_query(core_, input.data(), input.size(), now, offset);
  if (is_command == 0) {
    // 普通编码：Lua 版在此直接返回，不产生候选。
    last_input_ = input;
    return nullptr;
  }

  // 报表类命令先结算会话（§5.9）：可用则带样本，否则只做跨周期刷新。
  if (rime_input_stat_is_report_command(input.data(), input.size()) != 0) {
    const int settle = rime_input_stat_avg_available(core_);
    rime_input_stat_settle_session(core_, now, offset, settle);
  }

  // 报表风格：命令串末两位相同 → 长款（§5.0，跨命令保持、不落盘）。
  rime_input_stat_note_report_style(core_, input.data(), input.size());

  int needs_save = 0;
  char* text = rime_input_stat_run_command(core_, input.data(), input.size(), now,
                                            offset, &needs_save);
  if (text == nullptr) {
    // 未识别的 `/...` 输入：Lua 版同样不产出候选。
    return nullptr;
  }
  std::string summary(text);
  rime_input_stat_string_free(text);
  if (needs_save != 0) {
    Save();
  }
  if (summary.empty()) {
    // 已识别但本期还没有文本输出（报表排版仍在移植中）。
    return nullptr;
  }
  auto candidate = rime::New<rime::SimpleCandidate>(
      "stat", segment.start, segment.end, ToCrLf(summary), "");
  return rime::New<rime::UniqueTranslation>(candidate);
}

void InputStatsTranslator::OnCommit(rime::Context* context) {
  std::lock_guard<std::recursive_mutex> lock(g_stats_mutex);
  if (core_ == nullptr || context == nullptr) {
    return;
  }
  const std::string text = context->GetCommitText();
  // 提交时 ctx.input 可能已被清空，回退到最近一次编码（在 Query 里记录）。
  const std::string code = context->input().empty() ? last_input_ : context->input();
  last_input_.clear();

  // Lua commit 回调开头的守卫：空上屏 / 编码与上屏一致 / 命令输入 / 上屏含非汉字一律不统计，
  // 并清零击键计数。漏掉它的话，/rtj 产出的报表（上千字符）会被算成一次"输入上千字"的上屏，
  // 字数、均速与平均码长全部失真。
  const int disposition = rime_input_stat_classify_commit(
      code.data(), code.size(), text.data(), text.size());
  if (disposition == 1) {
    rime_input_stat_reset_key_touch(core_);
    return;
  }
  if (disposition == 2) {
    return;
  }
  if (text.empty()) {
    return;
  }
  const int64_t now = NowSeconds();
  const int offset = LocalOffsetSeconds(now);
  const int needs_save = rime_input_stat_on_commit(
      core_, text.data(), text.size(), static_cast<int64_t>(code.size()), now,
      offset);
  if (needs_save != 0) {
    Save();
  }
}

}  // namespace input_stat
