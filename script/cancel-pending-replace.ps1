# cancel-pending-replace.ps1 —— 撤销 weasel-next 的重启替换登记
#
# 只删与本项目相关的条目（路径里含 Weasel-Next），其它程序的登记（例如 Edge 的更新）
# 原样保留。同时删除暂存的 *.pending.new 文件。
#
# 必须以管理员身份运行（要写 HKLM）。

$ErrorActionPreference = 'Stop'

$key = 'SYSTEM\CurrentControlSet\Control\Session Manager'
$name = 'PendingFileRenameOperations'
$current = @((Get-ItemProperty -Path "HKLM:\$key" -Name $name -ErrorAction SilentlyContinue).$name)

Write-Host "原有 $($current.Count) 条："
$current | ForEach-Object { Write-Host "  [$_]" }

$kept = @($current | Where-Object { $_ -and $_ -notmatch 'Weasel-Next' })
$removed = $current.Count - $kept.Count

$baseKey = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($key, $true)
try {
    if ($kept.Count -eq 0) {
        $baseKey.DeleteValue($name, $false)
        Write-Host '已删除整个 PendingFileRenameOperations 值（保留的条目为 0）'
    }
    else {
        $baseKey.SetValue($name, [string[]]$kept, [Microsoft.Win32.RegistryValueKind]::MultiString)
        Write-Host "已写回 $($kept.Count) 条"
    }
}
finally {
    $baseKey.Dispose()
}

Write-Host "移除与 Weasel-Next 相关的 $removed 条。"

# 删除暂存文件
$target = 'C:\Program Files\Weasel-Next'
$staged = @(Get-ChildItem -LiteralPath $target -Filter '*.pending.new' -File -ErrorAction SilentlyContinue)
foreach ($file in $staged) {
    Remove-Item -LiteralPath $file.FullName -Force
    Write-Host "已删除暂存文件 $($file.Name)"
}
if ($staged.Count -eq 0) {
    Write-Host '没有暂存文件。'
}

$verify = @((Get-ItemProperty -Path "HKLM:\$key" -Name $name -ErrorAction SilentlyContinue).$name)
Write-Host "回读 $($verify.Count) 条："
$verify | ForEach-Object { Write-Host "  [$_]" }
