# Chaos

`yo-chaos` (tests/src/chaos.rs) runs thousands of accounts through the real core against an in-process hub (restarted
on its database meanwhile, audited at the end, with account names checked after a year of simulated age) or a hub by URL.
Every account is owned by one worker thread that runs random operations on it: texts that name themselves, bursts,
fan-outs to many friends, photos and videos (upload, `429`/`507` held, fetched once), blocks and unblocks, reports,
feedback past its limit, backups, sends to oneself, to blocked and to deleted accounts, restarts from the saved state,
new phones with the same key (the old one must get `410`), deleted accounts whose names are taken again with a new key,
YoGPT-like bots answering bursts. The network of every account injects faults (dropped requests, lost answers,
duplicates, delays, `503`, `429`) and crashes (the process dies at a random request; its saved state is frozen at that
moment and it starts again from it).

A ledger checks every message: received exactly once, by the user it was for, from the user who sent it, in the
order sent, never from someone the receiver blocks; and every message that must arrive did (what a fault may lose,
such as a recipient replaced or deleted or a send that crashed, is marked as such when it happens). At the end: empty
outboxes, every saved state opens, the hub's database has no orphan rows and every blob its file.

```sh
YO_ENV="CARGO_TARGET_DIR=…" dev/cargo run --release -p yo-tests --bin yo-chaos -- \
  --seed 7 --accounts 2000 --threads 16 --seconds 600 --faults 20 --crashes 20 --restarts 3
# against a hub by URL: a handful of accounts at most. Registrations are 10 per address per hour and shared with
# everyone else testing from this address; lookalike probes are skipped there, a new phone tries once.
  … -- --url https://dev.justyo.app --accounts 4 --threads 2 --seconds 3600
```

With `--threads 1` a seed replays the same operations; with more, threads interleave as they will. Every violation is
printed with the last things the users involved did.

## Runs

On sessions (libsignal), in-process hub, release, 16-core x86. The runs and findings of the MLS chats are in the git
history before the move.

| Run | Result |
|---|---|
| `--seed 1 --accounts 300 --threads 6 --seconds 300 --faults 20 --crashes 20 --restarts 3` | 2 violations: Yos the old phone sent before the account moved, dropped by friends who read them after learning of the new phone |
| seeds 1–6, `--accounts 300 --threads 4 --seconds 240 --faults 30 --crashes 30` | 1–22 violations each, the same; fewer once a roster that drops a device waits for the inbox to be read (its cases in `flows.rs`), the rest from devices replaced again before their friend read them |
| the same after the ledger change below | 0 violations each |
| seeds 11–26, `--accounts 40 --threads 1 --seconds 120 --faults 30 --crashes 30 --restarts 2` | 0 violations each |
| `--seed 77 --accounts 1000 --threads 8 --seconds 900 --faults 20 --crashes 20 --restarts 4` | 0 violations; 162302 messages that must arrive did, once each, in order |
| `tests/simulation.rs`: seeds 1–1000 (6 clients, 400 steps), 1001–1100 (12, 2000), 2001–2050 (16, 4000) | all pass |

What a replaced device sent and its friend had not received yet may be lost (the ledger marks it so at the new phone): a
receiver that reads it once the device is out of the account's roster drops it as a replaced device's (spec 4.6), and
cannot tell it from one the device sent after. Unless it knew the device's session still: then it reads its inbox
before it takes in the roster, and the message opens.

## Scale

`yo-scale` (tests/src/scale.rs) is the same idea at ten thousand accounts and millions of messages, against a hub that
is a process of its own (killed with SIGKILL mid-write and started again on its database), with requests from an
address per phone (`CF-Connecting-IP`, some phones behind one NAT) so the hub's limits by address hold, and groups.
Accounts are a small world (5–50 friends, a few everybody knows); a worker keeps a few of its accounts running and the
rest on disk as their sealed state, so accounts are away for long stretches and catch up on full inboxes (`507`
for their senders). Besides the faults above: deliveries held back and arriving much later, uploads cut off halfway
or stalled, fetches cut off, two fetches of one blob at once, a full disk on a phone for a while, a phone's clock off by
minutes or by hours for a while, photo Yos whose blob never came, real sizes of photos (0.2–6 MB) and videos (up to the
10 MiB blob), the blob quota and budget met; group Yos, photos and changes by several members at once. At the end:
everyone catches up, then a message each way between friends (no session wedged), the ledger, the groups' counts,
every saved state, the hub's audit, a month later no blob left.

