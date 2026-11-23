Unicode true
RequestExecutionLevel user
ManifestDPIAware true
SetCompressor /SOLID zlib
!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"
!include "WinVer.nsh"
!include "security.nsh"

!define PRODUCT_KEY "Software\KobaProduction\Blazer"
!define PRODUCT_ID "KobaProduction.Blazer.Windows"
Name "Blazer ${APP_VERSION}"
OutFile "${SETUP_OUTPUT}"
InstallDir "$PROGRAMFILES64\Blazer"
Icon "${APP_ICON}"
UninstallIcon "${APP_ICON}"
VIProductVersion "${APP_VERSION}.0.0"
VIAddVersionKey /LANG=0 "ProductName" "Blazer"
VIAddVersionKey /LANG=0 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=0 "FileVersion" "${APP_VERSION}.0.0"
VIAddVersionKey /LANG=0 "CompanyName" "KobaProduction"
VIAddVersionKey /LANG=0 "FileDescription" "Blazer installer"
VIAddVersionKey /LANG=0 "LegalCopyright" "KobaProduction"

Var Elevated
Var OperatorSid
Var BootstrapPid
Var Transaction
Var Arguments
Var StageRoot
Var StageDirectory
Var PowerShell

!define MUI_ABORTWARNING
!define MUI_CUSTOMFUNCTION_ABORT OnSetupAbort
!define MUI_CUSTOMFUNCTION_UNABORT un.OnSetupAbort
!define MUI_FINISHPAGE_RUN "$INSTDIR\blazer.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Blazer"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${APP_LICENSE}"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Russian"
LangString Failed ${LANG_ENGLISH} "Blazer maintenance failed (code 2). The previous installation was restored when possible. Retry this installer to recover an interrupted transaction. No unrelated service is removed."
LangString Failed ${LANG_RUSSIAN} "Обслуживание Blazer не выполнено (код 2). Предыдущее состояние восстановлено, если это возможно. Повторите запуск установщика для восстановления прерванной операции. Посторонние службы не удаляются."
LangString Cancelled ${LANG_ENGLISH} "Administrator permission was cancelled. Blazer was not closed."
LangString Cancelled ${LANG_RUSSIAN} "Разрешение администратора отменено. Blazer не закрывался."
LangString WrongPlatform ${LANG_ENGLISH} "Blazer requires 64-bit Windows 10 or newer."
LangString WrongPlatform ${LANG_RUSSIAN} "Для Blazer требуется 64-разрядная Windows 10 или новее."
LangString StartNormally ${LANG_ENGLISH} "Start this installer normally, without 'Run as administrator'. It will request administrator permission after confirmation."
LangString StartNormally ${LANG_RUSSIAN} "Запустите установщик обычным способом, без «Запуск от имени администратора». Он запросит права после подтверждения."

!macro InitBootstrap Prefix
Function ${Prefix}Initialize
!ifdef DEVELOPMENT_BUILD
  ${GetParameters} $Arguments
  ClearErrors
  ${GetOptions} $Arguments "/EXTRACTCHECK" $0
  ${IfNot} ${Errors}
    System::Call 'shell32::IsUserAnAdmin()i.r0'
    ${If} $0 != 0
      SetErrorLevel 2
      Quit
    ${EndIf}
    System::Call 'kernel32::GetFileAttributesW(w "$TEMP")i.r0'
    IntOp $0 $0 & 0x400
    ${If} $0 != 0
      SetErrorLevel 2
      Quit
    ${EndIf}
    System::Call 'kernel32::GetCurrentProcessId()i.r0'
    StrCpy $StageDirectory "$TEMP\Blazer.InstallerInspect.$0"
    System::Call 'kernel32::GetFileAttributesW(w "$StageDirectory")i.r0'
    ${If} $0 != -1
      SetErrorLevel 2
      Quit
    ${EndIf}
    ClearErrors
    CreateDirectory "$StageDirectory"
    ${If} ${Errors}
      SetErrorLevel 2
      Quit
    ${EndIf}
    System::Call 'kernel32::GetFileAttributesW(w "$StageDirectory")i.r0'
    IntOp $0 $0 & 0x400
    ${If} $0 != 0
      SetErrorLevel 2
      Quit
    ${EndIf}
    SetOutPath "$StageDirectory"
    File /r "${PAYLOAD_DIRECTORY}\*.*"
    SetErrorLevel 0
    Quit
  ${EndIf}
  MessageBox MB_OK|MB_ICONINFORMATION "Development compile-check only. This artifact cannot install, elevate or change Windows services." /SD IDOK
  SetErrorLevel 2
  Quit
