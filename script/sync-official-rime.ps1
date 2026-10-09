<#
.SYNOPSIS
    把官方小狼毫（C++ 版）的数据与用户配置合并到 weasel-rs。

.DESCRIPTION
    做两件事，都只"补齐/覆盖"，**从不删除**任何文件：

    A) 共享数据：<官方安装目录>\data  →  <weasel-rs 程序目录>\rime-data
       weasel-rs 的 librime 把共享数据目录设为 <程序目录>\rime-data（见
       server/src/rime/mod.rs），官方那 96 个文件（方案、词典、opencc、preview）里的
       缺失项就在这里补上——例如 sancang/cangjie5 需要的 cangjie5.*.dict.yaml。

    B) 用户数据：%APPDATA%\Rime  →  %APPDATA%\Weasel-RS\Rime
       补齐 default.custom.yaml（决定启用哪些方案）、weasel.custom.yaml、images\、
       可选的 *.userdb（用户词典）；installation.yaml / user.yaml 会**先备份**再覆盖。
       weasel-rs 自己的文件（weasel.custom.json、.weasel.lock、build\）不会被改动。

    合并完成后需要重新部署才会生效：
        <weasel-rs 程序目录>\weasel-server.exe --deploy --silent

.PARAMETER OfficialInstall
    官方小狼毫安装目录，默认 C:\Program Files\Rime\weasel-0.17.4。

.PARAMETER OfficialUserData
    官方用户数据目录，默认 %APPDATA%\Rime。

.PARAMETER TargetExeDir
    weasel-rs 程序目录（含 rime.dll / weasel-server.exe），默认 artifact\smoke。

.PARAMETER IncludeUserDb
    连同 *.userdb 用户词典一起复制。官方 WeaselServer 正在运行时会持有 leveldb 文件锁，
    被占用的文件会跳过并提示，此时请先退出官方输入法再重跑。

.PARAMETER DryRun
    只列出将要复制的文件，不实际写入。

.PARAMETER Deploy
    合并后自动重新部署方案。注意：server 常驻时会持有单实例锁，此时直接运行
    `weasel-server.exe --deploy` 会失败（这是 server/src/main.rs 的设计）。本开关按
    应用自己的顺序执行：先 `weasel-broker.exe --shutdown` 停服务，再 `--deploy --silent`，
    最后重新启动 broker（它会拉起 server 与 renderer）。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File script\sync-official-rime.ps1 -DryRun
    powershell -ExecutionPolicy Bypass -File script\sync-official-rime.ps1 -IncludeUserDb -Deploy
#>
[CmdletBinding()]
param(
    [string]$OfficialInstall = 'C:\Program Files\Rime\weasel-0.17.4',
    [string]$OfficialUserData,
    [string]$TargetExeDir,
    [string]$TargetUserData,
    [switch]$IncludeUserDb,
    [switch]$DryRun,
    [switch]$Deploy
)

# 默认值在脚本体内计算：PS 5.1 在 param 块求值时 $PSScriptRoot 尚未赋值。
$scriptRoot = $PSScriptRoot
if (-not $scriptRoot) { $scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $OfficialUserData) { $OfficialUserData = Join-Path $env:APPDATA 'Rime' }
if (-not $TargetExeDir) { $TargetExeDir = Join-Path (Split-Path -Parent $scriptRoot) 'artifact\smoke' }
if (-not $TargetUserData) { $TargetUserData = Join-Path $env:APPDATA 'Weasel-RS\Rime' }

$ErrorActionPreference = 'Continue'
$script:copied = 0
$script:skipped = 0
$script:failed = 0

