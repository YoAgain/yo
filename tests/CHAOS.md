# Chaos

`yo-chaos` (tests/src/chaos.rs) runs thousands of accounts through the real core against an in-process hub (restarted
on its database meanwhile, audited at the end, a year jumped for the release of names not secured) or a hub by URL.
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

None yet on sessions (libsignal). The runs and findings of the MLS chats are in the git history before the move.

## Noted

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
