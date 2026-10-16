# 决定性实验：本进程自己建一个窗口，再分别用 FindWindowW / EnumWindows 找它。
# - 两者都能找到 → FindWindow 在本环境可用，broker 找不到是"桌面/会话"层面的问题
# - 只有 EnumWindows 找得到 → 本环境下 FindWindow 被拦（沙箱/桌面限制），代码要改成枚举
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class T {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    public delegate IntPtr WndProc(IntPtr hWnd, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern ushort RegisterClassW(ref WNDCLASSW wc);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern IntPtr CreateWindowExW(uint ex, string cls, string title, uint style,
        int x, int y, int w, int h, IntPtr parent, IntPtr menu, IntPtr inst, IntPtr param);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct WNDCLASSW {
        public uint style; public IntPtr lpfnWndProc; public int cbClsExtra; public int cbWndExtra;
        public IntPtr hInstance; public IntPtr hIcon; public IntPtr hCursor; public IntPtr hbrBackground;
        public string lpszMenuName; public string lpszClassName;
    }
}
'@
$name = 'dsh-findwindow-probe-' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
$wc = New-Object T+WNDCLASSW
$wc.lpfnWndProc = [T+WndProc]{ param($h, $m, $w, $l) return [IntPtr]::Zero }
$wc.lpszClassName = $name
$atom = [T]::RegisterClassW([ref]$wc)
"注册窗口类 '$name' atom=$atom"
$hwnd = [T]::CreateWindowExW(0, $name, 'probe', 0x80000000, 0, 0, 10, 10, [IntPtr]::Zero, [IntPtr]::Zero, [IntPtr]::Zero, [IntPtr]::Zero)
"自建窗口 = 0x{0:X}" -f $hwnd.ToInt64()
$byFind = [T]::FindWindowW($name, $null)
"FindWindowW(自建类) = 0x{0:X}" -f $byFind.ToInt64()
$hit = $false
$cb = [T+EnumProc]{
    param($h, $l)
    $cs = New-Object System.Text.StringBuilder 256
    [void][T]::GetClassNameW($h, $cs, 256)
    if ($cs.ToString() -eq $name) { $script:hit = $true }
    return $true
}
[void][T]::EnumWindows($cb, [IntPtr]::Zero)
"EnumWindows 找到自建窗口: $hit"
