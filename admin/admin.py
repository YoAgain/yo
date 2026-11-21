#!/usr/bin/env python3
"""Hub administration with Cloudflare Access verification, scrypt login and signed sessions.
Python standard library only. See README.md for configuration.
"""

import base64
import gzip
import hashlib
import hmac
import json
import os
import re
import shutil
import subprocess
import threading
import time
import urllib.request
import urllib.error
from collections import deque
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs

LISTEN = (os.environ.get("ADMIN_LISTEN", "localhost"), int(os.environ.get("ADMIN_PORT", "8081")))
USER = os.environ["ADMIN_USER"]
PASSWORD_HASH = os.environ["ADMIN_PASSWORD_HASH"]
SESSION_KEY = bytes.fromhex(os.environ["ADMIN_SESSION_KEY"])
TEAM = os.environ["ACCESS_TEAM"]
AUD = os.environ["ACCESS_AUD"]
# Public origin used for POST origin checks.
ORIGIN = os.environ.get("ADMIN_ORIGIN", "https://localhost")
SESSION_SECONDS = 30 * 86400  # a month, renewed on every page load: the admin stays signed in while it is used
COOKIE = "__Host-yo_admin"
HUB_URLS = {"live": os.environ.get("YO_HUB_URL", "http://localhost:8080"),
            "dev": os.environ.get("YO_HUB_DEV_URL", "http://localhost:8090")}
ADMIN_TOKEN = os.environ.get("YO_ADMIN_TOKEN", "")


def b64u(data: str) -> bytes:
    return base64.urlsafe_b64decode(data + "=" * (-len(data) % 4))


# --- Cloudflare Access: the token Cloudflare puts on every request it lets through ---------------------------------

_keys: dict = {}
_keys_at = 0.0
_keys_lock = threading.Lock()
SHA256_PREFIX = bytes.fromhex("3031300d060960864801650304020105000420")  # DigestInfo, RFC 8017 9.2


def access_keys(refresh: bool = False) -> dict:
    global _keys, _keys_at
    with _keys_lock:
        if refresh and time.time() - _keys_at < 60:
            refresh = False  # an unknown key id fetches the keys again, at most once a minute
        if not _keys or refresh or time.time() - _keys_at > 3600:
            with urllib.request.urlopen(f"https://{TEAM}/cdn-cgi/access/certs", timeout=5) as r:
                keys = json.load(r)["keys"]
            _keys = {k["kid"]: (int.from_bytes(b64u(k["n"]), "big"), int.from_bytes(b64u(k["e"]), "big"))
                     for k in keys if k.get("kty") == "RSA"}
            _keys_at = time.time()
        return _keys


def rs256_valid(signed: bytes, signature: bytes, n: int, e: int) -> bool:
    size = (n.bit_length() + 7) // 8
    if len(signature) != size:
        return False
    s = int.from_bytes(signature, "big")
    if s >= n:
        return False
    em = pow(s, e, n).to_bytes(size, "big")
    t = SHA256_PREFIX + hashlib.sha256(signed).digest()
    expected = b"\x00\x01" + b"\xff" * (size - len(t) - 3) + b"\x00" + t
    return hmac.compare_digest(em, expected)


def access_user(token: str | None) -> str | None:
    """The e-mail address of a valid Access token for this application, else None."""
    if not token or token.count(".") != 2:
        return None
    try:
        head, body, sig = token.split(".")
        header = json.loads(b64u(head))
        if header.get("alg") != "RS256":
            return None
        keys = access_keys()
        if header.get("kid") not in keys:
            keys = access_keys(refresh=True)
        key = keys.get(header.get("kid"))
        if not key or not rs256_valid(f"{head}.{body}".encode(), b64u(sig), *key):
            return None
        claims = json.loads(b64u(body))
        now = time.time()
        aud = claims.get("aud")
        if AUD not in (aud if isinstance(aud, list) else [aud]):
            return None
        if claims.get("iss") != f"https://{TEAM}" or claims.get("exp", 0) < now or claims.get("nbf", 0) > now + 60:
            return None
        return claims.get("email") or claims.get("sub")
    except Exception:
        return None


# --- Login and session -----------------------------------------------------------------------------------------------

def password_ok(password: str) -> bool:
    _, n, r, p, salt, want = PASSWORD_HASH.split("$")
    got = hashlib.scrypt(password.encode(), salt=base64.b64decode(salt), n=int(n), r=int(r), p=int(p),
                         maxmem=128 << 20, dklen=len(base64.b64decode(want)))
    return hmac.compare_digest(got, base64.b64decode(want))


def session_mac(who: str, expires: int) -> str:
    return base64.urlsafe_b64encode(hmac.new(SESSION_KEY, f"{who}|{expires}".encode(), "sha256").digest()).decode()


def session_cookie(who: str) -> str:
    expires = int(time.time()) + SESSION_SECONDS
    value = f"{expires}.{session_mac(who, expires)}"
    return f"{COOKIE}={value}; Path=/; Max-Age={SESSION_SECONDS}; Secure; HttpOnly; SameSite=Strict"


def session_valid(cookie_header: str | None, who: str) -> bool:
    for part in (cookie_header or "").split(";"):
        name, _, value = part.strip().partition("=")
        if name == COOKIE and "." in value:
            expires, mac = value.split(".", 1)
            if expires.isdigit() and int(expires) > time.time():
                return hmac.compare_digest(mac, session_mac(who, int(expires)))
    return False


_failures: deque = deque()
_failures_lock = threading.Lock()


def locked_out() -> bool:
    """Five wrong logins within 15 minutes lock the login for the rest of those 15 minutes."""
    with _failures_lock:
        while _failures and _failures[0] < time.time() - 900:
            _failures.popleft()
        return len(_failures) >= 5


def failed() -> None:
    with _failures_lock:
        _failures.append(time.time())


# --- The numbers -----------------------------------------------------------------------------------------------------

_last = {"cpu": None, "net": None, "at": None}
_last_lock = threading.Lock()


USERS_FILE = os.environ.get("ADMIN_USERS_FILE", "./users.json")
REQUEST_FILE = os.environ.get("ADMIN_REQUEST_FILE", "/run/yo-admin-request/want")


