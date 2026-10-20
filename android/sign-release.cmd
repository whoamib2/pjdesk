@echo off
setlocal EnableExtensions
rem Signs an unsigned PJDesk APK with apksigner.
rem Passwords are asked in this window when they are not already in the
rem environment, and they are not written to a file.
rem
rem   android\sign-release.cmd PJDesk-Android-0.6.9-unsigned.apk
rem   android\sign-release.cmd unsigned.apk signed.apk
rem
rem Environment, all optional:
rem   PJDESK_KEYSTORE
rem   PJDESK_KEY_ALIAS
rem   PJDESK_KEYSTORE_PASSWORD
rem   PJDESK_KEY_PASSWORD
rem   ANDROID_HOME
rem
rem If PJDESK_KEYSTORE is unset and this file exists, it is the suggested path:
rem   C:\Users\BenSpader\AndroidStudioProjects\upload.jks
rem That file is not confirmed to be the key that signed the installed app.
rem Sign with the same key or the phone will refuse the update.

if "%~1"=="" goto usage
set "PJDESK_IN=%~f1"
if not exist "%PJDESK_IN%" (
  echo APK not found: %PJDESK_IN%
  exit /b 1
)
if "%~2"=="" (
  set "PJDESK_OUT=%~dpn1-signed.apk"
) else (
  set "PJDESK_OUT=%~f2"
)

if not defined PJDESK_KEYSTORE (
  if exist "C:\Users\BenSpader\AndroidStudioProjects\upload.jks" (
    set "PJDESK_KEYSTORE=C:\Users\BenSpader\AndroidStudioProjects\upload.jks"
    echo Using keystore C:\Users\BenSpader\AndroidStudioProjects\upload.jks
    echo If that is not the key that signed the installed app, type a different path.
  )
)
if not defined PJDESK_KEYSTORE set /p PJDESK_KEYSTORE=Keystore path: 
if not exist "%PJDESK_KEYSTORE%" (
  echo Keystore not found: %PJDESK_KEYSTORE%
  exit /b 1
)
if not defined PJDESK_KEY_ALIAS set /p PJDESK_KEY_ALIAS=Key alias: 
if "%PJDESK_KEY_ALIAS%"=="" (
  echo A key alias is required.
  exit /b 1
)

set "APKSIGNER="
if defined ANDROID_HOME (
  for /f "delims=" %%D in ('dir /b /ad /o-n "%ANDROID_HOME%\build-tools" 2^>nul') do (
    if not defined APKSIGNER if exist "%ANDROID_HOME%\build-tools\%%D\apksigner.bat" (
      set "APKSIGNER=%ANDROID_HOME%\build-tools\%%D\apksigner.bat"
    )
  )
)
if not defined APKSIGNER (
  echo apksigner was not found. Set ANDROID_HOME to the Android SDK.
  exit /b 1
)

powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$ErrorActionPreference = 'Stop';" ^
  "$store = $env:PJDESK_KEYSTORE_PASSWORD;" ^
  "if ([string]::IsNullOrEmpty($store)) {" ^
  "  $sec = Read-Host 'Keystore password' -AsSecureString;" ^
  "  $store = [Runtime.InteropServices.Marshal]::PtrToStringUni([Runtime.InteropServices.Marshal]::SecureStringToBSTR($sec));" ^
  "}" ^
  "$key = $env:PJDESK_KEY_PASSWORD;" ^
  "if ([string]::IsNullOrEmpty($key)) {" ^
  "  $sec = Read-Host 'Key password (press Enter if it is the same)' -AsSecureString;" ^
  "  $key = [Runtime.InteropServices.Marshal]::PtrToStringUni([Runtime.InteropServices.Marshal]::SecureStringToBSTR($sec));" ^
  "  if ([string]::IsNullOrEmpty($key)) { $key = $store }" ^
  "}" ^
  "& $env:APKSIGNER sign --ks $env:PJDESK_KEYSTORE --ks-key-alias $env:PJDESK_KEY_ALIAS --ks-pass ('pass:' + $store) --key-pass ('pass:' + $key) --out $env:PJDESK_OUT $env:PJDESK_IN;" ^
  "if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }" ^
  "& $env:APKSIGNER verify --verbose $env:PJDESK_OUT;" ^
  "exit $LASTEXITCODE"
set "RC=%ERRORLEVEL%"
if not "%RC%"=="0" exit /b %RC%
echo Signed %PJDESK_OUT%
exit /b 0

:usage
echo Usage: android\sign-release.cmd unsigned.apk [signed.apk]
exit /b 1
