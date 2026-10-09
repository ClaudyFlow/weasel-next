#pragma once
// 桩：只保留本插件用到的成员（真实头文件里的结构体字段顺序不影响语法检查）。
#include <cstddef>
#include <cstdint>
#define RIME_VERSION "1.17.0"
#define RIME_REGISTER_MODULE(name) \
  extern "C" void rime_require_module_##name(void) {}
typedef struct RimeApi {
  const char* (*get_user_data_dir)(void);
  void (*get_user_data_dir_s)(char* dir, size_t buffer_size);
} RimeApi;
RimeApi* rime_get_api(void);