class CollectorRequest:
    """Coalesce concurrent page requests; wait for an atomically published snapshot."""
    def __init__(self, request_file, users_file):
        self.request_file = request_file
        self.users_file = users_file
        self.lock = threading.Lock()
        self.last_at = None
        self.request_ns = None

    def request(self, force=False):
        with self.lock:
            now = time.monotonic()
            if force or self.last_at is None or now - self.last_at >= 5:
                with open(self.request_file, "ab"):
                    os.utime(self.request_file, None)
                self.request_ns = os.stat(self.request_file).st_mtime_ns
                self.last_at = now
            return self.request_ns

    def snapshot(self, timeout=3, after=None):
        try:
            requested = self.request(force=after is not None)
        except OSError:
            requested = None
        deadline = time.monotonic() + timeout
        while True:
            data = {}
            fresh = False
            try:
                # Read and stat the same inode even if the collector replaces the path.
                with open(self.users_file) as f:
                    data = json.load(f)
                    fresh = requested is not None and os.fstat(f.fileno()).st_mtime_ns > requested
            except (OSError, ValueError):
                pass
            if after is not None and data.get("at", 0) < after:
                fresh = False
            if fresh or time.monotonic() >= deadline:
                return {**data, "stale": not fresh}
            time.sleep(min(0.05, max(0, deadline - time.monotonic())))


collector = CollectorRequest(REQUEST_FILE, USERS_FILE)


def users_body(snapshot: dict) -> tuple[bytes, str, int]:
    """/users.json: its bytes, its ETag and its time. The ETag leaves out the collector's clock ("at", "updates.at",
    new on every run) so it changes only with the data; the page takes the time from the Yo-At header on a 304."""
    at = snapshot.get("at", 0) if isinstance(snapshot.get("at"), int) else 0
    body = json.dumps(snapshot, separators=(",", ":")).encode()
    upd = snapshot.get("updates")
    same = {**snapshot, "at": 0, "updates": {**upd, "at": 0} if isinstance(upd, dict) else upd}
    tag = '"' + hashlib.sha256(json.dumps(same, separators=(",", ":"), sort_keys=True).encode()).hexdigest()[:32] + '"'
    return body, tag, at


def gzip_ok(accept: str) -> bool:
    for part in accept.split(","):
        name, _, q = part.strip().partition(";")
        if name.strip().lower() == "gzip":
            return q.strip().replace(" ", "") not in ("q=0", "q=0.0", "q=0.00", "q=0.000")
    return False


_gzip = {"key": None, "body": b""}
_gzip_lock = threading.Lock()


def users_gzip(body: bytes) -> bytes:
    """The body gzipped, the last one kept: the 2.8 MB list is compressed once per collector run, not per request."""
    key = hashlib.sha256(body).digest()
    with _gzip_lock:
        if _gzip["key"] != key:
            _gzip.update(key=key, body=gzip.compress(body, 6, mtime=0))
        return _gzip["body"]

# Which feedback is done; To-Dos use the hubs' real feedback ids.
DONE_FILE = os.environ.get("STATE_DIRECTORY", "./state") + "/feedback-done.json"
_done_lock = threading.Lock()


def done_read() -> list[str]:
    try:
        return json.loads(read_bytes(DONE_FILE))
    except (OSError, ValueError):
        return []


def done_set(key: str, done: bool) -> None:
    with _done_lock:
        keys = set(done_read())
        (keys.add if done else keys.discard)(key)
        tmp = DONE_FILE + ".new"
        with open(tmp, "w") as f:
            json.dump(sorted(keys), f)
        os.replace(tmp, DONE_FILE)


# The STATUS bot (skills/status): how often, written here, read by the bot every minute; the numbers of its last
# report, written by the bot (no names). Whom it sends to is fixed on the server (STATUS_TO_USER), never set here.
STATUS_CONFIG = os.environ.get("STATUS_CONFIG", os.environ.get("STATE_DIRECTORY", "./state") + "/status-bot.json")
STATUS_LAST = os.environ.get("STATUS_LAST", "/run/yo-status-live/last.json")
STATUS_EVERY = (0, 15, 30, 60, 120, 1440)
_status_lock = threading.Lock()


def status_read() -> dict:
    def load(path):
        try:
            data = json.loads(read_bytes(path))
            return data if isinstance(data, dict) else None
        except (OSError, ValueError):
            return None
    config = load(STATUS_CONFIG) or {}
    return {"config": {"every_minutes": config.get("every_minutes", 0), "baseline_users": config.get("baseline_users", 0)},
            "last": load(STATUS_LAST), "every": list(STATUS_EVERY)}


def status_save(data) -> dict:
    """{"every_minutes": one of STATUS_EVERY}; the baseline is kept. ValueError when malformed."""
    every = data.get("every_minutes")
    if type(every) is not int or every not in STATUS_EVERY or set(data) != {"every_minutes"}:
        raise ValueError()
    with _status_lock:
        config = {**status_read()["config"], "every_minutes": every}
        tmp = STATUS_CONFIG + ".new"
        with open(tmp, "w") as f:
            json.dump(config, f)
        os.replace(tmp, STATUS_CONFIG)
    return status_read()


# Active users per UTC day for the Users tab: the collector's count of today (users seen since UTC midnight; the hub
# keeps one last-seen day per device), its highest value per day kept here, 60 days. Counts only, nobody's id.
ACTIVE_FILE = os.environ.get("STATE_DIRECTORY", "./state") + "/active-days.json"
_active_lock = threading.Lock()


