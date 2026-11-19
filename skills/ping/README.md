# PING

A visible echo contact users can add: everything it gets goes back once, as it came, at once. A Yo as a Yo; a photo
as the same photo and a video as the same video (same bytes, size and length: fetched and opened in memory by
`yo send --media`, sealed anew under a new key, uploaded for the receivers, never stored). A blob that is gone gets
nothing. In a group it answers into the group, a group at most once per 30 s; `--quiet-for YOGPT,PINGTEST` (the
other bots) are never answered. Per sender 1/s, burst 3 (volatile buckets, idle ones forgotten after 3 s); at most 64
photos, videos and group Yos wait for the one echo worker. Logs counts only (`echo answered=N`), never who.

```sh
YO_STATE=/var/lib/yo-ping-live YO_HUB=http://127.0.0.1:8080 yo register PING   # once
YO_STATE=/var/lib/yo-ping-live YO_HUB=http://127.0.0.1:8080 python3 skills/ping/ping.py --quiet-for YOGPT,PINGTEST
```

Unit: [yo-ping@.service](yo-ping@.service), with `/etc/yo-ping-%i.env` (app/cli/README.md).
