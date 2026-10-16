# deploy-install.ps1 — 编译并直接安装到安装路径
#
# 用法（普通权限即可，内部会用 sudo 提权做写入）：
#   powershell -ExecutionPolicy Bypass -File script\deploy-install.ps1
#   powershell -ExecutionPolicy Bypass -File script\deploy-install.ps1 -SkipBuild
#
# 行为：
#   1. 从工作区根做 release 编译（可用 -SkipBuild 跳过）；
#   2. 把运行所需文件同步到安装目录 C:\Program Files\Weasel-Next\；
#   3. 重启栈（weasel-next-Engine/Engine/renderer），从安装目录启动；
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
    # 提权脚本的完整输出留一份日志：同步失败时（例如 robocopy 报错）要能事后查
    $logPath = Join-Path $env:TEMP 'weasel-next-deploy.log'
    $donePath = Join-Path $env:TEMP 'weasel-next-deploy.done'
    Remove-Item $logPath, $donePath -Force -ErrorAction SilentlyContinue
    # sudo 处于「强制新建窗口」模式：命令是**异步**的，输出也不回传。
    # 所以既把日志写进文件，也用一个结束标记文件让调用方等它真正做完。
    $body = @('@echo off') + $Lines + @(
        'echo SYNC_EXIT=%ERRORLEVEL%',
        ('echo done > "{0}"' -f $donePath)
    )
    $quoted = $body -join "`r`n"
    Set-Content -Path $tmp -Value $quoted -Encoding OEM
    # 重定向写在 cmd 内部（用 call 包一层），避免 sudo 重新解析引号时丢掉重定向
    $wrapper = Join-Path $env:TEMP 'weasel-next-deploy-wrapper.cmd'
    Set-Content -Path $wrapper -Value (@('@echo off', ('call "{0}" > "{1}" 2>&1' -f $tmp, $logPath)) -join "`r`n") -Encoding OEM
    & sudo cmd /c $wrapper | Out-Null
    Write-Host "   提权日志：$logPath"
    # 等结束标记（最多 10 分钟：首次复制可能要拷几百 MB）
    for ($i = 0; $i -lt 1200; $i++) {
        if (Test-Path $donePath) { break }
        Start-Sleep -Milliseconds 500
    }
    $log = if (Test-Path $logPath) { Get-Content -LiteralPath $logPath -Raw } else { '' }
    foreach ($line in ($log -split "`r?`n" | Where-Object { $_.Trim() })) { Write-Host "     $line" }
    if (-not (Test-Path $donePath)) { Write-Warn2 '提权脚本在 10 分钟内没有结束（可能还在复制，或 UAC 未确认）' }
    Remove-Item $tmp, $wrapper, $donePath, $logPath -Force -ErrorAction SilentlyContinue
    return $log
}

# 1) 编译
if (-not $SkipBuild) {
    Write-Step '从工作区根做 release 编译'
    & cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw '编译失败，未安装' }
    Write-Ok '编译通过'
}

