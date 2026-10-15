@echo off
rem Builds DeskLink (server + client) in release mode and copies the
rem programs to .\dist. Run from anywhere: it switches to its own folder.
setlocal
cd /d "%~dp0"

where cargo >nul 2>nul
if errorlevel 1 (
  echo Rust is not installed. Run the one-time setup in README.md ^(build tools, then Rust^),
  echo then open a NEW terminal and run build.cmd again.
  exit /b 1
)

rem NASM is optional. With it, OpenH264 gets its hand-written SIMD code
rem (several times faster encoding). winget installs it without adding it
rem to PATH, so look in the default folder too.
where nasm >nul 2>nul
if errorlevel 1 if exist "%ProgramFiles%\NASM\nasm.exe" set "PATH=%ProgramFiles%\NASM;%PATH%"
where nasm >nul 2>nul
if errorlevel 1 echo Note: NASM not found - building the H.264 codec without SIMD. Install: winget install --id NASM.NASM -e

cargo build --release --workspace
if errorlevel 1 exit /b 1

if not exist dist mkdir dist
copy /y target\release\desklink.exe dist\ >nul
copy /y target\release\desklink-server.exe dist\ >nul
echo.
echo Built:
echo   %CD%\dist\desklink-server.exe
echo   %CD%\dist\desklink.exe
