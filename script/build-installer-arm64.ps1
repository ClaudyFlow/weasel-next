#Requires -Version 5.1
<#
.SYNOPSIS
    打 ARM64（Windows on ARM）安装包。

.DESCRIPTION
    矩阵（用户 2026-10-28 定）：
      * x64  : 只出 **64 位安装包**；装 x64 TIP + x86 TIP（覆盖 64 位 / 32 位宿主）——由 build-installer.ps1 产出。
      * ARM64: 只出 **64 位安装包**；装 arm64 TIP + x86 TIP（覆盖 64 位原生 / 32 位仿真宿主）——本脚本产出。

    流程：
      1. cargo build --release --target aarch64-pc-windows-gnullvm --workspace
      2. build-tip32.ps1（x86 TIP，和 ARM 包共用）
      3. build-installer.ps1（顺带产出 x64 安装包 + 刷新 payload-files.nsh）
      4. 把 arm64 产物 + x86 TIP + arm64 运行库摆进 target\arm64-release
      5. makensis /DTARGET_ARM64 打 -arm64-setup.exe

    前置：rustup target add aarch64-pc-windows-gnullvm（本机已装）；llvm-mingw 在 C:\Program Files\llvm-mingw。
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$SkipX64
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$mingw = 'C:\Program Files\llvm-mingw'
$target = 'aarch64-pc-windows-gnullvm'
$rel = Join-Path $root "target\$target\release"
$stage = Join-Path $root 'target\arm64-release'
# 2026-11-02 与 x64 对齐：安装包统一放仓库根 `install\`（原 `artifact\installer` 已弃用）。
$outputDirectory = Join-Path $root 'install'

$env:LIBCLANG_PATH = Join-Path $mingw 'bin'
$env:PATH = (Join-Path $mingw 'bin') + ';' + $env:PATH

function Step($text) { Write-Host ('== ' + $text) -ForegroundColor Cyan }

if (-not (Test-Path -LiteralPath (Join-Path $rel 'Weasel-Next-Setting.exe')) -and $SkipBuild) {
    throw "找不到 ARM64 产物（$rel）。去掉 -SkipBuild 先编一次。"
}

Step "编译 ARM64 release（$target）"
if (-not $SkipBuild) {
    Push-Location $root
    try {
        & cargo build --release --target $target --workspace
        if ($LASTEXITCODE -ne 0) { throw 'ARM64 编译失败' }
    } finally { Pop-Location }
} else { Write-Host '   跳过（-SkipBuild）' }

Step '编 x86 TIP（32 位宿主用）'
& (Join-Path $PSScriptRoot 'build-tip32.ps1')

if (-not $SkipX64) {
    Step '打 x64 安装包（顺带刷新 payload-files.nsh）'
    & (Join-Path $PSScriptRoot 'build-installer.ps1')
}

Step '组装 arm64 发布目录'
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
$null = New-Item -ItemType Directory -Path $stage -Force
# 安装器要的是 `weasel-next-theme.dll` 这个名字；cargo 产物叫 `weasel_theme_default.dll`，
# x64 由 build-installer.ps1 改名，arm64 这里自己补一份（缺了 makensis 会直接报错）。
$themeSrc = Join-Path $rel 'weasel_theme_default.dll'
$themeDst = Join-Path $rel 'weasel-next-theme.dll'
if ((Test-Path -LiteralPath $themeSrc) -and -not (Test-Path -LiteralPath $themeDst)) {
    Copy-Item -LiteralPath $themeSrc -Destination $themeDst -Force
}
$copy = {
    param($from, $to)
    if (-not (Test-Path -LiteralPath $from)) { throw "缺文件：$from" }
    Copy-Item -LiteralPath $from -Destination (Join-Path $stage $to) -Force
}
foreach ($name in 'Weasel-Next-Server.exe', 'Weasel-Next-Engine.exe', 'Weasel-Next-Renderer.exe',
                 'Weasel-Next-Setting.exe', 'shuangpin-schema.exe', 'weasel-next-theme.dll',
                 'plum-next.exe') {
    & $copy (Join-Path $rel $name) $name
}
# TIP：arm64 那份对外叫 Weasel-Next.dll；x86 那份沿用 build-tip32 的产物。
& $copy (Join-Path $rel 'weasel_tip.dll') 'Weasel-Next.dll'
& $copy (Join-Path $root 'target\release\Weasel-Next-32.dll') 'Weasel-Next-32.dll'
# 新引擎槽位：arm64 rime-next.dll（Engine 优先加载它）。
& $copy (Join-Path $rel 'librime_next.dll') 'rime-next.dll'
# arm64 运行库（llvm-mingw 自己那套；不带 x64 的 glog/opencc）。
foreach ($dll in 'libc++.dll', 'libunwind.dll', 'libwinpthread-1.dll') {
    & $copy (Join-Path $mingw "aarch64-w64-mingw32\bin\$dll") $dll
}

Step '读版本号'
$manifest = Get-Content -LiteralPath (Join-Path $root 'Tip\Cargo.toml') -Raw
$match = [regex]::Match($manifest, '(?m)^version\s*=\s*"(\d+\.\d+\.\d+)"\s*$')
if (-not $match.Success) { throw 'Tip/Cargo.toml 缺 version' }
$version = $match.Groups[1].Value

$payloadInclude = Join-Path $outputDirectory 'payload-files.nsh'
if (-not (Test-Path -LiteralPath $payloadInclude)) {
    throw "缺 $payloadInclude：先跑一次 build-installer.ps1（不带 -SkipX64 会自动跑）。"
}
$outputFile = Join-Path $outputDirectory "Weasel-Next-$version-arm64-setup.exe"

Step "makensis → $outputFile"
$arguments = @(
    '/V3', '/INPUTCHARSET', 'UTF8',
    "/DPROJECT_ROOT=$root",
    "/DPRODUCT_VERSION=$version",
    "/DOUTPUT_FILE=$outputFile",
    "/DPAYLOAD_INCLUDE=$payloadInclude",
    "/DRELEASE_DIR=$stage",
    '/DTARGET_ARM64',
    (Join-Path $root 'installer\weasel-next.nsi')
)
& makensis @arguments
if ($LASTEXITCODE -ne 0) { throw "NSIS 失败（$LASTEXITCODE）" }
Write-Host ("ARM64 安装包：{0}" -f $outputFile) -ForegroundColor Green
