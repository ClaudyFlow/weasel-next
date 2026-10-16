# 实机验证：造一个临时编辑窗（WinForms 文本框），给它打字，抓候选窗，然后关掉它。
#
# 为什么不用记事本：Win11 的 notepad.exe 只是启动器（Store 应用，进程会立刻退出），
# 拿不到稳定的窗口句柄；自己造的 WinForms 窗口句柄、焦点、消息循环都在手里。
#
# 用途：验证"实时模式"的候选窗 —— 预览模式走另一条定位分支（preview_position），
# anchor 定位、上翻 sticky 这些只有实时模式才走到。
#
# 用法: pwsh -File script\live-type-check.ps1 -Out live-shot.png [-Text wenjian]
param(
    [Parameter(Mandatory = $true)][string]$Out,
    [string]$Text = 'wenjian',
    [int]$WaitAfterTypeMs = 1500
)

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class LV {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint mapType);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
'@
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[void][LV]::SetProcessDPIAware()

$form = New-Object System.Windows.Forms.Form
$form.Text = 'Weasel-Next 候选窗实机验证（临时窗口，用完即关）'
$form.Width = 720
$form.Height = 200
$form.StartPosition = 'CenterScreen'
$form.TopMost = $true
$box = New-Object System.Windows.Forms.TextBox
$box.Multiline = $true
$box.Dock = 'Fill'
$box.Font = New-Object System.Drawing.Font('Microsoft YaHei', 14)
$form.Controls.Add($box)
$form.Add_Shown({ $box.Focus() })
$form.Show()
for ($i = 0; $i -lt 20; $i++) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100 }

$padHwnd = $form.Handle
# Windows 的前台上锁：从后台进程直接 SetForegroundWindow 会被忽略。
# 先合成一次 ALT 敲击（不产生任何输入内容）解开上锁，这是标准做法。
for ($i = 0; $i -lt 12; $i++) {
    [System.Windows.Forms.Application]::DoEvents()
    if ([LV]::GetForegroundWindow() -eq $padHwnd) { break }
    [LV]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
    [LV]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
    [void][LV]::SetForegroundWindow($padHwnd)
    [void]$form.Activate()
    Start-Sleep -Milliseconds 250
}
if ([LV]::GetForegroundWindow() -ne $padHwnd) {
    $form.Close()
    Write-Error '临时窗口没能拿到焦点（拒绝在不确定焦点的窗口上打字）'
    exit 2
}
$box.Focus()
[System.Windows.Forms.Application]::DoEvents()

Write-Output "向临时窗口发送：$Text"
# 必须用**虚拟键事件**打字：SendKeys 对普通字符发的是 KEYEVENTF_UNICODE 文本事件，
# 会绕过键盘布局/输入法（实测：输入法只当收到一段文本，不启动组字，快照一直是 visible=false）。
# keybd_event 走的是正常输入队列，输入法才认。
foreach ($ch in $Text.ToCharArray()) {
    $vk = [byte][char]([char]::ToUpper($ch))
    $scan = [byte][LV]::MapVirtualKey($vk, 0)
    [LV]::keybd_event($vk, $scan, 0, [UIntPtr]::Zero)
    [LV]::keybd_event($vk, $scan, 2, [UIntPtr]::Zero)
    [System.Windows.Forms.Application]::DoEvents()
    Start-Sleep -Milliseconds 120
}
$deadline = (Get-Date).AddMilliseconds($WaitAfterTypeMs)
while ((Get-Date) -lt $deadline) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 50 }

# 找可见候选窗（回调里要用 script: 作用域）
$script:candHwnd = [IntPtr]::Zero
$script:candPid = 0
$grab = [LV+EnumProc]{
    param($h, $l)
    $cls = New-Object System.Text.StringBuilder 256
    [void][LV]::GetClassNameW($h, $cls, 256)
    if ($cls.ToString() -eq 'Weasel.ThemeOfficial.Official' -and [LV]::IsWindowVisible($h)) {
        $owner = 0
        [void][LV]::GetWindowThreadProcessId($h, [ref]$owner)
        $script:candHwnd = $h
        $script:candPid = $owner
    }
    return $true
}
[void][LV]::EnumWindows($grab, [IntPtr]::Zero)

if ($script:candHwnd -eq [IntPtr]::Zero) {
    Write-Output '候选窗：没找到可见的候选窗 ✗'
} else {
    $rect = New-Object LV+RECT
    [void][LV]::GetWindowRect($script:candHwnd, [ref]$rect)
    $w = $rect.Right - $rect.Left
    $h = $rect.Bottom - $rect.Top
    Write-Output "候选窗：hwnd=$($script:candHwnd) pid=$($script:candPid) 位置=$($rect.Left),$($rect.Top) 尺寸=${w}x${h}"
    $bitmap = New-Object System.Drawing.Bitmap ($w + 24), ($h + 24)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($rect.Left - 12, $rect.Top - 12, 0, 0, $bitmap.Size)
    $bitmap.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose()
    $bitmap.Dispose()
    Write-Output "已抓图 -> $Out"
}

[System.Windows.Forms.SendKeys]::SendWait('{ESC}')
for ($i = 0; $i -lt 6; $i++) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 50 }
$form.Close()
Write-Output '临时窗口已关闭'
