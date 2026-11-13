# Tests

Everything runs against a real hub on a real socket, with clients over real HTTP. `dev/cargo` runs cargo in the
pinned toolchain (Docker); `cargo` works the same where Rust 1.99.1 is installed.

| File | What it shows |
|---|---|
| `core/wire` unit tests | Formats round-trip; parsers refuse every changed byte and never panic (proptest) |
| `tests/scenarios.rs` | The protocol end to end: Yo there and back, a new phone in the same chat (with a new key, and with the same key), a verified contact held until confirmed, accounts by username (lookalike names taken), a new phone restoring a username with its account key, backups by user id and by locator, names not secured released after 90 days, registration and backup fetches limited per address, account keys bound to their user, a copied registration refused, long poll, updates after 100 messages and after a week, simultaneous commits ordered by the hub, two chats started at once, a new phone (new identity, closed chat dropped after 31 days), KeyPackages running out and the last resort, a replayed signed request, a forged commit, garbage in a mailbox, blocking, app restart from the sealed state, the outbox through a hub outage, garbage on every route (never a 5xx), registration without the right code |
| `tests/simulation.rs` | Seeded random runs (sends, syncs, clock jumps, re-registrations): every message arrives exactly once, nothing is dropped, both sides of every chat agree on the epoch. And 200 clients at once |
| `tests/robustness.rs` | A commit, and a repair, whose answer was lost is repeated and settled; KeyPackages whose upload answer was lost still open chats; a commit lost to the peer is dropped cleanly; a crash after processing (lost ack) loses and repeats nothing; a passing 429 does not drop a Welcome; a stranger can fill a mailbox only until it is read and cannot rate-limit it; the hub repeats an accepted commit and nothing else; private keys of dead KeyPackages are deleted |
| `tests/blobs.rs` | Photos and videos at the hub: a blob fetched by each device and gone, file and all, after the last fetch; the largest one there and back, a larger one refused by its length; an upload signed whole and once (unsigned, tampered, replayed, the id taken); quotas per user and day and of bytes waiting; the budget and the disk's free space hold uploads (`507`) until fetches make room; 30 days at most; a deleted account takes its blobs, stray files are swept; limits per address |
| `tests/media.rs` | A photo and a video with their Yos end to end: uploaded for the chat's other devices before the Yo, fetched and opened once by the receiver, then gone at the hub; a blocked sender's dropped unseen; a hub that says "later" (the uploader's room or day used up) holds the upload and its Yo until a fetch makes room |
| `tests/push.rs` | APNs and FCM requests exactly as sent, with nothing in them but "there is mail"; dead tokens are forgotten |
| `tests/bindings.rs` | The UniFFI layer the apps call, with HTTP and the store as callbacks: calls run while the inbox waits and while a Yo is on its way to one friend (a Yo to another goes past it, one to the same friend queues behind it); bursts to and from 20 friends at once arrive exactly once, in bounded time; a typed name is looked up (who it is, the owner's spelling) before the first Yo |

```sh
dev/cargo test --workspace                                  # everything
YO_ENV="YO_SEEDS=27,41" dev/cargo test -p yo-tests --test simulation seeded   # replay seeds
```

A failing simulation names its seed; the run is deterministic in what it does, so the seed reproduces it.

## Fuzzing

```sh
dev/fuzz all 300          # every target 5 minutes (pinned nightly + cargo-fuzz, in Docker)
dev/fuzz core_receive 600
```

| Target | What it checks |
|---|---|
| `wire_envelope`, `wire_content`, `wire_cert`, `wire_api` | Never a panic; whatever parses encodes back to the same bytes (one encoding per value) |
| `hub_key_package` | The hub's KeyPackage check never panics, and accepts only what names the uploader it checks for |
| `hub_routes` | Every route of the hub in-process: never a panic, never a 5xx |
| `core_receive` | A real device state, then sequences of envelopes seeded with a real chat (Welcome, profile, Yos, a commit): never a panic, the state can always be saved |

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

Phase 1 is the whole system on one machine (clients with MLS, the hub, fetch, decrypt); phase 2 the hub alone
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

Yops: Yos delivered per second, the only benchmark unit that says what it measures. Runs before the last three
had the database in the temp dir, which is tmpfs here: in RAM. `YO_LOAD_DB` puts it elsewhere (a path, or
`:memory:`). The bound is the one writer thread; a real disk costs about a third.

### Registration and backups

```sh
dev/cargo run --release -p yo-tests --bin yo-register -- --accounts 200 --threads 16 --races 20 [--url http://…]
```

Accounts registered by username at once with the real core (each with 51 KeyPackages); races of four for one name
(`LEO…` twice, `LE0…`, `LÉO…`: exactly one wins, three get `409`); the per-address limit (`429` on the 11th
registration and the 11th backup fetch); backups stored and fetched by user id and by locator, a replaced locator
finding nothing, a taken one `409`; new phones registering again with their account key (the same user id, the old
phone `410`); malformed bodies `400`, an oversized one `413`. A failed check makes the exit status 1.

| Run | Registered | p50 / p99 | Races | Backups stored / fetched | Again with the key |
|---|---|---|---|---|---|
| 2026-11-13, 16 cores, 200 accounts | 390/s | 39 / 63 ms | 20 of 20 one winner | 4,968/s / 30,172/s | 321/s |
| 2026-11-13, 16 cores, 2000 accounts | 313/s | 47 / 87 ms | 50 of 50 one winner | 6,017/s / 26,548/s | 226/s |
