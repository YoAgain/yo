# Yo

**One tap, one Yo — end-to-end encrypted, and the server never learns who sent it.**

## Stack

### YoIP: Yo over IP

YoIP is the protocol between the apps and the hub: who you are, how a Yo is delivered, and what it says. Messages
reach the hub without a sender, padded to one of four sizes; a push only says "there is mail".

Its security layer is **YoLS**, Yo Layer Security: chats are MLS groups (RFC 9420), keys renew every week or 100
messages, and any contact can be checked with a 60-digit safety number. Full text: [spec/v1.md](spec/v1.md).

### Hub

The server: a mailbox per device, the order of changes to a chat, and the push that wakes a phone. One core
delivers about 16,000 Yops, Yos per second ([measured](tests/README.md#load)).

## Dev

<details markdown="1">
<summary>Test, load, fuzz</summary>

```sh
dev/cargo test --workspace                                          # needs only Docker
dev/cargo run --release -p yo-tests --bin yo-load -- --clients 200  # load test
dev/fuzz all 300                                                    # every fuzz target 5 minutes
```

</details>

## License

[O'Saasy](LICENSE.md): use, change and share freely, but not as a competing hosted service.
