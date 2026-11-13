# Yo

Yo, again. One tap, one Yo, end-to-end encrypted.

## YoIP: Yo over IP

The protocol, in three parts:

```
YoIP        Yo over IP, the whole protocol
├─ YoLS     Yo Layer Security: who you are, and chats only their members can read
├─ Hub      the server: mailboxes, delivery, push
└─ Content  what a message says: a Yo
```

### YoLS: Yo Layer Security

- **Chats** are MLS groups (RFC 9420), one suite: X25519, ChaCha20-Poly1305, Ed25519.
- **You** are a user id (never changes) with an account key that signs your devices. Phone number and username only
  help others find you.
- **Trust:** any contact can be checked with a 60-digit safety number.
- **Healing:** keys are renewed every week or every 100 messages.

### Hub

- **Sealed sender:** a message is delivered without saying who sent it.
- **All alike:** every message is padded to 1, 4, 16 or 64 KiB; a push says only "there is mail".
- **Order:** the hub settles which change to a chat comes first, without being able to read it.

| The hub knows | The hub never learns |
|---|---|
| phone numbers (as HMAC), devices, IP addresses | who sent a message |
| when and how much arrives | what it says |
| who looks up whom | who chats with whom |

Full text: [spec/v1.md](spec/v1.md).

## Speed

| Hub on one core | Yops | p99 |
|---|---|---|
| database on an SSD | 16,000 | 19 ms |
| database in RAM | 22,000 | 6 ms |

Yops: Yos delivered per second. The one thread that writes them bounds it, not the cores. All runs:
[tests/README.md](tests/README.md#load).

## Layout

```
core/wire       the contract between apps and hub
core/mls        the client core (Rust, one for iOS and Android)
core/bindings   Swift and Kotlin via UniFFI
hub/            the server
tests/          scenarios, simulation, robustness, load
app/ios/        the iPhone app (SwiftUI, a mockup for now)
```

## Run

```sh
dev/cargo test --workspace                                        # needs only Docker
dev/cargo run --release -p yo-tests --bin yo-load -- --clients 200
dev/fuzz all 300                                                  # every fuzz target 5 minutes
```

## License

[O'Saasy](LICENSE.md): use, change and share freely; not as a competing hosted service.
