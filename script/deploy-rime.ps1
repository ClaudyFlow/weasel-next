# 一键部署 rime.dll 到装机目录（需管理员身份运行）
# 生成于 2026-10-25 01:45 —— 沙箱内无法提权，留待用户执行
$ErrorActionPreference = 'Stop'
$src = 'C:\code\librime\build-mingw\bin\librime.dll'
$dst = 'C:\Program Files\Weasel-Next\rime.dll'
$expect = '4E678A82A99E474FD21C780B0F2CEC513A1A409EA8F4FCFBD35EA94579F9F79D'

Write-Host '== 1. 停止 Weasel-Next 进程组 =='
Get-Process -Name 'Weasel-Next-Server','Weasel-Next-Engine','Weasel-Next-Renderer' -ErrorAction SilentlyContinue |
    Stop-Process -Force
Start-Sleep -Seconds 1

Write-Host '== 2. 复制 DLL =='
Copy-Item -Path $src -Destination $dst -Force

Write-Host '== 3. SHA256 比对 =='
$actual = (Get-FileHash -Path $dst -Algorithm SHA256).Hash
if ($actual -eq $expect) {
    Write-Host "OK   $actual" -ForegroundColor Green
} else {
    Write-Host "FAIL 期望 $expect" -ForegroundColor Red
    Write-Host "     实际 $actual" -ForegroundColor Red
    exit 1
}

Write-Host '== 4. 重启 Server =='
Start-Process -FilePath 'C:\Program Files\Weasel-Next\Weasel-Next-Server.exe'
Start-Sleep -Seconds 3
Get-Process -Name 'Weasel-Next-Server','Weasel-Next-Engine','Weasel-Next-Renderer' -ErrorAction SilentlyContinue |
    Select-Object Id, ProcessName

Write-Host '== 5. 重新部署（build/ 按新布局重写）=='
# 通过 Weasel-Next-Setting 或直接调用部署入口；此处启动设置面板由用户点"重新部署"
Write-Host '请打开 Weasel-Next-Setting 点一次"重新部署"，或按 F4 后运行部署。' -ForegroundColor Yellow
Write-Host '完成。'
