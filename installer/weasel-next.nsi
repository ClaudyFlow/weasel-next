Unicode true
ManifestDPIAware true
RequestExecutionLevel admin
!ifdef DEV_INSTALLER
  SetCompress off
!else
  SetCompressor /SOLID lzma
!endif

!include MUI2.nsh
!include LogicLib.nsh
; x64.nsh 提供 ${RunningX64}${IsNativeARM64} 等平台判断宏。
!include x64.nsh
!include FileFunc.nsh
!include TextFunc.nsh
!include Sections.nsh
!include WordFunc.nsh
!include WinVer.nsh

ManifestSupportedOS all

!ifndef PROJECT_ROOT
  !error "Use script\build-installer.ps1 to supply PROJECT_ROOT."
!endif
; 构建输出目录：llvm-mingw 的产物。64 位 TIP 是 Weasel-Next.dll（cargo build --release），
; 32 位 TIP 是 Weasel-Next-32.dll（script\build-tip32.ps1）。两者并排装进 $INSTDIR，
; 注册表按视图各指一份 —— 32 位宿主（WPS 等）只查 WOW6432Node。
!ifndef RELEASE_DIR
!define RELEASE_DIR "${PROJECT_ROOT}\target\release"
!endif

!ifndef PRODUCT_VERSION
  !error "Missing PRODUCT_VERSION."
!endif
!ifndef OUTPUT_FILE
  !error "Missing OUTPUT_FILE."
!endif

!define PRODUCT_NAME "小狼毫Next"

!define PRODUCT_KEY "Software\Weasel-Next"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Weasel-Next"
; 版本化安装根（用户 2026-10-28 定）：每个版本一个目录、可并存，互不覆盖。
;   C:\Program Files\Rime-Next\Weasel-Next-<版本>\
; 产品级登记（PRODUCT_KEY / UNINSTALL_KEY）只指向**当前版本**；旧版本目录保留可回滚。
!define PRODUCT_ROOT "$PROGRAMFILES64\Rime-Next"
!define PRODUCT_DIR "$PROGRAMFILES64\Rime-Next\Weasel-Next-${PRODUCT_VERSION}"

Name "${PRODUCT_NAME}"
OutFile "${OUTPUT_FILE}"
InstallDir "${PRODUCT_DIR}"
VIProductVersion "${PRODUCT_VERSION}.0"
VIAddVersionKey /LANG=2052 "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey /LANG=2052 "FileDescription" "小狼毫Next 安装程序"
VIAddVersionKey /LANG=2052 "FileVersion" "${PRODUCT_VERSION}"
VIAddVersionKey /LANG=2052 "ProductVersion" "${PRODUCT_VERSION}"
VIAddVersionKey /LANG=2052 "LegalCopyright" "Weasel-Next contributors"

!define MUI_ICON "${PROJECT_ROOT}\Rime-Next\icon\weasel.ico"
!define MUI_UNICON "${PROJECT_ROOT}\Rime-Next\icon\weasel.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TEXT "欢迎安装小狼毫 RS，中州韵输入法的 Windows 文本服务框架前端。"
!insertmacro MUI_PAGE_WELCOME
!define MUI_LICENSEPAGE_CHECKBOX
!insertmacro MUI_PAGE_LICENSE "${PROJECT_ROOT}\LICENSE"
; 用户 2026-10-29：程序装到哪儿要能自己选。默认仍是版本化目录
; （`...\Rime-Next\Weasel-Next-<版本>`，同版本覆盖修复、不同版本并存），
; 改了就装到用户给的目录（此时不再追加版本号）。
!define MUI_DIRECTORYPAGE_TEXT_TOP "小狼毫Next 将安装到下面的目录。默认是按版本分的目录（同版本覆盖修复、不同版本并存），也可以改成任意目录。"
!insertmacro MUI_PAGE_DIRECTORY
; 用户文件夹（Rime 用户数据：方案/词库/配置/个人记录）也可以自选位置。
Page custom UserDirPageCreate UserDirPageLeave
; 不再提供「选择组件」页（用户 2026-10-28 要求）：一律全装，省一次点击。
; 所有 section 在 .onInit 里由 SelectAllComponents 勾上（含 /o 的可选 section）。
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_REBOOTLATER_DEFAULT
; ⚠️ 用户文件夹的变量必须在下面这两个 `!define` **之前**声明：MUI_PAGE_FINISH 展开时
; 就会解析结束页文本，晚于这里声明的 `$UserDir` 会被当成未知常量（warning 6000）。
Var UserDir
Var UserDirField
Var UserDirDefault
; 把**实际安装目录**写到结束页：版本化安装后一个版本一个目录，用户报过「显示的路径
; 还是旧版本号」——写出来就不必猜（$INSTDIR 在运行期展开）。
; 用户文件夹也一并写出来：自选位置时，用户要能一眼看到数据落在哪。
!define MUI_FINISHPAGE_TEXT_REBOOT "小狼毫Next 已安装到：$\r$\n$INSTDIR$\r$\n$\r$\n用户文件夹：$\r$\n$UserDir$\r$\n$\r$\n部分正在使用的 DLL 或运行库需要重启后才能完成更新。请保存工作后重启计算机。"
!define MUI_FINISHPAGE_TEXT "小狼毫Next 已安装到：$\r$\n$INSTDIR$\r$\n$\r$\n用户文件夹：$\r$\n$UserDir"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH
!insertmacro MUI_LANGUAGE "SimpChinese"

Var InstallerMutex
; 用户文件夹（Rime 用户数据）：默认 `%APPDATA%\Rime-Next`，用户可改。
; 选默认位置时**不写**指针文件（保持历史行为）；改了才在程序目录写 `userdir.txt`。
; 三个变量本身在上面 MUI_PAGE_FINISH 之前就声明了（结束页文本要引用 `$UserDir`）。
; .onInit 读到的"上一次安装"信息，留到安装段里对着**最终** $INSTDIR 判断。
Var PreviousInstallDir
Var PreviousVersion
Var IsUpgrade
Var ForceStopUnregisteredInstall
Var InstalledVersion
Var OldDll
Var BackupDll
Var BackupList
Var RetireResult
Var TransactionCommitted
Var PreviousTip
Var Tip32Exit

!include "${PROJECT_ROOT}\installer\records.nsh"
!include "${PROJECT_ROOT}\installer\cleanup-backups.nsh"
!include "${PROJECT_ROOT}\installer\processes.nsh"
!include "${PROJECT_ROOT}\installer\launch-broker.nsh"
!include "${PAYLOAD_INCLUDE}"

Function .onInstSuccess
  ${If} ${RebootFlag}
    SetErrorLevel 3010
  ${EndIf}
FunctionEnd

