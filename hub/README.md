# yo-hub

The Yo hub (spec 6): users and their devices, KeyPackages, delivery to users, the order of commits, push, blobs. It
holds no content key and never learns who sends a message. Configuration is read from the environment
(`src/main.rs` lists every variable); `hub/SECURITY.md` has the threat model and the review's findings.

## Routes

Every route the hub serves, from `Hub::router` in `src/lib.rs`. Anything else is `404` (unknown path) or `405`
(known path, other method), with an empty body. Bodies are TLS-encoded structs (`core/wire/src/api.rs`).

**Signed** means a device's request signature (spec 6.2): headers `Yo-Device`, `Yo-Time`, `Yo-Signature`, an
Ed25519 signature by the device's auth key over `"yo request v1"`, the method, the path **with its query, exactly as
sent**, the time and the SHA-256 of the body. The hub accepts a time within 5 minutes of its clock and each
signature once (an in-memory replay cache over that window). A signature that holds then costs one of the device's
1200 signed requests per minute; one that does not costs nothing of the device's. An unknown device is `401`, a
replaced or deleted one `410`. **Account-signed** is a signature by the account key, carried in the body (no time:
it is bound to fresh device keys and KeyPackages the hub has never seen, so a replay is refused).

Limits are token buckets (refilled evenly) in memory, per the key named; **IP** is the client address
(`CF-Connecting-IP` when `YO_TRUST_CLOUDFLARE=1`, else the socket's peer), an IPv6 address by its /64. Every body
is at most 65 KiB (`MAX_BODY`) except a blob upload, which is streamed.

Every request may name its app in `Yo-App: PLATFORM/BUILD` (spec 6.3): the hub keeps it per device (written when it
changes) and answers `426` to every request of a build older than `YO_MIN_APP` (`ios/202611140619,android/12`) for
its platform.

| Method | Path | Purpose | Auth | Limits (key) | Hub learns |
|---|---|---|---|---|---|
| GET | `/` | a page that says what this is | none | – | nothing |
| GET | `/.well-known/apple-app-site-association` | apps allowed to make passkeys for the domain (spec 7); only when `YO_APPLE_APP_IDS` is set | none | – | nothing |
| POST | `/v1/register` | username + certificate + KeyPackages → device registered, user id (spec 6.4) | none, account-signed proof over username, device id, device key, auth key | 10/h (IP) | name, account key, device keys, push token |
| GET | `/v1/username/{name}` | is the name free: `204`, `409` taken or reserved, `400` no username | none | 60/min (IP) | the name asked |
| GET | `/v1/backup/{user id or locator}` | the backup record, encrypted (spec 7); `404` none, and to any request not signed by one of its doors' auth keys; a record from before the auth keys unsigned until 2026-11-28 | signed by a door's key (`Yo-Time`, `Yo-Signature`) | 10/h (IP) | which record is fetched, from where, by which door |
| PUT | `/v1/backup` | store the account's backup record with its doors' auth keys | signed | signed | the record (opaque, its sealed part padded to 1, 4, 16 or 60 KiB), its locator, the doors' public auth keys |
| PUT | `/v1/keypackages` | add KeyPackages (each checked, spec 6.5) | signed | signed; ≤ 100 per upload, ≤ 100 held | – |
| POST | `/v1/lookup` | handle → user id, account key, device keys, KeyPackages if asked, username if asked | signed | 50/day with KeyPackages, 1000/day without (device) | who looks up whom |
| PUT | `/v1/push` | set the device's push token (≤ 512 bytes) | signed | signed | the token |
| PUT | `/v1/username` | change or give up the account's username | signed | signed | the name |
| DELETE | `/v1/account` | delete the account and everything about it (spec 6.4) | signed | signed | – |
| POST | `/v1/report` | report a user (reason 1–3); `404` no such user | signed | 20/day (user) | who reports whom, why |
| POST | `/v1/feedback` | feedback text (1–2000 bytes) and app version for the operator | signed | 10/day (user) | the text, plain |
| GET | `/v1/storage` | the account's storage, sealed by its devices (spec 6.9); `404` none yet | signed | 120/h (user, shared with the next row) | when it is read |
| PUT | `/v1/storage` | replace it if the request names the version stored now, else `409` with the current one; ≤ 60 KiB, not empty | signed | 120/h (user, shared) | its size (opaque, padded to 1 KiB), when it changes |
| GET | `/v1/inbox?wait=s` | up to 100 waiting envelopes, oldest first; waits up to 60 s if none | signed | signed; 4 under way at once (device) | when a device reads |
| POST | `/v1/inbox/ack` | delete envelopes by id (≤ 1000), only the device's own | signed | signed | – |
| POST | `/v1/inbox/{user id}` | deliver an envelope to every device of a user, `Yo-Devices` checked; `409` with the current account key and device keys | **none** (sealed sender) | 200/min and 64 MiB/h (IP); 5000 waiting per device; `507` disk floor | the recipient, the bucket size, the address |
| POST | `/v1/groups` | register a group: id, epoch, token hash | **none** | 60/h (IP); `507` disk floor | a random group id |
| POST | `/v1/groups/{id}/commit` | order a commit and keep its envelope (spec 4.3) | **none**, the epoch's token | 600/min (IP, shared with the next row) and 64 MiB/h (IP, shared with deliveries); `507` disk floor | that a group moved on |
| GET | `/v1/groups/{id}/commits/{e}` | the commit accepted on epoch `e`; `410` accepted once and no longer kept (30 days), `404` never (spec 4.9) | **none** | 600/min (IP, shared) | that a member catches up |
| PUT | `/v1/blobs/{id}?fetches=n` | store a photo or video, encrypted, for `n` (1–32) fetches (spec 6.8) | signed, by the body's hash (`Yo-Body-SHA256`) | 120/min (IP); 4 at once (user), 256 at once (all); 2000 and 2 GiB/day, 1 GiB waiting (user); 10 MiB; 300 s; budget and disk floor (`507`) | uploader, size, fetch count |
| GET | `/v1/blobs/{id}` | the blob, streamed; a complete fetch counts, the last deletes it | **none**, the 128-bit id | 600/min (IP) | which blob, from where |

Spec 6.3 lists the same routes, without `/` and the well-known file, which are not part of the protocol.

## Protections outside the routes

- **Reachable through Cloudflare alone.** The hub must listen on 127.0.0.1 (or, in its container, on a port
  published on 127.0.0.1 only) and be reached through a Cloudflare Tunnel. Cloudflare terminates TLS, refuses a
  `CF-Connecting-IP` sent by a client (error 1000) and sets its own.
- **`YO_TRUST_CLOUDFLARE=1`** makes the hub take the client address from `CF-Connecting-IP`. Only safe when nothing
  but cloudflared can reach the listening address: anyone else could name any address and pass every per-address
  limit. The hub warns at start when it is set and the listening address is not a loopback one.
- **The server's own services.** With `YO_TRUST_CLOUDFLARE=1`, a request from a loopback address *without*
  `CF-Connecting-IP` is the server itself (YoGPT, speaking for many users from one address) and is exempt from the
  per-address limits (not from the per-device and per-user ones, nor the disk floor). Anything on the server that
  can be made to send requests to the hub (a proxy, a fetcher of user-given URLs) inherits that exemption.
- **Disk.** Blobs have a budget (`YO_BLOB_BUDGET_MB`) and a floor of free space (`YO_BLOB_MIN_FREE_MB`); deliveries,
  commits and new groups are refused (`507`) while the database's disk has less than 1 GiB free.
- **Cleanup**, hourly: envelopes and commits after 30 days, blobs after their last fetch or 30 days, users without
  a backup idle for 365 days, replaced devices' `410` memory after a year. Rate-limit buckets idle for a day and
  expired replay-cache entries are dropped from memory.
