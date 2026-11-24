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

#### Messages (anonymous; a blob upload is signed)

| Method | Path | Purpose | Auth | Limits (key) | Hub learns |
|---|---|---|---|---|---|
| POST | `/v1/inbox/{user id}` | deliver an envelope to every device of a user, `Yo-Devices` checked; `409` with the current account key and device keys | **none** (sealed sender); signed for a request or an identified envelope (spec 4.10, 6.1) | 1000/min and 64 MiB/h (IP); sealed: 50 MiB at once +50 MiB/h of envelope bytes per recipient (`429 recipient`); 5000 waiting per device; `507` disk floor | the recipient, the bucket size, the address |
| PUT | `/v1/blobs/{id}?fetches=n` | store a photo or video, encrypted, for `n` (1–32) fetches (spec 6.8) | signed, by the body's hash (`Yo-Body-SHA256`) | 120/min (IP); 4 at once (user), 256 at once (all); 2000 and 2 GiB/day, 1 GiB waiting (user); 10 MiB; 300 s; budget and disk floor (`507`) | uploader, size, fetch count |
| GET | `/v1/blobs/{id}` | the blob, streamed; GET never consumes a fetch | **none**, the 128-bit id | 600/min (IP); 3 × original fetch count per blob | which blob, from where |
| DELETE | `/v1/blobs/{id}` | acknowledge durable storage; consumes one fetch, the last deletes it; unknown/expired also `204` | **none**, the 128-bit id | 600/min (IP), shared with GET | which blob, from where |

#### Groups (signed, plus a member proof that names no member)

| Method | Path | Purpose | Auth | Limits (key) | Hub learns |
|---|---|---|---|---|---|
| PUT, POST | `/v1/groups/{id}` | make, fetch, change a group (spec 4.9): kept encrypted, checked by zero-knowledge proof, changes by revision (`409`) | signed, and a member's auth credential presentation | signed | which device touches which group id, when; never its members or name |

#### Public (anonymous)

| Method | Path | Purpose | Auth | Limits (key) | Hub learns |
|---|---|---|---|---|---|
| GET | `/` | a page that says what this is | none | – | nothing |
| GET | `/.well-known/apple-app-site-association` | apps allowed to make passkeys for the domain (spec 7); only when `YO_APPLE_APP_IDS` is set | none | – | nothing |
| GET | `/v1/username/{name}` | is the name free: `204`, `409` taken or reserved, `400` no username | none | 60/min (IP) | the name asked |
| GET | `/.well-known/yo-hub-key` | the hub's public signing key, 32 bytes (spec 6.10); apps pin their own copy | none | – | nothing |
| GET | `/.well-known/yo-trust-root` | the sender-certificate trust root's public key, 32 bytes (spec 6.1); apps pin their own copy | none | – | nothing |
| GET | `/.well-known/yo-zk-params` | the groups' zkgroup `ServerPublicParams` (spec 4.9); apps pin their own copy | none | – | nothing |

#### Backup (anonymous, its long secret id)

| Method | Path | Purpose | Auth | Limits (key) | Hub learns |
|---|---|---|---|---|---|
| GET | `/v1/backup/{user id or locator}` | the backup record, encrypted (spec 7); `404` none, and to any request not signed by one of its doors' auth keys | signed by a door's key (`Yo-Time`, `Yo-Signature`) | 30 failed/h (IP; `YO_BACKUP_PER_IP_HOUR`); successful fetches always pass | which record is fetched, from where, by which door |

#### Support and crashes (random install id)

| Method | Path | Purpose | Auth | Limits (key) | Hub learns |
|---|---|---|---|---|---|
| POST | `/v1/help/report` | report a user (reason 1–3); `404` no such user | install id (`Yo-Install`), unsigned | 20/day (install); 200/day (IP, with messages) | which install reports whom, why |
| GET, POST | `/v1/help/messages` | the install's support thread; a message (1–2000 bytes) and app version for the operator | install id, unsigned | 10 messages/day (install), 200/day (IP, with reports); reads 120/h (install) | the text, plain, by install id |
| POST | `/v1/help/read` | the operator's answers read, through a displayed id | install id, unsigned | 120/h (install) | – |
| POST | `/v1/help/crash`, `/v1/help/error` | a crash or error report, JSON strings (core/README.md, Telemetry), ≤ 60 KiB | install id, unsigned | 300/h (install), 3000/h (IP) | the report, which install sent it on which day (90 days) |

#### Account (signed by the device key)