!macro LockInstaller
  ; Keep the handle until process exit; serialize install/uninstall across sessions.
  System::Call 'kernel32::CreateMutexW(p0, i0, w "Global\Weasel-Next-Installer") p.r0 ?e'
  Pop $1
  StrCpy $InstallerMutex $0
  ${If} $0 == 0
  ${OrIf} $1 == 183
    MessageBox MB_OK|MB_ICONSTOP "已有安装或卸载程序运行，或无法取得安装锁。" /SD IDOK
    SetErrorLevel 1
    Quit
  ${EndIf}
!macroend

!macro CheckPlatform
!ifdef TARGET_ARM64
  ${IfNot} ${IsNativeARM64}
    MessageBox MB_OK|MB_ICONSTOP "小狼毫Next（ARM64 版）仅支持 Windows on ARM（ARM64），不支持 x64 或 32 位系统。" /SD IDOK
    SetErrorLevel 1
    Quit
  ${EndIf}
!else
  ${IfNot} ${IsNativeAMD64}
    MessageBox MB_OK|MB_ICONSTOP "小狼毫Next 仅支持 64 位 Windows（x64），不支持 32 位或 ARM64 系统。" /SD IDOK
    SetErrorLevel 1
    Quit
  ${EndIf}
!endif
  ${IfNot} ${AtLeastBuild} 17763
    MessageBox MB_OK|MB_ICONSTOP "需要 Windows 10 1809 或更新版本。" /SD IDOK
    SetErrorLevel 1
    Quit
  ${EndIf}
  SetShellVarContext all
!macroend

Function .onInit
  ; ⚠️ `CheckPlatform` 里的 `SetShellVarContext all` 会把 `$APPDATA` 变成
  ; `C:\ProgramData`（2026-10-29 实测：before=`C:\Users\X\AppData\Roaming`，
  ; after-all=`C:\ProgramData`）。用户文件夹的默认值必须是**当前用户**的 Roaming，
  ; 所以在 CheckPlatform 之前先取下来 —— 否则默认位置会错成 ProgramData。
  StrCpy $UserDirDefault "$APPDATA\Rime-Next"
  !insertmacro CheckPlatform
  !insertmacro LockInstaller
  ; 没有组件选择页 ⇒ 在这里把每个 section 都勾上（含 /o 可选 section），等价于"全装"。
  Call SelectAllComponents
  StrCpy $TransactionCommitted 0
  StrCpy $PreviousTip ""
    StrCpy $IsUpgrade 0
  StrCpy $ForceStopUnregisteredInstall 0
  StrCpy $ScanExtraDir ""
  ; ⚠️ 这里**不要**再 `StrCpy $INSTDIR "${PRODUCT_DIR}"`：`InstallDir` 已经给了默认值，
  ; 而命令行 `/D=` 会在 .onInit 之前写进 $INSTDIR —— 覆盖它等于把 `/D=` 吃掉。
  ; 「同一份安装在哪 / 要不要覆盖修复」统一挪到安装段（用户可能在目录页里改了路径）。
  ; makensis emits an x86 installer, while product metadata is intentionally
  ; machine-wide in the 64-bit registry view.
  SetRegView 64
  ReadRegStr $PreviousInstallDir HKLM "${PRODUCT_KEY}" "InstallDir"
  ReadRegStr $InstalledVersion HKLM "${PRODUCT_KEY}" "CurrentVersion"
  ${If} $InstalledVersion == ""
    ReadRegStr $InstalledVersion HKLM "${UNINSTALL_KEY}" "DisplayVersion"
  ${EndIf}
  StrCpy $PreviousVersion "$InstalledVersion"
  ; 用户文件夹（用户 2026-10-29 要求可自选）：
  ;   `/USERDIR=` 优先（静默安装），否则沿用已记录的 `userdir.txt`，
  ;   都没有才回退默认 `%APPDATA%\Rime-Next`（= $UserDirDefault，上面已取）。
  StrCpy $UserDir "$UserDirDefault"
  ${GetParameters} $R0
  ${GetOptions} $R0 "/USERDIR=" $R1
  ${If} $R1 != ""
    StrCpy $UserDir "$R1"
  ${Else}
    ; 目标目录里的指针优先；目标目录是新的（跨版本升级装到新的版本化目录）时，
    ; 再看**上一次安装目录**里的指针 —— 否则用户选过的位置会在升级时被静默重置。
    StrCpy $R5 "$INSTDIR\userdir.txt"
    ${IfNot} ${FileExists} "$R5"
      ${If} $PreviousInstallDir != ""
        StrCpy $R5 "$PreviousInstallDir\userdir.txt"
      ${EndIf}
    ${EndIf}
    ${If} ${FileExists} "$R5"
      ClearErrors
      FileOpen $R4 "$R5" r
      ${IfNot} ${Errors}
        FileRead $R4 $R3
        FileClose $R4
        ${TrimNewLines} "$R3" $R3
        ${If} $R3 != ""
          StrCpy $UserDir "$R3"
        ${EndIf}
      ${EndIf}
    ${EndIf}
  ${EndIf}
  ; Remember the previously registered TIP so a failed replacement can restore it.
  ; A non-owned registration needs explicit approval but is not an install blocker.
  ; 注意：这里只查 **64 位视图**（产品主形态）。32 位视图的 InprocServer32 由下面的
  ; 注册段无条件覆盖 —— 那个 CLSID 是本产品私有的，不存在"别人的注册"可保护。
  SetRegView 64
  ReadRegStr $1 HKCR "CLSID\{16A7AEA9-9EE6-4540-8020-E384C0489BB1}\InprocServer32" ""
  StrCpy $PreviousTip "$1"
  ${If} $1 != ""
    ; 版本化安装后，“自己的注册”可能是任意一个版本目录，也可能是旧的非版本化目录。
    ; 只有落在别处的注册才算“别人的 TIP”，需要用户确认。
    ; （前缀判断用 StrLen+StrCpy，避免依赖 TextFunc 的 StrStr 宏。）
    StrLen $2 "${PRODUCT_ROOT}\"
    StrCpy $3 "$1" $2
    ${If} $3 != "${PRODUCT_ROOT}\"
    ${AndIf} $1 != "$PROGRAMFILES64\Weasel-Next\Weasel-Next.dll"
      Goto foreign_registration
    ${EndIf}
  ${EndIf}
  Goto registration_checked
  foreign_registration:
    MessageBox MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2 "检测到不属于当前安装记录的 TIP 注册：$\r$\n$\r$\n$PreviousTip$\r$\n$\r$\n继续安装将用当前版本替换这个注册。是否继续？" /SD IDYES IDYES registration_checked
    SetErrorLevel 1
    Quit
  registration_checked:
FunctionEnd

