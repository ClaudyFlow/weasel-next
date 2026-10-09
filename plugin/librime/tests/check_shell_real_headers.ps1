# 用**真实的 librime / boost 头文件**对插件 C++ 壳做语法检查（不需要 MSVC，也不需要编译 librime）
#
# 与 check_shell_syntax.ps1 的区别：
#   * check_shell_syntax.ps1 用 plugin/tests/stub 下手写的桩头，离线可用，只验证"自己代码的语法"；
#   * 本脚本用 librime 源码里的真实头 + 真实 boost 头，能验证"对 librime API 的每条用法"是否正确。
#
# 前置（都能联网取到）：
#   1. librime 源码：默认 C:\code\librime（含 src/rime_api.h 等）；
#   2. boost 头：默认 C:\code\deps-cache\boost_1_89_0（可从 archives.boost.io 下 boost 源码包解压，
#      只需要其中的 boost/ 头，不必编译）；
#   3. glog 头：librime 的子模块 —— 在 librime 目录执行 `git submodule update --init deps/glog`。
#
# 说明：librime 的 common.h 会 include 由 CMake 生成的 rime/build_config.h，glog 的公开头会用到
# 它自己 CMake 生成的 glog/export.h 宏（GLOG_EXPORT 等）。本脚本按模板生成等价的 build_config.h，
# 并把 GLOG_EXPORT/GLOG_NO_EXPORT/GLOG_DEPRECATED 定义为空（静态链接时它们本来就是空宏），
# 因此无需真正配置 glog 的 CMake。真实构建时这两者都由各自 CMake 生成。
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File plugin/tests/check_shell_real_headers.ps1
#   退出码 0 = 通过。

[CmdletBinding()]
param(
    [string]$Librime = 'C:\code\librime',
    [string]$BoostInclude = 'C:\code\deps-cache\boost_1_89_0',
    [string]$Cache = 'C:\code\deps-cache\rime-gen',
    [string]$RimeVersion = '1.17.0',
    [string]$Clang = 'C:\Program Files\llvm-mingw\bin\clang++.exe'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)   # 仓库根
$plugin = Join-Path $root 'plugin'

if (-not (Test-Path -LiteralPath $Clang)) {
    $found = Get-Command clang++ -ErrorAction SilentlyContinue
    if (-not $found) { throw "找不到 clang++：$Clang（可用 -Clang 指定）" }
    $Clang = $found.Source
}
if (-not (Test-Path -LiteralPath (Join-Path $Librime 'src\rime_api.h'))) {
    throw "在 $Librime 下找不到 src/rime_api.h，请用 -Librime 指定 librime 源码目录"
}
if (-not (Test-Path -LiteralPath (Join-Path $BoostInclude 'boost\signals2.hpp'))) {
    throw @"
在 $BoostInclude 下找不到 boost 头（boost/signals2.hpp）。
获取方式（约 180 MB）：
  Invoke-WebRequest https://archives.boost.io/release/1.89.0/source/boost_1_89_0.tar.gz -OutFile boost.tar.gz
  tar -xzf boost.tar.gz -C <目录>
或使用 -BoostInclude 指向已有的 boost 头目录。
"@
}
$glog = Join-Path $Librime 'deps\glog\src'
if (-not (Test-Path -LiteralPath (Join-Path $glog 'glog\logging.h'))) {
    throw "找不到 glog 头（$glog）。请在 librime 目录执行：git submodule update --init deps/glog"
}

# 1) 生成 librime 的 build_config.h（按 src/rime/build_config.h.in 的等价内容）
$generated = Join-Path $Cache 'rime'
New-Item -ItemType Directory -Force -Path $generated | Out-Null
$buildConfig = Join-Path $generated 'build_config.h'
@"
// 由 plugin/tests/check_shell_real_headers.ps1 生成，等价于 librime 的 CMake 产物。
#ifndef RIME_BUILD_CONFIG_H_
#define RIME_BUILD_CONFIG_H_

#define RIME_ENABLE_LOGGING
#define RIME_DATA_DIR "/usr/share/rime-data"

#endif  // RIME_BUILD_CONFIG_H_
"@ | Set-Content -LiteralPath $buildConfig -Encoding UTF8
Write-Host "已生成 $buildConfig"

# 2) 真实头文件语法检查
#    * 第三方头用 -isystem，只保留本插件代码自己的告警；
#    * -Wno-missing-field-initializers：librime 的 RIME_REGISTER_MODULE 宏展开成
#      `static RimeModule module = {0};`，对多字段结构体必然触发该告警，与本插件无关；
#    * RIME_VERSION 必须带引号传给宏（librime 的 CMake 就是 -DRIME_VERSION="1.17.0"）；
#      PowerShell 会吞掉普通引号，所以这里用 \" 转义。
$arguments = @(
    '--target=x86_64-w64-windows-gnu', '-std=c++17', '-fsyntax-only', '-Wall', '-Wextra',
    '-Wno-missing-field-initializers',
    "-DRIME_VERSION=\`"$RimeVersion\`"",
    '-DGLOG_EXPORT=', '-DGLOG_NO_EXPORT=', '-DGLOG_DEPRECATED=',
    '-isystem', $Cache,
    '-isystem', (Join-Path $Librime 'src'),
    '-isystem', $BoostInclude,
    '-isystem', $glog,
    '-I', (Join-Path $plugin 'include'),
    (Join-Path $plugin 'src\input_stat_translator.cc'),
    (Join-Path $plugin 'src\input_stat_module.cc')
)
# 原生程序的 stderr 在 PowerShell 里会被当成错误记录，这里临时放开以免中断捕获。
$previous = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$output = & $Clang @arguments 2>&1
$code = $LASTEXITCODE
$ErrorActionPreference = $previous
if ($code -eq 0) {
    Write-Host 'C++ 壳对真实 librime/boost 头文件的语法检查通过。' -ForegroundColor Green
    if ($output) { $output | ForEach-Object { Write-Host $_ } }   # 只可能是本插件代码的告警
} else {
    Write-Host "语法检查失败（exit=$code）：" -ForegroundColor Red
    $output | ForEach-Object { Write-Host $_ }
}
exit $code
