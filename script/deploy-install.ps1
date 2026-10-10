# deploy-install.ps1 — 编译并直接安装到安装路径
#
# 用法（普通权限即可，内部会用 sudo 提权做写入）：
#   powershell -ExecutionPolicy Bypass -File script\deploy-install.ps1
#   powershell -ExecutionPolicy Bypass -File script\deploy-install.ps1 -SkipBuild
#
# 行为：
#   1. 从工作区根做 release 编译（可用 -SkipBuild 跳过）；
#   2. 把运行所需文件同步到安装目录 C:\Program Files\Weasel-Next\；
#   3. 重启栈（broker/server/renderer），从安装目录启动；
#   4. 校验 TIP 注册表指向安装目录，不对则修正（sudo）；
#   5. 打印关键文件清单。
#
# 设计说明：
#   * 写入 Program Files 需要管理员权限。本脚本把复制/改注册表的命令写进一个
#     临时 .cmd，再用 sudo 执行 —— 直接把参数交给 sudo 会丢掉参数（实测）。
#   * 不复制 *.old-* / *.previous* / 日志 / 测试程序，保持安装目录干净。

[CmdletBinding()]
param(
    [string]$InstallDir = 'C:\Program Files\Weasel-Next',
    [switch]$SkipBuild,
    [switch]$NoRestart
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$stage = Join-Path $root 'artifact\smoke'
$clsid = 'HKLM\Software\Classes\CLSID\{16A7AEA9-9EE6-4540-8020-E384C0489BB1}\InprocServer32'

function Write-Step($text) { Write-Host "== $text" -ForegroundColor Cyan }
function Write-Ok($text) { Write-Host "   $text" -ForegroundColor Green }
function Write-Warn2($text) { Write-Host "   $text" -ForegroundColor Yellow }

function Invoke-ElevatedScript {
    param([string[]]$Lines)
    $tmp = Join-Path $env:TEMP ("weasel-next-deploy-{0}.cmd" -f ([Guid]::NewGuid().ToString('N').Substring(0, 8)))
    Set-Content -Path $tmp -Value ((@('@echo off') + $Lines + @('echo DONE')) -join "`r`n") -Encoding OEM
    $out = & sudo cmd /c $tmp 2>&1
    Remove-Item $tmp -Force -ErrorAction SilentlyContinue
    return $out
}

# 1) 编译
if (-not $SkipBuild) {
    Write-Step '从工作区根做 release 编译'
    & cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw '编译失败，未安装' }
    Write-Ok '编译通过'
}

# 2) 同步到安装目录（sudo + .cmd，参数才不会丢）
Write-Step "同步到安装目录 $InstallDir"
$elevated = @(
    ('if not exist "{0}" mkdir "{0}"' -f $InstallDir),
    ('robocopy "{0}" "{1}" /E /XF *.old-* *.previous* *.log /NFL /NDL /NJH /NJS /R:1 /W:1' -f $stage, $InstallDir),
    ('robocopy "{0}\asset" "{1}\asset" /E /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    # 清掉早先留下的旧文件（安装目录里不该有它们）
    ('del /f /q "{0}\weasel_tip.previous.dll" 2>nul' -f $InstallDir),
    ('del /f /q "{0}\weasel-server.previous.exe" 2>nul' -f $InstallDir),
    ('del /f /q "{0}\weasel_tip.previous-*.dll" 2>nul' -f $InstallDir),
    ('reg add "{0}" /ve /t REG_SZ /d "{1}\weasel_tip.dll" /f' -f $clsid, $InstallDir)
)
$log = Invoke-ElevatedScript -Lines $elevated
if ($log -notmatch 'DONE') { Write-Warn2 '提权脚本未正常结束，请检查输出' ; $log | ForEach-Object { Write-Host "     $_" } }
Write-Ok '文件已同步'

# 3) 重启栈（从安装目录启动）
if (-not $NoRestart) {
    Write-Step '重启栈（从安装目录启动）'
    Get-Process -Name 'weasel-broker', 'weasel-server', 'weasel-renderer' -ErrorAction SilentlyContinue |
        ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
    Get-Process -Name 'weasel-broker', 'weasel-server', 'weasel-renderer' -ErrorAction SilentlyContinue |
        ForEach-Object { try { $_.WaitForExit(3000) } catch { } }
    Start-Sleep -Seconds 3
    $env:WEASEL_TRACE_COMMIT = '1'
    Start-Process -FilePath (Join-Path $InstallDir 'weasel-broker.exe') -WorkingDirectory $InstallDir
    Start-Sleep -Seconds 12
    Get-Process -Name 'weasel-broker', 'weasel-server', 'weasel-renderer' -ErrorAction SilentlyContinue |
        ForEach-Object { Write-Ok ("{0} PID {1}  {2}" -f $_.ProcessName, $_.Id, $_.Path) }
}

# 4) 校验 TIP 注册表
Write-Step '校验 TIP 注册表'
$value = (reg query $clsid /ve 2>$null | Where-Object { $_ -match 'REG_SZ' }) -join ''
$expect = Join-Path $InstallDir 'weasel_tip.dll'
if ($value -match [regex]::Escape($expect)) { Write-Ok "指向安装目录 ✓ $expect" }
else { Write-Warn2 "仍是: $value" ; Write-Warn2 "期望: $expect（请重跑本脚本，或手动 reg add）" }

# 5) 清单
Write-Step '安装目录关键文件'
foreach ($f in 'weasel-broker.exe', 'weasel-server.exe', 'weasel-renderer.exe', 'weasel_tip.dll', 'rime.dll',
    'asset\Rime-Next\sound\typewriter-key.wav', 'asset\Rime-Next\sound\typewriter-bell.wav', 'themes\weasel_theme_ten.dll') {
    $p = Join-Path $InstallDir $f
    $mark = if (Test-Path $p) { '✓' } else { '✗' }
    $size = if (Test-Path $p) { '{0:N0} 字节' -f (Get-Item $p).Length } else { '缺失' }
    Write-Host ("   {0,-42} {1}  {2}" -f $f, $mark, $size)
}
$total = (Get-ChildItem $InstallDir -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
Write-Ok ('共 {0:N1} MB' -f ($total / 1MB))
