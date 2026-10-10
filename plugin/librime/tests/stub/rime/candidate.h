#pragma once
#include <cstddef>
#include <rime/common.h>
namespace rime {
class Candidate {
 public:
  virtual ~Candidate() = default;
};
class SimpleCandidate : public Candidate {
 public:
  SimpleCandidate(const string& type, size_t start, size_t end,
                  const string& text, const string& comment) {}
};
}  // namespace rime
