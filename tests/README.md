# Tests

Everything runs against a real hub on a real socket, with clients over real HTTP. `dev/cargo` runs cargo in the
pinned toolchain (Docker); `cargo` works the same where Rust 1.99.1 is installed.

| File | What it shows |
|---|---|
| `core/wire` unit tests | Formats round-trip; parsers refuse every changed byte and never panic (proptest) |
| `tests/scenarios.rs` | The protocol end to end: Yo there and back with a session per device, a friend with another key told and written to, a new phone with the same key seen by nobody, a verified contact held until confirmed, accounts by username (lookalike names taken), restoring a username with its account key (also renamed since), backups by user id and by locator, names not secured released, registration and backup fetches limited per address, account keys bound to their user, a copied registration refused, long poll, two sessions started at once, one-time prekeys running out and the last resort, a replayed signed request, garbage in a mailbox, blocking, app restart from the sealed state, the outbox through a hub outage, garbage on every route (never a 5xx) |
| `tests/simulation.rs` | Seeded random runs (sends, syncs, clock jumps, re-registrations): every message arrives exactly once, nothing is dropped. And 200 clients at once |
| `tests/robustness.rs` | A crash after processing (lost ack) loses and repeats nothing; a passing 429 does not drop a first message; a stranger can fill a mailbox only until it is read and cannot rate-limit it; a long poll cut off leaves nothing; prekeys whose upload answer was lost still open sessions; a refill not taken is sent again, one refused made anew; signed prekeys rotate weekly and sessions still start; a new phone gets what waits once its device is named |
| `tests/routes.rs` | Every route of spec 6.3 as the spec says: its answer to a good request, `401` without a signature, with another's, changed or replayed, `410` to a replaced or deleted device (for a year, then `401`), `400` to a malformed body (strict decoding), `413` beyond 65 KiB, each route's own limits and rates, repeats; one-time prekeys handed out once (also to lookups at once), the last resort after them, refills above every id before, a repeated upload bringing none back, at most 64, rotated signed prekeys handed out as uploaded; a delivery's `409` with the roster; a Yo to an account without prekeys `Outdated` until it registers again |
| `tests/sessions.rs` | Sessions (spec 4): a lost message costs only itself, a late one opens with its skipped key, a copy is skipped; a session out of step heals by a decryption error (the peer archives its session and sends that message again); a first message opens after four weeks away with the prekeys replaced weekly |
| `tests/blobs.rs` | Photos and videos at the hub: a blob fetched by each device and gone, file and all, after the last fetch; the largest one there and back, a larger one refused by its length; an upload signed whole and once (unsigned, tampered, replayed, the id taken); quotas per user and day and of bytes waiting; the budget and the disk's free space hold uploads (`507`) until fetches make room; 30 days at most; a deleted account takes its blobs, stray files are swept; limits per address |
| `tests/media.rs` | A photo and a video with their Yos end to end: uploaded for the receiving devices before the Yo, fetched and opened once by the receiver, then gone at the hub; a blocked sender's dropped unseen; a hub that says "later" (the uploader's room or day used up) holds the upload and its Yo until a fetch makes room |
| `tests/push.rs` | APNs and FCM requests exactly as sent, with nothing in them but "there is mail"; dead tokens are forgotten |
| `tests/storage.rs` | The account's storage (spec 6.9): friends, blocks and the icon back on a new phone before its first message; two writers merge and converge (`409`, merged, written again); the hub reads none of it; too large or empty refused; gone with the account |
| `tests/restore.rs` | Six accounts wiped and restored forty times each, by every way in turn: the passkey (PRF output, record by user id), the 12 words (record by locator), the iCloud Keychain (name and secret on a fresh device), the passkey replaced and the words renewed first (the old ones open nothing), while the old phone still sends (it gets `410`, its last message arrives once). After every cycle: the same user id, messages both ways exactly once, the block holding, the storage back and merged, no orphan rows; a deleted account's backup gone and its name anyone's. The app's backup record in Rust: `tests/src/backup.rs` |
| `tests/found.rs` | What the chaos runs found (`tests/CHAOS.md`), each as its smallest case |
| `tests/bindings.rs` | The UniFFI layer the apps call, with HTTP and the store as callbacks: calls run while the inbox waits and while a Yo is on its way to one friend (a Yo to another goes past it, one to the same friend queues behind it); bursts to and from 20 friends at once arrive exactly once, in bounded time; a typed name is looked up (who it is, the owner's spelling) before the first Yo; six accounts whose app long-polls while an extension opens the state as a guest again and again (sends, syncs) and the friend writes: every Yo once both ways, every lock given back |
| `tests/flows.rs` | Every 1:1 flow through the app layer (`YoClient`) against the hub: register, add a friend, Yo both ways; a photo and a video as blobs, opened once; 150 Yos each way at once, each once in order; lost, doubled and late envelopes; the hub restarted in the middle of a send; the app killed before its save (the disk full), after it, after the hub took a Yo, before the app had its events; a new phone from the words (friends back, the friend's sessions formed anew both ways); the replaced phone still delivering, not heard; a wedged session healed by a decryption error; one-time prekeys used up, sessions on the last resort; 30 days away; a friend blocked, one deleted (its name someone new's); a friend whose app must be updated, the outbox delivered once its prekeys are there; silent receipts turning a one-way stream into fresh ratchet keys; the notification service decrypting as a guest while the app holds the state; what sealed envelopes and the saved state show (nothing) |
| `tests/speed.rs` | How fast a Yo goes, through the app layer with the inbox's long wait running as in the app: `send_yo` to the hub's push request (60 Yos, alone and ten a second), the notification service's work for one push (the state opened as a guest, the inbox read without waiting: one request before its events, the ack after), and the hub's store to the receiver's events, with 0 and 40 ms of round trip (the app's long wait; the guest). Prints its numbers: on a 16-core x86 machine, release, send_yo to push request p50 1.8 ms, p90 2.1 ms; the service 2.8 ms;  stored to the app's events at 40 ms round trip p50 20.6 ms (62.0 ms while the ack came first), to the guest's 42.5 ms (83.7 ms) |

