# 实机验证（真程序版）：启动记事本 → 打字 → 看键入、引擎、渲染器、候选窗四处是否一致。
#
# 为什么不用自造窗口：自造窗口的插入符几何（anchor）由 TIP 现算，容易拿不到，
# 引擎在"锚点缺失"时会故意发空帧 ✗ —— 那样测不出真实场景的问题 ✓。

param(
    [string]$Text = 'wenjian',
    [int]$WaitAfterLaunchMs = 2500
)

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class RT {
    [DllImport("user32.dll")] public static extern IntPtr SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern IntPtr GetClassName(IntPtr h, StringBuilder s, int n);
    public static string Title(IntPtr h) { var sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
    public static string Class(IntPtr h) { var sb = new StringBuilder(256); GetClassName(h, sb, 256); return sb.ToString(); }
    public static void Key(byte vk) {
        keybd_event(vk, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(25);
        keybd_event(vk, 0, 2, UIntPtr.Zero);
        System.Threading.Thread.Sleep(60);
    }
}
'@

# ① 启动记事本（经典版；Store 版会立刻退出，那时会看不到窗口）
$before = @(Get-Process -Name notepad -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)
$proc = Start-Process notepad.exe -PassThru
Start-Sleep -Milliseconds $WaitAfterLaunchMs
$win = (Get-Process -Name notepad -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1)
if (-not $win) {
    Write-Output "记事本没有可用窗口（可能是 Store 版）；改用 PowerShell 控制台窗口"
    $win = Get-Process -Name powershell, pwsh -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
}
if (-not $win) { Write-Output "找不到任何可打字的真实窗口 ✗"; exit 1 }

$h = $win.MainWindowHandle
Write-Output "目标窗口: [$([RT]::Class($h))] $([RT]::Title($h))  pid=$($win.Id)"
[void][RT]::ShowWindow($h, 9)
[void][RT]::SetForegroundWindow($h)
Start-Sleep -Milliseconds 700
Write-Output "当前前台: [$([RT]::Class([RT]::GetForegroundWindow()))] $([RT]::Title([RT]::GetForegroundWindow()))"

# ② 打字
Write-Output "开始输入: $Text"
foreach ($ch in $Text.ToCharArray()) {
    $vk = [byte][char]([string]$ch).ToUpper()
    [RT]::Key($vk)
}
Start-Sleep -Milliseconds 1500

# ③ 结果
Write-Output "=== 候选窗（类名含 candidate / 标题含 Weasel）==="
$found = $false
Get-Process | Where-Object { $_.MainWindowHandle -ne 0 } | ForEach-Object {
    $cls = [RT]::Class($_.MainWindowHandle)
    if ($cls -match 'candidate|weasel' -or $_.ProcessName -match 'Renderer') {
        $found = $true
        "  候选窗: [$cls] $([RT]::Title($_.MainWindowHandle))  pid=$($_.Id)"
    }
}
if (-not $found) { "  ✗ 没找到可见的候选窗" }
