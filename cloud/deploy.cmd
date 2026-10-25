@echo off
rem Deploys PJDesk Cloud (the pjdesk.com website + account service) to
rem Cloudflare from this PC. Run it from this folder: deploy.cmd
rem
rem npx.cmd and npm.cmd start with SETLOCAL and do not return the program's
rem exit code, so "if errorlevel 1" after "call npx wrangler" stays 0.
rem Run Wrangler with node.exe. That leaves errorlevel set. Stop on the
rem first failed step and do not print success.
setlocal
cd /d "%~dp0"

where node >nul 2>nul
if errorlevel 1 (
  echo Node.js is needed to deploy. Install it with:
  echo     winget install OpenJS.NodeJS.LTS
  echo then close this window, open a new one, and run deploy.cmd again.
  exit /b 1
)

if exist node_modules goto :have_modules
echo Installing Cloudflare's deploy tool ^(first run only^)...
set "NPM_CLI="
for /f "delims=" %%I in ('where npm.cmd 2^>nul') do (
  set "NPM_CLI=%%~dpInode_modules\npm\bin\npm-cli.js"
  goto :npm_found
)
:npm_found
if not defined NPM_CLI (
  echo npm was not found. Install Node.js, then run deploy.cmd again.
  exit /b 1
)
if not exist "%NPM_CLI%" (
  echo npm was not found. Install Node.js, then run deploy.cmd again.
  exit /b 1
)
node "%NPM_CLI%" install --no-audit --no-fund
if errorlevel 1 (
  echo npm install failed.
  exit /b 1
)
:have_modules

set "WRANGLER=node_modules\wrangler\bin\wrangler.js"
if not exist "%WRANGLER%" (
  echo Wrangler is not installed. In this folder, run: npm install
  exit /b 1
)

rem Confirm the Cloudflare account before changing the database or the worker.
set "WHOAMI=%TEMP%\pjdesk-wrangler-whoami.txt"
node "%WRANGLER%" whoami >"%WHOAMI%" 2>&1
if errorlevel 1 (
  type "%WHOAMI%"
  echo.
  echo Wrangler is not signed in. Run: npx wrangler login
  del "%WHOAMI%" >nul 2>nul
  exit /b 1
)
findstr /i /c:"pjdesk@spader.us" "%WHOAMI%" >nul
if errorlevel 1 (
  type "%WHOAMI%"
  echo.
  echo Wrangler is not signed in. Run: npx wrangler login
  del "%WHOAMI%" >nul 2>nul
  exit /b 1
)
del "%WHOAMI%" >nul 2>nul

echo Updating the database...
set "CI=true"
node "%WRANGLER%" d1 migrations apply pjdesk-cloud --remote
if errorlevel 1 (
  echo Database update failed.
  exit /b 1
)
set "CI="

echo Deploying...
node "%WRANGLER%" deploy
if errorlevel 1 (
  echo Deploy failed.
  exit /b 1
)

echo.
echo PJDesk Cloud is deployed.
exit /b 0
