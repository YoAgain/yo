# YoGPT and the PING echo account

The ordinary YoGPT account keeps its existing replies. Add `--echo ACCOUNT_LABEL` for a dedicated
account, for example `--account PING=http://127.0.0.1:8080,/var/lib/yo-ping-live,PING --echo PING`.
Flags can precede account declarations; unknown echo labels fail startup. Never reuse YoGPT's state directory.
Each echo account answers a direct plain Yo with exactly one plain Yo, immediately after processing it:
no think delay, rhythm, pictures, mailbox, group answers, or sender identifiers in logs. Only aggregate
answer/drop counts are logged. Media is dropped unanswered without fetching or decoding it. Hostile content
never becomes instructions. The bounded, volatile per-sender token buckets allow 1/s, burst 3; idle buckets
expire after three seconds. The normal encrypted protocol sessions and delivery/replay state still persist.

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
account configuration or `Hub.swift`, relabelled `PING`). No zkgroup parameters are needed: PING does
not answer groups. These are public verification keys, never hub signing secrets. The unit allows
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
mandatory encrypted protocol/replay state is retained. PING is a reserved protocol-only peer: it never appears
in app contacts, requests, list events, notifications, stats, or Account Sync storage. Sends have no copy for
other devices; any device receiving a PING reply decrypts and consumes it silently, including late answers.
The app shares only its last result in memory between the Account title and Status page.
