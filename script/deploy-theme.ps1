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
    [string]$TargetDir  = 'C:\Code\Weasel-Next\target\release',
    # 编译产物名 → 安装目录里的名字。安装布局在 2026-11 改过：渲染器按
    # `weasel-next-theme.dll` 加载（LoadLibrary），而 cargo 产物是
    # `weasel_theme_official.dll`，所以两边名字不同，不能硬编码同一个。
    [string]$SourceName = 'weasel_theme_official.dll',
    [string]$DllName    = 'weasel-next-theme.dll',
    # 部署完成后在右下角弹通知（和输入法自己的"配置加载失败"同一条 Toast 通道）。
    # 批处理/CI 里不想要弹窗时加 -NoToast。
    [switch]$NoToast
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

function Show-DeployToast {
    <#
      在右下角弹一条通知，和输入法自己（broker）发的"配置加载失败"走同一条
      Windows Toast 通道：AUMID 用 `WeaselNext.Broker`，也就是开始菜单里
      「小狼毫Next算法服务.lnk」注册的那个 AppUserModelID。

      Toast 不可用（AUMID 没注册/通知被系统关掉）时退回托盘气泡，
      再不行就只写控制台 —— 通知失败绝不影响部署结果。
    #>
    param(
        [Parameter(Mandatory)] [string]$Title,
        [Parameter(Mandatory)] [string]$Body,
        [string]$AppId = 'WeaselNext.Broker',
        # 气泡/TToast 都是本进程持有的，脚本一退出就收走 ⇒ 停留几秒再收尾。
        [int]$Seconds = 7
    )
    $shown = $false
    try {
        $null = [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]
        $null = [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom, ContentType = WindowsRuntime]
        $escape = { param($s) [System.Security.SecurityElement]::Escape($s) }
        $xml = New-Object Windows.Data.Xml.Dom.XmlDocument
        $xml.LoadXml("<toast><visual><binding template=`"ToastGeneric`"><text>$(& $escape $Title)</text><text>$(& $escape $Body)</text></binding></visual></toast>")
        $toast = New-Object Windows.UI.Notifications.ToastNotification $xml
        [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($AppId).Show($toast)
        $shown = $true
    } catch {
        Write-Host "Toast 不可用（$($_.Exception.Message)），改用托盘气泡"
    }
    if (-not $shown) {
        try {
            Add-Type -AssemblyName System.Windows.Forms -ErrorAction Stop
            Add-Type -AssemblyName System.Drawing -ErrorAction Stop
            $icon = New-Object System.Windows.Forms.NotifyIcon
            $icon.Icon = [System.Drawing.SystemIcons]::Information
            $icon.BalloonTipTitle = $Title
            $icon.BalloonTipText = $Body
            $icon.BalloonTipIcon = [System.Windows.Forms.ToolTipIcon]::Info
            $icon.Visible = $true
            $icon.ShowBalloonTip($Seconds * 1000)
            Start-Sleep -Seconds $Seconds
            $icon.Visible = $false
            $icon.Dispose()
            return
        } catch {
            Write-Warning "托盘气泡也失败: $($_.Exception.Message)"
            return
        }
    }
    # Toast 由通知中心托管，本进程退出不会收走，但仍等一会儿让调用方看到弹窗过程。
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

$src = Join-Path $TargetDir $SourceName
$dst = Join-Path $InstallDir $DllName

if (-not (Test-Path -LiteralPath $src -PathType Leaf)) { throw "找不到编译产物: $src（请先 cargo build --release -p weasel-theme-official）" }
if (-not (Test-Path -LiteralPath $dst -PathType Leaf)) { throw "找不到安装目录里的 dll: $dst" }

$srcHash = (Get-FileHash -LiteralPath $src -Algorithm SHA256).Hash
$dstHash = (Get-FileHash -LiteralPath $dst -Algorithm SHA256).Hash
if ($srcHash -eq $dstHash) {
    Write-Host "内容一致，无需覆盖: $dst"
    if (-not $NoToast) {
        Show-DeployToast -Title '小狼毫Next：主题已是最新' `
            -Body ("DLL 哈希一致，本次没有覆盖。" + [Environment]::NewLine + "SHA256 " + $srcHash.Substring(0, 12))
    }
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
$after = (Get-FileHash -LiteralPath (Join-Path $InstallDir $DllName) -Algorithm SHA256).Hash
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

# 右下角通知：部署结果 + 当前档位/配色，省得再去翻 theme.log 对账。
if (-not $NoToast) {
    $themeDir = Join-Path $env:APPDATA 'Rime-Next\theme'
    $material = '未知'
    $mf = Join-Path $themeDir 'material.txt'
    if (Test-Path -LiteralPath $mf) { $material = (Get-Content -LiteralPath $mf -Raw).Trim() }
    $scheme = '未知'
    $yf = Join-Path $themeDir 'theme.yaml'
    if (Test-Path -LiteralPath $yf) {
        $match = Select-String -LiteralPath $yf -Pattern '^scheme\s*:\s*(\S+)' | Select-Object -First 1
        if ($match) { $scheme = $match.Matches[0].Groups[1].Value.Trim('"', "'") }
    }
    $body = @(
        "材质 $material · 配色 $scheme"
        "SHA256 $($srcHash.Substring(0, 12))"
    ) -join [Environment]::NewLine
    Show-DeployToast -Title '小狼毫Next：主题已部署' -Body $body
}
