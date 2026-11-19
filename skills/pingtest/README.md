# PINGTEST

The round-trip probe behind the app's Account title and Status → Server roundtrip. A probe carries a random encrypted
token (types 11/12); `yo listen` hands it out as a `probe` line and `yo echo-probe` sends the token back to its sender
only; the app completes its measurement on the matching reply and consumes late ones silently. Probes are
user-triggered; at the app they create no contact, request, list event, notification, stats or Account Sync entry.

Privacy: no per-user probe history or timing is stored or logged; only `echo answered=N` and drop counts are logged.
The per-sender limits (1/s, burst 3) live in memory only and expire after 3 s. The operator can see that the account
was active at that moment; the encrypted protocol and replay state persist as for any device. Anything else sent to
it is not answered; blobs of photos and videos are fetched into nothing, never opened.

```sh
YO_STATE=/var/lib/yo-pingtest-live YO_HUB=http://127.0.0.1:8080 YO_USERNAME=PINGTEST python3 skills/pingtest/pingtest.py
```

Unit: [yo-pingtest@.service](yo-pingtest@.service), with `/etc/yo-pingtest-%i.env` (no `YO_ZK_PARAMS` needed).
Deploy PINGTEST before apps that target it; the app gives up after ten seconds without a reply.
