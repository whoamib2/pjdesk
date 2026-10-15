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
2. In Settings, enter the address of your PJDesk server (for example
   desk.example.com or 192.168.1.10). The default port is 21200.
3. Your computer now shows an ID and a password. Give those to someone,
   or type another computer's ID to control it.

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
echo "Built:"
echo "  $HERE/PJDesk-Setup-$VERSION.exe"
echo "  $HERE/PJDesk-Server-Setup-$VERSION.exe"
