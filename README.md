# Yo

Yo, again. End-to-end encrypted with MLS (RFC 9420).

## Layout

```
core/         Rust: MLS client core, shared by iOS and Android
wire/         Rust: formats and API types shared by core and hub
hub/          Mailbox relay and push (APNs, FCM)
app/ios/      Swift + UniFFI
app/android/  Kotlin + UniFFI
spec/         Protocol specification
```

## v1 scope

- 1:1 chats only (an MLS group of two)
- One device per account
- Usernames for discovery, anonymous mailboxes for delivery (sealed sender)

See [spec/v1.md](spec/v1.md).
