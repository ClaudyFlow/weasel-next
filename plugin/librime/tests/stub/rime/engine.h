#pragma once
#include <rime/common.h>
namespace rime {
class Context;
class Schema;
class Engine {
 public:
  Schema* schema() const { return schema_.get(); }
  Context* context() const { return context_.get(); }
 private:
  an<Schema> schema_;
  an<Context> context_;
};
}  // namespace rime
