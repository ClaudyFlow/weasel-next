# 打开官方小狼毫的设置对话框，把里面的候选窗预览（IDC_PREVIEW = 1006）抓成 PNG。
#
# 这是拿"官方渲染器"当基准的最直接办法：同一台机器、同一 DPI、同一套 aqua 配色，
# 官方自己画出来的候选窗长什么样，一看便知。
#
# 用法：pwsh -File script\capture-official-preview.ps1 -Out official-preview.png
param(
    [Parameter(Mandatory = $true)][string]$Out,
    [string]$Deployer = 'C:\Program Files\Rime\weasel-0.17.4\WeaselDeployer.exe',
    [int]$PreviewControlId = 1006,
    [int]$WaitMs = 3000
)

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class W {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    public static string Title(IntPtr h) { var s = new StringBuilder(512); GetWindowTextW(h, s, 512); return s.ToString(); }
}
'@

[void][W]::SetProcessDPIAware()
$proc = Start-Process $Deployer -PassThru
Start-Sleep -Milliseconds $WaitMs

# 找出这次新开的对话框（类 #32770 或标题含 Weasel/小狼毫）
$dialogs = New-Object System.Collections.ArrayList
$cb = [W+EnumProc]{
    param($h, $l)
    if (-not [W]::IsWindowVisible($h)) { return $true }
    $cls = New-Object System.Text.StringBuilder 256
    [void][W]::GetClassNameW($h, $cls, 256)
    $title = [W]::Title($h)
    if ($cls.ToString() -eq '#32770' -or $title -match 'Weasel|小狼毫|rime|Rime') {
        [void]$dialogs.Add([pscustomobject]@{ Hwnd = $h; Class = $cls.ToString(); Title = $title })
    }
    return $true
}
[void][W]::EnumWindows($cb, [IntPtr]::Zero)
Write-Output "顶层窗口："
$dialogs | ForEach-Object { Write-Output ("  {0} [{1}] {2}" -f $_.Hwnd, $_.Class, $_.Title) }

$target = $null
foreach ($dialog in $dialogs) {
    $children = New-Object System.Collections.ArrayList
    $ccb = [W+EnumProc]{
        param($h, $l)
        [void]$children.Add($h)
        return $true
    }
    [void][W]::EnumChildWindows($dialog.Hwnd, $ccb, [IntPtr]::Zero)
    foreach ($child in $children) {
        if ([W]::GetDlgCtrlID($child) -eq $PreviewControlId) { $target = $child; break }
    }
    if ($target) { Write-Output "预览控件在对话框「$($dialog.Title)」里"; break }
}
if (-not $target) {
    Write-Output "没找到 ID=$PreviewControlId 的预览控件；列出子控件供排查："
    foreach ($dialog in $dialogs) {
        $ccb2 = [W+EnumProc]{
            param($h, $l)
            $cls = New-Object System.Text.StringBuilder 256
            [void][W]::GetClassNameW($h, $cls, 256)
            Write-Output ("    id={0} class={1} title={2}" -f [W]::GetDlgCtrlID($h), $cls.ToString(), [W]::Title($h))
            return $true
        }
        [void][W]::EnumChildWindows($dialog.Hwnd, $ccb2, [IntPtr]::Zero)
    }
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    exit 1
}

$rect = New-Object W+RECT
[void][W]::GetWindowRect($target, [ref]$rect)
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
Add-Type -AssemblyName System.Drawing
$bitmap = New-Object System.Drawing.Bitmap $width, $height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
$bitmap.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()
Write-Output "官方预览已抓取: ${width}x${height} @ ($($rect.Left),$($rect.Top)) -> $Out"

# 关掉对话框
foreach ($dialog in $dialogs) { [void][W]::PostMessageW($dialog.Hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) }
Start-Sleep -Milliseconds 500
Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
