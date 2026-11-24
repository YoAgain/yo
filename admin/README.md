# Yo admin

Python standard library dashboard, one page: Usage (sign-ups, Yos, media, groups) and Attention (feedback, errors,
reports) over one range next to the logo (7, 30 or 365 days: totals with a trend against the range before, bars per
day or month; green where more is good, orange where more is bad), Users (secured, active, platforms), Server (CPU,
memory, disk, network with the last hour's bars, kept in memory by admin.py; updates), Services (queue, the units, the
STATUS bot with its frequency, [skills/status](../skills/status)) and an always-open Database widget. No hovers: a click
on a row, a service or a chart opens its detail (all its cards, in the range; the server's last 24 hours, a point
per quarter hour in admin.py's memory) between the widgets and the Database; again, × or Escape closes it. Its sidebar starts with the views: Users (the sortable users table), Support (the support chat), Errors (open
groups first with Done/Reopen) and To-dos; below them every table as it is, by name (reserved with its switch and reasons); a footer eye
blurs names and ids for screenshots. The collector (`users.py`) reads SQLite databases read-only and atomically writes
a JSON snapshot. Yos, Media and Groups read the hub's `events` table (counts per kind and minute, no identifiers). Bots
(an account whose every device names a `yogpt/` or `cli/` app) are left out of Sign-ups; the hub counts a bot's
inbox apart (`yo-in-bot`) since that change, so Yos exclude them from then on. To-Dos go through the hub API; message completion lives in the hub; user-report done markers are stored locally.
The title bar's live strip draws the last hour from `events`; each new Yo arrives at once on `GET /live?hub=`, the
hub's `/v1/admin/live` stream (Server-Sent Events, its kind only) passed through unbuffered, and pops as a spark;
without it the page falls back to the 10 s refresh.
`migrate_todos.py` is the one-time legacy JSON migration.

Protection requires Cloudflare Access in front. The server verifies its JWT signature, issuer, audience
and expiry, then requires its own scrypt login. Sessions last 12 hours, are signed and bound to the
Access identity, and use Secure, HttpOnly, SameSite=Strict cookies. Five failed logins lock login for
15 minutes. POST requests check the configured origin. There is no local authentication bypass.

All relative defaults resolve against the process working directory. Required settings have no default;
keep credentials and deployment settings outside this public repository.

| Variable | Meaning | Default |
| --- | --- | --- |
| `ADMIN_USER` | Login name | required |
| `ADMIN_PASSWORD_HASH` | `scrypt$n$r$p$salt$hash`, salt and hash base64 | required |
| `ADMIN_SESSION_KEY` | Session signing key, hex | required |
| `ACCESS_TEAM` | Access issuer hostname | required |
| `ACCESS_AUD` | Access application audience | required |
| `ADMIN_ORIGIN` | Public HTTPS origin for POST checks | `https://localhost` |
| `ADMIN_LISTEN` | HTTP bind hostname/address | `localhost` |
| `ADMIN_PORT` | HTTP port | `8081` |
| `YO_HUB_URL` | Live hub HTTP API | `http://localhost:8080` |
| `YO_HUB_DEV_URL` | Development hub HTTP API | `http://localhost:8090` |
| `YO_ADMIN_TOKEN` | Shared hub token for To-Do writes | empty (writes fail closed) |
| `ADMIN_USERS_FILE` | Collector output and dashboard snapshot | `./users.json` |
| `ADMIN_REQUEST_FILE` | Path-unit request file; parent writable by yo-admin | `/run/yo-admin-request/want` |
| `STATE_DIRECTORY` | Writable done-state and migration directory; create beforehand | `./state` |
| `STATUS_CONFIG` | The STATUS bot's rhythm, written here | `$STATE_DIRECTORY/status-bot.json` |
| `STATUS_LAST` | The STATUS bot's last report (numbers only) | `/run/yo-status-live/last.json` |
| `YOGPT_REPLIES` | Reply pictures and their README table | `./replies` |
| `YO_HUB_DB` | Live SQLite database | `./hub.sqlite` |
| `YO_HUB_DEV_DB` | Development SQLite database | `./hub-dev.sqlite` |
| `YO_HUB_ENV_FILE` | Live hub env file, reads `YO_MIN_APP` | `./hub.env` |
| `YO_HUB_DEV_ENV_FILE` | Development hub env file, reads `YO_MIN_APP` | `./hub-dev.env` |
| `YOGPT_STATS_PATTERN` | Stats path with `{}` replaced by `live` or `dev` | `./yogpt/{}/stats.json` |
| `YO_HUB_VERSION_FILE` | Hub deploy version | `./version` |
| `YOGPT_VERSION_FILE` | YoGPT deploy version | `./version-yogpt` |
| `YO_ADMIN_INTERNAL_USERS` | Internal testers' user ids (hex, comma separated), on the server only | empty |
| `YO_HUB_UNIT` | Live systemd unit | `yo-hub.service` |
| `YO_HUB_DEV_UNIT` | Development systemd unit | `yo-hub-dev.service` |
| `ADMIN_TUNNEL_UNIT` | Tunnel systemd unit | `cloudflared.service` |
| `YOGPT_UNIT` | YoGPT systemd unit | `yo-yogpt.service` |

