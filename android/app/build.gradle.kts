plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "us.spader.pjdesk"
    compileSdk = 35
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "us.spader.pjdesk"
        minSdk = 26
        targetSdk = 35
        versionCode = 61900
        versionName = "0.6.19"
        ndk {
            abiFilters += listOf("armeabi-v7a", "arm64-v8a", "x86", "x86_64")
        }
    }

    // Release signing uses the environment only. On Ben's PC the keystore is
    // C:\Users\BenSpader\.pjdesk-android\pjdesk-release.jks and the alias is pjdesk.
    // android\sign-release.cmd signs an unsigned APK without writing the passwords down.
    // Set PJDESK_KEYSTORE, PJDESK_KEYSTORE_PASSWORD, PJDESK_KEY_ALIAS, and PJDESK_KEY_PASSWORD.
    signingConfigs {
        val store = System.getenv("PJDESK_KEYSTORE")
        val storePassword = System.getenv("PJDESK_KEYSTORE_PASSWORD")
        val alias = System.getenv("PJDESK_KEY_ALIAS")
        val keyPassword = System.getenv("PJDESK_KEY_PASSWORD")
        if (!store.isNullOrBlank() && !storePassword.isNullOrBlank() && !alias.isNullOrBlank() && !keyPassword.isNullOrBlank()) {
            create("release") {
                storeFile = file(store)
                this.storePassword = storePassword
                keyAlias = alias
                this.keyPassword = keyPassword
            }
        }
    }

    buildTypes {
        release {
            // Minify stays off. The rules still keep Log.w and Log.e if it is turned on,
            // because R8's default file deletes Log.i and a release device drops INFO.
            isMinifyEnabled = false
            isShrinkResources = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.findByName("release")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
    }

    testOptions {
        unitTests.isIncludeAndroidResources = true
        unitTests.isReturnDefaultValues = true
    }

    packaging {
        resources.excludes += setOf("META-INF/AL2.0", "META-INF/LGPL2.1")
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-core")
    implementation("androidx.datastore:datastore-preferences:1.1.1")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.fragment:fragment-ktx:1.8.5")
    implementation("androidx.biometric:biometric:1.1.0")
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    implementation("androidx.camera:camera-camera2:1.4.1")
    implementation("androidx.camera:camera-lifecycle:1.4.1")
    implementation("androidx.camera:camera-view:1.4.1")
    implementation("com.google.zxing:core:3.5.3")
    implementation("org.bouncycastle:bcprov-jdk18on:1.79")
    debugImplementation("androidx.compose.ui:ui-tooling")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.14.1")
    testImplementation(composeBom)
    testImplementation("androidx.compose.ui:ui-test-junit4")
    testImplementation("androidx.compose.ui:ui-test-manifest")
    testImplementation("androidx.test:core:1.6.1")
}

// assembleRelease does not compile libpjdesk_mobile.so. cargo-ndk writes it
// into jniLibs first. A release task fails when that library is missing.
val nativeAbis = listOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86")

tasks.register("requireNativeRelease") {
    group = "build"
    description = "Fail the release build when libpjdesk_mobile.so was not built with cargo-ndk."
    doLast {
        val missing = nativeAbis.filter { abi ->
            !file("src/main/jniLibs/$abi/libpjdesk_mobile.so").isFile
        }
        if (missing.isNotEmpty()) {
            val lines = missing.joinToString("\n") { abi -> "  src/main/jniLibs/$abi/libpjdesk_mobile.so" }
            throw GradleException(
                "Engine library missing from this build.\n" +
                    "assembleRelease does not compile libpjdesk_mobile.so. cargo-ndk has to run first.\n" +
                    "Missing:\n" +
                    lines + "\n" +
                    "Windows steps: android/BUILD-WINDOWS.md",
            )
        }
    }
}

afterEvaluate {
    listOf(
        "compileReleaseKotlin",
        "mergeReleaseNativeLibs",
        "packageRelease",
        "assembleRelease",
        "bundleRelease",
    ).forEach { name ->
        tasks.findByName(name)?.dependsOn("requireNativeRelease")
    }
}
