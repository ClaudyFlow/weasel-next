# 请 broker 重新部署（与设置程序同一条路：给 broker 窗口发 WM_COMMAND(1001)）。
# 窗口用 EnumWindows 按类名找 —— FindWindowW 在部分环境返回 0。
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class D {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", SetLastError = true)] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
}
'@
$broker = [IntPtr]::Zero
$cb = [D+EnumProc]{
    param($h, $l)
    $cs = New-Object System.Text.StringBuilder 64
    [void][D]::GetClassNameW($h, $cs, 64)
    if ($cs.ToString() -eq 'weasel-next-broker') { $script:broker = $h; return $false }
    return $true
}
[void][D]::EnumWindows($cb, [IntPtr]::Zero)
if ($broker -eq [IntPtr]::Zero) { '找不到 broker 窗口'; exit 1 }
"broker 窗口 = 0x{0:X}" -f $broker.ToInt64()
$ok = [D]::PostMessageW($broker, 0x0111, [IntPtr]1001, [IntPtr]::Zero)   # WM_COMMAND, DEPLOY=1001
"已发送部署请求: $ok"
