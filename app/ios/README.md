# Yo for iPhone

SwiftUI, iOS 26, Swift 6, built on Linux with [xtool](https://github.com/xtool-org/xtool) through
[omarchy-apple-dev](https://github.com/joshuaswarren/omarchy-apple-dev) (Swift 6.4, Xcode 27 SDK). Device builds need no Mac; the Simulator does (below).

The app talks to the live hub (TestFlight and App Store) or the dev hub (Yo Dev), `Hub.swift`.

- The original's list: one coloured bar per friend from edge to edge, the name large in the middle; tap = Yo.
  The latest activity comes first; hold a bar for Report, Block and Remove, swipe it for Block and Remove; the last
  bar adds one.
- A Yo arriving: the sender's bar knocks and says "Yo!!" inline (a "!" per Yo, `ShareBridge.bangs`), fading out; no
  unread marks.
- Camera mode (the original's camera context): the bars turn see-through over the live picture; tap a name for a
  photo (with the shutter's haptic), hold it for a video (the bar fills over 10 s); a ring at the bar's left undoes
  the send for 2 s, while the photo already uploads: the Yo goes only when the ring is done, and one still held
  after a crash is never sent. Photo library, zoom and front/back at the bottom. Received photos and videos wait as a round thumbnail; hold or tap it to see them once.
- Invite links: https://justyo.co/NAME (your code, Invite on the list) and yo://NAME open Yo and add NAME as a
  friend; before an account the name waits through the way in (`Invite.swift`). The first Yo sent says what it does.
- Share sheet (Yo from the App Store and TestFlight): "Yo" for up to ten photos or one video from Photos or any app;
  tap a friend's or group's Yo bar to send the media and see the result on that bar. Tap more bars to send to more
  recipients, then tap Done. Without network (or with the hub's "not now") shares wait in the App Group
  `group.app.justyo.ios` and go when Yo opens (`ShareBridge.swift`, `Share.swift`). Shared, library and camera videos
  have a 10 s limit. Longer shared or library videos open Yo's trimmer full-screen (`VideoTrimmer.swift`).
- Notifications (the same builds): "Yo from YoGPT", "📷 from …", "Yo!!!!! +4 from … · 📷 2" (one notification per run, as the bar says it), with the Yo sound and "Yo Back". The
  push only says "Yo"; the notification service (`Sources/Extensions/YoNotify`) receives once, names the sender and fetches a
  photo or video of up to 6 MiB into the App Group first (`Prefetch.swift`; the app and the service claim a blob
  before either fetches it, so one fetch is under way at a time, spec 6.7; storage is acknowledged only after writing),
  so the app shows it at once. A push that brings no new Yo (a receipt, a
  retry, the rest of a burst) goes quietly; Yos the app takes itself just after it was left get their
  notification from the app; Yos read in the app take theirs out of Notification Center.
- The app and its extensions use one account state, in the App Group, its key in the group's keychain group: one
  SQLite database in WAL mode (`yo-state-v3/tables.sqlite`, `SharedState.swift`, `StateDb` and `StateLock` in the
  core), with tables for client state and libsignal records. Reads (the list's friends, contacts, groups, settings) take the last
  committed state without any turn, so the app never waits for the notification service at launch. Each step that
  may write takes the turn (a file lock) and the newest state, and writes only over the state it started from
  (compare and swap in one short transaction, never across a request): one writer, never a write from an old state.
  A connection lives for one access, so no SQLite lock is held when iOS suspends a process (0xdead10cc). After an
  process wrote, a Darwin notification named for it (`StateChanged`, as Signal's SDSCrossProcess) makes the app take
  in what an extension saved. The SQLite settings follow Signal-iOS (IMMEDIATE writes, synchronous NORMAL, full fsync,
  a TRUNCATE checkpoint after each write that gives up after 50 ms). At startup, the main app can recover its
  identity from the iCloud Keychain or an older local database and register into a fresh database. Extensions
  wait for a registered state. Every state flock in the app gets background time. Refused time skips
  the turn (reads go on); expiry releases the flock, cancels its request and fences later writes (`held`) and
  requests; the next step starts again from the database. Lock acquisition has a 10 s total budget and retries a
  refused turn once. Active app turns are revoked on leaving the foreground; background turns require background
  time. Extensions revoke their turns before returning to iOS. Anonymous blob GET/DELETE requests run outside these
  turns; complete blob answers survive turn expiry. The app and service keep a shared acknowledgement journal,
  retrying after durable storage without delaying the photo.
  `app/ios/tests/shared-access.sh` tests the lock helper on Linux (Swift 6), without a device.
  `app/ios/tests/blob-ack.sh` checks storage ordering, failed acknowledgements and the service/app file handoff.
  Yo Dev (`dev/push.sh`) is built without the extensions and keeps its state to itself, in the same kind of
  database in its own folder.

Account storage backs up contacts, groups, keys and settings like the app icon and left-handed mode.

## Build and run

```sh
app/ios/dev/core.sh   # the Rust core first: YoApp/build/core (ignored), again after a change in core/
cd app/ios/YoApp
xtool dev build   # -> xtool/YoApp.app
xtool dev run     # build, sign with your Apple ID, install and launch on the iPhone over USB
```

Without a cable (iOS 27; the phone on Wi-Fi, reached by Tailscale or the LAN), no sudo:

```sh
app/ios/dev/push.sh PHONE                       # build, sign for development, install and launch (dev/wireless.py)
python3 app/ios/dev/wireless.py PHONE --shot s.png   # a screenshot of the phone
```

Once before: pair over USB (`tools/tailnet-tunnel.py pair` in omarchy-apple-dev) and make the development identity
(`tools/provision-dev.py --bundle-id app.justyo.ios.dev`). Keys, profiles and pairing records stay outside the
repository; this machine's phone address and paths go in `dev/local.env` (ignored). The scripts run with
pymobiledevice3's Python (`PY` in `local.env`).

Yo Dev signing expands the profile's wildcard to the concrete app-id prefix plus bundle id for both the
application identifier and default keychain group. The prefix comes from `ApplicationIdentifierPrefix`, which
need not be the team id. Legacy keychain grants stay available so an older state's sealing key can be adopted
only after it opens Yo Dev's own file. `python3 app/ios/tests/dev-sign.py` checks this with a synthetic profile.

To check on a phone: sign in to Yo Dev, note its account and friends, then run `dev/push.sh` twice over the
installed app, quitting and reopening after each install. The account and friends must remain, without
“Sign In Again”; Yo from TestFlight must keep its own account. Check an older dev install as well to exercise
key migration. Do not delete the app for this check: deleting it clears its private state file, so account
recovery is required even if its key survives in the keychain.

## In the Simulator (on a Mac)

```sh
app/ios/dev/sim.sh build                 # the core for aarch64-apple-ios-sim, then app and UI driver (XcodeGen, xcodebuild)
app/ios/dev/sim.sh device "Yo Sim"       # a Simulator of its own, booted
app/ios/dev/sim.sh install "Yo Sim"
app/ios/dev/sim.sh drive "Yo Sim" DIR &  # YoDriver: tap, type, read the screen, audit, by commands (dev/sim/drive.py)
```

Yo Dev in the Simulator talks to the dev hub, with its notification service and an App Group of its own
(`group.app.justyo.ios.dev`; the Simulator needs no profile for it). The Simulator runs no notification service for
`simctl push` and gets no APNs token without the Mac's device identity, so `sim.sh notify` runs the service's code
(`YoNotifyProbe`) on the hub's push and shows what it made. Passkeys and the iCloud Keychain's sync need a device.

**Passkeys and PRF.** Providers give the PRF output only when a passkey is used: asking for it while making one makes
1Password fail to save it, so the app makes the passkey, then uses it once at once (right away where the provider
says it supports PRF, else after a line that says why). What a provider reports then is not to be trusted: 1Password
on iOS says "not supported" and gives the output when asked.
`TESTING.md` lists what was gone through that way, and how.

## The Rust core

`core/bindings` (UniFFI) comes in as the module `YoCore`: `import YoCore`, then `YoClient`, `openBackup`,
`usernameFree` and the `HttpTransport` protocol the app implements. Every call blocks; make it off
the main thread. `dev/core.sh` builds it on Linux and `dev/push.sh` runs it first:

- `cargo rustc --crate-type staticlib --target aarch64-apple-ios` with the host's rustup (once:
  `rustup target add aarch64-apple-ios`) against the iPhoneOS SDK of the darwin Swift SDK, small (opt-level s, fat
  LTO), in `target/ios`;
- compiler-rt's C members dropped from the archive, which `ld64.lld` would otherwise call duplicates of
  `libclang_rt.ios.a`;
- `uniffi-bindgen` in library mode reads the interface from `libyo.a`: `yo.swift` becomes the target `YoCore`,
  `yoFFI.h` with its module map and the library a hand-made `yoFFI.xcframework` (binary target `yoFFI`).

## Shared list rules

`YoList` (`core/client/src/list.rs`, UniFFI in `core/bindings`) owns row ordering, titles/counts, requests, media
signs, undo/notice deadlines, group sender clocks, and the tapped row lock. `FriendList` reports monotonic time,
visible row ids, scene phase, camera mode, and touch/recording/flight events; it ticks at the returned deadline.
Yo lifetimes spend time only in the visible viewport, foreground, and outside camera mode. SwiftUI keeps the
animations and gestures, and anchors the core's locked row with `ScrollPosition` during reorders. The same model
and API are available to Android; see spec §5.1 and the Rust list regression tests and seeded simulation.

List notification priority is shared core policy: a pending deletion (3 seconds, Undo) takes precedence over
server offline, which takes precedence over the latest send failure. Clearing a higher priority notice reveals
the current lower priority notice. Replacing a deletion commits the previous one; backgrounding commits it too.
The platform inserts the white-on-amethyst bar above the list and animates layout changes, without covering rows.
Arrival and group sender clocks wait for the arrival's batched reorder to finish, then spend time only while
that row is visible in the foreground outside camera mode. A frozen arrival keeps its full lifetime.


## Frame performance (Yo Dev, physical phone)

`dev/perf.sh` installs an optimized Yo Dev, builds `yo-send`, and rotates **300 runs** through direct
100/3 s, group 100/3 s, six simultaneous senders × 20/1 s, one synthetic photo, cold launch, warm reopen,
and 30 physical tap-to-sends. It launches each process with `YO_PERF=1` and a unique run token. It uses
wireless.py's userspace pairing tunnel and pymobiledevice3's unified log relay. It retains only the
`YO_PERF_RUN {json}` summary, never unrelated device logs. Nothing is deployed until you run it.

Prepare test accounts only, on the dev hub. `yo-send` registers a missing state automatically (there is no
`--help`; options are at `tests/src/send.rs`). Put machine settings in ignored `dev/local.env`:

```sh
# Alongside the existing YO_PHONE/PY/signing settings; all names below must be synthetic test accounts.
PERF_TO=...      # account signed into Yo Dev; accept senders and the group before measuring
PERF_PREFIX=...  # unique synthetic prefix, short enough for username rules; retained across resumes
PERF_GROUP=...   # test group name
PERF_PHOTO=...   # absolute path to synthetic JPEG, no personal image/metadata
```

Before the first run, build and prepare six senders and a seven-member group (adjust members up to 20):

```sh
cargo build --release -p yo-tests --bin yo-send
source app/ios/dev/local.env
mkdir -p target/perf-state
for n in 0 1 2 3 4 5; do
  target/release/yo-send --url https://dev.justyo.app \
    --state "target/perf-state/$PERF_PREFIX-$n" --name "$PERF_PREFIX$n" --to "$PERF_TO" --count 1
done
target/release/yo-send --url https://dev.justyo.app \
  --state "target/perf-state/$PERF_PREFIX-0" --name "${PERF_PREFIX}0" \
  --make-group "$PERF_GROUP" \
  --members "$PERF_TO,${PERF_PREFIX}1,${PERF_PREFIX}2,${PERF_PREFIX}3,${PERF_PREFIX}4,${PERF_PREFIX}5"
# Accept requests in Yo Dev; leave it on the friend list, phone unlocked, Low Power Mode off.
N=300 app/ios/dev/perf.sh
app/ios/dev/perf.sh --summary
```

The runner holds an exclusive lock (never overlap another sender using these states). It fsyncs completed
runs to `target/perf/runs.jsonl`; rerun the same command/settings to skip completed indices. Remove that
file to start a new campaign after code, device, refresh-rate, group-size or account changes. Summaries are
`target/perf/summary.md`: worst **per-run** p50/p95/p99/max callback intervals, summed hitch count/time,
worst callback stall with overlapping named main-thread work, and process-start-to-list milliseconds. They are deliberately not percentiles
of percentiles represented as pooled frame percentiles. Failed sends, rate overruns and missing reports
stop without completing that index. Sender output is checked in memory and discarded.

Each measurement lasts 15 seconds after the list installs its watch. During burst runs, scroll and tap the
list to exercise interaction under load. Warm runs prompt for Home → reopen; tap runs prompt for 30 real
taps. Both require operator confirmation before saving. There is no private touch injection: this is an
operator-assisted harness, not 300 unattended UI tests. Cold runs restart the process, not the phone or
filesystem cache. Repeated warm runs restart first, then measure the operator's background/foreground
cycle. Background display gaps are excluded. The phone's actual 60/120 Hz mode must be checked separately.

`Perf.timed` emits Points of Interest signposts and records named synchronous main-thread spans. Each run
emits one public unified-log JSON line with percentiles, hitches, excess hitch milliseconds, longest excess
callback interval and overlapping spans (or explicitly unattributed). Deadline excess uses the current
CADisplayLink target interval, with 0.5 ms jitter tolerance. These are **CPU callback scheduling proxies**:
they cannot prove actual presentation deadlines, GPU hitches, or identify arbitrary uninstrumented stacks.
Use Instruments Animation Hitches/Time Profiler on the phone to confirm the zero-hitch target, launch before
list installation, and unattributed stalls. The baseline/fixed-device comparison is still outstanding.

The static audit and remaining blockers are in [PERFORMANCE.md](PERFORMANCE.md). For restricted Linux builds,
use `XDG_CACHE_HOME="$PWD/target/perf-cache"` (absolute repository path) with xtool when the default cache is
read-only. No core semantics or durability boundaries were changed; no state-save debounce is used.


## Camera wipe and Yo-to-Yo Roundtrip

The edge wipe uses only finger translation, with animations disabled during the drag. Touch-down queues
capture startup without awaiting permission or frames; permission is requested only after entering camera
mode when needed. Session construction, configuration, connection rotation/mirroring and start/stop all
run on the capture queue. A persistent amethyst-backed sample-buffer surface replaces the preview layer's
session attachment: assigning `AVCaptureVideoPreviewLayer.session` on the main thread while starting or
configuring capture could contend with AVFoundation's session work during the first drag. Preview insertion
and the old priming-layout changes are removed from gesture startup. The serial preview renderer displays
buffers without a session connection; only feed opacity fades in from black over 40 ms after its first buffer. Thumbnail
conversion stays off the main actor. HitchMonitor only samples callback intervals on main and writes on its
own utility queue; it does not gate the wipe or wait for camera readiness. Physical presentation smoothness
still needs an on-device swipe check; a successful compile cannot measure it.

The Account title and Status → Server share a user-triggered, ten-second PINGTEST probe and a volatile last result.
The title flies a small Yo out and back, bounces on landing and gives success/warning feedback; Reduce Motion
uses fades. The existing account client selects the same hub as `Hub.home` (including the dev test override).
Foreground inbox reception continuously long-polls; warm sends post without first fetching the inbox. Signed
and anonymous connection pools remain separate, with anonymous warm-up on launch and foreground. Empty
polls do not save full core state. Durable encrypted send state is still saved before delivery.

PING can be added as a normal echo contact for Yos, photos and videos. PINGTEST probes and their replies
create no visible Yo, notification, stats, contact, request or Account Sync entry. Probes have no copy to
another device. See
[operator deployment and privacy](../../skills/pingtest/README.md).


Help › Contact Us is a support conversation with the Yo team, not end-to-end encrypted (the chat says so).
The core signs sends, thread fetches and bounded read markers under the state lock and performs transport
outside it. Opening the chat marks displayed team replies read; pull to refresh fetches later replies.
Unread counts refresh at launch, foreground and opening Settings; no polling. Draft text remains on send
failure for retry. Deploy the hub and admin support changes before releasing this app (hub/README.md).
