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

# Feedback sent from the apps (not end-to-end encrypted: meant for us), newest first.
FEEDBACK = """
SELECT f.id, (SELECT coalesce(spelling, value) FROM handles WHERE user_id = f.user_id AND kind = 2),
       f.text, f.app_version, f.created_at, {auto}, {kind}
FROM feedback f ORDER BY f.created_at DESC LIMIT ?
"""


def text(v):
    return v.decode(errors="replace") if isinstance(v, bytes) else v


TABLE_ROWS = 200  # per table in the "all tables" view


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


def has_column(db, table: str, column: str) -> bool:
    return db.execute("SELECT count(*) FROM pragma_table_info(?) WHERE name = ?", (table, column)).fetchone()[0] > 0


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
        feedback = part("feedback", lambda: [
            {"id": fid, "name": text(name), "text": text(body), "version": text(version), "at": at, "auto": bool(auto), "type": text(kind)}
            for fid, name, body, version, at, auto, kind in (db.execute(
                # Crash reports (the column of hubs from 2026-11-15 on): marked "auto", with no user.
                FEEDBACK.format(auto="f.auto" if has_column(db, "feedback", "auto") else "0",
                    kind="f.type" if has_column(db, "feedback", "type") else
                        "CASE WHEN " + ("f.auto" if has_column(db, "feedback", "auto") else "0") +
                        " = 1 THEN 'crash' WHEN f.app_version = 'admin' THEN 'todo' ELSE 'feedback' END"), (LIMIT,))
                if has_table(db, "feedback") else [])
        ], [])
        return {
            "users": users,
            "reports": reports,
            "feedback": [f for f in feedback if f["type"] in ("feedback", "todo")],
            "todos": part("todos", lambda: [{"id": i, "text": text(t), "at": at, "done_at": done}
                for i,t,at,done in db.execute("SELECT id,text,created_at,done_at FROM support_messages WHERE user_id=X'' ORDER BY done_at IS NOT NULL,created_at DESC LIMIT ?", (LIMIT,))] if has_table(db,"support_messages") else [], []),
            "diagnostics_open": part("diagnostics_open", lambda: one("SELECT count(*) FROM diagnostics WHERE done=0") if has_table(db,"diagnostics") else 0),
            "todos_open": part("todos_open", lambda: one("SELECT count(*) FROM support_messages WHERE user_id=X'' AND done_at IS NULL") if has_table(db,"support_messages") else 0),
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
            # Sign-ups per UTC day, the last 30 days, oldest first.
            "signups": part("signups", lambda: [one(
                "SELECT count(*) FROM users WHERE created_at >= ? AND created_at < ?",
                midnight - i * DAY_MS, midnight - (i - 1) * DAY_MS) for i in range(29, -1, -1)]),
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
            "size": part("size", lambda: os.path.getsize(path)),
            "errors": errors,
        }
    finally:
        db.close()



# ---- GitHub releases (public, no key): the newest public beta build ------------------------------------------------
# Every push to main is an internal TestFlight build; a published release also goes to the public beta and the
# TestFlight workflow writes its build into the release notes ("Build: 202611191257"). Fetched at most every
# RELEASES_EVERY_S, kept in OUT between runs; a failed fetch keeps the last value.
RELEASES_URL = "https://api.github.com/repos/YoAgain/yo/releases"
RELEASES_EVERY_S = 600
BUILD_LINE = re.compile(r"^Build:\s*(\d+)\s*$", re.M)


def newest_release(items) -> dict:
    """The newest non-draft release that names its build: {"build", "tag", "prerelease"}, or {} when none does."""
    found = []
    for r in items or []:
        m = BUILD_LINE.search(r.get("body") or "")
        if m and not r.get("draft"):
            found.append((r.get("published_at") or r.get("created_at") or "", m.group(1), r))
    if not found:
        return {}
    _, build, r = max(found, key=lambda x: x[0])
    return {"build": build, "tag": r.get("tag_name"), "prerelease": bool(r.get("prerelease"))}


def releases_fetch() -> list:
    import urllib.request
    req = urllib.request.Request(RELEASES_URL + "?per_page=30", headers={
        "Accept": "application/vnd.github+json", "User-Agent": "yo-admin"})
    with urllib.request.urlopen(req, timeout=8) as r:
        return json.load(r)


