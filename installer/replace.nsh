; Shared by the client and server installers.
;
; A silent NSIS install skips a locked file and still exits 0. That left
; pjdesk-server.exe on the old build while the registry version and
; uninstall.exe moved forward, and the service kept running the old program.
; The new bytes are extracted first (the running program does not lock that
; copy). The service is then stopped and the image is killed until it is gone,
; so a watchdog cannot lock the file again before the copy. The old file is
; renamed aside, the new file is copied into its place, and the two files are
; compared. A failed attempt puts the old file back. When every attempt fails
; the service is started again and the installer exits non-zero.

!include LogicLib.nsh

Var PjInstallLog
Var PjImage
Var PjService
Var PjRunning

!macro PjDeclare UN ID
Function ${UN}PjInstallLog
  Exch $R9
  Push $R8
  DetailPrint $R9
  ${If} $PjInstallLog == ""
    ReadEnvStr $R8 ProgramData
    ${If} $R8 == ""
      StrCpy $R8 "C:\ProgramData"
    ${EndIf}
    CreateDirectory "$R8\PJDesk\updates"
    StrCpy $PjInstallLog "$R8\PJDesk\updates\install.log"
  ${EndIf}
  ClearErrors
  FileOpen $R8 "$PjInstallLog" a
  ${IfNot} ${Errors}
    FileSeek $R8 0 END
    FileWrite $R8 "$R9$\r$\n"
    FileClose $R8
  ${EndIf}
  ${If} $INSTDIR != ""
    ClearErrors
    FileOpen $R8 "$INSTDIR\install.log" a
    ${IfNot} ${Errors}
      FileSeek $R8 0 END
      FileWrite $R8 "$R9$\r$\n"
      FileClose $R8
    ${EndIf}
  ${EndIf}
  ClearErrors
  Pop $R8
  Pop $R9
FunctionEnd

Function ${UN}PjImageRunning
  Push $R1
  Push $R2
  Push $R3
  StrCpy $PjRunning 0
  ; find exits 0 when the image is in the list. tasklist's own exit code does not.
  nsExec::Exec 'cmd /c tasklist /NH /FI "IMAGENAME eq $PjImage" | find /I "$PjImage" >nul'
  Pop $R1
  ${If} $R1 == 0
    StrCpy $PjRunning 1
  ${EndIf}
  ClearErrors
  Pop $R3
  Pop $R2
  Pop $R1
FunctionEnd

Function ${UN}PjStopImage
  Push $R0
  Push "Stopping $PjService and waiting until $PjImage has exited."
  Call ${UN}PjInstallLog
  nsExec::Exec 'net stop "$PjService"'
  nsExec::Exec 'sc stop "$PjService"'
  StrCpy $R0 0
${ID}wait:
  Call ${UN}PjImageRunning
  ${If} $PjRunning == 0
    Goto ${ID}wait_done
  ${EndIf}
  nsExec::Exec 'taskkill /F /T /IM "$PjImage"'
  Sleep 500
  IntOp $R0 $R0 + 1
  ${If} $R0 < 40
    Goto ${ID}wait
  ${EndIf}
  Push "$PjImage is still running. The copy will rename the locked file and try again."
  Call ${UN}PjInstallLog
${ID}wait_done:
  Pop $R0
FunctionEnd
!macroend

!insertmacro PjDeclare "" "inst_"
!insertmacro PjDeclare "un." "uninst_"

