# Yo

The 2014 one-tap app, back. Tap a friend's name. They get “Yo”. It's that simple.

- One tap for a friend or a whole group.
- End-to-end encrypted with Signal's libsignal.
- Photos and videos, seen once. YoGPT to Yo back.

**[Get Yo on TestFlight](https://testflight.apple.com/join/V3wa1DeX)** · [justyo.co](https://justyo.co)

iOS now. App Store soon. Android in progress.

<img src="app/ios/YoApp/AppIcon.png" alt="Yo app icon" width="160">

## How to use

Pick a name, add a friend, tap their row. In camera mode, tap for a photo or hold for a video.
Add **YoGPT** if you want a Yo back.

## How it works

- **Just a name:** no phone number or email needed. Share your invite link or QR code.
- **Private:** only you and your recipients can read your Yos and media. Push notifications carry no message content.
- **Groups:** Yo everyone at once, with up to 20 members. Group names and membership are encrypted too.
- **Photos and videos:** view once, then the app deletes them and their keys. The server deletes the encrypted file after its last allowed fetch.
- **Recovery:** keep a passkey or your recovery key to bring your account back. On iPhone, your account key also syncs through iCloud Keychain.
- **Your choice:** accept requests, block or report someone, or delete your account in the app.

## Under the hood

<details>
<summary><b>Encryption &amp; protocol</b> · Signal sessions, sealed sender, private groups</summary>

[YoIP](spec/v1.md) uses libsignal's PQXDH and Double Ratchet with SPQR, sealed sender, and zkgroup for groups.
Contacts can verify a 60-digit safety number. Messages are padded; backups and account storage are encrypted.

Sealed sender hides the sender on deliveries to contacts. First-contact requests are identified. The hub sees
recipients, lookups and traffic metadata; see the [privacy limits](spec/v1.md#1-goals) and [threat model](hub/SECURITY.md).
[Recovery](spec/v1.md#7-backup-and-a-new-phone) uses passkey PRF or libsignal's 64-character recovery key.

</details>

<details>
<summary><b>Core</b> · one Rust implementation, Swift and Kotlin bindings</summary>

[core/client](core/client) handles identities, sessions, groups, media, recovery and the outbox.
[core/wire](core/wire) defines the protocol formats. [core/bindings](core/bindings) exposes the client through UniFFI;
apps supply HTTP and storage. [YoGPT](app/yogpt) is a Rust client that answers Yos and can reply with pictures.

</details>

<details>
<summary><b>Hub (server)</b> · mailboxes, encrypted files, push</summary>

The small [Rust hub](hub/README.md) stores accounts, public prekeys, queued envelopes and encrypted files.
It has no message content keys. SQLite persists each delivery before acknowledging it; measured throughput is
about 8,000 deliveries per second on one core ([load tests](tests/README.md#load)).

</details>

<details>
<summary><b>Speed</b> · the hub on one core</summary>

Hub, measured with [yo-load](tests/README.md#load) on one CPU core, database on an SSD with every delivery on disk
before it is acknowledged (`synchronous = FULL`):

| | |
|---|---|
| Deliveries, hub alone | about 8,000 Yos/s (p50 7.7 ms, p99 20 ms) |
| End to end (send, deliver, fetch, decrypt) | about 700 Yos/s |
| Lost under load | 0 |

Without the wait for the disk the hub does about 15,000 Yos/s; that trade is deliberate: an acknowledged Yo survives a
power cut.

</details>

<details>
<summary><b>iOS app</b> · SwiftUI, built on Linux with xtool</summary>

[SwiftUI, iOS 26, Swift 6](app/ios/README.md), using system navigation and controls.
Flat UI rows, Montserrat Bold and Amethyst (#9B59B6) keep the Yo look.
The app includes camera capture, a share extension and notification replies.

Build on Linux with [xtool](https://github.com/xtool-org/xtool); install wirelessly with
`app/ios/dev/push.sh`. See the [build instructions](app/ios/README.md#build-and-run) and [device checks](app/ios/TESTING.md).

</details>

<details>
<summary><b>Android</b> · in progress</summary>

A Kotlin, Jetpack Compose and Material 3 app on the same [core](core/client) through its
[UniFFI bindings](core/bindings) is being built on the `android` branch.

</details>

<details>
<summary><b>Tests &amp; fuzzing</b> · protocol, recovery, failures, load</summary>

[Tests](tests/README.md) cover real clients and the hub, groups, media, recovery, security and deterministic simulations.
[Chaos tests](tests/CHAOS.md) exercise faults, crashes and restarts; [fuzz targets](fuzz/fuzz_targets) cover parsers,
hub routes and the receive path. From the repository root, with Docker:

```sh
dev/cargo test --workspace
dev/cargo run --release -p yo-tests --bin yo-load -- --clients 200
dev/fuzz all 300
```

</details>

<details>
<summary><b>Repository layout</b> · where things live</summary>

| Folder | Contents |
|---|---|
| [app/ios](app/ios) | iPhone app and build tools |
| [app/yogpt](app/yogpt) | YoGPT bot |
| [core](core) | Rust client, wire formats and bindings |
| [hub](hub) | Rust server |
| [spec](spec) | YoIP protocol |
| [tests](tests), [fuzz](fuzz) | Integration tests, load tools and fuzzing |
| [dev](dev) | Development scripts |

</details>

<details>
<summary><b>Contributing / License</b> · source, checks, third-party code</summary>

Issues and pull requests are welcome. Start with the [spec](spec/v1.md), follow [AGENTS.md](AGENTS.md),
and run the relevant [checks](tests/README.md). Keep secrets and machine settings out of the repository.

Yo's own source is [MIT](LICENSE.md). The name, logo and sound are outside the code license.
The app and core link libsignal ([AGPL-3.0](LICENSES/AGPL-3.0.txt)); the built app as a whole is distributed under
AGPL-3.0, with corresponding source in this repository at the commit shown in Settings › Licenses.
The hub does not link libsignal and is MIT. Third-party license texts are in [LICENSES](LICENSES).

</details>
