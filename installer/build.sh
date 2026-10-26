#!/usr/bin/env bash
# Builds the PJDesk client and server installers with NSIS.
#
# This script builds the Windows release first. A failed cargo build stops
# here and does not pack whatever binaries were left in target/.
# `makensis` must be on PATH (Linux: apt install nsis; Windows: NSIS).
#
# Usage: installer/build.sh [version]
#
# Executables are not packed (no UPX). Packing is a common false-positive trigger.
# Optional Authenticode signing: set PJDESK_SIGN_PFX to a PKCS#12 file and
# PJDESK_SIGN_PASSWORD to its password. Signing is skipped when PJDESK_SIGN_PFX
# is unset. osslsigncode or signtool must be on PATH when a certificate is set.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
TARGET="$ROOT/target/x86_64-pc-windows-gnu/release"
# The workspace version, unless the caller passed one: installer/build.sh 0.4.1
if [ -n "${1:-}" ]; then
  VERSION="$1"
else
  VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
  if [ -z "$VERSION" ]; then
    echo "Could not read the version from $ROOT/Cargo.toml" >&2
    exit 1
  fi
fi
WORK="$HERE/build"

# Build before packing. `set -e` exits on a bare failed command, and the exit
# code is still checked so a failed build never reaches makensis. A previous
# successful exe in target/ is not packed.
cd "$ROOT"
set +e
cargo build --release --target x86_64-pc-windows-gnu -p pjdesk -p pjdesk-server
cargo_status=$?
set -e
if [ "$cargo_status" -ne 0 ]; then
  echo "The Windows build failed (cargo exit ${cargo_status}). The installers were not packed." >&2
  exit "$cargo_status"
fi

for exe in pjdesk.exe pjdesk-server.exe; do
  if [ ! -f "$TARGET/$exe" ]; then
    echo "Missing $TARGET/$exe — build the Windows release first." >&2
    exit 1
  fi
done

rm -rf "$WORK"
mkdir -p "$WORK"
cp "$TARGET/pjdesk.exe" "$TARGET/pjdesk-server.exe" "$WORK/"
cp "$ROOT/README.md" "$ROOT/LICENSE" "$WORK/"
cp "$ROOT/crates/pjdesk/assets/pjdesk.ico" "$WORK/"
cp "$ROOT/crates/pjdesk-server/assets/pjdesk-server.ico" "$WORK/"
cp "$HERE/pjdesk-client.nsi" "$HERE/pjdesk-server.nsi" "$WORK/"

# QUICKSTART packaged with the client.
cat > "$WORK/QUICKSTART.txt" <<'EOF'
PJDesk - quick start
====================

1. Open PJDesk (Start menu or desktop).
2. Your computer shows an ID and a one-time password. Give those to
   someone to let them control it - no setup or sign-in needed.
3. To control another computer, type its ID and click Connect.
   Enter the password when that computer answers.

PJDesk connects through pjdesk.spader.us by default. To point it at your
own PJDesk server instead, set the address in Settings (default port 21200).

For always-on access - before anyone signs in, and during UAC prompts -
install the background service from Settings > Security, or tick
"Background service" in the installer.

Full documentation: README.md   .   https://pjdesk.com
EOF
# CRLF line endings for Notepad.
sed -i 's/$/\r/' "$WORK/QUICKSTART.txt"

# 0.6.6 -> 0.6.6.0. NSIS VIProductVersion needs four numeric parts.
VERQUAD="$VERSION"
case "$VERQUAD" in
  *.*.*.*) ;;
  *.*.*) VERQUAD="${VERQUAD}.0" ;;
  *.*) VERQUAD="${VERQUAD}.0.0" ;;
  *) VERQUAD="${VERQUAD}.0.0.0" ;;
esac

sign_file() {
  local file="$1"
  if [ -z "${PJDESK_SIGN_PFX:-}" ]; then
    return 0
  fi
  if command -v osslsigncode >/dev/null 2>&1; then
    osslsigncode sign -pkcs12 "$PJDESK_SIGN_PFX" -pass "${PJDESK_SIGN_PASSWORD:-}" \
      -n "PJDesk" -i "https://pjdesk.com" \
      -t "http://timestamp.digicert.com" \
      -in "$file" -out "$file.signed"
    mv -f "$file.signed" "$file"
  elif command -v signtool >/dev/null 2>&1; then
    signtool sign /f "$PJDESK_SIGN_PFX" /p "${PJDESK_SIGN_PASSWORD:-}" /fd SHA256 \
      /tr "http://timestamp.digicert.com" /td SHA256 /d "PJDesk" "$file"
  else
    echo "PJDESK_SIGN_PFX is set but neither osslsigncode nor signtool is on PATH." >&2
    exit 1
  fi
}

if [ -z "${PJDESK_SIGN_PFX:-}" ]; then
  echo "Authenticode signing skipped (set PJDESK_SIGN_PFX to sign)."
else
  sign_file "$WORK/pjdesk.exe"
  sign_file "$WORK/pjdesk-server.exe"
fi

cd "$WORK"
makensis -DVERSION="$VERSION" -DVERQUAD="$VERQUAD" pjdesk-client.nsi
makensis -DVERSION="$VERSION" -DVERQUAD="$VERQUAD" pjdesk-server.nsi

mv -f "PJDesk-Client-Setup-$VERSION.exe" "PJDesk-Server-Setup-$VERSION.exe" "$HERE/"
if [ -n "${PJDESK_SIGN_PFX:-}" ]; then
  sign_file "$HERE/PJDesk-Client-Setup-$VERSION.exe"
  sign_file "$HERE/PJDesk-Server-Setup-$VERSION.exe"
fi
# Installed 0.5.x and 0.6.x clients look up PJDesk-Setup-<ver>.exe on GitHub.
# The copy is byte-identical. SHA256SUMS lists both names with the same hash.
cp -f "$HERE/PJDesk-Client-Setup-$VERSION.exe" "$HERE/PJDesk-Setup-$VERSION.exe"

# Checksums the auto-updater verifies downloads against (upload with the release).
cd "$HERE"
sha256sum "PJDesk-Client-Setup-$VERSION.exe" "PJDesk-Setup-$VERSION.exe" "PJDesk-Server-Setup-$VERSION.exe" > SHA256SUMS

echo "Built:"
echo "  $HERE/PJDesk-Client-Setup-$VERSION.exe"
echo "  $HERE/PJDesk-Setup-$VERSION.exe"
echo "  $HERE/PJDesk-Server-Setup-$VERSION.exe"
echo "  $HERE/SHA256SUMS"
