# yo: the CLI

A Yo account on the command line, on the core (`core/client`). Every command prints one JSON value on stdout; a
failure prints `{"error": …, "retryable": …}` on stderr. Exit codes: 0 done, 1 failed, 2 usage, 3 the hub no longer
knows this device (replaced, name released, hub reset: stop), 4 nothing registered in the directory. The bots in
[skills/](../../skills) are small scripts driving it.

```sh
cargo build --release --locked -p yo-cli          # target/release/yo
export YO_STATE=~/.yo YO_HUB=https://dev.justyo.app
yo register Chris                                   # once
yo whoami | yo status | yo friends | yo contacts
yo add lina                                         # a friend
yo send lina                                        # a Yo; --photo F.jpg, or --media - (a media.ref on stdin)
yo groups create "Trio" lina noah                   # groups [list|create|leave|accept|rename|add|remove]
yo listen                                           # one JSON line per event, until stopped
yo probe                                            # the round trip to PINGTEST, in ms
```

The hub's pins come from flags or the environment: `YO_HUB_KEYS` (`--hub-key`), `YO_TRUST_ROOTS` (`--trust-root`,
without one no sealed Yo opens) and `YO_ZK_PARAMS` (`--zk-params`, without them no group); lists comma separated.

**One process per directory.** While `yo listen` runs it holds the directory, and every other `yo` command on it goes
over the directory's control socket (`control.sock`, in the 0700 directory) and runs inside the listener, on its client.
So a script reads `yo listen` and answers with plain `yo send …` calls. Without a listener a command opens the
directory itself (and waits up to 30 s for another command that has it).

**Events.** `{"type":"ready","user":…}` first, then `message` (`from`, `group`, `reply_to`: the user or `group:ID` to
answer, `content`: `yo`, `photo`, `video` or `other`, `media`: size, width, height, duration and `ref`), `new_chat`,
`profile`, `identity_changed`, `group_changed`, and `probe` (`from`, `token`: answer with `yo echo-probe`). Printed
events are acknowledged at once; with `--manual-ack` only by `yo ack`, and the unacknowledged come again after a
restart. A `media.ref` holds the blob's key: whoever has it can open the photo, so `send --media -` reads it from
stdin, never from the arguments (which every local user sees). No video from a file: the CLI can neither check its
length nor take out its metadata. `--drain-media` forgets media keys instead and fetches and acknowledges every
received blob, never opened (the hub deletes it after its last fetch), for a bot that never looks.

**The state directory** is YoGPT's as before: `account` (the account secret), `key` (the state key) and `state.sqlite`,
all 0600; keep `account` in the operator's secret backup. A `state` file of the build before the database is taken
in offline at the first start (the same device, its sessions and outbox; nothing registers again) and kept as
`state.v2`. `register` with an `account` but no state registers the same account as a new device; a state that cannot
be opened is set aside as `state.old` by `register`. The CLI announces itself to the hub as `yogpt/BUILD`, as the
bot did (the admin page knows the bots by it).

## Server switch from yo-gpt (not deployed yet)

The bots were one binary, `/opt/yo/yo-gpt`, one process per unit with an `--account` per hub. They are now `yo` plus
one script and one unit per bot and hub. Every account keeps its directory, so YoGPT, PING and PINGTEST keep their
devices and sessions: nothing registers again.

1. Build: `YO_BUILD=$(date -u +%Y%m%d%H%M) cargo build --release --locked -p yo-cli`. Install `target/release/yo` as
   `/opt/yo/yo`, `skills/` as `/opt/yo/skills/` (root owned, read only; needs `python3`, standard library only), the
   units from `skills/*/yo-*@.service` into `/etc/systemd/system/`. **Keep `/opt/yo/yo-gpt` and its units** for the
   rollback.
2. One environment file per bot and hub, root owned, 0600: `/etc/yo-yogpt-live.env`, `/etc/yo-ping-dev.env`, … with
   the values the old `$PINS` had, relabelled:
   ```sh
   # dev: http://127.0.0.1:8090
   YO_HUB=http://127.0.0.1:8080
   YO_HUB_KEYS=PUBLIC_HUB_KEY,SPARE_PUBLIC_HUB_KEY
   YO_TRUST_ROOTS=PUBLIC_TRUST_ROOT,SPARE_PUBLIC_TRUST_ROOT
   YO_ZK_PARAMS=PUBLIC_ZK_PARAMS
   ```
   systemd takes a `#` only at the start of a line: a comment behind a value becomes part of it. PINGTEST's file
   leaves out `YO_ZK_PARAMS`.
3. State directories: the units use `/var/lib/yo-<bot>-<hub>` (`StateDirectory=`). PING and PINGTEST had exactly
   these. For YoGPT read the old unit's `--account NAME=HUB,DIR` per hub; with `yo-yogpt.service` stopped, move each
   DIR whole (`account`, `key`, `state.sqlite`, `outbox/`, `stats.json`) to `/var/lib/yo-yogpt-<hub>` (with
   `DynamicUser=` both live under `/var/lib/private/`), and point the admin page's `YOGPT_STATS_PATTERN` at
   `/var/lib/yo-yogpt-{}/stats.json` and `YOGPT_UNIT` at `yo-yogpt@live.service`. Reply pictures:
   `skills/yogpt/replies` to the old `--replies` directory, or change `--replies` in the unit.
4. Switch, one bot at a time, the old one stopped first (never two processes on one directory):
   ```sh
   systemctl disable --now yo-yogpt.service
   systemctl enable --now yo-yogpt@dev.service yo-yogpt@live.service
   systemctl disable --now yo-ping@dev.service yo-ping@live.service yo-pingtest@dev.service yo-pingtest@live.service
   systemctl daemon-reload   # the new yo-ping@ and yo-pingtest@ units replace the old ones of the same name
   systemctl enable --now yo-ping@dev.service yo-ping@live.service yo-pingtest@dev.service yo-pingtest@live.service
   journalctl -u 'yo-yogpt@*' -u 'yo-ping@*' -u 'yo-pingtest@*' --since '5 minutes ago'
   ```
   Check: a Yo to YOGPT and PING from a phone, Status → Server roundtrip (PINGTEST).
5. Rollback: stop the new units, put the old unit files back (`yo-ping@`, `yo-pingtest@` are replaced in place: keep
   copies), move YoGPT's directories back, start the old units. Which binary: a `yo-gpt` from before the database
   (its directories hold a `state` file) can go back only until `yo` first started there, since `yo` takes the
   file in and its sessions move on (the file is then `state.v2`, behind). So, before the switch, also build
   `yo-gpt` from the commit this CLI came from (`cargo build --release --locked -p yo-gpt` at a1e7544) and keep it as
   `/opt/yo/yo-gpt.db`: it opens the `state.sqlite` that `yo` writes (same core, same database), so it rolls back at
   any time with the same devices. Never let both run on one directory.

Exit 3 (the device gone) and 4 (nothing registered) are final: the units do not restart then
(`RestartPreventExitStatus=3 4`). A skill never registers, so a directory moved without its state cannot replace the
account's device; a new bot is set up once by hand, with its environment file:
`/opt/yo/yo --state /var/lib/private/yo-<bot>-<hub> register NAME`, before its unit first starts.