def active_days(snapshot: dict, now_ms=None) -> dict:
    """The snapshot with "active_days": {day: count} per hub, after folding in its "active_day" counts."""
    now_ms = int(time.time() * 1000) if now_ms is None else now_ms
    today = now_ms // 86400000
    with _active_lock:
        try:
            history = json.loads(read_bytes(ACTIVE_FILE))
            if not isinstance(history, dict):
                history = {}
        except (OSError, ValueError):
            history = {}
        changed = False
        for hub in ("live", "dev"):
            days = {k: v for k, v in (history.get(hub) or {}).items() if k.isdigit() and today - int(k) < 60}
            count = (snapshot.get(hub) or {}).get("active_day") if isinstance(snapshot.get(hub), dict) else None
            if isinstance(count, int) and snapshot.get("at", 0) // 86400000 == today and count > days.get(str(today), -1):
                days[str(today)] = count
                changed = True
            history[hub] = days
        if changed:
            try:
                tmp = ACTIVE_FILE + ".new"
                with open(tmp, "w") as f:
                    json.dump(history, f)
                os.replace(tmp, ACTIVE_FILE)
            except OSError:
                pass
    out = dict(snapshot)
    for hub in ("live", "dev"):
        if isinstance(out.get(hub), dict):
            out[hub] = {**out[hub], "active_days": history.get(hub, {})}
    return out


# The queue (Yos waiting for offline phones, and the bytes of the photos and videos waiting) per hub: the last value
# per UTC hour for 48 hours and per UTC day for 400 days, kept here from the collector's snapshots. Counts only.
QUEUE_FILE = os.environ.get("STATE_DIRECTORY", "./state") + "/queue.json"
_queue_lock = threading.Lock()


def queue_history(snapshot: dict, now_ms=None) -> dict:
    """The snapshot with "queue_hours" and "queue_days": {slot: [yos, bytes]} per hub, after folding in its values."""
    now_ms = int(time.time() * 1000) if now_ms is None else now_ms
    hour, day = now_ms // 3600000, now_ms // 86400000
    with _queue_lock:
        try:
            history = json.loads(read_bytes(QUEUE_FILE))
            if not isinstance(history, dict):
                history = {}
        except (OSError, ValueError):
            history = {}
        changed = False
        for hub in ("live", "dev"):
            h = snapshot.get(hub) if isinstance(snapshot.get(hub), dict) else {}
            old = history.get(hub) if isinstance(history.get(hub), dict) else {}
            hours = {k: v for k, v in (old.get("hours") or {}).items() if k.isdigit() and hour - int(k) < 48}
            days = {k: v for k, v in (old.get("days") or {}).items() if k.isdigit() and day - int(k) < 400}
            yos, blobs = h.get("messages"), h.get("blobs")
            if isinstance(yos, int) and isinstance(blobs, list) and len(blobs) == 2 and snapshot.get("at", 0) // 3600000 == hour:
                value = [yos, blobs[1] if isinstance(blobs[1], int) else 0]
                if hours.get(str(hour)) != value or days.get(str(day)) != value:
                    hours[str(hour)] = days[str(day)] = value
                    changed = True
            history[hub] = {"hours": hours, "days": days}
        if changed:
            try:
                tmp = QUEUE_FILE + ".new"
                with open(tmp, "w") as f:
                    json.dump(history, f)
                os.replace(tmp, QUEUE_FILE)
            except OSError:
                pass
    out = dict(snapshot)
    for hub in ("live", "dev"):
        if isinstance(out.get(hub), dict):
            out[hub] = {**out[hub], "queue_hours": history.get(hub, {}).get("hours", {}), "queue_days": history.get(hub, {}).get("days", {})}
    return out


PROBLEMS_FILE = os.environ.get("STATE_DIRECTORY", "./state") + "/problems-open.json"
_problems_lock = threading.Lock()


