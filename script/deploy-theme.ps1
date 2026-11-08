# deploy-theme.ps1 —— 把重新编译的 theme DLL 覆盖到安装目录并重启输入法。
#
# 必须以管理员身份运行（停进程 + 写 Program Files）。
# 用法：
#   powershell -ExecutionPolicy Bypass -File script\deploy-theme.ps1
#
# 覆盖内容：
#   - weasel_theme_default.dll  (候选窗主题：渐变底 / 圆角遮罩 / 字高绑定间距)

[CmdletBinding()]
param(
    [string]$InstallDir = 'C:\Program Files\Weasel-Next',
    [string]$TargetDir  = 'C:\Code\Weasel-Next\target\release',
    # 编译产物名 → 安装目录里的名字。安装布局在 2026-11 改过：渲染器按
    # `weasel-next-theme.dll` 加载（LoadLibrary），而 cargo 产物是
    # `weasel_theme_default.dll`（2026-11-02 定名，不再叫 official），
    # 所以两边名字不同，不能硬编码同一个。
    [string]$SourceName = 'weasel_theme_default.dll',
    [string]$DllName    = 'weasel-next-theme.dll',
    # 主题 id 改名（official → default）这类**契约变更**必须连渲染器一起换：
    # 老渲染器只认 `official`，新 DLL 自报 `default`，只换一个会加载失败（候选窗不出来）。
    [switch]$WithRenderer,
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
    # broker 必须**排第一个**：它是父进程，一秒钟内就能把 Engine/Renderer 重新拉起来，
    # 先杀子进程再去杀父进程的话，中间那几秒正好又被重生一轮，DLL 照样被占用。
    $names = @('weasel-broker', 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer', 'WeaselServer', 'Weasel-Next-Setting')
    foreach ($n in $names) {
        Get-Process -Name $n -ErrorAction SilentlyContinue | ForEach-Object {
            Write-Host "停止进程 $($_.Name) (pid=$($_.Id))"
            # 进程可能刚好自己退了（父进程一死子进程就跟着走），Kill 会报 Access denied；
            # 这不是错误，别让 $ErrorActionPreference='Stop' 把整个部署打断。
            try { $_.Kill() } catch { Write-Host "  （已自行退出：$($_.Exception.Message)）" }
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
        # Windows PowerShell 5.1 的 [Microsoft.Win32.Registry] 没有 OpenBaseKey（那是 .NET Core 才补的），
        # 只能用 Registry.LocalMachine（返回的是 RegistryKey，同样可 OpenSubKey 可写）。
        $baseKey = [Microsoft.Win32.Registry]::LocalMachine
        $sk = $baseKey.OpenSubKey($subKey, $true)
        if (-not $sk) { throw "打不开注册表键 $subKey" }
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

if (-not (Test-Path -LiteralPath $src -PathType Leaf)) { throw "找不到编译产物: $src（请先 cargo build --release -p weasel-theme-default）" }
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

# 顺带换渲染器（`-WithRenderer`）：DLL 与 exe 之间是"主题 id"契约，改名时不能只换一半。
if ($WithRenderer) {
    $rsrc = Join-Path $TargetDir 'Weasel-Next-Renderer.exe'
    $rdst = Join-Path $InstallDir 'Weasel-Next-Renderer.exe'
    if (-not (Test-Path -LiteralPath $rsrc -PathType Leaf)) { throw "找不到渲染器产物: $rsrc" }
    if (-not (Test-Path -LiteralPath $rdst -PathType Leaf)) {
        Write-Warning "安装目录没有渲染器: $rdst（跳过）"
    } elseif ((Get-FileHash -LiteralPath $rdst -Algorithm SHA256).Hash -eq
              (Get-FileHash -LiteralPath $rsrc -Algorithm SHA256).Hash) {
        Write-Host "渲染器已是最新: $rdst"
    } else {
        $rbak = "$rdst.bak-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
        try {
            Copy-Item -LiteralPath $rdst -Destination $rbak -Force -ErrorAction Stop
            Write-Host "已备份渲染器 -> $rbak"
        } catch {
            Write-Warning "渲染器备份失败（继续覆盖）: $_"
        }
        try {
            Copy-Item -LiteralPath $rsrc -Destination $rdst -Force -ErrorAction Stop
            Write-Host "OK 已覆盖: $rdst"
        } catch {
            Write-Host "渲染器直接覆盖失败 ($_)，改为登记重启替换"
            $staged = "$rdst.pending-$([Guid]::NewGuid().ToString('N').Substring(0,8)).new"
            Copy-Item -LiteralPath $rsrc -Destination $staged -Force
            Register-PendingRename -Staged $staged -Dest $rdst
        }
    }
}

# 重启输入法
#
# ⚠️ 必须用 `explorer.exe <exe>` 间接拉起，**不能**直接 Start-Process：
# 本脚本经 gsudo 提权运行，直接起来的进程是提权会话（job object）的子进程，
# gsudo 一退出就被全家带走 —— 2026-11-04 11:1x 就是这么把 Server/Engine/Renderer
# 全都弄没了，用户当场"怎么界面又没了"（打字没候选窗）。
# 交给已存在的 explorer 拉起，进程挂在用户会话下，才活得下去。
$server = Join-Path $InstallDir 'Weasel-Next-Server.exe'
# 现行宿主是 weasel-broker.exe，由登录计划任务 WeaselNextBoot 常驻拉起；
# 老部署形态（Weasel-Next-Server.exe 直接跑）才走 explorer 那条路。
$bootTask = Get-ScheduledTask -TaskName 'WeaselNextBoot' -ErrorAction SilentlyContinue
if ($bootTask) {
    Write-Host "重启输入法服务（计划任务 WeaselNextBoot）..."
    # 任务若是「已在运行则不重开」的策略，必须先显式停一次，否则 Start 是空操作。
    Stop-ScheduledTask -TaskName 'WeaselNextBoot' -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
    Start-ScheduledTask -TaskName 'WeaselNextBoot' -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 3
    $alive = Get-Process -Name 'weasel-broker','Weasel-Next-Engine','Weasel-Next-Renderer' -ErrorAction SilentlyContinue
    if ($alive) {
        $alive | ForEach-Object { Write-Host "  已启动: $($_.Name) (pid=$($_.Id))" }
    } else {
        Write-Warning "计划任务未拉起进程，请重新切换输入法或手动运行 weasel-broker.exe"
    }
} elseif (Test-Path -LiteralPath $server) {
    Write-Host "重启输入法服务..."
    Start-Process -FilePath 'explorer.exe' -ArgumentList "`"$server`"" | Out-Null
    Start-Sleep -Seconds 3
    $alive = Get-Process -Name 'Weasel-Next-Server','Weasel-Next-Engine','Weasel-Next-Renderer' -ErrorAction SilentlyContinue
    if ($alive) {
        $alive | ForEach-Object { Write-Host "  已启动: $($_.Name) (pid=$($_.Id))" }
    } else {
        # 老系统/explorer 不接参数时的兜底：直启至少当下能用，会话结束再手动重开。
        Write-Warning "explorer 拉起失败，改为直启（退出后可能被一起结束）"
        Start-Process -FilePath $server -WorkingDirectory $InstallDir | Out-Null
        Start-Sleep -Seconds 2
        $alive = Get-Process -Name 'Weasel-Next-Server','Weasel-Next-Engine','Weasel-Next-Renderer' -ErrorAction SilentlyContinue
        if ($alive) {
            $alive | ForEach-Object { Write-Host "  已启动: $($_.Name) (pid=$($_.Id))" }
        } else {
            Write-Warning "服务未起来，请手动运行 Weasel-Next-Server.exe 或重新切换输入法"
        }
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
