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
| [book.rs](src/book.rs) | One SQLite for all bots: a row per Yo, its time, its reminder; who only with a reminder or connected; and the connections, hashed |
| [entrance.rs](src/entrance.rs) | api.justyo.app: a process of its own, HTTP in, to the executors' sockets |
| [list.rs](src/list.rs) | `api.justyo.app/services` (JSON): the services, shown by justyo.co/services, with a folder's `logo.png` |
| [help.rs](src/help.rs) | The picture a user of a bridge gets who is not connected: its `help_url` |
| [partner.rs](src/partner.rs) | The other side of a service with a `[bridge]`: OAuth 2, its calls, calls to it |
| [script.rs](src/script.rs) | A connection starts a script |
| [yo.rs](src/yo.rs) | `yo listen` and its control socket |
| [systemd](systemd) | The sandbox; `install.sh` |

The account of a service declares itself a bot (`yo register NAME --bot`, or `yo bot on`): apps connect only those.

A bridge's environment: `SERVICES_KEY` (calls) and `SERVICES_CLIENT_SECRET` (OAuth 2), two different ones. Connecting:

```text
partner's site ─► GET /NAME/connect      client_id, redirect_uri, state: a request, 10 minutes
               ─► justyo.co/services/NAME#code=…&poll=…      draws the code, as a link and a QR code
app            ─► Connect with the code, encrypted      the first Allow is the one
page           ─► POST /NAME/allowed {"poll"}      {"request": "waiting" | "over" | "allowed" | "declined", "url": return_url?code&state, or ?error&state}
partner        ─► POST /NAME/token      the code, once, 5 minutes: a key
```

A request shows a compare code (nothing shows it yet): the first 20 bits of the SHA-256 of its code's 43 characters, as 4
of `0123456789ABCDEFGHJKMNPQRSTVWXYZ` (43 times `A`: `1W07`).

Security: [SECURITY.md](SECURITY.md). Tests: `cargo build -p yo-cli && cargo test -p yo-services`.
