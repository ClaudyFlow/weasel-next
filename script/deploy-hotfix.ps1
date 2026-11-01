# deploy-hotfix.ps1 —— 把本次 hotfix 编译产物覆盖到安装目录。
#
# 必须以管理员身份运行（停进程 + 写 Program Files + 登记待重启替换）。
# 用法（由部署流程以 RunAs 提权调用）：
#   powershell -ExecutionPolicy Bypass -File script\deploy-hotfix.ps1
#
# 覆盖内容：
#   - Weasel-Next.dll           (x64 TIP，数字上屏修复)
#   - Weasel-Next-32.dll        (x86 TIP，数字上屏修复)
#   - weasel-next-theme.dll (x64 theme，玻璃化修复)
#   - Rime-Next\key_bindings.yaml (逗号 when: paging -> has_menu)

[CmdletBinding()]
param(
    [string]$InstallDir = 'C:\Program Files\Rime-Next\Weasel-Next-0.18.5',
    [string]$TargetDir  = (Join-Path (Split-Path -Parent $PSScriptRoot) 'target')
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot

# 记录到日志，便于提权进程跑完后核对结果
$logFile = Join-Path $PSScriptRoot 'deploy-hotfix.log'
try { Start-Transcript -Path $logFile -Force -ErrorAction Stop } catch { }

function Assert-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $p  = [Security.Principal.WindowsPrincipal]::new($id)
    if (-not $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw '需要管理员权限：请用"以管理员身份运行"的 PowerShell 执行本脚本。'
    }
}

function Stop-WeaselProcesses {
    $names = @('Weasel-Next-Engine', 'Weasel-Next-Renderer', 'Weasel-Next-Server', 'WeaselServer')
    foreach ($n in $names) {
        Get-Process -Name $n -ErrorAction SilentlyContinue | ForEach-Object {
            Write-Host "停止进程 $($_.Name) (pid=$($_.Id))"
            $_.Kill()
        }
    }
    Start-Sleep -Seconds 1
}

# 直接覆盖；若因共享冲突失败，则登记到 PendingFileRenameOperations 待重启替换。
function Replace-File {
    param(
        [Parameter(Mandatory)] [string]$Source,
        [Parameter(Mandatory)] [string]$Dest
    )
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
        Write-Warning "找不到源：$Source （跳过）"
        return
    }
    $srcHash = (Get-FileHash -LiteralPath $Source -Algorithm SHA256).Hash
    if (Test-Path -LiteralPath $Dest -PathType Leaf) {
        $dstHash = (Get-FileHash -LiteralPath $Dest -Algorithm SHA256).Hash
        if ($srcHash -eq $dstHash) {
            Write-Host "已是最新（跳过）: $Dest"
            return
        }
    }
    try {
        Copy-Item -LiteralPath $Source -Destination $Dest -Force -ErrorAction Stop
        Write-Host "✓ 已覆盖: $Dest"
    } catch {
        Write-Host "直接覆盖失败 ($_)，改为登记重启替换: $Dest"
        $staged = "$Dest.pending-$([Guid]::NewGuid().ToString('N').Substring(0,8)).new"
        Copy-Item -LiteralPath $Source -Destination $staged -Force
        Register-PendingRename -Staged $staged -Dest $Dest
    }
}

function Register-PendingRename {
    param(
        [Parameter(Mandatory)] [string]$Staged,
        [Parameter(Mandatory)] [string]$Dest
    )
    $pair = @("\??\$Staged", "\??\$Dest")
    $key  = 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager'
    $existing = @((Get-ItemProperty -Path $key -Name PendingFileRenameOperations -ErrorAction SilentlyContinue).PendingFileRenameOperations)
    $merged = $existing + $pair
    $lines  = ($merged | Where-Object { $_ -ne '' }) -join "`0"
    $bytes  = [Text.Encoding]::Unicode.GetBytes($lines + "`0`0")
    $subKey = 'SYSTEM\CurrentControlSet\Control\Session Manager'
    try {
        $baseKey = [Microsoft.Win32.Registry]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine, 'ReadWriteSubTree')
        $sk = $baseKey.OpenSubKey($subKey, $true)
        $sk.SetValue('PendingFileRenameOperations', $bytes, [Microsoft.Win32.RegistryValueKind]::MultiString)
        $sk.Dispose(); $baseKey.Dispose()
        Write-Host "✓ 已登记重启替换: $Dest（重启后生效）"
    } catch {
        Write-Error "登记重启替换失败: $_"
    }
}

# ---------- 主流程 ----------
Assert-Admin

$rx64 = Join-Path $TargetDir 'x86_64-pc-windows-gnu\release'
$rx86 = Join-Path $TargetDir 'i686-pc-windows-gnu\release'

if (-not (Test-Path -LiteralPath $rx64)) { throw "找不到 x64 编译产物目录: $rx64（请先编译）" }

Stop-WeaselProcesses

# dll（被锁则走重启替换）
Replace-File -Source (Join-Path $rx64 'weasel_tip.dll')          -Dest (Join-Path $InstallDir 'Weasel-Next.dll')
if (Test-Path -LiteralPath $rx86) {
    Replace-File -Source (Join-Path $rx86 'weasel_tip.dll')      -Dest (Join-Path $InstallDir 'Weasel-Next-32.dll')
} else {
    Write-Warning "找不到 x86 编译产物，跳过 Weasel-Next-32.dll"
}
Replace-File -Source (Join-Path $rx64 'weasel-next-theme.dll') -Dest (Join-Path $InstallDir 'weasel-next-theme.dll')

# key_bindings.yaml（逗号修复，文本文件，停进程后可直接覆盖）
$kbSrc = Join-Path $repoRoot 'Rime-Next\key_bindings.yaml'
Replace-File -Source $kbSrc -Dest (Join-Path $InstallDir 'Rime-Next\key_bindings.yaml')

Write-Host ""
Write-Host "部署完成。若有 dll 走了重启替换，请重启一次机器以生效。"
