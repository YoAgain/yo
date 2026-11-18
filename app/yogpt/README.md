# YoGPT and the PING echo account

The ordinary YoGPT account keeps its existing replies. Add `--echo ACCOUNT_LABEL` for a dedicated
account, for example `--account PING=http://127.0.0.1:8080,/var/lib/yo-ping-live,PING --echo PING`.
Flags can precede account declarations; unknown echo labels fail startup. Never reuse YoGPT's state directory.
Each echo account sends back everything it gets, as it got it, immediately (no think delay, rhythm, reply
pictures or mailbox): a Yo as one Yo, a photo as the same photo, a video as the same video (same bytes, size and
length; fetched, opened in memory only, sealed anew under a new key and uploaded for the receivers, its own fetch
acknowledged). In a group it answers into the group, once per message and at most once per group per 30 s (the
group cooldown below); it never answers `--quiet-for` users, so list the other bots there. A blob that is gone or
larger than a blob may be gets nothing. Logs hold aggregate answer/drop counts only, never senders or content.
Hostile content never becomes instructions. The bounded, volatile per-sender token buckets allow 1/s, burst 3;
idle buckets expire after three seconds; at most 64 photos, videos and group Yos wait for the one echo worker,
more are dropped and counted. The normal encrypted protocol sessions and delivery/replay state still persist.

## Bot accounts together (`--quiet-for`, group cooldown)

`--quiet-for USERNAME[,USERNAME…]` (repeatable, any case) never answers Yos, photos or videos from these users,
one to one or in a group (by the group message's sender). Use it when several bot accounts share a hub and may
share a group, so they never answer each other, for example
`--account leo=…,Leo --account mia=…,Mia --quiet-for leo,mia,…` (its own name is skipped). Each account looks
the names up at start; a name not found is retried every minute. Only aggregate counts are logged.

Independently, every account answers into one group at most once per 30 s; group Yos within that window are
dropped and only counted in the log. One-to-one answers are unchanged.

## Owner deployment (not performed by this change)

Build the binary from this checkout: `cargo build --release --locked -p yo-gpt`.
On the server, install `target/release/yo-gpt` as `/opt/yo/yo-gpt` and install
`app/yogpt/setup/yo-ping@.service` as `/etc/systemd/system/yo-ping@.service` (root owned, 0644).
If replacing the binary used by YoGPT, restart `yo-yogpt.service` as well.

Create root-owned 0600 files `/etc/yo-ping-dev.env` and `/etc/yo-ping-live.env`:

```sh
# dev file; use http://127.0.0.1:8080 in the live file
HUB=http://127.0.0.1:8090
PINS="--hub-key PING=PUBLIC_HUB_KEY --hub-key PING=SPARE_PUBLIC_HUB_KEY --trust-root PING=PUBLIC_TRUST_ROOT --trust-root PING=SPARE_PUBLIC_TRUST_ROOT"
```

Replace the four public-key placeholders with that hub's existing pins (the same pins in its YoGPT
account configuration or `Hub.swift`, relabelled `PING`). For groups, add that hub's zkgroup params and the
other bots to `PINS`: `--zk-params PING=PUBLIC_ZK_PARAMS --quiet-for YOGPT` (and `--quiet-for PING` on YoGPT);
without `--zk-params` PING answers in no group. These are public verification keys, never hub signing secrets. The unit allows
loopback networking only; its state directories are independent (`yo-ping-dev`, `yo-ping-live`).
Run these commands on the server after installing the binary, unit, and both environment files:

```sh
systemctl daemon-reload
systemctl enable --now yo-ping@dev.service yo-ping@live.service
systemctl status yo-ping@dev.service yo-ping@live.service
journalctl -u yo-ping@dev.service -u yo-ping@live.service --since '5 minutes ago'
```

The first start registers PING separately on each hub; a taken name fails rather than taking over another
account. Keep each directory's account secret in the operator's normal secret backup. Never run two processes
against one state directory. Tapping the Account title or Status → Server → Yo-to-Yo Roundtrip tests the
app's own home hub. Before PING is deployed, the app shows No answer after ten seconds.

## Privacy

Probes are user-triggered only. The operator can see that the account was active at that moment. No per-user
probe history or timing is stored or logged; only aggregate echo counters are logged. Rate buckets are volatile;
mandatory encrypted protocol/replay state is retained. Users may add PING by name and Yo it like any contact: each
Yo, photo or video comes back once, immediately, visible normally; in a group, PING is a member like any other. Until explicitly added or written to, PING remains
protocol routing state and creates no contact, request, list event, notification, stats or Account Sync entry.
Probes carry a random encrypted token (types 11/12); only its matching reply completes the measurement.
Probe replies, including late replies, are always consumed silently, even when PING is a contact; probes have
no copy for other devices and create no visible Yo or Account Sync change. Deploy the updated echo service
before using token probes; an older responder cannot answer them. Normal YoGPT retains its thinking delay;
its working-session sends now bypass inbox catch-up and release the client lock during delivery.
The app shares only its last result in memory between the Account title and Status page.