; 「用户文件夹」页：Rime 用户数据（方案/词库/配置/个人记录）放哪儿。
;
; 用户 2026-10-29 要求可自选（默认 `%APPDATA%\Rime-Next`，有人想放非系统盘/同步盘）。
; 选择结果不写注册表，而是落到**程序目录**的 `userdir.txt`（一行绝对路径）；
; 各组件统一用 `weasel_common::process::user_data_dir()` 解析：
; 环境变量 `WEASEL_NEXT_USER_DIR` > `userdir.txt` > `%APPDATA%\Rime-Next`。
; 选默认位置时不写文件，老机器（没有这个文件）行为完全不变。
Function UserDirPageCreate
  !insertmacro MUI_HEADER_TEXT "用户文件夹" "选择方案、词库、配置和个人记录存放的位置。"
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 26u "用户文件夹存放输入方案、词库、配置与个人记录。$\r$\n默认在 %APPDATA%\Rime-Next；也可以改到非系统盘或同步盘（建议用空目录）。"
  Pop $0
  ${NSD_CreateText} 0 32u 78% 13u "$UserDir"
  Pop $UserDirField
  ${NSD_CreateButton} 80% 32u 20% 13u "浏览…"
  Pop $0
  ${NSD_OnClick} $0 UserDirBrowse
  ${NSD_CreateLabel} 0 52u 100% 20u "提示：多用户共用一台机器时，这个位置会被所有使用本输入法的账户共用。"
  Pop $0
  nsDialogs::Show
FunctionEnd

Function UserDirBrowse
  ${NSD_GetText} $UserDirField $0
  ${If} $0 == ""
    StrCpy $0 "$UserDir"
  ${EndIf}
  nsDialogs::SelectFolderDialog "选择用户文件夹" "$0"
  Pop $0
  ${If} $0 != error
    ${NSD_SetText} $UserDirField "$0"
  ${EndIf}
FunctionEnd

; 判断一个路径是不是绝对路径：`X:\...` 或 `\\server\share`。是则 $R9=1。
;
; NSIS 没有现成的"是否绝对路径"宏，用 StrCpy 取前缀比较；比引入 ${GetRoot} 之类
; 的 FileFunc 细节更直白，也不会被 "C:" 这种驱动器相对路径糊弄过去（要求带分隔符）。
Function IsAbsolutePath
  StrCpy $R9 0
  ${If} $R0 == ""
    Return
  ${EndIf}
  StrCpy $R1 "$R0" 2
  ${If} $R1 == "\\"
    StrCpy $R9 1
    Return
  ${EndIf}
  StrCpy $R1 "$R0" 1 1
  ${If} $R1 == ":"
    StrCpy $R1 "$R0" 1 2
    ${If} $R1 == "\"
    ${OrIf} $R1 == "/"
      StrCpy $R9 1
    ${EndIf}
  ${EndIf}
FunctionEnd

Function UserDirPageLeave
  ${NSD_GetText} $UserDirField $UserDir
  ; 去掉包围的引号与行尾多余的反斜杠（复制粘贴常见）。
  ${TrimNewLines} "$UserDir" $UserDir
  StrCpy $0 "$UserDir" 1
  ${If} $0 == '"'
    StrCpy $UserDir "$UserDir" -1 1
    StrCpy $0 "$UserDir" "" -1
    ${If} $0 == '"'
      StrCpy $UserDir "$UserDir" -1
    ${EndIf}
  ${EndIf}
  ${If} $UserDir == ""
    StrCpy $UserDir "$UserDirDefault"
  ${EndIf}
  StrCpy $0 "$UserDir" "" -1
  ${If} $0 == "\"
    StrCpy $UserDir "$UserDir" -1
  ${EndIf}
  StrCpy $R0 "$UserDir"
  Call IsAbsolutePath
  ${If} $R9 != 1
    MessageBox MB_OK|MB_ICONSTOP "用户文件夹必须是绝对路径（例如 D:\RimeData）。$\r$\n$\r$\n当前：$UserDir" /SD IDOK
    Abort
  ${EndIf}
FunctionEnd

; 安装段开头调用：对着**最终**的 $INSTDIR（用户可能在目录页改过）判断
; 「同版本覆盖修复 / 不同版本并存 / 非空目录需确认」，并算出要额外扫描的旧目录。
;
; 这一段原来在 .onInit 里，用的一直是默认目录；加了目录页之后必须挪到选完目录再判。
Function CheckTargetDirectory
  ; 版本化升级：旧版本进程在另一个目录里，扫描与优雅退出都要照顾到。
  StrCpy $ScanExtraDir ""
  ${If} $PreviousInstallDir != ""
  ${AndIf} $PreviousInstallDir != "$INSTDIR"
    StrCpy $ScanExtraDir "$PreviousInstallDir"
  ${EndIf}
  ; 版本化安装：不同版本各占一个目录，**不再**覆盖旧版本。
  ; 只有「同一个目录里已经有本产品的服务端」才算重复安装（提示覆盖修复）。
  ${If} ${FileExists} "$INSTDIR\Weasel-Next-Server.exe"
    StrCpy $IsUpgrade 1
    MessageBox MB_OKCANCEL|MB_ICONINFORMATION "目标目录里已经有一份小狼毫Next：$\r$\n$INSTDIR$\r$\n$\r$\n继续将覆盖修复它（用户数据不受影响）。" /SD IDOK IDOK check_dir_upgrade_ok
    Abort
    check_dir_upgrade_ok:
  ${ElseIf} $PreviousInstallDir != ""
    ; 旧版本仍在：并存安装，新目录装好后把 TIP 与自启动切到新版本。
    ; 旧布局（非版本化目录）单独讲清楚，免得用户以为新版要覆盖老目录。
    StrLen $2 "${PRODUCT_ROOT}\"
    StrCpy $3 "$PreviousInstallDir" $2
    ${If} $3 != "${PRODUCT_ROOT}\"
      MessageBox MB_OKCANCEL|MB_ICONINFORMATION "检测到旧布局安装 $PreviousVersion（$PreviousInstallDir）。$\r$\n$\r$\n${PRODUCT_VERSION} 将安装到：$\r$\n$INSTDIR$\r$\n$\r$\n旧目录不会自动删除；确认新版本可用后，可手动卸载或删除旧目录。" /SD IDOK IDOK check_dir_side_ok
    ${Else}
      MessageBox MB_OKCANCEL|MB_ICONINFORMATION "检测到已安装版本 $PreviousVersion（$PreviousInstallDir）。$\r$\n$\r$\n${PRODUCT_VERSION} 将安装到：$\r$\n$INSTDIR$\r$\n$\r$\n旧版本目录保留、可回滚；输入法与开机自启动会切到新版本。" /SD IDOK IDOK check_dir_side_ok
    ${EndIf}
    Abort
    check_dir_side_ok:
  ${EndIf}
  ; 没有安装记录、目录却不为空：允许覆盖，但必须先征得同意。
  StrCmp $IsUpgrade 1 check_dir_done
  IfFileExists "$INSTDIR\*.*" 0 check_dir_done
    MessageBox MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2 "目标目录不是空的：$\r$\n$INSTDIR$\r$\n$\r$\n继续会把本产品的文件写进去（不会删除目录里原有的其他文件）。是否继续？" /SD IDYES IDYES check_dir_unregistered
    Abort
    check_dir_unregistered:
    StrCpy $ForceStopUnregisteredInstall 1
  check_dir_done:
FunctionEnd

; Called only during the installation phase, never while browsing its pages.
Function StopApplicationProcesses
  StrCpy $ScanPassLimit 4
  StrCpy $ScanMode 0
  Call ScanApplicationProcesses
  StrCmp $ScanResult 0 processes_stopped
  StrCmp $ScanResult 10 0 process_error
  StrCmp $ForceStopUnregisteredInstall 1 force_stop
  ; Use the bundled new broker, because an old installed broker may not know
  ; --shutdown. This command does not start a tray or managed children.
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File /oname=shutdown-broker.exe "${RELEASE_DIR}\Weasel-Next-Server.exe"
  DetailPrint "正在请求算法服务正常退出……"
  nsExec::ExecToLog /TIMEOUT=40000 '"$PLUGINSDIR\shutdown-broker.exe" --shutdown "$INSTDIR"'
  Pop $0
  !insertmacro InstallLog "INFO: 退出命令返回：$0"
  ; 版本化升级：旧版本 broker 在旧目录里跑着，也要让它先正常退出；
  ; 同一个 broker 的 shutdown 命令会校验进程镜像路径，指向旧目录是安全的。
  ${If} $ScanExtraDir != ""
    DetailPrint "正在请求旧版本算法服务退出（$ScanExtraDir）……"
    nsExec::ExecToLog /TIMEOUT=40000 '"$PLUGINSDIR\shutdown-broker.exe" --shutdown "$ScanExtraDir"'
    Pop $0
    !insertmacro InstallLog "INFO: 旧版本退出命令返回：$0"
  ${EndIf}
  SetOutPath "$INSTDIR"
  StrCpy $ScanMode 0
  Call ScanApplicationProcesses
  StrCmp $ScanResult 0 processes_stopped
  StrCmp $ScanResult 10 0 process_error
  MessageBox MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2 "算法服务未能正常退出。是否强制结束仍在运行的小狼毫Next服务进程？" /SD IDYES IDYES force_stop
  SetErrorLevel 1
  Abort
  force_stop:
  !insertmacro InstallLog "INFO: 正在强制结束未退出的小狼毫Next服务进程"
  StrCpy $ScanMode 1
  Call ScanApplicationProcesses
  StrCmp $ScanResult 0 0 process_error
  StrCpy $ScanMode 0
  Call ScanApplicationProcesses
  StrCmp $ScanResult 0 processes_stopped
  process_error:
    MessageBox MB_OK|MB_ICONSTOP "无法检查或结束小狼毫Next 进程，请手动退出相关程序后重试。" /SD IDOK
    SetErrorLevel 1
    Abort
  processes_stopped:
FunctionEnd

; Rename before extraction: a loaded TIP must never be overwritten in place.
; GetTempFileName reserves a unique sibling, not a shared .old filename.
Function RetireDll
  StrCpy $RetireResult 1
  IfFileExists "$OldDll" 0 retire_done
  ${GetParent} "$OldDll" $0
  GetTempFileName $BackupDll "$0"
  Delete "$BackupDll"
  ${GetFileName} "$BackupDll" $1
  StrCpy $BackupDll "$OldDll.old.$1"
  ClearErrors
  Rename "$OldDll" "$BackupDll"
  ${If} ${Errors}
    StrCpy $RetireResult 0
    Return
  ${EndIf}
  ; Retain backups until all installation steps succeed.
  ClearErrors
  FileOpen $0 "$PLUGINSDIR\retired-dlls.txt" a
  ${If} ${Errors}
    Rename "$BackupDll" "$OldDll"
    StrCpy $RetireResult 0
    Return
  ${EndIf}
  FileWrite $0 "$BackupDll$\r$\n"
  ${If} ${Errors}
    FileClose $0
    Rename "$BackupDll" "$OldDll"
    StrCpy $RetireResult 0
    Return
  ${EndIf}
  FileClose $0
  retire_done:
FunctionEnd

Function DeleteRetiredDlls
  ClearErrors
  FileOpen $BackupList "$PLUGINSDIR\retired-dlls.txt" r
  IfErrors cleanup_done
  cleanup_next:
    ClearErrors
    FileRead $BackupList $BackupDll
    IfErrors cleanup_close
    ; Each record ends in CRLF; remove it before using the literal path.
    StrCpy $BackupDll $BackupDll -2
    ; Only the renamed OLD file is scheduled, never the new DLL's path.
    Delete /REBOOTOK "$BackupDll"
    Goto cleanup_next
  cleanup_close:
    FileClose $BackupList
  cleanup_done:
FunctionEnd


; Explicit file list: never recursively remove the install root or user data.
!macro AssertSafeUninstallDirectory DIRECTORY
  ClearErrors
  ${un.GetFileAttributes} "${DIRECTORY}" "REPARSE_POINT" $0
  ${IfNot} ${Errors}
  ${AndIf} $0 == 1
    MessageBox MB_OK|MB_ICONSTOP "安装目录包含重定向目录，拒绝删除：${DIRECTORY}" /SD IDOK
    SetErrorLevel 1
    Quit
  ${EndIf}
  ClearErrors
!macroend

