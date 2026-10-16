# 决定性验证：在真记事本里打字，**保持组合串不提交**，当场枚举所有顶层窗口，
# 看我们的渲染器到底有没有可见窗口（区分官方小狼毫 vs Weasel-Next）。

param([string]$Text = 'wenjian')

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class WV {
    public delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern IntPtr SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    public static string Class(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
    public static void Key(byte vk) {
        keybd_event(vk, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(25);
        keybd_event(vk, 0, 2, UIntPtr.Zero); System.Threading.Thread.Sleep(70);
    }
}
'@

$np = Get-Process -Name notepad -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $np) { $np = Start-Process notepad.exe -PassThru; Start-Sleep -Milliseconds 2500; $np = Get-Process -Name notepad | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1 }
[void][WV]::SetForegroundWindow($np.MainWindowHandle)
Start-Sleep -Milliseconds 600
Write-Output "目标: [$([WV]::Class($np.MainWindowHandle))] $($np.MainWindowTitle)"

foreach ($ch in $Text.ToCharArray()) { [WV]::Key([byte][char]([string]$ch).ToUpper()) }
Start-Sleep -Milliseconds 400   # 组合串保持不提交

Write-Output "=== 此刻所有可见的顶层窗口（含进程名）==="
$rows = New-Object System.Collections.ArrayList
$callback = [WV+EnumProc]{
    param($h, $p)
    if ([WV]::IsWindowVisible($h)) {
        $r = New-Object WV+RECT
        [void][WV]::GetWindowRect($h, [ref]$r)
        $w = $r.R - $r.L; $ht = $r.B - $r.T
        if ($w -gt 40 -and $ht -gt 20 -and $w -lt 900 -and $ht -lt 500) {
            $pid2 = 0
            [void][WV]::GetWindowThreadProcessId($h, [ref]$pid2)
            $name = try { (Get-Process -Id $pid2 -ErrorAction Stop).ProcessName } catch { "?" }
            [void]$rows.Add([pscustomobject]@{ 进程=$name; 类=[WV]::Class($h); 位置="$($r.L),$($r.T)"; 尺寸="${w}x${ht}" })
        }
    }
    return $true
}
[void][WV]::EnumWindows($callback, [IntPtr]::Zero)
$rows | Where-Object { $_.类 -match 'Weasel|weasel|andidate|Official|Rime' -or $_.进程 -match 'Weasel|weasel' } | Format-Table -AutoSize
Write-Output "（上面若只有官方 WeaselServer 的行 = 我们的渲染器没有窗口 ✗；若有 Weasel-Next-Renderer 的行 = 我们的窗口在 ✓）"
