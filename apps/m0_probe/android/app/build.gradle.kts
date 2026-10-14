plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "com.radishledger.radishledger_probe"
    compileSdk = flutter.compileSdkVersion
    buildToolsVersion = "36.1.0"
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        applicationId = "com.radishledger.radishledger_probe"
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    signingConfigs.getByName("debug") {
        // 本项目隔离测试签名；不能读取用户的正式签名或修改全局 debug.keystore。
        storeFile = rootProject.file("../../../local-data/signing/m0-probe-debug.keystore")
    }

    buildTypes {
        release {
            // M0 真机目标构建使用隔离测试签名，不是可发布的官方包。
            signingConfig = signingConfigs.getByName("debug")
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}
