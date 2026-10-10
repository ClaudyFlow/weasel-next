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