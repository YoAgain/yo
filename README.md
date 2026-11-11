# Yo

Yo, again. End-to-end encrypted with MLS (RFC 9420); the hub never learns who sends a Yo.

The protocol is a full messenger's; the v1 product sends one thing, a Yo. See [spec/v1.md](spec/v1.md).

## The protocol in one page

The full text is [spec/v1.md](spec/v1.md). In short:

**Who you are.** You register with your phone number and an SMS code. Your device makes an *account key* (who you
are) and *device keys* (this phone). The account key signs a certificate for each device: number, account key,
device key, mailbox. That certificate is the device's MLS credential, so everyone in a chat can check every member.
The hub vouches that a number belongs to an account key; apps pin it on first contact and say so when it changes.
Two people can compare a 60-digit safety number. With a passkey (later) the account key is backed up and the
identity survives a new phone.

**How a Yo travels.** A chat is an MLS group (RFC 9420, one suite: X25519, ChaCha20-Poly1305, Ed25519). To start one
you look the number up at the hub and get one KeyPackage per device of that person; you create the group and send
the Welcome. A message is an MLS `PrivateMessage` whose content is typed, never free bytes: `Yo` today, `Text`,
`Link`, `Location` reserved. Everything that leaves a device is padded into fixed buckets (1, 4, 16, 64 KiB), so the
hub sees a size class, not whether it is a Yo, a commit or a Welcome.

**Sealed sender.** Delivering into a mailbox takes no identity: the hub stores an envelope for a mailbox and wakes
the device with a push that says only "there is mail". The sender is inside the encryption. Fetching your own
mailbox is signed with your device's auth key.

**Healing.** Every device renews its keys with an MLS update after 7 days or 100 messages, so a stolen phone state
stops working (post-compromise security). Commits need one order: the hub is the referee. It stores, per group, the
epoch and the hash of an *epoch token* that only members can derive (MLS exporter). A commit is accepted only with
the current token; a race loses with `409` and tries again on top of the winner. The hub learns that some random
group id moved on, not who moved it.

**What the hub knows.** Which number (as an HMAC) has which devices, when a mailbox gets mail and in which size
bucket, IP addresses, and who looks up whom. Not: who sends, what, or who is in a chat with whom.

**Getting it right on bad networks.** Outbox before sending, save before acking, receive before sending, repeat a
commit whose answer was lost (the hub recognises it), keep reading a chat whose peer changed phones for 30 days. The
tests in [tests/](tests/README.md) break each of these on purpose.

## Layout

```
core/
├── wire/       Contract between apps and hub: formats, limits, error codes, certificate, signed request
├── mls/        The client core: identities, MLS groups, outbox, delivery (crate yo-core)
└── bindings/   UniFFI for Swift and Kotlin (library `yo`)
hub/            Mailboxes, KeyPackages, commit order, push (APNs, FCM), SMS (Twilio Verify)
app/ios/        Swift (to come)
app/android/    Kotlin (to come)
spec/           Protocol specification
tests/          Scenarios, simulation, push, bindings, load; results in tests/results/
dev/            Toolchain: dev/cargo runs cargo in the pinned Rust image
```

`hub/` depends on `core/wire` only.

## v1

Protocol complete, product minimal.

- Phone number + SMS to register and to be found
- MLS with regular updates (post-compromise security) and commit ordering by the hub, proven by epoch tokens
- Account key and device keys separated from day one; safety numbers
- Sealed sender: delivering and committing take no identity
- Content is typed (`Yo` today; `Text`, `Link`, `Location` reserved); envelopes padded to fixed buckets
- Optional passkey later: key backup, more devices, verification, registration lock

## Build, test, measure

Only Docker is needed: `dev/cargo` runs cargo in the pinned toolchain.

```sh
dev/cargo test --workspace
dev/cargo run --release -p yo-tests --bin yo-load -- --clients 200 --seconds 15
```

[tests/README.md](tests/README.md) lists what the tests show and the measured throughput.

## Bindings

```sh
dev/cargo build -p yo-bindings
dev/cargo run -p yo-bindings --bin uniffi-bindgen -- generate --library target/debug/libyo.so --language swift --out-dir target/bindings/swift
dev/cargo run -p yo-bindings --bin uniffi-bindgen -- generate --library target/debug/libyo.so --language kotlin --out-dir target/bindings/kotlin
```

The app implements `HttpTransport` (requests to the hub) and `StateStore` (one sealed blob; the 32-byte key lives in
the Keychain or Keystore), and calls `YoClient` off the main thread.

## Running the hub

```sh
YO_PHONE_SECRET=$(openssl rand -hex 32) YO_SMS=twilio YO_TWILIO_ACCOUNT_SID=… YO_TWILIO_AUTH_TOKEN=… \
YO_TWILIO_SERVICE_SID=… YO_APNS_KEY_FILE=… YO_APNS_KEY_ID=… YO_APNS_TEAM_ID=… YO_APNS_TOPIC=… \
YO_FCM_SERVICE_ACCOUNT_FILE=… YO_TRUST_CLOUDFLARE=1 yo-hub
```

Every variable is described at the top of [hub/src/main.rs](hub/src/main.rs). Without `YO_SMS` the hub accepts the
code `000000` for every number: development only.
