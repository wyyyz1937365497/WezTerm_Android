import java.io.File
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
}

val rustJniOutput = layout.buildDirectory.dir("generated/rustJniLibs/p0")
val rustJniOutputRelease = layout.buildDirectory.dir("generated/rustJniLibs/release")
val cargoExecutable = providers.environmentVariable("CARGO").orElse(
    providers.systemProperty("user.home").map { "$it/.cargo/bin/cargo" }
)
val localSdkProperties = Properties().apply {
    rootProject.file("local.properties").inputStream().use { load(it) }
}
val androidNdkHome = File(
    localSdkProperties.getProperty("sdk.dir"),
    "ndk/28.2.13676358",
)

android {
    namespace = "com.example.wezterm_android"
    compileSdk {
        version = release(37)
    }
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "com.example.wezterm_android"
        minSdk = 24
        targetSdk = 37
        versionCode = 7
        versionName = "0.2.4"

        ndk {
            abiFilters.add("arm64-v8a")
        }

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    buildTypes {
        release {
            optimization {
                enable = false
            }
            // Shipped as a debug-key signed developer build: keep run-as and
            // the provision-debug-identity.sh workflow working until a real
            // release signing / key import UI exists.
            isDebuggable = true
            signingConfig = signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }
    sourceSets {
        getByName("debug") {
            jniLibs.directories.add(rustJniOutput.get().asFile.absolutePath)
        }
        getByName("release") {
            jniLibs.directories.add(rustJniOutputRelease.get().asFile.absolutePath)
        }
    }
}

val buildRustAndroidRelease by tasks.registering(Exec::class) {
    group = "build"
    description = "Build the ARM64 Rust JNI library for the Android release APK"

    val rustRoot = rootProject.layout.projectDirectory.dir("rust")
    workingDir(rustRoot.asFile)
    inputs.file(rustRoot.file("Cargo.toml"))
    inputs.file(rustRoot.file("Cargo.lock"))
    inputs.file(rustRoot.file("wezterm-android-core/Cargo.toml"))
    inputs.dir(rustRoot.dir("wezterm-android-core/src"))
    inputs.file(rustRoot.file("wezterm-android-font/Cargo.toml"))
    inputs.dir(rustRoot.dir("wezterm-android-font/src"))
    inputs.dir(rustRoot.dir("wezterm-android-font/assets"))
    inputs.file(rustRoot.file("wezterm-android-mux/Cargo.toml"))
    inputs.dir(rustRoot.dir("wezterm-android-mux/src"))
    inputs.file(rustRoot.file("wezterm-android-ssh/Cargo.toml"))
    inputs.dir(rustRoot.dir("wezterm-android-ssh/src"))
    inputs.file(rustRoot.file("vendor/wezterm-ssh-android/Cargo.toml"))
    inputs.dir(rustRoot.dir("vendor/wezterm-ssh-android/src"))
    inputs.file(rustRoot.file("vendor/dirs-next-android/Cargo.toml"))
    inputs.dir(rustRoot.dir("vendor/dirs-next-android/src"))
    inputs.file(rustRoot.file("wezterm-android-native/Cargo.toml"))
    inputs.dir(rustRoot.dir("wezterm-android-native/src"))
    outputs.dir(rustJniOutputRelease)

    environment("ANDROID_NDK_HOME", androidNdkHome.absolutePath)
    commandLine(
        cargoExecutable.get(),
        "ndk",
        "-t", "arm64-v8a",
        "-P", "24",
        "-o", rustJniOutputRelease.get().asFile.absolutePath,
        "build",
        "--manifest-path", rustRoot.file("Cargo.toml").asFile.absolutePath,
        "--package", "wezterm-android-native",
        "--release",
    )
}

val buildRustAndroidDebug by tasks.registering(Exec::class) {
    group = "build"
    description = "Build the optimized ARM64 Rust JNI library for the Android debug APK"

    val rustRoot = rootProject.layout.projectDirectory.dir("rust")
    workingDir(rustRoot.asFile)
    inputs.file(rustRoot.file("Cargo.toml"))
    inputs.file(rustRoot.file("Cargo.lock"))
    inputs.file(rustRoot.file("wezterm-android-core/Cargo.toml"))
    inputs.dir(rustRoot.dir("wezterm-android-core/src"))
    inputs.file(rustRoot.file("wezterm-android-font/Cargo.toml"))
    inputs.dir(rustRoot.dir("wezterm-android-font/src"))
    inputs.dir(rustRoot.dir("wezterm-android-font/assets"))
    inputs.file(rustRoot.file("wezterm-android-mux/Cargo.toml"))
    inputs.dir(rustRoot.dir("wezterm-android-mux/src"))
    inputs.file(rustRoot.file("wezterm-android-ssh/Cargo.toml"))
    inputs.dir(rustRoot.dir("wezterm-android-ssh/src"))
    inputs.file(rustRoot.file("vendor/wezterm-ssh-android/Cargo.toml"))
    inputs.dir(rustRoot.dir("vendor/wezterm-ssh-android/src"))
    inputs.file(rustRoot.file("wezterm-android-native/Cargo.toml"))
    inputs.dir(rustRoot.dir("wezterm-android-native/src"))
    outputs.dir(rustJniOutput)

    environment("ANDROID_NDK_HOME", androidNdkHome.absolutePath)
    commandLine(
        cargoExecutable.get(),
        "ndk",
        "-t", "arm64-v8a",
        "-P", "24",
        "-o", rustJniOutput.get().asFile.absolutePath,
        "build",
        "--manifest-path", rustRoot.file("Cargo.toml").asFile.absolutePath,
        "--package", "wezterm-android-native",
        "--release",
    )
}

tasks.matching { it.name == "mergeDebugJniLibFolders" }.configureEach {
    dependsOn(buildRustAndroidDebug)
}

tasks.matching { it.name == "mergeReleaseJniLibFolders" }.configureEach {
    dependsOn(buildRustAndroidRelease)
}

dependencies {
    implementation(libs.androidx.activity)
    implementation(libs.androidx.appcompat)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.material)
    testImplementation(libs.junit)
    androidTestImplementation(libs.androidx.espresso.core)
    androidTestImplementation(libs.androidx.junit)
}
