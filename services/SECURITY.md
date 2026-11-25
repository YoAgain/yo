# services: security

A script is taken as hostile, reviewed or not.

| | Executor | Script |
|-|-|-|
| Account keys, decryption | yes (`yo listen`) | no |
| Who the sender is | a user id | nothing, not even a number |
| Network | the hub, on localhost; with a `[bridge]` also its `call_urls`, and calls by way of the entrance | none |
| Files | its account (`/var/lib/yo-services/accounts`), the book | Python and yo-services, read-only |
| Processes | its own | one per event, gone after 45 s |
| To whom, how often | decides | cannot say |

- The TOML is read before anything of the script runs; a wrong one, or another account, stops the service.
- An answer is at most 1 MB, 10 s, 4 actions: one to send, one reminder at least a minute ahead. Anything else in it and
  none of it happens. A probe only comes back.
- One answer to a Yo, one reminder for it, and none out of a reminder: no bot writes on after the user stops.
- A photo goes back by a word; its key never reaches the script.
- Per sender 1 a second (3 at once), per group one in 30 s, per bot 2000 an hour, also over a restart.
  Apart from that: one answer for each connection made or ended (any other counts, 1 a second per sender;
  a restart forgets how many of those went). Answers wait in a queue of 256: more, or those queued at a restart, are lost; the app
  then lets the user ask again. A call nobody waits for any more sends nothing.
- Each bot's scripts have their own memory and processes; together at most half a processor, so the hub is undisturbed.
- HTTP is read by the entrance, a process and user of its own without account, keys or book: one taken over sees the
  calls that pass, and their secrets, and nothing else.
- A picture a call names is fetched by the executor: from a public https name, its addresses checked and pinned, no
  redirect, a JPEG of at most 512 KB, 10 s with the lookup.
- A user of a bridge who is not connected gets a picture drawn at the start, the same for all; the script is not asked.
- A connection begins at `GET /NAME/connect`, for the one `client_id` and `redirect_uri` of the TOML; wrong ones lead nowhere.
  A request is one user's, the first to allow it by their own encrypted message, within 10 minutes; no other code makes one.
- `POST /NAME/allowed` takes no key but what the page alone was told (kept hashed, in the fragment); the QR code's own code
  tells nothing. justyo.co alone may ask from a browser, in 128 places of its own. A lost answer may be asked for again.
- A link declined is nobody's any more, and the partner is told (`error=access_denied`).
- From one address (`CF-Connecting-IP`; IPv6 by its /64) 10 requests at once, then one in 10 s, and 3
  waits at once; kept in memory, gone within 3 minutes, never logged. The header is the tunnel's word: the entrance
  listens on loopback, and a process on that machine, trusted, could say anything in it. Operations are told apart by the
  decoded path. A body is read once the call has a place, for 5 s.
- The OAuth 2 code is good for 5 minutes, once, with the client's secret, which is not the key of the calls.
- What the other side knows a user by is made of their id and a secret of the service: the same after a reconnect.
- The user ends it by such a message too: the key is void and what was kept of them is forgotten at once.
  Codes and keys are kept as SHA-256 (the app's code as it is, with its request); the script sees none. A call reaches the user of its key and nobody else.
- Of a connected user, what they sent the service directly (when, which kind): the newest 50 are told, kept at most 7 days, for the other side to ask for.
- Kept of anyone else: a user id on a reminder, one per service with `"cron"`; 7 days after it ended, or at once
  on `forget`, it is gone.

A script taken over can still send those who wrote to it something else within the limits. The executors share one book:
one taken over could read the others' reminders. Not covered: a hole in the executor, the CLI, systemd or the kernel.
