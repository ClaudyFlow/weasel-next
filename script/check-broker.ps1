# 判定 broker 是否比本进程权限高（UIPI）：往 broker 窗口发一条无害消息，看是否被拒。
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class P {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", SetLastError = true)] public static extern bool PostMessageW(IntPtr hWnd, uint Msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
}
'@
$target = [IntPtr]::Zero
$pid_ = 0
$cb = [P+EnumProc]{
    param($h, $l)
    $cs = New-Object System.Text.StringBuilder 256
    [void][P]::GetClassNameW($h, $cs, 256)
    if ($cs.ToString() -eq 'weasel-next-broker') {
        $script:target = $h
        $p = 0
        [void][P]::GetWindowThreadProcessId($h, [ref]$p)
        $script:pid_ = $p
    }
    return $true
}
[void][P]::EnumWindows($cb, [IntPtr]::Zero)
if ($target -eq [IntPtr]::Zero) { '找不到 broker 窗口'; exit 1 }
"broker 窗口 = 0x{0:X}，进程 PID = {1}" -f $target.ToInt64(), $pid_
$ok = [P]::PostMessageW($target, 0x0000, [IntPtr]::Zero, [IntPtr]::Zero)   # WM_NULL
$err = [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()
"PostMessage(WM_NULL) 返回 {0}，GetLastError={1}" -f $ok, $err
if (-not $ok -and $err -eq 5) { '→ 被 UIPI 拒绝（错误 5 = 拒绝访问）：broker 权限比本进程高（很可能是提权进程）' }
elseif ($ok) { '→ 可以给 broker 发消息 ✓' }
"本进程 PID = $PID"
