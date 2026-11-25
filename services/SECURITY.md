# services: security

A script is taken as hostile, reviewed or not.

| | Executor | Script |
|-|-|-|
| Account keys, decryption | yes (`yo listen`) | no |
| Who the sender is | a user id | nothing, not even a number |
| Network | the hub, on localhost | none |
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
- Kept of anyone: a user id on a reminder, one per service with `"cron"`; 7 days after it ended, or at once
  on `forget`, it is gone.

A script taken over can still send those who wrote to it something else within the limits. The executors share one book:
one taken over could read the others' reminders. Not covered: a hole in the executor, the CLI, systemd or the kernel.