!macro RemoveProgramFiles
  !insertmacro RemoveRimePayload
  !insertmacro RemoveWasmPayload
  Delete /REBOOTOK "$INSTDIR\Weasel-Next-Server.exe"
  Delete /REBOOTOK "$INSTDIR\Weasel-Next-Engine.exe"
  Delete /REBOOTOK "$INSTDIR\Weasel-Next-Renderer.exe"
  Delete /REBOOTOK "$INSTDIR\Weasel-Next-Setting.exe"
  Delete /REBOOTOK "$INSTDIR\shuangpin-schema.exe"
  Delete /REBOOTOK "$INSTDIR\plum-next.exe"
  Delete /REBOOTOK "$INSTDIR\rime.dll"
  Delete /REBOOTOK "$INSTDIR\rime-next.dll"
  ; weasel_tip.dll 是旧名字，升级时退休掉（新名字是 Weasel-Next.dll）
  Delete /REBOOTOK "$INSTDIR\weasel_tip.dll"
  ; 两份 TIP 本体。原来这两行都缺（只注销注册表、文件留在 $INSTDIR），
  ; 卸载后目录里会剩一个已注销的 Weasel-Next.dll。
  Delete /REBOOTOK "$INSTDIR\Weasel-Next.dll"
  Delete /REBOOTOK "$INSTDIR\Weasel-Next-32.dll"
  Delete /REBOOTOK "$INSTDIR\styles-LICENSE.txt"
  Delete /REBOOTOK "$INSTDIR\style-LICENSE.txt"
  Delete /REBOOTOK "$INSTDIR\LICENSE"
  Delete /REBOOTOK "$INSTDIR\weasel.json"
  ; weasel.yaml 是现在的正式宿主配置；weasel.toml 是过渡期旧格式，卸载时一并清掉。
  Delete /REBOOTOK "$INSTDIR\weasel.yaml"
  Delete /REBOOTOK "$INSTDIR\weasel.toml"
  Delete /REBOOTOK "$INSTDIR\THIRD-PARTY-LICENSES.txt"
  Delete /REBOOTOK "$INSTDIR\THIRD-PARTY-LICENSE.txt"
  Delete /REBOOTOK "$INSTDIR\THIRD-PARTY-GPL-3.0.txt"
  Delete /REBOOTOK "$INSTDIR\SETTINGS-LICENSE.txt"
  Delete /REBOOTOK "$INSTDIR\SETTING-LICENSE.txt"
  Delete /REBOOTOK "$INSTDIR\Uninstall.exe"
  Delete /REBOOTOK "$INSTDIR\installer-runtime.ps1"
  ; 用户文件夹指针（安装时自选的位置）。用户数据本身**不删**。
  Delete /REBOOTOK "$INSTDIR\userdir.txt"
  Delete "$INSTDIR\files.lst"
  Delete "$INSTDIR\files.pending.lst"
  Delete /REBOOTOK "$INSTDIR\weasel-installer-helper.exe"
  RMDir /REBOOTOK "$INSTDIR\theme-wasm"
  RMDir /REBOOTOK "$INSTDIR"
!macroend

Section "Weasel-Next" SEC_MAIN
  SectionIn RO
  ; 用户可能在目录页里改了 $INSTDIR —— 「覆盖修复 / 并存 / 非空目录」都要对着最终
  ; 目录判断，且必须在任何写入之前（Abort 就什么都不做）。
  Call CheckTargetDirectory
  Call BeginInstallLog
  ; 详情面板第一行就写明目标目录，安装日志里也留一份（排查「装到哪了」不用猜）。
  DetailPrint "安装到：$INSTDIR"
  !insertmacro InstallLog "INFO: 安装目录：$INSTDIR"
  InitPluginsDir
  Call BeginRecords
  SetOverwrite on
  SetOutPath "$INSTDIR"
  ClearErrors
  !insertmacro ManagedFile "${RELEASE_DIR}\Weasel-Next-Server.exe" "Weasel-Next-Server.exe"
  !insertmacro ManagedFile "${RELEASE_DIR}\Weasel-Next-Engine.exe" "Weasel-Next-Engine.exe"
  !insertmacro ManagedFile "${RELEASE_DIR}\Weasel-Next-Renderer.exe" "Weasel-Next-Renderer.exe"
  ; 自定义双拼解析器：把 parser\shuangpin\*.parser.yaml 转成 rime 认的 .schema.yaml。
  ; 随包分发，用户机器上不需要 Python / Rust。
  !insertmacro ManagedFile "${RELEASE_DIR}\shuangpin-schema.exe" "shuangpin-schema.exe"
  ; 配方管理器 PlumNext：设置界面「获取更多输入方案...」拉起它。
  !insertmacro ManagedFile "${RELEASE_DIR}\plum-next.exe" "plum-next.exe"
  !insertmacro ManagedFile "${PROJECT_ROOT}\Engine\src\style-LICENSE.txt" "style-LICENSE.txt"
  !insertmacro ManagedFile "${PROJECT_ROOT}\LICENSE" "LICENSE"
  ; 宿主配置的唯一格式是 YAML：style::parse_file 只认 YAML，配置树与序列化也统一成 YAML。
  ; TOML/JSON 既不读也不写，旧文件不再分发。
  ; 2026-11-08 源文件改名 setting.yaml；安装目录里仍落地为 weasel.yaml（运行时文件名不变）。
  !insertmacro ManagedFile "${PROJECT_ROOT}\Rime-Next\setting.yaml" "weasel.yaml"
  Delete /REBOOTOK "$INSTDIR\weasel.json"
  ; 过渡期：删掉安装目录里旧的 weasel.toml（新版本完全不读它）。只有确认 yaml 真的
  ; 装上才删，避免把机器上唯一的宿主配置删掉。
  ${If} ${FileExists} "$INSTDIR\weasel.yaml"
    Delete /REBOOTOK "$INSTDIR\weasel.toml"
  ${EndIf}
  !insertmacro ManagedFile "${PROJECT_ROOT}\THIRD-PARTY-LICENSE.txt" "THIRD-PARTY-LICENSE.txt"
  !insertmacro ManagedFile "${PROJECT_ROOT}\THIRD-PARTY-GPL-3.0.txt" "THIRD-PARTY-GPL-3.0.txt"
  !insertmacro ManagedFile "${RELEASE_DIR}\Weasel-Next.dll" "Weasel-Next.dll"
  ; 32 位 TIP：32 位宿主（WPS、部分旧软件）只会去 WOW6432Node 找 InprocServer32。
  ; 和 64 位那份并排放在 $INSTDIR 下，注册表两个视图各指一份。
  !insertmacro ManagedFile "${RELEASE_DIR}\Weasel-Next-32.dll" "Weasel-Next-32.dll"
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONSTOP "无法安装 Weasel-Next 文件，请检查磁盘空间和文件权限。" /SD IDOK
    SetErrorLevel 1
    Abort
  ${EndIf}
SectionEnd

Section "librime" SEC_LIBRIME
  SectionIn RO
  ClearErrors
  SetOutPath "$INSTDIR"
!ifdef TARGET_ARM64
  ; ARM64 包：装 arm64 的 rime-next.dll（Engine 优先加载这个名字），旧 rime.dll 槽位不放。
  ; 运行库用 llvm-mingw 的 arm64 版；libglog/libopencc 是 x64 C++ 引擎的依赖，ARM 包不需要。
  !insertmacro ManagedFile "${RELEASE_DIR}\rime-next.dll" "rime-next.dll"
  !insertmacro ManagedFile "${RELEASE_DIR}\libc++.dll" "libc++.dll"
  !insertmacro ManagedFile "${RELEASE_DIR}\libunwind.dll" "libunwind.dll"
  !insertmacro ManagedFile "${RELEASE_DIR}\libwinpthread-1.dll" "libwinpthread-1.dll"
