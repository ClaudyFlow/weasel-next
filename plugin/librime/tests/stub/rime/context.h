#pragma once
#include <rime/common.h>
namespace rime {
class Context;
using Notifier = signal<void(Context*)>;
class Context {
 public:
  Notifier& commit_notifier() { return commit_notifier_; }
  const string& input() const { return input_; }
  const string& GetCommitText() const { return commit_text_; }
 private:
  Notifier commit_notifier_;
  string input_;
  string commit_text_;
};
}  // namespace rime
