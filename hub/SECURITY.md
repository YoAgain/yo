# The hub's security

Routes and their protections: `hub/README.md`. Spec sections: `spec/v1.md`.

**Protects:** availability (disk, memory, connections, push accounts); account integrity (only the account key
registers a device, only a device's auth key speaks for it); what spec 1 says it does not learn.
**Attackers:** anyone through Cloudflare, with as many accounts as registration allows; a user against another; anyone
who learns a user, device or blob id. **Out of scope:** the operator and the machine, Cloudflare, the app's storage,
libsignal itself. **Assumed:** reachable only through the Cloudflare Tunnel when `YO_TRUST_CLOUDFLARE=1`.

## Threats

| Attack | Defence | Residual |
|---|---|---|
| Read who writes to whom from envelopes | Sealed to the recipient's account key, padded inside the seal (6.1); no group or session id anywhere (`tests/tests/sealed.rs`) | address and time (spec 1); Oblivious HTTP later |
| Learn a sender | Unsigned delivery; the payload's sender only inside the seal | as above |
| Swap a contact's keys | Session identity = pinned account key's X25519 form (4.1); the hub delivers no key a client trusts by itself | a lying hub's device list (3.2); key transparency later |
| Hand out other keys between app and hub (TLS broken, a proxy) | Lookups and `409` rosters signed by the hub key the app pins, bound to the client's nonce and question (spec 6.11) | the hub itself (or its key) lying: key transparency later; builds without a pinned key |
| Fill the disk without identity | 64 MiB/IP/hour of delivered bytes; `507` below 1 GiB free; 30-day TTL | many addresses: the floor holds, deliveries stop until cleanup |
| Fill one user's inbox | 5000 per device, then `507` for that recipient only | stranger spam until read; contact access keys later (8) |
| Spend another device's request budget | Counted only after the signature holds | – |
| Hold memory with inbox waits | 4 inbox requests per device at once | – |
| Use up a user's one-time prekeys | Lookups 50/device/day; last-resort Kyber prekey keeps sessions possible; replay of a last-resort message refused (4.1); refill below 16 | first messages then use the last resort (weaker forward secrecy for that message) |
| Forge or replay delivery tokens | Nonce looked up before the P-384 check, spent in the delivery's transaction; bad tokens `401` and counted; > 30/min/IP: no checks | verification cost per forged token (≈ a signature) |
| Tag users by token key | One directory for all | unsigned directory; a key-consistency check (RFC 9576 §6.2) later |
| Tokens from many accounts | 200/device/day; registration 10/IP/hour | bounded by accounts, not addresses |
| Read a backup record | Fetch signed by a door's auth key, else `404` as for none; sealed part padded | – |
| Push to another path on Apple's host | Non-hex APNs tokens cleared | – |
| Spoof `CF-Connecting-IP` | Only with `YO_TRUST_CLOUDFLARE`; warning when not listening on loopback | operator: check the published port |
| Lose an acknowledged delivery to a power cut | SQLite WAL `synchronous = FULL` | – |
| Act as a replaced device | Every write checks the device in its own transaction (`410`) | – |
| Replay a signed request | 5-minute window, cache keyed by signature (`verify_strict`) | the cache is in memory: a restart within 5 min |
| Probe without an account | username taken (by design), device replaced (`410` vs `401`) | device ids are random and unpublished |
| A delivery's `409` names a user's account and device keys | Needed for sealed delivery (4.4); public keys, user ids come from lookups anyway | – |
| Panics on input | 6 fuzz targets (`dev/fuzz`), nightly in CI | – |

## Checked and sound

- Signatures cover method, path with query as received, time, body hash; paths with `..`, `//`, `/`, `%` find no other route.
- Every signed route acts on the signer's own user, device or inbox; a blob needs its 128-bit id.
- SQL: bound parameters only. Bodies: 65 KiB, blobs `Content-Length` ≤ 10 MiB, counted as they come.
- Refusals carry a status and no text; logs carry no user text and no push token.
- Growing tables: `spent_tokens` (with their key), `feedback` (10/user/day), `gone` (a year).

Open: `cargo audit` in CI.
