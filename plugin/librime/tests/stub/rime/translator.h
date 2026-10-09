#pragma once
#include <rime/common.h>
#include <rime/ticket.h>
#include <rime/translation.h>
namespace rime {
struct Segment;
class Translator : public Class<Translator, const Ticket&> {
 public:
  explicit Translator(const Ticket& ticket)
      : engine_(ticket.engine), name_space_(ticket.name_space) {}
  virtual ~Translator() = default;
  virtual an<Translation> Query(const string& input, const Segment& segment) = 0;
  string name_space() const { return name_space_; }
 protected:
  Engine* engine_;
  string name_space_;
};
}  // namespace rime
