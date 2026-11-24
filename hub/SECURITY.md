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
| Swap a contact's keys | Session identity = pinned account key (4.1); the hub delivers no key a client trusts by itself | a lying hub's device list (3.2); key transparency later |
| Hand out other keys between app and hub (TLS broken, a proxy) | Lookups and `409` rosters signed by the hub key the app pins, bound to the client's nonce and question (spec 6.10) | the hub itself (or its key) lying: key transparency later; builds without a pinned key |
| Fill the disk without identity | 64 MiB/IP/hour of delivered bytes; `507` below 1 GiB free; 30-day TTL | many addresses: the floor holds, deliveries stop until cleanup |
| Fill one user's inbox | 5000 per device, then `507` for that recipient only; strangers' deliveries 20/recipient/day, none with requests off (spec 4.10) | a contact's spam (block: the key rotates); scarce request tokens later (8) |
| Tell contacts apart by the access key | One profile key per account (Signal's), its access key the same for every contact; the hub keeps a SHA-256 of the access key only, compared in constant time (4.10) | the hub learns a delivery is a contact's, and a key rotation after a block |
| Reach anyone through a group of one's own making | An invited member (no key in its entry) gets no endorsement and is endorsed for no one (4.9): deliveries to them are requests, signed and counted (4.10); with requests off a stranger's group and its Yos are dropped unseen | tokens issued before 2026-11-24, for their 2–3 days |
| Spend another device's request budget | Counted only after the signature holds and is not a replay | – |
| Hold every reading place with slow bodies | 64 of the 512 per address (`503` for it alone), 20 s per body | many addresses |
| Hold memory with inbox waits | 4 inbox requests per device at once | – |
| Use up a user's one-time prekeys | Lookups 50/device/day; last-resort Kyber prekey keeps sessions possible; replay of a last-resort message refused (4.1); refill below 16 | first messages then use the last resort (weaker forward secrecy for that message) |
| Read a backup record | Fetch signed by a door's auth key, else `404` as for none; sealed part padded | – |
| Push to another path on Apple's host | Non-hex APNs tokens cleared | – |
| Use the admin token from the internet | Behind Cloudflare the admin routes answer loopback without `CF-Connecting-IP` only (the admin page); else `404` | a process on the server |
| Spoof `CF-Connecting-IP` | Only with `YO_TRUST_CLOUDFLARE`; warning when not listening on loopback | operator: check the published port |
| Lose recent commits to an OS crash or power cut | SQLite WAL `synchronous = NORMAL`; process crashes preserve commits | Recent acknowledged deliveries and other state changes can roll back; see durability below |
| Act as a replaced device | Every write checks the device in its own transaction (`410`) | – |
| Replay a signed request | 5-minute window, cache keyed by signature; a signature has one encoding (its scalar reduced, `yo_wire::xeddsa_verify`) | the cache is in memory: a restart within 5 min |
| Probe without an account | username taken (by design), device replaced (`410` vs `401`) | device ids are random and unpublished |
| A delivery's `409` names a user's account and device keys | Needed for sealed delivery (4.4); public keys, user ids come from lookups anyway | – |
| Panics on input | 6 fuzz targets (`dev/fuzz`), nightly in CI | – |

## Checked and sound

- Signatures cover method, path with query as received, time, body hash; paths with `..`, `//`, `/`, `%` find no other route.
- Every signed route acts on the signer's own user, device or inbox; a blob needs its 128-bit id.
- SQL: bound parameters only. Bodies: 65 KiB, blobs `Content-Length` ≤ 10 MiB, counted as they come.
- Refusals carry a status and no text; logs carry no user text and no push token.
- Growing tables: `feedback` (10/user/day), `gone` (a year), `diagnostics` (8 MiB/IP/hour, `507` below the disk floor, 90 days),
  `zk_groups_gone` (an ended group's id, 40 bytes, kept for good; bound by the groups made a day).
- In-memory buckets: a delivery to a user that does not exist is `404` before any is made for it; a request's
  tokens are taken before it is stored and given back if it is not (requests sent at once share them).

Open: `cargo audit` in CI.

## Durability

The owner's throughput choice is SQLite WAL with `synchronous = NORMAL`, without an fsync per commit.
Committed transactions survive a hub/process crash. An OS crash or power loss can roll back recent commits;
SQLite keeps the database consistent. See [SQLite's synchronous documentation](https://www.sqlite.org/pragma.html#pragma_synchronous).

Clients persist their outbox before sending. Without a `202` response they keep the send and retry on the next
flush/sync, including after a client restart; duplicate envelopes are skipped by the recipient. This is checked by
`tests/tests/robustness.rs::an_unacknowledged_send_is_retried_after_restart`. Once `202` is received the send leaves
the outbox: an acknowledged envelope subsequently lost to power failure is not automatically resent. Signal's
ratchet tolerates a missing message; the missing message itself is lost.

**A lost acknowledged envelope is not the only possible effect.** NORMAL applies to the whole database: recent
registrations, device/key changes, consumed one-time prekeys, backups, sealed storage and blob metadata can also
roll back. A rolled-back inbox ack can redeliver an envelope (the client skips duplicates); rolled-back prekey
consumption can hand out a prekey again, and account/device changes can revert. Client send retries do not repair
all of these state changes. The protocol and libsignal processing are unchanged, but power-loss durability is
weaker than FULL. NORMAL is not a promise that every acknowledged operation survives a machine failure.
