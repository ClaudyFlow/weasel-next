# pfro.ps1 —— 用 PowerShell 的注册表 API 正确写入 PendingFileRenameOperations
#
# 为什么不用 reg.exe：路径里含空格（C:\Program Files\...），`reg add /d "a\0b"` 的
# \0 分隔与空格混在一起会被解析坏，值写进去了但系统执行时匹配不上，表现为"重启后
# 值被清空、文件却没换"。用 MultiString 直接写字节则不受空格影响。
#
# 必须以管理员身份运行（sudo 或提权窗口）。

$ErrorActionPreference = 'Stop'

$target = 'C:\Program Files\Weasel-Next'
$pairs = @()
foreach ($name in 'Weasel-Next.dll', 'Weasel-Next-Renderer.exe', 'rime.dll') {
    $staged = Join-Path $target "$name.pending.new"
    $final = Join-Path $target $name
    if (Test-Path -LiteralPath $staged) {
        $pairs += "\??\$staged"
        $pairs += "\??\$final"
    }
}

if ($pairs.Count -eq 0) {
    Write-Host '没有可替换的暂存文件。'
    exit 0
}

Write-Host "准备登记 $($pairs.Count) 条："
$pairs | ForEach-Object { Write-Host "  $_" }

$key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey(
    'SYSTEM\CurrentControlSet\Control\Session Manager', $true)
try {
    $key.SetValue('PendingFileRenameOperations', [string[]]$pairs,
        [Microsoft.Win32.RegistryValueKind]::MultiString)
    Write-Host '✓ 已写入'
}
finally {
    $key.Dispose()
}

# 回读校验：逐条打印，确认没有空格被吃掉、条数正确
$read = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager' `
        -Name PendingFileRenameOperations).PendingFileRenameOperations
Write-Host "回读 $($read.Count) 条："
$read | ForEach-Object { Write-Host "  [$_]" }
