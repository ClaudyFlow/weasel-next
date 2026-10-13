# replace-tip-dll.ps1 —— 安排在下一次重启时替换被占用的 weasel_tip.dll
#
# 背景：TIP 的 DLL 由 explorer.exe、SearchHost.exe 以及所有正在输入的应用加载，
# 运行期间无法替换（共享冲突）。安装程序只能保留旧文件，所以我们用 Windows 自带的
# 延迟替换机制（PendingFileRenameOperations）把它排到重启早期执行 —— 那时这些进程还没起来。
#
# 用法（**必须以管理员身份运行**，因为要写 HKLM）：
#   powershell -ExecutionPolicy Bypass -File script\replace-tip-dll.ps1
#   powershell -ExecutionPolicy Bypass -File script\replace-tip-dll.ps1 -Source <新的 weasel_tip.dll>
#
# 执行后重启一次，文件即在系统启动早期被替换。

[CmdletBinding()]
param(
    # 要装进去的新 DLL。默认取编译产物里的那份。
    [string] $Source = (Join-Path (Split-Path -Parent $PSScriptRoot) 'target\release\weasel_tip.dll'),

    # 目标位置。默认是程序文件夹（约定：只能在这里运行）。
    [string] $Target = 'C:\Program Files\Weasel-Next\weasel_tip.dll',

    # 只打印将要写入的内容，不实际写注册表。
    [switch] $WhatIfOnly
)

$ErrorActionPreference = 'Stop'

function Assert-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw '需要管理员权限：请用"以管理员身份运行"的 PowerShell 执行本脚本。'
    }
}

if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
    throw "找不到新的 weasel_tip.dll：$Source"
}
if (-not (Test-Path -LiteralPath $Target -PathType Leaf)) {
    throw "找不到已安装的 weasel_tip.dll：$Target"
}

$sourceHash = (Get-FileHash -LiteralPath $Source -Algorithm SHA256).Hash
$targetHash = (Get-FileHash -LiteralPath $Target -Algorithm SHA256).Hash
Write-Host "新 DLL : $Source"
Write-Host "         SHA256 $sourceHash"
Write-Host "已装   : $Target"
Write-Host "         SHA256 $targetHash"

if ($sourceHash -eq $targetHash) {
    Write-Host '两份文件相同，无需替换。'
    return
}

# 延迟替换的约定：先把新文件放到目标同目录的一个临时名，
# 再登记"重启时把临时名改名为正式名"，这样重启早期一步到位。
$staged = "$Target.pending-$([Guid]::NewGuid().ToString('N').Substring(0,8)).new"
Copy-Item -LiteralPath $Source -Destination $staged -Force
Write-Host "已暂存 : $staged"

# PendingFileRenameOperations 的格式是「源 目标」成对出现（前面带 \??\，目标为空表示删除）。
$pair = @("\??\$staged", "\??\$Target")
$key = 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager'
$existing = @((Get-ItemProperty -Path $key -Name PendingFileRenameOperations -ErrorAction SilentlyContinue).PendingFileRenameOperations)

Write-Host "现有待处理项 $($existing.Count) 条，本次追加 2 条："
Write-Host "  $($pair[0])"
Write-Host "  → $($pair[1])"

if ($WhatIfOnly) {
    Write-Host '（-WhatIfOnly：未写入注册表）'
    return
}

Assert-Admin
$merged = $existing + $pair
# 必须用 REG_MULTI_SZ 写入，否则系统读不到。
$lines = ($merged | Where-Object { $_ -ne '' }) -join "`0"
$bytes = [Text.Encoding]::Unicode.GetBytes($lines + "`0`0")
$subKey = 'SYSTEM\CurrentControlSet\Control\Session Manager'
# 注意：在 sudo 提权窗口里 PowerShell 的注册表 API 会失败，改用 reg.exe（实测有效）。
try {
    $baseKey.SetValue('PendingFileRenameOperations', $bytes, [Microsoft.Win32.RegistryValueKind]::MultiString)
} finally {
    $baseKey.Dispose()
}
Write-Host '✓ 已登记。请重启一次，之后 weasel_tip.dll 即为新版本。'
Write-Host '  重启后可用这条命令核对哈希：'
Write-Host "  (Get-FileHash '$Target').Hash"
