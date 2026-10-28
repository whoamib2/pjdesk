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

!ifndef VERQUAD
  !define VERQUAD "${VERSION}.0"
!endif
VIProductVersion "${VERQUAD}"
VIAddVersionKey "ProductName" "PJDesk"
VIAddVersionKey "CompanyName" "PJDesk"
VIAddVersionKey "FileDescription" "PJDesk remote desktop installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "PJDesk"
VIAddVersionKey "OriginalFilename" "PJDesk-Client-Setup-${VERSION}.exe"

!include "MUI2.nsh"
!include "StrFunc.nsh"
${Using:StrFunc} StrStr
!include "replace.nsh"

!define MUI_ICON "pjdesk.ico"
!define MUI_UNICON "pjdesk.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Open PJDesk now"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; Don't load DLLs from the folder the installer was launched from.
Function .onInit
  System::Call 'kernel32::SetDllDirectoryA(t "")'
  ; A locked file must fail the install. The default lets a silent install skip it.
  AllowSkipFiles off
FunctionEnd

Function un.onInit
  System::Call 'kernel32::SetDllDirectoryA(t "")'
FunctionEnd

; If the install stops after the service was stopped, put the previous
; program back when the new file is not there, then bring the service back.
Function .onInstFailed
  IfFileExists "$INSTDIR\${EXE}" client_failed_start 0
    IfFileExists "$INSTDIR\${EXE}.old" 0 client_failed_start
      Rename "$INSTDIR\${EXE}.old" "$INSTDIR\${EXE}"
  client_failed_start:
  StrCpy $PjService "${APPNAME}"
  Call PjEnsureRunning
  nsExec::Exec '"$INSTDIR\${EXE}" update-handoff'
  nsExec::Exec 'net start ${APPNAME}'
  Call PjEnsureRunning
FunctionEnd

!define ARP "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}"

Section "PJDesk (required)" SecCore
  SectionIn RO

  ; Extract beside the running program, then stop, wait, and swap.
  ; The service is started again inside the swap, before the registry is written.
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  SetOverwrite on
  File "${EXE}"
  SetOutPath "$INSTDIR"
  !insertmacro SwapLockedExe "${APPNAME}" "${EXE}"
  File "README.md"
  File "LICENSE"
  File "QUICKSTART.txt"

  WriteRegStr HKLM "Software\${APPNAME}" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; No scheduled task. The service installs updates itself. A task that
  ; Task Scheduler (svchost) launched was being flagged by Defender.
  nsExec::Exec 'schtasks /Delete /TN "PJDesk Update" /F'
  ; UDP 21201 is the direct LAN port. A program rule whose path contains
  ; spaces often does not apply, so the port rule is explicit.
  nsExec::Exec 'netsh advfirewall firewall delete rule name="PJDesk LAN"'
  nsExec::Exec 'netsh advfirewall firewall add rule name="PJDesk LAN" dir=in action=allow protocol=UDP localport=21201 profile=any'
  nsExec::Exec 'netsh advfirewall firewall delete rule name="PJDesk"'
  nsExec::Exec 'netsh advfirewall firewall add rule name="PJDesk" dir=in action=allow program="$INSTDIR\${EXE}" enable=yes profile=any'
  ; Unattended access is the service. Every install, including a silent update,
  ; installs and starts it. An existing computer-name.txt is left alone; a
  ; fresh install uses the Windows name until someone renames it in the app.
  DetailPrint "Installing the PJDesk background service..."
  ExecWait '"$INSTDIR\${EXE}" service install --firewall'
  ; The new exe restarts the service (several tries) and opens PJDesk in the
  ; signed-in user's session. net start is the fallback if that command is absent.
  nsExec::ExecToLog '"$INSTDIR\${EXE}" update-handoff'
  nsExec::Exec 'net start ${APPNAME}'

  Call RememberNetworkKey

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

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecCore} "The PJDesk app, Start-menu shortcut, and background service (so this PC can be reached at the sign-in screen)."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "Add a shortcut on the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

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
  StrCpy $PjService "${APPNAME}"
  StrCpy $PjImage "${EXE}"
  Call un.PjStopImage
  nsExec::Exec 'schtasks /Delete /TN "PJDesk Update" /F'
  ; Remove the service if it was installed (ignored if it wasn't).
  ExecWait '"$INSTDIR\${EXE}" service uninstall'

  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\${EXE}.old"
  Delete "$INSTDIR\install.log"
  Delete "$INSTDIR\$$null"
  IfFileExists "$INSTDIR\${EXE}" 0 client_exe_gone
    SetErrorLevel 1
  client_exe_gone:
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
