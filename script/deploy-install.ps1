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
$stage = Join-Path $root 'target\release'   # 直接取编译输出（不再经 artifact\smoke，避免装到旧产物）
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
    ('robocopy "{0}\Rime-Next" "{1}\Rime-Next" /E /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    ('robocopy "{0}\tools\7z" "{1}\7z" /E /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    ('robocopy "{0}\target\release" "{1}\themes" weasel_theme_*.dll /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    ('robocopy "{0}\Rime-Next\sound" "{1}\Rime-Next\sound" /E /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    # 清理已迁移的旧扁平 yaml：源已放进 schema\<方案>\，目标里残留的扁平副本要删掉
    ('for %F in ("{1}\Rime-Next\*.schema.yaml" "{1}\Rime-Next\*.dict.yaml") do if exist "%F" del /f /q "%F"' -f $root, $InstallDir),
    # 清掉早先留下的旧文件（安装目录里不该有它们）
    ('del /f /q "{0}\weasel_tip.previous.dll" 2>nul' -f $InstallDir),
    ('del /f /q "{0}\weasel-server.previous.exe" 2>nul' -f $InstallDir),
    ('del /f /q "{0}\weasel_tip.previous-*.dll" 2>nul' -f $InstallDir),
    ('reg add "{0}" /ve /t REG_SZ /d "{1}\weasel_tip.dll" /f' -f $clsid, $InstallDir)
)
$log = Invoke-ElevatedScript -Lines $elevated
if ($log -notmatch 'DONE') { Write-Warn2 '提权脚本未正常结束，请检查输出' ; $log | ForEach-Object { Write-Host "     $_" } }
Write-Ok '文件已同步'

# 2.5) 语言模型不再由本仓库分发：压缩包与解压步骤已移除。
#      模型改由外部获取（下载或从既有安装同步）后直接放进 <INSTDIR>\Rime-Next\model\；
#      缺失时只影响语言模型打分，不阻断部署与打字。

# 3) 重启栈（从安装目录启动）
if (-not $NoRestart) {
    Write-Step '重启栈（从安装目录启动）'
    Get-Process -Name 'weasel-broker', 'weasel-server', 'weasel-renderer' -ErrorAction SilentlyContinue |
        ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
    Get-Process -Name 'weasel-broker', 'weasel-server', 'weasel-renderer' -ErrorAction SilentlyContinue |
        ForEach-Object { try { $_.WaitForExit(3000) } catch { } }
    Start-Sleep -Seconds 3
    $env:WEASEL_TRACE_COMMIT = '1'
    # 用 WMI 启动：父进程是 WmiPrvSE，脱离调用方进程树。
    # 用 Start-Process / cmd start 会被调用方的会话回收，服务端随即消失（2026-10-10 故障总根源）。
    $broker = Join-Path $InstallDir 'weasel-broker.exe'
    $created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = "`"$broker`"" }
    if ($created.ReturnValue -ne 0) { Write-Warn2 ('WMI 启动失败，返回码 {0}' -f $created.ReturnValue) }
    else { Write-Ok ('已用 WMI 启动 broker（PID {0}）' -f $created.ProcessId) }
    Start-Sleep -Seconds 15
    # 三关校验：进程 / 命名管道 / 管道可连
    $missing = @()
    foreach ($name in 'weasel-broker', 'weasel-server', 'weasel-renderer') {
        if (-not (Get-Process -Name $name -ErrorAction SilentlyContinue)) { $missing += $name }
    }
    if ($missing.Count -gt 0) { Write-Warn2 ('未起来的组件: ' + ($missing -join ', ')) } else { Write-Ok '三个组件都在 ✓' }
    $pipes = @(Get-ChildItem '\\.\pipe\' -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'weasel-next' })
    Write-Ok ('命名管道 {0} 条' -f $pipes.Count)
    foreach ($pipe in ($pipes | Where-Object { $_.Name -match 'server' })) {
        try {
            $client = New-Object System.IO.Pipes.NamedPipeClientStream('.', $pipe.Name, [System.IO.Pipes.PipeDirection]::InOut)
            $client.Connect(2000)
            $client.Dispose()
            Write-Ok '服务端管道可连 ✓（输入法可用）'
        } catch { Write-Warn2 ('服务端管道连不上: ' + $_.Exception.Message) }
    }
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
    'Rime-Next\sound\typewriter-key.wav', 'Rime-Next\sound\typewriter-bell.wav', 'themes\weasel_theme_ten.dll') {
    $p = Join-Path $InstallDir $f
    $mark = if (Test-Path $p) { '✓' } else { '✗' }
    $size = if (Test-Path $p) { '{0:N0} 字节' -f (Get-Item $p).Length } else { '缺失' }
    Write-Host ("   {0,-42} {1}  {2}" -f $f, $mark, $size)
}
$total = (Get-ChildItem $InstallDir -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
Write-Ok ('共 {0:N1} MB' -f ($total / 1MB))
# ★ 只打包 rime.lua 这个接口到**用户文件夹**（插件本体不进包 ✓ 用户自己放 ✓）
$luaSrc = Join-Path $PSScriptRoot '..\plugin\lua\rime.lua'
$luaDst = Join-Path $env:APPDATA 'Rime-Next\plugin'
if (Test-Path $luaSrc) {
    New-Item -ItemType Directory -Force -Path $luaDst | Out-Null
    Copy-Item $luaSrc $luaDst -Force
    Write-Host ('rime.lua 已安装 → ' + $luaDst)
}
