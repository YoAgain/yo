# Yo

**One tap, one Yo — end-to-end encrypted, and the server never learns who sent it.**

## Stack

### YoIP: Yo over IP

YoIP is the protocol between the apps and the hub: who you are, how a Yo is delivered, and what it says. Messages
reach the hub without a sender, padded to one of four sizes; a push only says "there is mail".

Its security layer is **YoLS**, Yo Layer Security: chats are MLS groups (RFC 9420), keys renew every week or 100
messages, and any contact can be checked with a 60-digit safety number. Full text: [spec/v1.md](spec/v1.md); every
key, where it lives and what it opens: [spec 7, the keys at a glance](spec/v1.md#7-backup-and-a-new-phone).

### Hub

The server: a mailbox per device, the order of changes to a chat, and the push that wakes a phone. One core
delivers about 16,000 Yops, Yos per second ([measured](tests/README.md#load)).

### Apps

**iPhone** ([app/ios](app/ios/README.md)), SwiftUI on the Rust core (UniFFI):

- **Sign up with a name only:** no phone number, no e-mail. Names keep their spelling ("YoGPT") and are unique
  by their look (yogpt, YOGPT and Y0GPT are one name).
- **The list:** tap a friend, a Yo goes; their Yos land on their bar. Dozens of taps on dozens of friends while Yos
  come in; a burst over the hub's limit waits on the phone and goes later, in order.
- **Photos and videos:** camera mode, tap a friend for a photo, hold for a video; end-to-end encrypted, seen once,
  gone at the hub once fetched.
- **Push:** a Yo wakes the phone through APNs; the push carries nothing but "there is mail".
- **Your account comes back:** its key is kept in the iCloud Keychain (end-to-end encrypted by Apple), so a new
  iPhone or a reinstall signs straight back in. A passkey (Face ID) and a recovery key (12 words) bring it back
  beyond Apple.
- **Safety:** report and block on a friend's bar, delete your account (and everything the hub holds) in the app.
- **Profile:** your code (QR), statistics counted on the phone, the app icon (Appearance).

**YoGPT** ([app/yogpt](app/yogpt)): a real client that answers every Yo, keeping your rhythm, and now and then with
a picture; one process for its account on each hub.

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