def problem_history(snapshot, changed_hub=None):
    t = snapshot.get("at", 0)
    hour = 3600000
    with _problems_lock:
        try:
            history = json.loads(read_bytes(PROBLEMS_FILE))
        except (FileNotFoundError, ValueError):
            history = {}
        changed = False
        out = dict(snapshot)
        for hub in ("live", "dev"):
            data = snapshot.get(hub)
            if not isinstance(data, dict):
                continue
            points = history.get(hub, [])
            counts = data.get("open_report_counts")
            last = points[-1]["t"] if points else 0
            if isinstance(counts, dict) and t > last and (t // hour != last // hour or hub == changed_hub or not points):
                if points and t // hour == last // hour:
                    points.pop()
                points.append({"t": t, "errors_open": counts.get("error", 0), "crashes_open": counts.get("crash", 0)})
                cutoff = t - 30 * 24 * hour
                daily = {p["t"] // (24 * hour): p for p in points if p["t"] < cutoff}
                points = history[hub] = list(daily.values()) + [p for p in points if p["t"] >= cutoff]
                changed = True
            out[hub] = {**data, "open_report_history": points}
        if changed:
            tmp = PROBLEMS_FILE + ".new"
            with open(tmp, "w") as f:
                json.dump(history, f)
            os.replace(tmp, PROBLEMS_FILE)
        return out


def sample_problems_hourly():
    while True:
        try:
            problem_history(collector.snapshot())
        except OSError as exc:
            print(f"Could not record open reports: {exc}", flush=True)
        time.sleep(3600 - time.time() % 3600)


# App versions per UTC day for the Versions row: the collector's "versions_day" (of the devices seen today: current,
# update out, must update, unknown), the day's latest kept here for good (history starts with this file). Counts only.
VERSIONS_FILE = os.environ.get("STATE_DIRECTORY", "./state") + "/version-days.json"
VERSION_KEYS = ("current", "update", "must", "unknown")


def version_days(snapshot: dict, now_ms=None) -> dict:
    """The snapshot with "version_days": {day: [current, update, must, unknown]} per hub, today's from "versions_day",
    and "build_days"/"build_hours": {day or hour: {app: devices}} from "apps_today"."""
    now_ms = int(time.time() * 1000) if now_ms is None else now_ms
    today = str(now_ms // 86400000)
    with _active_lock:
        try:
            history = json.loads(read_bytes(VERSIONS_FILE))
            if not isinstance(history, dict):
                history = {}
        except (OSError, ValueError):
            history = {}
        changed = False
        for hub in ("live", "dev"):
            days = history.get(hub) if isinstance(history.get(hub), dict) else {}
            day = (snapshot.get(hub) or {}).get("versions_day") if isinstance(snapshot.get(hub), dict) else None
            if isinstance(day, dict) and str(snapshot.get("at", 0) // 86400000) == today:
                row = [int(day.get(k) or 0) for k in VERSION_KEYS]
                if days.get(today) != row:
                    days[today] = row
                    changed = True
            history[hub] = days
            # The same snapshot per hour, for the last 48 hours (the "1" range): the green share seen growing within a day.
            hours = history.get(hub + "_hours") if isinstance(history.get(hub + "_hours"), dict) else {}
            if isinstance(day, dict):
                hour = str(now_ms // 3600000)
                row = [int(day.get(k) or 0) for k in VERSION_KEYS]
                if hours.get(hour) != row:
                    hours[hour] = row
                    changed = True
                for h in [h for h in hours if int(h) < now_ms // 3600000 - 48]:
                    del hours[h]
            history[hub + "_hours"] = hours
            # Devices per build ("ios/202611191257": n, of those seen today; the CLI left out), per UTC day for good and
            # per hour for the last 48: the Versions detail's releases over time. Counts only.
            apps = (snapshot.get(hub) or {}).get("apps_today") if isinstance(snapshot.get(hub), dict) else None
            counts = {}
            for pair in apps if isinstance(apps, list) else []:
                if isinstance(pair, list) and len(pair) == 2 and isinstance(pair[0], str) and isinstance(pair[1], int) \
                        and pair[1] > 0 and not pair[0].startswith("cli/"):
                    counts[pair[0]] = counts.get(pair[0], 0) + pair[1]
            bdays = history.get(hub + "_build_days") if isinstance(history.get(hub + "_build_days"), dict) else {}
            bhours = history.get(hub + "_build_hours") if isinstance(history.get(hub + "_build_hours"), dict) else {}
            if counts:
                if str(snapshot.get("at", 0) // 86400000) == today and bdays.get(today) != counts:
                    bdays[today] = counts
                    changed = True
                hour = str(now_ms // 3600000)
                if bhours.get(hour) != counts:
                    bhours[hour] = counts
                    changed = True
            for h in [h for h in bhours if not h.isdigit() or int(h) < now_ms // 3600000 - 48]:
                del bhours[h]
                changed = True
            history[hub + "_build_days"], history[hub + "_build_hours"] = bdays, bhours
        if changed:
            try:
                tmp = VERSIONS_FILE + ".new"
                with open(tmp, "w") as f:
                    json.dump(history, f)
                os.replace(tmp, VERSIONS_FILE)
            except OSError:
                pass
    out = dict(snapshot)
    for hub in ("live", "dev"):
        if isinstance(out.get(hub), dict):
            out[hub] = {**out[hub], "version_days": history.get(hub, {}), "version_hours": history.get(hub + "_hours", {}),
                        "build_days": history.get(hub + "_build_days", {}), "build_hours": history.get(hub + "_build_hours", {})}
    return out


def support_request(hub, suffix="", body=None, method=None):
    if not ADMIN_TOKEN:
        raise ValueError("YO_ADMIN_TOKEN is not configured")
    request = urllib.request.Request(HUB_URLS[hub].rstrip("/") + "/v1/admin/" + suffix,
        data=None if body is None else body.encode("utf-8"), method=method,
        headers={"Yo-Admin-Token": ADMIN_TOKEN, "Content-Type": "text/plain; charset=utf-8"})
    with urllib.request.urlopen(request, timeout=5) as response:
        payload = response.read()
        return json.loads(payload) if payload else {}


def todo_add(hub: str, text: str) -> dict:
    if not ADMIN_TOKEN:
        raise ValueError("YO_ADMIN_TOKEN is not configured")
    request = urllib.request.Request(HUB_URLS[hub].rstrip("/") + "/v1/admin/todos",
                                     data=text.encode("utf-8"), method="POST",
                                     headers={"Yo-Admin-Token": ADMIN_TOKEN, "Content-Type": "text/plain; charset=utf-8"})
    with urllib.request.urlopen(request, timeout=5) as response:
        return json.load(response)


# The pictures YoGPT can send (NNN.jpg) and their README.md table (| NNN.jpg | idea |), read only.
REPLIES = os.environ.get("YOGPT_REPLIES", "./replies")
REPLY_NAME = re.compile(r"\d{1,4}\.jpg")


def replies() -> list[dict]:
    try:
        names = sorted((n for n in os.listdir(REPLIES) if REPLY_NAME.fullmatch(n)), key=lambda n: int(n[:-4]))
    except OSError:
        return []
    ideas = {}
    try:
        for line in read(os.path.join(REPLIES, "README.md")).splitlines():
            cells = [c.strip() for c in line.strip().strip("|").split("|", 1)]
            if len(cells) == 2 and REPLY_NAME.fullmatch(cells[0]):
                ideas[cells[0]] = cells[1].strip().rstrip("|").strip()
    except (OSError, UnicodeDecodeError):
        pass
    out = []
    for n in names:
        try:
            version = int(os.stat(os.path.join(REPLIES, n)).st_mtime)
        except OSError:
            continue
        out.append({"name": n, "number": int(n[:-4]), "idea": ideas.get(n), "v": version})
    return out


def read_bytes(path: str) -> bytes:
    with open(path, "rb") as f:
        return f.read()


def read(path: str) -> str:
    with open(path) as f:
        return f.read()


def cpu_times() -> tuple[int, int]:
    fields = [int(x) for x in read("/proc/stat").split("\n", 1)[0].split()[1:]]
    idle = fields[3] + fields[4]
    return sum(fields[:8]), idle


def net_bytes() -> tuple[int, int]:
    rx = tx = 0
    for line in read("/proc/net/dev").splitlines()[2:]:
        name, data = line.split(":", 1)
        if name.strip() == "lo":
            continue
        v = data.split()
        rx, tx = rx + int(v[0]), tx + int(v[8])
    return rx, tx


def unit(name: str) -> dict:
    props = "ActiveState,SubState,ActiveEnterTimestamp,NRestarts,MemoryCurrent,CPUUsageNSec,TasksCurrent"
    out = subprocess.run(["systemctl", "show", name, "--timestamp=unix", "-p", props],
                         capture_output=True, text=True, timeout=5).stdout
    d = dict(line.split("=", 1) for line in out.splitlines() if "=" in line)
    num = lambda k: int(d[k]) if d.get(k, "").isdigit() else None  # noqa: E731
    return {"state": d.get("ActiveState"), "sub": d.get("SubState"), "since": int(d["ActiveEnterTimestamp"][1:]) if d.get("ActiveEnterTimestamp", "").startswith("@") else None,
            "restarts": num("NRestarts"), "memory": num("MemoryCurrent"), "cpu_ns": num("CPUUsageNSec"),
            "tasks": num("TasksCurrent")}


def reserved_request(hub, method="GET", data=None, name=None):
    if not ADMIN_TOKEN:
        raise ValueError("YO_ADMIN_TOKEN is not configured")
    from urllib.parse import quote
    url = HUB_URLS[hub].rstrip("/") + "/v1/admin/reserved"
    if name is not None:
        url += "/" + quote(name, safe="")
    request = urllib.request.Request(url, method=method,
        data=None if data is None else json.dumps(data).encode(),
        headers={"Yo-Admin-Token": ADMIN_TOKEN, "Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=5) as response:
        return json.load(response) if method != "DELETE" else {}


def diagnostics_request(hub, data):
    request = urllib.request.Request(HUB_URLS[hub].rstrip("/") + "/v1/admin/diagnostics",
        method="PUT", data=json.dumps(data).encode(),
        headers={"Yo-Admin-Token": ADMIN_TOKEN, "Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=5) as response:
        return json.load(response)


# The header's live strip (GET /live?hub=): the hub's stream of Yos (Server-Sent Events, one tiny event per Yo, counts
# only), handed on to the page line by line as it comes, never buffered nor compressed (no-transform keeps Cloudflare
# from gzipping it). Its keep-alive every 15 s keeps the connection (and Cloudflare's) open; a hub silent for longer
# than LIVE_IDLE is taken as gone. A comment never reaches the page's script, so each keep-alive goes on as a "ping"
# event: the page's watchdog sees the stream alive while no one sends a Yo, and opens it again when the pings stop.
LIVE_IDLE = 40
LIVE_HEADERS = {"Content-Type": "text/event-stream", "Cache-Control": "no-cache, no-transform", "X-Accel-Buffering": "no"}
LIVE_PING = b"event: ping\ndata: 1\n"
# The page's EventSource tries again this soon after a cut (ms), not after its default of a few seconds.
LIVE_HELLO = b"retry: 2000\n" + LIVE_PING + b"\n"


def live_stream(hub: str):
    if not ADMIN_TOKEN:
        raise ValueError("YO_ADMIN_TOKEN is not configured")
    request = urllib.request.Request(HUB_URLS[hub].rstrip("/") + "/v1/admin/live",
                                     headers={"Yo-Admin-Token": ADMIN_TOKEN, "Accept": "text/event-stream"})
    return urllib.request.urlopen(request, timeout=LIVE_IDLE)


def live_relay(source, write) -> None:
    """Every line from the hub to the page at once, until either side goes."""
    for line in iter(source.readline, b""):
        write(LIVE_PING if line.startswith(b":") else line)


def reserved_names() -> dict:
    return {hub: section("reserved " + hub, lambda hub=hub: reserved_request(hub)) for hub in ("live", "dev")}


def section(name: str, fn):
    """One part of stats.json: a failing part is logged and comes out as None, the rest still answers."""
    try:
        return fn()
    except Exception as e:  # noqa: BLE001 -- one broken part must never break the whole page
        print(f"stats: {name}: {type(e).__name__}: {e}", flush=True)
        return None


def cpu_net() -> tuple[dict, dict]:
    now = time.monotonic()
    total, idle = cpu_times()
    rx, tx = net_bytes()
    with _last_lock:
        cpu = rx_s = tx_s = None
        if _last["cpu"] and total > _last["cpu"][0]:
            dt, di = total - _last["cpu"][0], idle - _last["cpu"][1]
            cpu = round(100 * (dt - di) / dt, 1)
        if _last["net"] and now > _last["at"]:
            rx_s = (rx - _last["net"][0]) / (now - _last["at"])
            tx_s = (tx - _last["net"][1]) / (now - _last["at"])
        _last.update(cpu=(total, idle), net=(rx, tx), at=now)
    load = read("/proc/loadavg").split()[:3]
    return ({"percent": cpu, "cores": os.cpu_count(), "load": [float(x) for x in load]},
            {"rx_per_s": rx_s, "tx_per_s": tx_s, "rx": rx, "tx": tx})


def memory() -> dict:
    mem = {k: int(v.split()[0]) * 1024 for k, v in
           (line.split(":", 1) for line in read("/proc/meminfo").splitlines())}
    return {"total": mem["MemTotal"], "available": mem["MemAvailable"],
            "swap_total": mem.get("SwapTotal", 0), "swap_free": mem.get("SwapFree", 0)}


def disk() -> dict:
    d = shutil.disk_usage("/")
    return {"total": d.total, "used": d.used, "free": d.free}


def reboot() -> dict:
    packages = read("/run/reboot-required.pkgs").split() if os.path.exists("/run/reboot-required.pkgs") else []
    return {"needed": os.path.exists("/run/reboot-required"), "packages": sorted(set(packages))}


def hub_unit() -> dict:
    try:
        version = read(os.environ.get("YO_HUB_VERSION_FILE", "./version")).strip()
    except OSError:
        version = None
    return {**unit(os.environ.get("YO_HUB_UNIT", "yo-hub.service")), "version": version}


# The server's figures once a minute, the last hour: the Server rows' bars. All three series are kept in a small file
# (HISTORY_FILE) so a restart does not empty them.
# Each [ms, cpu %, memory %, disk %, rx B/s, tx B/s].
HISTORY = []
# And the last 24 hours, a point per quarter of an hour (the average of its minutes): the Server rows' hover card.
# Each [ms of the quarter's start, cpu %, memory %, disk %, rx B/s, tx B/s, minutes in it].
HISTORY_DAY = []
QUARTER_MS = 900_000
# And the last 30 UTC days, a point a day (the average of its minutes): [day, cpu %, memory %, disk %, rx B/s, tx B/s, minutes].
HISTORY_DAYS = []
HISTORY_FILE = os.environ.get("STATE_DIRECTORY", "./state") + "/server-history.json"
_history_lock = threading.Lock()
_history_loaded = False


def load_history() -> None:
    """Once: what the file kept (a point older than its series' span is dropped)."""
    global _history_loaded
    _history_loaded = True
    try:
        saved = json.loads(read_bytes(HISTORY_FILE))
        now = time.time() * 1000
        HISTORY[:] = [p for p in saved.get("minutes", []) if now - p[0] < 3_600_000][-60:]
        HISTORY_DAY[:] = [p for p in saved.get("quarters", []) if now - p[0] < 86_400_000][-96:]
        HISTORY_DAYS[:] = [p for p in saved.get("days", []) if now / 86_400_000 - p[0] < 30][-30:]
    except (OSError, ValueError, AttributeError, TypeError, IndexError):
        pass


def save_history() -> None:
    try:
        tmp = HISTORY_FILE + ".new"
        with open(tmp, "w") as f:
            json.dump({"minutes": HISTORY, "quarters": HISTORY_DAY, "days": HISTORY_DAYS}, f)
        os.replace(tmp, HISTORY_FILE)
    except OSError:
        pass


def remember_daily(point: list) -> None:
    day = point[0] // 86_400_000
    if HISTORY_DAYS and HISTORY_DAYS[-1][0] == day:
        q = HISTORY_DAYS[-1]
        n = q[6] + 1
        for i in range(1, 6):
            v = point[i]
            if v is not None:
                q[i] = v if q[i] is None else round(q[i] + (v - q[i]) / n, 1)
        q[6] = n
    else:
        HISTORY_DAYS.append([day, *point[1:6], 1])
        del HISTORY_DAYS[:-30]


def remember_day(point: list) -> None:
    start = point[0] // QUARTER_MS * QUARTER_MS
    if HISTORY_DAY and HISTORY_DAY[-1][0] == start:
        q = HISTORY_DAY[-1]
        n = q[6] + 1
        for i in range(1, 6):
            v = point[i]
            if v is not None:
                q[i] = v if q[i] is None else round(q[i] + (v - q[i]) / n, 1)
        q[6] = n
    else:
        HISTORY_DAY.append([start, *point[1:6], 1])
        del HISTORY_DAY[:-96]


def remember(out: dict, now: float) -> list:
    def pct(part, whole):
        return round(100 * part / whole, 1) if part is not None and whole else None
    m, d, c, n = out.get("memory") or {}, out.get("disk") or {}, out.get("cpu") or {}, out.get("network") or {}
    with _history_lock:
        if not _history_loaded:
            load_history()
        if not HISTORY or now * 1000 - HISTORY[-1][0] >= 60_000:
            HISTORY.append([int(now * 1000), c.get("percent"), pct(m.get("total", 0) - m.get("available", 0) if m else None, m.get("total")),
                            pct(d.get("used"), d.get("total")), n.get("rx_per_s"), n.get("tx_per_s")])
            del HISTORY[:-60]
            remember_day(HISTORY[-1])
            remember_daily(HISTORY[-1])
            save_history()
        return list(HISTORY)


def stats() -> dict:
    out = stats_now()
    out["history"] = remember(out, time.time())
    with _history_lock:
        out["history_day"] = [q[:6] for q in HISTORY_DAY]
        out["history_days"] = [q[:6] for q in HISTORY_DAYS]
    return out


def stats_now() -> dict:
    cpu, net = section("cpu/network", cpu_net) or (None, None)
    uname = os.uname()
    return {
        "host": uname.nodename,
        "kernel": uname.release,
        "uptime": section("uptime", lambda: float(read("/proc/uptime").split()[0])),
        "cpu": cpu,
        "memory": section("memory", memory),
        "disk": section("disk", disk),
        "network": net,
        "reboot": section("reboot", reboot),
        "hub": section("hub", hub_unit),
        "hub_dev": section("hub_dev", lambda: unit(os.environ.get("YO_HUB_DEV_UNIT", "yo-hub-dev.service"))),
        "tunnel": section("tunnel", lambda: unit(os.environ.get("ADMIN_TUNNEL_UNIT", "cloudflared.service"))),
        "yogpt": section("yogpt", lambda: unit(os.environ.get("YOGPT_UNIT", "yo-yogpt.service"))),
        "reserved": section("reserved", reserved_names),
    }


# --- HTTP ------------------------------------------------------------------------------------------------------------

HEADERS = {
    "Content-Security-Policy": "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; "
                               "img-src 'self' data:; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
    "X-Content-Type-Options": "nosniff",
    "Referrer-Policy": "same-origin",  # no-referrer would make the form's Origin "null"
    "Cache-Control": "no-store",
    "Strict-Transport-Security": "max-age=31536000",
}
HERE = os.path.dirname(os.path.abspath(__file__))
FILES = {"/app.css": "text/css; charset=utf-8", "/app.js": "text/javascript; charset=utf-8",
         "/icon.svg": "image/svg+xml"}


def page(name: str) -> bytes:
    with open(os.path.join(HERE, name), "rb") as f:
        return f.read()


class Admin(BaseHTTPRequestHandler):
    server_version = "yo-admin"
    sys_version = ""

    def log_message(self, fmt, *args):  # one line per request, without the client's address (it is the tunnel)
        print(f"{self.command} {self.path.split('?')[0]} {args[1] if len(args) > 1 else ''}", flush=True)

    def send(self, status: int, body: bytes = b"", ctype: str = "text/html; charset=utf-8", extra=None):
        self.send_response(status)
        for k, v in {**HEADERS, **(extra or {})}.items():
            self.send_header(k, v)
        self.send_header("Content-Type", ctype)
        if status != 304:
            self.send_header("Content-Length", str(len(body)))
        try:
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            # The page went before its answer (reloaded, closed, a poll replaced): nothing to tell, no traceback.
            self.close_connection = True

    def who(self) -> str | None:
        return access_user(self.headers.get("Cf-Access-Jwt-Assertion"))

    def same_origin(self) -> bool:
        origin = self.headers.get("Origin")
        return origin is None or origin == ORIGIN

    def do_GET(self):
        who = self.who()
        if not who:
            return self.send(403, b"Forbidden", "text/plain")
        path = self.path.split("?")[0]
        if path in FILES:
            return self.send(200, page(path[1:]), FILES[path])
        logged_in = session_valid(self.headers.get("Cookie"), who)
        if path == "/":
            if logged_in:
                problem_history(collector.snapshot())
            # Signed in: the session starts again from now (sliding), so an admin in use never logs itself out.
            return self.send(200, page("dashboard.html" if logged_in else "login.html"), extra={"Set-Cookie": session_cookie(who)} if logged_in else None)
        if path == "/stats.json":
            if not logged_in:
                return self.send(401, b"{}", "application/json")
            return self.send(200, json.dumps(stats()).encode(), "application/json")
        if path == "/support.json":
            if not logged_in:
                return self.send(401, b"{}", "application/json")
            query = parse_qs(self.path.partition("?")[2])
            # A thread is a random install id (16 bytes, 32 hex; hub schema 9), never an account.
            hub, install = query.get("hub", [""])[0], query.get("install", [""])[0]
            if hub not in ("live", "dev") or (install and not re.fullmatch(r"[0-9a-fA-F]{32}", install)):
                return self.send(400, b"Bad request", "text/plain")
            try:
                result = support_request(hub, "messages" + ("/" + install if install else ""))
            except (OSError, ValueError):
                return self.send(502, b"Hub unavailable", "text/plain")
            return self.send(200, json.dumps(result).encode(), "application/json")
        if path == "/live":
            if not logged_in:
                return self.send(401, b"", "text/plain")
            hub = parse_qs(self.path.partition("?")[2]).get("hub", [""])[0]
            if hub not in ("live", "dev"):
                return self.send(400, b"Bad request", "text/plain")
            if self.command == "HEAD":
                return self.send(200, b"", LIVE_HEADERS["Content-Type"])
            try:
                source = live_stream(hub)
            except (OSError, ValueError):
                return self.send(502, b"Hub unavailable", "text/plain")
            with source:
                self.close_connection = True
                self.send_response(200)
                for k, v in {**HEADERS, **LIVE_HEADERS}.items():
                    self.send_header(k, v)
                self.end_headers()

                def write(line: bytes):
                    self.wfile.write(line)
                    self.wfile.flush()
                try:
                    write(LIVE_HELLO)
                    live_relay(source, write)
                except OSError:  # the page went (closed, reloaded) or the hub did: the page reconnects on its own
                    pass
            return None
        if path == "/done.json":
            if not logged_in:
                return self.send(401, b"[]", "application/json")
            return self.send(200, json.dumps(done_read()).encode(), "application/json")
        if path == "/yogpt.json":
            if not logged_in:
                return self.send(401, b"[]", "application/json")
            return self.send(200, json.dumps(replies()).encode(), "application/json")
        if path.startswith("/yogpt/"):  # one of YoGPT's pictures, only by a plain NNN.jpg name
            name = path[len("/yogpt/"):]
            if not logged_in:
                return self.send(401, b"", "text/plain")
            if not REPLY_NAME.fullmatch(name):
                return self.send(404, b"Not found", "text/plain")
            try:
                body = read_bytes(os.path.join(REPLIES, name))
            except OSError:
                return self.send(404, b"Not found", "text/plain")
            return self.send(200, body, "image/jpeg", {"Cache-Control": "private, max-age=3600"})
        if path == "/status.json":
            if not logged_in:
                return self.send(401, b"{}", "application/json")
            return self.send(200, json.dumps(status_read()).encode(), "application/json")
        if path == "/users.json":
            if not logged_in:
                return self.send(401, b"{}", "application/json")
            body, tag, at = users_body(queue_history(version_days(active_days(problem_history(collector.snapshot())))))
            # Revalidated every 10 s by the page: 304 while nothing changed (its If-None-Match, weak or strong).
            extra = {"Cache-Control": "no-cache, private", "ETag": tag, "Vary": "Accept-Encoding", "Yo-At": str(at)}
            if tag in [t.strip().removeprefix("W/") for t in self.headers.get("If-None-Match", "").split(",")]:
                return self.send(304, b"", "application/json", extra)
            if gzip_ok(self.headers.get("Accept-Encoding", "")):
                return self.send(200, users_gzip(body), "application/json", {**extra, "Content-Encoding": "gzip"})
            return self.send(200, body, "application/json", extra)
        self.send(404, b"Not found", "text/plain")

    do_HEAD = do_GET

    def do_POST(self):
        who = self.who()
        if not who or not self.same_origin():
            print(f"refused: access {'ok' if who else 'missing' if not self.headers.get('Cf-Access-Jwt-Assertion') else 'invalid'}"
                  f", origin {self.headers.get('Origin')!r}", flush=True)
            return self.send(403, b"Forbidden", "text/plain")
        path = self.path.split("?")[0]
        if path == "/logout":
            cookie = f"{COOKIE}=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Strict"
            return self.send(303, extra={"Location": "/", "Set-Cookie": cookie})
        if path == "/reserved":
            if not session_valid(self.headers.get("Cookie"), who):
                return self.send(401, b"", "text/plain")
            length = int(self.headers.get("Content-Length") or 0)
            if length > 65536:
                return self.send(413, b"Too large", "text/plain")
            try:
                data = json.loads(self.rfile.read(length))
                hub, action = data["hub"], data["action"]
                if hub not in ("live", "dev") or action not in ("add", "remove", "switch"):
                    raise ValueError()
                result = reserved_request(hub, {"add": "POST", "remove": "DELETE", "switch": "PUT"}[action],
                    data.get("names") if action == "add" else {"enabled": data["enabled"]} if action == "switch" else None,
                    data["name"] if action == "remove" else None)
            except (ValueError, KeyError, TypeError):
                return self.send(400, b"Bad request", "text/plain")
            except OSError:
                return self.send(502, b"Hub unavailable or refused change", "text/plain")
            return self.send(200, json.dumps(result).encode(), "application/json")
        if path in ("/support", "/todos/done", "/todos/delete", "/support/done"):
            if not session_valid(self.headers.get("Cookie"), who):
                return self.send(401, b"", "text/plain")
            try:
                length = int(self.headers.get("Content-Length") or 0)
                if not 0 < length <= 12000:
                    return self.send(413, b"Too large", "text/plain")
                data = json.loads(self.rfile.read(length))
                hub = data["hub"]
                if hub not in ("live", "dev"):
                    raise ValueError()
                method = None
                if path == "/todos/delete":
                    if type(data["id"]) is not int:
                        raise ValueError()
                    suffix, body, method = "todos/" + str(data["id"]), None, "DELETE"
                elif path in ("/todos/done", "/support/done"):
                    if type(data["id"]) is not int or type(data["done"]) is not bool:
                        raise ValueError()
                    suffix, body = ("messages/done/" if path == "/support/done" else "todos/") + str(data["id"]), "1" if data["done"] else "0"
                else:
                    install, action = data["install"], data["action"]
                    if not isinstance(install, str) or not re.fullmatch(r"[0-9a-fA-F]{32}", install):
                        raise ValueError()
                    suffix = "messages/" + install
                    if action == "reply":
                        body = data["text"]
                        if not isinstance(body, str) or not body.strip() or len(body.encode()) > 2000:
                            raise ValueError()
                    elif action == "handled":
                        if type(data["through"]) is not int or data["through"] < 0:
                            raise ValueError()
                        suffix += "/handled"
                        body = str(data["through"])
                    else:
                        raise ValueError()
                result = support_request(hub, suffix, body, method) if method else support_request(hub, suffix, body)
            except (ValueError, KeyError, TypeError):
                return self.send(400, b"Bad request", "text/plain")
            except OSError:
                return self.send(502, b"Hub unavailable or refused change", "text/plain")
            return self.send(200, json.dumps(result).encode(), "application/json")
        if path == "/todos":
            if not session_valid(self.headers.get("Cookie"), who):
                return self.send(401, b"", "text/plain")
            length = int(self.headers.get("Content-Length") or 0)
            if length > 65536:
                return self.send(413, b"Too large", "text/plain")
            form = parse_qs(self.rfile.read(length).decode(errors="replace"))
            hub, text = form.get("hub", [""])[0], form.get("text", [""])[0].strip()
            if hub not in ("live", "dev") or not text or len(text.encode("utf-8")) > 4000:
                return self.send(400, b"Bad request", "text/plain")
            try:
                todo = todo_add(hub, text)
            except urllib.error.HTTPError as error:
                return self.send(error.code if error.code == 429 else 502, b"Hub refused To-Do", "text/plain")
            except (OSError, ValueError):
                return self.send(502, b"Hub unavailable or not configured", "text/plain")
            return self.send(201, json.dumps(todo).encode(), "application/json")
        if path == "/status":
            if not session_valid(self.headers.get("Cookie"), who):
                return self.send(401, b"", "text/plain")
            length = int(self.headers.get("Content-Length") or 0)
            if not 0 < length <= 512:
                return self.send(413, b"", "text/plain")
            try:
                result = status_save(json.loads(self.rfile.read(length)))
            except (ValueError, KeyError, TypeError, AttributeError):
                return self.send(400, b"Bad request", "text/plain")
            except OSError:
                return self.send(500, b"Could not save", "text/plain")
            return self.send(200, json.dumps(result).encode(), "application/json")
        if path == "/diagnostics/done":
            if not session_valid(self.headers.get("Cookie"), who):
                return self.send(401, b"", "text/plain")
            length = int(self.headers.get("Content-Length") or 0)
            if length > 256:
                return self.send(413, b"", "text/plain")
            try:
                data = json.loads(self.rfile.read(length))
                if data["hub"] not in ("live", "dev") or type(data["id"]) is not int or data["id"] <= 0 or type(data["done"]) is not bool:
                    raise ValueError()
                diagnostics_request(data["hub"], {"id": data["id"], "done": data["done"]})
                # The open counts right after the change, for the Problems history; the change itself is done either way.
                snapshot = collector.snapshot(after=int(time.time() * 1000))
                if not snapshot["stale"] and snapshot.get(data["hub"], {}).get("open_report_counts") is not None:
                    problem_history(snapshot, data["hub"])
            except (ValueError, KeyError, TypeError):
                return self.send(400, b"", "text/plain")
            except OSError:
                return self.send(502, b"", "text/plain")
            return self.send(204)
        if path == "/feedback/done":  # a feedback (id=12) or report (id=r<reporter+reported, hex>) marked done or not: hub=live&id=12&done=1
            if not session_valid(self.headers.get("Cookie"), who):
                return self.send(401, b"", "text/plain")
            length = int(self.headers.get("Content-Length") or 0)
            if length > 256:
                return self.send(413, b"Too large", "text/plain")
            form = parse_qs(self.rfile.read(length).decode(errors="replace"))
            hub, fid = form.get("hub", [""])[0], form.get("id", [""])[0]
            if hub not in ("live", "dev") or not (fid.isdigit() or (fid[:1] == "r" and 0 < len(fid) <= 33 and all(c in "0123456789abcdef" for c in fid[1:]))):
                return self.send(400, b"Bad request", "text/plain")
            done_set(f"{hub}:{fid}", form.get("done", ["0"])[0] == "1")
            return self.send(204)
        if path != "/login":
            return self.send(404, b"Not found", "text/plain")
        length = int(self.headers.get("Content-Length") or 0)
        if length > 4096:
            return self.send(413, b"Too large", "text/plain")
        form = parse_qs(self.rfile.read(length).decode(errors="replace"))
        if locked_out():
            return self.send(303, extra={"Location": "/?locked"})
        user, password = form.get("user", [""])[0], form.get("password", [""])[0]
        if hmac.compare_digest(user.encode(), USER.encode()) & password_ok(password):
            return self.send(303, extra={"Location": "/", "Set-Cookie": session_cookie(who)})
        failed()
        time.sleep(1)
        self.send(303, extra={"Location": "/?wrong"})


if __name__ == "__main__":
    threading.Thread(target=sample_problems_hourly, daemon=True).start()
    ThreadingHTTPServer.daemon_threads = True
    print(f"yo-admin: listening on {LISTEN[0]}:{LISTEN[1]}", flush=True)
    section("cpu/network", cpu_net)  # a first sample, so the first page already gets CPU and network rates
    ThreadingHTTPServer(LISTEN, Admin).serve_forever()
