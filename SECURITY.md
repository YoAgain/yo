# Security

Design and threat model: [yoip/SPEC.md](yoip/SPEC.md).

## Protocol

- **Encryption:** libsignal (PQXDH, Double Ratchet), sealed sender, zkgroup groups. [§3](yoip/SPEC.md#3-session)
- **Hub sees:** recipient inbox, time, padded size, IP, lookups. [§1](yoip/SPEC.md#1-overview)
- **Cloudflare** (TLS proxy in front of the hub) sees what the hub sees, never content. Planned: Oblivious HTTP, so it sees only IP, time and size ([#8](https://github.com/YoAgain/yo/issues/8)).
- **Key change:** sending to a verified contact stops until you confirm the new key. [§2](yoip/SPEC.md#2-identity)

## App

- **Sign-up:** name only, no phone or email.
- **Keys:** iOS Keychain, Android Keystore.
- **Backup:** account key, sealed on the hub; opens with passkey or recovery key. [§2.4](yoip/SPEC.md#24-recovery)
- **Attestation:** App Attest, Play Integrity; measured, not enforced. [§4](yoip/SPEC.md#4-net)

## Server

- **Hub:** message content only as ciphertext; deleted on ack, else after 30 days. [§4](yoip/SPEC.md#4-net)
- **Admin:** Cloudflare Access + login, read-only.
- **Services:** **not end-to-end** with the service; hashed codes, last 50 items ≤ 7 days. [Services](README.md#services)

## Reporting

[Private vulnerability reporting](https://github.com/YoAgain/yo/security/advisories/new): part, version, steps. No bounty.
Only the latest release gets fixes.
