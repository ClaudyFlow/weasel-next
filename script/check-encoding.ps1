# 检查全仓文本文件是否为 UTF-8 无 BOM（有 BOM 则失败）
[CmdletBinding()] param([string]$Root = '')
Set-StrictMode -Version Latest
if ([string]::IsNullOrEmpty($Root)) {
  if ($PSScriptRoot) { $Root = Split-Path -Parent $PSScriptRoot } else { $Root = (Get-Location).Path }
}
$skip = '\\target\\|node_modules|\\.git\\|\\artifact\\|\\vendor\\'
$bad = @()
Get-ChildItem $Root -Recurse -File -ErrorAction SilentlyContinue |
  Where-Object { $_.FullName -notmatch $skip -and $_.Length -gt 0 } |
  ForEach-Object {
    $fs = [System.IO.File]::OpenRead($_.FullName); $b = New-Object byte[] 4
    $n = $fs.Read($b, 0, 4); $fs.Close()
    if ($n -ge 3 -and $b[0] -eq 0xEF -and $b[1] -eq 0xBB -and $b[2] -eq 0xBF) { $bad += "$($_.FullName) : UTF-8 BOM" }
    elseif ($n -ge 2 -and (($b[0] -eq 0xFF -and $b[1] -eq 0xFE) -or ($b[0] -eq 0xFE -and $b[1] -eq 0xFF))) { $bad += "$($_.FullName) : UTF-16/32 BOM" }
  }
if ($bad.Count -gt 0) {
  Write-Host "编码不合规（必须 UTF-8 无 BOM）:" -ForegroundColor Red
  $bad | ForEach-Object { Write-Host "  $_" }
  exit 1
}
Write-Host "编码检查通过：全部 UTF-8 无 BOM" -ForegroundColor Green