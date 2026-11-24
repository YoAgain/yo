#!/usr/bin/env python3
"""What the hubs know about their users, for the admin page: read-only from both databases, every 10 s (a systemd
timer, as root: the databases belong to the hubs' throwaway users). Writes ADMIN_USERS_FILE, which the admin
page serves after its login. Python's standard library only.

Per user: the name (as the hub keeps it, its skeleton), the user id, when it signed up, when one of its devices was
last seen (to the day: the hub writes it once a day, spec 6.4), its devices, whether they get pushes, whether the
account is secured (a backup record), how many messages wait for it and how many signed requests (to strangers,
spec 4.10) it sent in the last 24 hours. No IP addresses: the hub never stores them.
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

# Reports (spec 6.4): which install reported whom, why, when; newest first. No content: the hub has none. The
# reporter is a random install id (hub schema 9), never an account: no name to look up.
REPORTS = """
SELECT r.reporter, r.reported_id, (SELECT coalesce(spelling, value) FROM handles WHERE user_id = r.reported_id AND kind = 2),
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
        # The Errors tab: each crash or error group with its distinct installs (kept 90 days by the hub).
        extra = (", (SELECT count(DISTINCT install) FROM diagnostic_installs i WHERE i.diagnostic_id = diagnostics.id) AS installs"
                 if name == "diagnostics" and "diagnostic_installs" in names else "")
        cur = db.execute(f"SELECT *{extra} FROM {q} LIMIT ?", (TABLE_ROWS,))
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


def attestation(db, now):
    """Current account verdicts and active phones without any account verdict (spec 6.11)."""
    if not has_table(db, "attestation"):
        return None
    counts = {p: {"attested": 0, "failed": 0, "unknown": 0, "reasons": {}, "levels": {}, "last": None}
              for p in ("ios", "android")}
    for platform, state, reason, level, n, last in db.execute("""
        SELECT platform,state,reason,level,count(*),max(at_ms) FROM attestation
        GROUP BY platform,state,reason,level ORDER BY platform,state,reason,level"""):
        row = counts[platform]
        row[state] += n
        row["last"] = max(row["last"] or 0, last)
        if reason:
            row["reasons"][reason] = row["reasons"].get(reason, 0) + n
        if level:
            row["levels"][level] = row["levels"].get(level, 0) + n
    for platform, n in db.execute("""
        SELECT substr(app,1,instr(app,'/')-1),count(DISTINCT user_id) FROM devices d
        WHERE (app LIKE 'ios/%' OR app LIKE 'android/%') AND last_seen_at > ?
        AND NOT EXISTS (SELECT 1 FROM attestation a WHERE a.user_id=d.user_id) GROUP BY 1""", (now - 2 * DAY_MS,)):
        counts[platform]["unknown"] = n
    return counts


def has_table(db, table: str) -> bool:
    return db.execute("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?", (table,)).fetchone()[0] > 0


def push_backlog() -> int:
    """The hub's YO_PUSH_BACKLOG (the admin runs with the same environment file), default 50."""
    try:
        return int(os.environ.get("YO_PUSH_BACKLOG", "50"))
    except ValueError:
        return 50


