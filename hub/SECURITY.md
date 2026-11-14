# The hub's security

A review of `hub/src` and `core/wire` (request signing, encodings) from an attacker's side, in November 2026, with a
second, independent review by another model (Codex) whose findings were checked one by one. The routes and their
protections are listed in `hub/README.md`.

## Threat model, in short

- **What the hub protects:** the availability of the service (its disk, memory, connections, the push
  accounts), the integrity of accounts (only the account key registers a device for a username; only a device's
  auth key speaks for it), and what spec 1 says it does not learn (content, senders, the social graph beyond
  lookups).
- **Who attacks:** anyone on the internet, through Cloudflare, without an account or with as many accounts as
  registration allows (10 per address per hour); a user of the app attacking another user; someone who learns a
  user id (any account can look one up), a device id, a blob id or a group id.
- **Not in scope:** the operator and the machine itself (the hub is trusted to route, not to read), Cloudflare (it
  sees addresses and sizes, as the hub does), the app's own storage, MLS itself.
- **Assumed:** the hub is reachable only through the Cloudflare Tunnel; `YO_TRUST_CLOUDFLARE=1` only then.

## Findings

Severity is for the hub as deployed (behind Cloudflare). "Codex" marks a finding the second review also made.
Every fix is in commit `3d1cb97` on the branch `chaos-security` (with its tests: `tests/tests/security.rs`,
`tests/tests/push.rs`, unit tests in `hub/src/lib.rs` and `hub/src/limiter.rs`).

| # | Severity | Finding | Status |
|---|---|---|---|
| 1 | High | **Disk exhaustion without an identity** (also Codex). `POST /v1/groups` then `POST /v1/groups/{id}/commit` with 64 KiB envelopes, 600 a minute: about 50 GiB a day from one address, kept 30 days; deliveries to accounts of one's own (5000 envelopes of 64 KiB per device, 10 accounts an hour) about as much. No budget or floor covered the database's disk. | Fixed: bytes delivered and committed are limited to 64 MiB per address per hour; deliveries, commits and new groups are `507` while the database's disk has less than 1 GiB free (`anonymous_bytes_per_ip_hour`, `db_min_free_bytes`). |
| 2 | High (configuration) | **SMS in development mode by default** (also Codex). Without `YO_SMS`, every phone number verified with `000000`: anyone could register any number and take over an account registered by phone. The v1 apps do not use phone numbers. | Gone: the hub has no phone numbers and no SMS any more. |
| 3 | Medium | **A stranger could spend a device's requests** (also Codex). The 1200 signed requests per device per minute were counted before the signature was checked: with a device's id and garbage signatures, anyone could keep that device at `429`. Device ids are not published, which made it harder, not impossible. | Fixed: counted once the signature holds. |
| 4 | Medium | **Inbox answers and waits without a bound per device** (also Codex). An answer holds up to 100 envelopes of 64 KiB (6.4 MiB); one device could ask 1200 times a minute at once, each a wait of a minute or a full answer held in memory. | Fixed: 4 inbox requests of one device under way at once (each until its answer has been sent), more are `429`. |
| 5 | Low | **An APNs token went into a URL as it was sent.** `PUT /v1/push` takes any bytes; the APNs request is `https://api.push.apple.com/3/device/{token}` with the hub's credentials, so a device could make the hub send a request to another path on Apple's host. Nothing beyond that host, and nothing a device could not do with its own token. | Fixed: an APNs token that is not hex is treated as invalid and cleared. |
| 6 | Low | **`YO_TRUST_CLOUDFLARE` with a public listening address.** The container's default `YO_LISTEN` is `0.0.0.0:8080`; published on anything but 127.0.0.1, anyone reaching it could name any address in `CF-Connecting-IP`, or none from a loopback address. | Mitigated: the hub warns at start when the listening address is not a loopback one. **Operator:** check the published port. |
| 7 | Low | A size in MiB from the environment large enough to overflow made the hub panic (debug) or wrap (release). | Fixed: refused as an error. |
| 8 | Medium | **A backup record went to anyone who named its user id.** `GET /v1/backup/{id}` took no signature: every contact knows the user id (it is in the MLS credentials), so anyone could learn whether a user has a backup (has secured the name), roughly how large it is, and take the sealed record home to attack offline (128-bit doors, so only in theory). | Fixed: the fetch is signed (spec 6.2) by one of the record's two auth keys, Ed25519 keys derived from the passkey's PRF output and from the 12 words, which the record names; anything else is `404`, the same answer, and the same two signature checks, as for no record. Each signature once, within 5 minutes, as for devices. What K and the storage key seal is padded to 1, 4, 16 or 60 KiB, so the record's size tells little even to a door. Records stored before the auth keys are deleted when the hub starts; the apps make their backups anew (spec 7). Tests: `tests/tests/backup_auth.rs`. |

## Accepted risks and open points