| Method | Path | Purpose | Auth | Limits (key) | Hub learns |
|---|---|---|---|---|---|
| POST | `/v1/register` | username + certificate + prekeys → device registered, user id (spec 6.4) | none, account-signed proof over username, device id, device key, auth key | 10/h (IP) | name, account key, device keys, push token |
| PUT | `/v1/backup` | store the account's backup record with its doors' auth keys | signed | signed | the record (opaque, its sealed part padded to 1, 4, 16 or 60 KiB), its locator, the doors' public auth keys |
| PUT | `/v1/prekeys` | add one-time prekeys, replace the signed and last resort (spec 6.5) | signed | signed; ≤ 32 per kind per upload, ≤ 64 held | – |
| POST | `/v1/lookup` | handle → user id, account key, device keys, a bundle per device if asked, username if asked; signed by the hub key when asked with `Yo-Nonce` (spec 6.10) | signed | 50/day with prekeys, 1000/day without (device) | who looks up whom |
| PUT | `/v1/push` | set the device's push token (≤ 512 bytes) | signed | signed | the token |
| GET | `/v1/group-auth` | the day's group auth credential (spec 4.9) | signed | signed | that the device asked |
| POST | `/v1/group-credential` | a profile key credential for a user whose profile key the asker holds (spec 4.9) | signed | 1000/day (device) | who asks a credential for whom |
| GET | `/v1/certificate` | a sender certificate for the signing device, a day (spec 6.1) | signed | signed | that the device asked |
| PUT | `/v1/username` | change or give up the account's username | signed | signed | the name |
| DELETE | `/v1/account` | delete the account and everything about it (spec 6.4) | signed | signed | – |
| GET | `/v1/storage` | the account's storage, sealed by its devices (spec 6.9); `404` none yet | signed | 120/h (user, shared with the next row) | when it is read |
| PUT | `/v1/storage` | replace it if the request names the version stored now, else `409` with the current one; ≤ 60 KiB, not empty | signed | 120/h (user, shared) | its size (opaque, padded to 1 KiB), when it changes |
| GET | `/v1/inbox?wait=s` | up to 100 waiting envelopes, oldest first; waits up to 60 s if none | signed | signed; 4 under way at once (device) | when a device reads |
| POST | `/v1/inbox/ack` | delete envelopes by id (≤ 1000), only the device's own | signed | signed | – |

Spec 6.3 lists the same routes, without `/` and the apple-app-site-association file, which are not part of the protocol.

## Protections outside the routes

- **Reachable through Cloudflare alone.** The hub must listen on 127.0.0.1 (or, in its container, on a port
  published on 127.0.0.1 only) and be reached through a Cloudflare Tunnel. Cloudflare terminates TLS, refuses a
  `CF-Connecting-IP` sent by a client (error 1000) and sets its own.
- **`YO_TRUST_CLOUDFLARE=1`** makes the hub take the client address from `CF-Connecting-IP`. Only safe when nothing
  but cloudflared can reach the listening address: anyone else could name any address and pass every per-address
  limit. The hub warns at start when it is set and the listening address is not a loopback one.
- **Test robots.** Support messages, reports and crash/error reports from Firebase Test Lab's addresses (Google
  Play's pre-launch reports; `config::ROBOT_RANGES`, as Firebase lists them, or `YO_ROBOT_RANGES`) are answered `204`
  and dropped. The address is the one the limits use, so behind Cloudflare it needs `YO_TRUST_CLOUDFLARE=1`.
- **The server's own services.** With `YO_TRUST_CLOUDFLARE=1`, a request from a loopback address *without*
  `CF-Connecting-IP` is the server itself (YoGPT, speaking for many users from one address) and is exempt from the
  per-address limits (not from the per-device and per-user ones, nor the disk floor). Anything on the server that
  can be made to send requests to the hub (a proxy, a fetcher of user-given URLs) inherits that exemption.
- **The operator's bot accounts** (a stopgap until bots are an account kind of their own):
  `YO_BOT_ACCOUNTS=YoGPT:USERHEX,PING:USERHEX` (a label, letters and digits, and the user id as 32 hex; set in
  `/etc/yo/hub.env`, never in the repository). Their signed requests pass every per-device and per-user limit
  (signed requests, lookups, storage, credentials, request buckets, the blob quota); the disk floor still holds.
  What they send is counted apart, out of `yo-in`, `media-in` and the live strip: `bot-media` and `bot-media:LABEL`
  per upload, and `bot-yo` per Yo from the server's own services (sealed, so by address: not per bot).
- **Push pacing and flood limits** (spec 6.6, 6.7). Pushes are paced per device: one per window at most (contacts 1 s, requests
  10 s; `YO_PUSH_WINDOW_CONTACT_MS`, `YO_PUSH_WINDOW_REQUEST_MS`), what comes meanwhile in one push at its end, a
  quiet device's at once; past 50 envelopes waiting (`YO_PUSH_BACKLOG`) a device gets one push per 10 minutes (`YO_PUSH_BACKLOG_INTERVAL_S`, 600) until its backlog is down again (counted as `push-folded` and `push-paced`, shown in the admin's Push pacing card). Sealed deliveries' envelope bytes into one recipient are a bucket as Signal's inbound
  message bytes (`YO_RECIPIENT_BYTES_BURST_MB`, `YO_RECIPIENT_BYTES_PER_HOUR_MB`, 50 and 50): beyond, `429`
  `recipient` with `Retry-After`, the sender's outbox holds that recipient only. Signed requests are counted per
  sender and hour (`request_hours`, 25 hours) for the admin's "Requests 24h"; sealed deliveries never by sender.
