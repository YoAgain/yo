#!/usr/bin/env python3
"""IFTTT: Yo's service on IFTTT, as a bot account and the IFTTT Service API it serves
(https://ifttt.com/docs/api_reference), nothing else between them.

- Trigger `you_yo_ifttt`: a Yo (a photo, a video) someone sends IFTTT, once linked. Kept: when, and which kind; never
  the media (`listen --drain-media`), never a Yo from someone not linked.
- Action `yo_me`: IFTTT Yos the linked user, with a photo from a URL if one is given.

Linking (OAuth 2): IFTTT sends the user to `/authorize`; the page asks the Yo app to say yes (a `yo://connect` link on
a phone, a QR code of `https://ORIGIN/PREFIX/approve` on a computer); the app signs the page's ticket with the account
key (`yo_wire::connect`) and gives it to `/approve`; `yo verify` checks it against the key the hub attests, the bot
adds the user, the page goes back to IFTTT with a code, and `/token` turns the code into a token (kept hashed).

Logs counts only, never who. Every request is hostile: bodies are capped, tokens and the service key compared in
constant time.

  YO_STATE=… YO_HUB=… IFTTT_SERVICE_KEY=… IFTTT_CLIENT_ID=… IFTTT_CLIENT_SECRET=… ifttt.py --db PATH
  (once before: yo register IFTTT)
"""

import argparse
import hashlib
import hmac
import html
import ipaddress
import json
import os
import queue
import re
import secrets
import socket
import sqlite3
import sys
import tempfile
import threading
import time
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
sys.path.insert(0, HERE)
import qr  # noqa: E402
import yocli  # noqa: E402

TRIGGER, ACTION = "you_yo_ifttt", "yo_me"
KINDS = ("yo", "photo", "video")
KEEP = 50  # events per user: what a trigger poll returns at most
TICKET_TTL, CODE_TTL = 600, 300  # seconds
MAX_PENDING = 2000  # link requests waiting at once: more are refused, not stored
REQUEST_TIMEOUT = 20  # seconds a client may take to send its request
MAX_BODY = 64 * 1024
MAX_PHOTO = 512 * 1024  # as `yo send --photo` reads
PHOTO_DEADLINE = 15  # seconds for the whole download
SEND_TIMEOUT = 30  # seconds for a Yo: IFTTT waits about that long, and a retry would Yo twice
REALTIME = "https://realtime.ifttt.com/v1/notifications"
USER_ID = re.compile(r"[0-9a-f]{32}")
SIGNATURE = re.compile(r"[A-Za-z0-9_-]{86}")  # 64 bytes, base64url without padding
APP_STORE = "https://apps.apple.com/app/yo/id834335592"


def now():
    return time.time()


def digest(token):
    return hashlib.sha256(token.encode()).hexdigest()


def same(a, b):
    return bool(a) and bool(b) and hmac.compare_digest(a.encode(), b.encode())


