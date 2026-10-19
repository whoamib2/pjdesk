; PJDesk client installer (NSIS). Built by installer/build.sh, which places
; pjdesk.exe, README.md, LICENSE and QUICKSTART.txt next to this script.
Unicode true

!define APPNAME "PJDesk"
!define COMPANY "PJDesk"
!define EXE "pjdesk.exe"
!ifndef VERSION
  !if /FileExists "../Cargo.toml"
    !searchparse /file "../Cargo.toml" `version = "` VERSION `"`
  !endif
!endif
!ifndef VERSION
  !if /FileExists "../../Cargo.toml"
    !searchparse /file "../../Cargo.toml" `version = "` VERSION `"`
  !endif
!endif
!ifndef VERSION
  !error "Could not read the version from Cargo.toml. Pass -DVERSION=x.y.z"
!endif

Name "${APPNAME}"
OutFile "PJDesk-Client-Setup-${VERSION}.exe"
InstallDir "$PROGRAMFILES64\${APPNAME}"
InstallDirRegKey HKLM "Software\${APPNAME}" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "nsDialogs.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "StrFunc.nsh"
${Using:StrFunc} StrStr
${Using:StrFunc} StrTrimNewLines

!define MUI_ICON "pjdesk.ico"
!define MUI_UNICON "pjdesk.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Open PJDesk now"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
Page custom NamePage NamePageLeave
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Var ComputerName
Var NameField
Var ProgramDataDir

; Don't load DLLs from the folder the installer was launched from.
Function .onInit
  System::Call 'kernel32::SetDllDirectoryA(t "")'
  ReadEnvStr $ProgramDataDir ProgramData
  ${If} $ProgramDataDir == ""
    StrCpy $ProgramDataDir "C:\ProgramData"
  ${EndIf}
  StrCpy $ComputerName ""
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/NAME=" $ComputerName
  IfErrors name_no_switch
  Goto name_init_done
  name_no_switch:
  IfSilent name_silent name_keep
  name_silent:
  StrCpy $ComputerName "__KEEP__"
  Goto name_init_done
  name_keep:
  IfFileExists "$ProgramDataDir\PJDesk\computer-name.txt" 0 name_init_done
  FileOpen $0 "$ProgramDataDir\PJDesk\computer-name.txt" r
  IfErrors name_init_done
  FileRead $0 $ComputerName
  FileClose $0
  ${StrTrimNewLines} $ComputerName $ComputerName
  name_init_done:
FunctionEnd

Function NamePage
  StrCmp $ComputerName "__KEEP__" 0 name_page_show
  Abort
  name_page_show:
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 24u "Name this computer. People signed in to your account see this name in Saved computers. Leave it blank to use the Windows name."
  Pop $0
  ${NSD_CreateText} 0 28u 100% 12u $ComputerName
  Pop $NameField
  ReadEnvStr $1 COMPUTERNAME
  ${NSD_CreateLabel} 0 48u 100% 12u "Computer name: $1"
  Pop $0
  nsDialogs::Show
FunctionEnd

Function NamePageLeave
  ${NSD_GetText} $NameField $ComputerName
FunctionEnd

Function un.onInit
  System::Call 'kernel32::SetDllDirectoryA(t "")'
FunctionEnd

; If the install stops after the service was stopped, bring the computer back.
Function .onInstFailed
  nsExec::Exec '"$INSTDIR\${EXE}" update-handoff'
  nsExec::Exec 'net start ${APPNAME}'
FunctionEnd

!define ARP "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}"

