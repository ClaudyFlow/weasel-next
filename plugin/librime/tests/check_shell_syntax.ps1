# C++ 壳的语法检查（无需 MSVC / boost / librime 头文件）
#
# 用途：用 plugin/tests/stub 下按真实 librime 源码签名手写的桩头文件，对插件壳做
# `-fsyntax-only`，可以抓住自己代码里的语法/类型/ABI 调用错误（抓不到的是 MSVC ABI 链接问题）。
#
# 用法：powershell -ExecutionPolicy Bypass -File plugin/tests/check_shell_syntax.ps1
#   退出码 0 = 通过；非 0 = 有错误（同时打印错误行）。

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$clang = 'C:\Program Files\llvm-mingw\bin\clang++.exe'
if (-not (Test-Path $clang)) {
    $clang = (Get-Command clang++ -ErrorAction SilentlyContinue).Source
}
if (-not $clang) {
    Write-Error '找不到 clang++（可用 -DCLANG=... 指定）'
}

$arguments = @(
    '--target=x86_64-w64-windows-gnu', '-std=c++17', '-fsyntax-only', '-w',
    "-I$root\plugin\tests\stub",
    "-I$root\plugin\include",
    "-I$root\plugin\src",
    "$root\plugin\src\input_stat_translator.cc",
    "$root\plugin\src\input_stat_module.cc"
)
$output = & $clang @arguments 2>&1
$code = $LASTEXITCODE
if ($code -eq 0) {
    Write-Host 'C++ 壳语法检查通过（桩头文件模式）。'
} else {
    Write-Host "C++ 壳语法检查失败（exit=$code）："
    $output | ForEach-Object { Write-Host $_ }
}
exit $code
