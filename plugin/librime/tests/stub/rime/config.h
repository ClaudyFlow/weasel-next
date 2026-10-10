#pragma once
#include <rime/common.h>
#include <vector>
namespace rime {
class ConfigValue;
class ConfigList;
class Config {
 public:
  bool GetString(const string& path, string* value) { return false; }
  bool GetInt(const string& path, int* value) { return false; }
  an<ConfigList> GetList(const string& path) { return nullptr; }
};
class ConfigValue {
 public:
  bool GetInt(int* value) { return false; }
};
class ConfigList {
 public:
  size_t size() const { return 0; }
  an<ConfigValue> GetValueAt(size_t index) const { return nullptr; }
};
}  // namespace rime
