<p align="center">
  <img src="docs/yo-app-banner.png" alt="Yo app icon: It's that simple. 1. Pick a username. 2. Tap to send Yo." width="760">
</p>

<p align="center">The 2014 one-tap app, back. Tap a friend's name or a group to send an end-to-end encrypted Yo.<br>
Send photos and videos to view once.</p>

<p align="center"><a href="https://justyo.co"><b>justyo.co</b></a> · <a href="https://testflight.apple.com/join/V3wa1DeX">TestFlight</a></p>

## Apps

- **iOS:** [SwiftUI, iOS 26, Swift 6](app/ios/README.md), available on [TestFlight](https://testflight.apple.com/join/V3wa1DeX).
- **Android:** Kotlin, Jetpack Compose and Material 3; in progress.
- **[YoGPT](app/yogpt):** a Rust client that answers Yos and can reply with pictures.

<details>
<summary>App details, Linux builds and shared core</summary>

The iOS app uses system navigation and controls, with Flat UI rows, Montserrat Bold and Amethyst (#9B59B6).
It includes camera capture, a share extension and notification replies.
Build on Linux with [xtool](https://github.com/xtool-org/xtool); install wirelessly with
`app/ios/dev/push.sh`. See the [build instructions](app/ios/README.md#build-and-run) and [device checks](app/ios/TESTING.md).

Android is being built on the [`android` branch](https://github.com/YoAgain/yo/tree/android), using the same core through UniFFI.

[core/client](core/client) handles identities, sessions, groups, media, recovery and the outbox in Rust.
[core/wire](core/wire) defines protocol formats. [core/bindings](core/bindings) exposes the client to Swift and Kotlin
through UniFFI; apps supply HTTP and storage.

</details>

## Encryption

```text
┌──────────────────┐        ┌──────────────────┐        ┌──────────────────┐
│     Phone A      │        │       Hub        │        │     Phone B      │
│                  │        │                  │        │                  │
│ Signal protocol  │───Yo──►│ sealed envelope  │──push─►│ fetches and      │
│ PQXDH + Double   │        │ sees only the    │        │ decrypts         │
│ Ratchet, sender  │        │ recipient        │        │                  │
│ hidden           │        │ push: no content │        │                  │
└──────────────────┘        └──────────────────┘        └──────────────────┘
```

Sign up with a name only, no phone number or email. Your account is protected by Keychain sync, a passkey (PRF) or a 64-character recovery key. Details: the [YoIP spec](spec/v1.md).

<details>
<summary>Keys</summary>

Where each key lives on iOS, with made-up example values. One device per account; a new sign-in replaces it.

| Key | Where | Example | Purpose |
|---|---|---|---|
| Account key | iCloud, DB | `1cd3276568646ac5113afae6038fd3f745c22a20d2ef2e9a570cf69332a9d1c8` | identity, the account |
| User id | iCloud, DB | `9e4b02bb46c6cf346d6a046281bf768e` | fixed id at the hub |
| Name | iCloud, DB | `Christopher` | to be found |
| Recovery key | iCloud, device | `598t 08e1 7rns vrue br45 hdfl 5jce r5qh m4pl 4mas hhx9 0eyv 9u4e 9jlc s9ym 3qgy` | backup without iCloud |
| Backup status | iCloud | `{"byPasskey":true,"byCode":true,"version":3}` | same security after a restore |
| Ed25519 seed | iCloud | `d00b86d29901e0bfbf0ccf78195e1fd4f0bc1292384807a618e401b09034eaaa` | converted to today's key once |
| State key | device | `485ff0f29cf6d3eaf409470bb4d0751f533498007045f927d7be807476683972` | opens the local DB |
| Device key | DB | `e064d0dbe73a4e9f0a7331fed49b7eb3649f69a163864f2fd67acd8e8d30f026` | this device, encryption |
| Auth key | DB | `2cea6bee0a80939cee23a260a58a95000ee8077b2b60608fdabc0869c4bec8f3` | signs hub requests |
| Prekey | DB | `id 17 → 052a51fc10b0df55bef16d58339e947cab34cff6f6dcaf4e362fc31e5a8ae42e84` | first message while offline |
| Profile key | DB | `1e820bbd1a7b2baa1e1d1f68ff3ed67cc41429a97900d2b0a23f4b68b4a8ea00` | sealed sender for contacts |
| Group key | DB | `5e6bdf11e728b3dbb843f2de720435eac0bff47d73152ab5c85731550d242738` | read and change a group |
| Storage key | derived | `e55cab9cede427cfceca5d012f0c570992a208565ebe2904f5b13691d47e1e5d` | contacts at the hub |
| Session | DB | ratchet state, about 1–2 KB | a new key per message |
| Backup key K | RAM | never stored | encrypts the backup |
| Passkey | Apple Passwords | "Yo – Christopher" | backup by Face ID |

iCloud: one synced iCloud Keychain item per account. Device: this phone's keychain. DB: the local SQLite state,
sealed per column under the state key. Sessions and device keys are never backed up.

</details>

<details>
<summary>Account Sync</summary>

What a new device gets back besides the account ([storage](spec/v1.md#69-storage)), with made-up examples. Sealed
under the storage key, padded to 1, 4, 16 or 60 KiB; every value is `[value, ms]`, so the newer one wins a merge.

| Field | Example | Purpose |
|---|---|---|
| Friend name | `"n": "lina"` | shown in the list |
| Friend colour | `"c": 3` | the row's colour |
| Friend blocked | `"b": false` | a block survives a restore |
| Friend removed | `"r": false` | removed friends stay gone |
| Friends' profile keys | `"k": {"9e4b…": "HoILvRp7K6oeHR9o_z7WfMQUKal5ANKwoj9LaLSo6gA"}` | sealed sender to them |
| Own profile key | `"pk": "Hm4L0Rp7K6oeHR9o_z7WfMQUKal5ANKwoj9LaLSo6gA"` | contacts write to you sealed |
| Group master key | `"k": "XmvfEecos9u4Q_LecgQ16sC_9H1zFSq1yFcxVQ0kJzg"` | read the group at the hub |
| Group joined | `"a": true` | member or request |
| Group left | `"r": false` | left groups stay gone |
| App icon | `"icon": "amethyst"` | the same icon |
| Left-handed | `"left_handed": true` | the same side |
| Version | `"version": 42` | old copies are ignored |

Not synced: sessions, messages, media, list order.

</details>

<details>
<summary>Protocol, groups, media and privacy limits</summary>

[YoIP](spec/v1.md) uses Signal's libsignal: PQXDH and Double Ratchet with SPQR.
Contacts can verify a 60-digit safety number. Messages are padded; backups and account storage are encrypted.
[Recovery](spec/v1.md#7-backup-and-a-new-phone) uses passkey PRF or libsignal's 64-character recovery key.

Sealed sender hides the sender on deliveries to contacts, as shown above. First-contact requests are identified.
The hub sees recipients, lookups and traffic metadata; see the [privacy limits](spec/v1.md#1-goals)
and [threat model](hub/SECURITY.md). Only you and your recipients can read your Yos and media.

The optional Yo-to-Yo Roundtrip probe runs only when tapped. Its operator can see that the account was
active at that moment. No per-user probe history or timing is stored or logged (only aggregate counters;
mandatory encrypted protocol state and volatile rate limiting remain). Probes and their token-matched replies
stay out of requests, notifications, stats and Account Sync, including late replies. The probe uses PINGTEST.
PING is a separate, visible echo contact for Yos, photos and videos. See
[the echo deployment and privacy notes](app/yogpt/README.md).

Groups use zkgroup, with up to 20 members; group names and membership are encrypted too.
Photos and videos are view-once: the app deletes them and their keys after viewing.
The server deletes the encrypted file after its last acknowledged fetch.

</details>

## Hub

| | Yos/s | |
|---|---:|---|
| Hub alone | 10,431 | 1 CPU core · p50 2.1 ms · p99 130 ms |
| End to end | 3,826 | send, deliver, fetch, decrypt |
| Lost | 0 | |

The hub stores and forwards encrypted Yos until the recipient fetches them; it cannot read them.

<details>
<summary>Hub routes</summary>

| Route | Auth |
|---|---|
| `POST /v1/register` | proof (6.4) |
| `GET /v1/username/{name}` | – |
| `PUT /v1/prekeys` | signed |
| `POST /v1/lookup` | signed |
| `PUT /v1/username` | signed |
| `PUT /v1/push` | signed |
| `GET /v1/inbox?wait=s` | signed |
| `GET /v1/certificate` | signed |
| `POST /v1/inbox/ack` | signed |
| `POST /v1/inbox/{user_id}` | none; signed for a request (4.10) or identified (6.1) |
| `PUT /v1/access-key` | signed |
| `GET /v1/group-auth` | signed |
| `POST /v1/group-credential` | signed |
| `PUT`, `POST /v1/groups/{id}` | signed |
| `GET /.well-known/yo-zk-params` | – |
| `PUT /v1/backup` | signed |
| `GET /v1/backup/{user id or locator}` | door key (6.2) |
| `GET`, `PUT /v1/storage` | signed |
| `PUT /v1/blobs/{id}?fetches=n` | signed by body hash |
| `GET /v1/blobs/{id}` | none |
| `DELETE /v1/blobs/{id}` | none; acknowledges storage, the last fetch deletes it |
| `GET /.well-known/yo-hub-key` | – |
| `GET /.well-known/yo-trust-root` | – |
| `POST /v1/report` | signed |
| `POST /v1/feedback` | signed |
| `POST /v1/admin/todos` | shared `Yo-Admin-Token` (`YO_ADMIN_TOKEN`) |
| `POST /v1/crash` | none |
| `DELETE /v1/account` | signed |

Bodies and answers: [spec 6](spec/v1.md).

</details>

<details>
<summary>Storage, durability and load tests</summary>

The small [Rust hub](hub/README.md) stores accounts, public prekeys, queued envelopes and encrypted files.
It has no message content keys. SQLite WAL uses `synchronous = NORMAL`: commits survive a hub/process crash,
but an OS crash or power loss can roll back recent acknowledged deliveries and other state changes. Clients retry
unacknowledged sends; see the [durability tradeoff](hub/SECURITY.md#durability).
The numbers above are medians of three native release runs: hub on CPU 0, load on the other CPUs, fresh SSD/Btrfs
database under `~/.cache`, automatic checkpoints every 16,384 WAL pages (about 64 MiB). The hub reached about
10,400 Yos/s here, short of 20,000; larger checkpoints improve throughput but increase p99 latency.
See the [yo-load measurements and commands](tests/README.md#load).

</details>

## Development

<!-- loc -->
![Lines of code: per day, by part and by app](dev/loc-chart.png)
<!-- /loc -->


<details>
<summary>Tests, folders and contributing</summary>

[Tests](tests/README.md) cover real clients and the hub, groups, media, recovery, security and deterministic simulations.
[Chaos tests](tests/CHAOS.md) exercise faults, crashes and restarts; [fuzz targets](fuzz/fuzz_targets) cover parsers,
hub routes and the receive path. From the repository root, with Docker:

```sh
dev/cargo test --workspace
dev/cargo run --release -p yo-tests --bin yo-load -- --clients 200
dev/fuzz all 300
```

| Folder | Contents |
|---|---|
| [app/ios](app/ios) | iPhone app and build tools |
| [app/yogpt](app/yogpt) | YoGPT bot |
| [core](core) | Rust client, wire formats and bindings |
| [hub](hub) | Rust server |
| [spec](spec) | YoIP protocol |
| [tests](tests), [fuzz](fuzz) | Integration tests, load tools and fuzzing |
| [dev](dev) | Development scripts |

Issues and pull requests are welcome. Start with the [spec](spec/v1.md), follow [AGENTS.md](AGENTS.md),
and run the relevant [checks](tests/README.md). Keep secrets and machine settings out of the repository.

</details>

<details>
<summary><h2>License</h2></summary>

Yo's code is [MIT](LICENSE.md). The app and core link libsignal ([AGPL-3.0](LICENSES/AGPL-3.0.txt)),
so the built app as a whole is distributed under AGPL-3.0; corresponding source is in this repository at the
commit shown in Settings › Licenses. The hub does not link libsignal and is MIT.
The name, logo and sound are outside the code license. [Third-party license texts](LICENSES).

</details>