class Store:
    """The service's state: link requests, codes, tokens (hashed), trigger identities, and the last events per user."""

    def __init__(self, path):
        self.db = sqlite3.connect(path, check_same_thread=False, isolation_level=None)
        self.lock = threading.Lock()
        with self.lock:
            self.db.executescript(
                """
                PRAGMA journal_mode = WAL;
                CREATE TABLE IF NOT EXISTS requests (ticket TEXT PRIMARY KEY, poll TEXT NOT NULL, state TEXT NOT NULL,
                    redirect TEXT NOT NULL, created REAL NOT NULL, user TEXT, username TEXT, denied INTEGER NOT NULL
                    DEFAULT 0, done INTEGER NOT NULL DEFAULT 0);
                CREATE TABLE IF NOT EXISTS codes (code TEXT PRIMARY KEY, user TEXT NOT NULL, username TEXT,
                    redirect TEXT NOT NULL, created REAL NOT NULL);
                CREATE TABLE IF NOT EXISTS tokens (hash TEXT PRIMARY KEY, user TEXT NOT NULL, username TEXT,
                    created REAL NOT NULL);
                CREATE INDEX IF NOT EXISTS tokens_user ON tokens (user);
                CREATE TABLE IF NOT EXISTS identities (identity TEXT PRIMARY KEY, user TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS identities_user ON identities (user);
                CREATE TABLE IF NOT EXISTS events (id TEXT PRIMARY KEY, user TEXT NOT NULL, at_ms INTEGER NOT NULL,
                    kind TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS events_user ON events (user, at_ms);
                """
            )

    def run(self, sql, *args):
        with self.lock:
            return self.db.execute(sql, args).fetchall()

    def sweep(self):
        self.run("DELETE FROM requests WHERE created < ?", now() - TICKET_TTL)
        self.run("DELETE FROM codes WHERE created < ?", now() - CODE_TTL)

    # ---- linking ----

    def new_request(self, state, redirect):
        """A ticket and the page's poll key; None while too many wait."""
        self.sweep()
        if self.run("SELECT COUNT(*) FROM requests")[0][0] >= MAX_PENDING:
            return None
        ticket, poll = secrets.token_urlsafe(24), secrets.token_urlsafe(24)
        self.run("INSERT INTO requests (ticket, poll, state, redirect, created) VALUES (?, ?, ?, ?, ?)",
                 ticket, poll, state, redirect, now())
        return ticket, poll

    def request(self, ticket):
        self.sweep()
        rows = self.run("SELECT user, denied, done FROM requests WHERE ticket = ?", ticket)
        return rows[0] if rows else None

    def approve(self, ticket, user, username):
        self.run("UPDATE requests SET user = ?, username = ? WHERE ticket = ? AND user IS NULL AND denied = 0",
                 user, username, ticket)

    def deny(self, ticket):
        self.run("UPDATE requests SET denied = 1 WHERE ticket = ? AND user IS NULL", ticket)

    def burn(self, ticket):
        """A ticket whose signature failed: no second try; the page says it expired."""
        self.run("UPDATE requests SET done = 1 WHERE ticket = ?", ticket)

    def poll(self, poll):
        """Where a waiting page stands: `waiting`, `expired`, or a redirect back to IFTTT (a code once approved, an
        `access_denied` once refused), given once."""
        self.sweep()
        rows = self.run("SELECT ticket, state, redirect, user, username, denied, done FROM requests WHERE poll = ?",
                        poll)
        if not rows:
            return "expired", None
        ticket, state, redirect, user, username, denied, done = rows[0]
        if done or not (user or denied):
            return ("expired", None) if done else ("waiting", None)
        with self.lock:  # given once, also to two polls at once
            if not self.db.execute("UPDATE requests SET done = 1 WHERE ticket = ? AND done = 0", (ticket,)).rowcount:
                return "expired", None
        if denied:
            return "denied", with_query(redirect, error="access_denied", state=state)
        code = secrets.token_urlsafe(32)
        self.run("INSERT INTO codes VALUES (?, ?, ?, ?, ?)", code, user, username, redirect, now())
        return "approved", with_query(redirect, code=code, state=state)

    def redeem(self, code, redirect):
        """A token for a code, once; None for a code unknown, used, expired or for another redirect."""
        self.sweep()
        with self.lock:
            rows = self.db.execute("SELECT user, username, redirect FROM codes WHERE code = ?", (code,)).fetchall()
            self.db.execute("DELETE FROM codes WHERE code = ?", (code,))
        if not rows or (redirect and rows[0][2] != redirect):
            return None
        return self.issue(rows[0][0], rows[0][1])

    def issue(self, user, username):
        token = secrets.token_urlsafe(32)
        self.run("INSERT INTO tokens VALUES (?, ?, ?, ?)", digest(token), user, username, now())
        return token

    def holder(self, token):
        rows = self.run("SELECT user, username FROM tokens WHERE hash = ?", digest(token)) if token else []
        return rows[0] if rows else None

    def linked(self, user):
        return bool(self.run("SELECT 1 FROM tokens WHERE user = ? LIMIT 1", user))

    # ---- triggers ----

    def watch(self, identity, user):
        self.run("INSERT OR REPLACE INTO identities VALUES (?, ?)", identity, user)

    def unwatch(self, identity):
        self.run("DELETE FROM identities WHERE identity = ?", identity)

    def identities(self, user):
        return [r[0] for r in self.run("SELECT identity FROM identities WHERE user = ?", user)]

    def record(self, user, kind, at_ms=None):
        at_ms = int(now() * 1000) if at_ms is None else at_ms
        with self.lock:
            self.db.execute("INSERT INTO events VALUES (?, ?, ?, ?)", (secrets.token_hex(12), user, at_ms, kind))
            self.db.execute(
                "DELETE FROM events WHERE user = ? AND id NOT IN "
                "(SELECT id FROM events WHERE user = ? ORDER BY at_ms DESC, rowid DESC LIMIT ?)",
                (user, user, KEEP),
            )

    def events(self, user, limit):
        return self.run("SELECT id, at_ms, kind FROM events WHERE user = ? ORDER BY at_ms DESC, rowid DESC LIMIT ?",
                        user, max(0, min(limit, KEEP)))