EVENT_KINDS = ("yo-in", "yo-out", "media-in", "media-out", "group-create", "yo-in-bot", "media-in-bytes",
               "latency-1s", "latency-5s", "latency-60s", "latency-slow", "push-ok", "push-fail",
               "http", "http-4xx", "http-429", "http-5xx", "account-gone", "push-folded", "push-paced", "crash", "error")
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
    # The last 60 minutes per kind (the current minute last): the page's 1H view.
    minute = now // 60000
    by_minute = {}
    for kind, m, n in db.execute("SELECT kind, minute, sum(n) FROM events WHERE minute > ? AND minute <= ? GROUP BY 1, 2",
                                 (minute - 60, minute)):
        by_minute.setdefault(kind, {})[m] = n
    totals = dict(db.execute("SELECT kind, sum(n) FROM events GROUP BY 1").fetchall())
    by_day = {}
    for kind, d, n in db.execute("SELECT kind, minute / 1440, sum(n) FROM events WHERE minute >= ? GROUP BY 1, 2",
                                 ((day - 364) * 24 * 60,)):
        by_day.setdefault(kind, {})[d] = n
    out = {}
    # The operator's bots, counted apart (bot-yo, bot-media, bot-media:LABEL): out of the kinds above.
    bots = sorted(k for k in totals if k.startswith("bot-"))
    for kind in EVENT_KINDS + tuple(bots):
        hours = by_hour.get(kind, {})
        days = [sum(n for h, n in hours.items() if h // 24 == d) for d in range(day - 13, day + 1)]
        out[kind] = {"hours": [hours.get(h, 0) for h in range(hour - 23, hour + 1)], "days": days, "today": days[-1], "minutes": [by_minute.get(kind, {}).get(m, 0) for m in range(minute - 59, minute + 1)],
                     "total": totals.get(kind, 0), "year": [by_day.get(kind, {}).get(d, 0) for d in range(day - 364, day + 1)]}
    return out


def week_series(db, hour, has_app):
    """28 six-hour windows ending with the current hour, aligned across metrics."""
    start = hour - 167

    def counts(table, condition):
        slots = [0] * 28
        for i, n in db.execute(
                f"SELECT (created_at / 3600000 - ?) / 6, count(*) FROM {table} "
                f"WHERE created_at >= ? AND created_at < ? AND {condition} GROUP BY 1",
                (start, start * 3600000, (hour + 1) * 3600000)):
            slots[i] = n
        return slots

    events = {}
    if has_table(db, "events"):
        for kind, i, n in db.execute(
                "SELECT kind, (minute / 60 - ?) / 6, sum(n) FROM events "
                "WHERE minute >= ? AND minute < ? GROUP BY 1, 2",
                (start, start * 60, (hour + 1) * 60)):
            events.setdefault(kind, [0] * 28)[i] = n
    return {"start": start, "events": events,
            "signups": counts("users u", f"NOT ({BOT_USER})" if has_app else "1"),
            "feedback": counts("support_messages", "from_team = 0")
            if has_table(db, "support_messages") else None}


LIVE_KINDS = ("yo-in", "media-in")  # people's Yos and media
LIVE_BOT_KINDS = ("yo-in-bot", "bot-yo", "bot-media")  # a bot's: into its inbox, or sent by it (blue on the page)
LIVE_MINUTES = 60


def live(db, now: int) -> dict:
    """The header's live strip: all Yos (people's yo-in and media-in, and bots' in "bots") per minute for the last 60,
    the running minute last, and its UTC minute ("minute"), so the page can tell new Yos from ones it already showed.
    Counts only."""
    minute = now // 60000

    def per_minute(kinds):
        rows = dict(db.execute(f"SELECT minute, sum(n) FROM events WHERE kind IN ({','.join('?' * len(kinds))})"
                               " AND minute > ? AND minute <= ? GROUP BY 1",
                               (*kinds, minute - LIVE_MINUTES, minute)).fetchall())
        return [rows.get(m, 0) for m in range(minute - LIVE_MINUTES + 1, minute + 1)]

    people, bots = per_minute(LIVE_KINDS), per_minute(LIVE_BOT_KINDS)
    return {"minute": minute, "minutes": [p + b for p, b in zip(people, bots)], "bots": bots}


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


# Crash-free installs (estimated) per range of the page ("1" the last 24 hours, "7", "30", "0" Total): UTC days back
# from today, the 24 hours as today and yesterday (the hub keeps an install's crash by the day only).
CRASH_FREE_DAYS = {"1": 2, "7": 7, "30": 30, "0": 90}
# The phone apps: macOS and the bots (YoGPT, the CLI) left out, as on the rest of the page.
PHONE_APP = "(coalesce({0}, '') LIKE 'ios/%' OR coalesce({0}, '') LIKE 'android/%')"


# The network's own errors (domain, code), not the app's: a lost, timed-out or refused connection, a busy or failing
# server, a slow request. They never count against a phone in the Error-free stat. The hub keeps the same list for its
# hourly levels (`NETWORK_ERRORS` in hub/src/db.rs): change both together.
NETWORK_ERRORS = [("NSURLErrorDomain", code) for code in ("-1001", "-1004", "-1005", "-1009", "-1020")] \
    + [("HTTP", code) for code in ("429", "502", "503", "504")] \
    + [("NotificationFallback", "network"), ("SharedAccess", "a slow request")]


def not_network(alias: str = "d") -> str:
    """SQL on a diagnostics row (`alias`) that is none of NETWORK_ERRORS."""
    each = " OR ".join(f"(coalesce(json_extract({alias}.payload, '$.domain'), '') = '{domain}'"
                       f" AND coalesce(json_extract({alias}.payload, '$.code'), '') = '{code}')" for domain, code in NETWORK_ERRORS)
    return f"NOT ({each})"


def crash_free(db, now: int):
    """Per range: [installs that sent a crash, phone devices seen] since the range's first UTC day, both from the
    phone apps (iOS, Android; not macOS). The hub has no app start per install id, so the devices seen stand for the
    installs in use (one install, one device): an estimate. None on an older hub without diagnostic_installs."""
    if not has_table(db, "diagnostic_installs"):
        return None
    out = {}
    for key, days in CRASH_FREE_DAYS.items():
        first = now // DAY_MS - days + 1
        crashed = db.execute("SELECT count(DISTINCT i.install) FROM diagnostic_installs i JOIN diagnostics d ON d.id = i.diagnostic_id"
                             f" WHERE d.kind = 'crash' AND i.day >= ? AND {PHONE_APP.format('d.app')} AND d.os NOT LIKE 'mac%'",
                             (first,)).fetchone()[0]
        seen = db.execute(f"SELECT count(*) FROM devices WHERE last_seen_at >= ? AND {PHONE_APP.format('app')}",
                          (first * DAY_MS,)).fetchone()[0]
        out[key] = [crashed, seen]
    return out


def problem_installs(db, now: int):
    """Per range (as crash_free): the distinct phone installs that sent an error, and any error or crash, since the
    range's first UTC day: counts only. None on an older hub without diagnostic_installs."""
    if not has_table(db, "diagnostic_installs"):
        return None
    out = {}
    for key, days in CRASH_FREE_DAYS.items():
        first = now // DAY_MS - days + 1
        row = db.execute("SELECT count(DISTINCT CASE WHEN d.kind = 'error' THEN i.install END), count(DISTINCT i.install)"
                         " FROM diagnostic_installs i JOIN diagnostics d ON d.id = i.diagnostic_id"
                         f" WHERE i.day >= ? AND {PHONE_APP.format('d.app')} AND d.os NOT LIKE 'mac%' AND {not_network()}", (first,)).fetchone()
        out[key] = [row[0], row[1]]
        # Per platform: installs with an error or crash, phones seen (the same days, apps as in crash_free).
        for prefix in ("ios", "android"):
            hit = db.execute("SELECT count(DISTINCT i.install) FROM diagnostic_installs i JOIN diagnostics d ON d.id = i.diagnostic_id"
                             f" WHERE i.day >= ? AND d.app LIKE ? AND d.os NOT LIKE 'mac%' AND {not_network()}", (first, prefix + "/%")).fetchone()[0]
            seen = db.execute("SELECT count(*) FROM devices WHERE last_seen_at >= ? AND app LIKE ?", (first * DAY_MS, prefix + "/%")).fetchone()[0]
            crash = db.execute("SELECT count(DISTINCT i.install) FROM diagnostic_installs i JOIN diagnostics d ON d.id = i.diagnostic_id"
                               " WHERE d.kind = 'crash' AND i.day >= ? AND d.app LIKE ? AND d.os NOT LIKE 'mac%'", (first, prefix + "/%")).fetchone()[0]
            out[key] += [hit, seen, crash]
    return out


def current_build_installs(db, now: int):
    """[phones on current builds, of them with an error or crash report in the last 24 hours; then the same for iOS and Android]. Current: builds under 24 hours
    old by their id (`ios/YYYYMMDDHHMM`, UTC); the phones are the latest `build:<app>` levels of them, the problem phones the
    distinct installs with a report from such a build in the last 24 hours. Counts only; None on an older hub."""
    if not (has_table(db, "diagnostic_installs") and has_table(db, "levels")):
        return None
    current = {}
    for kind, value in db.execute("SELECT l.kind, l.value FROM levels l JOIN (SELECT kind, max(hour) AS hour FROM levels"
                                  " WHERE kind LIKE 'build:%' GROUP BY kind) m ON m.kind = l.kind AND m.hour = l.hour"):
        app = kind[6:]
        try:
            stamp = datetime.datetime.strptime(app.split("/", 1)[1], "%Y%m%d%H%M").replace(tzinfo=datetime.timezone.utc)
        except (IndexError, ValueError):
            continue
        if app.split("/", 1)[0] in ("ios", "android") and 0 <= now - stamp.timestamp() * 1000 < DAY_MS:
            current[app] = value
    stamp = ("coalesce(i.at, i.day * 86400000)" if "at" in {r[1] for r in db.execute("PRAGMA table_info(diagnostic_installs)")}
             else "i.day * 86400000")
    out = []
    for prefix in ("", "ios/", "android/"):
        apps = [a for a in current if a.startswith(prefix)]
        hit = 0
        if apps:
            marks = ",".join("?" * len(apps))
            hit = db.execute("SELECT count(DISTINCT i.install) FROM diagnostic_installs i JOIN diagnostics d ON d.id = i.diagnostic_id"
                             f" WHERE {stamp} > ? AND d.app IN ({marks}) AND d.os NOT LIKE 'mac%' AND {not_network()}",
                             (now - DAY_MS, *apps)).fetchone()[0]
        out += [sum(current[a] for a in apps), hit]
    return out


def daily_problem_installs(db, now: int):
    if not has_table(db, "diagnostic_installs"):
        return None
    today = now // DAY_MS
    return dict(db.execute(
        "SELECT i.day, count(DISTINCT i.install) FROM diagnostic_installs i"
        " JOIN diagnostics d ON d.id = i.diagnostic_id"
        f" WHERE i.day BETWEEN ? AND ? AND {PHONE_APP.format('d.app')}"
        f" AND d.os NOT LIKE 'mac%' AND d.kind IN ('error', 'crash') AND {not_network()} GROUP BY i.day",
        (today - CRASH_FREE_DAYS["0"] + 1, today)))


def diagnostics(db):
    installs = ("(SELECT count(DISTINCT install) FROM diagnostic_installs WHERE diagnostic_id = d.id)"
                if has_table(db, "diagnostic_installs") else "NULL")
    cursor = db.execute(f"SELECT d.*, {installs} AS installs FROM diagnostics d ORDER BY last_seen DESC")
    columns = [c[0] for c in cursor.description]
    rows = [dict(zip(columns, row)) for row in cursor]
    # Its builds ([app, reports, last seen]) and its reports in the last 24 hours, per hour (newer hubs, spec 6.4).
    builds, hours = {}, {}
    if has_table(db, "diagnostic_builds"):
        for i, app, n, seen in db.execute("SELECT diagnostic_id, app, count, last_seen FROM diagnostic_builds ORDER BY app"):
            builds.setdefault(i, []).append([app, n, seen])
    if has_table(db, "diagnostic_hours"):
        now_h = int(time.time()) // 3600
        for i, hour, n in db.execute("SELECT diagnostic_id, hour, count FROM diagnostic_hours WHERE hour > ?", (now_h - 24,)):
            hours.setdefault(i, [0] * 24)[min(23, max(0, 23 - (now_h - hour)))] += n
    for r in rows:
        r["builds"], r["hours_24h"] = builds.get(r["id"], []), hours.get(r["id"], [0] * 24)
    return with_causes(db, rows)


def with_causes(db, rows):
    """Each diagnostics row with its cause's totals over every group of it (builds, lines): reports (sum of counts) and
    the distinct installs. Counts only."""
    if not rows or not has_table(db, "diagnostic_installs"):
        return rows
    ids = {}
    for diagnostic_id, install in db.execute("SELECT diagnostic_id, install FROM diagnostic_installs"):
        ids.setdefault(diagnostic_id, set()).add(install)
    total = {}
    for r in rows:
        try:
            p = json.loads(r["payload"] or "{}") or {}
            p = p if isinstance(p, dict) else {}
        except ValueError:
            p = {}
        r["_cause"] = problem_cause(r["kind"], p)
        t = total.setdefault(r["_cause"], [0, set()])
        t[0] += r["count"] or 0
        t[1] |= ids.get(r["id"], set())
    for r in rows:
        t = total[r.pop("_cause")]
        r["cause_reports"], r["cause_installs"] = t[0], len(t[1])
    return rows


def problem_cause(kind: str, payload: dict):
    """What a crash or error group is about, without the build it came from or the line it was raised on: the hub keeps
    a group per build ("app") and per line, so every new build would open the same problem again."""
    if kind == "crash":
        return (kind, payload.get("exception", ""), payload.get("termination", ""))
    place = re.sub(r":\d+", "", re.sub(r"^\w+/", "", str(payload.get("place", ""))))  # YoNotify/Hub.swift:240 -> Hub.swift
    return (kind, payload.get("domain", ""), payload.get("code", ""), place)


def open_problems(db, now: int) -> dict:
    """Problems open at each point: each support message not handled, and each cause (problem_cause) with a crash or
    error group not done, "expected" ones left out."""
    spans = []
    if has_table(db, "support_messages"):
        spans += [(a, b, None) for a, b in
                  db.execute("SELECT created_at, done_at FROM support_messages WHERE from_team = 0").fetchall()]
    if has_table(db, "diagnostics"):
        for kind, first, last, done, payload in db.execute("SELECT kind, first_seen, last_seen, done, payload FROM diagnostics"):
            try:
                p = json.loads(payload or "{}") or {}
                p = p if isinstance(p, dict) else {}
            except ValueError:
                p = {}
            if p.get("severity") != "expected":
                spans.append((first, last if done else None, problem_cause(kind, p)))

    def at(t):
        open_ = [(a, b, c) for a, b, c in spans if a <= t and (b is None or b > t)]
        return sum(1 for *_, c in open_ if c is None) + len({c for *_, c in open_ if c is not None})
    hour, midnight = now // 3600000, now - now % DAY_MS
    return {"hours": [at(min(now, (h + 1) * 3600000)) for h in range(hour - 23, hour + 1)],
            "days": [at(min(now, midnight + (1 - i) * DAY_MS)) for i in range(364, -1, -1)]}


def levels(db, now: int, messages=None, blobs=None) -> dict:
    """The hub's levels (`levels`: counts only, sampled at each hour's start; older than 30 days only the day's last,
    taken at the next midnight, UTC) as the page's histories. A sample at an hour's start is the level at the end of
    the hour before, so its day is (hour - 1) // 24. "active48" [[hour, accounts seen within 48 hours], ...] (from when the hub samples it), "active_days" {day: users seen in its 24 hours} (today left out:
    the page has active_day), "queue_hours" {hour: [envelopes, bytes]} (48 hours) and "queue_days" {day: [...]} with
    the current values for this hour and today, "open_report_history" [{t, errors_open, crashes_open}]."""
    rows = {}
    for kind, hour, value in db.execute("SELECT kind, hour, value FROM levels ORDER BY hour"):
        rows.setdefault(kind, {})[hour] = value
    hour_now, today = now // 3600000, now // DAY_MS
    by_day = lambda kind: {(h - 1) // 24: v for h, v in rows.get(kind, {}).items()}  # noqa: E731 (in order: the day's last)
    env, media = rows.get("queue-envelopes", {}), rows.get("queue-media-bytes", {})
    hours = {str(h): [v, media.get(h, 0)] for h, v in env.items() if hour_now - h < 48}
    day_media = by_day("queue-media-bytes")
    days = {str(d): [v, day_media.get(d, 0)] for d, v in by_day("queue-envelopes").items()}
    if isinstance(messages, int) and isinstance(blobs, (list, tuple)) and len(blobs) == 2:
        hours[str(hour_now)] = days[str(today)] = [messages, blobs[1] or 0]
    errors, crashes = rows.get("open-errors", {}), rows.get("open-crashes", {})
    builds, error_free = {}, {}
    for kind, values in rows.items():
        if kind.startswith("build:"):
            for h, v in values.items():
                # Older than a week every 4th hour alone (the 30-day view's step): the page's size kept small.
                if v > 0 and (hour_now - h < 168 or h % 4 == 0 or hour_now - h > 30 * 24):
                    builds.setdefault(str(h), {})[kind[6:]] = v
    # [phones, with a report, iOS phones, iOS with a report, Android phones, Android with a report]; None where the hub
    # had no per-platform level yet.
    for h, phones in rows.get("phones-24h", {}).items():
        if phones > 0 and (hour_now - h < 168 or h % 4 == 0 or hour_now - h > 30 * 24):
            row = [phones, rows.get("problem-installs-24h", {}).get(h, 0)]
            for p in ("ios", "android"):
                n = rows.get(f"phones-24h:{p}", {}).get(h)
                row += [n, rows.get(f"problem-installs-24h:{p}", {}).get(h, 0)] if n is not None else [None, None]
            error_free[str(h)] = row
    return {
        "active48": sorted(rows.get("active-48h", {}).items()),
        "active_days": {str(d): v for d, v in by_day("active-24h").items() if d < today},
        "queue_hours": hours,
        "queue_week": {str(h): [v, media.get(h, 0)] for h, v in env.items() if hour_now - 168 < h <= hour_now} | hours,
        "queue_days": days,
        "open_report_history": [{"t": h * 3600000 - 1, "errors_open": errors.get(h, 0), "crashes_open": crashes.get(h, 0)}
                                for h in sorted(set(errors) | set(crashes))],
        # {hour: {build: devices seen within 24 hours}} (`build:…`, hourly for a week, every 4th hour to 30 days, then
        # the day's last, at midnight): the releases over time.
        "build_levels": builds,
        # {hour: [phones seen within 24 hours, of them with an error or crash report in those 24 hours, then the same for iOS and Android]}: the error-free share.
        "error_free_levels": error_free,
    }


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
        # Signed requests (spec 4.10) each user sent in the last 24 hours: strangers' floods. The hub counts requests
        # only, per hour (request_hours, kept 25 hours); a contact's delivery is sealed and never counted by sender.
        hour = now // 3_600_000
        requests = dict(part("requests_24h", lambda: db.execute(
            "SELECT user_id, sum(n) FROM request_hours WHERE hour > ? GROUP BY user_id", (hour - 24,)).fetchall()
            if has_table(db, "request_hours") else [], []))
        for u in users:
            u["requests_24h"] = requests.get(bytes.fromhex(u["id"]), 0)
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
            {"id": a.hex()[:16] + b.hex()[:16], "reporter": a.hex(), "reported": b.hex(), "reported_name": text(bn),
             "reason": REASONS.get(reason, str(reason)), "at": at}
            for a, b, bn, reason, at in (db.execute(REPORTS, (LIMIT,)) if has_table(db, "reports") else [])
        ], [])
        out = {
            "users": users,
            "reports": reports,
            # The team's to-dos: their own table from the hub's schema 7, the X'' thread before it.
            "todos": part("todos", lambda: [{"id": i, "text": text(t), "at": at, "done_at": done}
                for i,t,at,done in db.execute(TODOS_SQL(db) + " ORDER BY done_at IS NOT NULL,created_at DESC LIMIT ?", (LIMIT,))] if TODOS_SQL(db) else [], []),
            "open_report_counts": part("open_report_counts", lambda: dict(db.execute(
                "SELECT kind, sum(count) FROM diagnostics WHERE done=0 GROUP BY kind"))
                if has_table(db, "diagnostics") else {}),
            "diagnostics_open": part("diagnostics_open", lambda: one("SELECT count(*) FROM diagnostics WHERE done=0") if has_table(db,"diagnostics") else 0),
            "todos_open": part("todos_open", lambda: one(f"SELECT count(*) FROM ({TODOS_SQL(db)}) WHERE done_at IS NULL") if TODOS_SQL(db) else 0),
            "diagnostic_days": part("diagnostic_days", lambda: [{"day": day, "kind": kind, "count": count}
                for day,kind,count in db.execute("SELECT day,kind,count FROM diagnostic_days ORDER BY day")]
                if has_table(db,"diagnostic_days") else [], []),
            "diagnostics": part("diagnostics", lambda: diagnostics(db) if has_table(db, "diagnostics") else [], []),
            "daily_problem_installs": part("daily_problem_installs", lambda: daily_problem_installs(db, now)),
            "problem_installs": part("problem_installs", lambda: problem_installs(db, now)),
            # Crash-free installs (estimated) per range: installs with a crash against the phones seen.
            "current_build_installs": part("current_build_installs", lambda: current_build_installs(db, now)),
            "crash_free": part("crash_free", lambda: crash_free(db, now)),
            # Support chat: unread user messages at the end of each of the last 14 UTC days (today: now), oldest
            # first; and each thread's last line, by the install id's hex (lower case, as the hub's thread list has it).
            "support_unread_days": part("support_unread_days", lambda: [one(
                "SELECT count(*) FROM support_messages WHERE from_team = 0 AND created_at < ? AND (done_at IS NULL OR done_at >= ?)",
                end, end) for end in (min(now, midnight - (i - 1) * DAY_MS) for i in range(13, -1, -1))]
                if has_table(db, "support_messages") else [], []),
            # Problems open at each point: user messages not yet handled, causes with a crash or error group not done (from
            # their first report; a done one counted until its last, the hub keeps no time of the tick), "expected"
            # ones left out. "hours": at the end of each of the last 24 (the current one: now), "days": at the end of
            # each of the last 365 UTC days (today: now). Reports are added by the page (their ticks live there).
            "problems_open": part("problems_open", lambda: open_problems(db, now)),
            # Sign-ups per UTC day since the first account, bots left out (every device a bot's: YoGPT or the CLI, as
            # the hub's db::is_bot_app), [[day, n], ...]: Sign-ups' bars and total.
            "week": part("week", lambda: week_series(db, hour, has_app)),
            "signup_days": part("signup_days", lambda: [list(r) for r in db.execute(
                f"SELECT created_at / ?, count(*) FROM users u WHERE NOT ({BOT_USER}) GROUP BY 1 ORDER BY 1", (DAY_MS,))], []),
            # Messages users wrote to the team per UTC day, the last 365 (today last): Feedback's bars and total.
            "support_days": part("support_days", lambda: [one(
                "SELECT count(*) FROM support_messages WHERE from_team = 0 AND created_at >= ? AND created_at < ?",
                (midnight - i * DAY_MS), (midnight - (i - 1) * DAY_MS)) for i in range(364, -1, -1)]
                if has_table(db, "support_messages") else None),
            # Messages users wrote to the team per hour, the last 24 (the current one last): the Feedback card's bars.
            "support_hours": part("support_hours", lambda: [one(
                "SELECT count(*) FROM support_messages WHERE from_team = 0 AND created_at >= ? AND created_at < ?",
                h * 3600000, (h + 1) * 3600000) for h in range(now // 3600000 - 23, now // 3600000 + 1)]
                if has_table(db, "support_messages") else None),
            "support_last": part("support_last", lambda: {h: {"text": text(t)[:160], "team": bool(team)}
                for h, t, team, _ in db.execute(
                    "SELECT lower(hex(install)), text, from_team, max(id) FROM support_messages GROUP BY install LIMIT ?", (LIMIT,))}
                if has_table(db, "support_messages") else {}, {}),
            "tables": part("tables", lambda: tables(db), []),
            "total": part("total", lambda: one("SELECT count(*) FROM users")),
            "secured": part("secured", lambda: one("SELECT count(*) FROM backups")),
            "devices": part("devices", lambda: one("SELECT count(*) FROM devices")),
            "messages": part("messages", lambda: one("SELECT count(*) FROM messages")),
            # Stored reserved names; enforcement is a separate database setting.
            "reserved": part("reserved", lambda: one("SELECT count(*) FROM reserved")),
            # Statistics without anyone's friends: with Signal's protocol the hub keeps no chats or groups at all.
            "attestation": part("attestation", lambda: attestation(db, now)),
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
            # Devices in push pacing now: more envelopes waiting than the hub's YO_PUSH_BACKLOG (default 50), a count only.
            "push_backlog_devices": part("push_backlog_devices", lambda: one(
                "SELECT count(*) FROM (SELECT 1 FROM messages GROUP BY device_id HAVING count(*) > ?)", push_backlog())),
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
            # The version each build named (Yo-App, spec 6.3; devices.version comes with a newer hub).
            "app_versions": part("app_versions", lambda: dict(db.execute(
                "SELECT app, max(version) FROM devices WHERE app IS NOT NULL AND version IS NOT NULL GROUP BY app")
                .fetchall()) if has_app and one("SELECT count(*) FROM pragma_table_info('devices') WHERE name = 'version'")
                else {}, {}),
            "size": part("size", lambda: os.path.getsize(path)),
        }
        # The hub's hourly levels as the page's histories (see levels); a hub without the table has none.
        out.update(part("levels", lambda: levels(db, now, out["messages"], out["blobs"]) if has_table(db, "levels") else {}, {}))
        out["errors"] = errors
        return out
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


# ---- App versions: current is the highest build that at least two users report, per platform -------------------
# Every TestFlight and Play build is its UTC minute "yyyymmddHHMM"; anything else (xtool's "1") is a dev build. A
# build only one user has is "fresh": everyone on the previous build stays "current". Once a second user has it, it
# becomes the baseline and older builds are "update". If no build has two users, the highest build is the baseline
# (single-user builds are then simply current). The team's own users count like any other. Each platform is judged
# on its own builds. No network.
STORE_BUILD = re.compile(r"\d{12}")
# The releases' builds from before the apps named their version in Yo-App (spec 6.3: "ios/202611241344 1.0.0-RC36";
# devices.version): each the TestFlight, Play or Android (floss APK) build of a GitHub release (event release, v1.0.0-RCn),
# its stamp read once from the runs' logs. Builds that are no release are not here and keep their date.
RELEASES = {
    "ios/202611191626": "1.0.0-RC", "ios/202611192349": "1.0.0-RC2", "ios/202611200118": "1.0.0-RC3",
    "ios/202611201341": "1.0.0-RC4", "ios/202611201522": "1.0.0-RC5", "ios/202611201742": "1.0.0-RC6",
    "ios/202611202019": "1.0.0-RC7", "ios/202611202133": "1.0.0-RC8", "ios/202611202245": "1.0.0-RC9",
    "android/202611202313": "1.0.0-RC10", "ios/202611202315": "1.0.0-RC10", "android/202611202317": "1.0.0-RC10",
    "ios/202611202353": "1.0.0-RC11", "android/202611210651": "1.0.0-RC12", "ios/202611210652": "1.0.0-RC12",
    "android/202611210716": "1.0.0-RC13", "ios/202611210719": "1.0.0-RC13", "android/202611210924": "1.0.0-RC14",
    "ios/202611210927": "1.0.0-RC14", "android/202611210935": "1.0.0-RC14", "android/202611211145": "1.0.0-RC15",
    "ios/202611211148": "1.0.0-RC15", "android/202611211152": "1.0.0-RC15", "android/202611211219": "1.0.0-RC16",
    "ios/202611211223": "1.0.0-RC16", "android/202611211233": "1.0.0-RC16", "android/202611211414": "1.0.0-RC17",
    "ios/202611211417": "1.0.0-RC17", "android/202611211421": "1.0.0-RC17", "android/202611211644": "1.0.0-RC18",
    "ios/202611211645": "1.0.0-RC18", "android/202611211848": "1.0.0-RC19", "ios/202611211849": "1.0.0-RC19",
    "android/202611212024": "1.0.0-RC20", "ios/202611212026": "1.0.0-RC20", "android/202611212030": "1.0.0-RC20",
    "ios/202611212223": "1.0.0-RC21", "android/202611212224": "1.0.0-RC21", "android/202611220813": "1.0.0-RC22",
    "ios/202611220814": "1.0.0-RC22", "ios/202611221337": "1.0.0-RC23", "android/202611221343": "1.0.0-RC23",
    "android/202611221431": "1.0.0-RC24", "ios/202611221433": "1.0.0-RC24", "android/202611221709": "1.0.0-RC25",
    "ios/202611221711": "1.0.0-RC25", "android/202611221959": "1.0.0-RC26", "android/202611222002": "1.0.0-RC26",
    "ios/202611222002": "1.0.0-RC26", "android/202611222209": "1.0.0-RC27", "android/202611222210": "1.0.0-RC27",
    "android/202611230132": "1.0.0-RC28", "android/202611230133": "1.0.0-RC28", "ios/202611230135": "1.0.0-RC28",
    "android/202611230856": "1.0.0-RC29", "ios/202611230858": "1.0.0-RC29", "android/202611231355": "1.0.0-RC30",
    "ios/202611231358": "1.0.0-RC30", "android/202611231450": "1.0.0-RC31", "android/202611231451": "1.0.0-RC31",
    "ios/202611231452": "1.0.0-RC31", "android/202611232120": "1.0.0-RC32", "android/202611232225": "1.0.0-RC33",
    "ios/202611232228": "1.0.0-RC33", "android/202611240752": "1.0.0-RC34", "ios/202611240754": "1.0.0-RC34",
    "android/202611241341": "1.0.0-RC35", "ios/202611241344": "1.0.0-RC35", "android/202611241517": "1.0.0-RC36"
}
BUILD = {"ios": STORE_BUILD, "android": STORE_BUILD}


def classify(app, newest=None, bot_min=None) -> dict:
    """One reported app ("platform/build"): its platform word, state ("fresh", "current", "update" or None:
    unknown) and the details for the tooltip. newest: the baseline, the highest iOS build two users report; a build
    above it is "fresh" (only one user so far)."""
    platform, _, b = (app or "").partition("/")
    if not platform or not b:
        return {"label": "unknown", "state": None, "title": app or "no app reported"}
    if platform == "yogpt":
        top = bot_min.split("/")[-1] if bot_min else None
        state = None if not top else "current" if b >= top else "update"
        return {"label": "Bot", "state": state, "title": f"YoGPT build {b}" + (f"; newest {top}" if top else "")}
    if platform not in BUILD:
        name = {"cli": "CLI"}.get(platform, platform)
        return {"label": name, "state": None, "title": f"{name} build {b}"}
    name, store = ("iOS", "TestFlight") if platform == "ios" else ("Android", "Play")
    if not BUILD[platform].fullmatch(b):
        return {"label": name, "state": None, "title": f"{name} dev build {b}" + (" (dev/push.sh)" if platform == "ios" else "")}
    state = None if not newest else "fresh" if b > newest else "current" if b == newest else "update"
    note = "; fresh: only 1 user so far" if state == "fresh" else f"; current {newest}" if newest and newest != b else ""
    return {"label": name, "state": state, "title": f"{store} build {b}" + note}


def app_versions(out) -> dict:
    """Each release build's version ("ios/202611241344": "1.0.0-RC36"): RELEASES, then what the devices named."""
    found = dict(RELEASES)
    for name in HUBS:
        found.update((out.get(name) or {}).get("app_versions") or {})
    return found


def app_builds(out) -> dict:
    """Every reported app of both hubs ("ios/202611191257") with its label, state and details."""
    seen, users_of = set(), {}
    for name in HUBS:
        h = out.get(name) or {}
        seen.update(a for a, _ in h.get("apps") or [])
        for i, u in enumerate(h.get("users") or []):
            apps = {u.get("app")} | {d.get("app") for d in u.get("device_list") or []}
            seen.update(apps)
            for a in apps:
                p, _, b = (a or "").partition("/")
                # The baseline: store builds; the dev hub's count for iOS (TestFlight builds pointed at it), not for
                # Android, whose builds there are debug builds (older ones still said a store-like build).
                if p in BUILD and BUILD[p].fullmatch(b) and (name == "live" or p == "ios"):
                    users_of.setdefault(p, {}).setdefault(b, set()).add(u.get("id") or f"{name}#{i}")
    seen = {a for a in seen if a and a != "unknown"}
    newest = {}
    for p, builds in users_of.items():
        two = [b for b, ids in builds.items() if len(ids) >= 2]
        newest[p] = max(two or builds)
    bot = next((out[n].get("yogpt_min") for n in HUBS if isinstance(out.get(n), dict) and out[n].get("yogpt_min")), None)
    builds = {a: classify(a, newest.get(a.partition("/")[0]), bot) for a in seen}
    for a, version in app_versions(out).items():
        if a in builds:
            builds[a] = {**builds[a], "version": version, "title": f"{version} · {builds[a]['title']}"}
    return builds


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
    """Of the user devices seen today (bots and the CLI left out): how many run the current build (a fresh one counts as current), have an update
    out, must update (below the hub's minimum) or are unknown (dev builds)."""
    out = {"current": 0, "update": 0, "must": 0, "unknown": 0}
    for app, n in apps or []:
        if (app or "").startswith(("yogpt/", "cli/")):
            continue
        state = "must" if below_min(app, minimum) else (builds.get(app) or {}).get("state") or "unknown"
        state = "current" if state == "fresh" else state
        out[state if state in out else "unknown"] += n
    return out


def finish(out):
    """What needs both hubs: every build's state, and each hub's versions of today."""
    out["app_builds"] = app_builds(out)
    out["app_versions"] = app_versions(out)
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
