# 抓取指定标题的窗口成一个 PNG（候选窗预览/官方候选窗都适用）。
#
# 注意（踩过的坑）：截图进程必须先是 DPI 感知的，否则 GetWindowRect 给的是逻辑坐标、
# CopyFromScreen 用的是物理像素，抓到的会是桌面而不是窗口。
#
# 用法：pwsh -File script\capture-window.ps1 -Title "Weasel-Next 皮肤预览" -Out shot.png
param(
    [Parameter(Mandatory = $true)][string]$Title,
    [Parameter(Mandatory = $true)][string]$Out,
    [int]$DelayMs = 1200,
    [int]$Padding = 0,
    [string]$Class = '',
    [int]$ProcessId = 0
)

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class Win {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
'@

[void][Win]::SetProcessDPIAware()
Start-Sleep -Milliseconds $DelayMs

# 定位窗口：优先按「类名 + 进程号」，其次按标题。
# （主题窗口的标题在跨进程读取时可能是空的，所以类名更可靠。）
$hwnd = [IntPtr]::Zero
if ($Class -ne '') {
    $candidates = New-Object System.Collections.ArrayList
    $cb = [Win+EnumProc]{
        param($h, $l)
        $owner = 0
        [void][Win]::GetWindowThreadProcessId($h, [ref]$owner)
        if ($ProcessId -ne 0 -and $owner -ne $ProcessId) { return $true }
        if (-not [Win]::IsWindowVisible($h)) { return $true }
        $name = New-Object System.Text.StringBuilder 256
        [void][Win]::GetClassNameW($h, $name, 256)
        if ($name.ToString() -eq $Class) { [void]$candidates.Add($h) }
        return $true
    }
    [void][Win]::EnumWindows($cb, [IntPtr]::Zero)
    if ($candidates.Count -gt 0) { $hwnd = $candidates[0] }
}
if ($hwnd -eq [IntPtr]::Zero) { $hwnd = [Win]::FindWindowW($null, $Title) }
if ($hwnd -eq [IntPtr]::Zero) { Write-Error "找不到窗口: title=$Title class=$Class pid=$ProcessId"; exit 1 }
if (-not [Win]::IsWindowVisible($hwnd)) { Write-Error "窗口不可见: $Title"; exit 2 }

$rect = New-Object Win+RECT
[void][Win]::GetWindowRect($hwnd, [ref]$rect)
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -le 0 -or $height -le 0) { Write-Error "窗口尺寸异常: ${width}x${height}"; exit 3 }

Add-Type -AssemblyName System.Drawing
$x = $rect.Left - $Padding
$y = $rect.Top - $Padding
$w = $width + $Padding * 2
$h = $height + $Padding * 2
$bitmap = New-Object System.Drawing.Bitmap $w, $h
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($x, $y, 0, 0, $bitmap.Size)
$bitmap.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()

Write-Output "已抓取 $Title : ${width}x${height} @ ($($rect.Left),$($rect.Top)) -> $Out"