!endif
  SetRegView 64
  StrCpy $INSTDIR "$PROGRAMFILES64\Blazer"
  StrCpy $StageRoot "$PROGRAMFILES64\Blazer.Setup.Stage"
  StrCpy $PowerShell "$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe"
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "$(WrongPlatform)" /SD IDOK
    SetErrorLevel 2
    Quit
  ${EndIf}
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_OK|MB_ICONSTOP "$(WrongPlatform)" /SD IDOK
    SetErrorLevel 2
    Quit
  ${EndIf}
  ${GetParameters} $Arguments
  ClearErrors
  ${GetOptions} $Arguments "/ELEVATED=" $Elevated
  ${If} $Elevated == "1"
    System::Call 'shell32::IsUserAnAdmin()i.r0'
    ${If} $0 == 0
      SetErrorLevel 2
      Quit
    ${EndIf}
    ${GetOptions} $Arguments "/OPERATORSID=" $OperatorSid
    ${GetOptions} $Arguments "/BOOTSTRAPPID=" $BootstrapPid
    ${GetOptions} $Arguments "/TRANSACTION=" $Transaction
    StrLen $0 $Transaction
    ${If} $0 != 32
      SetErrorLevel 2
      Quit
    ${EndIf}
    ; Reject argument metacharacters before constructing any external command.
    StrCpy $0 0
    ${DoWhile} $0 < 32
      StrCpy $1 $Transaction 1 $0
      StrCpy $2 0
      validate_hex_${Prefix}:
        StrCpy $3 "0123456789abcdef" 1 $2
        StrCmp $1 $3 valid_hex_${Prefix}
        IntOp $2 $2 + 1
        IntCmp $2 16 invalid_${Prefix} validate_hex_${Prefix} invalid_${Prefix}
      valid_hex_${Prefix}:
      IntOp $0 $0 + 1
    ${Loop}
    StrLen $4 $OperatorSid
    ${If} $4 > 184
      Goto invalid_${Prefix}
    ${EndIf}
    StrCpy $0 0
    ${DoWhile} $0 < $4
      StrCpy $1 $OperatorSid 1 $0
      StrCpy $2 0
      validate_sid_${Prefix}:
        StrCpy $3 "S-0123456789" 1 $2
        StrCmp $1 $3 valid_sid_${Prefix}
        IntOp $2 $2 + 1
        IntCmp $2 12 invalid_${Prefix} validate_sid_${Prefix} invalid_${Prefix}
      valid_sid_${Prefix}:
      IntOp $0 $0 + 1
    ${Loop}
    StrLen $4 $BootstrapPid
    ${If} $4 > 10
      Goto invalid_${Prefix}
    ${EndIf}
    StrCpy $0 0
    ${DoWhile} $0 < $4
      StrCpy $1 $BootstrapPid 1 $0
      StrCpy $2 0
      validate_pid_${Prefix}:
        StrCpy $3 "0123456789" 1 $2
        StrCmp $1 $3 valid_pid_${Prefix}
        IntOp $2 $2 + 1
        IntCmp $2 10 invalid_${Prefix} validate_pid_${Prefix} invalid_${Prefix}
      valid_pid_${Prefix}:
      IntOp $0 $0 + 1
    ${Loop}
    SetSilent silent
    Return
    invalid_${Prefix}:
      SetErrorLevel 2
      Quit
  ${EndIf}
  StrCpy $Elevated "0"
  System::Call 'shell32::IsUserAnAdmin()i.r0'
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "$(StartNormally)" /SD IDOK
    SetErrorLevel 2
    Quit
  ${EndIf}
  System::Call 'kernel32::GetCurrentProcessId()i.r0'
  StrCpy $BootstrapPid $0
  nsExec::ExecToStack '"$PowerShell" -NoLogo -NoProfile -NonInteractive -Command "[Console]::Write([Security.Principal.WindowsIdentity]::GetCurrent().User.Value)"'
  Pop $0
  Pop $OperatorSid
  ${If} $0 != "0"
    SetErrorLevel 2
    Quit
  ${EndIf}
  nsExec::ExecToStack '"$PowerShell" -NoLogo -NoProfile -NonInteractive -Command "[Console]::Write([Guid]::NewGuid().ToString().Replace([string][char]45,[string]::Empty))"'
  Pop $0
  Pop $Transaction
  ${If} $0 != "0"
    SetErrorLevel 2
    Quit
  ${EndIf}
