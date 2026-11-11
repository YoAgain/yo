# Yo

Yo, again. End-to-end encrypted with MLS (RFC 9420); the hub never learns who sends a Yo.

The protocol is a full messenger's; the v1 product sends one thing, a Yo. See [spec/v1.md](spec/v1.md).

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