!else
  ; 新引擎槽位：Engine 优先加载 rime-next.dll；旧 rime.dll 留着做兜底/兼容。
  !insertmacro ManagedFile "${RELEASE_DIR}\rime-next.dll" "rime-next.dll"
  !insertmacro ManagedFile "${PROJECT_ROOT}\artifact\smoke\rime.dll" "rime.dll"
  !insertmacro ManagedFile "${PROJECT_ROOT}\runtime\libc++.dll" "libc++.dll"
  !insertmacro ManagedFile "${PROJECT_ROOT}\runtime\libunwind.dll" "libunwind.dll"
  !insertmacro ManagedFile "${PROJECT_ROOT}\runtime\libwinpthread-1.dll" "libwinpthread-1.dll"
  !insertmacro ManagedFile "${PROJECT_ROOT}\runtime\libgcc_s_seh-1.dll" "libgcc_s_seh-1.dll"
  !insertmacro ManagedFile "${PROJECT_ROOT}\runtime\libglog.dll" "libglog.dll"
  !insertmacro ManagedFile "${PROJECT_ROOT}\runtime\libopencc.dll" "libopencc.dll"
!endif
  !insertmacro RimePayload
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONSTOP "无法安装 librime，请检查磁盘空间和文件权限。" /SD IDOK
    SetErrorLevel 1
    Abort
  ${EndIf}
SectionEnd

Section "设置应用" SEC_SETTINGS
  ClearErrors
  SetOutPath "$INSTDIR"
  !insertmacro ManagedFile "${RELEASE_DIR}\Weasel-Next-Setting.exe" "Weasel-Next-Setting.exe"
  !insertmacro ManagedFile "${PROJECT_ROOT}\Setting\LICENSE-NOTICE.txt" "SETTING-LICENSE.txt"
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONSTOP "无法安装设置应用，请检查磁盘空间和文件权限。" /SD IDOK
    SetErrorLevel 1
    Abort
  ${EndIf}
SectionEnd

SectionGroup /e "候选主题" SEC_THEMES
  Section "default（默认皮肤）" SEC_THEME_OFFICIAL
    SetOutPath "$INSTDIR"
    !insertmacro ManagedFile "${RELEASE_DIR}\weasel-next-theme.dll" "weasel-next-theme.dll"
  SectionEnd
SectionGroupEnd

!ifndef MINI_INSTALLER
Section /o "调试符号" SEC_SYMBOLS
  ClearErrors
  SetOutPath "$INSTDIR"
  ${If} $UseUiAccess == 1
  ${Else}
  ${EndIf}
  ${If} ${SectionIsSelected} ${SEC_SETTINGS}
  ${EndIf}
!ifdef HAVE_WASM_THEME
!endif
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONSTOP "无法安装调试符号，请检查磁盘空间和文件权限。" /SD IDOK
    SetErrorLevel 1
    Abort
  ${EndIf}
SectionEnd

!endif
; Commit only after every selected component has been staged successfully.
Section "-注册与安装信息" SEC_REGISTER
  SetOutPath "$StageRoot"
  ClearErrors
  WriteUninstaller "$StageRoot\Uninstall.exe"
  ${If} ${Errors}
    Goto install_failed
  ${EndIf}
  FileWriteUTF16LE $ManifestHandle "Uninstall.exe$\r$\n"
  IfErrors install_failed
  Call CloseRecords

  ; Prerequisite failure must leave the existing application untouched.
  Call StopApplicationProcesses
  Call RemoveOldPayload
  Delete "$INSTDIR\files.lst"
  Delete "$INSTDIR\files.pending.lst"
  ; Persist the new payload list before copying; a failure leaves files in
  ; place rather than attempting to restore or unregister the old version.
  ClearErrors
  CopyFiles /SILENT "$StageRoot\files.pending.lst" "$INSTDIR"
  IfErrors install_failed
  !insertmacro InstallLog "INFO: 开始提交新文件"
  Call CommitStagedPayload

  ; 用户 2026-10-29：用户文件夹可选。**只在用户改了默认位置时**写 `userdir.txt`，
  ; 选默认（%APPDATA%\Rime-Next）就不写 —— 老机器没有这个文件，行为完全不变。
  ; 组件侧解析顺序：WEASEL_NEXT_USER_DIR > userdir.txt > %APPDATA%\Rime-Next
  ; （`weasel_common::process::user_data_dir`）。
  ${If} $UserDir == "$UserDirDefault"
    Delete "$INSTDIR\userdir.txt"
    !insertmacro InstallLog "INFO: 用户文件夹（默认）：$UserDir"
  ${Else}
    ClearErrors
    FileOpen $0 "$INSTDIR\userdir.txt" w
    ${IfNot} ${Errors}
      FileWrite $0 "$UserDir"
      FileClose $0
      !insertmacro InstallLog "INFO: 用户文件夹：$UserDir"
    ${Else}
      !insertmacro InstallLog "WARN: 写 userdir.txt 失败，用户文件夹回退到 $UserDirDefault"
      StrCpy $UserDir "$UserDirDefault"
    ${EndIf}
  ${EndIf}
  CreateDirectory "$UserDir"

  !insertmacro InstallLog "INFO: 开始注册 TIP"
  ; 两份 TIP 并排放在 $INSTDIR 下（没有架构子目录）：
  ;   Weasel-Next.dll     64 位，给 64 位宿主 / 64 位注册表视图
  ;   Weasel-Next-32.dll  32 位，给 32 位宿主（WPS 等）/ WOW6432Node
  ;
  ; ⚠️ makensis 产出的是 **32 位** 安装器，它自己会被 WOW64 文件系统重定向，
  ;    所以 "$SYSDIR\regsvr32.exe" 实际是 SysWOW64 里那个 32 位 regsvr32
  ;    （本机实测：NSIS 里 ExecWait "$SYSDIR\cmd.exe" 报 x86）。
  ;    用它去注册 64 位 DLL 必然失败（BadImageFormat）⇒ 64 位那份必须先关掉重定向；
  ;    32 位那份直接写 SysWOW64（该目录本身不被重定向，不会有歧义）。
  SetOutPath "$INSTDIR"
  ClearErrors
  ${DisableX64FSRedirection}
  ExecWait '"$SYSDIR\regsvr32.exe" /s "$INSTDIR\Weasel-Next.dll"' $0
  ${EnableX64FSRedirection}
!ifdef TARGET_ARM64
  ; ARM64：上面那条走「关掉重定向后的 System32」= **arm64** 的 regsvr32，能加载 arm64 TIP。
  ; 但 Wow64 文件系统重定向 API 在 ARM64 上不是每个版本都受支持（无法在 x64 机器上验证），
  ; 所以失败时再用 Sysnative 别名重试一次 —— 该别名只对 32 位进程可见，恰好就是本安装器。
  ${If} $0 != 0
    !insertmacro InstallLog "WARN: $SYSDIR\regsvr32 未能注册 arm64 TIP（$0），改用 Sysnative 重试"
    ClearErrors
    ExecWait '"$WINDIR\Sysnative\regsvr32.exe" /s "$INSTDIR\Weasel-Next.dll"' $0
    ${If} ${Errors}
      !insertmacro InstallLog "WARN: 无法启动 Sysnative\regsvr32.exe"
    ${EndIf}
  ${EndIf}
