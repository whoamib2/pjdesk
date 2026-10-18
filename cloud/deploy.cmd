@echo off
rem Deploys PJDesk Cloud (the pjdesk.com website + account service) to
rem Cloudflare from this PC. Run it from this folder: deploy.cmd
setlocal
cd /d "%~dp0"

where node >nul 2>nul
if errorlevel 1 (
  echo Node.js is needed to deploy. Install it with:
  echo     winget install OpenJS.NodeJS.LTS
  echo then close this window, open a new one, and run deploy.cmd again.
  exit /b 1
)

if not exist node_modules (
  echo Installing Cloudflare's deploy tool ^(first run only^)...
  call npm install --no-audit --no-fund
  if errorlevel 1 exit /b 1
)

rem Sign in to Cloudflare as pjdesk@spader.us (opens your browser the first time).
call npx wrangler whoami 2>nul | findstr /i /c:"pjdesk@spader.us" >nul
if errorlevel 1 (
  call npx wrangler logout >nul 2>nul
  echo A browser window will open: sign in to Cloudflare as pjdesk@spader.us and click Allow.
  call npx wrangler login
  if errorlevel 1 exit /b 1
)

echo Updating the database...
set "CI=true"
call npx wrangler d1 migrations apply pjdesk-cloud --remote
if errorlevel 1 exit /b 1
set "CI="

echo Deploying...
call npx wrangler deploy
if errorlevel 1 exit /b 1

echo.
echo PJDesk Cloud is deployed.
endlocal
