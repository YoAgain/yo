import java.time.Instant
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}
// Yo-App "android/<build>": the build's UTC minute (YYYYMMDDHHMM) as iOS and the CLI; CI sets YO_BUILD, a local build
// takes the minute it is configured in.
val yoBuild = providers.environmentVariable("YO_BUILD").getOrElse(
    DateTimeFormatter.ofPattern("yyyyMMddHHmm").withZone(ZoneOffset.UTC).format(Instant.now()))
    .also { require(Regex("\\d{12}").matches(it)) { "YO_BUILD '$it' is not YYYYMMDDHHMM" } }
// Firebase normally gets these resources from the Google Services plugin. Emit them only when google-services.json
// (ignored: CI writes it from the secret GOOGLE_SERVICES_JSON, a dev copies it; README: Push) is present, so builds
// without Firebase use the same variant. Each variant takes the client of its own package: release app.justyo.android,
// debug app.justyo.android.dev.
val deviceSoak = providers.gradleProperty("yoDeviceSoak").map(String::toBoolean).getOrElse(false)
val firebaseConfig = layout.projectDirectory.file("google-services.json")
val firebaseJson = if (deviceSoak) null else providers.fileContents(firebaseConfig).asText.orNull
fun firebaseResources(applicationId: String): Map<String, String> {
    val data = groovy.json.JsonSlurper().parseText(firebaseJson ?: return emptyMap()) as Map<*, *>
    val project = data["project_info"] as Map<*, *>
    val client = (data["client"] as List<*>).filterIsInstance<Map<*, *>>().firstOrNull {
        val info = it["client_info"] as Map<*, *>
        (info["android_client_info"] as Map<*, *>)["package_name"] == applicationId
    } ?: error("google-services.json does not contain an Android client for $applicationId")
    val info = client["client_info"] as Map<*, *>
    val apiKey = (client["api_key"] as List<*>).first() as Map<*, *>
    return mapOf(
        "google_app_id" to info["mobilesdk_app_id"],
        "gcm_defaultSenderId" to project["project_number"],
        "project_id" to project["project_id"],
        "google_api_key" to apiKey["current_key"],
    ).mapValues { (_, value) -> requireNotNull(value).toString() }
}
android {
    namespace = "app.justyo.android"
    compileSdk = 37
    buildToolsVersion = "37.0.0"
    defaultConfig {
        applicationId = if (deviceSoak) "app.justyo.android.dev.soak" else "app.justyo.android.dev"
        manifestPlaceholders["yoAppLabel"] = if (deviceSoak) "Yo Device QA" else "Yo Dev"
        minSdk = 28
        targetSdk = 37
        versionCode = providers.gradleProperty("yoVersionCode").map(String::toInt).getOrElse(1)  // rising, from CI
        versionName = providers.gradleProperty("yoVersionName").getOrElse("0.1-dev")  // the release tag, from CI
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
        buildConfigField("String", "HUB", "\"https://dev.justyo.app\"")  // debug and the device QA build: the dev hub
        // Debug and QA builds say "android/dev<build>": not a Play build, so the admin never takes one as the newest.
        buildConfigField("String", "YO_APP", "\"android/dev$yoBuild\"")
    }
    buildFeatures { compose = true; buildConfig = true; resValues = true }
    signingConfigs {
        // The Play upload key (Play App Signing holds the app signing key): keystore.properties next to settings.gradle.kts
        // (ignored, never checked in) or, in CI, YO_UPLOAD_* from the environment `play`. Without either the release is unsigned.
        val local = rootProject.file("keystore.properties").takeIf { it.isFile }
            ?.let { file -> Properties().apply { file.inputStream().use { load(it) } } }
        val setting = { key: String -> local?.getProperty(key) ?: System.getenv("YO_UPLOAD_" + key.uppercase()) }
        if (setting("storeFile") != null) create("upload") {
            storeFile = file(setting("storeFile")!!)
            storePassword = setting("storePassword")
            keyAlias = setting("keyAlias")
            keyPassword = setting("keyPassword")
        }
    }
    buildTypes {
        release {
            optimization { enable = true }
            proguardFiles("proguard-rules.pro")
            signingConfig = signingConfigs.findByName("upload")
            // The store app talks to the live hub, as the iOS App Store and TestFlight build; Hub.kt has its pins.
            if (!deviceSoak) buildConfigField("String", "HUB", "\"https://hub.justyo.app\"")
            if (!deviceSoak) buildConfigField("String", "YO_APP", "\"android/$yoBuild\"")
        }
    }
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
    implementation("androidx.heifwriter:heifwriter:1.1.0")
    implementation("androidx.glance:glance-appwidget:1.1.1")
    implementation("androidx.work:work-runtime-ktx:2.10.5")
    implementation("com.google.firebase:firebase-messaging:24.1.2")
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
    implementation("com.google.android.gms:play-services-auth-blockstore:16.4.0")
    implementation("androidx.camera:camera-camera2:1.7.0-rc02")
    implementation("androidx.camera:camera-lifecycle:1.7.0-rc02")
    implementation("androidx.camera:camera-view:1.7.0-rc02")
    implementation("androidx.camera:camera-video:1.7.0-rc02")
    implementation("com.google.zxing:core:3.5.4")
    implementation("androidx.exifinterface:exifinterface:1.4.2")
    implementation("androidx.media3:media3-transformer:1.11.1")
    implementation("androidx.media3:media3-effect:1.11.1")
    testRuntimeOnly("net.java.dev.jna:jna:5.18.1@jar")
    testImplementation("junit:junit:4.13.2")
}
androidComponents {
    onVariants { variant ->
        // The store app (Google Play): its package can never change. Debug and the device QA build keep theirs.
        val store = variant.buildType == "release" && !deviceSoak
        if (store) {
            variant.applicationId.set("app.justyo.android")
            variant.manifestPlaceholders.put("yoAppLabel", "Yo")
        }
        firebaseResources(if (store) "app.justyo.android" else android.defaultConfig.applicationId!!).forEach { (key, value) ->
            variant.resValues.put(variant.makeResValueKey("string", key), com.android.build.api.variant.ResValue(value))
        }
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
