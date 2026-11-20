# YoGPT

Answers every Yo, photo or video after 1 to 3 s with a photo, in the rhythm they came: one picture from `--replies`
(`NNN.jpg`, at most 512 KB and 1600 px, metadata taken out; never one of the last 50 that sender got while others are
left). A plain Yo only when no picture can go (none there, the hub taking none now). What it is sent is never opened (`yo listen --drain-media`). In a group it answers into the group, at most once per 30 s, and
leaves groups it is alone in. `--quiet-for YOGPT,PING,…` (other bots) are never answered. Bounded: 200 answers owed
to one sender, 32 senders at once. Logs counts only, never who.

The operator's mailbox: `NAME.anything.jpg` put into `$YO_STATE/outbox` goes to NAME as a photo (`.failed` when it
could not); `$YO_STATE/stats.json` (`{"contacts":N}`, once a minute) is for the admin page.

```sh
YO_STATE=/var/lib/yo-yogpt-live YO_HUB=http://127.0.0.1:8080 yo register YOGPT   # once
YO_STATE=/var/lib/yo-yogpt-live YO_HUB=http://127.0.0.1:8080 \
  python3 skills/yogpt/yogpt.py --replies skills/yogpt/replies --quiet-for PING,PINGTEST
```

Unit: [yo-yogpt@.service](yo-yogpt@.service), with `/etc/yo-yogpt-%i.env` (app/cli/README.md).
