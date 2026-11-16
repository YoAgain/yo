# yo-hub

The Yo hub (spec 6): users and their devices, their prekeys, delivery to users, push, blobs. It holds no content key
and never learns who sends a message. Configuration is read from the environment
(`src/main.rs` lists every variable); `hub/SECURITY.md` has the threat model and the review's findings.

## Routes

Every route the hub serves, from `Hub::router` in `src/lib.rs`. Anything else is `404` (unknown path) or `405`
(known path, other method), with an empty body. Bodies are TLS-encoded structs (`core/wire/src/api.rs`).

**Signed** means a device's request signature (spec 6.2): headers `Yo-Device`, `Yo-Time`, `Yo-Signature`, an
XEdDSA signature (Curve25519, libsignal) by the device's auth key over `"yo request v1"`, the method, the path **with its query, exactly as
sent**, the time and the SHA-256 of the body. The hub accepts a time within 5 minutes of its clock and each
signature once (an in-memory replay cache over that window). A signature that holds then costs one of the device's
1200 signed requests per minute; one that does not costs nothing of the device's. An unknown device is `401`, a
replaced or deleted one `410`. **Account-signed** is a signature by the account key, carried in the body (no time:
it is bound to a fresh device id, which the hub refuses once taken or replaced, so a replay registers nothing).

