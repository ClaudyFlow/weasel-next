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
    [switch]$NoRestart,
    [switch]$ForceBackup,
    [switch]$BackupOnly,
    [switch]$ListBackups,
    [string]$Restore
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$stage = Join-Path $root 'target\release'   # 直接取编译输出（不再经 artifact\smoke，避免装到旧产物）

# ── 用户数据自动备份（学自蓝宝石 DictStore 的 backupNow / autoBackupIfDue）──
#
# 背景：这两天我们的清理/归类逻辑真的删过一次用户的日月 dict，也搬走过 rime.lua。
# 这里在每次部署动手之前，先把「会被动到的用户数据」打包一份，出事了能一键还原。
#
#   -BackupOnly     只备份，不部署（想单独做快照时用）
#   -ListBackups    列出已有备份
#   -Restore <名字> 从某个备份还原（名字就是目录名，例如 20261020-171530）
$userDir = Join-Path $env:APPDATA 'Rime-Next'
$backupRoot = Join-Path $userDir 'backup'
$backupKeep = 10          # 只留最近 10 份
$backupDueHours = 6       # 距上次备份超过 6 小时才自动做（可 -ForceBackup 强制）

function Get-BackupFiles {
    param([string]$Dir)
    $paths = @()
    foreach ($pattern in '*.dict.yaml', '*.schema.yaml', 'rime.lua', 'user.yaml', 'weasel.yaml', 'weasel.custom.toml', 'symbols.yaml', 'schema.toml') {
        # ⚠️ 必须带 -Force：根部的日月/宇浩词库带 Hidden+System（部署脚本 2.7 步留下的），
        # 不带 -Force 数不出来 → 备份会**静默漏掉**最该备份的那批词库
        # （2026-10-20 实际踩到：备份只报了 59 个文件，8 个根部 dict 一个都没进去）。
        $paths += Get-ChildItem $Dir -File -Filter $pattern -Force -ErrorAction SilentlyContinue
    }
    $paths += Get-ChildItem (Join-Path $Dir 'schema') -Recurse -File -Force -Include '*.schema.yaml', '*.dict.yaml', 'schema.toml' -ErrorAction SilentlyContinue
    # 用户补丁 2026-10-21 起在用户目录根（`weasel.yaml`，上面那行已收）；
    # 这里是它上半场的中途落点 `weasel\weasel.yaml`，还没迁走的机器也要备份到。
    # 用 -Filter 而不是 -Include —— -Include 不加 -Recurse 不生效。
    $paths += Get-ChildItem (Join-Path $Dir 'weasel') -File -Force -Filter '*.yaml' -ErrorAction SilentlyContinue
    return $paths | Sort-Object FullName -Unique
}

