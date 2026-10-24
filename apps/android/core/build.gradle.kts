// The app's logic and its API client, generated at build time from the hub's committed spec.
plugins {
    alias(libs.plugins.kotlin.jvm)
    alias(libs.plugins.kotlin.serialization)
    alias(libs.plugins.openapi.generator)
}

kotlin {
    jvmToolchain(17)
}

val apiOut = layout.buildDirectory.dir("generated/openapi")

openApiGenerate {
    generatorName = "kotlin"
    library.set("jvm-okhttp4")
    inputSpec = rootProject.file("../../api/openapi.json").absolutePath
    outputDir = apiOut.get().asFile.absolutePath
    packageName = "com.digitaltracer.switchboard.api"
    // The hub's spec has an empty license, which this validator alone rejects.
    skipValidateSpec = true
    configOptions = mapOf(
        "serializationLibrary" to "kotlinx_serialization",
        "enumUnknownDefaultCase" to "true",
        "useCoroutines" to "true",
        "omitGradleWrapper" to "true",
        "omitGradlePluginVersions" to "true",
    )
    globalProperties = mapOf("apis" to "", "models" to "", "supportingFiles" to "")
    generateApiTests = false
    generateModelTests = false
    generateApiDocumentation = false
    generateModelDocumentation = false
}

sourceSets.main {
    kotlin.srcDir(apiOut.map { it.dir("src/main/kotlin") })
}

tasks.named("compileKotlin") { dependsOn("openApiGenerate") }

dependencies {
    api(libs.coroutines.core)
    api(libs.serialization.json)
    api(libs.okhttp)

    testImplementation(platform(libs.junit.bom))
    testImplementation(libs.junit.jupiter)
    testImplementation(libs.kotlin.test)
    testImplementation(libs.coroutines.test)
    testImplementation(libs.okhttp.mockwebserver)
    testRuntimeOnly(libs.junit.launcher)
}

tasks.test {
    useJUnitPlatform()
    // The relay vector and the app-v1 fixtures are shared with the hub's Rust tests.
    systemProperty("switchboard.repo", rootProject.file("../..").absolutePath)
}