| Severity | What | Why it stays |
|---|---|---|
| Medium | **A stranger can fill a user's inbox** (5000 envelopes) and make the hub push for each (also Codex). | Spec 6.6: a delivery carries no identity (sealed sender), so there is no rate per recipient; the owner reading the inbox frees it. Access keys (only contacts may deliver) are planned. |
| Medium | **Anyone with an account can use up a user's single-use KeyPackages** with lookups (50 a day per device; also Codex). | The last resort KeyPackage keeps chats possible; the app refills below 20. A limit per looked-up user would let a stranger block lookups of that user instead. |
| Low | **Anonymous storage from many addresses.** Finding 1's budget is per address (an IPv6 /64): someone with many addresses still stores 64 MiB an hour from each. | The disk floor keeps the disk from filling; the service then refuses deliveries until the cleanup (30 days) or the operator makes room. A global budget would let one attacker shut everyone out sooner. |
| Low | **A delivery's `409` names a user's account key and device keys** to anyone who knows the user id, without the signed lookup's limit (also Codex). | Spec 4.6: the sender needs them to repair a group without an identity. User ids are handed out by lookups anyway; the keys are public keys. |
| Low | **Tables that only grow:** `seen_key_packages` (every KeyPackage hash, forever, also after the account is deleted), `groups` (never pruned), `feedback` (10 a day per user, kept, without the sender once the account goes). Also Codex. | Small rows, each behind a signed or rate-limited route. Pruning needs a time column (a schema change): later. |
| Low | **A replay after a restart.** The replay cache is in memory: a signed request captured and sent again after a restart, within its 5 minutes, is accepted again (also Codex). | Needs the request in the clear (TLS) and a restart in that window. Most signed requests are idempotent; the others (a lookup, feedback) do little harm twice. |
| Low | **What anyone can ask without an account:** whether a username is taken (by design, while it is typed), whether a device id was replaced (`410` against `401`). | Spec 6.3. Device ids are random and not published. |
| Low | **Unsigned requests to signed routes** cost a database read each, limited by nothing but Cloudflare. | Cheap reads; a limit per address on failed signatures would be the next step if it shows up in the logs. |
| Info | **The loopback exemption** (the server's own services, `hub/README.md`) passes to anything on the server that can be made to send requests to the hub. | Nothing on the server may forward user-given URLs to the hub's address. |
| Info | **The device id is not in the signed bytes** (Codex). | The hub picks the auth key by the device id and verifies with it: a signature by one device's key holds for another device only if both have the same auth key, which only that key's owner can arrange, to no gain. |

## Rejected

- **The delivery queue is unbounded** (Codex). Each delivery waits for its own answer, so the queue holds at most one
  envelope (65 KiB at most) per delivery request open: it is bounded by the connections, like every other request
  body.
- **The rate limiter's map grows without bound** (Codex). One entry per key used within a day (an address, a device,
  a user), dropped after a day idle; an attacker with very many addresses costs about 100 bytes per address.

## Checked and found sound

- **Signatures** (`core/wire/src/request.rs`): over the method, the path with the query exactly as sent (the hub
  takes `path_and_query` as received, before any decoding), the time and the body's hash; `verify_strict` (no
  malleable signatures, so the replay cache keyed by signature cannot be dodged); the time within 5 minutes; the
  cache kept until the window has passed. Paths with `..`, `//`, a trailing `/` or percent-encoding find no other
  route (tried against the dev hub too). The blob upload is signed by the body's hash and the body checked against
  it as it comes.
- **Access to other users' things:** every signed route acts on the signing device's own user, device or inbox
  (an ack deletes only the device's own envelopes); a backup by user id is that user's record, by locator only where
  no user has that id; a blob or commit needs its random id (128 and 256 bits); a commit needs the epoch's token,
  compared in constant time.
- **Registration:** a username is its user's again only with the user's current account key; a replayed
  registration is refused (its KeyPackages were seen, its device id is taken or remembered as replaced).
- **SQL:** every value is a bound parameter; the few `format!` in SQL are constants.
- **Bodies:** 65 KiB on every route but the blob upload, which takes `Content-Length` at most 10 MiB, counts what
  comes and stops at more. Decoders are bounded by the bytes they are given.
- **Errors and logs:** refusals carry a status and no text, except axum's own rejections ("Invalid URL: Invalid
  UTF-8 in `name`", "Failed to buffer the request body: length limit exceeded"), which tell nothing. The logs carry
  database and APNs/FCM errors, no user-written text and no push token: a failed push is logged without its URL
  (an APNs request's holds the token), and a service's answer with the token taken out (Codex).
- **Panics:** all seven fuzz targets (`dev/fuzz all 120`, two minutes each, after the fixes) found nothing:
  wire_envelope 57 M runs, wire_content 26 M, wire_cert 3.9 M, wire_api 4.0 M, hub_key_package 4.2 M, hub_routes
  136 k (with report, feedback, account deletion and odd queries added), core_receive 13 k. The CI runs them
  nightly for 15 minutes each.
- **Cloudflare:** a `CF-Connecting-IP` sent by a client is refused by Cloudflare (error 1000); rate limits on the dev
  hub held as configured (the username check `429` after 60 a minute, the backup fetch after 10 an hour).

Not done: `cargo audit` (not available offline on the review machine); run it in CI.
