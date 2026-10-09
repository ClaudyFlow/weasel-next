// 语法检查用桩：仅覆盖本插件用到的 librime API，签名照抄 src/rime/*.h。
#pragma once
#include <iostream>
#include <memory>
#include <ostream>
#include <string>
#include <utility>
namespace rime {
using std::string;
template <class T> using an = std::shared_ptr<T>;
template <class T, class... Args> inline an<T> New(Args&&... args) {
  return std::make_shared<T>(std::forward<Args>(args)...);
}
// boost::signals2::connection 的最小等价物（本插件只用 connected()/disconnect()）。
class connection {
 public:
  bool connected() const { return connected_; }
  void disconnect() { connected_ = false; }
  void set_connected(bool value) { connected_ = value; }
 private:
  bool connected_ = true;
};
template <class Signature> class signal;
template <class R, class... Args> class signal<R(Args...)> {
 public:
  template <class F> connection connect(F&&) { return connection(); }
};
template <class T, class... Args> class Class {
 public:
  virtual ~Class() = default;
};
#define LOG(level) std::cerr
#define DLOG(level) std::cerr
}  // namespace rime

