#!/usr/bin/env bash
# Builds the PJDesk client and server installers with NSIS.
#
# Prerequisites: the Windows release binaries have been built
#   cargo build --release --target x86_64-pc-windows-gnu
# and `makensis` is on PATH (Linux: apt install nsis; Windows: NSIS).
#
# Usage: installer/build.sh [version]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
TARGET="$ROOT/target/x86_64-pc-windows-gnu/release"
VERSION="${1:-0.2.0}"
WORK="$HERE/build"

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
cp "$HERE/pjdesk-client.nsi" "$HERE/pjdesk-server.nsi" "$WORK/"

# QUICKSTART packaged with the client.
cat > "$WORK/QUICKSTART.txt" <<'EOF'
PJDesk - quick start
====================

1. Open PJDesk (Start menu or desktop).
2. Your computer shows an ID and a one-time password. Give those to
   someone to let them control it - no setup or sign-in needed.
3. To control another computer, type its ID and password, then Connect.

PJDesk connects through pjdesk.spader.us by default. To point it at your
own PJDesk server instead, set the address in Settings (default port 21200).

For always-on access - before anyone signs in, and during UAC prompts -
install the background service from Settings > Security, or tick
"Background service" in the installer.

Full documentation: README.md   .   https://pjdesk.com
EOF
# CRLF line endings for Notepad.
sed -i 's/$/\r/' "$WORK/QUICKSTART.txt"

cd "$WORK"
makensis -DVERSION="$VERSION" pjdesk-client.nsi
makensis -DVERSION="$VERSION" pjdesk-server.nsi

mv -f "PJDesk-Setup-$VERSION.exe" "PJDesk-Server-Setup-$VERSION.exe" "$HERE/"

# Checksums the auto-updater verifies downloads against (upload with the release).
cd "$HERE"
sha256sum "PJDesk-Setup-$VERSION.exe" "PJDesk-Server-Setup-$VERSION.exe" > SHA256SUMS

echo "Built:"
echo "  $HERE/PJDesk-Setup-$VERSION.exe"
echo "  $HERE/PJDesk-Server-Setup-$VERSION.exe"
echo "  $HERE/SHA256SUMS"
