#pragma once
#include <rime/common.h>
namespace rime {
class ComponentBase {
 public:
  virtual ~ComponentBase() = default;
};
template <class T> class Component : public ComponentBase {
 public:
  template <class... Args> explicit Component(Args&&... args) {}
};
class Registry {
 public:
  static Registry& instance();
  void Register(const string& name, ComponentBase* component);
};
}  // namespace rime
