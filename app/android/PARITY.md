# Android / iOS parity

Reference: current `app/ios/YoApp/Sources/YoApp`, current UniFFI bindings and
`core/client/src/list.rs`; main through `12a320c`, plus the owner’s round-three reversal and latest row-status instructions.
**Done** means implemented and build checked, not device acceptance. This is still
**partial parity**, not a release-ready claim. No hub or core behavior changed. iOS files and the prior shared-test addition are restored to main; see merge review below.

| Feature | Status | Android implementation / remaining work |
|---|---|---|
| Current core / bindings | Done | `dev/core.sh` rebuilt arm64, x86_64 and host libraries and Kotlin bindings. `Repository`, `MediaQueue` use the current client APIs. Generated outputs remain ignored. |
| Shared list model | Done | `Repository` uses `YoList` with monotonic time, core `nextTick`, viewport, foreground, camera, recording, presses and outgoing undo inputs. Rows render core order, titles, counts, sender IDs and timing boundaries. Removed Kotlin `FeedbackTiming`, arrival countdown jobs and promotion rules. Native binding tests cover freezes, promotion, sent expiry and sender identity. |
| Ordering / delete refinements | Done, device acceptance pending | Core owns live presentation order; merges by stable contact key retain contacts added concurrently with a tick. Presentation promotions merge into the latest contact list by stable key and persist in the encrypted UI record. Native swipe removal and menu removal use core delete/undo with a three-second Snackbar; group member removal uses stable user IDs. No outgoing flights, incoming sweeps or flight clock freezes remain. |
| Retry, fallback, access-key refresh | Done | Current Rust sealed-send/key-refresh/signed fallback and delivery queue are used. Removed Kotlin's recursive first-send retries, which could create additional Yos. Android schedules `deliver()` for the core's pending queue, including after a failure. Durable media upload scheduling remains Android-owned. |
| Group sender identity / names | Done | Core `ListSender.user` identifies sender runs; `Profile` events and refreshed contacts feed `senderNamed`. Two identical names retain different sender identities. Unknown names use the core's placeholder and heal when the core returns a profile. |
| Yo design / native controls | Done, native adaptation | Full-width Flat UI bars, eight colors, Montserrat Bold, Amethyst; Material 3 **1.5.0-rc01** expressive motion scheme, native sheets, menus, Snackbars, Photo Picker and CameraX. Buttons use native Material shapes and controls; the Android layout intentionally differs from iOS glass controls. |
| Welcome / sign-in | Done | Name field, availability feedback and Sign Up on the welcome/tutorial screen; “Got Yo? Sign In” opens a Material bottom sheet with gated Credential Manager and recovery-key flows. Availability cache remains in the Android presentation. |
| Settings / contacts / help | Done, native adaptation | Contacts card with encrypted-list save/checks; Help includes Android tutorial rows, Contact Us, Status and Licenses. Existing account, media, appearance, blocked and statistics pages retained. Help reuses the complete welcome carousel. Status exposes round-trip, crash history, key/refill/session/PQ/contact checks. Statistics include local sent/received totals, top friend, and per-contact last sent/received times. Exact iOS check/icon hierarchy remains visually different. |
| Support chat | Done | Contact Us renders core `supportThread`, sends with `sendSupport`, marks visible thread read with `supportRead`, and refreshes while open. Displays Yo Team / You. Core handles protocol and account authentication. Poll errors retain the last visible thread and show an offline message with Retry. |
| Status round trip | Done | User-triggered core `roundTripProbe("PINGTEST", 10000)` with busy/result state. Runs off the UI thread; no synthetic latency. |
| Anonymous crash reports | Done, device acceptance pending | Default-on, retained 16-report local viewer, managed uncaught handler on API 28+, Android 11+ OS native-crash/ANR collection and Android 12+ bounded native tombstone parsing. Structured string-field JSON on iOS’s unsigned `/v1/diagnostics/crash` route, independent anonymous connection pool. Only allowlisted binary basenames/relative offsets and app-owned managed frame locations; no messages, paths, thread names, memory, account or device identifiers. Upload cursor advances only on success. API 28–30 native traces are unavailable from the OS API. |
| Neighbour colors | Done | Consecutive equal colors darken progressively in displayed core order. Base contact colors remain unchanged. |
| Non-overlapping row text | Done, device acceptance pending | Received status stays at the trailing edge; name and sent status stay centred. Sent status uses full-size Sent Yo, then a bold ↑ Yo only when necessary, then hides for received priority. All counts use up to five bangs plus rolling +N overflow; bangs yield first and numbers keep 30 sp. Logical placement follows RTL. Extreme fonts/narrow screens, groups and every undo/media combination need device acceptance. |
| Incoming flash / pulse | Done, device acceptance pending | Plain white SDR labels; row flash and a gentle scale pulse from 1 to 1.025, never dimmed by the pulse. No HDR, gain maps, flights or sweeps. Core expiry fade remains. |
| Photo stack | Done, device acceptance pending | As in current main, one photo is shown with its count on top; the landing photo cannot cover it. At most three thumbnails are decoded/cached for previews. Encrypted receive/outgoing queues remain the source. Incoming photo/video thumbnails are decoded off-thread after download and stored encrypted. Holding incoming media pauses the viewer; release dismisses a held preview, while a short tap leaves the viewer open. Outgoing hold previews dismiss on release; a short tap opens the native preview dialog. |
| Viewer discard | Done | Swipe-down forgets the whole viewer queue, including unseen items. Normal close/back forgets only viewed items. Consumed-before-display and encrypted storage remain. Needs device gesture/process-death acceptance. |
| Send click / haptics | Done | Android key-click sound only in normal ringer mode; native `HapticFeedbackConstants` for taps/receive/hold. Hardware and user sound-setting acceptance remain. |
| Camera / media | Partial | CameraX photo/video, hold to record, zoom/flip, permissions, 10-second videos, picker, image normalization and Media3 export. Existing hardware lens/mirroring/metadata/export-size acceptance gaps remain. |
| Durable send / undo | Partial | Encrypted manifests/blobs, uploads with three slots, per-target send order, retry/backoff, account fences. Media undo duration comes from `listTimings`; core list freezes for all held stacks, even offscreen. Sent feedback now follows successful core media send, not staging. Restart drops held items. Encrypted outgoing thumbnails are implemented. Disk-full/crash/rate-limit soak remains device acceptance. |
| State / account / recovery | Done, platform differences | Atomic throwing core store, nonexportable Keystore wrapping, encrypted UI/media vault, no Android backup, staged registration, iOS-compatible recovery backups, QR image import/export, document export, rotate/sign-out/delete. Live recovery QR camera scanning is implemented. The profile shows the secure-account reminder. Synced Keystore recovery and iOS-only legacy Keychain/UserDefaults migrations have no Android storage equivalent; recovery-key backups remain the cross-platform migration path. |
| Passkeys / App Links | Missing externally | Credential Manager assertion path is gated. Domain `assetlinks.json`, signing certificate association, PRF creation and cross-platform restore acceptance remain. Name registration/recovery keys work independently. No fake security success. |
| Groups / notices | Done, device acceptance pending | Core create/rename/member add/remove/leave; member IDs drive removal even with duplicate names. JPEG notices match iOS’s Amethyst, 96 px heading, adaptive 116 px sentence, Sunflower names, underlined groups, and explicit made/added/removed/renamed/left wording. Creation includes up to four member names (three plus the remaining count for larger groups). Burst/scroll/interoperability acceptance remains. |
| Encryption / contacts | Done, native adaptation | Core security/contact/session/safety-number APIs, confirm changed key, verify/unverify, undo preferences. Empty session lists no longer alone mark an unexchanged contact unhealthy. Safety QR display/scanning uses core safetyCode/safetyCodeMatches and only verifies matching codes; unknown/unexchanged contacts never request a safety number. Full iOS status visuals remain different. |
| Accessibility / language | Done for resource readiness; device acceptance pending | Standard Material semantics, actionable labels, minimum touch sizes, growing row heights, logical RTL placement. UI labels, settings footers, dynamic sentences, app-owned errors, content descriptions and count plurals are in resources; internal navigation IDs and core-provided labels remain protocol/model strings, with row labels translated at rendering. English only; translations remain owner work. TalkBack, large-font and RTL acceptance untested. |
| Background notifications / Yo Back | Missing externally | Hub currently supports APNs. FCM credentials and a hub transport/token route are required; no FCM service or false background-delivery promise. Foreground long polling only. |
| Sharing / other platform features | Partial | ACTION_SEND/MULTIPLE into the single client; normalized media stages encrypted before destination selection and restores after process death. An account fence protects imports, and transferring to the send queue removes the staging entry. Native chooser shortcuts are published for up to four contacts, removed when blocked/deleted/signed out, and route media to the selected stable contact ID. The source URI grant is needed only during initial import. All resolved runtime Maven artifacts have a build-generated inventory of inherited POM license declarations and embedded notices, included in the lazily rendered Licenses page. Background claims/prefetch depend on the missing push transport. Three launcher aliases, licenses, update gate and optional frame probe retained. |