function Copy-Tree {
    param(
        [Parameter(Mandatory)][string]$Source,
        [Parameter(Mandatory)][string]$Destination,
        [Parameter(Mandatory)][string]$Label,
        [string[]]$Exclude = @(),
        [string[]]$Only = @()
    )
    Write-Host "== $Label ==" -ForegroundColor Cyan
    Write-Host "   $Source  ->  $Destination"
    if (-not (Test-Path -LiteralPath $Source)) { Write-Warning "  源目录不存在，跳过"; return }
    Get-ChildItem -LiteralPath $Source -Recurse -File -Force | ForEach-Object {
        $relative = $_.FullName.Substring($Source.Length).TrimStart('\')
        if ($Exclude.Count -gt 0 -and ($Exclude | Where-Object { $relative -like $_ })) { return }
        if ($Only.Count -gt 0 -and -not ($Only | Where-Object { $relative -like $_ })) { return }
        $target = Join-Path $Destination $relative
        $same = (Test-Path -LiteralPath $target) -and
                ((Get-Item -LiteralPath $target).Length -eq $_.Length) -and
                ((Get-Item -LiteralPath $target).LastWriteTimeUtc -ge $_.LastWriteTimeUtc)
        if ($same) { $script:skipped++; return }
        if ($DryRun) {
            Write-Host ("   [dry] {0}" -f $relative)
            $script:copied++
            return
        }
        try {
            $parent = Split-Path -Parent $target
            if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
            Copy-Item -LiteralPath $_.FullName -Destination $target -Force -ErrorAction Stop
            $script:copied++
        } catch {
            $script:failed++
            Write-Warning ("   跳过（可能被占用）：{0} —— {1}" -f $relative, $_.Exception.Message)
        }
    }
}

Write-Host "官方安装目录 : $OfficialInstall"
Write-Host "官方用户目录 : $OfficialUserData"
Write-Host "weasel-rs 程序目录 : $TargetExeDir"
Write-Host "weasel-rs 用户目录 : $TargetUserData"
Write-Host ""

# A) 共享数据 -> <程序目录>\rime-data
Copy-Tree -Source (Join-Path $OfficialInstall 'data') `
          -Destination (Join-Path $TargetExeDir 'rime-data') `
          -Label 'A) 共享数据（方案 / 词典 / opencc / preview）'

# B) 用户数据 -> %APPDATA%\Weasel-RS\Rime
Copy-Tree -Source $OfficialUserData `
          -Destination $TargetUserData `
          -Label 'B1) 用户配置（default.custom.yaml / weasel.custom.yaml / images 等）' `
          -Exclude @('build\*', '*.userdb\*', '*.lock')

if ($IncludeUserDb) {
    # 用户词典（leveldb）必须"整库"复制：官方服务运行时以独占方式打开其中的
    # MANIFEST/*.log，只复制到一半的库会因为 CURRENT 指向缺失文件而损坏。
    # 因此先复制到临时目录，任一文件不可读或结果不完整就整库跳过。
    Write-Host '== B2) 用户词典 *.userdb（整库复制，任一文件被占用则整库跳过）==' -ForegroundColor Cyan
    $databases = Get-ChildItem -LiteralPath $OfficialUserData -Directory -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like '*.userdb' }
    foreach ($database in $databases) {
        $target = Join-Path $TargetUserData $database.Name
        $staging = Join-Path $env:TEMP ('userdb-' + $database.Name + '-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Force -Path $staging | Out-Null
        $blocked = $false
        foreach ($file in Get-ChildItem -LiteralPath $database.FullName -File -Force) {
            try {
                Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $staging $file.Name) -Force -ErrorAction Stop
            } catch {
                Write-Warning ('   {0}\{1} 被占用：{2}' -f $database.Name, $file.Name, $_.Exception.Message)
                $blocked = $true
                break
            }
        }
        $current = Join-Path $staging 'CURRENT'
        if (-not $blocked -and (Test-Path -LiteralPath $current)) {
            $manifest = (Get-Content -LiteralPath $current -Raw).Trim()
            if (-not (Test-Path -LiteralPath (Join-Path $staging $manifest))) { $blocked = $true }
        }
        if ($blocked) {
            Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
            Write-Warning ('   跳过整个 {0}：官方输入法正在使用它。请退出官方小狼毫后加 -IncludeUserDb 重跑。' -f $database.Name)
            continue
        }
        if ($DryRun) {
            Write-Host ('   [dry] 整库复制 {0}' -f $database.Name)
            Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
            $script:copied++
            continue
        }
        # 只在整库复制成功且校验通过后才替换目标，绝不留下半个库。
        if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target -Recurse -Force }
        Move-Item -LiteralPath $staging -Destination $target
        Write-Host ('   已复制整库 {0}' -f $database.Name)
        $script:copied++
    }
} else {
    Write-Host '== B2) 用户词典 *.userdb ==' -ForegroundColor Cyan
    Write-Host '   已跳过（如需合并官方用户词典，加 -IncludeUserDb；建议先退出官方小狼毫）'
}

# installation.yaml / user.yaml：先备份再覆盖，保持与官方一致的 installation_id 与上次方案
foreach ($name in 'installation.yaml', 'user.yaml') {
    $source = Join-Path $OfficialUserData $name
    $target = Join-Path $TargetUserData $name
    if (-not (Test-Path -LiteralPath $source)) { continue }
    if ($DryRun) { Write-Host "   [dry] 覆盖 $name（先备份）"; continue }
    if (Test-Path -LiteralPath $target) {
        $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
        Copy-Item -LiteralPath $target -Destination "$target.bak-$stamp" -Force
    }
    Copy-Item -LiteralPath $source -Destination $target -Force
    Write-Host ("   已用官方 {0} 覆盖（旧文件备份为 {0}.bak-*）" -f $name)
}

Write-Host ""
Write-Host ("完成：复制 {0} 个，已最新跳过 {1} 个，失败 {2} 个{3}" -f `
    $script:copied, $script:skipped, $script:failed, $(if ($DryRun) { '（dry-run，未写入）' } else { '' })) -ForegroundColor Green

if ($DryRun) { return }

if ($Deploy) {
    # 应用自己的部署顺序：停服务（释放单实例锁）→ 静默部署 → 重新拉起 broker。
    $broker = Join-Path $TargetExeDir 'weasel-broker.exe'
    $server = Join-Path $TargetExeDir 'weasel-server.exe'
    Write-Host ''
    Write-Host '== C) 重新部署方案 ==' -ForegroundColor Cyan
    if (Test-Path -LiteralPath $broker) {
        Start-Process -FilePath $broker -ArgumentList '--shutdown' -WorkingDirectory $TargetExeDir -Wait
        Start-Sleep -Seconds 3
    }
    $deployProcess = Start-Process -FilePath $server -ArgumentList '--deploy', '--silent' -WorkingDirectory $TargetExeDir -PassThru -Wait
    Write-Host ("   部署退出码 {0}" -f $deployProcess.ExitCode)
    if (Test-Path -LiteralPath $broker) {
        Start-Process -FilePath $broker -WorkingDirectory $TargetExeDir | Out-Null
        Write-Host '   已重新启动 broker（它会拉起 server 与 renderer）'
    }
} else {
    Write-Host "下一步：加 -Deploy 自动重新部署，或手动执行 ——"
    Write-Host "    1) & '$TargetExeDir\weasel-broker.exe' --shutdown      # server 在跑时必须先停，否则 --deploy 会因单实例锁失败"
    Write-Host "    2) & '$TargetExeDir\weasel-server.exe' --deploy --silent"
    Write-Host "    3) & '$TargetExeDir\weasel-broker.exe'                # 重新拉起 server/renderer"
}
