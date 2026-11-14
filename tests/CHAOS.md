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

In-process hub on one machine (16 cores; clients, hub and SQLite in one process), 2% of requests faulted, 2% of
operations crashing, unless said otherwise. "Must" are the messages that had to arrive; "received" includes
messages a fault allowed to be lost that arrived anyway.

| Seed | Accounts | Seconds | Sent | Received (per s) | Must, lost | Faults, crashes | New phones, deleted, hub restarts | Violations | RSS |
|---|---|---|---|---|---|---|---|---|---|
| 12 | 1000 | 600 | 247,671 | 173,503 (288/s) | 171,751, 32 lost | 20,795, 5,029 | 2,689, 1,095, 3 | 46: finding 8 (the lost burst), driver issues since fixed | 1.9 GB |
| 13 | 1200 | 900 | 358,653 | 254,492 (282/s) | 252,296, **0 lost** | 30,264, 7,468 | 4,003, 1,685, 4 | 4: finding 10 (order), a driver issue | 2.4 GB |
| 14 | 1500 | 1200 | 426,600 | 294,368 (245/s) | 290,640, **0 lost** | 84,508 (4%), 13,283 (3%) | 5,072, 2,066, 5 | 2, both in the driver (since fixed) | 2.8 GB |
| 16 | 2500 | 900 | 403,786 | 320,643 (354/s) | 318,247, **0 lost** | 39,795, 7,088 | 3,943, 1,561, 3 | 1: a whois raced a name being taken again (the driver's directory a moment behind) | 4.9 GB |
| 21 | 400 | 300 | 77,421 | 58,424 (193/s) | 57,550, **0 lost** | 6,506, 1,498 | 823, 337, 2 | 0 (after the core review, finding 12) | 0.9 GB |

Against the dev hub (seed 101): 15 of 16 accounts registered over 52 minutes (its limit, 10 per address per hour;
one real `503` from the hub on a registration), then 492 messages sent and 144 received before the driver stood still:
a new phone waited for the next hour's registrations (since then it tries once on a shared hub). A second run was
stopped to give the address's registrations back to the app testers. The accounts it left (names with `bpc`, `qwt`)
have no backup and are released after 365 days.

Registration: 1000 accounts in 8.1 s (124/s), 2500 in 20.2 s, every lookalike (3,181) and reserved spelling refused.
A year later (prune at +366 days): every name without a backup released and free, every secured one kept. The
hub's database audited after every run: no orphan rows, every blob with its file.

## Findings

| # | Seed, scenario | What broke | Fix | Test |
|---|---|---|---|---|
| 1 | 1, lost answer to `POST /v1/register` | The app registers a new account without a key of its own (`register_username(…, None)`); the hub took the registration, the answer was lost, and the next try made a new key: `409`, the name taken by the user's own lost account until the 365-day release. | The core tries again itself, with the same account key and a new device (the first one's KeyPackages are spent): the name is that key's, so the same user. The app should also keep the secret across its own retries (see below). | `found.rs` `a_registration_whose_answer_was_lost_keeps_the_name` |
| 2 | 1, crash and restart; extensions | Two processes of one device (a restart after a crash, or the app and an extension) signing the same request in the same millisecond made the same signature, which the hub takes once: `401` for the second, an error to the app. | The last time signed is saved with the state and every process signs later than it; a `401` is signed anew and sent once more (also the long poll's). | `found.rs` `a_signature_the_hub_saw_already_is_made_anew` |
| 3 | 1, `429`/`503` on the inbox during a send | A send receives first; a passing refusal there failed the whole send, the message never queued. | Receiving first may fail "for now": the message goes into the outbox anyway (as offline). Same for opening a chat for a photo. | `found.rs` `a_send_goes_into_the_outbox_when_receiving_first_is_refused_for_now` |
| 4 | 1, delete and take the name again | A Yo to a friend who deleted the account was dropped at delivery (`404`) and the app was told it was sent; the name, taken by a stranger since, still meant the old friend's chat (a Yo to it went nowhere, or `Blocked` if the old one was blocked). | A delivery's `404` marks the contact gone: its chats dropped, its handles forgotten (the name is asked of the hub again), `NotOnYo` for the send and every later one. `ContactInfo`/`YoContact` have `gone`. | `found.rs` `a_friend_who_deleted_the_account_is_gone_and_the_name_free` |
| 5 | 11, 1000 accounts: a crash, then a block | What arrived before a block and waited to be handed out (the app restarted before it had it) was handed out after the block: shown from someone blocked. | A block drops what from that user waits to be handed out. | `found.rs` `a_block_also_drops_what_waits_to_be_shown` |
| 6 | review of the branch (Codex) | The storage: a replayed or unreadable blob was taken; unknown fields kept only at the top; no bound on its size; blocks it brought back left waiting messages to be shown. A send's `NotOnYo` was lost in a split send. A detached signed request (long poll, whois, feedback, upload) could leave without the time it signed being saved. | Sealed version must match the hub's; extras kept on every level; names at most 64 bytes, compact JSON, removed friends forgotten first past the limit; `apply_block` shared; `to` carried through `Sent::More`; saved before it leaves. | `storage.rs` `a_replayed_storage_is_not_taken` |
| 7 | probe: a device away for 31 days | A dead chat: the hub forgot the commit the device lacked (30 days), so it could never read its peer again; every later message was dropped as `WrongEpoch`, silently, while its own sends still went. | The chat restart (spec 4.9): the commits a later message needs are fetched from the hub, and if one is gone the chat is restarted in a new group with a silent `Restart { last_seen }`; the peer drops the old chat and sends again what it sent after `last_seen` (the last 64 plaintexts per chat are kept) and what was waiting. | `deadchat.rs` `a_chat_lost_after_a_month_away_is_restarted_and_nothing_after_is_lost` |
| 8 | 12, 1000 accounts: a burst never arrived | A chat started by someone blocked was not joined but ignored for good: after the unblock, everything the sender wrote in it was discarded unseen, without even counting as dropped. | A blocked user's Welcome is joined quietly (no `NewChat`); its messages are dropped while the block holds; after the unblock the chat works (spec 4.8). | `found.rs` `a_chat_started_while_blocked_works_after_the_unblock` |
| 9 | review of the restart (Codex) | Restart: the hub could name another user for the restarted chat; a made-up later epoch in an unsigned delivery marked a healthy chat lost (the hub's `404`); fetching recursed on what it fetched; two restarts at once lost each other's history; a peer waiting for confirmation lost the waiting messages; more than 64 missing commits counted as lost. A blocked user's quietly joined chat got updates and names. | The hub answers `410` for a commit it no longer keeps, `404` for one it never had, and only `410` loses a chat; a restart only with the hub's answer for that user id; fetched commits must move the group on and fetch nothing themselves; old histories move into the new chat; `Unconfirmed` waits; more commits are fetched next time; strictly increasing `sent_at` and a receiver's memory of 2000 messages make copies harmless; no updates or names to a blocked peer. | `deadchat.rs` `a_made_up_epoch_does_not_restart_a_chat` |
| 10 | 13, 1200 accounts: #28 after #29 | A message renewed for a user's new devices went to the back of the outbox, behind a later message already made for the new devices (queued while the renewal was held by a `429`): the user got them out of order. | The renewed message takes the later ones of that user and chat behind it again. | the chaos ledger (order is checked per sender and receiver) |
| 11 | review of the fixes (Codex) | Catching up stopped at 64 commits and rolled back what it fetched, for ever; a commit accepted between two reads of the hub was answered `410`; the messages sent again after a restart were in group order, not sent order. | Up to 1000 commits at once, the target checked after the last; the hub reads the commit and the group's epoch under one lock; the replay sorted by `sent_at`. | deadchat.rs, the hub's tests |
| 12 | review of the core (Codex) | Fifteen: what waited for someone blocked still left; a refused commit turned updates off for good; a name that changed hands went to its old owner; a failed save stopped nothing; catching up past 1000 commits rolled back; a lost race could encrypt in an old epoch; registrations and new groups went to the hub before they were saved; a restart replayed only after `last_seen`; a last resort was retired too early; storage merged unknown fields by value and left a blob nobody could open; an unreadable `409` dropped the message; a name skipped while a key waited was never told; a digit with an accent was an emoji. | Each fixed as spec/v1.md now says (4.2, 4.6–4.9, 6.4, 6.5, 6.9, 3.3). | `review.rs`, and tests in storage.rs, scenarios.rs, bindings.rs, handle.rs |

Not bugs, but noted:

- With the hub's own limits (`--limits`, seed 15: 30 accounts on one address, 2 minutes): every delivery refused
  with `429` waited in its outbox and arrived (399 that had to, 0 lost, outboxes empty at the end). But a *new* chat
  cannot wait: its `POST /v1/groups` is limited to 60 per address per hour, and the send fails with `429` (4,352 of
  8,620 sends in that run). Phones behind one carrier address (CGNAT) share that budget: at scale, new chats would
  fail for everyone behind a busy address. The limit (and `lookup` with KeyPackages, 50 per device per day) is the
  operator's to weigh; since finding 12 a first message waits in the outbox, with its Welcome, until the hub takes the new group.

- A restarted hub forgets its replay cache: a signed request captured within the last 5 minutes could be replayed once
  after a restart. Only someone who sees the requests in the clear (the hub's operator, Cloudflare) could; the cache
  could be kept in the database if that matters.
- A photo whose fetch counted at the hub and whose answer was then lost is gone for that device (spec 6.8: a fetch
  counts when the hub handed on its last byte).

## Left open

- The received-message memory (dedupe after a restart) is keyed by the sender's user and the plaintext, whose time
  rises per device: two devices of one user sending the same thing in the same millisecond would be one. v1 has one
  device per account; with more, a message needs an id of its own (device and counter), kept through a restart.
- A renewed message keeps the later messages of its user *in the same chat* behind it; two chats with the same user
  (4.5, or a restart at the same time on both sides) are ordered each on its own.
- The runs are on one machine: 2500 accounts at once (4.9 GB for clients with full MLS state, hub and ledger in one
  process) is about where its 15 GB end.

## How a chat can die, besides

The state on the phone is excluded from backups and its key is this-device-only, so an old state cannot come back
from an iCloud backup (which would have been the other way to a dead chat: an MLS state behind its group). What is
left: a device away longer than 30 days (handled, 4.9); a hub restored from an older backup (groups' epochs behind the
devices': commits are refused as stale, the chats work without updates, as for a group the hub does not know); and a
commit one side refuses as invalid (a bug or an attack): then the sender's later messages come from an epoch the
receiver never reaches, the commit is at the hub, the receiver processes it again and refuses it again. That last
case is not restarted (it would hide an attack); it shows as dropped messages.

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
- A restarted chat (spec 4.9) may bring a `NewChat` for a friend the app knows already: it is the same friend.
  `MessageKind.Unsupported(kind: 6)` never comes from this core (a `Restart` is never an event).
