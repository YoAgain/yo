# STATUS

The server's numbers as a picture, sent as a Yo to one user: users, sign-ups since a baseline, services up, load and
open crash reports, on a 1080×1080 card (amethyst top, four Flat UI rows, Montserrat Bold). It runs on the server and
reads everything there: the live hub's database read-only, `systemctl is-active`, `/proc/loadavg`. It never listens
and answers nobody.

How often and to whom is set on the admin page (tab Status), which writes `/var/lib/yo-admin/status-bot.json`:
`{"every_minutes": 30, "to": "NAME", "baseline_users": 30}`, every 0 (off), 15, 30, 60, 120 or 1440 (daily, 09:00
Berlin). Slots are whole multiples from Berlin midnight; the config is read again every minute. After each slot
`/run/yo-status-live/last.json` holds the numbers (no names) for the admin page; the journal says `sent`.

Needs Pillow (`apt install python3-pil`); [Montserrat-Bold.ttf](Montserrat-Bold.ttf) is under the SIL Open Font
License ([Montserrat-OFL.txt](Montserrat-OFL.txt)).

```sh
YO_STATE=/var/lib/yo-status-live YO_HUB=http://127.0.0.1:8080 yo register NAME   # once
YO_STATE=/var/lib/yo-status-live YO_HUB=http://127.0.0.1:8080 python3 skills/status/status.py [--once]
```

Unit: [yo-status@.service](yo-status@.service), with `/etc/yo-status-%i.env` (app/cli/README.md). It runs as root
with no capability but `CAP_DAC_READ_SEARCH` (the hub's database belongs to the hub's own user) and sees only the
hub's directory of `/var/lib/private`.
