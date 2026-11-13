# Yo

Yo, again. One tap, one Yo, end-to-end encrypted.

## YoIP: Yo over IP

The protocol: MLS (RFC 9420) for a messenger whose server never learns who sends. Its security layer, identities
and chats, is **YoLS** (Yo Layer Security); YoIP is all of it, with the hub and what a message says.

- **Identity** in four layers:

  | Layer | What | Changes? |
  |---|---|---|
  | User id | 16 bytes from the hub | never |
  | Account key | signs the account's devices | with a new phone, unless a passkey keeps it |
  | Devices | MLS members, each with its own keys | added and replaced |
  | Handles | phone number (SMS), username | any time; only to find you |

- **Certificate** (the MLS credential, 96 bytes): account key + its signature over the device key. Names no person;
  who an account key is, the hub attests. Any contact can be verified (60-digit safety number).
- **Chats:** MLS groups, one suite (X25519, ChaCha20-Poly1305, Ed25519), typed content (`Yo` now, text later).
- **Sealed sender:** delivering takes no identity; addressed to a user, copied to all their devices; envelopes padded
  to fixed sizes; pushes say only "there is mail".
- **Healing:** keys renewed weekly or every 100 messages; the hub orders commits by epoch tokens only members hold.
- **The hub knows** numbers (as HMAC), devices, timing, sizes, IPs, who looks up whom. Not who sends, what, or
  who chats with whom.

Full text: [spec/v1.md](spec/v1.md).

## Layout

```
core/wire       contract between apps and hub
core/mls        client core (Rust, one for iOS and Android)
core/bindings   Swift and Kotlin via UniFFI
hub/            mailboxes, KeyPackages, commit order, push, SMS
tests/          scenarios, simulation, robustness, load
app/ios/        the iPhone app (SwiftUI; a mockup for now, see app/ios/README.md)
```

## Speed

| Hub on one core | Yops | p99 |
|---|---|---|
| database on an SSD (as on a server) | 16,000 | 19 ms |
| database in RAM (tmpfs, or SQLite `:memory:`) | 22,000 | 6 ms |

Yops: Yos delivered per second, the only benchmark unit that says what it measures. The bound is the one writer
thread (every delivery is a write, one after the other), not SQLite and not the cores: more cores do not help, SQLite
in memory is no faster than a file in RAM, and a real disk costs about a third. All runs:
[tests/README.md](tests/README.md#load).

## Run

```sh
dev/cargo test --workspace                                        # needs only Docker
dev/cargo run --release -p yo-tests --bin yo-load -- --clients 200
dev/fuzz all 300                                                  # cargo-fuzz, every target 5 minutes
```

Tests and measured throughput: [tests/README.md](tests/README.md). Hub configuration: [hub/src/main.rs](hub/src/main.rs).

## License

[O'Saasy](LICENSE.md): use, change and share freely; not as a competing hosted service.
