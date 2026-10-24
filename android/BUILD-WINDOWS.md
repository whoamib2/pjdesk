# Building the PJDesk Android release on Windows

`gradlew assembleRelease` does not compile `libpjdesk_mobile.so`. The 0.6.10 and 0.6.11 APKs were built that way, so the phone opened and the Saved list worked, and Connect stayed on "PJDesk is still starting." A release build now fails if that library is missing. Build it with cargo-ndk first, then run Gradle.

The phone needs the arm64 library:

`android\app\src\main\jniLibs\arm64-v8a\libpjdesk_mobile.so`

The release build also requires the other ABIs listed below, because the APK says it supports them.

## Tools

- JDK 21. `java -version` should report 21.
- Android SDK with platform 35 and build-tools 35.
- Android NDK **27.2.12479018**.
- Rust stable, installed with rustup.
- cargo-ndk.

## One-time setup

From a command prompt:

```bat
rustup default stable
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
cargo install cargo-ndk
```

Point the build at JDK 21, the SDK, and that exact NDK. Change the SDK path if Android Studio installed it somewhere else.

```bat
set JAVA_HOME=C:\Program Files\Java\jdk-21
set ANDROID_HOME=%LOCALAPPDATA%\Android\Sdk
set ANDROID_SDK_ROOT=%ANDROID_HOME%
set ANDROID_NDK_HOME=%ANDROID_HOME%\ndk\27.2.12479018
```

Confirm the NDK is the 27.2.12479018 folder:

```bat
dir "%ANDROID_NDK_HOME%\toolchains\llvm\prebuilt"
```

`dir` should list a `windows-x86_64` directory. If the folder is missing, install NDK 27.2.12479018 from the Android SDK Manager.

## Every release

From the repository root, with those variables set:

```bat
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 --platform 26 -o android\app\src\main\jniLibs build --release -p pjdesk-mobile
```

That writes:

- `android\app\src\main\jniLibs\arm64-v8a\libpjdesk_mobile.so`
- `android\app\src\main\jniLibs\armeabi-v7a\libpjdesk_mobile.so`
- `android\app\src\main\jniLibs\x86_64\libpjdesk_mobile.so`
- `android\app\src\main\jniLibs\x86\libpjdesk_mobile.so`

Then:

```bat
cd android
gradlew.bat assembleRelease
```

Sign with the existing release key. The script asks for the passwords and does not save them.

```bat
android\sign-release.cmd app\build\outputs\apk\release\app-release-unsigned.apk
```

The default keystore is `C:\Users\BenSpader\.pjdesk-android\pjdesk-release.jks`, alias `pjdesk`. A different key will not update the phone.

Do not commit `jniLibs`, `local.properties`, or the keystore. `android\build.sh` is the Linux script. On Windows, use the cargo-ndk command above and then `gradlew.bat assembleRelease`.