!endif
  ${If} ${Errors}
    !insertmacro InstallLog "ERROR: 无法启动 TIP 注册程序"
    Goto install_failed
  ${EndIf}
  !insertmacro InstallLog "INFO: 64 位 TIP 注册退出码=$0"
  ${If} $0 != 0
    !insertmacro InstallLog "ERROR: 64 位 regsvr32 返回 $0，TIP 未能注册"
    Goto install_failed
  ${EndIf}

  ; 32 位 TIP 注册失败**不**中断安装：它是本次新增的能力，缺了它 64 位用户照常能用。
  ${If} ${FileExists} "$INSTDIR\Weasel-Next-32.dll"
    ClearErrors
    ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s "$INSTDIR\Weasel-Next-32.dll"' $Tip32Exit
    ${If} ${Errors}
      !insertmacro InstallLog "WARN: 无法启动 32 位 TIP 注册程序"
    ${ElseIf} $Tip32Exit != 0
      !insertmacro InstallLog "WARN: 32 位 regsvr32 返回 $Tip32Exit，32 位程序（WPS 等）可能没有输入法"
    ${Else}
      !insertmacro InstallLog "INFO: 32 位 TIP 注册成功"
    ${EndIf}
  ${EndIf}

  ClearErrors
  ; 产品级登记只指向“当前版本”目录；旧版本目录保留但不占这里。
  WriteRegStr HKLM "${PRODUCT_KEY}" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "${PRODUCT_KEY}" "CurrentVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\Weasel-Next-Server.exe,0"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoRepair" 1
  IfErrors install_failed
  Call FinishRecords
  StrCpy $TransactionCommitted 1

  CreateDirectory "$SMPROGRAMS\小狼毫Next"
  Delete "$SMPROGRAMS\小狼毫Next\小狼毫Next.lnk"
  ; 设置入口：官方那几个对话框各一个快捷方式，命名照官方（【小狼毫】→【小狼毫Next】）。
  ; 不带参数会打开旧的 Slint 界面，所以每个都必须带开关。
  CreateShortcut "$SMPROGRAMS\小狼毫Next\【小狼毫Next】界面风格设定.lnk" "$INSTDIR\Weasel-Next-Setting.exe" "/style"
  CreateShortcut "$SMPROGRAMS\小狼毫Next\【小狼毫Next】方案选单设定.lnk" "$INSTDIR\Weasel-Next-Setting.exe" "/switcher"
  CreateShortcut "$SMPROGRAMS\小狼毫Next\【小狼毫Next】用户词典管理.lnk" "$INSTDIR\Weasel-Next-Setting.exe" "/dict"
  CreateShortcut "$SMPROGRAMS\小狼毫Next\【小狼毫Next】用户文件夹.lnk" "$UserDir"
  CreateShortcut "$SMPROGRAMS\小狼毫Next\【小狼毫Next】程序文件夹.lnk" "$INSTDIR"
  CreateShortcut "$SMPROGRAMS\小狼毫Next\卸载.lnk" "$INSTDIR\Uninstall.exe"
  ${If} ${Errors}
    !insertmacro InstallLog "WARN: 无法创建卸载快捷方式"
  ${EndIf}
  nsExec::ExecToLog '"$INSTDIR\Weasel-Next-Server.exe" --install-shortcut "$SMPROGRAMS\小狼毫Next\小狼毫Next算法服务.lnk"'
  Pop $0
  ${If} $0 != 0
    !insertmacro InstallLog "WARN: 创建算法服务快捷方式失败：$0"
  ${EndIf}
  Call DeleteRetiredDlls
  Call CleanupOldDllBackups
  !insertmacro InstallLog "INFO: 正在初始化当前桌面用户……"
  Call ProvisionCurrentUser
  !insertmacro InstallLog "INFO: 正在启动算法服务……"
  Call LaunchBrokerUnelevated
  Call FinishInstallLog
  Goto install_done
  install_failed:
    MessageBox MB_OK|MB_ICONSTOP "安装失败：TIP 注册未完成或文件写入失败。$\r$\n$\r$\n请确认以管理员身份运行，并检查 $INSTDIR 的写入权限；详情见安装日志（%TEMP%\Weasel-Next-install.log）。" /SD IDOK
    SetErrorLevel 1
    Abort
  install_done:
SectionEnd


; 安装器没有组件选择页（见文件头的 MUI 页面列表）⇒ 这里把全部 section 勾上。
; SEC_REGISTER 是最后一个安装 section。逐个置 SF_SELECTED，保留只读等原有标志。
Function SelectAllComponents
  Push $0
  Push $1
  ${For} $0 0 ${SEC_REGISTER}
    SectionGetFlags $0 $1
    IntOp $1 $1 | ${SF_SELECTED}
    SectionSetFlags $0 $1
  ${Next}
  Pop $1
  Pop $0
FunctionEnd

Function .onInstFailed
  ${If} $TransactionCommitted != 1
    ${If} ${FileExists} "$PLUGINSDIR\new-uiaccess.cer"
    ${EndIf}
  ${EndIf}
  !insertmacro InstallLog "ERROR: 安装未完成；保留当前文件，不执行回滚；日志：$InstallLog"
  Call CloseRecords
  ; Do not delete payload, undo registrations, or erase installation records.
  ; Keep retired DLLs on failure for the next successful installation to clean.
  ${If} $InstallLogHandle != ""
    FileClose $InstallLogHandle
    StrCpy $InstallLogHandle ""
  ${EndIf}
  SetErrorLevel 1
  MessageBox MB_OK|MB_ICONEXCLAMATION "安装未完成，部分文件可能已更新，未执行回滚。请解决错误后重新运行安装程序。$\r$\n安装日志：$InstallLog" /SD IDOK
FunctionEnd

Function un.onInit
  !insertmacro CheckPlatform
  !insertmacro LockInstaller
  SetRegView 64
  StrCpy $ScanExtraDir ""
  ; 卸载器只删自己这个版本目录（构建时已把 $INSTDIR 烘成 ...\Weasel-Next-<版本>）。
  ; 别的版本可能还在用，所以这里不再要求它等于产品级 InstallDir；只校验路径形状。
  StrLen $1 "${PRODUCT_ROOT}\"
  StrCpy $2 "$INSTDIR" $1
  ${If} $INSTDIR != "${PRODUCT_DIR}"
  ${OrIf} $2 != "${PRODUCT_ROOT}\"
    MessageBox MB_OK|MB_ICONSTOP "安装目录校验失败，拒绝注销或删除文件。" /SD IDOK
    SetErrorLevel 1
    Quit
  ${EndIf}
  !insertmacro AssertSafeUninstallDirectory "$INSTDIR"
  !insertmacro AssertSafeUninstallDirectory "$INSTDIR\theme"
  !insertmacro ValidateRimePayloadRemoval
  !insertmacro ValidateWasmPayloadRemoval
