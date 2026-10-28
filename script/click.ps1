param(
    [Parameter(Mandatory=$true)][int]$TargetPid,
    [Parameter(Mandatory=$true)][int]$X,
    [Parameter(Mandatory=$true)][int]$Y,
    [int]$SettleMs = 700
)

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CK {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
'@

$hwnd = [IntPtr]::Zero
$best = 0
$cb = [CK+EnumProc]{
    param($h, $l)
    $owner = 0
    [void][CK]::GetWindowThreadProcessId($h, [ref]$owner)
    if ($owner -ne $TargetPid) { return $true }
    if (-not [CK]::IsWindowVisible($h)) { return $true }
    $r = New-Object CK+RECT
    [void][CK]::GetWindowRect($h, [ref]$r)
    $area = ($r.Right - $r.Left) * ($r.Bottom - $r.Top)
    if ($area -gt $best) { $script:best = $area; $script:hwnd = $h }
    return $true
}
[void][CK]::EnumWindows($cb, [IntPtr]::Zero)
if ($hwnd -eq [IntPtr]::Zero) { Write-Error "no visible window for pid $TargetPid"; exit 1 }

$lp = [IntPtr](($Y -shl 16) -bor $X)
[void][CK]::PostMessage($hwnd, 0x0200, [IntPtr]::Zero, $lp)
[void][CK]::PostMessage($hwnd, 0x0201, [IntPtr]1, $lp)
Start-Sleep -Milliseconds 60
[void][CK]::PostMessage($hwnd, 0x0202, [IntPtr]::Zero, $lp)
Start-Sleep -Milliseconds $SettleMs
Write-Output "clicked client ($X,$Y) on hwnd $hwnd"
