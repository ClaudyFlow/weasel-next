librime —— 本仓库自用的输入法核心
================================

为什么在这里
------------
weasel-rs 用的不是官方发布版 librime，而是从 master 自行构建、并带本地改动的核心。
构建脚本与补丁存档原本放在另一个仓库（rime-speed-stats），CI 无法复现；现移入本仓库根目录。

组成
----
  build-librime-mingw.ps1        构建脚本（llvm-mingw / libc++ / 静态链接）
  librime-local-patches.patch    本地改动存档（可 git apply 到干净的 librime 检出）

源码与版本
----------
  上游: https://github.com/rime/librime  分支 master
  当前: 7bc3fb0 (2026-10-01)  ci: add ubuntu arm runner (#1242)
  产物: artifact\smoke\rime.dll   8,420,352 字节（2026-10-08 构建）

包含的本地改进
--------------
  1. Prism 5.0 —— prism 增加 max_key_length，syllabifier 改为步进查找，
     长整句输入时不再按整串长度反复试探（配合 GetValue(string_view)）。
  2. 输入统计插件（input_stat）静态链入，Windows 不加载外部 DLL 插件。
  3. cmake/AddRCInfo.cmake 的 llvm-rc 分支修正（仅在 Clang 下走 llvm-rc）。
  4. 若干 canonicalize / reverse_lookup 分隔符修复。

工具链与依赖（Windows x64）
---------------------------
  编译器: llvm-mingw（x86_64-w64-windows-gnu, libc++），路径 C:\llvm-mingw
  依赖:   boost_regex（手工编译）、glog、leveldb、yaml-cpp、marisa-trie、opencc —— 全部静态
  说明:   Windows 版 librime 不加载外部插件，因此统计插件必须静态链接（脚本会生成 force_link.cc）

构建
----
  pwsh .\librime\build-librime-mingw.ps1
  产物落在 artifact\smoke\rime.dll；构建前请先停掉 weasel-broker/server/renderer。

注意
----
  脚本里的依赖缓存路径默认为 C:\code\deps-cache、工具链为 C:\llvm-mingw，
  在别的机器上需要按实际情况调整（或用环境变量覆盖）。
