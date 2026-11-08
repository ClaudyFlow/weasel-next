# deploy-librime.ps1 —— 把重编的引擎产物（含 librime-next 改动）覆盖到安装目录。
#
# 必须以管理员身份运行（停进程 + 写 Program Files）。
# 用法：
#   gsudo powershell -NoProfile -ExecutionPolicy Bypass -File C:\Code\Weasel-Next\script\deploy-librime.ps1
#
# 覆盖内容：
#   - Weasel-Next.dll  （= weasel_tip.dll，输入法本体；引擎/学词/删词都在里面）

[CmdletBinding()]
param(
    [string]$InstallDir = 'C:\Program Files\Rime-Next\Weasel-Next-0.18.5',
    [string]$TargetDir  = 'C:\Code\Weasel-Next\target\release'
)

$ErrorActionPreference = 'Stop'

# 经 gsudo 提权调用时 $PSScriptRoot 可能为空，直接用绝对仓库路径
$repoRoot = 'C:\Code\Weasel-Next'
$jobs = @(
    # 引擎本体：librime-next 的 cdylib（`librime_next.dll`），安装目录里叫 `rime-next.dll`。
    # 用户词典缓存 / 拼写纠错 / 学词删词全在这一颗里。
    @{ Src = (Join-Path $TargetDir 'librime_next.dll'); Dst = (Join-Path $InstallDir 'rime-next.dll') };
    # TIP：输入法本体（时段统计等宿主侧采集在这里）。
    @{ Src = (Join-Path $TargetDir 'weasel_tip.dll');    Dst = (Join-Path $InstallDir 'Weasel-Next.dll') }
)
$logFile = Join-Path $repoRoot 'script\deploy-librime.log'
try { Start-Transcript -Path $logFile -Force -ErrorAction Stop | Out-Null } catch { }

function Assert-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $p  = [Security.Principal.WindowsPrincipal]::new($id)
    if (-not $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw '需要管理员权限：请用 gsudo / "以管理员身份运行" 的 PowerShell 执行本脚本。'
    }
}

function Stop-WeaselProcesses {
    $names = @('Weasel-Next-Engine', 'Weasel-Next-Renderer', 'Weasel-Next-Server', 'WeaselServer', 'Weasel-Next-Setting', 'Weasel-Next-Setting-D2D')
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
    $subKey = 'SYSTEM\CurrentControlSet\Control\Session Manager'
    try {
        $baseKey = [Microsoft.Win32.Registry]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine, 'ReadWriteSubTree')
        $sk = $baseKey.OpenSubKey($subKey, $true)
        $existing = @($sk.GetValue('PendingFileRenameOperations'))
        $merged = ($existing + @("\??\$Staged", "\??\$Dest")) | Where-Object { $_ -ne $null -and $_ -ne '' }
        $bytes = [Text.Encoding]::Unicode.GetBytes((($merged -join "`0") + "`0`0"))
        $sk.SetValue('PendingFileRenameOperations', $bytes, [Microsoft.Win32.RegistryValueKind]::MultiString)
        $sk.Dispose(); $baseKey.Dispose()
        Write-Host "已登记重启替换: $Dest（重启后生效）"
    } catch {
        Write-Error "登记重启替换失败: $_"
    }
}

# ---------- 主流程 ----------
Assert-Admin

# （真正生效的 `$jobs` 在文件头部，这里沿用，不再重复定义）
foreach ($j in $jobs) {
    if (-not (Test-Path -LiteralPath $j.Src -PathType Leaf)) { Write-Warning "找不到源：$($j.Src)（跳过）"; continue }
    if (-not (Test-Path -LiteralPath $j.Dst -PathType Leaf)) { Write-Warning "找不到目标：$($j.Dst)（跳过）"; continue }
    $sh = (Get-FileHash -LiteralPath $j.Src -Algorithm SHA256).Hash
    $dh = (Get-FileHash -LiteralPath $j.Dst -Algorithm SHA256).Hash
    if ($sh -eq $dh) { Write-Host "已是最新（跳过）: $($j.Dst)"; continue }
    Write-Host "源:   $($j.Src) ($((Get-Item $j.Src).Length) bytes, $((Get-Item $j.Src).LastWriteTime))"
    Write-Host "目标: $($j.Dst) ($((Get-Item $j.Dst).Length) bytes, $((Get-Item $j.Dst).LastWriteTime))"
    $script:pending += @($j)
}

if (-not $pending) { Write-Host "没有需要更新的文件。"; exit 0 }

Stop-WeaselProcesses

foreach ($j in $pending) {
    $bak = "$($j.Dst).bak-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
    try { Copy-Item -LiteralPath $j.Dst -Destination $bak -Force -ErrorAction Stop; Write-Host "已备份 -> $bak" }
    catch { Write-Warning "备份失败（继续）: $_" }
    try {
        Copy-Item -LiteralPath $j.Src -Destination $j.Dst -Force -ErrorAction Stop
        Write-Host "OK 已覆盖: $($j.Dst)"
    } catch {
        Write-Host "直接覆盖失败 ($_)，改为登记重启替换"
        $staged = "$($j.Dst).pending-$([Guid]::NewGuid().ToString('N').Substring(0,8)).new"
        Copy-Item -LiteralPath $j.Src -Destination $staged -Force
        Register-PendingRename -Staged $staged -Dest $j.Dst
    }
    $after = (Get-FileHash -LiteralPath $j.Dst -Algorithm SHA256).Hash
    $src = (Get-FileHash -LiteralPath $j.Src -Algorithm SHA256).Hash
    if ($after -eq $src) { Write-Host "校验通过" } else { Write-Warning "校验不一致（可能待重启替换）" }
}

$server = Join-Path $InstallDir 'Weasel-Next-Server.exe'
if (Test-Path -LiteralPath $server) {
    Write-Host "重启输入法服务..."
    Start-Process -FilePath $server -WorkingDirectory $InstallDir | Out-Null
    Start-Sleep -Seconds 3
    $alive = Get-Process -Name 'Weasel-Next-Server','Weasel-Next-Engine','Weasel-Next-Renderer' -ErrorAction SilentlyContinue
    if ($alive) { $alive | ForEach-Object { Write-Host "  已启动: $($_.Name) (pid=$($_.Id))" } }
    else { Write-Warning "服务未起来，请手动跑 Weasel-Next-Server.exe" }
}

Write-Host ""
Write-Host "部署完成。"
