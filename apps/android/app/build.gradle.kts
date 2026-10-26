import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

// The upload key lives outside any repo, with mode 0600: a properties file naming the keystore,
// its alias and a file holding its password.
val uploadKey = file(
    providers.gradleProperty("switchboard.uploadKey")
        .getOrElse("${System.getProperty("user.home")}/.config/switchboard/android-upload.properties"),
)

android {
    namespace = "com.digitaltracer.switchboard"
    compileSdk = 37

    defaultConfig {
        applicationId = "com.digitaltracer.switchboard"
        minSdk = 31
        targetSdk = 37
        // The hub compares this with its oldest supported build, so keep it a plain number.
        // Each Play upload needs a higher one: `-Pswitchboard.build=N`.
        versionCode = providers.gradleProperty("switchboard.build").getOrElse("1").toInt()
        versionName = "1.0"
    }

    signingConfigs {
        if (uploadKey.exists()) {
            create("upload") {
                val p = Properties().apply { uploadKey.inputStream().use(::load) }
                val password = file(p.getProperty("passwordFile")).readText().trim()
                storeFile = file(p.getProperty("storeFile"))
                storePassword = password
                keyAlias = p.getProperty("alias")
                keyPassword = password
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.findByName("upload")
        }
    }

    buildFeatures {
        compose = true
    }
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation(project(":core"))
    implementation(libs.core.ktx)
    implementation(libs.activity.compose)
    implementation(libs.code.scanner)
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.material3)
    implementation(libs.compose.icons)
    implementation(libs.compose.ui.tooling.preview)
    debugImplementation(libs.compose.ui.tooling)
}
