# Tests

Everything runs against a real hub on a real socket, with clients over real HTTP. `dev/cargo` runs cargo in the
pinned toolchain (Docker); `cargo` works the same where Rust 1.99.1 is installed.

| File | What it shows |
|---|---|
| `core/wire` unit tests | Formats round-trip; parsers refuse every changed byte and never panic (proptest) |
| `tests/scenarios.rs` | The protocol end to end: Yo there and back, long poll, updates after 100 messages and after a week, simultaneous commits ordered by the hub, two chats started at once, a new phone (new identity, closed chat dropped after 31 days), KeyPackages running out and the last resort, a replayed signed request, a forged commit, garbage in a mailbox, blocking, app restart from the sealed state, the outbox through a hub outage, garbage on every route (never a 5xx), registration without the right code |
| `tests/simulation.rs` | Seeded random runs (sends, syncs, clock jumps, re-registrations): every message arrives exactly once, nothing is dropped, both sides of every chat agree on the epoch. And 200 clients at once |
| `tests/push.rs` | APNs and FCM requests exactly as sent, with nothing in them but "there is mail"; dead tokens are forgotten |
| `tests/bindings.rs` | The UniFFI layer the apps call, with HTTP and the store as callbacks |

```sh
dev/cargo test --workspace                                  # everything
YO_ENV="YO_SEEDS=27,41" dev/cargo test -p yo-tests --test simulation seeded   # replay seeds
```

A failing simulation names its seed; the run is deterministic in what it does, so the seed reproduces it.

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
