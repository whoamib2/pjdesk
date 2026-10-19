#!/usr/bin/env bash
# Builds libpjdesk_mobile for Android, then a debug APK and an unsigned
# release APK. A release key is used only when PJDESK_KEYSTORE,
# PJDESK_KEYSTORE_PASSWORD, PJDESK_KEY_ALIAS, and PJDESK_KEY_PASSWORD are set.
# Keystores are never created or stored by this script.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/android-sdk}}"
export ANDROID_HOME
if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
  if [[ -d "$ANDROID_HOME/ndk/27.2.12479018" ]]; then
    ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.2.12479018"
  else
    ANDROID_NDK_HOME="$(find "$ANDROID_HOME/ndk" -mindepth 1 -maxdepth 1 -type d | sort | tail -1)"
  fi
fi
export ANDROID_NDK_HOME
export ANDROID_SDK_ROOT="$ANDROID_HOME"

cd "$ROOT"
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 --platform 26 \
  -o "$ROOT/android/app/src/main/jniLibs" \
  build --release -p pjdesk-mobile

printf 'sdk.dir=%s\n' "$ANDROID_HOME" > "$ROOT/android/local.properties"
cd "$ROOT/android"
chmod +x ./gradlew
./gradlew assembleDebug assembleRelease --no-daemon

VER="$(sed -n 's/.*versionName = "\([^"]*\)".*/\1/p' app/build.gradle.kts | head -1)"
VER="${VER:-0.6.0}"
mkdir -p "$ROOT/dist"
cp -f app/build/outputs/apk/debug/app-debug.apk "$ROOT/dist/PJDesk-Android-${VER}-debug.apk"
REL="$(find app/build/outputs/apk/release -name '*.apk' | head -1)"
cp -f "$REL" "$ROOT/dist/PJDesk-Android-${VER}.apk"
echo "debug  $ROOT/dist/PJDesk-Android-${VER}-debug.apk"
echo "release $ROOT/dist/PJDesk-Android-${VER}.apk"