For a local sample, from the repository root (no real user data needed):

```sh
sample=$(mktemp -d)
python3 - "$sample" <<'PY'
import sqlite3, sys
from pathlib import Path
for name in ('hub.sqlite', 'hub-dev.sqlite'):
    with sqlite3.connect(Path(sys.argv[1]) / name) as db:
        db.executescript("""
        CREATE TABLE handles (user_id BLOB, spelling TEXT, value TEXT, kind INTEGER);
        CREATE TABLE feedback (id INTEGER PRIMARY KEY, user_id BLOB, text TEXT,
                              app_version TEXT, created_at INTEGER, auto INTEGER, type TEXT);
        INSERT INTO feedback VALUES (1, NULL, 'Sample task', 'admin', 1, 0, 'todo');
        """)
PY
YO_HUB_DB="$sample/hub.sqlite" YO_HUB_DEV_DB="$sample/hub-dev.sqlite" \
  ADMIN_USERS_FILE="$sample/users.json" python3 admin/users.py
mkdir "$sample/state"
```

Active users per day are the collector's count of users seen since UTC midnight (the hub keeps one last-seen day
per device); the days before come from the hub's own `levels` (`active-24h` at each midnight, counts only, kept for
good), as do the queue and the open problems over time. Sign-ups per day come from `users.created_at` (30 UTC days).

Missing sample tables produce per-section errors without hiding feedback or tables. To serve this
snapshot, export the five required authentication settings from a private local env file, set
`ADMIN_USERS_FILE` and `STATE_DIRECTORY` to the sample paths, set `ADMIN_ORIGIN` to your HTTPS
proxy origin, and run `python3 admin/admin.py`. Put a local HTTPS proxy with Cloudflare Access in
front; plain local HTTP requests without a valid Access assertion return 403. Host metrics still
come from the local Linux machine; unavailable systemd units appear unavailable.

For design work there is a demo with a made-up world (about 120 users, support chats, to-dos, crashes, groups,
reserved names): `python3 admin/dev/demo.py --demo`, then http://127.0.0.1:8099. It builds fake hub databases in a
temporary directory, answers the hub's admin API from them, makes up the server's numbers, and serves this admin on
the loopback address only, without Access or login (it refuses to start without `--demo` or on another address).
The header shows "Demo". It never reaches a hub; nothing in production uses it.

Checks from the repository root:

```sh
python3 -m py_compile admin/*.py
node --check admin/app.js
python3 -m unittest discover -s admin -p 'test_*.py'
```

Reserved names are read and changed through the selected hub's token-protected admin routes, using the same
`YO_ADMIN_TOKEN` as To-Dos. Deploy the hubs before this admin version. The card shows the live/dev hub,
count and enforcement switch (red when off), search, comma/newline-separated adds with a reason, each name's
reason edited in place (≤ 200 characters), and individual removals.
Disabling keeps the names. The Users table keeps its page per hub during refresh, with ten users per page and space for ten rows on the final page.

