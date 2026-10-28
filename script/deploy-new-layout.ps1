#requires -RunAsAdministrator
<#
  deploy-new-layout.ps1
  ---------------------
  把「当前构建版本」部署到【新版程序文件夹布局】：

      C:\Program Files\Rime-Next\            <- 程序文件夹（安装路径 + 产品名，到此为止）
          Weasel-Next-Server.exe  ...        <- 可执行文件
          weasel.yaml                        <- 外壳配置（跟 exe 走）
          data\                              <- 内置数据目录 = librime 的 shared_data_dir
              default.yaml  schema\  opencc\  parser\  eassy\  theme\ ...

  版本不进目录层级：Weasel-Next-<版本> 只是安装包/exe 的名字，不是目录名。

  用法（管理员 PowerShell）：
      .\script\deploy-new-layout.ps1
      .\script\deploy-new-layout.ps1 -RegisterTip     # 顺带注册新程序文件夹里的 TIP dll
      .\script\deploy-new-layout.ps1 -BuildDir ... -InstallDir ...

  默认【不】注册 TIP，避免动到正在用的输入法；要切换再显式加 -RegisterTip。
#>
[CmdletBinding()]
param(
    [string]$BuildDir   = "C:\Code\Weasel-Next\target\debug",
    [string]$DataDir    = "C:\Code\Weasel-Next\Rime-Next",
    [string]$InstallDir = (Join-Path $env:ProgramFiles "Rime-Next"),

    # 顺带把新程序文件夹里的 TIP dll 注册掉（会切换系统输入法的 dll 指向）
    [switch]$RegisterTip,
    # 不停现有 Weasel-Next 进程（文件可能被占用而复制失败）
    [switch]$NoStop
)

$ErrorActionPreference = 'Stop'
$DataTarget = Join-Path $InstallDir 'data'

function Step($msg) { Write-Host "`n== $msg" -ForegroundColor Cyan }
function Ok($msg)   { Write-Host "   ok  $msg" -ForegroundColor Green }
function Warn($msg) { Write-Host "   !!  $msg" -ForegroundColor Yellow }

# ---------------------------------------------------------------- 0 自检
Step "自检"
if (-not (Test-Path $BuildDir)) { throw "构建目录不存在：$BuildDir （先 cargo build）" }
if (-not (Test-Path $DataDir))  { throw "源数据目录不存在：$DataDir" }
Ok "构建目录 $BuildDir"
Ok "源数据目录 $DataDir"
Ok "目标程序文件夹 $InstallDir"

# ---------------------------------------------------------------- 1 停进程
if (-not $NoStop) {
    Step "停止正在运行的 Weasel-Next 进程"
    $procs = Get-Process -Name 'Weasel-Next-Server','Weasel-Next-Engine','Weasel-Next-Renderer','Weasel-Next-Setting' -ErrorAction SilentlyContinue
    if ($procs) {
        $procs | ForEach-Object {
            try { $_ | Stop-Process -Force; Ok "已停止 $($_.ProcessName)" }
            catch { Warn "停不掉 $($_.ProcessName)：$($_.Exception.Message)" }
        }
        Start-Sleep -Seconds 1
    } else { Ok "没有在跑的 Weasel-Next 进程" }
}

# ---------------------------------------------------------------- 2 建目录
Step "建立新版程序文件夹"
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
Ok $InstallDir

# ---------------------------------------------------------------- 3 可执行文件
Step "复制可执行文件 / DLL"
$files = @(
    'Weasel-Next-Server.exe',
    'Weasel-Next-Engine.exe',
    'Weasel-Next-Renderer.exe',
    'Weasel-Next-Setting.exe',
    'librime_next.dll',
    'rime_input_stat_core.dll',
    'weasel_theme_official.dll',
    'weasel_tip.dll'
)
foreach ($f in $files) {
    $src = Join-Path $BuildDir $f
    if (Test-Path $src) {
        Copy-Item -Path $src -Destination (Join-Path $InstallDir $f) -Force
        Ok $f
    } else { Warn "缺 $f（跳过）" }
}

# ---------------------------------------------------------------- 4 内置数据 -> data\
Step "复制内置数据 -> $DataTarget"
$rc = & robocopy.exe $DataDir $DataTarget /MIR /R:2 /W:1 /NFL /NDL /NJH /NJS /XD .git
if ($LASTEXITCODE -ge 8) { throw "robocopy 失败（exit=$LASTEXITCODE）" }
Ok "数据同步完成（robocopy exit=$LASTEXITCODE）"

# ---------------------------------------------------------------- 5 外壳配置（跟 exe 走）
Step "外壳配置"
foreach ($cfg in @('weasel.yaml','weasel.official.yaml')) {
    $src = Join-Path $DataDir $cfg
    if (Test-Path $src) {
        Copy-Item -Path $src -Destination (Join-Path $InstallDir $cfg) -Force
        Ok "$cfg -> 程序文件夹根"
    }
}

# ---------------------------------------------------------------- 6 可选：注册 TIP
if ($RegisterTip) {
    Step "注册 TIP（新程序文件夹）"
    $tip = Join-Path $InstallDir 'Weasel-Next.dll'
    if (-not (Test-Path $tip)) { Copy-Item (Join-Path $InstallDir 'weasel_tip.dll') $tip -Force }
    & "$env:SystemRoot\System32\regsvr32.exe" /s $tip
    Ok "regsvr32 $tip"
}

# ---------------------------------------------------------------- 7 汇报
Step "结果"
$schemeCount = 0
if (Test-Path (Join-Path $DataTarget 'schema')) {
    $schemeCount = (Get-ChildItem -Path (Join-Path $DataTarget 'schema') -Directory).Count
}
Write-Host "   程序文件夹 : $InstallDir"
Write-Host "   数据目录   : $DataTarget   (shared_data_dir)"
Write-Host "   方案目录数 : $schemeCount"
Write-Host "`n下一步：从 $InstallDir 启动 Weasel-Next-Server.exe，data_dir 会命中 data\。" -ForegroundColor Cyan
