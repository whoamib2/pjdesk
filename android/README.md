# PJDesk for Android

The Android app is both a host and a controller. It speaks the same protocol as the Windows client. A fresh install has no server until you sign in with a PJDesk account; the account supplies the private server address and key. The Rust core is `crates/pjdesk-mobile` (`libpjdesk_mobile`), built with cargo-ndk and called through JNI. The UI is Kotlin and Jetpack Compose.

## Codec

Video is **H.264 Annex-B**, the access-unit format already used by the Windows client.

* The Windows host encodes with OpenH264 and the Windows viewer decodes with OpenH264 (`crates/pjdesk/src/viewer/decoder.rs`). No second decoder was added.
* The Android host encodes with MediaCodec `video/avc` on a `MediaProjection` surface. Constrained Baseline level 5 is requested when the device accepts it. Level 3.1 is only 720p, and a phone picture is taller than that. Codec-config and length-prefixed buffers are converted to Annex-B. On Android 10 and later the encoder is also asked to prepend SPS and PPS to each sync frame (`KEY_PREPEND_HEADER_TO_SYNC_FRAMES`). PJDesk repeats them on every IDR, including an IDR the codec did not mark as a keyframe (`AnnexB.kt`). An IDR waiting to be sent is not replaced by a later delta. A decoder error on Windows already asks for a new IDR.
* The Android controller decodes those same Annex-B access units with MediaCodec onto a `Surface`.

Flow control is unchanged: at most three frames are in flight, and the controller sends `FrameAck`.

## Permissions

The user grants these before this phone can be controlled:

1. **Notifications**, so the foreground service can show a persistent notification. While a session is active the notification says the phone is being controlled and has a one-tap **Stop**.
2. **Accessibility** (`PjAccessibilityService`). Remote taps, swipes, long-press, and pinch use `dispatchGesture`. Back, Home, and Recents are global actions. Typed text uses `ACTION_SET_TEXT` on the focused field. Volume uses `AudioManager`. Power locks the screen on Android 9+ and opens the power dialog on older releases. The service does not turn on touch exploration.
3. **Screen capture** (`MediaProjection`), started inside `HostService` with the media-projection foreground type. Android shows a system consent. Android 14 and later give each consent one capture, so every session asks again after sharing stops. The foreground service stays up and switches to the media-projection type before `getMediaProjection`. Every session shares the whole display. On Android 14 and later the intent is `MediaProjectionConfig.createConfigForDefaultDisplay()`, which limits the system dialog to that display. A capture whose region is smaller than the physical display, or hidden while the screen is on, is stopped and the consent is requested again. The PC keeps "Waiting for the phone to allow screen sharing" until the whole display is shared. An edge-to-edge single app that is the same pixel size as the display and is currently visible is not distinguishable from the whole display with the public callbacks. The config is what keeps that choice off the dialog.

Controlling another computer needs only the network permission.

Unattended access has two controls under Settings → Security. **Use the permanent password** still skips PJDesk's Allow prompt for a password that matches. **Unattended access (accept automatically)** is what taps Android's screen-share dialog. It is off by default. Turning it on needs a permanent password, the accessibility service, and a PIN or fingerprint. The first launch of 0.6.18 copies an existing "Use the permanent password" choice onto that switch, once. Opening the app does not write the switch off when accessibility is not bound yet. A session start checks the password and the accessibility service. If either is missing, the normal screen-share prompt is shown and the switch stays on. When the switch is on, a matching permanent password, a trusted account or computer, or View phone from another device signed into this phone's own account skips Allow and Deny, and PJDesk taps the system dialog. A preapproved view is always that auto-accept.

The tap uses SystemUI resource ids first (`screen_share_mode_options`, `screen_share_mode_spinner`, `android:id/button1`), then text. The English labels are Entire screen, Share entire screen, A single app, Start, Share screen, and Start now. A few other locales are recognized, and a two-row list taps the row that is not "a single app". The spinner's own text is the selected mode. Start is not tapped while that text is a single app, even if a child also says Entire screen. The service is marked `isAccessibilityTool` so Android 12 and later will let it read a dialog marked accessibility-data-sensitive. It does not use root, a device owner, or an adb grant.

The sharing notification stays visible. The host service stays in the foreground during a capture and holds a partial wake lock, so the screen turning off does not freeze the process. Each auto-accept is written to the PJDesk log, including in a release build, and the phone's connection notice says "Accepted automatically". Release logcat gets those lines as warnings, because a release device drops info. Share logs also records the spinner text and the Share screen button when the dialog is on screen. The tap runs on a watch that outlives the activity and a service rebind. A button the dialog will not click is tapped at its center. A locked phone is woken and the consent is shown over the keyguard; the dialog is not tapped while the lock screen is up. If the tap does not land within about 50 seconds, the normal "Allow screen sharing" prompt is what the person sees, and the PC already has "Waiting for the phone to allow screen sharing".

What was verified: the choice of which control to tap, in `ProjectionConsentTest`, including the Android 14 spinner, the Share entire screen row, Start now, Share screen, a Spanish "Pantalla completa" row, an unknown-language row next to "A single app", a spinner whose child text mentions Entire screen while the selected text is still A single app, and the Pixel Android 17 dialog (systemui `screen_share_mode_options` whose child text is "Share entire screen", then `android:id/button1` "Share screen"). That dialog was dumped from Ben's Pixel during the 0.6.17 field test and matches `decide()`. `EntireDisplayTest` checks that a full 1080×2400 region is kept, in either orientation, and that a window short by a status bar is rejected. `AnnexBTest` checks that an IDR repeats SPS and PPS. `ConsentWatchTest` checks that a locked phone is not tapped and that a missing accessibility service keeps the watch alive. `ControlGateTest` checks that a short control drop is not announced. Those tests do not drive a phone. This environment has no Pixel, no emulator image, and no Android 17 image, so 0.6.19 was not run on the device. The 0.6.18 field test on Ben's Pixel opened the dialog and did not tap it. `compileSdk` and `targetSdk` are 35 (Android 15). Android 10 through 13 have no single-app chooser and no `createConfigForDefaultDisplay`; they use the older Start now button. Android 14, 15, 16, and 17 were not run on a device from this machine. If a later Android release hides this dialog from accessibility tools, the click fails and the normal prompt remains. `createConfigForDefaultDisplay` still limits that prompt to the whole display on Android 14 and later. That limit is accessibility-only. It is not worked around with root, a device owner, or adb.

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
android\sign-release.cmd dist\PJDesk-Android-0.6.19-unsigned.apk
```

The script uses `apksigner` from `ANDROID_HOME`. The release key is `C:\Users\BenSpader\.pjdesk-android\pjdesk-release.jks`, alias `pjdesk`. Those are the defaults when `PJDESK_KEYSTORE` and `PJDESK_KEY_ALIAS` are unset. The script asks for the passwords and does not save them. A different key will not update the phone.

The later release asset name is `PJDesk-Android-<ver>.apk`.

Unit tests that do not need a device:

```bash
cd android && ./gradlew testDebugUnitTest
cargo test -p pjdesk-proto -p pjdesk-link -p pjdesk-mobile
```

Coordinate mapping is tested in both Rust (`gesture.rs`) and Kotlin (`GestureMapTest`). The formulas match.
