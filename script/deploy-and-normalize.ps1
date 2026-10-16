# deploy-and-normalize.ps1 —— 编译 server、装机、跑部署、归类并清理扁平残留、重启并验证
#
# 做三件事：
#   1. 编译 Weasel-Next-Engine 并复制进程序文件夹；
#   2. 跑一次部署（新 server 会在初始化/部署前调用 rime_layout::normalize，
#      自动把根目录扁平的 *.schema.yaml / *.dict.yaml / *.eassy.yaml / *.gram 归类）；
#   3. 安全删除残留的扁平 yaml：**仅当嵌套位置已存在同名文件时**才删（绝不误删）。
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File script\deploy-and-normalize.ps1
#   powershell -ExecutionPolicy Bypass -File script\deploy-and-normalize.ps1 -SkipBuild

[CmdletBinding()]
param(
    [string]$InstallDir = 'C:\Program Files\Weasel-Next',
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$userData = Join-Path $env:APPDATA 'Rime-Next'

function Step($text) { Write-Host ('== ' + $text) -ForegroundColor Cyan }
function Ok($text) { Write-Host ('   ' + $text) -ForegroundColor Green }
function Warn($text) { Write-Host ('   ' + $text) -ForegroundColor Yellow }

# 1) 编译 + 装机
if (-not $SkipBuild) {
    Step '编译 Weasel-Next-Engine'
    & cargo build --release -p Weasel-Next-Engine --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw '编译失败' }
    Ok '编译通过'
}

