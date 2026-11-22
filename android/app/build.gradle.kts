import java.io.File

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "dk.maarse.app"
    compileSdk = 35
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "dk.maarse.app"
        minSdk = 26
        targetSdk = 35
        versionCode = 5
        versionName = "0.5.0"
    }

    buildTypes {
        // Signed afterwards by tools/build_apk.py (release key or debug key).
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    buildFeatures { compose = true }
    packaging { jniLibs { useLegacyPackaging = false } }
}

// maarse-host as a native library (maarse-android crate), built with cargo-ndk.
val cargoNdk by tasks.registering(Exec::class) {
    val root = rootProject.projectDir.parentFile
    workingDir = root
    val ndk = System.getenv("ANDROID_NDK_HOME")
        ?: File(System.getenv("LOCALAPPDATA") ?: "", "Android/Sdk/ndk/27.2.12479018").path
    environment("ANDROID_NDK_HOME", ndk)
    commandLine(
        "cargo", "ndk", "-t", "arm64-v8a", "-t", "x86_64", "-P", "26",
        "-o", File(projectDir, "src/main/jniLibs").path,
        "build", "-p", "maarse-android", "--release",
    )
}
tasks.named("preBuild") { dependsOn(cargoNdk) }

// The language files (shared with the web client) go into the app as assets/i18n.
val copyLanguages by tasks.registering(Copy::class) {
    from(File(rootProject.projectDir.parentFile, "i18n")) { include("*.json") }
    into(layout.buildDirectory.dir("generated/i18n/i18n"))
}
android.sourceSets.getByName("main").assets.srcDir(layout.buildDirectory.dir("generated/i18n"))
tasks.named("preBuild") { dependsOn(copyLanguages) }

dependencies {
    val bom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(bom)
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    // QR codes: linking a device by scanning its code on the master device.
    implementation("com.google.zxing:core:3.5.3")
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    testImplementation("junit:junit:4.13.2")
}