- **Disk.** Blobs have a budget (`YO_BLOB_BUDGET_MB`) and a floor of free space (`YO_BLOB_MIN_FREE_MB`); deliveries
  are refused (`507`) while the database's disk has less than 1 GiB free.
- **Cleanup**, hourly: envelopes after 30 days, blobs after their last acknowledgement or 30 days, crash and error groups 90 days after last seen (replaced device ids stay for good). Rate-limit buckets idle for a day and
  expired replay-cache entries are dropped from memory.
- **Schema.** A database of schema 1 (the hubs of MLS chats) is brought to schema 2 at start: accounts stay, the
  MLS tables and the envelopes waiting go (spec 6.4).

Operator To-Dos: `POST /v1/admin/todos`, plain UTF-8 body (1–4000 bytes),
`Yo-Admin-Token` header matching `YO_ADMIN_TOKEN`; returns `201 {"id": integer}`.
Unset/empty token or missing/wrong credential returns `404`; comparison is constant time.
The route uses the crash-report rate (5/address/day) in its own bucket; no peer address grants access.
Behind Cloudflare (`YO_TRUST_CLOUDFLARE=1`) every admin route answers only the server itself (loopback without
`CF-Connecting-IP`, the admin page): through the tunnel even the right token is `404`.
To-dos live in `todos` (schema 7; `GET`, done/reopen and `DELETE` on `/v1/admin/todos/{id}`); support threads live in `support_messages` by install id; crashes/errors live in `diagnostics`, their installs per day in `diagnostic_installs`.

Reserved names live only in SQLite. New databases enforce reservations by default; existing databases migrate
once to ON when `reserved` has rows, otherwise OFF. Disabling enforcement preserves names and existing holders.
`YO_RESERVED_FILE` is deprecated and ignored with a startup log message; remove it from environment files.
The same `YO_ADMIN_TOKEN` and hidden-route 404 rules as To-Dos protect:
`GET /v1/admin/reserved` → `{ "enabled": true, "names": ["Admin"], "notes": { "Admin": "role" } }` (`notes`: the
reason kept with a name, by its spelling; names without one are left out),
`POST /v1/admin/reserved` with a JSON list (up to 1000 entries, names 256 UTF-8 bytes each) of names (`"Admin"`:
added if new, an existing note kept) or objects (`{ "name": "Admin", "note": "role" }`: added if new, else its note
replaced; the note ≤ 200 characters, empty or absent clears it),
`PUT /v1/admin/reserved` with `{ "enabled": false }`, and
`DELETE /v1/admin/reserved/{name}`. Adds and removals compare skeletons; duplicate adds preserve spelling.
POST and PUT return the current state; DELETE returns 204, including when the name was absent.

`DELETE /v1/admin/users/{user}` deletes an account using the same cleanup as account deletion, including its blobs.
Requires `Yo-Admin-Token`; `user` is its 32 hexadecimal digit user id (as in support routes).
Returns `204`, or `404` for an unknown user or missing/incorrect admin credentials.

`GET /v1/admin/live` (same token) is a stream (Server-Sent Events): one `data: {"k":"yo"}` or `data: {"k":"media"}`
per Yo the moment it is counted (`yo-in`, `media-in`; bots left out, nothing of who), a comment every 15 s, from
memory and without a write. A stream more than 256 Yos behind skips them; it ends when the hub shuts down. The admin
page's live strip shows each as a spark.

## Support chat rollout

Back up the hub DB and admin state before rollout. If old dashboard JSON done flags still need
importing, run `migrate_support_done.py` offline against schema 5 **before** upgrading to schema 6.
Deploy the hub first, then admin and its collector. No app update is required.
Startup follows schema 3 → 4 → 5 → 6 (or 4/5 → 6), atomically migrating each step.
Schema 6 moves `todos` into `support_messages` with `user_id=X''`, `from_team=1`,
preserving text, creation and done times, and allocating fresh message IDs to avoid collisions.
It drops `todos` and backfills user-message `done_at` from existing team read markers once.
The pinned Team To-Dos thread uses `team` in admin URLs. Every bubble can be completed or reopened;
viewing a thread changes no completion state. Badges and the Overview red curve count open user
messages only. App unread team replies still use `read_at`. Old JSON flags still serve user reports.