## What should move into the core

Keep Android thin when adding the remaining shared behavior: source/presentation-order
persistence, durable outgoing stack scheduling/cancellation and completion semantics,
first-Yo onboarding state, account reminder/migration policy, shared statistics, group
notice sentences, name-availability debounce/cache policy, structured sanitized crash
report records, and structured list-label localization. Animation geometry,
Android sound/haptics, permissions and OS exit collection belong to the platform.
The existing core supplies counts and timing; Android renders localized row labels and restored +N overflow digits without another count/timer model.

## Verification

- Current native libraries and generated Kotlin bindings rebuilt for both Android ABIs and host.
- Offline JVM tests cover core/list contracts, bounded crash sanitization, binary safety QR round trips; the live dev-hub mutation test is skipped.
- Final build/test/lint result is recorded in README; release is also built and shrunk. CI runs all four requested tasks in a separate Android workflow.
- `nice -n 10 cargo test --workspace --locked` passed; core/tests now equal main. No new clippy run is claimed.
- No emulator, device, deployment, SSH, hub change or commit in this pass.
- Device acceptance remains necessary for CameraX, provider/biometric flows, physical sound/haptics,
  animation timing, Android 11+ exit reports, RTL/large-font/TalkBack, and two-account media/group interoperability.

## Round-two review

