#pragma once
#include <rime/candidate.h>
#include <rime/common.h>
namespace rime {
class Translation {
 public:
  virtual ~Translation() = default;
};
class UniqueTranslation : public Translation {
 public:
  explicit UniqueTranslation(an<Candidate> candidate) : candidate_(candidate) {}
 private:
  an<Candidate> candidate_;
};
}  // namespace rime
