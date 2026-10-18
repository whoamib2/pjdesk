; PJDesk server installer (NSIS). Built by installer/build.sh, which places
; pjdesk-server.exe, README.md and LICENSE next to this script.
Unicode true

!define APPNAME "PJDesk Server"
!define COMPANY "PJDesk"
!define EXE "pjdesk-server.exe"
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
OutFile "PJDesk-Server-Setup-${VERSION}.exe"
InstallDir "$PROGRAMFILES64\PJDesk Server"
InstallDirRegKey HKLM "Software\PJDeskServer" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma

!include "MUI2.nsh"

!define MUI_ICON "pjdesk-server.ico"
!define MUI_UNICON "pjdesk-server.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Open the PJDesk Server window"

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
FunctionEnd

Function un.onInit
  System::Call 'kernel32::SetDllDirectoryA(t "")'
FunctionEnd

; If the install stops after the service was stopped, bring it back.
Function .onInstFailed
  nsExec::Exec '"$INSTDIR\${EXE}" update-handoff'
  nsExec::Exec 'net start PJDeskServer'
FunctionEnd

!define ARP "Software\Microsoft\Windows\CurrentVersion\Uninstall\PJDeskServer"

Section "PJDesk Server (required)" SecCore
  SectionIn RO

  ; Upgrade in place: stop the service and close the window so files unlock.
  nsExec::Exec 'net stop PJDeskServer'
  nsExec::Exec 'taskkill /F /IM ${EXE}'
  Sleep 800

  SetOutPath "$INSTDIR"
  File "${EXE}"
  File "README.md"
  File "LICENSE"

  WriteRegStr HKLM "Software\PJDeskServer" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Automatic updates are off until an administrator turns them on
  ; (`pjdesk-server update-task`). Drop a task left by 0.3.x, which installed
  ; every newer release on its own. The data folder under ProgramData is not
  ; touched: the key, accounts, cloud settings and the update preference stay.
  nsExec::Exec 'schtasks /Delete /TN "PJDesk Server Update" /F'
  ; Tray icon for the user who installed, started at every sign-in.
  ; The service itself cannot show a tray (it runs in session 0).
  WriteRegDWORD HKCU "Software\PJDeskServer" "TrayAtLogin" 1
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "PJDesk Server" '"$INSTDIR\${EXE}" --tray'
  ; Restart the service (several tries) and start the tray in the signed-in
  ; user's session. Launching it from this installer would run it as SYSTEM
  ; when the update task is the one installing. net start is the fallback.
  nsExec::ExecToLog '"$INSTDIR\${EXE}" update-handoff'
  nsExec::Exec 'net start PJDeskServer'

  CreateDirectory "$SMPROGRAMS\${APPNAME}"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\PJDesk Server.lnk" "$INSTDIR\${EXE}"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\Uninstall.lnk" "$INSTDIR\uninstall.exe"

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

Section "Run as a service now (recommended)" SecService
  DetailPrint "Installing and starting the PJDesk Server service..."
  ExecWait '"$INSTDIR\${EXE}" service install --firewall'
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecCore} "The PJDesk Server program and its management window."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecService} "Install and start the server as an auto-start Windows service (listens on TCP+UDP 21200 and adds a firewall rule). You can also do this later from the server window."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  nsExec::Exec 'schtasks /Delete /TN "PJDesk Server Update" /F'
  ExecWait '"$INSTDIR\${EXE}" service uninstall'

  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APPNAME}\PJDesk Server.lnk"
  Delete "$SMPROGRAMS\${APPNAME}\Uninstall.lnk"
  RMDir "$SMPROGRAMS\${APPNAME}"

  DeleteRegKey HKLM "${ARP}"
  DeleteRegKey HKLM "Software\PJDeskServer"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "PJDesk Server"
  DeleteRegValue HKCU "Software\PJDeskServer" "TrayAtLogin"
  DeleteRegKey HKCU "Software\PJDeskServer"
SectionEnd
