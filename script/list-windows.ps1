# 列出指定进程的所有顶层窗口（含无标题的），用于抓主题自己画的窗口。
# 用法：pwsh -File script\list-windows.ps1 -ProcessName Weasel-Next-Renderer
param(
    [string]$ProcessName = '',
    [int]$ProcessId = 0
)
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class W {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr h, int index);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
'@
[void][W]::SetProcessDPIAware()
$wanted = @()
if ($ProcessId -ne 0) { $wanted += $ProcessId }
if ($ProcessName) { $wanted += (Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id) }
$rows = New-Object System.Collections.ArrayList
$cb = [W+EnumProc]{
    param($h, $l)
    $pid_ = 0
    [void][W]::GetWindowThreadProcessId($h, [ref]$pid_)
    if ($wanted.Count -eq 0 -or $wanted -contains $pid_) {
        $sb = New-Object System.Text.StringBuilder 512
        [void][W]::GetWindowTextW($h, $sb, 512)
        $cn = New-Object System.Text.StringBuilder 256
        [void][W]::GetClassNameW($h, $cn, 256)
        $r = New-Object W+RECT
        [void][W]::GetWindowRect($h, [ref]$r)
        [void]$rows.Add([pscustomobject]@{
            HWND = ('0x{0:X}' -f $h.ToInt64()); PID = $pid_
            可见 = [W]::IsWindowVisible($h); 类 = $cn.ToString(); 标题 = $sb.ToString()
            宽 = $r.Right - $r.Left; 高 = $r.Bottom - $r.Top; 左 = $r.Left; 上 = $r.Top
            样式 = ('0x{0:X}' -f [W]::GetWindowLongW($h, -16))
        })
    }
    return $true
}
[void][W]::EnumWindows($cb, [IntPtr]::Zero)
$rows | Format-Table -AutoSize
