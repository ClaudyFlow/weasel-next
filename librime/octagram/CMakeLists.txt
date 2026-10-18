# rime-octagram —— 语言模型（*.gram）语法打分插件
#
# 来源：https://github.com/lotem/librime-octagram （master，2026-10-18 抓取；LICENSE 见同目录）
# 它给 librime 注册一个名为 "grammar" 的组件：`script_translator` 的 Poet（组句器）
# 会通过 `Grammar::Require("grammar")` 找到它，按 schema 里的 `grammar/language`
# 加载 `<数据目录>/model/<language>.gram`，对候选/整句做 N-gram 加权调序。
#
# 本项目的用法（Windows 上 librime 不加载外部 DLL 插件，只能静态合并）：
#   build-librime-mingw.ps1 会把本目录同步到 <librime>/plugins/octagram，
#   librime 的 plugins/CMakeLists.txt 在 BUILD_MERGED_PLUGINS=ON 下把这里的 .o
#   直接并进 rime.dll，并把 "octagram" 写进 RIME_EXTRA_MODULES → 启动时 LoadModules
#   会自动加载它（模块组 "octagram" 内含 "grammar" 模块）。
#
# ★ 不要像 input_stat 那样用 add_subdirectory 从仓库路径注入：
#   librime 顶层 CMakeLists 会用 plugins/ 里发现的模块**覆盖** RIME_SETUP_EXTRA_MODULES，
#   从 -D 传进去的模块名会被顶掉，模块注册了却永远不会被 LoadModules 初始化。
cmake_minimum_required(VERSION 3.13)
project(rime-octagram CXX)

aux_source_directory(src octagram_src)

add_library(rime-octagram-objs OBJECT ${octagram_src})
set_target_properties(rime-octagram-objs PROPERTIES
  CXX_STANDARD 17
  CXX_STANDARD_REQUIRED ON)
if(BUILD_SHARED_LIBS)
  set_target_properties(rime-octagram-objs
    PROPERTIES
    POSITION_INDEPENDENT_CODE ON)
endif()

set(plugin_name rime-octagram PARENT_SCOPE)
set(plugin_objs $<TARGET_OBJECTS:rime-octagram-objs> PARENT_SCOPE)
set(plugin_deps ${rime_library} PARENT_SCOPE)
set(plugin_modules "octagram" PARENT_SCOPE)
