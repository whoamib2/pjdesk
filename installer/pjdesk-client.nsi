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
OutFile "PJDesk-Setup-${VERSION}.exe"
InstallDir "$PROGRAMFILES64\${APPNAME}"
InstallDirRegKey HKLM "Software\${APPNAME}" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma

!include "MUI2.nsh"

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

Section "Uninstall"
  nsExec::Exec 'schtasks /Delete /TN "PJDesk Update" /F'
  ; Remove the service if it was installed (ignored if it wasn't).
  ExecWait '"$INSTDIR\${EXE}" service uninstall'

  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\QUICKSTART.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APPNAME}\PJDesk.lnk"
  Delete "$SMPROGRAMS\${APPNAME}\Uninstall PJDesk.lnk"
  RMDir "$SMPROGRAMS\${APPNAME}"
  Delete "$DESKTOP\PJDesk.lnk"

  DeleteRegKey HKLM "${ARP}"
  DeleteRegKey HKLM "Software\${APPNAME}"
SectionEnd
