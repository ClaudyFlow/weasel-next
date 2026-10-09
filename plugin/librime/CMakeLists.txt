# rime-input-stat 插件（librime 模块 + Rust 核心）
#
# 两种用法：
#   1) 作为 librime 的子目录（推荐，Windows 上唯一可行）：
#        add_subdirectory(<本目录> ${CMAKE_BINARY_DIR}/rime-input-stat)
#        target_link_libraries(rime PRIVATE rime-input-stat)
#      并在配置 librime 时加 -DRIME_SETUP_EXTRA_MODULES=input_stat，
#      使 rime_api.cc 生成对 rime_require_module_input_stat() 的调用。
#   2) 独立配置做静态检查：需要显式提供 RIME_INCLUDE_DIR 与 Rust 静态库路径。
#
# Rust 核心（core/）需先生成静态库：
#   cargo build --release --manifest-path ../core/Cargo.toml --target x86_64-pc-windows-msvc
# 产物：core/target/x86_64-pc-windows-msvc/release/rime_input_stat_core.lib
#
cmake_minimum_required(VERSION 3.13)
project(rime-input-stat CXX)

set(RIME_INPUT_STAT_CORE_LIB "" CACHE FILEPATH
    "Rust 核心静态库（rime_input_stat_core.lib / librime_input_stat_core.a）")
if(NOT RIME_INPUT_STAT_CORE_LIB)
  message(FATAL_ERROR
    "请设置 -DRIME_INPUT_STAT_CORE_LIB=<Rust 静态库路径>（见本文件头部的构建说明）")
endif()

# 独立配置时需要；作为 librime 子目录时由 librime 提供同名包含目录。
if(NOT TARGET rime)
  set(RIME_INCLUDE_DIR "" CACHE PATH "librime 源码的 src/ 目录（包含 rime/*.h）")
  if(NOT RIME_INCLUDE_DIR)
    message(FATAL_ERROR "独立配置时请设置 -DRIME_INCLUDE_DIR=<librime>/src")
  endif()
endif()

add_library(rime_input_stat_core STATIC IMPORTED GLOBAL)
set_target_properties(rime_input_stat_core PROPERTIES
  IMPORTED_LOCATION "${RIME_INPUT_STAT_CORE_LIB}")

add_library(rime-input-stat STATIC
  src/input_stat_module.cc
  src/input_stat_translator.cc)

target_include_directories(rime-input-stat PUBLIC
  "${CMAKE_CURRENT_SOURCE_DIR}/include")

# Rust 核心（windows-gnu 的 std）需要这些系统库：文件系统用 NtOpenFile，管道用
# NtCreateNamedPipeFile，另有 bcrypt/ws2_32/userenv/COM 等。作为 INTERFACE 传播给最终
# 链接 rime.dll 的一方——插件本身是静态库，不在这里链接。
target_link_libraries(rime-input-stat INTERFACE
  ntdll ws2_32 userenv bcrypt ole32 oleaut32 uuid advapi32)

if(TARGET rime)
  # 作为 librime 的子目录时：**不要**链接 rime。librime 用的是 target_link_libraries 的 plain
  # 签名，而 rime → rime-input-stat → rime 会构成共享库与静态库之间的循环依赖（CMake 直接
  # 报错）。插件所需的 librime C++ 符号会在最终链接 rime.dll 时自然解析；头文件路径也由
  # 父目录的 include_directories 继承而来。
  target_link_libraries(rime-input-stat PRIVATE rime_input_stat_core)
else()
  target_include_directories(rime-input-stat PRIVATE "${RIME_INCLUDE_DIR}")
  target_link_libraries(rime-input-stat PRIVATE rime_input_stat_core)
endif()

# 让 C++ 侧与 Rust 侧的 ABI 头保持同步：该头文件被 C++ 与 C 冒烟测试共用。
set_target_properties(rime-input-stat PROPERTIES
  CXX_STANDARD 17
  CXX_STANDARD_REQUIRED ON)