A manually maintained startup baseline is packaged with the release APK; it is not a
measured device profile. Draw-time reads isolate press scale and fade animation
from list composition. Lists use stable contact, member, message and sender keys.
Predictive back handles nested settings, sign-in, camera and viewer cancellation;
native sheets retain their system transitions. Accessibility still requires hardware acceptance.

Merge review compares every non-Android file, including symlink targets and modes,
against current origin/main. Restoring the older branch's iOS physics and shared binding
test changes is intentional: the resulting iOS/core/tests content must equal main.
The only intended new non-Android file is `.github/workflows/android.yml`.
`dev/core.sh` was previously excluded by `core.*`; an explicit ignore exception fixes
fresh-clone builds. No emulator, deploy, SSH, hub mutation or commit was used.

## Round-three review

Removed the debug physics engine, sensor registration, spawn limits, performance brakes,
engine tests, HDR window mode and gain-map renderer. No incoming or outgoing flights
remain. Viewer swipe-down discard and send click are retained. Single-photo thumbnails
show the stack number on the photo without layered edges.

Fetched main explicitly without writing FETCH_HEAD (the shared Git metadata is read-only).
The reference at `12a320c` still contains flights and the older repeated-send arrow;
Android follows the owner's newer explicit instructions for those changes. Final comparison found FriendList.swift unchanged after the last fetch; the newer iOS
implementation is not yet on origin/main. iOS/core files remain identical to main.

Validation: `nice -n 10 ./gradlew --max-workers=4 assembleDebug assembleRelease
testDebugUnitTest lint` passed with the prescribed environment. Thirteen offline tests
passed; the live hub smoke test was skipped. No device acceptance, deployment or commit
was performed. `git diff --check` passed; the only non-Android branch difference is the
previously added `.github/workflows/android.yml`.

Remaining acceptance needs actual hardware: predictive-back gestures, CameraX/QR/provider
flows, TalkBack/large fonts/RTL, media interoperability and disk-full/crash/rate-limit soak.
Android does not emulate iOS-only legacy Keychain/UserDefaults migration or synced
Keychain recovery. External blockers remain push transport/FCM, domain/signing association
for passkeys, release signing, and translations beyond the supplied English resources.
