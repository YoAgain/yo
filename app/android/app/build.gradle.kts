plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}
android {
    namespace = "app.justyo.android"
    compileSdk = 37
    buildToolsVersion = "37.0.0"
    defaultConfig {
        applicationId = "app.justyo.android.dev"
        minSdk = 28
        targetSdk = 37
        versionCode = 1
        versionName = "0.1-dev"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
        buildConfigField("String", "HUB", "\"https://dev.justyo.app\"")
        buildConfigField("String", "YO_APP", "\"android/202611160001\"")
    }
    buildFeatures { compose = true; buildConfig = true }
    buildTypes { release { optimization { enable = true } } }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_21; targetCompatibility = JavaVersion.VERSION_21 }
    sourceSets["main"].java.directories.add("build/generated/yo")
    sourceSets["main"].jniLibs.directories.add("build/rustJniLibs")
    packaging { jniLibs { useLegacyPackaging = true } }
    lint { warningsAsErrors = true; abortOnError = true }
}
val buildCore = tasks.register<Exec>("buildCore") {
    workingDir(rootDir)
    commandLine("bash", "dev/core.sh")
    inputs.files(fileTree(rootProject.file("../../core")), rootProject.file("../../Cargo.lock"), rootProject.file("../../Cargo.toml"), rootProject.file("dev/core.sh"), rootProject.file("dev/uniffi.toml"), rootProject.file("dev/fix-bindings.py"))
    outputs.dirs(file("build/generated/yo"), file("build/rustJniLibs"))
}
tasks.named("preBuild") { dependsOn(buildCore) }
dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.activity:activity-compose:1.12.4")
    implementation("androidx.compose.material3:material3:1.5.0-rc01")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.10.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    implementation("net.java.dev.jna:jna:5.18.1@aar")
    implementation("com.squareup.okhttp3:okhttp:5.3.2")
    implementation("androidx.credentials:credentials:1.6.0")
    implementation("androidx.credentials:credentials-play-services-auth:1.6.0")
    implementation("androidx.camera:camera-camera2:1.5.3")
    implementation("androidx.camera:camera-lifecycle:1.5.3")
    implementation("androidx.camera:camera-view:1.5.3")
    implementation("androidx.camera:camera-video:1.5.3")
    implementation("com.google.zxing:core:3.5.4")
    implementation("androidx.exifinterface:exifinterface:1.4.2")
    implementation("androidx.media3:media3-transformer:1.11.1")
    implementation("androidx.media3:media3-effect:1.11.1")
    testRuntimeOnly("net.java.dev.jna:jna:5.18.1@jar")
    testImplementation("junit:junit:4.13.2")
}
androidComponents {
    onVariants { variant ->
        variant.sources.kotlin?.addStaticSourceDirectory("build/generated/yo")
    }
}
tasks.withType<Test>().configureEach {
    systemProperty("jna.library.path", rootProject.file("../../target/android/debug").absolutePath)
    val smoke = providers.environmentVariable("YO_DEV_SMOKE").orElse("0")
    inputs.property("devHubSmoke", smoke)
    environment("YO_DEV_SMOKE", smoke.get())
}
val licenseAssets = tasks.register<Exec>("licenseAssets") {
    workingDir(rootDir)
    commandLine("bash", "dev/licenses.sh")
    inputs.files(rootProject.file("../../Cargo.lock"), rootProject.file("dev/licenses.sh"), rootProject.file("dev/rust-licenses.py"))
    outputs.dir(file("build/license-assets"))
}
android.sourceSets["main"].assets.directories.add("build/license-assets")
tasks.named("preBuild") { dependsOn(licenseAssets) }

// Package the actual resolved runtime inventory, inherited POM licenses and embedded notices.
val runtimeCoordinates = providers.provider {
    configurations.getByName("releaseRuntimeClasspath").incoming.resolutionResult.allComponents
        .mapNotNull { it.id as? org.gradle.api.artifacts.component.ModuleComponentIdentifier }
        .map { "${it.group}:${it.module}:${it.version}" }.distinct().sorted()
}
val mavenLicenseAssets = tasks.register<Exec>("mavenLicenseAssets") {
    val coordinateFile = layout.buildDirectory.file("maven-runtime-coordinates.txt")
    inputs.property("coordinates", runtimeCoordinates)
    inputs.file(rootProject.file("dev/maven-licenses.py"))
    outputs.dir(layout.buildDirectory.dir("maven-license-assets"))
    doFirst {
        coordinateFile.get().asFile.apply { parentFile.mkdirs();writeText(runtimeCoordinates.get().joinToString("\n")) }
    }
    commandLine("python3", rootProject.file("dev/maven-licenses.py"), gradle.gradleUserHomeDir,
        coordinateFile.get().asFile, layout.buildDirectory.file("maven-license-assets/Maven-Licenses.txt").get().asFile)
}
android.sourceSets["main"].assets.directories.add("build/maven-license-assets")
tasks.named("preBuild") { dependsOn(mavenLicenseAssets) }