Requests (spec 4.10) as the app has them: a phone says yes to every request and group invite once it is shown (its
friends' and strangers' alike), the request limits are the hub's own with their hour 10 s (the run is days of use in
minutes). Envelopes that wait for someone who blocked the sender (it does not know: they go as requests, one an hour,
and are dropped there unseen) are counted apart (`held_for_blockers`), not as an outbox left. What may be lost is
counted by why (a send that failed before it was queued, a crash in it, a block, a new phone, a deletion).
The order check covers uninterrupted device pairs. Messages replayed after a phone replacement still count
toward the delivery and duplicate checks.

```sh
cargo build --release -p yo-tests --bin yo-scale
target/release/yo-scale --accounts 10000 --messages 5000000 --seed 21 --kills 10 --hot 16 --inbox 100 \
  --db-dir /dev/shm/yo-scale --out run.json
```

Runs (release, 16-core x86, hub database on tmpfs, 2026-11-15):

| Run | Result |
|---|---|
| seeds 1–9, `--accounts 100 --messages 25000–50000 --kills 3–4 --hot 4` | found the four fixed in `found.rs` (a clock set right, a block not saved, a group member left out, a group ask stuck); 0 violations after |
| `--accounts 100 --messages 20000 --media 80 --blob-quota-mb 24 --blob-budget-mb 300` | 0 violations; quota `429`, budget `507`, 300 cut or stalled uploads, cut fetches, fetch races |
| `--seed 11 --accounts 1000 --messages 500000 --kills 6 --hot 24` | 408598 must arrive, 0 lost, 0 twice; 843 groups, 3 lost (the member left out, fixed); audit clean; 472 sent/s |
| `--seed 21 --accounts 10000 --messages 5000000 --kills 10 --hot 8 --inbox 100` | stopped at 1157047 sent (2946 s, 393/s, 2 kills) with 0 violations so far; the end checks did not run; memory flat (1.4–1.9 GB, the hub 60 MB) |
| `--seed 31 --accounts 50 --messages 6000 --media 40 --blob-quota-mb 48 --blob-budget-mb 600` (groups, attachments) | 0 violations: 20 groups, 3173 group Yos, 99 photos/videos, 98 uploads cut or stalled; audit clean |

Send call p50 6–12 ms, p99 90–180 ms; delivery to a running receiver p50 0.6–2.6 s (most receivers are away).

## Noted

- A group member's new phone is a device the other members may not know. A member who never wrote to them takes its
  payloads in only once the hub names the device, asked at most once a minute a user and 10 times a day in all
  (recovery lookups); past that, they wait for the next day (64 kept locally, with overflow left at the hub).
  In yo-scale, phones change far more often than in life, so group Yos from a new phone have only an upper count bound.
- A cut-off fetch after the hub handed on the last byte (into the socket's buffers) counts: on loopback that is every
  blob up to a few MB (spec 6.8, "Noted" below).
- A phone whose clock ran hours ahead sent Yos whose `sent_at` stays that far ahead (each device's times rise) until
  the real time catches up: shown with that time.

- A restarted hub forgets its replay cache: a signed request captured within the last 5 minutes could be replayed once
  after a restart. Only someone who sees the requests in the clear (the operator, Cloudflare) could.
- A photo whose fetch counted at the hub and whose answer was then lost is gone for that device (spec 6.8).
- The received-message memory is keyed by the sender's user and the plaintext, whose time rises per device: two
  devices of one user sending the same thing in the same millisecond would be one.

## What the app needs (iOS)

- Registering a new account: make the 32-byte account secret in the app and pass it as `accountSecret`, keeping it
  until the registration succeeded or was refused with `409`. The core now retries a lost answer itself, but the
  app's own retry (the person taps again) with `nil` makes a new key and finds the name taken by the lost try.
- `YoContact.gone`: the friend deleted the account (or the name was released). Show the friend as gone; `send_yo`
  says `NotOnYo`. A name typed again is asked of the hub (`whois`) and may be someone new.
- Storage (spec 6.9): `friends()`, `setFriend(user:name:color:)`, `removeFriend(user:)`, `appIcon()`,
  `setAppIcon(icon:)`. The app calls `setFriend` when a friend is added, renamed or recoloured, `removeFriend` when
  removed, `setAppIcon` when the icon changes (`nil`, `"Negative"`, `"Original"`); blocks go with `setBlocked` as
  before. After a restore (or at every start), it takes `friends()` as its list and applies `appIcon()` with
  `setAlternateIconName`. The core writes to the hub with the next `sync`/`maintenance`; nothing else to call.
- `YoContact.outdated`: the friend's app is too old for sessions; a Yo to it is `Outdated` and waits in the outbox.
- `MessageKind.Unsupported(kind: 0)` never comes from this core (receipts and retries are never events).