; The new ${IMAGE} is already at $PLUGINSDIR\${IMAGE}. SERVICE is the SCM name.
!macro SwapLockedExe SERVICE IMAGE
  StrCpy $PjService "${SERVICE}"
  StrCpy $PjImage "${IMAGE}"
  Push "${VERSION} Replacing $PjImage for $PjService."
  Call PjInstallLog
  ; cmd.exe creates a file with this name when a command uses PowerShell's
  ; discard word as its redirect target. The install folder is the working
  ; directory of the shortcuts and of programs this installer launches.
  IfFileExists "$INSTDIR\$$null" 0 inst_pjswap_no_stray
    Delete "$INSTDIR\$$null"
    Push "Removed a stray file whose name is a dollar sign followed by null. It is left by a cmd redirect, not by the PJDesk programs."
    Call PjInstallLog
  inst_pjswap_no_stray:
  ${IfNot} ${FileExists} "$PLUGINSDIR\${IMAGE}"
    Push "The installer is missing $PLUGINSDIR\${IMAGE}."
    Call PjInstallLog
    SetErrorLevel 1
    Abort "The installer is missing ${IMAGE}."
  ${EndIf}
  Call PjStopImage
  StrCpy $R0 0
  inst_pjswap_try:
    nsExec::Exec 'taskkill /F /T /IM "$PjImage"'
    Sleep 400
    ${IfNot} ${FileExists} "$INSTDIR\$PjImage"
      ${If} ${FileExists} "$INSTDIR\$PjImage.old"
        ClearErrors
        Rename "$INSTDIR\$PjImage.old" "$INSTDIR\$PjImage"
      ${EndIf}
    ${EndIf}
    ClearErrors
    ${If} ${FileExists} "$INSTDIR\$PjImage"
      Rename "$INSTDIR\$PjImage" "$INSTDIR\$PjImage.old"
      IfErrors inst_pjswap_locked
    ${EndIf}
    ClearErrors
    CopyFiles /SILENT /FILESONLY "$PLUGINSDIR\${IMAGE}" "$INSTDIR\$PjImage"
    IfErrors inst_pjswap_restore
    nsExec::Exec 'cmd /c fc /b "$PLUGINSDIR\${IMAGE}" "$INSTDIR\$PjImage" >nul'
    Pop $R1
    ${If} $R1 == 0
      Goto inst_pjswap_ok
    ${EndIf}
  inst_pjswap_restore:
    ${If} ${FileExists} "$INSTDIR\$PjImage.old"
      Delete "$INSTDIR\$PjImage"
      ClearErrors
      Rename "$INSTDIR\$PjImage.old" "$INSTDIR\$PjImage"
    ${EndIf}
  inst_pjswap_locked:
    IntOp $R0 $R0 + 1
    ${If} $R0 < 12
      Push "Could not replace $PjImage (attempt $R0). Waiting for the lock to clear."
      Call PjInstallLog
      Sleep 500
      Goto inst_pjswap_try
    ${EndIf}
    Goto inst_pjswap_fail
  inst_pjswap_ok:
    ClearErrors
    Delete "$INSTDIR\$PjImage.old"
    IfErrors 0 inst_pjswap_old_cleared
      System::Call 'kernel32::MoveFileEx(t "$INSTDIR\$PjImage.old", p 0, i 4)'
    inst_pjswap_old_cleared:
    Push "Replaced $PjImage. The installed file matches the installer. Starting $PjService."
    Call PjInstallLog
    ; End a process that still has the old file mapped, then start the new one.
    Call PjStopImage
    Call PjImageRunning
    ${If} $PjRunning == 1
      Push "$PjImage was replaced but a process is still running. $PjService will keep serving until that process exits."
      Call PjInstallLog
    ${EndIf}
    nsExec::Exec 'net start "$PjService"'
    ClearErrors
    Goto inst_pjswap_done
  inst_pjswap_fail:
    ${IfNot} ${FileExists} "$INSTDIR\$PjImage"
      ${If} ${FileExists} "$INSTDIR\$PjImage.old"
        Rename "$INSTDIR\$PjImage.old" "$INSTDIR\$PjImage"
      ${EndIf}
    ${EndIf}
    Push "$PjService did not replace $PjImage. Starting the previous program again."
    Call PjInstallLog
    nsExec::Exec 'net start "$PjService"'
    ${If} ${FileExists} "$INSTDIR\$PjImage"
      nsExec::ExecToLog '"$INSTDIR\$PjImage" update-handoff'
    ${EndIf}
    nsExec::Exec 'net start "$PjService"'
    SetErrorLevel 1
    Abort "$PjService did not replace $PjImage. The previous program was started again. See C:\ProgramData\PJDesk\updates\install.log"
  inst_pjswap_done:
!macroend
