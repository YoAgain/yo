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
# against a hub by URL (its limits: few accounts, long):
  … -- --url https://dev.justyo.app --accounts 100 --threads 4 --seconds 3600
```

With `--threads 1` a seed replays the same operations; with more, threads interleave as they will. Every violation is
printed with the last things the users involved did.

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

Not bugs, but noted:

- A restarted hub forgets its replay cache: a signed request captured within the last 5 minutes could be replayed once
  after a restart. Only someone who sees the requests in the clear (the hub's operator, Cloudflare) could; the cache
  could be kept in the database if that matters.
- A photo whose fetch counted at the hub and whose answer was then lost is gone for that device (spec 6.8: a fetch
  counts when the hub handed on its last byte).

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
