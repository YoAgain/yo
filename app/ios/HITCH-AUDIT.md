# Hitch audit — settings-tidy

No device measurements were collected. The single `dev/push.sh` attempt built and signed, then failed because the phone's RemotePairing service refused the connection. Nothing was installed or launched. Physical presentation, corner stability, context-menu scale and the flight matrix (list/camera, both hands, screen edges) remain unverified.

## Monitor

`Sources/YoApp/Hitches.swift:4`: `YO_DEV` excludes the display link and Help page from distribution compilation. `Package.swift:19` enables it only with `YO_DEV_BUILD=1`, which `dev/push.sh` supplies. A production binary was checked for absence of the monitor; the final dev binary contains it. The callback interval exceeding 1.5 times the display link's target interval records duration, date, scene and `kind: hitch`. Foreground notifications reset the timestamp to exclude suspension. Disk access uses a serial utility queue; records are bounded to 100. Help → Hitches groups counts/worst milliseconds and lists records, with Clear.

This checkout has no local diagnostics table/error database. The implementation therefore uses a dedicated local `Documents/diagnostics-hitches.json` file. It has no upload path, even when crash sharing is enabled. Existing MetricKit consent behavior is unchanged. Callback timing is a CPU scheduling proxy, not evidence of GPU presentation deadlines.

## Findings fixed

- `Sources/YoApp/Settings.swift:36`: Help previously selected its measured height after layout, producing a separate detent update. Root, Help, App Icon and Your Code now share the root detent. Removed hidden Help measurement and its duplicate status tasks. Root and destinations now respect the same navigation-bar safe area. Message already uses a fixed large detent.
- `Sources/YoApp/FriendList.swift:717`: full-width system context-menu preview with rectangular preview shape. Existing tap highlight and system dimming are retained; a physical no-scale check is still required.
- `Sources/YoApp/FriendList.swift:555`: row-local flight z-index, avoiding parent observation of each bar's send state. Removed the clipping around the whole row and the swipe wrapper. Backgrounds, request choices and image pixels retain their own bounded clipping.
- `Sources/YoApp/FriendList.swift:63`: cached rows compared identity-only Friend equality, missing metadata updates. Compare full displayed content while retaining stable identities (`Friends.swift:55`).
- The former Swift ordering gate (now `core/client/src/list.rs`): central ordering gate catches direct insert/promote/reconcile/group/refresh paths. Metadata remains live; arrivals append while frozen, with intended moves queued. Multiple queued arrivals preserve prior moves. Removed identities are dropped at release.
- `Sources/YoApp/FriendList.swift:1717`: release waits for pressed bars, recording, thumbnail preparation, undo stacks and send flights. Camera mode holds ordering for its entire lifetime.
- `Sources/YoApp/FriendList.swift:1786`: camera timing now belongs to the Rust list model: entry pauses remaining visible time, arrivals show immediately, and exit resumes clocks and releases queued ordering. Group sender fades use the same viewport/foreground gates.
- `Sources/YoApp/FriendList.swift:1942`: resting and flying video thumbnails display a white play glyph below the stack count. Received items use the existing video camera sign rather than a thumbnail.
- `Sources/YoApp/FriendList.swift:3062`: recording progress now paints in Canvas; it no longer measures geometry and changes rectangle layout every frame.
- `Sources/YoApp/FriendList.swift:685`: cancellation/cleanup paths move file removal off the main actor (including recording defer and library failure cleanup).
- `Sources/YoApp/Settings.swift:103`, `Intro.swift:201`, `AppIconPage.swift:86`: replaced UIKit named-image lookup in view construction with SwiftUI asset images.
- `Sources/YoApp/Inbox.swift:229`, `Media.swift:432`, `SendFailure.swift:4`, `YoApp.swift:61`: send failures use the existing top banner placement, yield to offline, expire and announce to VoiceOver. Queued plain Yos have a retry action using existing pending sends; media retry invokes the existing outbox path. Permanently dropped media has no resend payload.
- `Sources/YoApp/GroupNotice.swift:40`, `NewGroup.swift:126`: one searchable selection list and shared avatar/body-font/check-circle row, own account first and disabled, confirmed removals, capacity header and Done.

## Other animation paths reviewed

- FriendList camera edge wipe: shared screen boundary, background masks only; no flight rasterization/clipping added. Scroll row and intro page IDs remain stable; the thumbnail ID intentionally creates one new flight per new item.
- FriendList request/add expansion: layout changes only during their explicit transitions. Request clipping is scoped to request choices; add-row clipping does not contain flights.
- FriendList undo ring, ClockFade and media viewer progress: timeline work is localized to trim/opacity/progress, with image decoding and video thumbnail creation already on workers. RecordingFill was the per-frame geometry/layout exception fixed above.
- FriendList Bangs, knock, press flash and FlyingYo: local state and scalar transforms; no container-wide implicit animation was added. Formatting in zoom labels is outside display-link loops. No heavy overlay warranted a blanket drawingGroup; adding one around flight rows would reintroduce clipping/rasterization boundaries.
- SettingsPageLayout: custom layout preserves destination content's intended height during a sheet transition; no timeline-driven layout. SettingsRows expansion stays scoped to one disclosure.
- Contacts: system navigation and menus; contact security reads use Hub.run, including ContactState.loadAll. No explicit whole-form animation or per-frame geometry loop.
- Intro: static page identities, scroll transitions and local dot animation; asset lookup fixed above. No frame-loop decoding or synchronous hub/keychain calls.
- Your Code's existing synchronous ImageRenderer/QR export preparation remains on the main actor before the push; it is cached but can still cause tap latency on a cold render. This audit does not establish zero hitches.

## Checks

Passed: cargo fmt --all -- --check; cargo clippy --workspace --all-targets --locked -- -D warnings; cargo test --workspace --locked; CI seeds 100–130; optimized production and final dev xtool builds; shared-access.sh; blob-ack.sh; dev-sign.py; git diff --check.

The ordering gate and regression tests have since moved into the Rust list model (spec §5.1). Run them with:

```sh
export XDG_CACHE_HOME=/tmp/claude-1000/xdg-cache
cargo test --locked -p yo-core list::tests
```

All work remains uncommitted on settings-tidy. No git push.
