<p align="center">
  <img src="docs/yo-app-banner.png" alt="Yo app icon: It's that simple. 1. Pick a username. 2. Tap to send Yo." width="760">
</p>

<p align="center">The 2014 one-tap app is back.<br>
Tap a name to send an encrypted Yo, photo or video.</p>

<p align="center">
<a href="app/ios/README.md"><b>iOS</b></a> SwiftUI · <b>Android</b> Compose · <a href="app/cli"><b>CLI</b></a> Rust
</p>

<p align="center"><a href="https://justyo.co"><b>justyo.co</b></a> · <a href="https://testflight.apple.com/join/V3wa1DeX"><b>iPhone beta</b></a> · <a href="https://groups.google.com/g/yo-android-testers"><b>Android beta</b></a></p>

<details>
<summary><h2>Core</h2></summary>

One Rust core runs the whole client. The apps only draw and pass HTTP and storage through.

- [core/client](core/client): identities, sessions, groups, media, recovery, outbox
- [core/wire](core/wire): protocol formats
- [core/bindings](core/bindings): Swift and Kotlin through UniFFI
- [Telemetry](core/README.md#telemetry): every crash, error and support field the apps send, and what the hub counts

The apps on top:

- [app/ios](app/ios): Swift, the iPhone app and its extensions
- [app/android](app/android): Kotlin, the Android app
- [app/cli](app/cli): Rust, the CLI; the bots on it in [skills](skills)

### iOS and Android

The same job on each platform, so both apps behave alike.

| Job | iOS | Android |
|---|---|---|
| Account key | iCloud Keychain | Google Backup |
| Device keys | Keychain | Keystore |
| Push | APNs | FCM |
| UI | SwiftUI | Material 3 |

What the hub knows per endpoint group ([details](spec/v1.md#1-goals)):

| Endpoint group | Endpoints | Authentication |
|---|---|---|
| Messages | `inbox`, `blobs` | anonymous (blob upload: account) |
| Groups | `groups` | account + member proof (names no member) |
| Public | `username`, keys | anonymous |
| Backup | `backup/{id}` | anonymous, with its long secret id |
| Account | `register`, `lookup`, `push`, `storage`, `backup` … | account (signed by its device key) |
| Support and crashes | `help/messages`, `help/read`, `help/report`, `help/crash`, `help/error` | random install id |

### Account Sync (end-to-end encrypted)

What a new phone gets back besides the account, sealed and padded ([spec 6.9](spec/v1.md#69-storage)).
Not synced: sessions, messages, media, list order.

| Group | Fields | Example |
|---|---|---|
| Friend | name, colour, blocked, removed, profile key | `"n": "lina"`, `"c": 3`, `"b": false` |
| Group | master key, joined, left | `"k": "Xmvf…"`, `"a": true` |
| You | own profile key | `"pk": "Hm4L…"` |
| App settings | app icon, left-handed | `"icon": "amethyst"`, `"left_handed": true` |
| Copy | version (older copies are ignored) | `"version": 42` |

</details>

<details>
<summary><h2>Hub</h2></summary>

The relay between phones. It holds encrypted Yos until the recipient fetches them and cannot read them.

| | Yos/s | |
|---|---:|---|
| Hub alone | 10,431 | 1 CPU core · p50 2.1 ms · p99 130 ms |
| Send, one client core | 50 | p50 290 ms · p99 708 ms |
| Lost | 0 | |

Routes: [hub README](hub/README.md). Durability: [SECURITY.md](hub/SECURITY.md#durability). Load runs: [tests](tests/README.md#load).

</details>

<details>
<summary><h2>Encryption</h2></summary>

Only you and your friends can read a Yo. The hub passes it on sealed and sees only the recipient.

```text
Phone A ──Yo──► Hub ──push──► Phone B
Signal          sealed        fetches and
protocol        envelope      decrypts
```

- Signal protocol through libsignal: PQXDH, Double Ratchet, sealed sender
- Sign up with a name only, no phone number or email
- Account protected by iCloud Keychain, a passkey or a 64-character recovery key
- Groups up to 20 with zkgroup; photos and videos are view-once

Limits and details: [YoIP spec](spec/v1.md), [threat model](hub/SECURITY.md).

### Keys

Where each key lives on iOS, with made-up examples. iCloud: the synced Keychain item. Device: this phone's keychain.
DB: the local database, sealed per column.

| Key | Where | Example | Purpose |
|---|---|---|---|
| Account key | iCloud, DB | `1cd32765…32a9d1c8` (64 chars) | identity, the account |
| User id | iCloud, DB | `9e4b02bb46c6cf346d6a046281bf768e` | fixed id at the hub |
| Name | iCloud, DB | `Christopher` | to be found |
| Recovery key | iCloud, device | `598t 08e1 … s9ym 3qgy` (64 chars) | backup without iCloud |
| Backup status | iCloud | `{"byPasskey":true,"byCode":true,"version":3}` | same security after a restore |
| Ed25519 seed | iCloud | `d00b86d2…9034eaaa` (64 chars) | converted to today's key once |
| State key | device | `485ff0f2…76683972` (64 chars) | opens the local DB |
| Device key | DB | `e064d0db…8d30f026` (64 chars) | this device, encryption |
| Auth key | DB | `2cea6bee…c4bec8f3` (64 chars) | signs hub requests |
| Prekey | DB | `id 17 → 052a51fc…8ae42e84` (66 chars) | first message while offline |
| Profile key | DB | `1e820bbd…b4a8ea00` (64 chars) | sealed sender for contacts |
| Group key | DB | `5e6bdf11…0d242738` (64 chars) | read and change a group |
| Storage key | derived | `e55cab9c…d47e1e5d` (64 chars) | contacts at the hub |
| Session | DB | ratchet state, about 1–2 KB | a new key per message |
| Backup key K | RAM | never stored | encrypts the backup |
| Passkey | Apple Passwords | "Yo – Christopher" | backup by Face ID |

</details>

<details>
<summary><h2>Development</h2></summary>

<!-- loc -->
![Lines of code: per day, by part and by app](dev/loc-chart.png)
<!-- /loc -->

- [app](app)
  - [ios](app/ios): the iPhone app ([YoApp](app/ios/YoApp)), its extensions and [build tools](app/ios/dev)
  - [android](app/android): the Android app
  - [cli](app/cli): the CLI, `yo`
- [skills](skills): the bots (YoGPT, PING, PINGTEST, STATUS) as scripts driving the CLI; [skills/status](skills/status) sends the server's numbers as a picture at the rhythm set on the admin page
- [core](core)
  - [client](core/client): state, sessions, groups, media, recovery, outbox
  - [wire](core/wire): protocol formats
  - [product](core/product): the apps' rules and texts (en, de)
  - [bindings](core/bindings): Swift and Kotlin through UniFFI
- [hub](hub): the Rust server, with its [routes](hub/README.md) and [threat model](hub/SECURITY.md)
- [admin](admin): the admin page
- [spec](spec): the YoIP protocol, [v1.md](spec/v1.md)
- [tests](tests): integration, chaos and load tests, with [fixtures](tests/fixtures)
- [fuzz](fuzz): fuzz targets
- [dev](dev): Docker builds, this chart
- [update-deps.sh](update-deps.sh): every dependency to its newest release, tested, one commit

Workflows ([.github/workflows](.github/workflows)):

- **iOS**: push → internal TestFlight · RC release (`v1.0.0-RC`) → public TestFlight · stable release → App Store (by hand for now)
- **Android**: push → build · RC release → test build · stable release → Play Store (by hand for now)
- **CLI**: push → binary artifact · release → binary attached to the release
- **Release**: `dev/release.sh 1.0.0-RC37`, or the button Actions → Release: bumps [version.properties](app/android/version.properties) in a signed commit on main and publishes `v1.0.0-RC37`, which runs the rows above; F-Droid picks up the tag by itself ([Android](app/android/README.md#releases-and-f-droid))
- **Test**: every push → Rust tests, lint · **Fuzz**: push → 1 min, nightly → 15 min · **Dependencies**: monthly → pull request

```sh
dev/cargo test --workspace     # tests, with Docker
dev/fuzz all 300               # fuzzing
```

[Tests](tests/README.md) · [chaos tests](tests/CHAOS.md) · [spec](spec/v1.md) · [AGENTS.md](AGENTS.md). Pull requests welcome.

</details>

<details>
<summary><h2>License</h2></summary>

Code: [MIT](LICENSE.md). The built app links libsignal and is [AGPL-3.0](LICENSES/AGPL-3.0.txt); its source is the commit
shown in Settings › Licenses. Name, logo and sound are not covered. [Third-party licenses](LICENSES).

</details>
