<p align="center">
  <img src="dev/readme-banner.png" alt="Yo. It's that simple." width="200">
</p>

<p align="center">The 2014 one-tap app is back.<br>
Tap a name to send an encrypted Yo, photo or video.</p>

<p align="center"><a href="https://justyo.co">Web</a> · <a href="https://testflight.apple.com/join/V3wa1DeX">TestFlight</a> · <a href="https://play.google.com/apps/testing/app.justyo.android">Play Store</a></p>

## Under the hood

Yo is based on Signal encryption, see the [YoIP spec](yoip/SPEC.md).<br>
All apps share one [Rust core](app/core). Services are defined in [yo-services](https://github.com/YoAgain/yo-services).

<details>
<summary><b>App</b></summary>

#### Core

- **One core** for iOS, Android and CLI: account, sessions, groups, local database.
- **Apps add** UI, push and the keystore.
- **New phone:** friends, groups and settings come back via encrypted Account Sync; messages don't.
- **Crash reports:** on by default, off in Settings › Status; random install id, no content.
- **Migrations:** old CLI, database and Keychain formats are converted once.

| | iOS | Android |
|---|---|---|
| Account key | iCloud Keychain | Google Block Store |
| Device keys | Keychain | Keystore |
| Push | APNs | FCM |
| UI | SwiftUI | Compose, Material 3 |

<a id="ios"></a>
#### iOS

- SwiftUI, iOS 26; share sheet, stickers, widgets, Apple Watch.
- Built on Linux with [xtool](https://github.com/xtool-org/xtool), no Mac: `app/ios/dev/core.sh`, then `app/ios/dev/push.sh`.

<a id="android"></a>
#### Android

- Kotlin, Jetpack Compose; flavors `play` (FCM, Google backup) and `floss` (F-Droid).
- `app/android/dev/bootstrap.sh`, then `./gradlew assemblePlayDebug`.

<a id="cli"></a>
#### CLI

- `yo`: a Yo account on the command line, JSON out; bots use `yo listen`.
- `cargo build --release -p yo-cli`, then `yo register NAME`, `yo send lina`.

</details>

<details>
<summary><b>YoIP Protocol</b></summary>

<a id="encryption"></a>
#### Encryption

```text
Phone A ──Yo──► Hub ──push──► Phone B
Signal          sealed        fetches and
protocol        envelope      decrypts
```

- libsignal: PQXDH, Double Ratchet, sealed sender.
- Groups up to 20 with zkgroup; photos and videos view-once.
- Sign-up with a name only.

#### Keys

- **Account key:** the identity; iCloud Keychain or Google Block Store; restore with passkey or recovery key.
- **Device keys:** local database, sealed by the keystore.
- Code: `yoip/net` (client ↔ hub) and `yoip/protocol` (end to end).
- Details: [spec §2](yoip/SPEC.md#2-identity), [§2.4](yoip/SPEC.md#24-recovery), [SECURITY.md](SECURITY.md).

</details>

<details>
<summary><b>Services</b></summary>

<a id="services"></a>

- Bots (PING, YoGPT) and bridges (IFTTT), one folder each in [yo-services](https://github.com/YoAgain/yo-services).
- The [executor](server/services) runs a service with its own Yo account; the [entrance](server/services/src/entrance.rs) at `api.justyo.app` handles the catalog, OAuth and bridge requests.
- **Not end-to-end** with the service: it decrypts what you send it.

```text
Yo        phone ◄─encrypted─► hub ◄─► executor ◄─► script
Bridge    IFTTT ──► api.justyo.app/ifttt/… ──► entrance ──► executor ──► script
```

</details>

<details>
<summary><b>Server</b></summary>

<a id="routes"></a>
#### Hub

- One Rust server on SQLite, behind a Cloudflare Tunnel; cannot read Yos.
- Routes, auth and limits: [spec §4](yoip/SPEC.md#4-net). Code: `server/hub`.

#### Admin

- Read-only operator page in plain Python, behind Cloudflare Access.
- Demo with made-up data: `python3 server/admin/dev/demo.py --demo`.

</details>

<details>
<summary><b>Development</b></summary>

<!-- loc -->
![Lines of code: per day, by part and by app](dev/loc-chart.png)
<!-- /loc -->

#### Layout

```text
app/ios, app/android, app/cli       the apps
app/core, app/core/{ui,bindings}    the core
yoip/                               YoIP spec, net/ and protocol/
server/{hub,admin,services}         the server
tests/, tests/fuzz/                 tests and fuzz targets
dev/                                scripts
```

#### Release

- `dev/release.sh VERSION notes.md`: signed tag → TestFlight, Play testing, F-Droid, CLI binary.
- Every push: Rust tests, lint, short fuzz run.

#### Benchmark

| | |
|---|---|
| Hub alone | **10,431 Yos/s** · 1 CPU core · p50 2.1 ms · p99 130 ms |
| Yo to Yo | **p50 71 ms** · p90 89 ms · real phone, Wi-Fi, round trip to a bot |
| Send, one client core | **50 Yos/s** · p50 290 ms · p99 708 ms |
| Lost | **0** |

<a id="tests"></a>
#### Tests

- **End-to-end:** every flow, route and restore against a real hub.
- **Simulation:** seeded random runs; a failure names its seed.
- **Fuzz:** wire formats, hub routes, receive path never panic.
- **Chaos:** thousands of accounts through crashes; every message once, in order.
- **Load:** throughput of one hub.

`dev/cargo test --workspace` runs everything.

</details>

## License

[MIT](LICENSE.md). libsignal: [AGPL-3.0](app/core/LICENSE-AGPL.md), in the app under Settings › Licenses.
