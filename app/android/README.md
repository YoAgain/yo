# Yo for Android

Kotlin, Jetpack Compose and Material 3 over the existing Rust core. One activity, an Android ViewModel, StateFlow,
coroutines and CameraX. API 34 (Android 14) minimum; compile/target API 37 (Android 17). Dynamic color is off. The list uses
Yo’s original Flat UI colors, 90 dp rows, 30 sp Montserrat Bold, Amethyst, and the bundled Yo sound.

This is a development implementation, **not complete iOS parity**. [PARITY.md](PARITY.md) audits each iOS source file
and records the remaining refinements. No server or Rust source changes. No FCM or passkey capability is pretended.
This package is `app.justyo.android.dev`, permanently pointed at `https://dev.justyo.app`; the Play build talks to
`https://hub.justyo.app` (below).

## Toolchain, without sudo

On Linux x86_64, with curl, unzip, tar, Python 3, a host C/C++ toolchain, protoc and the repository’s Rust toolchain:

```sh
app/android/dev/bootstrap.sh
```

The script contains the exact installation steps and SHA-256 checks for the JDK and command-line tools. This run
installed Temurin **25.0.5.1+2**, command-line tools **16111833**, SDK platform **android-37.2**,
build-tools **37.0.0**, NDK **30.0.16248370**, platform-tools and the emulator. Rust **1.100.0** comes from the root
`rust-toolchain.toml`; targets are `aarch64-linux-android` and `x86_64-linux-android`, cargo-ndk **4.1.2**.
All Android tools go under `~/Android`. Gradle **9.9.2** is downloaded by the checked-in wrapper, with a pinned
checksum, into `~/.gradle`. AGP **9.5.2** and Kotlin Compose compiler **2.4.21** are pinned.

`dev/env.sh` sets `JAVA_HOME`, `ANDROID_HOME`, `ANDROID_USER_HOME`, `ANDROID_AVD_HOME`, `GRADLE_USER_HOME` and PATH.
The Android SDK’s default user directory may otherwise be outside a restricted workspace. No shell profile changes
are required. Android Studio can also open this directory with the same JDK and SDK.

## Build, verify, install

From the repository root:

```sh
export CARGO_BUILD_JOBS=4 PATH=$HOME/.cargo/bin:$PATH
. app/android/dev/env.sh
cd app/android
nice -n 10 ./gradlew --max-workers=4 assembleDebug assembleRelease testDebugUnitTest lint
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n app.justyo.android.dev/app.justyo.android.MainActivity
```

`./gradlew assembleDebug` builds both Rust `.so`s, generates Kotlin bindings, and packages them with JNA’s Android
AAR. First builds are much slower than subsequent ones. `CARGO_BUILD_JOBS` defaults to 4 to bound libsignal’s build
memory. No native binaries or generated bindings are checked in. `dev/core.sh` may also be run by itself.

The offline JVM tests load the actual host Rust library, built by the same task: the binding's typed callback errors,
bar titles from the core, UTF-8 limits and crash-frame sanitizing. The core's own rules are tested in Rust.
There are no live-account mutations in unit tests. Lint treats warnings as errors; only dependency-update advisory
checks are disabled because dependency versions are deliberately pinned. Runtime/API and accessibility checks run.

UniFFI 0.32.2 generates Kotlin names that collide with `AutoCloseable.close` and `Throwable.message` in this API.
`dev/fix-bindings.py` checks the expected generator output and renames only Kotlin identifiers: the exported core
method becomes `shutdown`, the three error payload fields become `detail`. Rust symbols, serialization and APIs
are unchanged. Generation uses Java’s Cleaner (API 33+).

## Device benchmarks

`dev/bench.py` measures the QA package on a USB phone against the DEV hub (never the live one): cold start, a peer's
Yo to the bar's frame (receive), a tap to the "Sent" frame and the peer's receipt (send), and main-thread frames over
the 120 Hz budget while Yos come and go (Perfetto, `--trace`, needs the Python `perfetto` package). The app writes its
latency marks (logcat tag `YoLat`, `Lat.kt`) only when launched with `--ez yo_perf true`. Run it before and after a
change on the same phone and network; the JSON goes to stdout.

## State, identity, recovery

`CoreState` opens the core's SQLite database (`yo-state-v3.sqlite`) in `noBackupFilesDir`. There is one migration:
an older install's state files are deleted at launch (signed out), and a recovery key it had saved signs it in again. A failed save
stops any send or acknowledgement that depended on it. All blocking core/network/disk calls run on `Dispatchers.IO`; a long poll is separate
from sending. Requests have bounded answers (11 MiB), timeouts, no cookies/cache, no redirects, and independent
OkHttp pools for identified and anonymous traffic. Hub keys, sender-certificate roots and group parameters are the
public dev pins from iOS `Hub.swift`. HTTP 426 stops networking and replaces the UI with the update screen.

