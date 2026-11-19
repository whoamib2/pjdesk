# Run a command with PJDESK_UPDATE_TOKEN loaded from
# %USERPROFILE%\pjdesk-update-token.txt (or PJDESK_UPDATE_TOKEN_FILE).
# The value is never printed. It is removed from the environment when the
# command finishes. Usage:
#   powershell -NoProfile -File cloud\tools\with-update-token.ps1 -- installer\build.sh 0.6.97
$ErrorActionPreference = 'Stop'
$path = if ($env:PJDESK_UPDATE_TOKEN_FILE) { $env:PJDESK_UPDATE_TOKEN_FILE } else { Join-Path $env:USERPROFILE 'pjdesk-update-token.txt' }
if (-not (Test-Path -LiteralPath $path)) { throw "update token file is missing" }
$token = [System.IO.File]::ReadAllText($path).Trim()
if ([string]::IsNullOrWhiteSpace($token)) { throw "update token file is empty" }
$env:PJDESK_UPDATE_TOKEN = $token
$token = $null
try {
  if ($args.Count -lt 1) { throw "pass the command to run" }
  $cmd = $args[0]
  $cmdArgs = @()
  if ($args.Count -gt 1) { $cmdArgs = $args[1..($args.Count - 1)] }
  & $cmd @cmdArgs
  exit $LASTEXITCODE
} finally {
  Remove-Item Env:PJDESK_UPDATE_TOKEN -ErrorAction SilentlyContinue
}
