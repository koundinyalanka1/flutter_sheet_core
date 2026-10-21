// Packages the prebuilt Rust library (src/main/jniLibs/<abi>/libsheet_ffi.so)
// with the app. There is no Java or Kotlin code: Dart calls the library
// directly through dart:ffi.
group = "com.yourmateapps.flutter_sheet_core"
version = "1.0"

buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:9.0.1")
    }
}

allprojects {
    repositories {
        google()
        mavenCentral()
    }
}

plugins {
    id("com.android.library")
}

android {
    namespace = "com.yourmateapps.flutter_sheet_core"
    compileSdk = 36

    defaultConfig {
        minSdk = 24
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
