# dict-tool.ps1 — 词库管理工具（查重码 / 加词 / 删词 / 备份 / 部署）
#
# 学自蓝宝石 DictStore 的能力拆分：
#   candidatesOf(编码)  → -Find
#   insertPhrase        → -Add
#   deleteEntries       → -Remove
#   backupNow           → （复用 deploy-install.ps1 的 -BackupOnly）
#   deployRime          → （复用 deploy-install.ps1）
#
# 只改「用户目录里已经存在」的 *.dict.yaml，并且**只动目标词条行**：
# 注释、表头（--- ... ...）、其它条目一律原样保留（学自它的 extraLineCount 思路）。
#
# 用法：
#   powershell -File script\dict-tool.ps1 -List
#   powershell -File script\dict-tool.ps1 -Find zh
#   powershell -File script\dict-tool.ps1 -Find zh -Dict custom_phrase
#   powershell -File script\dict-tool.ps1 -Add "测试" "ceshi" -Dict custom_phrase
#   powershell -File script\dict-tool.ps1 -Remove "测试" -Dict custom_phrase
#   powershell -File script\dict-tool.ps1 -Find zh -Deploy

[CmdletBinding()]
param(
    [switch]$List,
    [string]$Find,
    [string]$Add,
    [string]$Code,
    [string]$Remove,
    [string]$Dict,
    [switch]$Deploy,
    [switch]$NoBackup
)

$ErrorActionPreference = 'Stop'
$userDir = Join-Path $env:APPDATA 'Rime-Next'
$scriptDir = $PSScriptRoot
$utf8 = New-Object System.Text.UTF8Encoding($false)

function Get-DictFiles {
    if ($Dict) {
        $candidates = @(
            (Join-Path $userDir ($Dict + '.dict.yaml')),
            (Join-Path $userDir ($Dict + '.yaml'))
        )
        foreach ($path in $candidates) {
            if (Test-Path $path) { return @(Get-Item $path) }
        }
        throw "找不到词典：$Dict（用 -List 看可用的）"
    }
    return @(Get-ChildItem $userDir -File -Filter '*.dict.yaml' -ErrorAction SilentlyContinue)
}

# 解析 dict.yaml：返回 { Word, Code, LineNo, Raw } 列表。
# Rime 词典格式：文件头 --- ... --- 之后，每行 `词\t编码[\t权重]`。
function Read-DictEntries {
    param([string]$Path)
    $lines = [IO.File]::ReadAllLines($Path, [Text.Encoding]::UTF8)
    $entries = New-Object System.Collections.Generic.List[object]
    # Rime 的头部用 --- 开头、... 结束（老格式也有用第二个 --- 结束的）。
    $hasHeader = @($lines | Where-Object { $_.Trim() -eq '---' }).Count -gt 0
    $inHeader = $false
    $headerDone = -not $hasHeader
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        if (-not $headerDone) {
            $trimmed = $line.Trim()
            if ($trimmed -eq '---') {
                if ($inHeader) { $headerDone = $true } else { $inHeader = $true }
                continue
            }
            if ($inHeader -and $trimmed -eq '...') { $headerDone = $true; continue }
            continue
        }
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        if ($line.TrimStart().StartsWith('#')) { continue }
        $parts = $line -split "`t"
        if ($parts.Count -lt 2) { continue }
        $entries.Add([pscustomobject]@{
            Word   = $parts[0]
            Code   = $parts[1]
            Weight = $(if ($parts.Count -ge 3) { $parts[2] } else { '' })
            LineNo = $i
            Raw    = $line
        })
    }
    return $entries
}

function Backup-Now {
    if ($NoBackup) { return }
    $deploy = Join-Path $scriptDir 'deploy-install.ps1'
    if (Test-Path $deploy) {
        & powershell -NoProfile -ExecutionPolicy Bypass -File $deploy -BackupOnly -ForceBackup | Out-Null
        Write-Host '已先备份用户数据（backup\ 目录）'
    }
}

function Invoke-Deploy {
    $deploy = Join-Path $scriptDir 'deploy-install.ps1'
    if (Test-Path $deploy) {
        Write-Host '开始部署（会重启输入法栈）...'
        & powershell -NoProfile -ExecutionPolicy Bypass -File $deploy -SkipBuild
    } else {
        Write-Warning '找不到 deploy-install.ps1，请手动「保存并部署」'
    }
}