# 1.5) 先停栈：安装目录里的 exe/dll 被运行中的进程锁着，robocopy 会静默跳过它们
Write-Step '停止运行中的组件（否则文件被占用，复制会静默失败）'
$running = @(Get-Process -Name 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer' -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    $running | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
    for ($i = 0; $i -lt 40; $i++) {
        $left = @(Get-Process -Name 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer' -ErrorAction SilentlyContinue)
        if ($left.Count -eq 0) { break }
        Start-Sleep -Milliseconds 250
    }
    Write-Ok ("已停止 {0} 个进程" -f $running.Count)
} else {
    Write-Ok '没有在跑的组件'
}

# 2) 同步到安装目录（sudo + .cmd，参数才不会丢）
Write-Step "同步到安装目录 $InstallDir"
$elevated = @(
    ('if not exist "{0}" mkdir "{0}"' -f $InstallDir),
    # 只拷运行时产物：cargo 的 deps\、build\、incremental\ 有几 GB，绝不能进安装目录
    ('robocopy "{0}" "{1}" /E /XD deps build incremental .fingerprint examples /XF *.old-* *.previous* *.log *.d *.rlib *.rmeta *.pdb /NFL /NDL /NJH /NJS /R:1 /W:1' -f $stage, $InstallDir),
    # 早先被整体复制进去的构建目录要清掉（安装目录只放运行时文件）。
    # 注意：批处理里循环变量是 %%D；这里干脆一个目录一条命令，避免再次踩坑。
    ('if exist "{0}\deps" rmdir /s /q "{0}\deps"' -f $InstallDir),
    ('if exist "{0}\build" rmdir /s /q "{0}\build"' -f $InstallDir),
    ('if exist "{0}\incremental" rmdir /s /q "{0}\incremental"' -f $InstallDir),
    ('if exist "{0}\.fingerprint" rmdir /s /q "{0}\.fingerprint"' -f $InstallDir),
    ('if exist "{0}\examples" rmdir /s /q "{0}\examples"' -f $InstallDir),
    ('robocopy "{0}\Rime-Next" "{1}\Rime-Next" /E /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    ('robocopy "{0}\tool\7z" "{1}\7z" /E /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    ('robocopy "{0}\target\release" "{1}\themes" weasel_theme_*.dll /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    ('robocopy "{0}\Rime-Next\sound" "{1}\Rime-Next\sound" /E /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    # 方案清单已改成 librime 直读 schema.yaml：清掉老机器上生成的这份中转 YAML
    ('del /f /q "%APPDATA%\Rime-Next\default.custom.yaml" 2>nul'),
    # TIP 的 cargo 产物叫 weasel_tip.dll，对外/注册的名字是 Weasel-Next.dll。
    # 必须在这里刷新对外名，否则注册表指向的那个 DLL 永远是上一版的（改动看着"没生效"）。
    # 它被每个开着输入法的进程加载着 → 原地覆盖必然失败；照安装包的做法先改名再放新的
    # （NTFS 允许给已加载的 DLL 改名），旧文件留成 weasel_tip.old.*.dll 等进程退出后再删。
    ('if exist "{1}\Weasel-Next.dll" ren "{1}\Weasel-Next.dll" "weasel_tip.old.%RANDOM%%RANDOM%.dll"' -f $root, $InstallDir),
    ('copy /y "{0}\target\release\weasel_tip.dll" "{1}\Weasel-Next.dll"' -f $root, $InstallDir),
    ('del /f /q "{1}\weasel_tip.old.*.dll" 2>nul' -f $root, $InstallDir),
    # rime.dll 由 artifact\smoke 提供（不在 target\release 里）——必须显式搬，否则永远装旧版
    ('copy /y "{0}\artifact\smoke\rime.dll" "{1}\rime.dll"' -f $root, $InstallDir),
    # punctuation.yaml 已并入 symbols.yaml：把安装目录里那份也清掉
    ('del /f /q "{1}\Rime-Next\punctuation.yaml" 2>nul' -f $root, $InstallDir),
    # 宿主配置已改回 YAML（同名换后缀）：装 weasel.yaml，清掉 weasel.json 与上一版的 weasel.toml
    ('copy /y "{0}\weasel.yaml" "{1}\weasel.yaml"' -f $root, $InstallDir),
    ('del /f /q "{1}\weasel.toml" 2>nul' -f $root, $InstallDir),
    ('del /f /q "{1}\Rime-Next\theme\default\default.theme.toml" 2>nul' -f $root, $InstallDir),
    ('del /f /q "{1}\Rime-Next\eassy\default\default.eassy.toml" 2>nul' -f $root, $InstallDir),
    ('del /f /q "{1}\Rime-Next\schema\schema.toml" 2>nul' -f $root, $InstallDir),
    ('del /f /q "{1}\weasel.json" 2>nul' -f $root, $InstallDir),
    # 清理已迁移的旧扁平 yaml：源已放进 schema\<方案>\，目标里残留的扁平副本要删掉
    ('for %%F in ("{1}\Rime-Next\*.schema.yaml" "{1}\Rime-Next\*.dict.yaml") do if exist "%%F" del /f /q "%%F"' -f $root, $InstallDir),
    # 清掉早先留下的旧文件（安装目录里不该有它们）
    ('del /f /q "{0}\weasel_tip.previous.dll" 2>nul' -f $InstallDir),
    ('del /f /q "{0}\Weasel-Next-Engine.previous.exe" 2>nul' -f $InstallDir),
    ('del /f /q "{0}\weasel_tip.previous-*.dll" 2>nul' -f $InstallDir),
    ('reg add "{0}" /ve /t REG_SZ /d "{1}\Weasel-Next.dll" /f' -f $clsid, $InstallDir)
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
    Get-Process -Name 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer' -ErrorAction SilentlyContinue |
        ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
    Get-Process -Name 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer' -ErrorAction SilentlyContinue |
        ForEach-Object { try { $_.WaitForExit(3000) } catch { } }
    Start-Sleep -Seconds 3
    $env:WEASEL_TRACE_COMMIT = '1'
    # 用 WMI 启动：父进程是 WmiPrvSE，脱离调用方进程树。
    # 用 Start-Process / cmd start 会被调用方的会话回收，服务端随即消失（2026-10-10 故障总根源）。
    $broker = Join-Path $InstallDir 'Weasel-Next-Server.exe'
    $created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = "`"$broker`"" }
    if ($created.ReturnValue -ne 0) { Write-Warn2 ('WMI 启动失败，返回码 {0}' -f $created.ReturnValue) }
    else { Write-Ok ('已用 WMI 启动 broker（PID {0}）' -f $created.ProcessId) }
    Start-Sleep -Seconds 15
    # 三关校验：进程 / 命名管道 / 管道可连
    $missing = @()
    foreach ($name in 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer') {
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
    Get-Process -Name 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer' -ErrorAction SilentlyContinue |
        ForEach-Object { Write-Ok ("{0} PID {1}  {2}" -f $_.ProcessName, $_.Id, $_.Path) }
}

# 4) 校验 TIP 注册表
Write-Step '校验 TIP 注册表'
$value = (reg query $clsid /ve 2>$null | Where-Object { $_ -match 'REG_SZ' }) -join ''
$expect = Join-Path $InstallDir 'Weasel-Next.dll'
if ($value -match [regex]::Escape($expect)) { Write-Ok "指向安装目录 ✓ $expect" }
else { Write-Warn2 "仍是: $value" ; Write-Warn2 "期望: $expect（请重跑本脚本，或手动 reg add）" }

# 5) 清单
Write-Step '安装目录关键文件'
foreach ($f in 'Weasel-Next-Server.exe', 'Weasel-Next-Engine.exe', 'Weasel-Next-Renderer.exe', 'Weasel-Next.dll', 'rime.dll',
    'Rime-Next\sound\typewriter-key.wav', 'Rime-Next\sound\typewriter-bell.wav', 'weasel_theme_official.dll') {
    $p = Join-Path $InstallDir $f
    $mark = if (Test-Path $p) { '✓' } else { '✗' }
    $size = if (Test-Path $p) { '{0:N0} 字节' -f (Get-Item $p).Length } else { '缺失' }
    Write-Host ("   {0,-42} {1}  {2}" -f $f, $mark, $size)
}
$total = (Get-ChildItem $InstallDir -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
Write-Ok ('共 {0:N1} MB' -f ($total / 1MB))
# ★ 只打包 rime.lua 这个接口到**用户文件夹**（插件本体不进包 ✓ 用户自己放 ✓）
$luaSrc = Join-Path $PSScriptRoot '..\Rime-Next\plugin\rime.lua'   # ★ 打包源（按用户要求 ✓）
$luaDst = Join-Path $env:APPDATA 'Rime-Next\plugin'
if (Test-Path $luaSrc) {
    New-Item -ItemType Directory -Force -Path $luaDst | Out-Null
    Copy-Item $luaSrc $luaDst -Force
    Write-Host ('rime.lua 已安装 → ' + $luaDst)
}
