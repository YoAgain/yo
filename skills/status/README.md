# STATUS

The server's numbers as a picture, sent as a Yo to one user, fixed by user id in the unit's environment
(`STATUS_TO_USER=<user id, 32 hex>` in `/etc/yo-status-%i.env`); it sends to nobody else and refuses any other
recipient: users, sign-ups since a baseline, services up, load and
open crash reports, on a 1080×1080 card (amethyst top, four Flat UI rows, Montserrat Bold). It runs on the server and
reads everything there: the live hub's database read-only, `systemctl is-active`, `/proc/loadavg`. It never answers
anyone else and accepts no chat: it runs `yo listen --drain-media` (photos and videos fetched into nothing, never
opened), so other people's Yos are taken in, dropped and do not pile up at the hub. A Yo from the recipient (not in a
group) gets the card back at once, at most one per 30 s; the timed reports go over the listener's control socket.

How often is set on the admin page (tab Status), which writes `/var/lib/yo-admin/status-bot.json`:
`{"every_minutes": 30, "baseline_users": 30}`, every 0 (off), 15, 30, 60, 120 or 1440 (daily, 09:00
Berlin). Slots are whole multiples from Berlin midnight; the config is read again every minute. After each slot
`/run/yo-status-live/last.json` holds the numbers (no names) for the admin page; the journal says `sent`.

A release card on demand: `status.py --release "Yo 1.0.0 RC17" NOTES` (NOTES a file, `-` for stdin) puts the title
and "New release" on amethyst, the notes' `- ` bullets below as a bullet list (each wrapped whole, hanging indent,
`+n more` for what does not fit; the card as tall as its content, 1080×600 to 1080×1350) and sends it to the same
one user. Run with the bot's `YO_STATE` and env file, `yo send` goes over the running listener's control socket: no
second device and no restart. `--save PATH` only writes the card. The hub's operations repository calls it after
tagging (`./release-yo TAG`).

Needs Pillow (`apt install python3-pil`); [Montserrat-Bold.ttf](Montserrat-Bold.ttf) is under the SIL Open Font
License ([Montserrat-OFL.txt](Montserrat-OFL.txt)).

```sh
YO_STATE=/var/lib/yo-status-live YO_HUB=http://127.0.0.1:8080 yo register NAME   # once
STATUS_TO_USER=… YO_STATE=/var/lib/yo-status-live YO_HUB=http://127.0.0.1:8080 python3 skills/status/status.py [--once]
```

Unit: [yo-status@.service](yo-status@.service), with `/etc/yo-status-%i.env` (app/cli/README.md). It runs as root
with no capability but `CAP_DAC_READ_SEARCH` (the hub's database belongs to the hub's own user) and sees only the
hub's directory of `/var/lib/private`.
