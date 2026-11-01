# 把乙路这次改过的宿主组件换进 live 安装目录（**需要提权**）。
# 结果写到 C:\Code\_logs\deploy-host-perf.log
#
# 换的东西：
#   * Weasel-Next-Tip.dll        —— 按键延迟采样（乙-1 插桩）
#   * Weasel-Next-Engine.exe     —— 引擎侧按键耗时采样
#   * Weasel-Next-Renderer.exe   —— 主题时间戳不再每键打文件系统
#   * weasel-next-theme.dll  —— 同上（主题 stamp 缓存 + 不再每键写 theme.log）
#
# 流程：停全部 Weasel-Next 进程 → 覆盖 → 重新拉起 Server（它会带起引擎/渲染器）。
# Tip DLL 会被各个应用进程占着，所以先停干净再拷。

$ErrorActionPreference = 'Stop'
$src = 'C:\Code\Weasel-Next\target\release'
$dst = 'C:\Program Files\Rime-Next\Weasel-Next-0.18.5'
$log = 'C:\Code\_logs\deploy-host-perf.log'
New-Item -ItemType Directory -Force -Path (Split-Path $log) | Out-Null

# 先把 cargo 的产出名换成对外文件名（TIP 叫 Weasel-Next.dll）
Copy-Item "$src\weasel_tip.dll" "$src\Weasel-Next.dll" -Force
# 32 位那份由 script\build-tip32.ps1 单独编出，已经在 target\release\ 下

$files = @(
    'Weasel-Next.dll',
    'Weasel-Next-32.dll',
    'Weasel-Next-Engine.exe',
    'Weasel-Next-Renderer.exe',
    'weasel-next-theme.dll'
)

$report = @()
try {
    foreach ($name in @('Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer', 'Weasel-Next-Setting')) {
        Get-Process $name -ErrorAction SilentlyContinue | Stop-Process -Force
    }
    Start-Sleep -Seconds 2

    foreach ($name in $files) {
        $s = Join-Path $src $name
        $d = Join-Path $dst $name
        if (-not (Test-Path -LiteralPath $s)) { $report += "MISSING $name"; continue }
        Copy-Item -LiteralPath $s -Destination $d -Force
        $report += "OK $name $((Get-Item -LiteralPath $d).Length) bytes $((Get-Item -LiteralPath $d).LastWriteTime)"
    }
} catch {
    $report += "FAIL $($_.Exception.Message)"
} finally {
    # ⚠️ 无论复制成功与否都要把服务拉起来 —— 否则输入法就停在这儿了。
    $server = Join-Path $dst 'Weasel-Next-Server.exe'
    if (Test-Path -LiteralPath $server) { Start-Process -FilePath $server }
    Start-Sleep -Seconds 3
    $alive = (Get-Process 'Weasel-Next-Server', 'Weasel-Next-Engine', 'Weasel-Next-Renderer' -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty ProcessName) -join ','
    $report += "ALIVE $alive"
    $report | Out-File -LiteralPath $log -Encoding utf8
}
