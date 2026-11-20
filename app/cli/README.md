# yo: the CLI

A Yo account on the command line, on the core (`core/client`). Every command prints one JSON value on stdout; a
failure prints `{"error": …, "retryable": …}` on stderr. Exit codes: 0 done, 1 failed, 2 usage, 3 this device is
gone for good (replaced by another, name released: stop), 4 nothing registered in the directory, 5 the hub did not
accept the device's signature (a `401`: a clock still off after a reboot, a hub restarting; retryable). The bots in
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
`YO_RECOVERY_LOOKUPS` (environment only) sets the roster lookups a day for messages from a device under another's
number (a user's new phone; the core's default is 10). A bot the hub does not limit (`YO_BOT_ACCOUNTS`) raises it,
e.g. `YO_RECOVERY_LOOKUPS=2000` in its environment file: with many users, ten a day leave their messages parked.

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
received blob, never opened (the hub deletes it after its last fetch), for a bot that never looks. `sync --drain-media` does the same once, for a bot that only sends (skills/status).

**The state directory** is YoGPT's as before: `account` (the account secret), `key` (the state key) and `state.sqlite`,
all 0600; keep `account` in the operator's secret backup. A `state` file of the build before the database is taken
in offline at the first start (the same device, its sessions and outbox; nothing registers again) and kept as
`state.v2`. `register` with an `account` but no state registers the same account as a new device. A directory with a
state but no `key`, or a state that cannot be opened (a wrong key, damaged), is refused: registering there replaces the
account's device. Only `register NAME --force` does it, the state set aside as `state.old`. The CLI announces itself to the hub as `yogpt/BUILD`, as the
bot did (the admin page knows the bots by it).

## Server switch from yo-gpt (not deployed yet)

The bots were one binary, `/opt/yo/yo-gpt`, one process per unit with an `--account` per hub. They are now `yo` plus
one script and one unit per bot and hub. Every account keeps its directory, so YoGPT, PING and PINGTEST keep their
devices and sessions: nothing registers again. All steps as root on the server.

1. **Which build runs.** `systemctl status yo-yogpt.service` (and `yo-ping@*`, `yo-pingtest@*`): since when it runs,
   and the build of `/opt/yo/yo-gpt`. It must be d112fd3 or later, or have been restarted on such a build since: the
   state file it wrote is what `yo` takes in.
2. **Build and install beside the old one.** `YO_BUILD=$(date -u +%Y%m%d%H%M) cargo build --release --locked -p yo-cli`.
   Install `target/release/yo` as `/opt/yo/yo`, `skills/` as `/opt/yo/skills/` (root owned, read only; needs
   `python3`, standard library only). Do not install the units yet. `/opt/yo/yo-gpt` stays where it is.
3. **Back up**, every old unit stopped from here until its bot is switched (a backup taken while one runs is behind):
   ```sh
   systemctl stop yo-yogpt.service 'yo-ping@*' 'yo-pingtest@*'
   mkdir -m 700 /root/yo-before
   cp -a /etc/systemd/system/yo-yogpt.service /etc/systemd/system/yo-ping@.service \
         /etc/systemd/system/yo-pingtest@.service /etc/yo-*.env /opt/yo/yo-gpt /root/yo-before/
   cp -a /var/lib/private/yo-ping-live /var/lib/private/yo-ping-dev /var/lib/private/yo-pingtest-live \
         /var/lib/private/yo-pingtest-dev /root/yo-before/          # and every YoGPT --account DIR
   ```
   The copies hold the old `state` files: they are the rollback.
4. **Dry run, required, per bot and hub** (YoGPT, PING, PINGTEST; live and dev), on the backups, with
   [dev/dry-run.sh](dev/dry-run.sh) copied to the server:
   ```sh
   /root/dry-run.sh /root/yo-before/yo-ping-live /opt/yo/yo
   ```
   The script copies the directory to a temporary one, opens the copy offline (no network namespace where it can make
   one, the hub `http://127.0.0.1:9` in any case, the live environment not passed on), prints `yo status` (user,
   username, device, counts) and removes the copy. User and device must be the live ones (the admin page, the old
   journal), the counts plausible, and for a directory with a `state` file it must say "taken in (state.v2)". The
   directory given is only read. Never run `listen`, `sync` or `register` on a copy against a real hub: the copy would
   take the live device's messages and move its sessions on. A failed dry run stops the switch for that bot: start
   its old unit again.
