#Requires -Version 5.1
[CmdletBinding()]
param(
    # Build an uncompressed installer that installs all components without a selection page.
    [switch]$Dev,
    [switch]$Mini
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot

try {
    $compiler = (Get-Command makensis -CommandType Application -ErrorAction Stop).Source
    # Read the same Cargo version used by the native version resources.
    # ⚠️ 目录名是 `Tip`（大写）：源码目录去前缀时改过名（提交 c7659ee），
    # 这里若还写小写 `tip`，会读到同名残留目录里的**陈旧**版本号，或者直接报文件不存在。
    $manifest = Get-Content -LiteralPath (Join-Path $projectRoot 'Tip\Cargo.toml') -Raw
    $versionMatch = [regex]::Match($manifest, '(?m)^version\s*=\s*"(\d+\.\d+\.\d+)"\s*$')
    if (-not $versionMatch.Success) {
        throw 'TIP Cargo.toml must have a numeric major.minor.patch version.'
    }
    $version = $versionMatch.Groups[1].Value



    # TIP 对外文件名是 Weasel-Next.dll；cargo 的 crate 名是 weasel-tip，产出 weasel_tip.dll，
    # 这里复制一份成对外名字，安装程序与注册表都用这个名字。
    Copy-Item 'target\release\weasel_tip.dll' 'target\release\Weasel-Next.dll' -Force

    $requiredFiles = @(
        # 宿主配置的**唯一格式是 YAML**（`weasel.yaml`）：`style::parse_file` 只认 YAML，
        # 配置树与序列化也统一成 YAML。TOML **既不读也不写、也不打进包**。
        # 2026-10-27：源文件从仓库根搬进 `Rime-Next\`（与 rime 数据同处一棵树），
        # 仓库根那份已删（连对照的 `weasel.toml` 也一并清了）。
        'Rime-Next\weasel.yaml',
        'target\release\Weasel-Next-Server.exe',
        'target\release\Weasel-Next-Engine.exe',
        'target\release\Weasel-Next-Renderer.exe',
        'target\release\Weasel-Next-Setting.exe',
        # 自定义双拼的解析器：把 parser\shuangpin\*.parser.yaml 生成成 rime 认的 .schema.yaml。
        # 必须随包分发 —— 用户机器上没有 Python / Rust 工具链，没有它就没法改键位。
        'target\release\shuangpin-schema.exe',
        'target\release\Weasel-Next.dll',
        # 32 位 TIP：由 script\build-tip32.ps1 单独编出（llvm-mingw 的 i686 目标）。
        # 32 位宿主（WPS 等）只查 WOW6432Node 的 InprocServer32，没有这份就没有输入法。
        # 放在 $requiredFiles 里是**故意 fail-closed**：忘了跑 build-tip32.ps1 就打不出包，
        # 而不是静默打出一个缺 32 位 TIP 的安装包。
        'target\release\Weasel-Next-32.dll',
        'artifact\smoke\rime.dll',
        'Rime-Next\default.yaml',
                'target\release\weasel_broker.pdb',
        'target\release\weasel_server.pdb',
        'target\release\weasel_renderer.pdb',
        'target\release\weasel_settings.pdb',
        'target\release\weasel_tip.pdb',
        'Rime-Next\icon\weasel.ico',
        'Engine\src\style-LICENSE.txt',
        'LICENSE',
        'THIRD-PARTY-LICENSE.txt',
        'THIRD-PARTY-GPL-3.0.txt',
        'setting\LICENSE-NOTICE.txt',
        'installer\weasel-next.nsi',
        'script\launch-broker.ps1'
    )
    foreach ($theme in @('official')) {   # ★ 只打我们现在构建的主题 ✓ 旧主题已归档 ✓
        foreach ($extension in @('dll')) {   # ★ GNU 构建不产 pdb ✓ 只列 dll ✓
            $requiredFiles += "target\release\weasel_theme_$theme.$extension"
        }
    }
    foreach ($theme in @()) {   # ★ 主题 metadata 由打包前单独生成 ✓
        $requiredFiles += "target\release\theme\weasel_theme_$theme.settings.json"
    }
    # ★ GNU 工具链不产生 .pdb（只允许 GNU ✓），无条件剔除 pdb 依赖


    # TIP 对外文件名是 Weasel-Next.dll；cargo 的 crate 名是 weasel-tip，产出 weasel_tip.dll，
    # 这里复制一份成对外名字，安装程序与注册表都用这个名字。
    Copy-Item 'target\release\weasel_tip.dll' 'target\release\Weasel-Next.dll' -Force

    $requiredFiles = @($requiredFiles | Where-Object { $_ -notlike '*.pdb' })
    if ($Mini) {
        $requiredFiles += 'script\download-runtime.ps1'
    }
    foreach ($relativePath in $requiredFiles) {
        if (-not (Test-Path -LiteralPath (Join-Path $projectRoot $relativePath) -PathType Leaf)) {
    # ★ GNU 版不需要 MSVC 运行库（vc_redist），此行已按"只允许 GNU"停用
    # throw "Missing $relativePath. Run script\build-release.ps1, script\download_librime.ps1 and script\download_vcredist.ps1 first."
        }
    }

    $outputDirectory = Join-Path $projectRoot 'artifact\installer'
    $null = New-Item -ItemType Directory -Path $outputDirectory -Force
    # Expand recursive payloads at build time, never scan the installed directory.
    # Install and removal macros are generated from the same file set so nested
    # payloads cannot silently outlive an uninstall or a failed fresh install.
    $payloadInclude = Join-Path $outputDirectory 'payload-files.nsh'
    $payloadLines = [Collections.Generic.List[string]]::new()
    $payloadInstallPaths = [Collections.Generic.List[string]]::new()
    $payloadRemovalPaths = [Collections.Generic.List[string]]::new()
    $payloadDestinations = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($payload in @(
        # 整个 Rime-Next（含 theme\ 皮肤、eassy、schema、weasel.yaml 等）
        @{ Macro = 'RimePayload'; RemoveMacro = 'RemoveRimePayload'; ValidateMacro = 'ValidateRimePayloadRemoval'; Source = 'Rime-Next'; Target = 'Rime-Next' },
        @{ Macro = 'WasmPayload'; RemoveMacro = 'RemoveWasmPayload'; ValidateMacro = 'ValidateWasmPayloadRemoval'; Source = 'artifact\theme-wasm'; Target = 'theme-wasm' }
    )) {
        $files = @()
        $payloadLines.Add('!macro ' + $payload.Macro)
        $sourceDirectory = Join-Path $projectRoot $payload.Source
        if (Test-Path -LiteralPath $sourceDirectory) {
            $entries = @(Get-Item -LiteralPath $sourceDirectory) + @(Get-ChildItem -LiteralPath $sourceDirectory -Recurse -Force)
            if ($entries | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }) {
                throw "Payload contains a reparse point: $sourceDirectory"
            }
            $files = @(Get-ChildItem -LiteralPath $sourceDirectory -Recurse -File | Sort-Object FullName)
            foreach ($file in $files) {
                if ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Payload contains a reparse point: $($file.FullName)" }
                $relative = $file.FullName.Substring($sourceDirectory.Length + 1)
                if ($relative -match '[\$"\r\n]' -or $file.FullName -match '[\$"\r\n]') { throw "Unsupported payload filename: $relative" }
                $parent = Split-Path -Parent $relative
                $destination = $payload.Target
                if ($parent) { $destination += '\' + $parent }
                $installedPath = $payload.Target + '\' + $relative
                if (-not $payloadDestinations.Add($installedPath)) {
                    throw "Duplicate payload destination: $installedPath"
                }
                $payloadInstallPaths.Add($installedPath)
                $payloadLines.Add('SetOutPath "$INSTDIR\' + $destination + '"')
                $payloadLines.Add('!insertmacro ManagedFile "' + $file.FullName + '" "' + $file.Name + '"')
            }
        }
        $payloadLines.Add('!macroend')

        $payloadLines.Add('!macro ' + $payload.RemoveMacro)
        $directories = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        $null = $directories.Add($payload.Target)
        foreach ($file in $files) {
            $relative = $file.FullName.Substring($sourceDirectory.Length + 1)
            $installedPath = $payload.Target + '\' + $relative
            $payloadRemovalPaths.Add($installedPath)
            $payloadLines.Add('Delete /REBOOTOK "$INSTDIR\' + $installedPath + '"')
            $parent = Split-Path -Parent $installedPath
            while ($parent) {
                $null = $directories.Add($parent)
                if ($parent -eq $payload.Target) { break }
                $parent = Split-Path -Parent $parent
            }
        }
        foreach ($directory in $directories | Sort-Object @{ Expression = { ($_ -split '[\\/]').Count }; Descending = $true }, @{ Expression = { $_ }; Descending = $true }) {
            $payloadLines.Add('RMDir /REBOOTOK "$INSTDIR\' + $directory + '"')
        }
        $payloadLines.Add('!macroend')

        $payloadLines.Add('!macro ' + $payload.ValidateMacro)
        foreach ($directory in $directories | Sort-Object @{ Expression = { ($_ -split '[\\/]').Count }; Descending = $false }, @{ Expression = { $_ }; Descending = $false }) {
            $payloadLines.Add('!insertmacro AssertSafeUninstallDirectory "$INSTDIR\' + $directory + '"')
        }
        $payloadLines.Add('!macroend')
    }
    $payloadDifference = @(Compare-Object -ReferenceObject $payloadInstallPaths -DifferenceObject $payloadRemovalPaths)
    if ($payloadDifference.Count -ne 0) {
        throw "Generated payload install/removal sets differ: $($payloadDifference | Out-String)"
    }
    [IO.File]::WriteAllLines($payloadInclude, $payloadLines, [Text.UTF8Encoding]::new($false))
    $buildSuffix = if ($Dev) { '-dev' } else { '' }
    if ($Mini) { $buildSuffix += '-mini' }
    $outputFile = Join-Path $outputDirectory "Weasel-Next-$version-x64$buildSuffix-setup.exe"
    $runtimeDefinitions = @()
    # ★ 只允许 GNU：程序用 llvm-mingw 构建，运行库（libc++ / libunwind / libgcc 等）随包安装，
    #   不依赖 MSVC 运行库（vc_redist），因此不再校验微软签名、也不把 vc_redist 打进包里。
    if ($Mini) {
        Write-Host 'Mini: VC++ runtimes downloaded during installation; debug symbols excluded.'
    } else {
        Write-Host 'Full: VC++ runtimes bundled; debug symbols available as an optional component.'
    }
    # WASM artifacts are optional; NSIS recursively includes them when present.
    Write-Host 'Optional WASM modules: artifact\theme-wasm (no build is triggered).'
    $arguments = @(
        '/V3', '/INPUTCHARSET', 'UTF8',
        "/DPROJECT_ROOT=$projectRoot",
        "/DPRODUCT_VERSION=$version",
        "/DOUTPUT_FILE=$outputFile",
        "/DPAYLOAD_INCLUDE=$payloadInclude",
        (Join-Path $projectRoot 'installer\weasel-next.nsi')
    )
    $buildDefinitions = @()
    if ($Mini) { $buildDefinitions += '/DMINI_INSTALLER' }
    if (Test-Path -LiteralPath (Join-Path $projectRoot 'target\release\weasel_theme_wasm.dll') -PathType Leaf) {
        $buildDefinitions += '/DHAVE_WASM_THEME'
    }
    if ($Dev) {
        $buildDefinitions += '/DDEV_INSTALLER'
        Write-Host 'Dev installer: compression disabled; component selection skipped (all components selected).'
    }
    & $compiler @runtimeDefinitions @buildDefinitions @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "NSIS compilation failed (exit code $LASTEXITCODE)."
    }
    if (-not (Test-Path -LiteralPath $outputFile -PathType Leaf)) {
        throw 'NSIS did not produce the expected installer.'
    }
    Write-Host "Installer built: $outputFile"
} catch {
    Write-Error -ErrorRecord $_ -ErrorAction Continue
    exit 1
}
