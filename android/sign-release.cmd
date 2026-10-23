@echo off
setlocal EnableExtensions
rem Signs an unsigned PJDesk APK with apksigner.
rem Passwords are asked in this window when they are not already in the
rem environment, and they are not written to a file.
rem
rem   android\sign-release.cmd PJDesk-Android-0.6.10-unsigned.apk
rem   android\sign-release.cmd unsigned.apk signed.apk
rem
rem Environment, all optional:
rem   PJDESK_KEYSTORE   default C:\Users\BenSpader\.pjdesk-android\pjdesk-release.jks
rem   PJDESK_KEY_ALIAS  default pjdesk
rem   PJDESK_KEYSTORE_PASSWORD
rem   PJDESK_KEY_PASSWORD
rem   ANDROID_HOME
rem
rem The release key is C:\Users\BenSpader\.pjdesk-android\pjdesk-release.jks, alias pjdesk.
rem Sign with that key or the phone will refuse the update. Passwords are not saved.

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

if not defined PJDESK_KEYSTORE set "PJDESK_KEYSTORE=C:\Users\BenSpader\.pjdesk-android\pjdesk-release.jks"
if not defined PJDESK_KEY_ALIAS set "PJDESK_KEY_ALIAS=pjdesk"
echo Using keystore %PJDESK_KEYSTORE%
echo Using alias %PJDESK_KEY_ALIAS%
if not exist "%PJDESK_KEYSTORE%" (
  echo Keystore not found: %PJDESK_KEYSTORE%
  echo Set PJDESK_KEYSTORE if the release key is somewhere else.
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
