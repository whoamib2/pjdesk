#!/usr/bin/env bash
# Run a command with PJDESK_UPDATE_TOKEN loaded from
# $HOME/pjdesk-update-token.txt (or $PJDESK_UPDATE_TOKEN_FILE).
# The value is never printed. It is unset when the command finishes.
set -euo pipefail
path="${PJDESK_UPDATE_TOKEN_FILE:-$HOME/pjdesk-update-token.txt}"
if [ ! -f "$path" ]; then
  echo "update token file is missing" >&2
  exit 1
fi
# tr strips CR/LF; the value is only assigned, never echoed.
PJDESK_UPDATE_TOKEN="$(tr -d '\r\n' < "$path")"
export PJDESK_UPDATE_TOKEN
if [ -z "$PJDESK_UPDATE_TOKEN" ]; then
  unset PJDESK_UPDATE_TOKEN
  echo "update token file is empty" >&2
  exit 1
fi
if [ "$#" -lt 1 ]; then
  unset PJDESK_UPDATE_TOKEN
  echo "pass the command to run" >&2
  exit 1
fi
set +e
"$@"
status=$?
set -e
unset PJDESK_UPDATE_TOKEN
exit "$status"
