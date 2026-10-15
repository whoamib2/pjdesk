; PJDesk client installer (NSIS). Built by installer/build.sh, which places
; pjdesk.exe, README.md, LICENSE and QUICKSTART.txt next to this script.
Unicode true

!define APPNAME "PJDesk"
!define COMPANY "PJDesk"
!define EXE "pjdesk.exe"
!ifndef VERSION
  !define VERSION "0.2.0"
!endif

Name "${APPNAME}"
OutFile "PJDesk-Setup-${VERSION}.exe"
InstallDir "$PROGRAMFILES64\${APPNAME}"
InstallDirRegKey HKLM "Software\${APPNAME}" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma

!include "MUI2.nsh"

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

!define ARP "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}"

Section "PJDesk (required)" SecCore
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${EXE}"
  File "README.md"
  File "LICENSE"
  File "QUICKSTART.txt"

  WriteRegStr HKLM "Software\${APPNAME}" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"

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
