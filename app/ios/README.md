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
- Profile sheet: save the photos and videos you send to your library.

## Build and run

```sh
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
(`tools/provision-dev.py --bundle-id co.justyo.ios`). Keys, profiles and pairing records stay outside the
repository; this machine's phone address and paths go in `dev/local.env` (ignored). The scripts run with
pymobiledevice3's Python (`PY` in `local.env`).

Later the Rust core comes in as a Swift package (`core/bindings`, UniFFI).
