#pragma once
#include <rime/common.h>
namespace rime {
class Engine;
struct Ticket {
  Engine* engine = nullptr;
  string name_space;
};
}  // namespace rime
