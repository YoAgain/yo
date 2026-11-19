# Yo admin

Python standard library dashboard for server health, systemd services, a Users tab (today's figures, sign-ups and
active users per day, every user in a table sortable by each column), hub database tables,
User Reports, feedback and To-Dos, anonymous crash/error diagnostics with counts and done ticks, reserved-name management, and YoGPT reply pictures. The collector (`users.py`)
reads SQLite databases read-only and atomically writes a JSON snapshot. To-Dos go through the hub API;
message completion lives in the hub; user-report done markers are stored locally. `migrate_todos.py` is the one-time legacy JSON migration.

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
| `YOGPT_REPLIES` | Reply pictures and their README table | `./replies` |
| `YO_HUB_DB` | Live SQLite database | `./hub.sqlite` |
| `YO_HUB_DEV_DB` | Development SQLite database | `./hub-dev.sqlite` |
| `YO_HUB_ENV_FILE` | Live hub env file, reads `YO_MIN_APP` | `./hub.env` |
| `YO_HUB_DEV_ENV_FILE` | Development hub env file, reads `YO_MIN_APP` | `./hub-dev.env` |
| `YOGPT_STATS_PATTERN` | Stats path with `{}` replaced by `live` or `dev` | `./yogpt/{}/stats.json` |
| `YO_HUB_VERSION_FILE` | Hub deploy version | `./version` |
| `YOGPT_VERSION_FILE` | YoGPT deploy version | `./version-yogpt` |
| `YO_ASC_KEY_PATH` | App Store Connect API key (`.p8`), read by the collector (root) only | empty (no channels) |
| `YO_ASC_KEY_ID` | Its key id | empty |
| `YO_ASC_ISSUER_ID` | Its issuer id | empty |
| `YO_ASC_APP_ID` | App Store Connect app id | `6834493649` |
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
per device); the admin keeps the highest count per day, counts only, for 60 days in `STATE_DIRECTORY/active-days.json`,
so the chart fills from the first day this version runs. Sign-ups per day come from `users.created_at` (30 UTC days).

Missing sample tables produce per-section errors without hiding feedback or tables. To serve this
snapshot, export the five required authentication settings from a private local env file, set
`ADMIN_USERS_FILE` and `STATE_DIRECTORY` to the sample paths, set `ADMIN_ORIGIN` to your HTTPS
proxy origin, and run `python3 admin/admin.py`. Put a local HTTPS proxy with Cloudflare Access in
front; plain local HTTP requests without a valid Access assertion return 403. Host metrics still
come from the local Linux machine; unavailable systemd units appear unavailable.

Checks from the repository root:

```sh
python3 -m py_compile admin/*.py
node --check admin/app.js
python3 -m unittest discover -s admin -p 'test_*.py'
```

Reserved names are read and changed through the selected hub's token-protected admin routes, using the same
`YO_ADMIN_TOKEN` as To-Dos. Deploy the hubs before this admin version. The card shows the live/dev hub,
count and enforcement switch (red when off), search, comma/newline-separated adds and individual removals.
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
The root oneshot collector keeps its existing 15-minute/daily update caches. No collection runs on a timer.

`setup/install-admin.sh` in yo-hub creates `/run/yo-admin-request` via tmpfiles, owned by yo-admin,
mode 0700, and allows the web service to write there. The collector output remains root:yo-admin 0640.
Keep `ADMIN_REQUEST_FILE` at the installed path (including in admin.env); custom paths need matching
path-unit, tmpfiles and sandbox settings. For a local sample, set it to `$sample/state/want`;
without a path unit, run the collector manually after each request to supply fresh data.


## App versions

The collector names each reported app (`ios/202611191257`: the build is its UTC minute) by channel and state, in
`app_builds` of `users.json`. With an App Store Connect key it reads, at most every 10 minutes and with GET only:
the newest 200 builds (version, upload, expired), the builds in the TestFlight groups "Public" (GitHub releases) and
"Team" (every push to main), and the App Store versions with their build. A build in an App Store version is
"App Store 1.0.1 (1859)", newest when it is the live one; in "Public" it is "TestFlight 1859", newest against the
newest unexpired Public build; otherwise "TestFlight (internal) 1257", against the newest Team build. A build App
Store Connect does not know (and `ios/1`) is a dev build, YoGPT is compared with its deploy, Android and the CLI show
their build only. Only an older build on the same channel shows the yellow "update"; expired builds and builds below
`YO_MIN_APP` show red. Without the key, or before the first fetch, every iOS build shows "?", never "update"; a failed
fetch keeps the last good data and shows its error on the App versions card.

The key: an App Store Connect API key with the Developer role (read-only use), its `.p8` file on the server outside
this repository (for example `/etc/yo/asc.p8`, root:root 0600), and the three `YO_ASC_*` lines in the collector's
environment (`/etc/yo/admin.env`). JWTs are signed ES256 with `cryptography` when installed, else with `openssl`.
Tests use fixtures only: `python3 -m unittest discover -s admin -p 'test_versions.py'`.

## Support chat rollout

Back up the hub DB and admin state before rollout. If old dashboard JSON done flags still need
importing, run `migrate_support_done.py` offline against schema 5 **before** upgrading to schema 6.
Deploy the hub first, then admin and its collector. No app update is required.
Startup follows schema 3 → 4 → 5 → 6 (or 4/5 → 6), atomically migrating each step.
Schema 6 moves `todos` into `support_messages` with `user_id=X''`, `from_team=1`,
preserving text, creation and done times, and allocating fresh message IDs to avoid collisions.
It drops `todos` and backfills user-message `done_at` from existing team read markers once.
The team's to-dos use `team` in admin URLs. In the Support tab all of them are one row, "To-dos · N" (open ones),
which opens the checklist; a to-do written in a user's conversation also shows at the top of that user's chat. The
list shows by default only users who have written to the team ("Chats"), starred first, then newest; "Open", "★" and
"All" (every user) switch it. Stars and which user a to-do is about are kept by the admin only, in
`STATE_DIRECTORY/support-marks.json` (`/marks.json`, `POST /marks`). The list's logic is `supportList` in `app.js`,
tested in node by `test_support.py` (skipped without node). The Crashes tab shows a quiet count of open, non-expected entries. Every bubble can be completed or reopened;
viewing a thread changes no completion state. Badges and the Overview red curve count open user
messages only. App unread team replies still use `read_at`. Old JSON flags still serve user reports.
