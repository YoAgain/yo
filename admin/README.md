# Yo admin

Python standard library dashboard for server health, systemd services, hub users and database tables,
User Reports, feedback and To-Dos, reserved-name management, and YoGPT reply pictures. The collector (`users.py`)
reads SQLite databases read-only and atomically writes a JSON snapshot. To-Dos go through the hub API;
only done markers are stored locally. `migrate_todos.py` is the one-time legacy JSON migration.

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