The **Updates** tile reads only the root collector's snapshot. APT is checked at most every
15 minutes, release availability daily (including failed attempts); other details refresh with the
collector on dashboard requests. No `apt update`, install, reboot, or release upgrade is performed. Counts reflect
APT's existing package lists, so the server's normal APT timers must keep those fresh. Install
`update-notifier-common` for the default `apt-check` and `ubuntu-release-upgrader-core` for the release
check if absent. The release check refuses to run unless the configured release policy says `Prompt=lts`.

Yellow means pending updates, reboot, a new LTS, or unavailable checks. Red means security updates have
been continuously observed for over two days, or the reboot marker's mtime is over seven days old.
APT does not expose a reliable per-update publication age here: security age starts at first observation
and resets once no security updates remain. Cache and observation history live inside `ADMIN_USERS_FILE`;
removing it (including clearing `/run` on boot) resets that history. Errors remain visible in the popup;
missing or stale collector data is never green. Log details show the last relevant timestamped events
from the final 32 KiB; the timer's last trigger is also shown, even when logs are unavailable.

All following settings apply to **users.py**, with prefix `ADMIN_UPDATES_`:

| Suffix | Default / meaning |
| --- | --- |
| `APT_COMMAND` | `/usr/lib/update-notifier/apt-check` (output must include `total;security`) |
| `REBOOT_FILE` | `/var/run/reboot-required` |
| `REBOOT_PACKAGES` | `/var/run/reboot-required.pkgs` |
| `UNAME_COMMAND` | `/usr/bin/uname -r` |
| `KERNEL_GLOB` | `/boot/vmlinuz-*` (numeric version ordering) |
| `UNATTENDED_LOG` | `/var/log/unattended-upgrades/unattended-upgrades.log` |
| `UPGRADE_TIMER_COMMAND` | `/usr/bin/systemctl show apt-daily-upgrade.timer --property=LastTriggerUSec --value` |
| `REBOOT_TIMER_COMMAND` | `/usr/bin/systemctl show yo-reboot.timer --property=NextElapseUSecRealtime --value` |
| `OS_RELEASE` | `/etc/os-release` |
| `RELEASE_CONFIG` | `/etc/update-manager/release-upgrades` (must contain `Prompt=lts`) |
| `RELEASE_CACHE` | `/var/lib/ubuntu-release-upgrader/release-upgrade-available` (Ubuntu's daily release check) |

Commands are split with `shlex`, run without a shell, with `LC_ALL=C` and a 120-second timeout.
Use trusted, read-only commands. No extra state directory or web-service permission is needed.
The release check reads what Ubuntu's own daily check left (`RELEASE_CACHE`); the collector needs no network.
Updates tests use only sample files and mocked commands:
`python3 -m unittest discover -s admin -p 'test_updates.py'`.

The installer replaces `yo-admin-users.timer` with `yo-admin-users.path`. Authenticated dashboard
and `/users.json` requests touch `ADMIN_REQUEST_FILE` at most once per five seconds and wait up to
three seconds for a snapshot whose mtime is newer than that request. JSON responses include `stale`;
timeouts retain the available snapshot (or return only `stale: true` if none exists). The page shows
loading/updating status and refreshes every ten seconds while visible, immediately on becoming visible.
`/users.json` is sent gzip-compressed when accepted, with an ETag over the data without the collector's clock
(`at`, `updates.at`); the page asks with `If-None-Match` and keeps its list on a 304 (time from `Yo-At`).
The root oneshot collector keeps its existing 15-minute/daily update caches. No collection runs on a timer.

`setup/install-admin.sh` in yo-hub creates `/run/yo-admin-request` via tmpfiles, owned by yo-admin,
mode 0700, and allows the web service to write there. The collector output remains root:yo-admin 0640.
Keep `ADMIN_REQUEST_FILE` at the installed path (including in admin.env); custom paths need matching
path-unit, tmpfiles and sandbox settings. For a local sample, set it to `$sample/state/want`;
without a path unit, run the collector manually after each request to supply fresh data.


## App versions

The collector (no network) states each reported app (`ios/202611191257`: every TestFlight build is its UTC minute) in
`app_builds` of `users.json`. Current is, per platform, the highest build (iOS and Android: its UTC minute) reported by at least two distinct users (if none has two, the highest build);
a build only one user has is "fresh" (counts as current, tooltip "fresh: only 1 user so far"), an older one has an update. Internal testers (on newer main builds) do not move "current" and are never shown with an
update; their tooltip says "internal tester". `ios/1` is a dev build; YoGPT is compared with its deploy; the CLI is
unknown. Tables show only the platform word ("iOS", "Android", "Bot"): green when current, yellow when an
update is out, plain when unknown, with the builds in its tooltip. Builds below `YO_MIN_APP` count as update. The App
versions card counts current, update and unknown per platform. App Store releases will be added the same way later.
admin.py keeps, beside the state counts in `version-days.json`, the devices per build seen today (`apps_today`, the CLI
left out; counts only): per UTC day for good and per hour for the last 48 hours. The hub itself samples the devices
per build seen within 24 hours each hour (its `build:` levels: by the hour for 30 days, then the day's last); the page
takes those first, so 1D and 7D are drawn by the hour, 30D every 4 hours and Total by the day. The Versions detail draws them as
"Releases over time", each build's share 100 % stacked, the newest on top: of the builds the hub still takes, the
newest green through the oldest yellow; builds below `YO_MIN_APP` red ("below minimum" in the hover).
A release's build is named by its version and build, `1.0.0-RC36 (202611241344)` (`Android 1.0.0-RC36 (…)`), wherever
builds are named; any other keeps its date (`Nov 24 07:54`). The version is what the app sends after its build in
`Yo-App` (spec 6.3, `devices.version`), for older releases `RELEASES` in `users.py`, read once from the release runs'
logs; both go out as `app_versions` in `users.json`.
Tests: `python3 -m unittest discover -s admin -p 'test_versions.py'`, `test_build_names.py`, `test_release_colors.py`.

## Support chat rollout

Back up the hub DB and admin state before rollout. If old dashboard JSON done flags still need
importing, run `migrate_support_done.py` offline against schema 5 **before** upgrading to schema 6.
Deploy the hub first, then admin and its collector. No app update is required.
Startup follows schema 3 → 4 → 5 → 6 → 7 (or 4/5/6 → 7), atomically migrating each step.
Schema 6 moves `todos` into `support_messages` with `user_id=X''`, `from_team=1`,
preserving text, creation and done times, and allocating fresh message IDs to avoid collisions.
It drops `todos` and backfills user-message `done_at` from existing team read markers once.
Schema 7 moves that team thread back into a `todos` table of its own (text and times kept, ids new).
From the hub's schema 7 the team's to-dos are a table of their own, `todos`, shown raw in the Database widget (Support
group) with a field to add one, a tick per row and a delete. The support chat (support_messages, Chat view; Feedback
in Attention, key 2 or `#feedback` opens it) lists the installs that wrote (hub schema 9: a random install id, no
account; shown by its first 6 hex characters, the people it reported folded under the facts), "Open" (something of theirs not handled,
the default) or "Answered", newest first; the right side is the conversation under one line of facts. Each of their
messages is ticked handled by hand; a reply ticks the ones it answers. The list's logic is `supportList` in `app.js`,
tested in node by `test_support.py` (skipped without node). Errors in Attention open the diagnostics table, open
groups first, newest first, with Done/Reopen per row; Reports open the reports table; Sign-ups the users table. Every bubble can be completed or reopened;
viewing a thread changes no completion state. Badges and the Overview red curve count open user
messages only. App unread team replies still use `read_at`. Old JSON flags still serve user reports.