def pairing_code(ticket):
    """Four digits both the page and the app show, from the ticket: the user checks the app asks for this page."""
    return f"{int.from_bytes(hashlib.sha256(b'yo pairing ' + ticket.encode()).digest()[:4], 'big') % 10000:04}"


def with_query(url, **params):
    parts = urllib.parse.urlsplit(url)
    query = urllib.parse.parse_qsl(parts.query) + [(k, v) for k, v in params.items() if v is not None]
    return urllib.parse.urlunsplit(parts._replace(query=urllib.parse.urlencode(query)))


def iso(at_ms):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(at_ms / 1000))


class Config:
    def __init__(self, env):
        self.service_key = env.get("IFTTT_SERVICE_KEY", "")
        self.client_id = env.get("IFTTT_CLIENT_ID", "")
        self.client_secret = env.get("IFTTT_CLIENT_SECRET", "")
        slug = env.get("IFTTT_SERVICE_SLUG", "yo")
        self.redirect = env.get("IFTTT_REDIRECT_URI", f"https://ifttt.com/channels/{slug}/authorize")
        self.origin = env.get("CONNECT_ORIGIN", "https://justyo.co").rstrip("/")
        self.prefix = "/" + env.get("CONNECT_PREFIX", "/connect").strip("/")
        self.test_user = env.get("IFTTT_TEST_USER", "")  # the Yo account IFTTT's endpoint tests act as
        self.realtime = env.get("IFTTT_REALTIME_URL", REALTIME)
        if not (self.service_key and self.client_id and self.client_secret):
            raise SystemExit("IFTTT_SERVICE_KEY, IFTTT_CLIENT_ID and IFTTT_CLIENT_SECRET are needed")


class Yo:
    """The bot's `yo` commands; a seam for tests."""

    def verify(self, user, origin, ticket, signature):
        out = yocli.yo("verify", user, origin, ticket, signature)
        return out.get("ok") is True and out.get("user") == user, out.get("username")

    def add(self, user):
        yocli.yo("add", user)

    def send(self, user, photo=None):
        if photo is None:
            return yocli.yo("send", user, timeout=SEND_TIMEOUT)
        with tempfile.NamedTemporaryFile(suffix=".jpg") as f:
            f.write(photo)
            f.flush()
            os.chmod(f.name, 0o600)
            return yocli.yo("send", user, "--photo", f.name, timeout=SEND_TIMEOUT)


