param(
    [Parameter(Mandatory=$true)][int]$TargetPid,
    [Parameter(Mandatory=$true)][string]$Out,
    [int]$WaitMs = 1200,
    [switch]$Screen
)

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CW {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdc, uint flags);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
'@

[void][CW]::SetProcessDPIAware()
$hwnd = [IntPtr]::Zero
$best = 0
$cb = [CW+EnumProc]{
    param($h, $l)
    $owner = 0
    [void][CW]::GetWindowThreadProcessId($h, [ref]$owner)
    if ($owner -ne $TargetPid) { return $true }
    if (-not [CW]::IsWindowVisible($h)) { return $true }
    $r = New-Object CW+RECT
    [void][CW]::GetWindowRect($h, [ref]$r)
    $area = ($r.Right - $r.Left) * ($r.Bottom - $r.Top)
    if ($area -gt $best) { $script:best = $area; $script:hwnd = $h }
    return $true
}
[void][CW]::EnumWindows($cb, [IntPtr]::Zero)
if ($hwnd -eq [IntPtr]::Zero) { Write-Error "no visible window for pid $TargetPid"; exit 1 }

Start-Sleep -Milliseconds $WaitMs
$rect = New-Object CW+RECT
[void][CW]::GetWindowRect($hwnd, [ref]$rect)
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top

Add-Type -AssemblyName System.Drawing
$bitmap = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bitmap)
if ($Screen) {
    $g.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size); $mode = 'screen'
} else {
    $hdc = $g.GetHdc(); $ok = [CW]::PrintWindow($hwnd, $hdc, 2); $g.ReleaseHdc($hdc); $mode = "printwindow ok=$ok"
}
$bitmap.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bitmap.Dispose()
Write-Output "captured $mode ${w}x${h} @ ($($rect.Left),$($rect.Top)) -> $Out"