Section "PJDesk (required)" SecCore
  SectionIn RO

  ; Upgrade in place: stop the service and close the app so the files unlock.
  nsExec::Exec 'net stop ${APPNAME}'
  nsExec::Exec 'taskkill /F /IM ${EXE}'
  Sleep 800

  SetOutPath "$INSTDIR"
  File "${EXE}"
  File "README.md"
  File "LICENSE"
  File "QUICKSTART.txt"

  WriteRegStr HKLM "Software\${APPNAME}" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Auto-update check every 30 minutes, as SYSTEM (short path avoids quoting the spaces).
  GetFullPathName /SHORT $0 "$INSTDIR\${EXE}"
  nsExec::Exec 'schtasks /Create /TN "PJDesk Update" /TR "$0 update" /SC MINUTE /MO 30 /RU SYSTEM /RL HIGHEST /F'
  ; The new exe restarts the service (several tries) and opens PJDesk in the
  ; signed-in user's session. net start is the fallback if that command is absent.
  nsExec::ExecToLog '"$INSTDIR\${EXE}" update-handoff'
  nsExec::Exec 'net start ${APPNAME}'

  Call RememberNetworkKey
  Call WriteComputerName

  CreateDirectory "$SMPROGRAMS\${APPNAME}"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\PJDesk.lnk" "$INSTDIR\${EXE}"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\Uninstall PJDesk.lnk" "$INSTDIR\uninstall.exe"

  WriteRegStr HKLM "${ARP}" "DisplayName" "${APPNAME}"
  WriteRegStr HKLM "${ARP}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${ARP}" "DisplayIcon" "$INSTDIR\${EXE}"
  WriteRegStr HKLM "${ARP}" "Publisher" "${COMPANY}"
  WriteRegStr HKLM "${ARP}" "URLInfoAbout" "https://pjdesk.com"
  WriteRegStr HKLM "${ARP}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr HKLM "${ARP}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" /S"
  WriteRegStr HKLM "${ARP}" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKLM "${ARP}" "NoModify" 1
  WriteRegDWORD HKLM "${ARP}" "NoRepair" 1
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\PJDesk.lnk" "$INSTDIR\${EXE}"
SectionEnd

Section /o "Background service (reach this PC at the sign-in screen)" SecService
  DetailPrint "Installing the PJDesk background service..."
  ExecWait '"$INSTDIR\${EXE}" service install --firewall'
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecCore} "The PJDesk app and Start-menu shortcut."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "Add a shortcut on the desktop."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecService} "Run PJDesk from startup as a background service, so you can connect before anyone signs in and during UAC prompts. Needs Administrator rights; a one-time password is shown during setup."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; Saves the optional friendly name where the app and the service can both read it.
; A silent install with no /NAME leaves an existing name alone. /NAME="" clears it.
Function WriteComputerName
  StrCmp $ComputerName "__KEEP__" computer_name_done
  CreateDirectory "$ProgramDataDir\PJDesk"
  FileOpen $0 "$ProgramDataDir\PJDesk\computer-name.txt" w
  IfErrors computer_name_done
  FileWrite $0 "$ComputerName"
  FileClose $0
  nsExec::Exec 'icacls "$ProgramDataDir\PJDesk\computer-name.txt" /inheritance:r /grant:r *S-1-5-18:F *S-1-5-32-544:F *S-1-5-32-545:M /Q'
  computer_name_done:
FunctionEnd

; If the setup file was renamed to PJDesk-Client-Setup-<version>--pjdesk-net1:<blob>.exe,
; remember that name beside the installed exe. The blob stays encrypted.
; The passphrase is not written. The path is not printed.
Function RememberNetworkKey
  ${StrStr} $0 $EXEPATH "pjdesk-net1:"
  StrCmp $0 "" network_key_done
  ClearErrors
  FileOpen $1 "$INSTDIR\network-key.name" w
  IfErrors network_key_done
  FileWrite $1 $EXEPATH
  FileClose $1
  network_key_done:
FunctionEnd

Section "Uninstall"
  nsExec::Exec 'schtasks /Delete /TN "PJDesk Update" /F'
  ; Remove the service if it was installed (ignored if it wasn't).
  ExecWait '"$INSTDIR\${EXE}" service uninstall'

  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\QUICKSTART.txt"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$INSTDIR\network-key.name"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APPNAME}\PJDesk.lnk"
  Delete "$SMPROGRAMS\${APPNAME}\Uninstall PJDesk.lnk"
  RMDir "$SMPROGRAMS\${APPNAME}"
  Delete "$DESKTOP\PJDesk.lnk"

  DeleteRegKey HKLM "${ARP}"
  DeleteRegKey HKLM "Software\${APPNAME}"
SectionEnd
