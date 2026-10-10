# 文件编码规范

**本项目所有文本文件必须为 UTF-8（无 BOM）。**

- 不允许 UTF-8 BOM（EF BB BF）；不允许 UTF-16/UTF-32。
- 换行：提交时统一由 git 处理（仓库内 LF，检出时按平台）。
- 编辑器请设置：UTF-8 / without BOM。

检查：
```powershell
pwsh -File script\check-encoding.ps1
```
脚本会扫描全仓（跳过 target/ node_modules/ .git/ artifact/ vendor/），发现 BOM 即报错并以非零码退出。
## 写文件时的注意事项（Windows PowerShell 5.1 的坑）

本会话的 shell 是 Windows PowerShell 5.1（`Set-Content -Encoding UTF8` 会**写入 BOM**），
因此生成文本文件一律用显式无 BOM 写法：

```powershell
$noBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($path, $text, $noBom)
```

或先写再核查（BOM = EF BB BF）：
```powershell
$b = [System.IO.File]::ReadAllBytes($path); $b[0] -eq 0xEF -and $b[1] -eq 0xBB -and $b[2] -eq 0xBF
```

提交前跑：`pwsh -File script\check-encoding.ps1`（有 BOM 即非零退出）。