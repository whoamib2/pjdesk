# PJDesk for Android

The Android app is both a host and a controller. It speaks the same protocol as the Windows client. A fresh install has no server until you sign in with a PJDesk account; the account supplies the private server address and key. The Rust core is `crates/pjdesk-mobile` (`libpjdesk_mobile`), built with cargo-ndk and called through JNI. The UI is Kotlin and Jetpack Compose.

## Codec

Video is **H.264 Annex-B**, the access-unit format already used by the Windows client.

* The Windows host encodes with OpenH264 and the Windows viewer decodes with OpenH264 (`crates/pjdesk/src/viewer/decoder.rs`). No second decoder was added.
* The Android host encodes with MediaCodec `video/avc` on a `MediaProjection` surface. Constrained Baseline is requested when the device accepts it. Codec-config and length-prefixed buffers are converted to Annex-B, and SPS/PPS are repeated on each keyframe (`AnnexB.kt`), which is what OpenH264 reads.
* The Android controller decodes those same Annex-B access units with MediaCodec onto a `Surface`.

Flow control is unchanged: at most three frames are in flight, and the controller sends `FrameAck`.

## Permissions

The user grants these before this phone can be controlled:

1. **Notifications**, so the foreground service can show a persistent notification. While a session is active the notification says the phone is being controlled and has a one-tap **Stop**.
2. **Accessibility** (`PjAccessibilityService`). Remote taps, swipes, long-press, and pinch use `dispatchGesture`. Back, Home, and Recents are global actions. Typed text uses `ACTION_SET_TEXT` on the focused field. Volume uses `AudioManager`. Power locks the screen on Android 9+ and opens the power dialog on older releases. The service does not turn on touch exploration.
3. **Screen capture** (`MediaProjection`), started inside `HostService` with the media-projection foreground type. Android shows a system consent. That consent does not survive process death or a reboot: open PJDesk and allow the screen again.

Controlling another computer needs only the network permission.

Unattended access matches the Windows client: an optional permanent password of at least 8 characters, plus a choice to ask every time or accept with that password. Every password field has Show and Hide.

## Build

Requirements: Rust (stable), `cargo-ndk`, the Android SDK (platform 35, build-tools 35), NDK 27.2.12479018, and JDK 21.

`gradlew assembleRelease` does not compile `libpjdesk_mobile.so`. Build that library first or the release task fails. On Windows, follow [BUILD-WINDOWS.md](BUILD-WINDOWS.md).

```bash
./android/build.sh
```

The script writes:

* `dist/PJDesk-Android-<ver>-debug.apk`
* `dist/PJDesk-Android-<ver>.apk` when the four keystore variables below are set
* `dist/PJDesk-Android-<ver>-unsigned.apk` otherwise

`ANDROID_HOME` defaults to `$HOME/android-sdk`. `ANDROID_NDK_HOME` defaults to `$ANDROID_HOME/ndk/27.2.12479018`.

To sign the release APK, set these in the environment. Do not commit the keystore.

* `PJDESK_KEYSTORE` — path to the keystore file. The release key is `C:\Users\BenSpader\.pjdesk-android\pjdesk-release.jks`
* `PJDESK_KEYSTORE_PASSWORD`
* `PJDESK_KEY_ALIAS` — `pjdesk` for that keystore
* `PJDESK_KEY_PASSWORD`

All four have to be set. A partial set leaves the APK unsigned.

On Windows, sign an unsigned APK without saving the passwords:

```bat
android\sign-release.cmd dist\PJDesk-Android-0.6.12-unsigned.apk
```

The script uses `apksigner` from `ANDROID_HOME`. The release key is `C:\Users\BenSpader\.pjdesk-android\pjdesk-release.jks`, alias `pjdesk`. Those are the defaults when `PJDESK_KEYSTORE` and `PJDESK_KEY_ALIAS` are unset. The script asks for the passwords and does not save them. A different key will not update the phone.

The later release asset name is `PJDesk-Android-<ver>.apk`.

Unit tests that do not need a device:

```bash
cd android && ./gradlew testDebugUnitTest
cargo test -p pjdesk-proto -p pjdesk-link -p pjdesk-mobile
```

Coordinate mapping is tested in both Rust (`gesture.rs`) and Kotlin (`GestureMapTest`). The formulas match.
