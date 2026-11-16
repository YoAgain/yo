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
- Invite links: https://justyo.co/NAME (your code, Invite on the list) and yo://add/NAME open Yo and add NAME as a
  friend; before an account the name waits through the way in (`Invite.swift`). The first Yo sent says what it does.
- Profile sheet: save the photos and videos you send to your library (at capture).
- Share sheet (Yo from the App Store and TestFlight): "Yo" for up to ten photos or one video from Photos or any app;
  the same bars, a tap on a name sends them to that friend from there (`Sources/YoShare`), as the app would. Without
  network (or with the hub's "not now") they wait in the App Group `group.app.justyo.ios` and the app sends them as
  soon as it opens (`ShareBridge.swift`, `Share.swift`).
- Notifications (the same builds): "Yo from YoGPT", "📷 from …", "Yo!!!!! +4 from … · 📷 2" (one notification per run, as the bar says it), with the Yo sound and "Yo Back". The
  push only says "Yo"; the notification service (`Sources/YoNotify`) receives once, names the sender and fetches a
  photo or video of up to 6 MiB into the App Group first (`Prefetch.swift`; the app and the service claim a blob
  before either fetches it, so one fetch is under way at a time, spec 6.7; storage is acknowledged only after writing),
  so the app shows it at once. A push that brings no new Yo (a receipt, a
  retry, the rest of a burst) goes quietly; Yos the app takes itself just after it was left get their
  notification from the app; Yos read in the app take theirs out of Notification Center.
- The app and both extensions use one account state, in the App Group, its key in the group's keychain group, in
  turns under a file lock (`SharedState.swift`, `StateLock` in the core): each step of the core takes the lock and the
  newest state, so there is one writer and never a write from an old state. The state moves there once, at the
  first start of a build that has the group. Every state flock in the app gets background time. Refused time skips the
  turn; expiry releases the flock, cancels its request and fences later saves and requests. The next turn reloads
  first start of a build that has the group. Lock acquisition has a 10 s total budget and retries a refused turn once. Active app turns are revoked
  on leaving the foreground; background turns require background time. Expiry releases the flock, cancels its
  request and fences later saves and requests. Foreground probes retry and reopen the same saved client after
  three failed reads, without resetting the account. Failed reads retain displayed groups and settings. The next turn reloads
  from disk, including when its generation did not change. Extensions revoke their turns before returning to iOS.
  Anonymous blob GET/DELETE requests run outside these turns; complete blob answers survive turn expiry. The app
  and service keep a shared acknowledgement journal, retrying after durable storage without delaying the photo.
  `app/ios/tests/shared-access.sh` tests the lock helper on Linux (Swift 6), without a device.
  `app/ios/tests/blob-ack.sh` checks storage ordering, failed acknowledgements and the service/app file handoff.
  Yo Dev (`dev/push.sh`) is built without the extensions and keeps its
  state to itself.

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
`usernameFree` and the `HttpTransport` and `StateStore` protocols the app implements. Every call blocks; make it off
the main thread. `dev/core.sh` builds it on Linux and `dev/push.sh` runs it first:

- `cargo rustc --crate-type staticlib --target aarch64-apple-ios` with the host's rustup (once:
  `rustup target add aarch64-apple-ios`) against the iPhoneOS SDK of the darwin Swift SDK, small (opt-level s, fat
  LTO), in `target/ios`;
- compiler-rt's C members dropped from the archive, which `ld64.lld` would otherwise call duplicates of
  `libclang_rt.ios.a`;
- `uniffi-bindgen` in library mode reads the interface from `libyo.a`: `yo.swift` becomes the target `YoCore`,
  `yoFFI.h` with its module map and the library a hand-made `yoFFI.xcframework` (binary target `yoFFI`).

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
