# yo: the CLI

A Yo account on the command line, on the core (`core/client`). Every command prints one JSON value on stdout; a
failure prints `{"error": …, "retryable": …}` on stderr. Exit codes: 0 done, 1 failed, 2 usage, 3 this device is
gone for good (replaced by another, name released: stop), 4 nothing registered in the directory, 5 the hub did not
accept the device's signature (a `401`: a clock still off after a reboot, a hub restarting; retryable). Small bots drive it through `yo listen`.

```sh
cargo build --release --locked -p yo-cli          # target/release/yo
export YO_STATE=~/.yo YO_HUB=https://dev.justyo.app
yo register Chris                                   # once
yo whoami | yo status | yo friends | yo contacts
yo add lina                                         # a friend
yo send lina                                        # a Yo; --photo F.jpg, or --media - (a media.ref on stdin)
yo groups create "Trio" lina noah                   # groups [list|create|leave|accept|rename|add|remove]
yo listen                                           # one JSON line per event, until stopped
yo probe                                            # the round trip to PING, in ms
```

The hub's pins come from flags or the environment: `YO_HUB_KEYS` (`--hub-key`), `YO_TRUST_ROOTS` (`--trust-root`,
without one no sealed Yo opens) and `YO_ZK_PARAMS` (`--zk-params`, without them no group); lists comma separated.
`YO_RECOVERY_LOOKUPS` (environment only) sets the roster lookups a day for messages from a device under another's
number (a user's new phone; the core's default is 10). A bot the hub does not limit (`YO_BOT_ACCOUNTS`) raises it,
e.g. `YO_RECOVERY_LOOKUPS=2000` in its environment file: with many users, ten a day leave their messages parked.

**One process per directory.** While `yo listen` runs it holds the directory, and every other `yo` command on it goes
over the directory's control socket (`control.sock`, in the 0700 directory) and runs inside the listener, on its client.
So a script reads `yo listen` and answers with plain `yo send …` calls. Without a listener a command opens the
directory itself (and waits up to 30 s for another command that has it).

**Events.** `{"type":"ready","user":…}` first, then `message` (`from`, `group`, `reply_to`: the user or `group:ID` to
answer, `content`: `yo`, `photo`, `video` or `other`, `media`: size, width, height, duration and `ref`), `new_chat`,
`profile`, `identity_changed`, `group_changed`, `probe` (`from`, `token`: answer with `yo echo-probe`) and
`connect` (`from`, `code`: a user connects this service). Printed
events are acknowledged at once; with `--manual-ack` only by `yo ack`, and the unacknowledged come again after a
restart. A `media.ref` holds the blob's key: whoever has it can open the photo, so `send --media -` reads it from
stdin, never from the arguments (which every local user sees). No video from a file: the CLI can neither check its
length nor take out its metadata. `--drain-media` forgets media keys instead and fetches and acknowledges every
received blob, never opened (the hub deletes it after its last fetch), for a bot that never looks. `sync --drain-media` does the same once, for a bot that only sends.

**The state directory** is YoGPT's as before: `account` (the account secret), `key` (the state key) and `state.sqlite`,
all 0600; keep `account` in the operator's secret backup. A `state` file of the build before the database is taken
in offline at the first start (the same device, its sessions and outbox; nothing registers again) and kept as
`state.v2`. `register` with an `account` but no state registers the same account as a new device. A directory with a
state but no `key`, or a state that cannot be opened (a wrong key, damaged), is refused: registering there replaces the
account's device. Only `register NAME --force` does it, the state set aside as `state.old`. The CLI announces itself to the hub as `yogpt/BUILD`, as the
bot did (the admin page knows the bots by it).

Exit 3 (the device gone for good) and 4 (nothing registered) are final: the units do not restart then. A bot never
registers by itself; a new one is set up once by hand, `yo --state DIR register NAME`, before its unit first starts.
