# Tests

Everything runs against a real hub on a real socket, with clients over real HTTP. `dev/cargo` runs cargo in the
pinned toolchain (Docker); `cargo` works the same where Rust 1.99.1 is installed.

| File | What it shows |
|---|---|
| `core/wire` unit tests | Formats round-trip; parsers refuse every changed byte and never panic (proptest) |
| `tests/scenarios.rs` | The protocol end to end: Yo there and back, long poll, updates after 100 messages and after a week, simultaneous commits ordered by the hub, two chats started at once, a new phone (new identity, closed chat dropped after 31 days), KeyPackages running out and the last resort, a replayed signed request, a forged commit, garbage in a mailbox, blocking, app restart from the sealed state, the outbox through a hub outage, garbage on every route (never a 5xx), registration without the right code |
| `tests/simulation.rs` | Seeded random runs (sends, syncs, clock jumps, re-registrations): every message arrives exactly once, nothing is dropped, both sides of every chat agree on the epoch. And 200 clients at once |
| `tests/robustness.rs` | A commit whose answer was lost is repeated and settled; a commit lost to the peer is dropped cleanly; a crash after processing (lost ack) loses and repeats nothing; a passing 429 does not drop a Welcome; a stranger can fill a mailbox only until it is read and cannot rate-limit it; the hub repeats an accepted commit and nothing else; private keys of dead KeyPackages are deleted |
| `tests/push.rs` | APNs and FCM requests exactly as sent, with nothing in them but "there is mail"; dead tokens are forgotten |
| `tests/bindings.rs` | The UniFFI layer the apps call, with HTTP and the store as callbacks |

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

Phase 1 is the whole system on one machine (clients with MLS, the hub, fetch, decrypt); phase 2 the hub alone
(deliveries over many connections). Every run writes `tests/results/<date>-<clients>c.json`; commit the ones worth
keeping, so a regression shows against them.

| Run | Hub alone | p50 / p99 | End to end | Dropped |
|---|---|---|---|---|
| 2026-11-11, 16 cores, one transaction per delivery | 5,793/s | 6.4 / 34.0 ms | 2,047 Yos/s | 0 |
| 2026-11-11, 16 cores, group commit | 16,777/s | 1.7 / 27.7 ms | 2,089 Yos/s | 0 |
