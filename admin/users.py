#!/usr/bin/env python3
"""What the hubs know about their users, for the admin page: read-only from both databases, every 10 s (a systemd
timer, as root: the databases belong to the hubs' throwaway users). Writes ADMIN_USERS_FILE, which the admin
page serves after its login. Python's standard library only.

Per user: the name (as the hub keeps it, its skeleton), the user id, when it signed up, when one of its devices was
last seen (to the day: the hub writes it once a day, spec 6.4), its devices, whether they get pushes, whether the
account is secured (a backup record) and how many messages wait for it. No IP addresses: the hub never stores them.
"""

import json
import os
import sqlite3
import time
import subprocess
import shlex
import re
import glob
import datetime

HUBS = {"live": os.environ.get("YO_HUB_DB", "./hub.sqlite"),
        "dev": os.environ.get("YO_HUB_DEV_DB", "./hub-dev.sqlite")}
OUT = os.environ.get("ADMIN_USERS_FILE", "./users.json")
YOGPT = os.environ.get("YOGPT_STATS_PATTERN", "./yogpt/{}/stats.json")  # how many people YoGPT knows, written by YoGPT once a minute
DAY_MS = 86400 * 1000
WEEK_MS = 7 * DAY_MS
ENVS = {"live": os.environ.get("YO_HUB_ENV_FILE", "./hub.env"),
        "dev": os.environ.get("YO_HUB_DEV_ENV_FILE", "./hub-dev.env")}  # YO_MIN_APP: older apps get the update screen
LIMIT = 1000  # rows per hub; the totals count everyone

USERS = """
SELECT u.id, u.created_at,
       (SELECT coalesce(spelling, value) FROM handles WHERE user_id = u.id AND kind = 2 LIMIT 1),
       count(d.id), max(d.last_seen_at), max(d.push_platform), max(length(d.push_token) > 0),
       b.updated_at,
       (SELECT count(*) FROM messages m JOIN devices md ON md.id = m.device_id WHERE md.user_id = u.id),
       max(d.app)
FROM users u
LEFT JOIN devices d ON d.user_id = u.id
LEFT JOIN backups b ON b.user_id = u.id
GROUP BY u.id
ORDER BY max(d.last_seen_at) DESC, u.created_at DESC
LIMIT ?
"""

# Reports (spec 6.4): who reported whom, why, when; newest first. No content: the hub has none.
REPORTS = """
SELECT r.reporter_id, (SELECT coalesce(spelling, value) FROM handles WHERE user_id = r.reporter_id AND kind = 2),
       r.reported_id, (SELECT coalesce(spelling, value) FROM handles WHERE user_id = r.reported_id AND kind = 2),
       r.reason, r.created_at
FROM reports r ORDER BY r.created_at DESC LIMIT ?
"""
REASONS = {1: "spam", 2: "abuse", 3: "other"}
PUSH = {0: None, 1: "APNs", 2: "APNs (sandbox)", 3: "FCM"}


def text(v):
    return v.decode(errors="replace") if isinstance(v, bytes) else v


TABLE_ROWS = 2500  # per table in the Database widget: shown whole, the page scrolls (no box of its own)


def cell_value(v):
    """A value for the table view: short blobs as hex, long ones (ciphertexts, keys) by their length only."""
    if isinstance(v, bytes):
        return v.hex() if len(v) <= 32 else f"<{len(v)} bytes>"
    return v