def releases(previous=None, now=None) -> dict:
    """The cached newest release, fetched again after RELEASES_EVERY_S; a failed fetch keeps the last value."""
    now = now or time.time()
    previous = previous or {}
    if previous.get("checked") and now - previous["checked"] < RELEASES_EVERY_S:
        return previous
    try:
        return {**newest_release(releases_fetch()), "checked": now}
    except Exception as e:  # noqa: BLE001  network or API: shown in the admin, the last value stays
        return {**{k: v for k, v in previous.items() if k != "error"}, "checked": now, "error": f"{type(e).__name__}: {e}"[:300]}


def short(b: str) -> str:
    """A timestamp build (yyyymmddHHMM) by its time of day, as the team calls them ("1257"); others as they are."""
    return b[-4:] if re.fullmatch(r"\d{12}", b) else b


def classify(app, release=None, bot_min=None) -> dict:
    """Label and state of one reported app ("platform/build"). An iOS build is compared with the newest public
    release: the same is "newest", older is "older" (an update is out), newer is a main build; no release known
    leaves the plain build. state is newest, older or None."""
    platform, _, b = (app or "").partition("/")
    if not platform or not b:
        return {"channel": "unknown", "label": "unknown", "state": None, "title": app or "no app reported"}
    if platform == "yogpt":
        state = None if not bot_min else "newest" if b >= bot_min.split("/")[-1] else "older"
        return {"channel": "bot", "label": f"YoGPT {short(b)}", "state": state, "title": f"YoGPT (bot) build {b}"}
    if platform != "ios":
        name = {"android": "Android", "cli": "CLI"}.get(platform, platform)
        return {"channel": platform, "label": f"{name} {short(b)}", "state": None, "title": f"{name} build {b}"}
    if not re.fullmatch(r"\d{12}", b):  # xtool dev builds report "1"
        return {"channel": "dev", "label": f"dev {b}", "state": None, "title": f"iOS dev build {b} (dev/push.sh)"}
    newest = (release or {}).get("build")
    if not newest:
        return {"channel": "ios", "label": f"iOS {short(b)}", "state": None, "title": f"iOS build {b}; no release known"}
    of = f"; newest release {release.get('tag') or ''} build {newest}" + (" (pre-release)" if release.get("prerelease") else "")
    if b == newest:
        return {"channel": "testflight", "label": "TestFlight · newest", "state": "newest", "title": f"iOS build {b}{of}"}
    if b < newest:
        return {"channel": "testflight", "label": "TestFlight · update", "state": "older", "title": f"iOS build {b}{of}"}
    return {"channel": "main", "label": "TestFlight (main)", "state": None, "title": f"iOS build {b}, from main{of}"}


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


def app_builds(out) -> dict:
    """Every reported app of both hubs ("ios/202611191257") with its label and state."""
    seen = set()
    for name in HUBS:
        h = out.get(name) or {}
        seen.update(a for a, _ in h.get("apps") or [])
        seen.update(u.get("app") for u in h.get("users") or [])
        seen.update(d.get("app") for u in h.get("users") or [] for d in u.get("device_list") or [])
    bot = next((out[n].get("yogpt_min") for n in HUBS if isinstance(out.get(n), dict) and out[n].get("yogpt_min")), None)
    return {a: classify(a, out.get("releases"), bot) for a in seen if a and a != "unknown"}


def main():
    try:
        with open(OUT) as f:
            previous = json.load(f)
        previous = previous if isinstance(previous, dict) else {}
    except (OSError, ValueError):
        previous = {}
    out = {"at": int(time.time() * 1000), "updates": updates(previous.get("updates", {}))}
    out["releases"] = releases(previous.get("releases"))
    for name, path in HUBS.items():
        try:
            out[name] = hub(name, path)
        except (sqlite3.Error, OSError) as e:
            out[name] = {"error": str(e)}
    out["app_builds"] = app_builds(out)
    tmp = OUT + ".new"
    with open(tmp, "w") as f:
        json.dump(out, f, separators=(",", ":"))
    os.chmod(tmp, 0o640)  # names of users: the admin page's group only
    os.replace(tmp, OUT)


if __name__ == "__main__":
    main()
