#Requires -Version 5.1
<#
.SYNOPSIS
    编出 32 位（i686）的 TIP：target\release\Weasel-Next-32.dll

.DESCRIPTION
    32 位宿主（WPS、部分旧软件）只会去 WOW6432Node 找 32 位 InprocServer32，
    所以必须单独编一份 x86 的 Weasel-Next.dll 并单独注册。

    用 llvm-mingw 的 i686 目标。它的 i686 只带 libunwind.a，没有 gcc 版
    libgcc*.a，而 Rust 的 windows-gnu 目标固定要 -lgcc_eh/-lgcc：
      * 把 libunwind.a 复制成 libgcc.a / libgcc_eh.a 放进 build\x86-linkstubs；
      * MinGW 的 crtbegin 还要 __register_frame_info/__deregister_frame_info，
        llvm-mingw 不提供 → 用 unwind_shim.c 里的空实现垫上（见该文件注释）。
    这两步只影响链接，不改任何运行期行为。

    产物名字带 -32：和 64 位的 Weasel-Next.dll 在同一个目录里共存，
    注册表两个视图各指一份。
#>
[CmdletBinding()]
param(
    [switch]$OnlyIfMissing
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$mingw = 'C:\Program Files\llvm-mingw'
$stub = Join-Path $projectRoot 'build\x86-linkstubs'
$target = 'i686-pc-windows-gnu'
$output = Join-Path $projectRoot 'target\release\Weasel-Next-32.dll'

if ($OnlyIfMissing -and (Test-Path -LiteralPath $output)) {
    Write-Host "已存在，跳过：$output"
    exit 0
}

if (-not (Test-Path -LiteralPath (Join-Path $mingw 'bin\i686-w64-mingw32-gcc.exe'))) {
    throw "找不到 llvm-mingw 的 i686 编译器：$mingw"
}

# 1) rust target
$installed = & rustup target list --installed
if ($installed -notcontains $target) {
    Write-Host "安装 rust target $target …"
    & rustup target add $target
    if ($LASTEXITCODE -ne 0) { throw "rustup target add $target 失败" }
}

# 2) 链接桩：libgcc / libgcc_eh → libunwind，外加帧注册符号垫片
New-Item -ItemType Directory -Path $stub -Force | Out-Null
$unwind = Join-Path $mingw 'i686-w64-mingw32\lib\libunwind.a'
foreach ($name in 'libgcc.a', 'libgcc_eh.a') {
    Copy-Item -LiteralPath $unwind -Destination (Join-Path $stub $name) -Force
}
$shimSource = Join-Path $stub 'unwind_shim.c'
$shimObject = Join-Path $stub 'unwind_shim.o'
$shimArchive = Join-Path $stub 'libgcc_shim.a'
if (-not (Test-Path -LiteralPath $shimSource)) { throw "缺少 $shimSource" }
& (Join-Path $mingw 'bin\i686-w64-mingw32-gcc.exe') -O2 -c $shimSource -o $shimObject
if ($LASTEXITCODE -ne 0) { throw '编译 unwind 垫片失败' }
& (Join-Path $mingw 'bin\llvm-ar.exe') rcs $shimArchive $shimObject
if ($LASTEXITCODE -ne 0) { throw '打包 unwind 垫片失败' }

# 3) 编
$env:LIBCLANG_PATH = Join-Path $mingw 'bin'
$env:PATH = (Join-Path $mingw 'bin') + ';' + $env:PATH
$env:CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER = 'i686-w64-mingw32-gcc'
$env:RUSTFLAGS = "-C link-arg=-L$stub -C link-arg=$shimArchive"

Push-Location $projectRoot
try {
    & cargo build --release -p weasel-tip --target $target
    if ($LASTEXITCODE -ne 0) { throw "32 位 TIP 编译失败" }
} finally {
    Pop-Location
}

# 4) 取产物、改成对外名字
$built = Join-Path $projectRoot "target\$target\release\weasel_tip.dll"
if (-not (Test-Path -LiteralPath $built)) { throw "没有产物：$built" }
Copy-Item -LiteralPath $built -Destination $output -Force

$machine = 0
$fs = [IO.File]::Open($output, 'Open', 'Read', 'ReadWrite')
try {
    $br = New-Object IO.BinaryReader($fs)
    $fs.Position = 0x3C
    $fs.Position = $br.ReadInt32() + 4
    $machine = $br.ReadUInt16()
} finally { $fs.Dispose() }
if ($machine -ne 0x14c) { throw ("产物不是 x86：machine=0x{0:X}" -f $machine) }

$file = Get-Item -LiteralPath $output
$hash = (Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash
Write-Host ("32 位 TIP 就绪：{0}  {1:N0} 字节  SHA256={2}" -f $file.FullName, $file.Length, $hash)