class Realtime:
    """Tells IFTTT a user's trigger has something new (one worker; at most 1000 waiting, more dropped)."""

    def __init__(self, config, store):
        self.config, self.store, self.jobs = config, store, queue.Queue(1000)
        self.failed = yocli.Tally("realtime notifications failed")

    def start(self):
        threading.Thread(target=self.work, daemon=True).start()
        return self

    def notify(self, user):
        try:
            self.jobs.put_nowait(user)
        except queue.Full:
            self.failed.add()

    def work(self):
        while True:
            try:
                self.notify_now(self.jobs.get())
            except Exception:  # noqa: BLE001 - a lost notification only delays: IFTTT polls anyway
                self.failed.add()

    def notify_now(self, user):
        identities = self.store.identities(user)
        data = [{"trigger_identity": i} for i in identities] or [{"user_id": user}]
        request = urllib.request.Request(
            self.config.realtime,
            data=json.dumps({"data": data}).encode(),
            headers={
                "IFTTT-Service-Key": self.config.service_key,
                "Accept": "application/json",
                "Content-Type": "application/json",
                "X-Request-ID": secrets.token_hex(16),
            },
            method="POST",
        )
        urllib.request.urlopen(request, timeout=10).close()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None  # a redirect is an error: it could lead anywhere


PHOTO_OPENER = urllib.request.build_opener(NoRedirect)


def public_host(host):
    """Whether every address `host` has is a public one: an action's URL never reaches this server's own network."""
    try:
        addresses = {info[4][0] for info in socket.getaddrinfo(host, 443, type=socket.SOCK_STREAM)}
    except (OSError, UnicodeError):
        return False
    return bool(addresses) and all(ipaddress.ip_address(a.split("%")[0]).is_global for a in addresses)


def fetch_photo(url):
    """A JPEG of at most `MAX_PHOTO` from a public https URL, without redirects, or a message for the user."""
    parts = urllib.parse.urlsplit(url)
    if parts.scheme != "https" or not parts.hostname:
        return None, "Photo URL must start with https://"
    if not public_host(parts.hostname):
        return None, "Photo URL must be a public address"
    deadline = time.monotonic() + PHOTO_DEADLINE
    try:
        with PHOTO_OPENER.open(urllib.request.Request(url, headers={"User-Agent": "Yo"}), timeout=10) as r:
            data = b""
            while len(data) <= MAX_PHOTO:  # a server that drips bytes is cut off at the deadline
                if time.monotonic() > deadline:
                    raise TimeoutError
                chunk = r.read(16 * 1024)
                if not chunk:
                    break
                data += chunk
    except Exception:  # noqa: BLE001
        return None, "Could not download the photo"
    if len(data) > MAX_PHOTO:
        return None, "Photo is larger than 512 KB"
    if not data.startswith(b"\xff\xd8\xff"):
        return None, "Photo must be a JPEG"
    return data, None


FONT = os.path.join(HERE, "..", "status", "Montserrat-Bold.ttf")  # Yo's typeface (OFL), served by this page