function Invoke-UserBackup {
    param([string]$Reason = 'auto')
    if (-not (Test-Path $userDir)) { return }
    $files = Get-BackupFiles -Dir $userDir
    if (-not $files -or $files.Count -eq 0) { return }
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $dest = Join-Path $backupRoot $stamp
    New-Item -ItemType Directory -Path $dest -Force | Out-Null
    $manifest = @()
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($userDir.Length).TrimStart('\')
        $target = Join-Path $dest $relative
        $parent = Split-Path -Parent $target
        if ($parent -and -not (Test-Path $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
        Copy-Item $file.FullName $target -Force
        $manifest += ('{0}`t{1}' -f $relative, (Get-FileHash $file.FullName -Algorithm SHA256).Hash)
    }
    Set-Content (Join-Path $dest 'MANIFEST.txt') -Value ($manifest -join "`r`n") -Encoding UTF8
    Set-Content (Join-Path $dest 'REASON.txt') -Value $Reason -Encoding UTF8
    # 只保留最近 $backupKeep 份
    $all = Get-ChildItem $backupRoot -Directory | Sort-Object Name -Descending
    if ($all.Count -gt $backupKeep) {
        $all | Select-Object -Skip $backupKeep | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    }
    Write-Host ("用户数据已备份：{0}（{1} 个文件，原因：{2}）" -f $stamp, $files.Count, $Reason)
}

if ($ListBackups) {
    if (Test-Path $backupRoot) {
        Get-ChildItem $backupRoot -Directory | Sort-Object Name -Descending | ForEach-Object {
            $count = (Get-ChildItem $_.FullName -Recurse -File | Where-Object { $_.Name -ne 'MANIFEST.txt' -and $_.Name -ne 'REASON.txt' }).Count
            $reason = Get-Content (Join-Path $_.FullName 'REASON.txt') -ErrorAction SilentlyContinue
            '{0}   {1} 个文件   {2}' -f $_.Name, $count, $reason
        }
    } else { '还没有任何备份。' }
    return
}

if ($Restore) {
    $src = Join-Path $backupRoot $Restore
    if (-not (Test-Path $src)) { throw "找不到备份：$src（用 -ListBackups 看可用的）" }
    $files = Get-ChildItem $src -Recurse -File | Where-Object { $_.Name -ne 'MANIFEST.txt' -and $_.Name -ne 'REASON.txt' }
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($src.Length).TrimStart('\')
        $target = Join-Path $userDir $relative
        $parent = Split-Path -Parent $target
        if ($parent -and -not (Test-Path $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
        Copy-Item $file.FullName $target -Force
    }
    Write-Host ("已从备份 {0} 还原 {1} 个文件到用户目录" -f $Restore, $files.Count)
    return
}

# 自动备份：距上次超过 $backupDueHours 才做，避免每次部署都堆一份
$shouldBackup = $ForceBackup
if (-not $shouldBackup -and (Test-Path $backupRoot)) {
    $latest = Get-ChildItem $backupRoot -Directory | Sort-Object Name -Descending | Select-Object -First 1
    if (-not $latest) { $shouldBackup = $true }
    elseif ($latest.LastWriteTime -lt (Get-Date).AddHours(-$backupDueHours)) { $shouldBackup = $true }
} elseif (-not $shouldBackup) { $shouldBackup = $true }
if ($shouldBackup) { Invoke-UserBackup -Reason 'deploy' }
if ($BackupOnly) { Write-Host '仅备份模式：已生成快照，未做任何部署。'; return }


# ★ 第一步就重新生成双拼方案：把 `parser\shuangpin\*.parser.yaml` 的键位"烤"进 `.schema.yaml`。
#
# 必须放在部署/编译**之前** —— 否则方案生成晚于部署，这次部署编译的还是旧内容。
# 生成器自己会：优先读用户目录的 parser 数据、写到用户目录的 schema，**不需要提权**。
# 用户明确要求："不需要内置小鹤，用户文件夹有就行" —— 所以仓库里**不放**方案文件，
# 全靠这一步在用户机器上生成。
$generatorEarly = Join-Path $stage 'shuangpin-schema.exe'
if (Test-Path $generatorEarly) {
    Write-Host '重新生成双拼方案（TOML → schema.yaml）...'
    & $generatorEarly
    if ($LASTEXITCODE -ne 0) { Write-Warn2 '双拼方案生成失败（TOML 语法错？）' }
} else {
    Write-Host ('跳过双拼生成：找不到 ' + $generatorEarly)
}

$clsid = 'HKLM\Software\Classes\CLSID\{16A7AEA9-9EE6-4540-8020-E384C0489BB1}\InprocServer32'
# 32 位视图。⚠️ 这个 CLSID 的**字面量不能直接写进 `-f` 的格式串**：
# PowerShell 会把 `{16A7AEA9-...}` 当成格式化占位符，报
# "Index (zero based) must be greater than or equal to zero..."（2026-10-21 实测踩到，
# 而且失败发生在拼命令阶段 —— 栈已经停了、提权脚本根本没跑）。
# 所以一律走变量 + {1} 占位。
$clsid32 = 'HKLM\SOFTWARE\Classes\WOW6432Node\CLSID\{16A7AEA9-9EE6-4540-8020-E384C0489BB1}\InprocServer32'

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
# ⚠️ 设置界面也要停：`Weasel-Next-Setting.exe` 同样来自安装目录，
# 开着它部署 → robocopy 对**被占用的**文件只报 ERROR 32 就跳过，
# 结果是"部署显示成功、设置界面还是旧版"（2026-10-17 实际踩到：/style 页面白改一场）。
$running = @(Get-Process -Name 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer', 'Weasel-Next-Setting' -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    $running | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
    for ($i = 0; $i -lt 40; $i++) {
        $left = @(Get-Process -Name 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer', 'Weasel-Next-Setting' -ErrorAction SilentlyContinue)
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
    ('robocopy "{0}" "{1}" /E /XD deps build incremental .fingerprint examples /XF librime-next.exe *.old-* *.previous* *.log *.d *.rlib *.rmeta *.pdb /NFL /NDL /NJH /NJS /R:1 /W:1' -f $stage, $InstallDir),
    # 早先被整体复制进去的构建目录要清掉（安装目录只放运行时文件）。
    # 注意：批处理里循环变量是 %%D；这里干脆一个目录一条命令，避免再次踩坑。
    ('if exist "{0}\deps" rmdir /s /q "{0}\deps"' -f $InstallDir),
    ('if exist "{0}\build" rmdir /s /q "{0}\build"' -f $InstallDir),
    ('if exist "{0}\incremental" rmdir /s /q "{0}\incremental"' -f $InstallDir),
    ('if exist "{0}\.fingerprint" rmdir /s /q "{0}\.fingerprint"' -f $InstallDir),
    ('if exist "{0}\examples" rmdir /s /q "{0}\examples"' -f $InstallDir),
    # 主题目录改名（复数 → 单数）：老机器上的 themes\ 清掉，新代码只认 theme\。
    ('if exist "{0}\themes" rmdir /s /q "{0}\themes" 2>nul' -f $InstallDir),
    ('robocopy "{0}\Rime-Next" "{1}\Rime-Next" /E /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    ('robocopy "{0}\tool\7z" "{1}\7z" /E /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    ('robocopy "{0}\target\release" "{1}\theme" weasel_theme_*.dll /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    ('robocopy "{0}\Rime-Next\sound" "{1}\Rime-Next\sound" /E /NFL /NDL /NJH /NJS /R:1 /W:1' -f $root, $InstallDir),
    # 方案清单已改成 librime 直读 schema.toml：清掉老机器上生成的这份中转 YAML
    ('del /f /q "%APPDATA%\Rime-Next\default.custom.yaml" 2>nul'),
    # TIP 的 cargo 产物叫 weasel_tip.dll，对外/注册的名字是 Weasel-Next.dll。
    # 必须在这里刷新对外名，否则注册表指向的那个 DLL 永远是上一版的（改动看着"没生效"）。
    # 它被每个开着输入法的进程加载着 → 原地覆盖必然失败；照安装包的做法先改名再放新的
    # （NTFS 允许给已加载的 DLL 改名），旧文件留成 weasel_tip.old.*.dll 等进程退出后再删。
    ('if exist "{1}\Weasel-Next.dll" ren "{1}\Weasel-Next.dll" "weasel_tip.old.%RANDOM%%RANDOM%.dll"' -f $root, $InstallDir),
    ('copy /y "{0}\target\release\weasel_tip.dll" "{1}\Weasel-Next.dll"' -f $root, $InstallDir),
    ('del /f /q "{1}\weasel_tip.old.*.dll" 2>nul' -f $root, $InstallDir),
    # 32 位 TIP 同理，但更麻烦：它**没有**未加载的中间名（宿主直接加载 Weasel-Next-32.dll），
    # 所以上面那条 robocopy 一旦撞上开着的 WPS 就只报 ERROR 32 跳过，结果是
    # "部署报成功、32 位还是旧版"。这里照样先改名再放新的。
    ('if exist "{1}\Weasel-Next-32.dll" ren "{1}\Weasel-Next-32.dll" "weasel_tip32.old.%RANDOM%%RANDOM%.dll"' -f $root, $InstallDir),
    ('if exist "{0}\target\release\Weasel-Next-32.dll" copy /y "{0}\target\release\Weasel-Next-32.dll" "{1}\Weasel-Next-32.dll"' -f $root, $InstallDir),
    ('del /f /q "{1}\weasel_tip32.old.*.dll" 2>nul' -f $root, $InstallDir),
    # rime.dll 由 artifact\smoke 提供（不在 target\release 里）——必须显式搬，否则永远装旧版
    ('copy /y "{0}\artifact\smoke\rime.dll" "{1}\rime.dll"' -f $root, $InstallDir),
    # punctuation.yaml 已并入 symbols.yaml：把安装目录里那份也清掉
    ('del /f /q "{1}\Rime-Next\punctuation.yaml" 2>nul' -f $root, $InstallDir),
    # 宿主配置：**唯一格式是 YAML**（weasel.yaml）—— style::parse_file 只认 YAML，
    # 配置树与序列化也统一成 YAML。TOML/JSON 既不读也不写，安装目录里的旧文件一并清掉。
    # 删 TOML 前先确认 yaml 已就位（fail-closed：copy 万一失败时宁可留着旧文件）。
    # 注意新版本**没有迁移层**：没有 weasel.yaml 时程序直接用内嵌出厂默认值。
    ('copy /y "{0}\Rime-Next\weasel.yaml" "{1}\weasel.yaml"' -f $root, $InstallDir),
    ('del /f /q "{1}\weasel.json" 2>nul' -f $root, $InstallDir),
    ('if exist "{1}\weasel.yaml" del /f /q "{1}\weasel.toml" 2>nul' -f $root, $InstallDir),
    # 清掉 TOML 时代留下的同名换后缀文件（现在的正式格式是 .yaml）。
    # ⚠️ 这里曾经写反过：删的是 `*.theme.yaml` / `*.eassy.yaml`，也就是**本次要装的新格式**，
    # 每次部署都把刚装上的 YAML 主题与词库删掉。改成删对应的旧 `.toml`（2026-10-27）。
    ('del /f /q "{1}\Rime-Next\theme\default\default.theme.toml" 2>nul' -f $root, $InstallDir),
    ('del /f /q "{1}\Rime-Next\eassy\default\default.eassy.toml" 2>nul' -f $root, $InstallDir),
    # 清理已迁移的旧扁平 yaml：源已放进 schema\<方案>\，目标里残留的扁平副本要删掉
    ('for %%F in ("{1}\Rime-Next\*.schema.yaml" "{1}\Rime-Next\*.dict.yaml") do if exist "%%F" del /f /q "%%F"' -f $root, $InstallDir),
    # rime.lua 已按用户要求收进 plugin\（根部禁止）：删掉程序文件夹里遗留的根部副本，
    # 否则用户目录那份一旦缺失，librime 会静默回退到这份**过期**的共享根部文件。
    ('del /f /q "{1}\Rime-Next\rime.lua" 2>nul' -f $root, $InstallDir),
    # 清掉早先留下的旧文件（安装目录里不该有它们）
    ('del /f /q "{0}\weasel_tip.previous.dll" 2>nul' -f $InstallDir),
    # 2026-10-23：`librime-next` 早先叫 `rime-config`，被整目录 robocopy 漏进安装目录；
    # 名字换了，旧的那份要清掉（新名字已在 robocopy 的 /XF 里排除，不进安装目录）。
    ('del /f /q "{0}\rime-config.exe" 2>nul' -f $InstallDir),
    ('del /f /q "{0}\Weasel-Next-Engine.previous.exe" 2>nul' -f $InstallDir),
    ('del /f /q "{0}\weasel_tip.previous-*.dll" 2>nul' -f $InstallDir),
    # 改名前的旧二进制：`WeaselNextService` 计划任务启动的是 weasel-broker.exe。
    # 直接删掉会让计划任务下次登录起不来（实测 schtasks /Change 在本机也被拒），
    # 所以这里用**新服务端的副本**占住这个名字 —— 托盘菜单由新二进制提供，不再是旧菜单；
    # 另外三个（weasel-server / weasel-settings / weasel-renderer）彻底删掉。
    ('copy /y "{0}\Weasel-Next-Server.exe" "{0}\weasel-broker.exe"' -f $InstallDir),
    ('del /f /q "{0}\weasel-server.exe" 2>nul' -f $InstallDir),
    ('del /f /q "{0}\weasel-settings.exe" 2>nul' -f $InstallDir),
    ('del /f /q "{0}\weasel-renderer.exe" 2>nul' -f $InstallDir),
    # 64 位 TIP 注册
    ('reg add "{0}" /ve /t REG_SZ /d "{1}\Weasel-Next.dll" /f' -f $clsid, $InstallDir),
    # 32 位 TIP：32 位宿主（WPS、部分旧软件）只会去 WOW6432Node 找 InprocServer32。
    # 先用 SysWOW64 的 regsvr32 让 DLL 自注册（DllRegisterServer 在 32 位进程里写的
    # HKCR 会自动落到 WOW6432Node），再显式补一条 InprocServer32 —— 自注册万一失败，
    # 也不至于让 32 位程序彻底没有输入法。
    ('if exist "{0}\Weasel-Next-32.dll" "%SystemRoot%\SysWOW64\regsvr32.exe" /s "{0}\Weasel-Next-32.dll"' -f $InstallDir),
    ('if exist "{0}\Weasel-Next-32.dll" reg add "{1}" /ve /t REG_SZ /d "{0}\Weasel-Next-32.dll" /f' -f $InstallDir, $clsid32),
    ('if exist "{0}\Weasel-Next-32.dll" reg add "{1}" /v ThreadingModel /t REG_SZ /d "Apartment" /f' -f $InstallDir, $clsid32)
)
$log = Invoke-ElevatedScript -Lines $elevated
# ⚠️ 判据别再找 'DONE'：结束标记是**写进 .done 文件**的，日志里只有 `SYNC_EXIT=<code>`。
# 以前写成 `$log -notmatch 'DONE'` → 每次部署都假报「提权脚本未正常结束」，
# 而实际上 SYNC_EXIT=0、hash 核对也全过（2026-10-20 实测，白紧张一次）。
if ($log -notmatch 'SYNC_EXIT=(\d+)') {
    Write-Warn2 '提权脚本没有回传结束标记（UAC 未确认或脚本中途被杀？），请检查上面的输出'
} elseif ($Matches[1] -ne '0') {
    Write-Warn2 ('提权脚本退出码 ' + $Matches[1] + '，请检查上面的输出')
}
Write-Ok '文件已同步'

# 2.5) 语言模型不再由本仓库分发：压缩包与解压步骤已移除。
#      模型改由外部获取（下载或从既有安装同步）后直接放进 <INSTDIR>\Rime-Next\model\；
#      缺失时只影响语言模型打分，不阻断部署与打字。

# 2.6) ★ 强制 rime 部署：编译词典 → 生成 %APPDATA%\Rime-Next\build\<方案>\*.bin
#
# 为什么必须单独做：robocopy 只同步二进制，**不触发 librime 的词典编译**。
# 2026-10-18 实测踩到两次：
#   * 删掉 build\ 后本脚本仍报「全部与本次构建一致 ✓」，但 build\ 是空的 → 切方案没候选；
#   * 新加 wubi86 方案后同样不编译，必须手动跑 `Engine.exe --deploy` 才出 table.bin。
# 所以把"编译"这一步写进部署流程，并逐方案校验 prism.bin 是否真的生成。
if (-not $NoRestart) {
    Write-Step '编译词典（rime deploy）'
    $engineExe = Join-Path $InstallDir 'Weasel-Next-Engine.exe'
    if (Test-Path $engineExe) {
        Get-Process -Name 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer' -ErrorAction SilentlyContinue |
            ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
        Start-Sleep -Seconds 2
        # --deploy 会被「常驻引擎的单实例」挡住，所以上面必须先停栈（坑 14）
        $deployProc = Start-Process $engineExe -ArgumentList '--deploy' -PassThru -WindowStyle Hidden
        [void]$deployProc.WaitForExit(180000)
        Write-Ok ('rime deploy 退出码 ' + $deployProc.ExitCode)

        # 校验：schema.toml 里登记的每个方案都应有 build\<方案>\<方案>.prism.bin
        $schemaToml = Join-Path $env:APPDATA 'Rime-Next\schema\schema.toml'
        $schemes = @()
        if (Test-Path $schemaToml) {
            $schemes = [regex]::Matches((Get-Content $schemaToml -Raw), '"([a-zA-Z0-9_]+)"') |
                ForEach-Object { $_.Groups[1].Value } | Select-Object -Unique
        }
        # 纯 lua 方案（如虎整句 tiger_sentence）没有 dictionary，本来就不产 prism：
        # 判据是源 schema 里有没有 `dictionary:` 行，不能一刀切要求 prism.bin。
        $schemeRoot = Join-Path $env:APPDATA 'Rime-Next\schema'
        $dictSchemes = @()
        $luaSchemes = @()
        foreach ($scheme in $schemes) {
            $source = Join-Path $schemeRoot ("{0}\{0}.schema.yaml" -f $scheme)
            $needsDict = $true
            if (Test-Path $source) {
                $needsDict = [bool](Select-String -Path $source -Pattern '^\s*dictionary:' -Quiet)
            }
            if ($needsDict) { $dictSchemes += $scheme } else { $luaSchemes += $scheme }
        }
        $missingPrism = @()
        foreach ($scheme in $dictSchemes) {
            $prism = Join-Path $env:APPDATA ('Rime-Next\build\{0}\{0}.prism.bin' -f $scheme)
            if (-not (Test-Path $prism)) { $missingPrism += $scheme }
        }
        if ($missingPrism.Count -eq 0) {
            $luaNote = if ($luaSchemes.Count -gt 0) { '，另有 {0} 个纯 lua 方案不需要 prism' -f $luaSchemes.Count } else { '' }
            Write-Ok ('词典编译产物完整（{0} 个方案{1}）' -f $dictSchemes.Count, $luaNote)
        } else {
            Write-Warn2 ('这些方案的词典没编出来: ' + ($missingPrism -join ', '))
            Write-Warn2 '常见原因：方案的 dict.yaml 缺失/格式不对，或引用了不存在的词典（一个方案失败会让整批 deploy 失败）。'
        }
    }
}

$stagingDir = Join-Path $env:APPDATA 'Rime-Next\build'

# 2.7) build 布局整理：把 <方案>.schema.yaml 收进 <方案>\，扁平位置留隐藏硬链接
#
# 为什么要这么做：rime（librime 的 SaveOutputPlugin）会把展开后的方案配置写成
# build\<方案>.schema.yaml（扁平），而运行时也按扁平路径读取 —— 这是 C++ 里写死的。
# 直接挪走会 "nonexistent config file"、方案全部消失（实测过两次）。
# 这里采用折中：真实文件放 build\<方案>\<方案>.schema.yaml，扁平位置建**硬链接**
# 并加 +h +s 属性 → 资源管理器里只看到文件夹，rime 仍能按原路径读到。
if (Test-Path $stagingDir) {
    $moved = 0
    Get-ChildItem $stagingDir -File -Filter '*.schema.yaml' | ForEach-Object {
        $id = $_.BaseName -replace '\.schema$', ''
        $folder = Join-Path $stagingDir $id
        $target = Join-Path $folder ($_.Name)
        if (-not (Test-Path $folder)) { New-Item -ItemType Directory -Path $folder -Force | Out-Null }
        # 真实文件搬到子目录（内容以大的那份为准：子目录已有就留子目录那份）
        if (-not (Test-Path $target)) {
            Move-Item $_.FullName $target -Force
        } else {
            # 2026-10-24 修：扁平那份常是指向子目录文件的**硬链接**（同一 inode），
            # Copy-Item 会报 `Cannot overwrite the item ... with itself`，整个部署在此失败。
            # 按上面那句约定（"子目录已有就留子目录那份"），这里不复制 —— 下面只删扁平这份
            # （删一个硬链接不影响子目录里的真实文件）。
            Remove-Item $_.FullName -Force
        }
        # 扁平位置建硬链接（同一份数据），再加隐藏+系统属性
        $flat = Join-Path $stagingDir ($id + '.schema.yaml')
        if (-not (Test-Path $flat)) {
            cmd /c mklink /H "$flat" "$target" *> $null
        }
        if (Test-Path $flat) {
            (Get-Item $flat -Force).Attributes = 'Hidden, System, Archive'
        }
        $moved++
    }
    if ($moved -gt 0) {
        Write-Ok ("build 布局整理：{0} 份方案配置收进文件夹，扁平位置留隐藏硬链接" -f $moved)
    }

    # 顺手把露在根下的编译产物（*.reverse.bin / *.table.bin / *.prism.bin）归位。
    # 这些 rime 本来就是按 build\<名字>\<名字>.<类型>.bin 读的，扁平那份只是旧部署残留，
    # 直接搬进文件夹即可（不需要硬链接）。
    $binMoved = 0
    foreach ($suffix in '.reverse.bin', '.table.bin', '.prism.bin') {
        Get-ChildItem $stagingDir -File -Filter ('*' + $suffix) | ForEach-Object {
            $stem = $_.Name.Substring(0, $_.Name.Length - $suffix.Length)
            if ([string]::IsNullOrWhiteSpace($stem)) { return }
            $folder = Join-Path $stagingDir $stem
            $target = Join-Path $folder $_.Name
            if (-not (Test-Path $folder)) { New-Item -ItemType Directory -Path $folder -Force | Out-Null }
            if (Test-Path $target) {
                Remove-Item $_.FullName -Force
            } else {
                Move-Item $_.FullName $target -Force
            }
            $binMoved++
        }
    }
    if ($binMoved -gt 0) {
        Write-Ok ("编译产物归位：{0} 个 .bin 移进各自方案文件夹" -f $binMoved)
    }
}

# 2.8) ★ 清理旧版部署记录 `deploy.toml`
#
# 用户 2026-10-20 明确要求：**根目录不许再生成 deploy 这种文件**。
# 新版 `common/src/deploy_manifest.rs` 的 `STATE_FILE` 已改成 `user.yaml`
# （状态作为带标记的 YAML 块并进去，不再单开文件）；但**装到 Program Files 的旧二进制
# 仍会写 `deploy.toml`** —— 所以这里做一次性清理，等新二进制部署后就不会再出现。
$legacyDeployToml = Join-Path $env:APPDATA 'Rime-Next\deploy.toml'
if (Test-Path $legacyDeployToml) {
    Remove-Item $legacyDeployToml -Force -ErrorAction SilentlyContinue
    Write-Ok ('已删除旧版部署记录（根目录不再生成）：' + $legacyDeployToml)
}

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

# 32 位视图（WPS 这类 32 位宿主走这里）
$clsid32 = 'HKLM\SOFTWARE\Classes\WOW6432Node\CLSID\{16A7AEA9-9EE6-4540-8020-E384C0489BB1}\InprocServer32'
$value32 = (reg query $clsid32 /ve 2>$null | Where-Object { $_ -match 'REG_SZ' }) -join ''
$expect32 = Join-Path $InstallDir 'Weasel-Next-32.dll'
if ($value32 -match [regex]::Escape($expect32)) { Write-Ok "32 位指向安装目录 ✓ $expect32" }
else { Write-Warn2 "32 位仍是: $value32" ; Write-Warn2 "期望: $expect32（请重跑本脚本）" }

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

# 5.5) ★ 事后核对：提权那步失败时上面照样会打印"文件已同步"，
# 所以**必须**比 hash/mtime 才知道到底换没换。2026-10-17 被这个假象骗过两次：
# 脚本报成功、退出码 0，而安装目录的 Weasel-Next-Setting.exe 一整天都是旧的。
Write-Step '核对文件是否真的换成了本次构建'
$mismatched = @()
# ⚠️ 这个列表必须**覆盖所有会被复制的东西**，漏一个就会假报"全部一致"。
# 2026-10-17 实际踩到：漏了 `rime.dll`，于是脚本报"全部与本次构建一致 ✓"，
# 而安装目录里的 rime.dll 还是旧版（10,088,960 vs 新构建 10,091,520），
# 导致"改了 librime 却没生效"又白查一轮。
$stagePairs = @(
    @{ Src = 'Weasel-Next-Server.exe';        Dst = 'Weasel-Next-Server.exe' },
    @{ Src = 'Weasel-Next-Engine.exe';        Dst = 'Weasel-Next-Engine.exe' },
    @{ Src = 'Weasel-Next-Renderer.exe';      Dst = 'Weasel-Next-Renderer.exe' },
    @{ Src = 'Weasel-Next-Setting.exe';       Dst = 'Weasel-Next-Setting.exe' },
    @{ Src = 'weasel_theme_official.dll';     Dst = 'weasel_theme_official.dll' },
    @{ Src = 'weasel_tip.dll';                Dst = 'Weasel-Next.dll' },
    # rime.dll 从 artifact\smoke 复制（不在 target\release 里）
    @{ Src = '..\artifact\smoke\rime.dll';    Dst = 'rime.dll' }
)
foreach ($pair in $stagePairs) {
    $src = Join-Path $stage $pair.Src
    $dst = Join-Path $InstallDir $pair.Dst
    if (-not (Test-Path $src)) { continue }
    $same = (Test-Path $dst) -and
            ((Get-FileHash $src -Algorithm MD5).Hash -eq (Get-FileHash $dst -Algorithm MD5).Hash)
    if (-not $same) { $mismatched += $pair.Dst }
}
if ($mismatched.Count -eq 0) {
    Write-Ok '全部与本次构建一致 ✓'
} else {
    Write-Warn2 ('以下文件**没有**换成本次构建：' + ($mismatched -join ', '))
    Write-Warn2 '原因通常是提权没被确认（UAC 点了取消），或文件被运行中的进程占用。'
    Write-Warn2 '请重跑本脚本，并在 UAC 弹窗上点"是"。'
    throw '部署未真正生效：安装目录里还有旧文件（比对 hash 失败）'
}

# ★ rime.lua（lua 引导入口）统一放 **plugin\ 子目录**；用户 2026-10-20 明确要求**根部不许放**。
#
# 依据：librime-lua 的 `plugins/lua/src/modules.cc` 有 Weasel-Next 补丁，搜索顺序是
#   用户根 → 用户 plugin → 共享根 → 共享 plugin
# 所以 `plugin\rime.lua` 能被正常加载（2026-10-20 实测：放 plugin\ 后引擎重启，
# `plugin\_boot.log` 照样出现「=== rime.lua 开始加载 ===」和「虎整句已注册」）。
$luaPluginDir = Join-Path $env:APPDATA 'Rime-Next\plugin'
$luaDst = Join-Path $luaPluginDir 'rime.lua'
$luaRoot = Join-Path $env:APPDATA 'Rime-Next\rime.lua'
New-Item -ItemType Directory -Force -Path $luaPluginDir | Out-Null

# 1) 根部那份历史遗留：plugin 里没有就**搬**过去，有就删掉 —— 根部禁止保留
if (Test-Path $luaRoot) {
    if (-not (Test-Path $luaDst)) {
        Move-Item $luaRoot $luaDst -Force
        Write-Host ('rime.lua 已从根部搬到 plugin\ → ' + $luaDst)
    } else {
        Remove-Item $luaRoot -Force
        Write-Host ('根部多余的 rime.lua 已删除（plugin\ 那份优先）→ ' + $luaRoot)
    }
}

# 2) 缺失时才从仓库装一份（**不覆盖**用户/插件包已经改过的那份）
$luaSrc = Join-Path $PSScriptRoot '..\Rime-Next\plugin\rime.lua'
if ((Test-Path $luaSrc) -and -not (Test-Path $luaDst)) {
    Copy-Item $luaSrc $luaDst -Force
    Write-Host ('rime.lua 已安装 → ' + $luaDst)
} elseif (Test-Path $luaDst) {
    # ⚠️ 不能无条件"保留用户那份"：本项目的旧版 rime.lua 只把「用户目录\plugin」加进
    # package.path，而安装包把 yuhao / tiger / input_statistics 放在**程序文件夹**。
    # 新装或换机的机器上 require("yuhao.…") 找不到文件 ⇒ librime-lua 把
    # lua_processor@*yuhao.… 组件**静默丢弃** ⇒ 宇浩的「自分割」没了（2026-10-23 反馈）。
    # 所以：认得是自己的旧版（有标记注释、但没有程序文件夹搜索路径）就升级并留备份；
    # 别人家的 rime.lua 不覆盖，只提示。
    $existing = Get-Content $luaDst -Raw -Encoding UTF8
    $isOurs = $existing -match 'rime-lua 引导入口'
    $hasSharedPath = $existing -match 'bootstrap-v2'
    if ($isOurs -and -not $hasSharedPath) {
        $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
        Copy-Item $luaDst ($luaDst + '.bak-' + $stamp) -Force
        Copy-Item $luaSrc $luaDst -Force
        Write-Host ('rime.lua 是旧版（缺程序文件夹搜索路径）→ 已升级，旧的存为 rime.lua.bak-' + $stamp)
    } elseif ($isOurs) {
        Write-Host ('rime.lua 已是最新（含程序文件夹搜索路径），保留 → ' + $luaDst)
    } else {
        Write-Host ('rime.lua 不是本项目的版本，未覆盖 → ' + $luaDst)
    }
}

# ★ 自定义双拼方案 → **用户文件夹**（用户要能改；程序文件夹是只读的，写进去等于不能用）
#
# 为什么不是只放程序文件夹：
#   * 程序文件夹需要提权才能改，普通用户改不了；
#   * 重装会被覆盖，用户改的键位就没了；
#   * `list_schemas` 同名时**用户那份优先**，所以真正生效的是用户目录这份。
# 程序文件夹里也留一份作为"出厂备份"（删坏了能恢复），但**要改的是用户目录这份**。
$shuangpinSrc = Join-Path $PSScriptRoot '..\Rime-Next\schema'
$userSchemaDir = Join-Path $env:APPDATA 'Rime-Next\schema'
if (Test-Path $shuangpinSrc) {
    $copied = @()
    foreach ($dir in Get-ChildItem $shuangpinSrc -Directory -ErrorAction SilentlyContinue) {
        # 只同步"双拼类"方案。
        # ⚠️ 判断用目录名，别拿方案内容去 `-match` —— 双拼表里有
        # `dict = "luna_pinyin"`，那样会把 luna_pinyin / luna_pinyin_simp 也误判成自定义方案
        # （实测确实复制了 3 个）。双拼的 schema_id 一律以 `double_pinyin` 开头。
        if ($dir.Name -notlike 'double_pinyin*') { continue }
        $dst = Join-Path $userSchemaDir $dir.Name
        New-Item -ItemType Directory -Force -Path $dst | Out-Null
        Copy-Item (Join-Path $dir.FullName '*') $dst -Recurse -Force
        $copied += $dir.Name
    }
    if ($copied.Count -gt 0) {
        Write-Host ('自定义方案已安装到用户文件夹 → ' + $userSchemaDir)
        Write-Host ('   ' + ($copied -join ', '))
    }
}

# parser/*.parser.yaml 不再拷到用户文件夹：parser 是出厂数据，随安装包进程序
# 文件夹（Rime-Next\parser\，payload 整目录打包），引擎读程序文件夹那份。

# ★ 重新生成双拼方案：把 `parser\shuangpin\*.parser.yaml` 的键位"烤"进 `.schema.yaml`。
#
# 为什么必须在这里调：librime 只认 `.schema.yaml`，它没有任何机制在运行时读 parser 键位表。
# 所以"改键位 → 生效"必须经过这一步转换；放在部署流程里，用户点一次部署就完成了。
# 生成器自己会：优先读用户目录的 parser 数据、写到用户目录的 schema，**不需要提权**。
$generator = Join-Path $stage 'shuangpin-schema.exe'
if (Test-Path $generator) {
    Write-Host '重新生成双拼方案（TOML → schema.yaml）...'
    & $generator
    if ($LASTEXITCODE -ne 0) {
        Write-Warn2 '双拼方案生成失败，上面有具体原因（TOML 语法错？）'
    }
} else {
    Write-Host ('跳过双拼生成：找不到 ' + $generator)
}

