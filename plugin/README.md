weasel-next 输入统计插件
======================

这是什么
--------
把输入统计（上屏次数、字数、速度、键触等）做成 librime 的翻译器插件，随输入法一起工作。
原先独立成 rime-speed-stats 仓库，现并入 weasel-next，作为单独插件目录维护。

两个子目录（拆开，不混在一起）
------------------------------
  core\       Rust 统计核心：staticlib + C ABI（capi.rs），被下面的 C++ 外壳调用
  librime\    C++ 外壳：librime 插件模块（input_stat_translator / input_stat_module）

为什么必须静态链入
------------------
Windows 版 librime 不加载外部 DLL 插件（plugins_module.cc 的 _WIN32 分支返回空路径），
因此插件必须编译进 rime.dll：由 librime\build-librime-mingw.ps1 注入 plugin 子目录 +
生成的 force_link.cc，并从 core\ 构建出的 Rust 静态库链接进去。

构建
----
  1. Rust 核心:  cargo build --release --manifest-path plugin\core\Cargo.toml --target x86_64-pc-windows-gnu
  2. 整个核心:  pwsh .\librime\build-librime-mingw.ps1 -Step all
  产物: artifact\smoke\rime.dll（自带统计插件）

与仓库其它部分的关系
--------------------
  librime\     —— 自用核心的构建脚本与补丁存档
  plugin\      —— 本插件的实现（本目录）
  server\      —— 语音/服务端逻辑；统计上报经它读回并落盘