def page(title, body, script=""):
    """Yo's page: amethyst, Montserrat Bold, the wordmark large; light enough for a phone on a slow network."""
    return f"""<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="theme-color" content="#9B59B6"><meta name="color-scheme" content="light">
<title>{html.escape(title)}</title>
<style>
@font-face {{ font-family: Yo; src: url("font.ttf") format("truetype"); font-weight: 700; font-display: swap; }}
:root {{ --amethyst: #9B59B6; --deep: #7D3C98; --ink: #2B0F38; --white: #fff; }}
* {{ box-sizing: border-box; }}
html, body {{ margin: 0; min-height: 100%; }}
body {{ min-height: 100vh; min-height: 100dvh; display: grid; place-items: center; padding: 24px 16px;
  background: radial-gradient(120% 90% at 50% 0%, #B57BCB 0%, var(--amethyst) 45%, var(--deep) 100%);
  color: var(--white); font-family: Yo, Montserrat, -apple-system, system-ui, sans-serif; font-weight: 700;
  text-align: center; -webkit-font-smoothing: antialiased; }}
main {{ width: 100%; max-width: 420px; }}
.lockup {{ display: flex; align-items: center; justify-content: center; gap: 14px; margin-bottom: 18px; }}
.yo {{ font-size: 88px; line-height: .9; letter-spacing: -1px; }}
.plus {{ font-size: 34px; opacity: .55; }}
.partner {{ font-size: 22px; letter-spacing: 1px; padding: 10px 14px 9px; border-radius: 12px;
  background: var(--ink); color: var(--white); }}
h1 {{ font-size: 24px; line-height: 1.25; margin: 0 0 22px; }}
.card {{ position: relative; margin: 0 auto 20px; width: 260px; padding: 14px; border-radius: 28px;
  background: var(--white); color: var(--ink); box-shadow: 0 24px 60px rgba(43, 15, 56, .35); }}
.card::before {{ content: ""; position: absolute; inset: -10px; border-radius: 36px; border: 2px solid rgba(255,255,255,.55);
  animation: pulse 2.4s ease-out infinite; }}
.card svg {{ display: block; width: 100%; height: auto; }}
.code {{ display: inline-flex; gap: 8px; margin: 4px 0 18px; }}
.code span {{ width: 46px; height: 58px; display: grid; place-items: center; font-size: 30px; border-radius: 14px;
  background: rgba(255,255,255,.16); border: 1px solid rgba(255,255,255,.28); }}
.label {{ font-size: 13px; letter-spacing: 1.5px; text-transform: uppercase; opacity: .75; margin: 0; }}
ol {{ list-style: none; padding: 0; margin: 0 auto 20px; max-width: 340px; text-align: left; font-size: 16px; }}
ol li {{ display: flex; align-items: center; gap: 12px; padding: 8px 0; }}
ol li b {{ flex: none; width: 28px; height: 28px; display: grid; place-items: center; border-radius: 50%;
  background: var(--white); color: var(--amethyst); font-size: 15px; }}
a.button {{ display: block; padding: 18px; border-radius: 999px; background: var(--white); color: var(--amethyst);
  text-decoration: none; font-size: 22px; box-shadow: 0 14px 34px rgba(43, 15, 56, .3); margin-bottom: 18px; }}
.status {{ display: inline-flex; align-items: center; gap: 10px; font-size: 15px; opacity: .9; min-height: 24px; }}
.dot {{ width: 10px; height: 10px; border-radius: 50%; background: var(--white); animation: blink 1.2s ease-in-out infinite; }}
.done .yo {{ font-size: 120px; animation: pop .5s cubic-bezier(.2, 1.6, .4, 1) both; }}
.desktop {{ display: none; }}
@media (hover: hover) and (pointer: fine) {{ .desktop {{ display: block; }} .phone {{ display: none; }} }}
@keyframes pulse {{ from {{ transform: scale(1); opacity: .9; }} to {{ transform: scale(1.12); opacity: 0; }} }}
@keyframes blink {{ 50% {{ opacity: .25; }} }}
@keyframes pop {{ from {{ transform: scale(.4); opacity: 0; }} }}
@media (prefers-reduced-motion: reduce) {{ *, *::before {{ animation: none !important; }} }}
</style></head><body><main>{body}</main>{script}</body></html>"""


LOCKUP = '<div class="lockup" aria-label="Yo and IFTTT"><span class="yo">Yo</span><span class="plus">+</span>' \
    '<span class="partner">IFTTT</span></div>'