```sh
dev/cargo test --workspace                                  # everything
YO_ENV="YO_SEEDS=27,41" dev/cargo test -p yo-tests --test simulation seeded   # replay seeds
dev/cargo run --release -p yo-tests --bin yo-yops           # Yos per second of one core, no hub (send, receive, fan-out, first Yo with PQXDH; state saved or not; MLS 930e34e beside)
YO_ENV="YO_SEEDS=1,2 YO_CLIENTS=12 YO_STEPS=4000" dev/cargo test -p yo-tests --test simulation seeded   # longer
```

A failing simulation names its seed; the run is deterministic in what it does, so the seed reproduces it.

## Chaos

`yo-chaos`: thousands of accounts with faults, crashes and hub restarts, every message accounted for. How to run it
and what it found: [CHAOS.md](CHAOS.md).

## Fuzzing

```sh
dev/fuzz all 300          # every target 5 minutes (pinned nightly + cargo-fuzz, in Docker)
dev/fuzz core_receive 600
```

| Target | What it checks |
|---|---|
| `wire_envelope`, `wire_content`, `wire_cert`, `wire_api` | Never a panic; whatever parses encodes back to the same bytes (one encoding per value) |
| `hub_routes` | Every route of the hub in-process: never a panic, never a 5xx |
| `core_receive` | A real device state, then sequences of sealed payloads seeded with real ones (a first message, a profile, Yos, a text): never a panic, the state can always be saved |

Seeds are in `fuzz/seeds/` (`dev/cargo test -p yo-tests --test fuzz_corpus -- --ignored` writes them anew when a
format changes); CI fuzzes each target a minute on every push and fifteen minutes nightly, and keeps any crash as an
artifact. A crash found becomes a regular test.

## Load

```sh
YO_ENV="YO_DATE=$(date +%F) YO_COMMIT=$(git rev-parse --short HEAD)" \
  dev/cargo run --release -p yo-tests --bin yo-load -- --clients 200 --seconds 15 --threads 16 --connections 64
```