5. **Environment files**, one per bot and hub, root owned, 0600: `/etc/yo-yogpt-live.env`, `/etc/yo-ping-dev.env`, …
   PING's and PINGTEST's files keep their names and their old `HUB=` and `PINS=` lines (the old units read them):
   **add** the new lines below them, change nothing else. The values are the ones the old `$PINS` had, relabelled:
   ```sh
   # dev: http://127.0.0.1:8090
   YO_HUB=http://127.0.0.1:8080
   YO_HUB_KEYS=PUBLIC_HUB_KEY,SPARE_PUBLIC_HUB_KEY
   YO_TRUST_ROOTS=PUBLIC_TRUST_ROOT,SPARE_PUBLIC_TRUST_ROOT
   YO_ZK_PARAMS=PUBLIC_ZK_PARAMS
   ```
   systemd takes a `#` only at the start of a line: a comment behind a value becomes part of it. PINGTEST's file
   leaves out `YO_ZK_PARAMS`.
6. **State directories:** the units use `/var/lib/yo-<bot>-<hub>` (`StateDirectory=`). PING and PINGTEST had exactly
   these. For YoGPT read the old unit's `--account NAME=HUB,DIR` per hub; move each DIR whole (`account`, `key`,
   `state` or `state.sqlite`, `outbox/`, `stats.json`) to `/var/lib/yo-yogpt-<hub>` (with `DynamicUser=` both live
   under `/var/lib/private/`), and point the admin page's `YOGPT_STATS_PATTERN` at `/var/lib/yo-yogpt-{}/stats.json`
   and `YOGPT_UNIT` at `yo-yogpt@live.service`; check that the admin page can read `stats.json`. Reply pictures:
   `skills/yogpt/replies` to `/opt/yo/replies`, or change `--replies` in the unit (without it only Yos go out).
7. **Switch, one bot at a time**, the old unit stopped first (never two processes on one directory):
   ```sh
   cp skills/*/yo-*@.service /etc/systemd/system/ && systemctl daemon-reload   # yo-ping@, yo-pingtest@ replaced
   fuser /var/lib/private/yo-pingtest-live/lock || echo free                    # nobody holds it
   systemctl enable --now yo-pingtest@live.service
   journalctl -u yo-pingtest@live.service --since '5 minutes ago'   # "ready", never "registered"
   ```
   Then PINGTEST dev, PING, YoGPT (`systemctl disable yo-yogpt.service` first) the same way. Check: Status → Server
   roundtrip (PINGTEST), a Yo to PING and YOGPT from a phone.

**Rollback.** Safe only until `yo` first started on a directory: from then on its sessions move on, and the backup is
behind (messages to the bot open no more, its answers can be refused as replays). After a successful first start,
fix forward, do not roll back. Before it, or when that start failed (the dry run should have caught it), per bot:
```sh
systemctl disable --now yo-pingtest@live.service                       # the new unit, stopped
rm -rf /var/lib/private/yo-pingtest-live                              # what yo made (state.sqlite, state.v2)
cp -a /root/yo-before/yo-pingtest-live /var/lib/private/              # the old state file back
cp -a /root/yo-before/yo-pingtest@.service /etc/systemd/system/       # the old unit (same name, replaced)
systemctl daemon-reload && systemctl enable --now yo-pingtest@live.service
journalctl -u yo-pingtest@live.service --since '5 minutes ago'         # the old binary, no registration
```
YoGPT: the same with its directories moved back to the old `--account` DIRs and `yo-yogpt.service` re-enabled. The old
units read `HUB=` and `PINS=`, still in the environment files (step 5), and run `/opt/yo/yo-gpt`, kept (step 2; a
copy in `/root/yo-before/`). No other binary is a rollback: a `yo-gpt` that opens `state.sqlite` registers again when
it cannot open a state.

Exit 3 (the device gone for good) and 4 (nothing registered) are final: the units do not restart then
(`RestartPreventExitStatus=3 4`). Every other exit restarts, also 5 (a `401`); `yo listen` waits a `401` out itself,
like the network, and the units start after `time-sync.target`. A skill never registers, so a directory moved without
its state cannot replace the account's device; a new bot is set up once by hand, with its environment file:
`/opt/yo/yo --state /var/lib/private/yo-<bot>-<hub> register NAME`, before its unit first starts. `register` refuses a
directory with a state it cannot open (`--force` only when that device is to be replaced).
