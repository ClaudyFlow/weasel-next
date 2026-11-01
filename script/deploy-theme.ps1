# deploy-theme.ps1 —— 把重新编译的 theme DLL 覆盖到安装目录并重启输入法。
#
# 必须以管理员身份运行（停进程 + 写 Program Files）。
# 用法：
#   powershell -ExecutionPolicy Bypass -File script\deploy-theme.ps1
#
# 覆盖内容：
#   - weasel_theme_official.dll  (候选窗主题：渐变底 / 圆角遮罩 / 字高绑定间距)

[CmdletBinding()]
param(
    [string]$InstallDir = 'C:\Program Files\Rime-Next\Weasel-Next-0.18.5',
    [string]$TargetDir  = 'C:\Code\Weasel-Next\target\release'
)

$ErrorActionPreference = 'Stop'

# 经 gsudo 提权调用时 $PSScriptRoot 可能为空，直接用绝对仓库路径
$repoRoot = 'C:\Code\Weasel-Next'

# 记录到日志，便于提权进程跑完后核对结果
$logFile = Join-Path $repoRoot 'script\deploy-theme.log'
try { Start-Transcript -Path $logFile -Force -ErrorAction Stop | Out-Null } catch { }

function Assert-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $p  = [Security.Principal.WindowsPrincipal]::new($id)
    if (-not $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw '需要管理员权限：请用"以管理员身份运行"的 PowerShell 执行本脚本。'
    }
}

function Stop-WeaselProcesses {
    $names = @('Weasel-Next-Engine', 'Weasel-Next-Renderer', 'Weasel-Next-Server', 'WeaselServer', 'Weasel-Next-Setting')
    foreach ($n in $names) {
        Get-Process -Name $n -ErrorAction SilentlyContinue | ForEach-Object {
            Write-Host "停止进程 $($_.Name) (pid=$($_.Id))"
            $_.Kill() | Out-Null
        }
    }
    Start-Sleep -Seconds 2
}

function Register-PendingRename {
    param(
        [Parameter(Mandatory)] [string]$Staged,
        [Parameter(Mandatory)] [string]$Dest
    )
    $pair = @("\??\$Staged", "\??\$Dest")
    $subKey = 'SYSTEM\CurrentControlSet\Control\Session Manager'
    try {
        $baseKey = [Microsoft.Win32.Registry]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine, 'ReadWriteSubTree')
        $sk = $baseKey.OpenSubKey($subKey, $true)
        $existing = @($sk.GetValue('PendingFileRenameOperations'))
        $merged = ($existing + $pair) | Where-Object { $_ -ne $null -and $_ -ne '' }
        $lines = $merged -join "`0"
        $bytes = [Text.Encoding]::Unicode.GetBytes($lines + "`0`0")
        $sk.SetValue('PendingFileRenameOperations', $bytes, [Microsoft.Win32.RegistryValueKind]::MultiString)
        $sk.Dispose(); $baseKey.Dispose()
        Write-Host "已登记重启替换: $Dest（重启后生效）"
    } catch {
        Write-Error "登记重启替换失败: $_"
    }
}

# ---------- 主流程 ----------
Assert-Admin

$src = Join-Path $TargetDir 'weasel_theme_official.dll'
$dst = Join-Path $InstallDir 'weasel_theme_official.dll'

if (-not (Test-Path -LiteralPath $src -PathType Leaf)) { throw "找不到编译产物: $src（请先 cargo build --release -p weasel-theme-official）" }
if (-not (Test-Path -LiteralPath $dst -PathType Leaf)) { throw "找不到安装目录里的 dll: $dst" }

$srcHash = (Get-FileHash -LiteralPath $src -Algorithm SHA256).Hash
$dstHash = (Get-FileHash -LiteralPath $dst -Algorithm SHA256).Hash
if ($srcHash -eq $dstHash) {
    Write-Host "内容一致，无需覆盖: $dst"
    exit 0
}

Write-Host "源: $src ($((Get-Item $src).Length) bytes, $((Get-Item $src).LastWriteTime))"
Write-Host "目标: $dst ($((Get-Item $dst).Length) bytes, $((Get-Item $dst).LastWriteTime))"

Stop-WeaselProcesses

# 备份
$bak = "$dst.bak-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
try {
    Copy-Item -LiteralPath $dst -Destination $bak -Force -ErrorAction Stop
    Write-Host "已备份原文件 -> $bak"
} catch {
    Write-Warning "备份失败（继续覆盖）: $_"
}

# 覆盖（被锁则登记重启替换）
try {
    Copy-Item -LiteralPath $src -Destination $dst -Force -ErrorAction Stop
    Write-Host "OK 已覆盖: $dst"
} catch {
    Write-Host "直接覆盖失败 ($_)，改为登记重启替换"
    $staged = "$dst.pending-$([Guid]::NewGuid().ToString('N').Substring(0,8)).new"
    Copy-Item -LiteralPath $src -Destination $staged -Force
    Register-PendingRename -Staged $staged -Dest $dst
}

# 校验
$after = (Get-FileHash -LiteralPath (Join-Path $InstallDir 'weasel_theme_official.dll') -Algorithm SHA256).Hash
if ($after -eq $srcHash) { Write-Host "校验通过（哈希一致）" } else { Write-Warning "校验不一致，可能待重启替换生效" }

# 重启输入法
$server = Join-Path $InstallDir 'Weasel-Next-Server.exe'
if (Test-Path -LiteralPath $server) {
    Write-Host "重启输入法服务..."
    Start-Process -FilePath $server -WorkingDirectory $InstallDir | Out-Null
    Start-Sleep -Seconds 2
    $alive = Get-Process -Name 'Weasel-Next-Server','Weasel-Next-Engine','Weasel-Next-Renderer' -ErrorAction SilentlyContinue
    if ($alive) {
        $alive | ForEach-Object { Write-Host "  已启动: $($_.Name) (pid=$($_.Id))" }
    } else {
        Write-Warning "服务未起来，请手动运行 Weasel-Next-Server.exe 或重新切换输入法"
    }
}

Write-Host ""
Write-Host "部署完成。"