`YO_HUB_CPU=n` (in `YO_ENV`) pins every thread of the hub to CPU n and the load to the other CPUs: the hub's
throughput on one core. Its one writer thread with group commit is what bounds it, not the cores.

Phase 1 is the whole system on one machine (clients with sessions, the hub, fetch, decrypt); phase 2 the hub alone
(deliveries over many connections). Every run writes `tests/results/<date>-<clients>c.json`; commit the ones worth
keeping, so a regression shows against them.

| Run | Yops (hub alone) | p50 / p99 | End to end | Dropped |
|---|---|---|---|---|
| 2026-11-11, 16 cores, one transaction per delivery | 5,793 | 6.4 / 34.0 ms | 2,047 Yos/s | 0 |
| 2026-11-11, 16 cores, group commit | 16,777 | 1.7 / 27.7 ms | 2,089 Yos/s | 0 |
| 2026-11-12, hub on one core (`YO_HUB_CPU=0`), load on 15 | 14,718 | 3.1 / 19.9 ms | 1,275 Yos/s | 0 |
| 2026-11-13, one core, static musl build (as deployed) | 18,295 | 3.4 / 6.5 ms | 1,078 Yos/s | 0 |
| 2026-11-13, one core, database on the SSD (`YO_LOAD_DB`) | 16,038 | 2.7 / 18.8 ms | 1,300 Yos/s | 0 |
| 2026-11-13, one core, database in tmpfs | 22,536 | 2.7 / 5.7 ms | 1,603 Yos/s | 0 |
| 2026-11-13, one core, SQLite `:memory:` | 22,147 | 2.8 / 5.8 ms | 1,669 Yos/s | 0 |
| 2026-11-14, one core, SSD, `synchronous = NORMAL` (before) | 15,297 | 3.0 / 18.1 ms | 1,250 Yos/s | 0 |
| 2026-11-14, one core, SSD, `synchronous = FULL` | 8,085 | 7.7 / 18.4 ms | 198 Yos/s | 0 |
| 2026-11-14, one core, SSD, FULL, database work off the async workers | 8,086 | 7.6 / 18.8 ms | 235 Yos/s | 0 |
| 2026-11-14, one core, SSD, FULL, acks without their own `fsync` | 7,935 | 7.7 / 20.4 ms | 705 Yos/s | 0 |

Yops: Yos delivered per second, the only benchmark unit that says what it measures. Runs before the last three
had the database in the temp dir, which is tmpfs here: in RAM. `YO_LOAD_DB` puts it elsewhere (a path, or
`:memory:`). The bound is the one writer thread; a real disk costs about a third. With `synchronous = FULL` (since
2026-11-14: an acknowledged write survives a power loss) every transaction waits for its `fsync`: group commit
shares one among a batch of deliveries, a transaction of its own pays one alone. An ack commits without one (the
next commit's takes it along): lost to a power cut, it costs a repeat the device skips.

### Registration and backups

```sh
dev/cargo run --release -p yo-tests --bin yo-register -- --accounts 200 --threads 16 --races 20 [--url http://…]
```

Accounts registered by username at once with the real core (each with its prekeys); races of four for one name
(`LEO…` twice, `LE0…`, `LÉO…`: exactly one wins, three get `409`); the per-address limit (`429` on the 11th
registration and the 11th backup fetch); backups stored and fetched by user id and by locator, a replaced locator
finding nothing, a taken one `409`; new phones registering again with their account key (the same user id, the old
phone `410`); malformed bodies `400`, an oversized one `413`. A failed check makes the exit status 1.

| Run | Registered | p50 / p99 | Races | Backups stored / fetched | Again with the key |
|---|---|---|---|---|---|
| 2026-11-13, 16 cores, 200 accounts | 390/s | 39 / 63 ms | 20 of 20 one winner | 4,968/s / 30,172/s | 321/s |
| 2026-11-13, 16 cores, 2000 accounts | 313/s | 47 / 87 ms | 50 of 50 one winner | 6,017/s / 26,548/s | 226/s |