Step '安装 lua 插件到用户文件夹（代码在 plugin\lua ✓ 不进程序文件夹 ✓）'
$luaSrc = Join-Path $root 'Rime-Next\plugin'   # ★ 打包源（按用户要求 ✓）
$luaDst = Join-Path $userData 'plugin'
if (Test-Path $luaSrc) {
    New-Item -ItemType Directory -Force -Path $luaDst | Out-Null
    Copy-Item (Join-Path $luaSrc 'rime.lua') $luaDst -Force   # ★ 只装 rime.lua 这个接口 ✓ 插件本体由用户自己放 ✓
    # ★ pinyin.yaml / zhuyin.yaml → 用户数据目录（模糊音与注音模板实际会被用到的地方 ✓）
    foreach ($cfg in 'pinyin.yaml','zhuyin.yaml') {
        $srcCfg = Join-Path $root ('Rime-Next\' + $cfg)
        if (Test-Path $srcCfg) { Copy-Item $srcCfg (Join-Path $userData $cfg) -Force }
    }
    Ok ('已安装 rime.lua → ' + $luaDst)
    Ok ('已安装 lua 插件 → ' + $luaDst)
} else { Warn '找不到 plugin\lua（跳过）' }
Step '停栈并安装新 server'
Get-Process -Name 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer' -ErrorAction SilentlyContinue |
    ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
Start-Sleep -Seconds 4
Copy-Item (Join-Path $root 'target\release\Weasel-Next-Engine.exe') (Join-Path $InstallDir 'Weasel-Next-Engine.exe') -Force
Ok ('已安装 Weasel-Next-Engine.exe ' + (Get-Item (Join-Path $InstallDir 'Weasel-Next-Engine.exe')).Length + ' 字节')

# 重建后的 broker 必须先跑一次 --post-install 注册服务（否则它不监听 RPC 管道 ✗
# → 渲染器 readiness 失败 → "renderer failed before readiness" ✗ → 候选窗不显示 ✓）
Step '注册 broker 服务（--post-install）'
& cmd /c ('"' + (Join-Path $InstallDir 'Weasel-Next-Server.exe') + '" --post-install') | Out-Null
Ok ('post-install 退出码 ' + $LASTEXITCODE)
Step '跑一次部署（触发 normalize 归类）'
$server = Join-Path $InstallDir 'Weasel-Next-Engine.exe'
& cmd /c ('"' + $server + '" --deploy --silent')
Ok ('部署退出码 ' + $LASTEXITCODE)
Start-Sleep -Seconds 3

# 2) 汇报归类结果
foreach ($base in @((Join-Path $InstallDir 'Rime-Next'), $userData)) {
    $tag = if ($base -like '*Program Files*') { '<程序>' } else { '%Rime-Next%' }
    Step ('归类结果 ' + $tag)
    $flat = @(Get-ChildItem $base -File -Include '*.schema.yaml', '*.dict.yaml' -ErrorAction SilentlyContinue)
    Ok ('根目录仍扁平 yaml: ' + $flat.Count + ' 个')
    Get-ChildItem (Join-Path $base 'schema') -Directory -ErrorAction SilentlyContinue | ForEach-Object {
        $files = (Get-ChildItem $_.FullName -File | ForEach-Object { $_.Name }) -join ', '
        Ok ('schema\' + $_.Name + '\  ' + $files)
    }
}

# 3) 安全清理扁平残留
Step '安全清理扁平残留（仅当嵌套位置已有同名文件）'
$deleted = 0
$kept = 0
foreach ($base in @((Join-Path $InstallDir 'Rime-Next'), $userData)) {
    $tag = if ($base -like '*Program Files*') { '<程序>' } else { '%Rime-Next%' }
    $flat = @(Get-ChildItem $base -File -Include '*.schema.yaml', '*.dict.yaml' -ErrorAction SilentlyContinue)
    foreach ($f in $flat) {
        if ($f.Name -notmatch '^(.+?)\.(schema|dict)\.yaml$') { continue }
        $name = $Matches[1]
        $nested = Join-Path (Join-Path (Join-Path $base 'schema') $name) $f.Name
        if (Test-Path $nested) {
            Remove-Item $f.FullName -Force
            $deleted++
            Ok ('删 ' + $tag + '\' + $f.Name + '（嵌套位置已有）')
        }
        else {
            $kept++
            Warn ('保留 ' + $tag + '\' + $f.Name + '（嵌套位置没有同名文件，不能删）')
        }
    }
}
Ok ('删除 ' + $deleted + ' 个 · 保留 ' + $kept + ' 个')

# 4) 重启（WMI：脱离本会话，避免被回收）
Step '重启栈（WMI）'
$broker = Join-Path $InstallDir 'Weasel-Next-Server.exe'
$created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = ('"' + $broker + '"') }
if ($created.ReturnValue -ne 0) { Warn ('WMI 启动失败，返回码 ' + $created.ReturnValue) } else { Ok ('已启动 broker（PID ' + $created.ProcessId + '）') }
Start-Sleep -Seconds 16
foreach ($name in 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer') {
    if (Get-Process -Name $name -ErrorAction SilentlyContinue) { Ok ($name + ' ✓') } else { Warn ($name + ' 未起来 ✗') }
}
$pipes = @(Get-ChildItem '\\.\pipe\' -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'weasel-next' })
Ok ('命名管道 ' + $pipes.Count + ' 条')
foreach ($pipe in ($pipes | Where-Object { $_.Name -match 'server' })) {
    try {
        $client = New-Object System.IO.Pipes.NamedPipeClientStream('.', $pipe.Name, [System.IO.Pipes.PipeDirection]::InOut)
        $client.Connect(2000)
        $client.Dispose()
        Ok '服务端管道可连 ✓（输入法可用）'
    }
    catch { Warn ('服务端管道连不上: ' + $_.Exception.Message) }
}

# 5) 引擎日志
Step '引擎日志'
$log = Get-ChildItem $env:TEMP -File -Filter 'rime.weasel-next*.log*' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($log) {
    $errors = @(Get-Content $log.FullName -ErrorAction SilentlyContinue | Select-String -Pattern 'E2026|Error|nonexistent')
    if ($errors.Count -eq 0) { Ok ('错误 0 条 ✓（' + $log.Name + '）') } else { Warn ('错误 ' + $errors.Count + ' 条'); $errors | Select-Object -Last 5 | ForEach-Object { Warn $_.Line.Trim() } }
}
