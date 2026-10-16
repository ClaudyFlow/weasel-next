# 语法检查：把脚本解析一遍，有错就打印
$errors = $null
$null = [System.Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot 'deploy-install.ps1'), [ref]$null, [ref]$errors)
if ($errors) { $errors | ForEach-Object { "行 $($_.Extent.StartLineNumber): $($_.Message)" } }
else { 'deploy-install.ps1 语法 OK' }
