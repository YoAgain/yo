# Yo

**One tap, one Yo — end-to-end encrypted, and the server never learns who sent it.**

## Stack

### YoIP: Yo over IP

YoIP is the protocol between the apps and the hub: who you are, how a Yo is delivered, and what it says. Messages
reach the hub without a sender, padded to one of four sizes; a push only says "there is mail".

Its security layer is Signal's protocol through libsignal: PQXDH and the Double Ratchet with SPQR, quantum-safe, healing
with every answer, and any contact can be checked with a 60-digit safety number. Full text: [spec/v1.md](spec/v1.md); every
key, where it lives and what it opens: [spec 7, the keys at a glance](spec/v1.md#7-backup-and-a-new-phone).

### Hub

The server: a mailbox per device, each device's prekeys, and the push that wakes a phone. One core
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

Yo's own source is [MIT](LICENSE.md), like the original Yo's `yo-ios` and `yo-api`. The name Yo, its logo and its
sound are not covered by the code license.

**Third-party: libsignal.** The iOS app and its core (`core/`) link [libsignal](https://github.com/signalapp/libsignal)
(AGPL-3.0, © Signal Messenger, LLC; [text](LICENSES/AGPL-3.0.txt)). Yo's own source is MIT; the built app as a whole
is distributed under the AGPL-3.0, its complete corresponding source being this repository at the commit the app
shows under Settings › Licenses. The hub (`hub/`) does not link libsignal and is MIT. Signal has stated it has no
objection to App Store distribution of GPL-compliant apps ([2016](https://signal.org/blog/license-update/)); an
explicit App Store additional permission for libsignal is proposed in
[signalapp/libsignal#705](https://github.com/signalapp/libsignal/pull/705) (see #684).
