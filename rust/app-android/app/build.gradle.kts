import java.util.Properties

plugins {
    id("com.android.application")
}

// Release signing details live outside the repository.
val keys = Properties().apply {
    val file = rootProject.file("key.properties")
    if (file.exists()) file.inputStream().use { load(it) }
}

android {
    namespace = "com.canvasstudio.rust"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.canvasstudio.rust"
        minSdk = 24
        targetSdk = 36
        versionCode = 28
        versionName = "0.12.2"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    signingConfigs {
        if (keys.getProperty("storeFile") != null) {
            create("release") {
                storeFile = file(keys.getProperty("storeFile"))
                storePassword = keys.getProperty("storePassword")
                keyAlias = keys.getProperty("keyAlias")
                keyPassword = keys.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
            // ML Kit creates classes by reflection; shrinking breaks it.
            isMinifyEnabled = false
            isShrinkResources = false
        }
    }
}

dependencies {
    implementation("com.google.mlkit:digital-ink-recognition:19.0.0")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test:runner:1.7.0")
}