class Handler(BaseHTTPRequestHandler):
    server_version = "Yo"
    sys_version = ""
    timeout = REQUEST_TIMEOUT
    config: Config
    store: Store
    yo: Yo
    realtime: Realtime
    served = yocli.Tally("requests")

    def log_message(self, *args):  # never who, never which URL (it holds tickets and codes)
        pass

    # ---- plumbing ----

    def route(self):
        path = urllib.parse.urlsplit(self.path).path
        prefix = self.config.prefix
        return path[len(prefix):] if path.startswith(prefix + "/") else None

    def query(self):
        return dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(self.path).query))

    def body(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length < 0 or length > MAX_BODY:
            raise ValueError("too large")
        return self.rfile.read(length) if length else b""

    def json_body(self):
        raw = self.body()
        value = json.loads(raw) if raw.strip() else {}
        if not isinstance(value, dict):
            raise ValueError("not an object")
        return value

    def send(self, status, body=b"", kind="application/json; charset=utf-8", headers=()):
        if isinstance(body, (dict, list)):
            body = json.dumps(body).encode()
        elif isinstance(body, str):
            body = body.encode()
        self.send_response(status)
        if body:
            self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Referrer-Policy", "no-referrer")
        for name, value in headers:
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(body)

    def html(self, status, text):
        self.send(status, text, "text/html; charset=utf-8",
                  [("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; "
                    "script-src 'unsafe-inline'; img-src data:; font-src 'self'; connect-src 'self'; "
                    "frame-ancestors 'none'")])

    def error(self, status, message, skip=False):
        item = {"message": message}
        if skip:
            item["status"] = "SKIP"
        self.send(status, {"errors": [item]})

    def keyed(self):
        if same(self.headers.get("IFTTT-Service-Key", ""), self.config.service_key):
            return True
        self.error(401, "Invalid IFTTT-Service-Key")
        return False

    def bearer(self):
        auth = self.headers.get("Authorization", "")
        holder = self.store.holder(auth[7:].strip()) if auth.startswith("Bearer ") else None
        if holder is None:
            self.error(401, "Invalid access token")
        return holder

    def dispatch(self, method):
        self.served.add()
        route = self.route()
        try:
            handler = ROUTES.get((method, route))
            if handler is None and route and method == "DELETE" and route.startswith(f"/ifttt/v1/triggers/{TRIGGER}/trigger_identity/"):
                handler = Handler.forget_identity
            if handler is None:
                return self.error(404, "Not found")
            handler(self)
        except (ValueError, UnicodeDecodeError):
            self.error(400, "Bad request")
        except yocli.Failed as failed:
            yocli.log(f"yo: {yocli.reason(failed)}")
            self.error(503 if failed.retryable else 500, "Yo is not reachable right now")

    def do_GET(self):
        self.dispatch("GET")

    def do_POST(self):
        self.dispatch("POST")

    def do_DELETE(self):
        self.dispatch("DELETE")

    # ---- OAuth: authorize, approve, wait, token ----

    def authorize(self):
        q = self.query()
        if not same(q.get("client_id", ""), self.config.client_id) or q.get("redirect_uri") != self.config.redirect:
            return self.html(400, page("Yo", f"{LOCKUP}<h1>This link is not for Yo.</h1>"))
        if q.get("response_type", "code") != "code":
            return self.send(302, headers=[("Location", with_query(self.config.redirect, error="unsupported_response_type", state=q.get("state")))])
        made = self.store.new_request(q.get("state", ""), self.config.redirect)
        if made is None:
            return self.html(503, page("Yo", f"{LOCKUP}<h1>Busy right now.<br>Try again in a minute.</h1>"))
        ticket, poll = made
        app = "yo://connect?" + urllib.parse.urlencode({"origin": self.config.origin, "ticket": ticket})
        scan = f"{self.config.origin}{self.config.prefix}/approve?" + urllib.parse.urlencode({"t": ticket})
        wait = f"{self.config.prefix}/wait?" + urllib.parse.urlencode({"p": poll})
        code = "".join(f"<span>{d}</span>" for d in pairing_code(ticket))
        body = f"""<div id="ask">{LOCKUP}
<h1>Connect your Yo account</h1>
<div class="desktop"><div class="card" role="img" aria-label="QR code to open Yo">{qr.svg(scan)}</div>
<ol><li><b>1</b>Scan it with your phone’s camera</li><li><b>2</b>Check Yo shows this code</li>
<li><b>3</b>Tap Allow, and this page carries on</li></ol></div>
<div class="phone"><a class="button" href="{html.escape(app)}">Open Yo</a>
<ol><li><b>1</b>Check Yo shows this code</li><li><b>2</b>Tap Allow, then come back here</li></ol></div>
<p class="label">Pairing code</p><div class="code" aria-label="Pairing code">{code}</div><br>
<div class="status" id="status"><span class="dot"></span>Waiting for Yo…</div></div>"""
        script = f"""<script>
(function poll() {{
  fetch({json.dumps(wait)}, {{cache: "no-store"}}).then(r => r.json()).then(s => {{
    const status = document.getElementById("status");
    if (s.state === "approved" && s.redirect) {{
      document.getElementById("ask").innerHTML = '{LOCKUP.replace("lockup", "lockup done")}<h1>Connected!</h1>'
        + '<div class="status">Taking you back to IFTTT…</div>';
      setTimeout(() => location.replace(s.redirect), 1400);
      return;
    }}
    if (s.redirect) {{ location.replace(s.redirect); return; }}
    if (s.state === "expired") {{ status.textContent = "This link expired. Start again from IFTTT."; return; }}
    setTimeout(poll, 1200);
  }}).catch(() => setTimeout(poll, 3000));
}})();
</script>"""
        self.html(200, page("Connect Yo to IFTTT", body, script))

    def font(self):
        with open(FONT, "rb") as f:
            self.send(200, f.read(), "font/ttf", [("Cache-Control", "public, max-age=604800")])

    def wait(self):
        state, redirect = self.store.poll(self.query().get("p", ""))
        self.send(200, {"state": state, "redirect": redirect})

    def approve_page(self):
        # The QR code's universal link, opened where Yo is not installed.
        self.html(200, page("Yo", f'{LOCKUP}<h1>Open this with Yo on your phone to connect IFTTT.</h1>'
                                  f'<a class="button" href="{APP_STORE}">Get Yo</a>'))

    def approve(self):
        b = self.json_body()
        ticket, user = str(b.get("ticket", "")), str(b.get("user", "")).lower()
        found = self.store.request(ticket)
        if found is None or found[0] or found[1] or found[2]:
            return self.send(410, {"ok": False, "error": "expired"})
        if b.get("allow") is False:
            self.store.deny(ticket)
            return self.send(200, {"ok": True})
        signature = str(b.get("signature", ""))
        # Both go to `yo` as arguments: nothing that could read as a flag.
        if not USER_ID.fullmatch(user) or not SIGNATURE.fullmatch(signature):
            return self.send(400, {"ok": False, "error": "format"})
        ok, username = self.yo.verify(user, self.config.origin, ticket, signature)
        if not ok:
            self.store.burn(ticket)  # one try per ticket: the page starts again
            return self.send(403, {"ok": False, "error": "signature"})
        self.yo.add(user)  # this side's yes (spec 4.10): its Yos are no requests
        self.store.approve(ticket, user, username)
        self.send(200, {"ok": True, "name": "IFTTT"})

    def token(self):
        form = dict(urllib.parse.parse_qsl(self.body().decode()))
        auth = self.headers.get("Authorization", "")
        if auth.startswith("Basic "):  # client credentials in the header, as some clients send them
            import base64

            cid, _, secret = base64.b64decode(auth[6:]).decode().partition(":")
            form.setdefault("client_id", urllib.parse.unquote(cid))
            form.setdefault("client_secret", urllib.parse.unquote(secret))
        if not (same(form.get("client_id", ""), self.config.client_id)
                and same(form.get("client_secret", ""), self.config.client_secret)):
            return self.send(401, {"error": "invalid_client"})
        if form.get("grant_type") != "authorization_code":
            return self.send(400, {"error": "unsupported_grant_type"})
        token = self.store.redeem(form.get("code", ""), form.get("redirect_uri"))
        if token is None:
            return self.send(400, {"error": "invalid_grant", "error_description": "Code unknown, used or expired"})
        self.send(200, {"token_type": "Bearer", "access_token": token})

    # ---- IFTTT Service API ----

    def status(self):
        if self.keyed():
            self.send(200)

    def test_setup(self):
        if not self.keyed():
            return
        if not self.config.test_user:
            return self.error(503, "No test account set (IFTTT_TEST_USER)")
        user = self.config.test_user
        while len(self.store.events(user, 3)) < 3:
            self.store.record(user, "yo")
        self.store.run("DELETE FROM tokens WHERE user = ?", user)  # one test token at a time
        token = self.store.issue(user, "IFTTT test")
        self.send(200, {"data": {"accessToken": token, "samples": {
            "triggers": {TRIGGER: {}},
            "actions": {ACTION: {"photo_url": ""}},
        }}})

    def user_info(self):
        holder = self.bearer()
        if holder:
            user, username = holder
            info = {"name": username or "Yo", "id": user}
            if username:
                info["url"] = f"{self.config.origin}/{urllib.parse.quote(username)}"
            self.send(200, {"data": info})

    def trigger(self):
        holder = self.bearer()
        if not holder:
            return
        b = self.json_body()
        limit = b.get("limit", KEEP)
        if not isinstance(limit, int) or isinstance(limit, bool):
            limit = KEEP
        identity = b.get("trigger_identity")
        if isinstance(identity, str) and 0 < len(identity) <= 200:
            self.store.watch(identity, holder[0])
        items = [{"received_at": iso(at_ms), "kind": kind, "meta": {"id": eid, "timestamp": at_ms // 1000}}
                 for eid, at_ms, kind in self.store.events(holder[0], limit)]
        self.send(200, {"data": items})

    def forget_identity(self):
        if self.keyed():
            self.store.unwatch(urllib.parse.unquote(self.route().rsplit("/", 1)[-1]))
            self.send(200)

    def action(self):
        holder = self.bearer()
        if not holder:
            return
        fields = self.json_body().get("actionFields")
        if not isinstance(fields, dict):
            return self.error(400, "actionFields missing")
        url = str(fields.get("photo_url") or "").strip()
        photo = None
        if url:
            photo, problem = fetch_photo(url)
            if problem:
                return self.error(400, problem, skip=True)
        try:
            self.yo.send(holder[0], photo)
        except yocli.Failed as failed:
            if failed.retryable:
                raise
            return self.error(400, "Yo could not reach you. Is IFTTT still your friend on Yo?", skip=True)
        self.send(200, {"data": [{"id": secrets.token_hex(12)}]})


ROUTES = {
    ("GET", "/authorize"): Handler.authorize,
    ("GET", "/font.ttf"): Handler.font,
    ("GET", "/wait"): Handler.wait,
    ("GET", "/approve"): Handler.approve_page,
    ("POST", "/approve"): Handler.approve,
    ("POST", "/token"): Handler.token,
    ("GET", "/ifttt/v1/status"): Handler.status,
    ("POST", "/ifttt/v1/test/setup"): Handler.test_setup,
    ("GET", "/ifttt/v1/user/info"): Handler.user_info,
    ("POST", f"/ifttt/v1/triggers/{TRIGGER}"): Handler.trigger,
    ("POST", f"/ifttt/v1/actions/{ACTION}"): Handler.action,
}


def receive(store, realtime):
    """The bot's Yos: one event per Yo from a linked user, directly (not in a group); then IFTTT is told."""
    kept, ignored, limited = yocli.Tally("Yos kept"), yocli.Tally("messages ignored"), yocli.Tally("Yos over the limit")
    limits = yocli.Buckets()
    for event in yocli.listen("--drain-media"):
        if event.get("type") != "message" or event.get("group") or event.get("content") not in KINDS:
            continue
        user = event.get("from", "")
        if not store.linked(user):
            ignored.add()
            continue
        if not limits.allow(user, time.monotonic()):
            limited.add()
            continue
        store.record(user, event["content"])
        realtime.notify(user)
        kept.add()


def make_server(address, config, store, yo, realtime):
    handler = type("Bound", (Handler,), {"config": config, "store": store, "yo": yo, "realtime": realtime})
    return ThreadingHTTPServer(address, handler)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", default=os.path.join(os.environ.get("STATE_DIRECTORY", "."), "ifttt.sqlite"))
    parser.add_argument("--listen", default="127.0.0.1:8095")
    args = parser.parse_args()
    config = Config(os.environ)
    yocli.registered()
    store = Store(args.db)
    realtime = Realtime(config, store).start()
    host, _, port = args.listen.rpartition(":")
    server = make_server((host, int(port)), config, store, Yo(), realtime)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yocli.log(f"serving {config.prefix} on {args.listen}")
    receive(store, realtime)


if __name__ == "__main__":
    main()