FunctionEnd

; Run only after the user confirms uninstallation. Do not block the confirm
; page just because the broker is still running.
Function un.StopApplicationProcesses
  ; Settings has no broker-managed graceful shutdown path, so the shared scan
  ; force-stops its path-verified process before handling the service family.
  StrCpy $ScanPassLimit 4
  StrCpy $ScanMode 0
  Call un.ScanApplicationProcesses
  StrCmp $ScanResult 0 uninstall_processes_stopped

  ; The installed broker understands --shutdown and asks its children to exit.
  ; A missing or damaged broker falls through to the path-checked force stop.
  IfFileExists "$INSTDIR\Weasel-Next-Server.exe" 0 uninstall_force_stop
  DetailPrint "正在请求算法服务正常退出……"
  nsExec::ExecToLog /TIMEOUT=40000 '"$INSTDIR\Weasel-Next-Server.exe" --shutdown "$INSTDIR"'
  Pop $0
  DetailPrint "退出命令返回：$0"
  StrCpy $ScanMode 0
  Call un.ScanApplicationProcesses
  StrCmp $ScanResult 0 uninstall_processes_stopped

  uninstall_force_stop:
    DetailPrint "正在结束未退出的小狼毫Next服务进程……"
    StrCpy $ScanMode 1
    Call un.ScanApplicationProcesses
    StrCpy $ScanMode 0
    Call un.ScanApplicationProcesses
    StrCmp $ScanResult 0 uninstall_processes_stopped
    ; Continue uninstalling and let /REBOOTOK retire files still in use.
    DetailPrint "警告：部分服务进程未能结束；剩余文件将在重启后移除。"
    SetRebootFlag true
  uninstall_processes_stopped:
FunctionEnd

Section "Uninstall"
  SetOutPath "$TEMP"
  Call un.StopApplicationProcesses
  ClearErrors
  ; 版本化安装后可能同时存在多个版本目录：卸载**旧版本**绝不能动当前版本的
  ; TIP 注册、快捷方式与产品登记。只有登记里指向本目录时才算“当前版本”。
  SetRegView 64
  ReadRegStr $3 HKLM "${PRODUCT_KEY}" "InstallDir"
  ; 先看 CLSID 实际指向谁，确认这次注销不会误删别的版本。
  ReadRegStr $4 HKCR "CLSID\{16A7AEA9-9EE6-4540-8020-E384C0489BB1}\InprocServer32" ""
  ${If} $4 == "$INSTDIR\Weasel-Next.dll"
    ; 两份 TIP 都要注销。同注册段：安装器/卸载器是 32 位进程，$SYSDIR 会被重定向到
    ; SysWOW64，所以 64 位那份要先关掉重定向，32 位那份直接走 SysWOW64。
    ${DisableX64FSRedirection}
    ExecWait '"$SYSDIR\regsvr32.exe" /s /u "$INSTDIR\Weasel-Next.dll"' $0
    ${EnableX64FSRedirection}
    ${If} ${Errors}
      DetailPrint "提示：无法启动 TIP 注销程序，将继续卸载。"
    ${ElseIf} $0 != 0
      ; DLL 被输入法框架占用时注销会失败；不阻断卸载，文件删除与重启后清理照常进行。
      DetailPrint "提示：64 位 TIP 注销返回 $0，将继续卸载。"
    ${EndIf}
  ${Else}
    DetailPrint "提示：64 位 TIP 已指向别的版本（$4），跳过注销。"
  ${EndIf}
  ${If} ${FileExists} "$INSTDIR\Weasel-Next-32.dll"
    SetRegView 32
    ReadRegStr $5 HKCR "CLSID\{16A7AEA9-9EE6-4540-8020-E384C0489BB1}\InprocServer32" ""
    SetRegView 64
    ${If} $5 == "$INSTDIR\Weasel-Next-32.dll"
      ClearErrors
      ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s /u "$INSTDIR\Weasel-Next-32.dll"' $Tip32Exit
      ${If} ${Errors}
        DetailPrint "提示：无法启动 32 位 TIP 注销程序，将继续卸载。"
      ${ElseIf} $Tip32Exit != 0
        DetailPrint "提示：32 位 TIP 注销返回 $Tip32Exit，将继续卸载。"
      ${EndIf}
    ${Else}
      DetailPrint "提示：32 位 TIP 已指向别的版本（$5），跳过注销。"
    ${EndIf}
  ${EndIf}
  ${If} $3 == "$INSTDIR"
    ; 只有当前版本才删快捷方式与产品登记；旧版本目录里的卸载器只清自己的文件。
    Delete "$SMPROGRAMS\小狼毫Next\小狼毫Next.lnk"
    Delete "$SMPROGRAMS\小狼毫Next\小狼毫Next算法服务.lnk"
    Delete "$SMPROGRAMS\小狼毫Next\【小狼毫Next】界面风格设定.lnk"
    Delete "$SMPROGRAMS\小狼毫Next\【小狼毫Next】方案选单设定.lnk"
    Delete "$SMPROGRAMS\小狼毫Next\【小狼毫Next】用户词典管理.lnk"
    Delete "$SMPROGRAMS\小狼毫Next\【小狼毫Next】用户文件夹.lnk"
    Delete "$SMPROGRAMS\小狼毫Next\【小狼毫Next】程序文件夹.lnk"
    Delete "$SMPROGRAMS\小狼毫Next\卸载.lnk"
    RMDir "$SMPROGRAMS\小狼毫Next"
  ${EndIf}
  ${If} $3 == "$INSTDIR"
    ; 当前版本：清掉指向本目录的自启动项（broker 自己会做路径比对，别的版本的值不会被删）。
    Call un.RemoveCurrentUserProvisioning
  ${EndIf}
  !insertmacro RemoveProgramFiles
  ${If} $3 == "$INSTDIR"
    DeleteRegKey HKLM "${UNINSTALL_KEY}"
    DeleteRegKey HKLM "${PRODUCT_KEY}"
  ${EndIf}
  Goto uninstall_done
  uninstall_failed:
    MessageBox MB_OK|MB_ICONSTOP "TIP 注销失败，已停止删除文件。请关闭使用输入法的应用后重试。" /SD IDOK
    SetErrorLevel 1
    Abort
  uninstall_done:
SectionEnd
