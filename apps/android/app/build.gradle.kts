plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

android {
    namespace = "com.digitaltracer.switchboard"
    compileSdk = 37

    defaultConfig {
        applicationId = "com.digitaltracer.switchboard"
        minSdk = 31
        targetSdk = 37
        // The hub compares this with its oldest supported build, so keep it a plain number.
        versionCode = 1
        versionName = "0.1"
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
