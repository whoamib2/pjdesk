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
3. **Screen capture** (`MediaProjection`), started inside `HostService` with the media-projection foreground type. Android shows a system consent. Android 14 and later give each consent one capture, so every session asks again after sharing stops. The foreground service stays up and switches to the media-projection type before `getMediaProjection`.

Controlling another computer needs only the network permission.

Unattended access has two switches under Settings → Security. **Use the permanent password** skips PJDesk's Allow prompt for a password that matches. **Unattended access (accept automatically)** is off by default. Turning it on needs a permanent password, the accessibility service, and a PIN or fingerprint. When it is on, a session that authenticated with the permanent password, or a trusted account or computer, also skips Allow and Deny. PJDesk then opens Android's screen-share dialog and the accessibility service taps it.

The tap uses SystemUI resource ids first (`screen_share_mode_options`, `screen_share_mode_spinner`, `android:id/button1`), then text. The English labels are Entire screen, Share entire screen, A single app, Start, Share screen, and Start now. A few other locales are recognized, and a two-row list taps the row that is not "a single app". The service is marked `isAccessibilityTool` so Android 12 and later will let it read a dialog marked accessibility-data-sensitive. It does not use root, a device owner, or an adb grant.

The sharing notification stays visible. Each auto-accept is written to the PJDesk log, and the phone's connection notice says "Accepted automatically". If the tap does not land within about twelve seconds, the normal "Allow screen sharing" prompt is what the person sees, and the PC already has "Waiting for the phone to allow screen sharing".

What was verified: the choice of which control to tap, in `ProjectionConsentTest`, including the Android 14 spinner, the Share entire screen row, Start now, Share screen, a Spanish "Pantalla completa" row, and an unknown-language row next to "A single app". Those tests do not drive a phone. This environment has no Pixel and no Android 17 image. `compileSdk` and `targetSdk` are 35 (Android 15). Android 10 through 13 are the older Start now button with no spinner; the same code clicks that button, and that path was not run on a device either. If a later Android release hides this dialog from accessibility tools, the click fails and the normal prompt remains. That limit is accessibility-only. It is not worked around with root, a device owner, or adb.

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
android\sign-release.cmd dist\PJDesk-Android-0.6.17-unsigned.apk
```

The script uses `apksigner` from `ANDROID_HOME`. The release key is `C:\Users\BenSpader\.pjdesk-android\pjdesk-release.jks`, alias `pjdesk`. Those are the defaults when `PJDESK_KEYSTORE` and `PJDESK_KEY_ALIAS` are unset. The script asks for the passwords and does not save them. A different key will not update the phone.

The later release asset name is `PJDesk-Android-<ver>.apk`.

Unit tests that do not need a device:

```bash
cd android && ./gradlew testDebugUnitTest
cargo test -p pjdesk-proto -p pjdesk-link -p pjdesk-mobile
```

Coordinate mapping is tested in both Rust (`gesture.rs`) and Kotlin (`GestureMapTest`). The formulas match.