Limits are token buckets (refilled evenly) in memory, per the key named; **IP** is the client address
(`CF-Connecting-IP` when `YO_TRUST_CLOUDFLARE=1`, else the socket's peer), an IPv6 address by its /64. Every body
is at most 65 KiB (`MAX_BODY`) except a blob upload, which is streamed.

Every request may name its app in `Yo-App: PLATFORM/BUILD` (spec 6.3): the hub keeps it per device (from the
registration on, written when it changes) and answers `426` to every request of a build older than `YO_MIN_APP`
(`ios/202611140619,android/12`) for its platform.

| Method | Path | Purpose | Auth | Limits (key) | Hub learns |
|---|---|---|---|---|---|
| GET | `/` | a page that says what this is | none | – | nothing |
| GET | `/.well-known/apple-app-site-association` | apps allowed to make passkeys for the domain (spec 7); only when `YO_APPLE_APP_IDS` is set | none | – | nothing |
| POST | `/v1/register` | username + certificate + prekeys → device registered, user id (spec 6.4) | none, account-signed proof over username, device id, device key, auth key | 10/h (IP) | name, account key, device keys, push token |
| GET | `/v1/username/{name}` | is the name free: `204`, `409` taken or reserved, `400` no username | none | 60/min (IP) | the name asked |
| GET | `/v1/backup/{user id or locator}` | the backup record, encrypted (spec 7); `404` none, and to any request not signed by one of its doors' auth keys | signed by a door's key (`Yo-Time`, `Yo-Signature`) | 30 failed/h (IP; `YO_BACKUP_PER_IP_HOUR`); successful fetches always pass | which record is fetched, from where, by which door |
| PUT | `/v1/backup` | store the account's backup record with its doors' auth keys | signed | signed | the record (opaque, its sealed part padded to 1, 4, 16 or 60 KiB), its locator, the doors' public auth keys |
| PUT | `/v1/prekeys` | add one-time prekeys, replace the signed and last resort (spec 6.5) | signed | signed; ≤ 32 per kind per upload, ≤ 64 held | – |
| POST | `/v1/lookup` | handle → user id, account key, device keys, a bundle per device if asked, username if asked; signed by the hub key when asked with `Yo-Nonce` (spec 6.10) | signed | 50/day with prekeys, 1000/day without (device) | who looks up whom |
| PUT | `/v1/push` | set the device's push token (≤ 512 bytes) | signed | signed | the token |
| GET | `/.well-known/yo-hub-key` | the hub's public signing key, 32 bytes (spec 6.10); apps pin their own copy | none | – | nothing |
| GET | `/.well-known/yo-trust-root` | the sender-certificate trust root's public key, 32 bytes (spec 6.1); apps pin their own copy | none | – | nothing |
| GET | `/.well-known/yo-zk-params` | the groups' zkgroup `ServerPublicParams` (spec 4.9); apps pin their own copy | none | – | nothing |
| GET | `/v1/group-auth` | the day's group auth credential (spec 4.9) | signed | signed | that the device asked |
| POST | `/v1/group-credential` | a profile key credential for a user whose profile key the asker holds (spec 4.9) | signed | 1000/day (device) | who asks a credential for whom |
| PUT, POST | `/v1/groups/{id}` | make, fetch, change a group (spec 4.9): kept encrypted, checked by zero-knowledge proof, changes by revision (`409`) | signed, and a member's auth credential presentation | signed | which device touches which group id, when; never its members or name |
| GET | `/v1/certificate` | a sender certificate for the signing device, a day (spec 6.1) | signed | signed | that the device asked |
| PUT | `/v1/username` | change or give up the account's username | signed | signed | the name |
| DELETE | `/v1/account` | delete the account and everything about it (spec 6.4) | signed | signed | – |
| POST | `/v1/report` | report a user (reason 1–3); `404` no such user | signed | 20/day (user) | who reports whom, why |
| POST | `/v1/feedback` | feedback text (1–2000 bytes) and app version for the operator | signed | 10/day (user) | the text, plain |
| GET | `/v1/storage` | the account's storage, sealed by its devices (spec 6.9); `404` none yet | signed | 120/h (user, shared with the next row) | when it is read |
| PUT | `/v1/storage` | replace it if the request names the version stored now, else `409` with the current one; ≤ 60 KiB, not empty | signed | 120/h (user, shared) | its size (opaque, padded to 1 KiB), when it changes |
| GET | `/v1/inbox?wait=s` | up to 100 waiting envelopes, oldest first; waits up to 60 s if none | signed | signed; 4 under way at once (device) | when a device reads |
| POST | `/v1/inbox/ack` | delete envelopes by id (≤ 1000), only the device's own | signed | signed | – |
| POST | `/v1/inbox/{user id}` | deliver an envelope to every device of a user, `Yo-Devices` checked; `409` with the current account key and device keys | **none** (sealed sender); signed for a request or an identified envelope (spec 4.10, 6.1) | 200/min and 64 MiB/h (IP); 5000 waiting per device; `507` disk floor | the recipient, the bucket size, the address |
| PUT | `/v1/blobs/{id}?fetches=n` | store a photo or video, encrypted, for `n` (1–32) fetches (spec 6.8) | signed, by the body's hash (`Yo-Body-SHA256`) | 120/min (IP); 4 at once (user), 256 at once (all); 2000 and 2 GiB/day, 1 GiB waiting (user); 10 MiB; 300 s; budget and disk floor (`507`) | uploader, size, fetch count |
| GET | `/v1/blobs/{id}` | the blob, streamed; GET never consumes a fetch | **none**, the 128-bit id | 600/min (IP); 3 × original fetch count per blob | which blob, from where |
| DELETE | `/v1/blobs/{id}` | acknowledge durable storage; consumes one fetch, the last deletes it; unknown/expired also `204` | **none**, the 128-bit id | 600/min (IP), shared with GET | which blob, from where |

Spec 6.3 lists the same routes, without `/` and the apple-app-site-association file, which are not part of the protocol.

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
- **Disk.** Blobs have a budget (`YO_BLOB_BUDGET_MB`) and a floor of free space (`YO_BLOB_MIN_FREE_MB`); deliveries
  are refused (`507`) while the database's disk has less than 1 GiB free.
- **Cleanup**, hourly: envelopes after 30 days, blobs after their last acknowledgement or 30 days, users without
  a backup idle for 365 days, replaced devices' `410` memory after a year. Rate-limit buckets idle for a day and
  expired replay-cache entries are dropped from memory.
- **Schema.** A database of schema 1 (the hubs of MLS chats) is brought to schema 2 at start: accounts stay, the
  MLS tables and the envelopes waiting go (spec 6.4).

Operator To-Dos: `POST /v1/admin/todos`, plain UTF-8 body (1–4000 bytes),
`Yo-Admin-Token` header matching `YO_ADMIN_TOKEN`; returns `201 {"id": integer}`.
Unset/empty token or missing/wrong credential returns `404`; comparison is constant time.
The route uses the crash-report rate (5/address/day) in its own bucket; no peer address grants access.
Rows have `type` feedback/crash/todo; `auto` remains compatible with older readers.

Reserved names live only in SQLite. New databases enforce reservations by default; existing databases migrate
once to ON when `reserved` has rows, otherwise OFF. Disabling enforcement preserves names and existing holders.
`YO_RESERVED_FILE` is deprecated and ignored with a startup log message; remove it from environment files.
The same `YO_ADMIN_TOKEN` and hidden-route 404 rules as To-Dos protect:
`GET /v1/admin/reserved` → `{ "enabled": true, "names": ["Admin"] }`,
`POST /v1/admin/reserved` with a JSON string list (up to 1000 names, 256 UTF-8 bytes each),
`PUT /v1/admin/reserved` with `{ "enabled": false }`, and
`DELETE /v1/admin/reserved/{name}`. Adds and removals compare skeletons; duplicate adds preserve spelling.
POST and PUT return the current state; DELETE returns 204, including when the name was absent.