FunctionEnd
!macroend
!insertmacro InitBootstrap ""
!insertmacro InitBootstrap "un."

Function .onInit
  Call Initialize
FunctionEnd
Function un.onInit
  Call un.Initialize
FunctionEnd
Function OnSetupAbort
  SetErrorLevel 1602
FunctionEnd
Function un.OnSetupAbort
  SetErrorLevel 1602
FunctionEnd

!macro RelaunchFunction Prefix
Function ${Prefix}RelaunchRestored
  ${If} $Elevated == "1"
    Return
  ${EndIf}
  ReadRegStr $0 HKLM "${PRODUCT_KEY}" "LastRollbackTransaction"
  StrCmp $0 $Transaction 0 relaunch_done_${Prefix}
  ReadRegStr $0 HKLM "${PRODUCT_KEY}" "LastClosedTransaction"
  StrCmp $0 $Transaction 0 relaunch_done_${Prefix}
  ReadRegStr $0 HKLM "${PRODUCT_KEY}" "ProductId"
  StrCmp $0 "${PRODUCT_ID}" 0 relaunch_done_${Prefix}
  nsExec::ExecToStack `"$PowerShell" -NoLogo -NoProfile -NonInteractive -Command "$$env:PSModulePath=$$PSHOME+'\Modules';$$p=Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Blazer\blazer.exe';$$open=@(Get-Process blazer -ErrorAction SilentlyContinue|Where-Object {$$_.Path-ieq $$p-and $$_.MainWindowHandle-ne 0});if(-not $$open.Count-and(Test-Path -LiteralPath $$p)){Start-Process -FilePath $$p}"`
  Pop $0
  Pop $1
  relaunch_done_${Prefix}:
FunctionEnd
!macroend
!insertmacro RelaunchFunction ""
!insertmacro RelaunchFunction "un."

!macro CleanupStage
  SetOutPath "$TEMP"
  RMDir /r "$StageDirectory"
  nsExec::ExecToStack `"$PowerShell" -NoLogo -NoProfile -NonInteractive -Command "$$env:PSModulePath=$$PSHOME+'\Modules';$$p=Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Blazer.Setup.Stage';$$items=@(Get-ChildItem -LiteralPath $$p -Force);if($$items.Count-eq 1-and $$items[0].Name-ceq 'owner.txt'){[IO.File]::Delete((Join-Path $$p 'owner.txt'));[IO.Directory]::Delete($$p)}"`
  Pop $1
  Pop $2
!macroend