def tables(db) -> list:
    out = []
    names = [r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")]
    for name in names:
        q = '"' + name.replace('"', '""') + '"'
        cur = db.execute(f"SELECT * FROM {q} LIMIT ?", (TABLE_ROWS,))
        out.append({"name": name, "columns": [c[0] for c in cur.description],
                    "rows": [[cell_value(v) for v in row] for row in cur],
                    "count": db.execute(f"SELECT count(*) FROM {q}").fetchone()[0]})
    return out


def yogpt_contacts(name: str):
    try:
        with open(YOGPT.format(name)) as f:
            return json.load(f).get("contacts")
    except (OSError, ValueError):
        return None


def min_app(name: str):
    """The hub's pinned minimum app (platform/build), None when not set."""
    try:
        with open(ENVS[name]) as f:
            for line in f:
                k, _, v = line.strip().partition("=")
                if k == "YO_MIN_APP" and v:
                    return v.strip().strip('"')
    except OSError:
        pass
    return None


def yogpt_min():
    """YoGPT's build as deployed: the deploy minute (UTC) of YOGPT_VERSION_FILE, less 2 minutes of slack."""
    import datetime
    try:
        with open(os.environ.get("YOGPT_VERSION_FILE", "./version-yogpt")) as f:
            at = datetime.datetime.strptime(f.read().split()[1], "%Y-%m-%dT%H:%M:%SZ")
        return "yogpt/" + (at - datetime.timedelta(minutes=2)).strftime("%Y%m%d%H%M")
    except (OSError, IndexError, ValueError):
        return None


def has_table(db, table: str) -> bool:
    return db.execute("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?", (table,)).fetchone()[0] > 0


EVENT_KINDS = ("yo-in", "yo-out", "media-in", "media-out", "group-create", "yo-in-bot", "media-in-bytes",
               "latency-1s", "latency-5s", "latency-60s", "latency-slow", "push-ok", "push-fail",
               "http", "http-4xx", "http-429", "http-5xx", "account-gone")
# A bot's account (alias u): it has devices and every one names a bot's app (YoGPT or the CLI), as the hub's is_bot_app.
BOT_USER = ("EXISTS (SELECT 1 FROM devices d WHERE d.user_id = u.id) AND NOT EXISTS (SELECT 1 FROM devices d"
            " WHERE d.user_id = u.id AND NOT (coalesce(d.app, '') LIKE 'yogpt/%' OR coalesce(d.app, '') LIKE 'cli/%'))")


def TODOS_SQL(db):  # noqa: N802
    """The team's to-dos as (id, text, created_at, done_at): `todos` (schema 7) or the X'' thread (schema 6)."""
    if has_table(db, "todos"):
        return "SELECT id, text, created_at, done_at FROM todos"
    if has_table(db, "support_messages"):
        return "SELECT id, text, created_at, done_at FROM support_messages WHERE user_id = X''"
    return None


def events(db, now: int) -> dict:
    """Per event kind: "hours" (the last 24, the current one last), "days" (the last 14 UTC days, today last),
    "today" (since UTC midnight), "total" (since the hub began counting) and "year" (the last 365 UTC days, today last), summed from the hub's `events` (counts per
    kind and minute, nothing else)."""
    hour, day = now // 3600000, now // DAY_MS
    by_hour = {}
    for kind, h, n in db.execute("SELECT kind, minute / 60, sum(n) FROM events WHERE minute >= ? GROUP BY 1, 2",
                                 ((day - 13) * 24 * 60,)):
        by_hour.setdefault(kind, {})[h] = n
    totals = dict(db.execute("SELECT kind, sum(n) FROM events GROUP BY 1").fetchall())
    by_day = {}
    for kind, d, n in db.execute("SELECT kind, minute / 1440, sum(n) FROM events WHERE minute >= ? GROUP BY 1, 2",
                                 ((day - 364) * 24 * 60,)):
        by_day.setdefault(kind, {})[d] = n
    out = {}
    for kind in EVENT_KINDS:
        hours = by_hour.get(kind, {})
        days = [sum(n for h, n in hours.items() if h // 24 == d) for d in range(day - 13, day + 1)]
        out[kind] = {"hours": [hours.get(h, 0) for h in range(hour - 23, hour + 1)], "days": days, "today": days[-1],
                     "total": totals.get(kind, 0), "year": [by_day.get(kind, {}).get(d, 0) for d in range(day - 364, day + 1)]}
    return out


LIVE_KINDS = ("yo-in", "media-in")  # people's Yos and media; a bot's inbox (yo-in-bot) left out
LIVE_MINUTES = 60


def live(db, now: int) -> dict:
    """The header's live strip: Yos (yo-in and media-in, bots left out) per minute for the last 60, the running
    minute last, and its UTC minute ("minute"), so the page can tell new Yos from ones it already showed. Counts only."""
    minute = now // 60000
    rows = dict(db.execute(f"SELECT minute, sum(n) FROM events WHERE kind IN ({','.join('?' * len(LIVE_KINDS))})"
                           " AND minute > ? AND minute <= ? GROUP BY 1",
                           (*LIVE_KINDS, minute - LIVE_MINUTES, minute)).fetchall())
    return {"minute": minute, "minutes": [rows.get(m, 0) for m in range(minute - LIVE_MINUTES + 1, minute + 1)]}


def retention(db, now: int) -> dict:
    """D1, D7, D30 as [came back, of]: of the accounts made 1, 7 or 30 to 90 days ago (bots left out), those seen
    (a device's last_seen_at) at least that many days after they signed up. Counts only."""
    out = {}
    for n in (1, 7, 30):
        out[f"d{n}"] = list(db.execute(
            "SELECT coalesce(sum(seen >= created_at + ?), 0), count(*) FROM (SELECT u.created_at,"
            " (SELECT max(d.last_seen_at) FROM devices d WHERE d.user_id = u.id) AS seen FROM users u"
            f" WHERE u.created_at >= ? AND u.created_at < ? AND NOT ({BOT_USER}))",
            (n * DAY_MS, now - 90 * DAY_MS, now - n * DAY_MS)).fetchone())
    return out


def open_problems(db, now: int) -> dict:
    spans = []
    if has_table(db, "support_messages"):
        spans += db.execute("SELECT created_at, done_at FROM support_messages WHERE length(user_id)>0 AND from_team = 0").fetchall()
    if has_table(db, "diagnostics"):
        for first, last, done, payload in db.execute("SELECT first_seen, last_seen, done, payload FROM diagnostics"):
            try:
                expected = (json.loads(payload or "{}") or {}).get("severity") == "expected"
            except (ValueError, AttributeError):
                expected = False
            if not expected:
                spans.append((first, last if done else None))
    at = lambda t: sum(1 for a, b in spans if a <= t and (b is None or b > t))  # noqa: E731
    hour, midnight = now // 3600000, now - now % DAY_MS
    return {"hours": [at(min(now, (h + 1) * 3600000)) for h in range(hour - 23, hour + 1)],
            "days": [at(min(now, midnight + (1 - i) * DAY_MS)) for i in range(364, -1, -1)]}


def hub(name: str, path: str) -> dict:
    """Each part on its own: one that fails (a table or column the hub's schema no longer has) comes out as None (or
    empty) with its error in "errors", the others still show."""
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=5)
    errors = {}

    def part(key, fn, empty=None):
        try:
            return fn()
        except (sqlite3.Error, OSError, ValueError, TypeError, AttributeError) as e:
            errors[key] = str(e)
            return empty

    try:
        one = lambda sql, *args: db.execute(sql, args).fetchone()[0]  # noqa: E731
        now = int(time.time() * 1000)
        week_ago = now - WEEK_MS
        midnight = now - now % DAY_MS  # UTC days
        # Devices.app (Yo-App, spec 6.3) comes with a newer hub.
        has_app = part("app", lambda: one("SELECT count(*) FROM pragma_table_info('devices') WHERE name = 'app'"), 0)
        users = part("users", lambda: [
            {"id": uid.hex(), "created": created, "name": text(name),
             "devices": devices, "last_seen": seen, "push": bool(push_token), "platform": platform,
             "secured": backup is not None, "backup_at": backup, "queued": queued, "app": text(app)}
            for uid, created, name, devices, seen, platform, push_token, backup, queued, app
            in db.execute(USERS if has_app else USERS.replace("max(d.app)", "NULL"), (LIMIT,))
        ], [])
        # Each user's devices, for the user panel: when added, last seen (to the day), push service, app build.
        device_list = part("device_list", lambda: [
            (uid.hex(), {"created": created, "last_seen": seen, "push": PUSH.get(platform, str(platform)) if has_token else None,
                         "app": text(app)})
            for uid, created, seen, platform, has_token, app in db.execute(
                "SELECT user_id, created_at, last_seen_at, push_platform, length(push_token) > 0, "
                + ("app" if has_app else "NULL") + " FROM devices ORDER BY last_seen_at DESC")], [])
        by_user = {}
        for uid, d in device_list:
            by_user.setdefault(uid, []).append(d)
        for u in users:
            u["device_list"] = by_user.get(u["id"], [])
        reports = part("reports", lambda: [
            {"id": a.hex()[:16] + b.hex()[:16], "reporter": a.hex(), "reporter_name": text(an), "reported": b.hex(), "reported_name": text(bn),
             "reason": REASONS.get(reason, str(reason)), "at": at}
            for a, an, b, bn, reason, at in (db.execute(REPORTS, (LIMIT,)) if has_table(db, "reports") else [])
        ], [])
        return {
            "users": users,
            "reports": reports,
            # The team's to-dos: their own table from the hub's schema 7, the X'' thread before it.
            "todos": part("todos", lambda: [{"id": i, "text": text(t), "at": at, "done_at": done}
                for i,t,at,done in db.execute(TODOS_SQL(db) + " ORDER BY done_at IS NOT NULL,created_at DESC LIMIT ?", (LIMIT,))] if TODOS_SQL(db) else [], []),
            "diagnostics_open": part("diagnostics_open", lambda: one("SELECT count(*) FROM diagnostics WHERE done=0") if has_table(db,"diagnostics") else 0),
            "todos_open": part("todos_open", lambda: one(f"SELECT count(*) FROM ({TODOS_SQL(db)}) WHERE done_at IS NULL") if TODOS_SQL(db) else 0),
            "diagnostic_days": part("diagnostic_days", lambda: [{"day": day, "kind": kind, "count": count}
                for day,kind,count in db.execute("SELECT day,kind,count FROM diagnostic_days ORDER BY day")]
                if has_table(db,"diagnostic_days") else [], []),
            "diagnostics": part("diagnostics", lambda: [dict(zip(
                ("id", "kind", "count", "first_seen", "last_seen", "app", "os", "model", "payload", "done"), row))
                for row in db.execute("SELECT id,kind,count,first_seen,last_seen,app,os,model,payload,done FROM diagnostics ORDER BY last_seen DESC LIMIT ?", (LIMIT,))]
                if has_table(db, "diagnostics") else [], []),
            # Support chat: unread user messages at the end of each of the last 14 UTC days (today: now), oldest
            # first; and each thread's last line, by the user id's hex (upper case, as the hub's thread list has it).
            "support_unread_days": part("support_unread_days", lambda: [one(
                "SELECT count(*) FROM support_messages WHERE length(user_id)>0 AND from_team = 0 AND created_at < ? AND (done_at IS NULL OR done_at >= ?)",
                end, end) for end in (min(now, midnight - (i - 1) * DAY_MS) for i in range(13, -1, -1))]
                if has_table(db, "support_messages") else [], []),
            # Problems open at each point: user messages not yet handled, crash and error groups not done (from
            # their first report; a done one counted until its last, the hub keeps no time of the tick), "expected"
            # ones left out. "hours": at the end of each of the last 24 (the current one: now), "days": at the end of
            # each of the last 365 UTC days (today: now). Reports are added by the page (their ticks live there).
            "problems_open": part("problems_open", lambda: open_problems(db, now)),
            # Sign-ups per UTC day since the first account, bots left out (every device a bot's: YoGPT or the CLI, as
            # the hub's db::is_bot_app), [[day, n], ...]: Sign-ups' bars and total.
            "signup_days": part("signup_days", lambda: [list(r) for r in db.execute(
                f"SELECT created_at / ?, count(*) FROM users u WHERE NOT ({BOT_USER}) GROUP BY 1 ORDER BY 1", (DAY_MS,))], []),
            # Messages users wrote to the team per UTC day, the last 365 (today last): Feedback's bars and total.
            "support_days": part("support_days", lambda: [one(
                "SELECT count(*) FROM support_messages WHERE length(user_id)>0 AND from_team = 0 AND created_at >= ? AND created_at < ?",
                (midnight - i * DAY_MS), (midnight - (i - 1) * DAY_MS)) for i in range(364, -1, -1)]
                if has_table(db, "support_messages") else None),
            # Messages users wrote to the team per hour, the last 24 (the current one last): the Feedback card's bars.
            "support_hours": part("support_hours", lambda: [one(
                "SELECT count(*) FROM support_messages WHERE length(user_id)>0 AND from_team = 0 AND created_at >= ? AND created_at < ?",
                h * 3600000, (h + 1) * 3600000) for h in range(now // 3600000 - 23, now // 3600000 + 1)]
                if has_table(db, "support_messages") else None),
            "support_last": part("support_last", lambda: {h: {"text": text(t)[:160], "team": bool(team)}
                for h, t, team, _ in db.execute(
                    "SELECT hex(user_id), text, from_team, max(id) FROM support_messages GROUP BY user_id LIMIT ?", (LIMIT,))}
                if has_table(db, "support_messages") else {}, {}),
            "tables": part("tables", lambda: tables(db), []),
            "total": part("total", lambda: one("SELECT count(*) FROM users")),
            "secured": part("secured", lambda: one("SELECT count(*) FROM backups")),
            "devices": part("devices", lambda: one("SELECT count(*) FROM devices")),
            "messages": part("messages", lambda: one("SELECT count(*) FROM messages")),
            # Stored reserved names; enforcement is a separate database setting.
            "reserved": part("reserved", lambda: one("SELECT count(*) FROM reserved")),
            # Statistics without anyone's friends: with Signal's protocol the hub keeps no chats or groups at all.
            "active_users": part("active_users", lambda: one(
                "SELECT count(DISTINCT user_id) FROM devices WHERE last_seen_at > ?", week_ago)),
            "active_today": part("active_today", lambda: one(
                "SELECT count(DISTINCT user_id) FROM devices WHERE last_seen_at > ?", now - DAY_MS)),
            # Users seen since UTC midnight (the admin page keeps its highest value per day).
            "active_day": part("active_day", lambda: one(
                "SELECT count(DISTINCT user_id) FROM devices WHERE last_seen_at >= ?", midnight)),
            # Users seen in the last 30 days (DAU/MAU with active_day), and retention D1/D7/D30.
            "active_month": part("active_month", lambda: one(
                "SELECT count(DISTINCT user_id) FROM devices WHERE last_seen_at > ?", now - 30 * DAY_MS)),
            "retention": part("retention", lambda: retention(db, now)),
            # Sign-ups per UTC day, the last 30 days, oldest first.
            "signups": part("signups", lambda: [one(
                "SELECT count(*) FROM users WHERE created_at >= ? AND created_at < ?",
                midnight - i * DAY_MS, midnight - (i - 1) * DAY_MS) for i in range(29, -1, -1)]),
            # Sign-ups per hour, the last 24 (the current hour last), for the overview's chart.
            "signups_hours": part("signups_hours", lambda: [one(
                "SELECT count(*) FROM users WHERE created_at >= ? AND created_at < ?",
                h * 3600000, (h + 1) * 3600000) for h in range(now // 3600000 - 23, now // 3600000 + 1)]),
            # Per sign-up UTC day, the last 14 (today last): [accounts made that day, those of them with a backup now].
            "secured_days": part("secured_days", lambda: [list(db.execute(
                "SELECT count(*), count(b.user_id) FROM users u LEFT JOIN backups b ON b.user_id = u.id"
                " WHERE u.created_at >= ? AND u.created_at < ?", (midnight - i * DAY_MS, midnight - (i - 1) * DAY_MS)).fetchone())
                for i in range(13, -1, -1)] if has_table(db, "backups") else None),
            # Groups on the hub now.
            "groups": part("groups", lambda: one("SELECT count(*) FROM zk_groups") if has_table(db, "zk_groups") else None),
            # The hub's per-minute event counts (no identifiers): per kind, the last 24 hours, the last 14 UTC days
            # and today; None on a hub without the table.
            "events": part("events", lambda: events(db, now) if has_table(db, "events") else None),
            # Yos per minute, the last hour, for the header's live strip (see live).
            "live": part("live", lambda: live(db, now) if has_table(db, "events") else None),
            "oldest_waiting": part("oldest_waiting", lambda: one("SELECT min(received_at) FROM messages")),
            "blobs": part("blobs", lambda: db.execute(
                "SELECT count(*), coalesce(sum(size), 0) FROM blobs").fetchone() if has_table(db, "blobs") else None),
            # Deliveries counted by the hub per hour (count only, kept 8 days); None on a hub without the table.
            "deliveries": part("deliveries", lambda: {"day": one(
                "SELECT coalesce(sum(n), 0) FROM deliveries_hourly WHERE hour >= ?", now // 3600000 - 24),
                # Per hour, the last 24 (the current one last), for the overview's graph.
                "hours": [dict(db.execute("SELECT hour, n FROM deliveries_hourly WHERE hour > ?", (now // 3600000 - 24,)).fetchall())
                          .get(h, 0) for h in range(now // 3600000 - 23, now // 3600000 + 1)],
                # Per UTC day, the last 14 (the hub keeps 8 days of hours: older days are None).
                "days": [None if (midnight - i * DAY_MS) // 3600000 < now // 3600000 - 8 * 24 else one(
                    "SELECT coalesce(sum(n), 0) FROM deliveries_hourly WHERE hour >= ? AND hour < ?",
                    (midnight - i * DAY_MS) // 3600000, (midnight - (i - 1) * DAY_MS) // 3600000) for i in range(13, -1, -1)]}
                if has_table(db, "deliveries_hourly") else None),
            "min_app": min_app(name),
            "yogpt_min": yogpt_min(),
            "yogpt_contacts": yogpt_contacts(name),
            "apps": part("apps", lambda: db.execute(
                "SELECT coalesce(app, 'unknown'), count(*) FROM devices GROUP BY 1 ORDER BY 2 DESC").fetchall()
                if has_app else [], []),
            # The same for the devices seen since UTC midnight: the Versions row's day (see version_day).
            "apps_today": part("apps_today", lambda: db.execute(
                "SELECT coalesce(app, 'unknown'), count(*) FROM devices WHERE last_seen_at >= ? GROUP BY 1 ORDER BY 2 DESC",
                (midnight,)).fetchall() if has_app else [], []),
            "size": part("size", lambda: os.path.getsize(path)),
            "errors": errors,
        }
    finally:
        db.close()



# Update checks belong to this root collector, never the HTTP process. Only OUT is written.
def update_path(key, default):
    return os.environ.get("ADMIN_UPDATES_" + key, default)


def update_command(key, default):
    result = subprocess.run(shlex.split(update_path(key, default)), capture_output=True,
                            text=True, timeout=120, env={**os.environ, "LC_ALL": "C", "PYTHONDONTWRITEBYTECODE": "1"})
    return result.returncode, result.stdout + "\n" + result.stderr


def apt_counts(output):
    match = re.search(r"^\s*(\d+);(\d+)\s*$", output, re.M)
    if not match:
        raise ValueError("apt-check returned no package/security counts")
    return {"packages": int(match[1]), "security": int(match[2])}


def kernel_key(value):
    return tuple((1, int(x)) if x.isdigit() else (0, x) for x in re.split(r"(\d+)", value))


def unattended_result(output):
    lines = output.splitlines()
    events = [line for line in lines if re.search(
        r"Starting unattended upgrades|All upgrades installed|No packages found|ERROR|WARNING|error message|Packages that will be upgraded", line, re.I)]
    return "\n".join(events[-12:]) or "No run result in log tail"


def update_status(data, now):
    apt = data.get("apt", {})
    overdue = (apt.get("security", 0) > 0 and now - apt.get("security_since", now) > 2 * 86400)
    overdue |= bool(data.get("reboot") and now - data.get("reboot_since", now) > 7 * 86400)
    if overdue:
        return "bad"
    if data.get("errors") or apt.get("error") or data.get("release", {}).get("error") or not apt or data.get("reboot") or apt.get("packages", 0) or data.get("release", {}).get("available"):
        return "warn"
    return "ok"


def updates(previous=None, now=None):
    now = time.time() if now is None else now
    previous = previous or {}
    data = {"at": now, "errors": []}

    def attempt(name, fn):
        try:
            return fn()
        except (OSError, ValueError, subprocess.SubprocessError) as exc:
            data["errors"].append(name + ": " + str(exc))
            return None

    def read_path(key, default):
        with open(update_path(key, default)) as f:
            return f.read()

    apt = dict(previous.get("apt", {}))
    if now - apt.get("checked", 0) >= 900:
        try:
            rc, output = update_command("APT_COMMAND", "/usr/lib/update-notifier/apt-check")
            if rc:
                raise ValueError(output.strip() or "apt-check failed")
            counts = apt_counts(output)
            counts["security_since"] = apt.get("security_since", now) if apt.get("security", 0) else now
            apt = {**counts, "checked": now}
        except (OSError, ValueError, subprocess.SubprocessError) as exc:
            apt.update(checked=now, error=str(exc))
    data["apt"] = apt
    marker = update_path("REBOOT_FILE", "/var/run/reboot-required")
    try:
        stat = os.stat(marker)
        data.update(reboot=True, reboot_since=stat.st_mtime)
    except FileNotFoundError:
        data["reboot"] = False
    except OSError as exc:
        data["errors"].append(str(exc))
    pkgs = update_path("REBOOT_PACKAGES", "/var/run/reboot-required.pkgs")
    data["reboot_packages"] = attempt("reboot packages", lambda: read_path("REBOOT_PACKAGES", pkgs).split()) if data.get("reboot") else []
    data["running_kernel"] = attempt("running kernel", lambda: update_command("UNAME_COMMAND", "/usr/bin/uname -r")[1].strip())
    kernels = glob.glob(update_path("KERNEL_GLOB", "/boot/vmlinuz-*"))
    data["installed_kernel"] = max((os.path.basename(p).removeprefix("vmlinuz-") for p in kernels), key=kernel_key, default=None)
    if not kernels:
        data["errors"].append("No installed kernel found")
    def log_tail():
        with open(update_path("UNATTENDED_LOG", "/var/log/unattended-upgrades/unattended-upgrades.log"), "rb") as f:
            f.seek(0, 2)
            f.seek(max(0, f.tell() - 32768))
            return unattended_result(f.read().decode(errors="replace"))
    data["unattended"] = attempt("unattended log", log_tail)
    def timer(key, default):
        rc, output = update_command(key, default)
        if rc:
            raise ValueError(output.strip())
        return output.strip()
    data["last_upgrade_trigger"] = attempt("upgrade timer", lambda: timer("UPGRADE_TIMER_COMMAND", "/usr/bin/systemctl show apt-daily-upgrade.timer --property=LastTriggerUSec --value"))
    data["next_reboot"] = attempt("reboot timer", lambda: timer("REBOOT_TIMER_COMMAND", "/usr/bin/systemctl show yo-reboot.timer --property=NextElapseUSecRealtime --value"))
    os_release = attempt("OS release", lambda: read_path("OS_RELEASE", "/etc/os-release")) or ""
    fields = dict(re.findall(r'^([A-Z_]+)=(.*)$', os_release, re.M))
    data["current_release"] = fields.get("PRETTY_NAME", "Unknown").strip('"')
    release = dict(previous.get("release", {}))
    if now - release.get("checked", 0) >= 86400:
        release = {"checked": now}
        try:
            config = read_path("RELEASE_CONFIG", "/etc/update-manager/release-upgrades")
            if not re.search(r"^\s*Prompt\s*=\s*lts\s*$", config, re.M):
                raise ValueError("Release check requires Prompt=lts")
            # Ubuntu's own daily check leaves its answer here (empty: no new release); the collector has no network.
            output = read_path("RELEASE_CACHE", "/var/lib/ubuntu-release-upgrader/release-upgrade-available")
            found = re.search(r"New release ['\"]([^'\"]+)", output)
            if found:
                release["available"] = found[1]
            release["result"] = output.strip() or "No new release found"

        except (OSError, ValueError, subprocess.SubprocessError) as exc:
            release["error"] = str(exc)
    data["release"] = release
    data["status"] = update_status(data, now)
    return data


# ---- App versions: current is the highest iOS build users report --------------------------------------------------
# Every TestFlight build is "yyyymmddHHMM"; anything else iOS reports (xtool's "1") is a dev build. Current is the
# highest build any user reports (the team's own included, no exceptions); older has an update. No network.
IOS_BUILD = re.compile(r"\d{12}")


def classify(app, newest=None, bot_min=None) -> dict:
    """One reported app ("platform/build"): its platform word, state ("current", "update" or None: unknown) and the
    details for the tooltip. newest: the highest iOS build any user reports."""
    platform, _, b = (app or "").partition("/")
    if not platform or not b:
        return {"label": "unknown", "state": None, "title": app or "no app reported"}
    if platform == "yogpt":
        top = bot_min.split("/")[-1] if bot_min else None
        state = None if not top else "current" if b >= top else "update"
        return {"label": "Bot", "state": state, "title": f"YoGPT build {b}" + (f"; newest {top}" if top else "")}
    if platform != "ios":
        name = {"android": "Android", "cli": "CLI"}.get(platform, platform)
        return {"label": name, "state": None, "title": f"{name} build {b}"}
    if not IOS_BUILD.fullmatch(b):
        return {"label": "iOS", "state": None, "title": f"iOS dev build {b} (dev/push.sh)"}
    state = None if not newest else "current" if b >= newest else "update"
    return {"label": "iOS", "state": state,
            "title": f"TestFlight build {b}" + (f"; newest reported {newest}" if newest and newest != b else "")}


def app_builds(out) -> dict:
    """Every reported app of both hubs ("ios/202611191257") with its label, state and details."""
    seen, reported = set(), set()
    for name in HUBS:
        h = out.get(name) or {}
        seen.update(a for a, _ in h.get("apps") or [])
        for u in h.get("users") or []:
            apps = {u.get("app")} | {d.get("app") for d in u.get("device_list") or []}
            seen.update(apps)
            reported.update(apps)
    seen = {a for a in seen if a and a != "unknown"}
    ios = [a.partition("/")[2] for a in reported if a and a.startswith("ios/") and IOS_BUILD.fullmatch(a[4:])]
    newest = max(ios) if ios else None
    bot = next((out[n].get("yogpt_min") for n in HUBS if isinstance(out.get(n), dict) and out[n].get("yogpt_min")), None)
    return {a: classify(a, newest, bot) for a in seen}


def below_min(app, minimum) -> bool:
    """A build below the hub's YO_MIN_APP ("ios/…,android/…", per platform): refused by the hub (as its request.rs:
    any numeric build, a dev build's "1" too), it must update."""
    p, _, b = (app or "").partition("/")
    for m in (minimum or "").split(","):
        mp, _, mb = m.strip().partition("/")
        if mp and p == mp and b.isdigit() and mb.isdigit() and int(b) < int(mb):
            return True
    return False


def version_day(apps, builds, minimum) -> dict:
    """Of the user devices seen today (bots and the CLI left out): how many run the current build, have an update
    out, must update (below the hub's minimum) or are unknown (dev builds, Android until it has builds)."""
    out = {"current": 0, "update": 0, "must": 0, "unknown": 0}
    for app, n in apps or []:
        if (app or "").startswith(("yogpt/", "cli/")):
            continue
        state = "must" if below_min(app, minimum) else (builds.get(app) or {}).get("state") or "unknown"
        out[state if state in out else "unknown"] += n
    return out


def finish(out):
    """What needs both hubs: every build's state, and each hub's versions of today."""
    out["app_builds"] = app_builds(out)
    for name in HUBS:
        h = out.get(name)
        if isinstance(h, dict) and "apps_today" in h:
            h["versions_day"] = version_day(h["apps_today"], out["app_builds"], h.get("min_app"))
    return out


def main():
    try:
        with open(OUT) as f:
            previous = json.load(f)
        previous = previous if isinstance(previous, dict) else {}
    except (OSError, ValueError):
        previous = {}
    out = {"at": int(time.time() * 1000), "updates": updates(previous.get("updates", {}))}
    for name, path in HUBS.items():
        try:
            out[name] = hub(name, path)
        except (sqlite3.Error, OSError) as e:
            out[name] = {"error": str(e)}
    finish(out)
    tmp = OUT + ".new"
    with open(tmp, "w") as f:
        json.dump(out, f, separators=(",", ":"))
    os.chmod(tmp, 0o640)  # names of users: the admin page's group only
    os.replace(tmp, OUT)


if __name__ == "__main__":
    main()
