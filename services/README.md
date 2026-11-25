# services

Runs the bots: PING and YoGPT, live and on the dev hub.

A bot is a Yo account and one Python file from [yo-services](https://github.com/YoAgain/yo-services). The executor, one
process per bot, is an ordinary user to the hub:

```text
Hub <-> executor (account, keys, book) <-> script (no keys, no network)
         └─ gatekeeper: may it do that?
```

1. A Yo comes; the executor starts the script for it, one process per event (systemd, about 10 ms: one is started ahead).
2. The script gets one JSON object, says one JSON list, ends. It never learns who it is about.
3. The gatekeeper refuses the whole answer unless every action is allowed. The executor does the rest and keeps the book.

| | |
|-|-|
| [executor.rs](src/executor.rs) | What comes, the script, what goes |
| [gatekeeper](src/gatekeeper) | TOML, actions, limits: rules only, no I/O |
| [book.rs](src/book.rs) | One SQLite for all bots: a row per Yo, its time, its reminder; who only with a reminder |
| [script.rs](src/script.rs) | A connection starts a script |
| [yo.rs](src/yo.rs) | `yo listen` and its control socket |
| [systemd](systemd) | The sandbox; `install.sh` |

Security: [SECURITY.md](SECURITY.md). Tests: `cargo build -p yo-cli && cargo test -p yo-services`.
