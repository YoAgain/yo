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
- Each bot's scripts have their own memory and processes; together at most half a processor, so the hub is undisturbed.
- HTTP is read by the entrance, a process and user of its own without account, keys or book: one taken over sees the
  calls that pass, and their secrets, and nothing else.
- A picture a call names is fetched by the executor: from a public https name, its addresses checked and pinned, no
  redirect, a JPEG of at most 512 KB, 10 s.
- A user of a bridge who is not connected gets a picture drawn at the start, the same for all; the script is not asked.
- A connection is made by the user's own encrypted message; its code is good for 10 minutes, once, for the partner alone.
- The user ends it by such a message too: the key is void and what was kept of them is forgotten at once.
  Codes and keys are kept as SHA-256; the script sees neither. A call reaches the user of its key and nobody else.
- Of a connected user, what they sent the service directly (when, which kind) is kept 7 days, for the other side to ask for.
- Kept of anyone else: a user id on a reminder, one per service with `"cron"`; 7 days after it ended, or at once
  on `forget`, it is gone.

A script taken over can still send those who wrote to it something else within the limits. The executors share one book:
one taken over could read the others' reminders. Not covered: a hole in the executor, the CLI, systemd or the kernel.
