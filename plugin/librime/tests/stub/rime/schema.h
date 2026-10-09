#pragma once
#include <rime/common.h>
namespace rime {
class Config;
class Schema {
 public:
  const string& schema_id() const { return schema_id_; }
  Config* config() const { return config_.get(); }
 private:
  string schema_id_;
  an<Config> config_;
};
}  // namespace rime