# ── -List ──────────────────────────────────────────────────────────────
if ($List) {
    $files = @(Get-ChildItem $userDir -File -Filter '*.dict.yaml' -ErrorAction SilentlyContinue)
    if ($files.Count -eq 0) { '用户目录根部没有 *.dict.yaml'; return }
    foreach ($file in $files) {
        $entries = Read-DictEntries -Path $file.FullName
        '{0,-34} {1,7} 条' -f $file.Name, $entries.Count
    }
    return
}

# ── -Find ──────────────────────────────────────────────────────────────
if ($Find) {
    $needle = $Find.Trim().ToLower()
    $total = 0
    foreach ($file in Get-DictFiles) {
        if (-not $file) { continue }
        $entries = Read-DictEntries -Path $file.FullName
        $hits = @($entries | Where-Object { $_.Code -ieq $needle -or $_.Code.ToLower().StartsWith($needle) })
        if ($hits.Count -eq 0) { continue }
        "== {0} ==" -f $file.Name
        $rank = 0
        foreach ($hit in $hits) {
            $rank++
            '  {0,3}. {1,-10} {2,-12} {3}' -f $rank, $hit.Word, $hit.Code, $(if ($hit.Weight) { '权重 ' + $hit.Weight } else { '' })
        }
        $total += $hits.Count
    }
    if ($total -eq 0) { "没有匹配 '$Find' 的词条" } else { "共 $total 条" }
    if ($Deploy) { Invoke-Deploy }
    return
}

# ── -Add ───────────────────────────────────────────────────────────────
if ($Add) {
    if (-not $Code) { throw '-Add 需要同时给 -Code "<编码>"' }
    $target = Get-DictFiles | Select-Object -First 1
    if (-not $target) { throw '没有可写的词典；用 -Dict 指定，或先建一个 *.dict.yaml' }
    $code = $Code.Trim().ToLower()
    $word = $Add.Trim()
    $entries = Read-DictEntries -Path $target.FullName
    # 冲突检测（学自 codeExists / isSupersetWord）
    $same = @($entries | Where-Object { $_.Code -ieq $code })
    $dup = @($entries | Where-Object { $_.Word -eq $word -and $_.Code -ieq $code })
    if ($dup.Count -gt 0) {
        "已存在：{0} → {1}（{2} 第 {3} 行），未改动" -f $word, $code, $target.Name, ($dup[0].LineNo + 1)
        return
    }
    if ($same.Count -gt 0) {
        '该编码已有 {0} 个词：{1}' -f $same.Count, (($same | Select-Object -First 5 | ForEach-Object { $_.Word }) -join '、')
        '新词会追加到末尾（名次靠后），如需靠前请用 -Remove 后重加，或手改顺序'
    }
    Backup-Now
    $lines = New-Object System.Collections.Generic.List[string]
    $lines.AddRange([IO.File]::ReadAllLines($target.FullName, [Text.Encoding]::UTF8))
    # 插到同编码最后一条之后；没有同编码就插到文件末尾（保持表头完整）
    if ($same.Count -gt 0) {
        $insertAt = ($same | ForEach-Object { $_.LineNo } | Measure-Object -Maximum).Maximum + 1
    } else {
        $insertAt = $lines.Count
    }
    $lines.Insert($insertAt, ($word + "`t" + $code))
    [IO.File]::WriteAllLines($target.FullName, $lines, $utf8)
    "已加入：{0} → {1}（{2} 第 {3} 行）" -f $word, $code, $target.Name, ($insertAt + 1)
    if ($Deploy) { Invoke-Deploy }
    return
}

# ── -Remove ────────────────────────────────────────────────────────────
if ($Remove) {
    $word = $Remove.Trim()
    $removed = 0
    foreach ($file in Get-DictFiles) {
        if (-not $file) { continue }
        $entries = Read-DictEntries -Path $file.FullName
        $hit = @($entries | Where-Object { $_.Word -eq $word })
        if ($hit.Count -eq 0) { continue }
        Backup-Now
        $lines = New-Object System.Collections.Generic.List[string]
        $lines.AddRange([IO.File]::ReadAllLines($file.FullName, [Text.Encoding]::UTF8))
        # 从后往前删，避免行号错位
        foreach ($lineNo in ($hit | ForEach-Object { $_.LineNo } | Sort-Object -Descending)) {
            $lines.RemoveAt($lineNo)
        }
        [IO.File]::WriteAllLines($file.FullName, $lines, $utf8)
        "已从 {0} 删除 {1} 条：{2}" -f $file.Name, $hit.Count, $word
        $removed += $hit.Count
    }
    if ($removed -eq 0) { "没有找到词条：$word" }
    if ($Deploy) { Invoke-Deploy }
    return
}

Write-Host '至少给一个动作：-List / -Find <编码> / -Add <词> -Code <编码> / -Remove <词>'
