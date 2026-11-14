# Yo for iPhone

SwiftUI, iOS 26, Swift 6, built on Linux with [xtool](https://github.com/xtool-org/xtool) through
[omarchy-apple-dev](https://github.com/joshuaswarren/omarchy-apple-dev) (Swift 6.4, Xcode 27 SDK). No Mac.

For now a mockup without the hub: tapping a friend sends the Yo to yourself, as if that friend had sent it (the
first bar at once, the last three after five seconds, those between after two: time to lock the phone and see the
push).

- The original's list: one coloured bar per friend from edge to edge, the name large in the middle; tap = Yo.
  Hold a bar to drag it elsewhere or remove the friend; the last bar adds one.
- A Yo arriving: the title gains a "!" per Yo, the sender's bar knocks and shows an Amethyst "Yo" disc ("3×"),
  fading out; no unread marks. Notifications: "Yo from Leo", "Yo back" right on them.
- Camera mode (the original's camera context): the bars turn see-through over the live picture; tap a name for a
  photo, hold it for a video; a ring at the bar's left undoes the send for 1.5 s. Photo library and front/back at the
  bottom. Received photos and videos wait as a round thumbnail; hold or tap it to see them once.
- Invite links: https://justyo.co/NAME (your code, Invite on the list) and yo://add/NAME open Yo and add NAME as a
  friend; before an account the name waits through the way in (`Invite.swift`). The first Yo sent says what it does.
- Profile sheet: save the photos and videos you send to your library.
- Share sheet (Yo from the App Store and TestFlight): "Yo" for up to ten photos or one video from Photos or any app;
  the same bars, a tap on a name sends them to that friend from there (`Sources/YoShare`), as the app would. Without
  network (or with the hub's "not now") they wait in the App Group `group.app.justyo.ios` and the app sends them as
  soon as it opens (`ShareBridge.swift`, `Share.swift`).
- Notifications (the same builds): "Yo from YoGPT", "📷 from …", "3 Yos from …", with the Yo sound and "Yo Back". The
  push only says "Yo"; the notification service (`Sources/YoNotify`) receives once and names the sender, without
  fetching photos or videos. A push that brings no new Yo (the Welcome before a first Yo, the rest of a burst) goes
  quietly; Yos the app takes itself just after it was left get their notification from the app.
- The app and both extensions use one account state, in the App Group, its key in the group's keychain group, in
  turns under a file lock (`SharedState.swift`, `StateLock` in the core): each step of the core takes the lock and the
  newest state, so there is one writer and never a write from an old state. The state moves there once, at the
  first start of a build that has the group. Yo Dev (`dev/push.sh`) is built without the extensions and keeps its
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