!macro ExtractMaintenance Suffix
  ; Program Files is privileged before any helper script or payload is extracted.
  !insertmacro ValidateStagingRoot ${Suffix}
  IfFileExists "$StageRoot\." stage_exists_${Suffix} stage_create_${Suffix}
  stage_exists_${Suffix}:
  IfFileExists "$StageRoot\owner.txt" 0 stage_failed_${Suffix}
  FileOpen $0 "$StageRoot\owner.txt" r
  FileRead $0 $1
  FileClose $0
  StrCmp $1 "${PRODUCT_ID}" stage_owned_${Suffix} stage_failed_${Suffix}
  stage_create_${Suffix}:
  CreateDirectory "$StageRoot"
  stage_owned_${Suffix}:
  nsExec::ExecToStack '"$SYSDIR\icacls.exe" "$StageRoot" /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" "*S-1-5-32-545:(OI)(CI)RX"'
  Pop $0
  Pop $1
  StrCmp $0 "0" +2
  Goto stage_failed_${Suffix}
  FileOpen $0 "$StageRoot\owner.txt" w
  FileWrite $0 "${PRODUCT_ID}"
  FileClose $0
  nsExec::ExecToStack '"$SYSDIR\icacls.exe" "$StageRoot\owner.txt" /setowner "*S-1-5-32-544"'
  Pop $0
  Pop $1
  StrCmp $0 "0" +2
  Goto stage_failed_${Suffix}
  StrCpy $StageDirectory "$StageRoot\$Transaction"
  IfFileExists "$StageDirectory\." stage_failed_${Suffix}
  CreateDirectory "$StageDirectory"
  SetOutPath "$StageDirectory\maintenance"
  File "maintenance.ps1"
  File "transaction.ps1"
  File "paths.ps1"
  File "service.ps1"
  File "provider.ps1"
  nsExec::ExecToStack '"$SYSDIR\icacls.exe" "$StageDirectory" /setowner "*S-1-5-32-544" /T /L'
  Pop $0
  Pop $1
  StrCmp $0 "0" +2
  Goto stage_failed_${Suffix}
  Goto stage_ready_${Suffix}
  stage_failed_${Suffix}:
    SetErrorLevel 2
    Quit
  stage_ready_${Suffix}:
!macroend

Section "Blazer"
  ${If} $Elevated == "1"
    !insertmacro ExtractMaintenance install
    SetOutPath "$StageDirectory\payload"
    File /r "${PAYLOAD_DIRECTORY}\*.*"
    WriteUninstaller "$StageDirectory\payload\uninstall.exe"
    nsExec::ExecToStack '"$SYSDIR\icacls.exe" "$StageDirectory" /setowner "*S-1-5-32-544" /T /L'
    Pop $0
    Pop $1
    ${If} $0 != "0"
      SetErrorLevel 2
      Quit
    ${EndIf}
    nsExec::ExecToLog '"$PowerShell" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$StageDirectory\maintenance\maintenance.ps1" -Operation Install -PayloadDirectory "$StageDirectory\payload" -OperatorSid "$OperatorSid" -BootstrapPid $BootstrapPid -SetupPath "$EXEPATH" -Transaction "$Transaction"'
    Pop $0
    !insertmacro CleanupStage
    SetErrorLevel $0
    Quit
  ${EndIf}
  ClearErrors
  ExecShellWait "runas" "$EXEPATH" '/ELEVATED=1 /OPERATORSID=$OperatorSid /BOOTSTRAPPID=$BootstrapPid /TRANSACTION=$Transaction /S' SW_HIDE
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONINFORMATION "$(Cancelled)" /SD IDOK
    SetErrorLevel 1223
    Abort
  ${EndIf}
  ReadRegStr $0 HKLM "${PRODUCT_KEY}" "LastSetupTransaction"
  ${If} $0 != $Transaction
    Call RelaunchRestored
    MessageBox MB_OK|MB_ICONSTOP "$(Failed)" /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
  SetErrorLevel 0
SectionEnd

Section "Uninstall"
  ${If} $Elevated == "1"
    !insertmacro ExtractMaintenance uninstall
    nsExec::ExecToLog '"$PowerShell" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$StageDirectory\maintenance\maintenance.ps1" -Operation Uninstall -OperatorSid "$OperatorSid" -BootstrapPid $BootstrapPid -SetupPath "$EXEPATH" -Transaction "$Transaction"'
    Pop $0
    !insertmacro CleanupStage
    SetErrorLevel $0
    Quit
  ${EndIf}
  ClearErrors
  ExecShellWait "runas" "$EXEPATH" '/ELEVATED=1 /OPERATORSID=$OperatorSid /BOOTSTRAPPID=$BootstrapPid /TRANSACTION=$Transaction /S _?=$INSTDIR' SW_HIDE
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONINFORMATION "$(Cancelled)" /SD IDOK
    SetErrorLevel 1223
    Abort
  ${EndIf}
  ClearErrors
  ReadRegStr $0 HKLM "${PRODUCT_KEY}" "ProductId"
  ${IfNot} ${Errors}
    Call un.RelaunchRestored
    MessageBox MB_OK|MB_ICONSTOP "$(Failed)" /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
  SetErrorLevel 0
SectionEnd