The iOS keychain holds an exportable random 32-byte state-sealing key. Android Keystore keys cannot be exported:
Android stores a nonexportable AES-256-GCM wrapping key there, wrapping the same kind of random 32-byte Rust state
key in the private vault. It also encrypts UI metadata, recovery/registration secrets and media/manifests, with a
fresh nonce and the item name as AAD. Keystore failures or damaged files propagate; they never silently create an
empty state. The raw state key is in memory only while the core uses it. Android backup and device transfer are
excluded. There is one client/process; share intents use it, so no extension or second state writer exists.

A name-only registration works without passkeys. A retry keeps its staged identity secret. Recovery-key backups
use the **iOS-compatible JSON** inside the core’s encrypted record (base64 Data fields, nested account secret,
backup version 3). The 64-character Signal recovery key restores the account on either platform. Text/PNG-card export uses the
system Create Document flow; the backup is marked secured only after writing the document and uploading the
backup. New keys are staged before changing the hub record. Do not sign out of an unsecured account you need.
As iOS keeps its account item in the iCloud Keychain, Android keeps the same item (the core's `keychain` format) in
Google Block Store: a new phone with the same Google account signs in by itself before onboarding. The cloud copy is
made only while `isEndToEndEncryptionAvailable()` (a screen lock and Google's device backup turned on; spec 7, "Key
material only in an end-to-end encrypted cloud"); otherwise the item stays for a device-to-device transfer, the menu
shows a red dot and Account Recovery's "Back Up to Google" is off, saying which of the two is missing (the keyguard
tells them apart). Coming back to the app retries. Signing out marks the item (`signedOut`), deleting the account deletes it; a mark or
deletion Block Store did not take is kept in the vault and blocks the automatic sign-in until it is made. A phone
replaced by another one notes that account (`left`) and never signs it in again by itself. An old install's sign-in
is kept in the vault before its old state goes and is removed only once the hub answered.

The media queue persists encrypted blobs and manifests before upload; a media Yo waits for its upload and the
2-second undo window. A restart discards still-held items. Per-target order is maintained, and temporary failures
retry. Incoming media descriptors are saved before acknowledging their core events. Downloads stay encrypted
until opened, are marked consumed before display, and are deleted afterward; a restart also discards consumed
items. Camera/library photos are resized to 1600 px and encoded as metadata-free JPEGs. Videos use CameraX and
Media3: camera recordings stop at 10 seconds; shared or picked videos may be 10 seconds (a longer one is refused,
Android has no system trimmer), with an output byte-size check (9 MB). See PARITY for remaining stress/crash tests.

## Device acceptance

From `app/android`, use the API 37 `YoAndroid` AVD for headless UI checks on hosts with `/dev/kvm`.

```sh
source dev/env.sh
nice -n 10 "$ANDROID_HOME/emulator/emulator" -avd YoAndroid -no-window -no-audio \
  -gpu swiftshader_indirect -no-snapshot -memory 3072 -cores 2 &
adb wait-for-device
```

Wait for `adb shell getprop sys.boot_completed` to return `1` before installing the APK.
Use a physical Android device for camera hardware, sound, and credential-provider checks.
Keep captures synthetic and free of recovery keys.

## Push

FCM through the Firebase project **Yo Push** (`yo-push-97767`), with one Android client per package:
`app.justyo.android` (release, Google Play) and `app.justyo.android.dev` (debug). Its `google-services.json` is never
checked in (`app/google-services.json` is ignored): `play.yml` writes it from the secret `GOOGLE_SERVICES_JSON` of the
environment `play` and fails without it; on a dev machine copy it from the secret store
(`cp ~/.config/yo-secrets/google-services.json app/google-services.json`). `app/build.gradle.kts` turns it into the
Firebase resources of each variant's own client. Without the file (pull-request CI, the device QA build) the app
has no Firebase: it polls in the foreground only and Status says No Token.

The app asks for notifications once when the friend list first shows, then fetches the FCM token and
registers it with the hub as `PUSH_FCM` after each sign-in and restore and on `onNewToken`; Settings › Status ›
Renew Push Registration deletes the token and registers a new one. The hub (`YO_FCM_SERVICE_ACCOUNT_FILE`) sends
data-only messages (`yo=1`, `urgent`, Android priority HIGH/NORMAL) that `YoMessagingService` turns into a sync.

## Platform work still needed

Android App Links and passkeys need
`assetlinks.json` on the relevant domains with the package/signing certificate. `Passkeys.kt` contains a gated
Credential Manager PRF assertion path and explicit TODO; it cannot secure or restore an account yet. `yo://NAME`
works; HTTPS invites are App Links (autoVerify) once justyo.co's assetlinks.json names the release certificate. Shares use ACTION_SEND/ACTION_SEND_MULTIPLE
(up to ten images or one video) in this activity. See PARITY for all remaining gaps and validation limits.

## Live dev-hub smoke test

Opt in explicitly; this creates a synthetic account on the dev hub, checks registration, sealed-state reopening
and recovery backup round-trip, and deletes that account in `finally`:

```sh
. app/android/dev/env.sh
cd app/android
YO_DEV_SMOKE=1 ./gradlew testDebugUnitTest --tests '*DevHubSmokeTest'
```

The normal run skips the live test. No live hub smoke test or account mutation was
performed in this parity pass. Build heavy tasks sequentially, under `nice -n 10`,
with four workers/jobs; do not overlap Gradle and a separate cargo build.

## Current parity work

The Android list now consumes UniFFI `YoList` rather than duplicating Rust ordering,
feedback counts and timers. Core `nextTick` drives monotonic scheduling; Compose only
interpolates animations between those boundaries. Foreground, visible rows, camera,
recording, press and held outgoing stack state feed the shared model. Core sender IDs
remain distinct even when profile names match. Rust owns send retries/key refresh/fallback;
Android drives its pending delivery queue rather than recursively sending another Yo.

Welcome contains the intro carousel (iOS's cards) and name signup; sign-in is a native sheet. Help shows the same cards,
Yo Team support chat and Status. Status uses the actual core round-trip probe and exposes
anonymous crash reporting (default on: managed frames, OS native-crash/ANR collection, sanitized native frames). Media send
feedback follows confirmed sending, and swipe-down discards the unseen viewer queue.
Material 3 1.5.0-rc01 enables expressive motion (see the
[AndroidX release notes](https://developer.android.com/jetpack/androidx/releases/compose-material3)).
Incoming rows use plain white SDR text, row flash and a gentle pulse that only grows.
No HDR, physics overlay, sensors, incoming sweeps or outgoing flights remain.
Incoming and outgoing photo/video thumbnails are encrypted at rest and decoded
off the UI thread. Crash reports use the same structured anonymous diagnostics route as
iOS and can be inspected in Status. See [PARITY.md](PARITY.md) for remaining code gaps.

Yo's own words (bar titles, send and arrival lines, notifications, widget texts) and product rules (limits,
encoding, widget selection, notification row) come from the core's `yo_product` via UniFFI, in English like iOS.
Settings, dialogs and sheets are Material 3 Expressive standard components (segmented list items, switches, top
app bars, sheets); the Yo look stays on the content (Flat UI bars, Montserrat Bold, Amethyst). Logical placement supports RTL. TalkBack, large fonts and RTL still need
hardware acceptance. Nested settings, onboarding, camera and viewer use predictive back. Camera QR scanning reads recovery keys; in camera mode a Yo code in view asks as an opened invite link does, once while it stays in view (`CodeSightings`, read with ZXing only where the camera runs all its streams as they are); safety numbers are compared as digits, as on iOS; native chooser targets support direct media shares. Presentation promotions persist and swipe removal has core-owned undo. Status counts the Yos (total, sent/received, the top friend); the long-press menu shows the last sent/received time.

The release APK contains a manually maintained startup baseline profile, in addition to
library profiles; no device performance measurements are claimed. An independent
`.github/workflows/android.yml` builds debug/release, runs offline unit tests and lint on
Android/core PR and branch changes. `dev/core.sh` is explicitly unignored for fresh clones.

For restricted build environments with short-lived temporary directories, disable the
Kotlin incremental backup cache and use a single-use daemon:

```sh
nice -n 10 ./gradlew --no-daemon --max-workers=4 -Pkotlin.incremental=false assembleDebug assembleRelease testDebugUnitTest lint
```


## Google Play

The release build is the store app `app.justyo.android` ("Yo"); debug stays `app.justyo.android.dev` and the device QA
build `app.justyo.android.dev.soak`. The release talks to the live hub `hub.justyo.app`, as the iOS TestFlight and App
Store build; debug and device QA stay on the dev hub (`BuildConfig.HUB`; the pins per hub in `Hub.kt`, the same as
`Hub.swift`). Invite links (`justyo.co/.well-known/assetlinks.json`) and passkeys (the live hub's
`YO_ASSET_LINKS_FILE`, for the passkey domain justyo.app) name the Play app signing certificate. Play App Signing
holds the app signing key; the release is signed with the upload key from `keystore.properties` next to
`settings.gradle.kts` (ignored: `storeFile`, `storePassword`, `keyAlias`, `keyPassword`) or from
`YO_UPLOAD_STOREFILE`, `YO_UPLOAD_STOREPASSWORD`, `YO_UPLOAD_KEYALIAS` and `YO_UPLOAD_KEYPASSWORD`; without either it
stays unsigned. The upload key and its passwords are in 1Password (Yo vault, "Yo Android upload key"). Each upload
needs a higher `-PyoVersionCode` (minutes since 1970):

```sh
nice -n 10 ./gradlew :app:bundleRelease -PyoVersionCode=$(( $(date +%s) / 60 ))
```

Generated bindings, native libraries, SDK paths, signing files and machine settings are not checked in.
