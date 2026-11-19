#!/usr/bin/env python3
"""A local demo of the admin page with a made-up world, for working on its design. Python's standard library only.

    python3 admin/dev/demo.py --demo            # http://127.0.0.1:8099

Builds two fake hub databases (live ~120 users, dev a few) with the hub's schema in a temporary directory, writes
users.json from them with the real collector (users.hub), and serves the real admin.py on 127.0.0.1 only, with:
- no Cloudflare Access and no login (only with --demo or YO_ADMIN_DEMO=1, and only on a loopback address);
- the hub's admin API (support threads, replies, to-dos, diagnostics, reserved names) answered from the fake
  databases, so every button works; nothing ever reaches a real hub (outgoing HTTP is blocked in this process);
- made-up server numbers (CPU, memory, disk, network, systemd units, updates) instead of this machine's;
- a "Demo" badge in the header.
Nothing here is used by the production admin; admin.py is not changed. The world is new on every start.
"""

import argparse
import atexit
import hashlib
import ipaddress
import json
import math
import os
import random
import shutil
import sqlite3
import sys
import tempfile
import threading
import time
import unicodedata
import urllib.request

ADMIN_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HOUR, DAY = 3600_000, 86400_000

SCHEMA = """
CREATE TABLE users (id BLOB PRIMARY KEY, account_key BLOB NOT NULL UNIQUE, created_at INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE account_keys (key BLOB PRIMARY KEY, user_id BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE handles (kind INTEGER NOT NULL, value BLOB NOT NULL, user_id BLOB NOT NULL, spelling TEXT,
    PRIMARY KEY (kind, value)) WITHOUT ROWID;
CREATE TABLE reserved (skeleton TEXT PRIMARY KEY, spelling TEXT NOT NULL, note TEXT, created_at INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE devices (id BLOB PRIMARY KEY, user_id BLOB NOT NULL, device_key BLOB NOT NULL UNIQUE, auth_key BLOB NOT NULL,
    push_platform INTEGER NOT NULL, push_token BLOB NOT NULL, created_at INTEGER NOT NULL, last_seen_at INTEGER NOT NULL,
    app TEXT, registration_id INTEGER NOT NULL DEFAULT 0, number INTEGER NOT NULL DEFAULT 1,
    prekey_max INTEGER NOT NULL DEFAULT 0) WITHOUT ROWID;
CREATE TABLE gone (id BLOB PRIMARY KEY, gone_at INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE prekeys (device_id BLOB NOT NULL, kind INTEGER NOT NULL, id INTEGER NOT NULL, key BLOB NOT NULL,
    signature BLOB NOT NULL, PRIMARY KEY (device_id, kind, id)) WITHOUT ROWID;
CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, device_id BLOB NOT NULL, envelope BLOB NOT NULL,
    received_at INTEGER NOT NULL);
CREATE TABLE backups (user_id BLOB PRIMARY KEY, locator BLOB UNIQUE, record BLOB NOT NULL, updated_at INTEGER NOT NULL,
    auth_passkey BLOB, auth_code BLOB) WITHOUT ROWID;
CREATE TABLE reports (reporter_id BLOB NOT NULL, reported_id BLOB NOT NULL, reason INTEGER NOT NULL,
    created_at INTEGER NOT NULL, PRIMARY KEY (reporter_id, reported_id)) WITHOUT ROWID;
CREATE TABLE deliveries_hourly (hour INTEGER PRIMARY KEY, n INTEGER NOT NULL);
CREATE TABLE feedback (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id BLOB NOT NULL, text TEXT NOT NULL,
    app_version TEXT NOT NULL, created_at INTEGER NOT NULL, auto INTEGER NOT NULL DEFAULT 0,
    type TEXT NOT NULL DEFAULT 'feedback');
CREATE TABLE contact_keys (user_id BLOB PRIMARY KEY, current BLOB NOT NULL, requests_open INTEGER NOT NULL DEFAULT 1) WITHOUT ROWID;
CREATE TABLE storage (user_id BLOB PRIMARY KEY, version INTEGER NOT NULL, data BLOB NOT NULL, updated_at INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE blobs (id BLOB PRIMARY KEY, user_id BLOB NOT NULL, size INTEGER NOT NULL, fetches_left INTEGER NOT NULL,
    created_at INTEGER NOT NULL, fetching INTEGER NOT NULL DEFAULT 0, gets_left INTEGER NOT NULL DEFAULT 192,
    ready INTEGER NOT NULL DEFAULT 1) WITHOUT ROWID;
CREATE TABLE profile_commitments (user_id BLOB PRIMARY KEY, commitment BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE zk_groups (id BLOB PRIMARY KEY, params BLOB NOT NULL, revision INTEGER NOT NULL, state BLOB NOT NULL,
    updated_at INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE blob_quota (user_id BLOB NOT NULL, day INTEGER NOT NULL, count INTEGER NOT NULL, bytes INTEGER NOT NULL,
    PRIMARY KEY (user_id, day)) WITHOUT ROWID;
CREATE TABLE settings (key TEXT PRIMARY KEY, value INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE diagnostics (id INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK(kind IN ('crash','error')),
    signature TEXT NOT NULL UNIQUE, count INTEGER NOT NULL DEFAULT 1, first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL, app TEXT NOT NULL, os TEXT NOT NULL, model TEXT NOT NULL, payload TEXT NOT NULL,
    done INTEGER NOT NULL DEFAULT 0);
CREATE TABLE diagnostic_days (day INTEGER NOT NULL, kind TEXT NOT NULL, count INTEGER NOT NULL, PRIMARY KEY(day,kind)) WITHOUT ROWID;
CREATE TABLE diagnostic_seen (uuid TEXT PRIMARY KEY, diagnostic_id INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE support_messages (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id BLOB NOT NULL,
    from_team INTEGER NOT NULL CHECK(from_team IN (0,1)), text TEXT NOT NULL, created_at INTEGER NOT NULL,
    read_at INTEGER, app_version TEXT NOT NULL DEFAULT '', done_at INTEGER);
"""

# ---- names --------------------------------------------------------------------------------------------------------

FIRST = ["lukas", "sofia", "mateo", "amara", "priya", "arjun", "chen_wei", "minjun", "jiwoo", "lena", "jonas", "emma",
         "noah", "zeynep", "elif", "omar", "fatima", "inès", "joão", "björn", "astrid", "léa", "chloé", "mia", "luca",
         "giulia", "pablo", "valentina", "kofi", "ade", "chidi", "nia", "anya", "dmitri", "olga", "kasia", "mateusz",
         "tomás", "ana_lu", "rafa", "camille", "hugo", "nour", "yusuf", "aylin", "freya", "oskar", "maja", "sven",
         "ingrid", "thanh", "linh", "bao", "tariq", "leila", "sami", "noa", "ari", "tal", "eitan", "kai", "nalu",
         "moana", "tane", "aroha", "diego", "lucía", "andrés", "zoë", "milan", "petra", "radek", "eszter", "bence"]
JAPANESE = ["yuki", "haruto", "sakura", "kenji", "aiko", "ren", "mei", "sora", "takumi", "hinata", "yui", "riku",
            "haruka", "daiki", "nanami", "kaito", "airi", "sota", "miyu", "yamato", "tanaka", "suzuki", "kobayashi"]
EXTRA = ["_dev", "_x", "_jp", "99", "_2k", "yo", "_real", "san", "chan", "kun", "_k", "7", "_m", "_art"]
EMOJI = ["🌸", "🦊", "😎", "🍕", "🐼", "🚀", "🌊", "🔥", "✨", "🍣", "🎧", "🐙", "🌵", "🍜", "⚡"]


def skeleton(name: str) -> str:
    """Close to the hub's (yo_wire::handle::skeleton): upper case, accents and variation selectors dropped."""
    s = "".join(c for c in unicodedata.normalize("NFKD", name.upper())
                if unicodedata.category(c) != "Mn" and c not in "︎️")
    return s.replace("0", "O").replace("1", "I").replace("VV", "W")


def names(rng: random.Random, n: int) -> list:
    out, seen = [], {"YOGPT", "STATUS"}
    while len(out) < n:
        r = rng.random()
        base = rng.choice(JAPANESE if r < 0.3 else FIRST)
        if r < 0.15:
            name = base + "_" + rng.choice(JAPANESE)
        elif r < 0.27:
            name = rng.choice([base + rng.choice(EMOJI), rng.choice(EMOJI) + base])
        elif r < 0.45:
            name = base + rng.choice(EXTRA)
        elif r < 0.5:
            name = base.capitalize()
        else:
            name = base
        if len(name) >= 3 and skeleton(name) not in seen:
            seen.add(skeleton(name))
            out.append(name)
    return out


# ---- the world ----------------------------------------------------------------------------------------------------

def rid(rng, n=16) -> bytes:
    return bytes(rng.getrandbits(8) for _ in range(n))


def ios(ms: int) -> str:
    return "ios/" + time.strftime("%Y%m%d%H%M", time.gmtime(ms / 1000))


SUPPORT = [
    [("u", "Yo! Is there a way to see who sent me a Yo when the notification is gone?"), ("t", "Yo 👋 Open the chat list, the newest Yo is on top. A history view is planned."), ("u", "ah got it, thanks!")],
    [("u", "The app crashed right after I took a photo, twice now. iPhone 13 mini.")],
    [("u", "Can I use Yo on my iPad too?"), ("t", "Not yet, iPhone only for now. iPad is on the list."), ("u", "ok cool"), ("u", "and Android? my sister wants it")],
    [("u", "Love the app!!! 💜 Super simple. Please add dark purple app icon")],
    [("u", "I lost my phone. How do I get my name back? It was secured.")],
    [("u", "Notifications stopped working since the last update."), ("t", "Thanks! Could you open the app once and check Settings → Notifications? We fixed a bug in today's build."), ("u", "Works again ✅")],
    [("u", "Someone keeps sending me Yos at 3am lol. Can I block them?")],
    [("u", "日本語に対応していますか? (sorry, does it work in Japanese?)"), ("t", "Yo! Usernames work in romaji and with emoji. Japanese text is coming.")],
    [("u", "Group name doesn't update for others when I change it.")],
    [("u", "Bug: the Yo sound plays twice when the app is open."), ("t", "Got it, we see it too. Fix is in the next build.")],
    [("u", "Hi, how do I delete my account?")],
    [("u", "Videos longer than 10s don't send?")],
    [("u", "Yo"), ("t", "Yo 😄")],
    [("u", "Could you add a widget? Want to Yo my partner from the home screen.")],
]
TODOS = [
    ("Answer the App Store review note about account deletion", None),
    ("Check APNs certificate expiry (renew before December)", None),
    ("Yo sound plays twice in foreground: reproduce on iOS 26.1", None),
    ("Write the FAQ entry: secured names and moving to a new phone", None),
    ("Rate limit for group invites: look at the numbers from Monday", None),
    ("Translate onboarding into Japanese", None),
    ("Open Graph preview for justyo.co links", None),
    ("Ask the photo-crash user for the build number", "photo"),
    ("Back up hub DB before the schema 7 migration", "done"),
    ("Remove the old feedback endpoint", "done"),
    ("Update reserved names list with team members", "done"),
]
CRASHES = [
    ("crash", {"exception": "EXC_BAD_ACCESS (SIGSEGV)", "details": "KERN_INVALID_ADDRESS at 0x0000000000000010",
               "frames": "0  Yo  ChatView.body.getter + 412\n1  SwiftUI  ViewGraph.updateOutputs + 220\n2  SwiftUI  GraphHost.flushTransactions + 96"}),
    ("crash", {"exception": "EXC_CRASH (SIGABRT)", "termination": "Namespace SPRINGBOARD, Code 0x8badf00d",
               "frames": "0  libsystem_kernel  __pthread_kill + 8\n1  Yo  CameraSession.start() + 188\n2  Yo  ComposeView.takePhoto() + 64"}),
    ("crash", {"exception": "EXC_BREAKPOINT (SIGTRAP)", "details": "Swift runtime failure: Index out of range",
               "frames": "0  Yo  GroupMembers.subscript.getter + 52\n1  Yo  GroupHeader.title + 140"}),
    ("crash", {"termination": "Jetsam: per-process limit", "details": "Memory 1.9 GB while uploading a video"}),
    ("error", {"domain": "NSURLErrorDomain", "code": -1001, "place": "Hub.fetchMessages", "message": "The request timed out."}),
    ("error", {"domain": "NSURLErrorDomain", "code": -1009, "place": "Hub.send", "message": "The Internet connection appears to be offline.", "severity": "expected"}),
    ("error", {"domain": "YoCore", "code": 17, "place": "Session.decrypt", "message": "untrusted identity: the contact reinstalled"}),
    ("error", {"domain": "APNs", "code": 410, "place": "Push.register", "message": "token no longer valid", "severity": "expected"}),
    ("error", {"domain": "NSCocoaErrorDomain", "code": 4, "place": "Keychain.read", "message": "Locked since restart: push arrived before first unlock", "severity": "expected"}),
    ("error", {"domain": "YoCore", "code": 3, "place": "Backup.upload", "message": "backup record too large (412 KB)"}),
    ("error", {"domain": "AVFoundation", "code": -11800, "place": "VideoExport.run", "message": "The operation could not be completed (-16364)"}),
    ("error", {"domain": "YoCore", "code": 29, "place": "Groups.apply", "message": "group revision behind: 14 < 15"}),
    ("crash", {"exception": "EXC_BAD_ACCESS (SIGBUS)", "frames": "0  libswiftCore  swift_retain + 12\n1  Yo  ImageCache.thumbnail(for:) + 210"}),
]
RESERVED = [("Yo", "the app"), ("YoGPT", "our bot"), ("STATUS", "our status bot"), ("admin", "staff"), ("support", "staff"),
            ("help", "staff"), ("team", "staff"), ("official", "impersonation"), ("justyo", "domain"), ("apple", "brand"),
            ("google", "brand"), ("root", "system"), ("security", "staff"), ("null", "system"), ("moderator", "staff"),
            ("press", "staff"), ("billing", "scam bait"), ("verified", "impersonation")]


def make_hub(path: str, rng: random.Random, now: int, n_users: int, live: bool) -> dict:
    """One fake hub database; returns facts the rest of the demo needs (ids by role)."""
    db = sqlite3.connect(path)
    db.executescript(SCHEMA)
    midnight = now - now % DAY
    newest = ios(now - 4 * HOUR - 17 * 60_000)
    builds = [newest, ios(now - 2 * DAY - 3 * HOUR), ios(now - 5 * DAY - 7 * HOUR), ios(now - 9 * DAY)]
    users = []

    def signup_time(i):
        if i < 8:  # the first testers, weeks ago
            return now - rng.randint(15, 29) * DAY - rng.randint(0, DAY)
        if live and i >= n_users - 26:  # today's spike, mostly in the last hours
            return now - int(abs(rng.gauss(2.5, 2.2)) * HOUR) - rng.randint(0, HOUR)
        day = rng.choices(range(1, 15), weights=[3, 4, 5, 3, 2, 6, 3, 2, 2, 3, 1, 2, 1, 1])[0]
        return midnight - day * DAY + rng.randint(6 * HOUR, 23 * HOUR)

    spellings = names(rng, n_users)
    for i, name in enumerate(spellings):
        uid, created = rid(rng), min(now - 60_000, signup_time(i))
        db.execute("INSERT INTO users VALUES (?,?,?)", (uid, rid(rng, 32), created))
        db.execute("INSERT INTO account_keys VALUES (?,?)", (rid(rng, 32), uid))
        db.execute("INSERT INTO handles VALUES (2,?,?,?)", (skeleton(name).encode(), uid, name))
        db.execute("INSERT INTO contact_keys VALUES (?,?,1)", (uid, rid(rng, 32)))
        db.execute("INSERT INTO profile_commitments VALUES (?,?)", (uid, rid(rng, 64)))
        db.execute("INSERT INTO storage VALUES (?,?,?,?)", (uid, rng.randint(1, 40), rid(rng, 96), created + rng.randint(0, now - created)))
        platform = rng.choices(["ios", "android", "dev"], weights=[86, 10, 4])[0]
        if platform == "ios":
            age = now - created
            app = rng.choices(builds, weights=[60 if age < 2 * DAY else 45, 25, 15, 8])[0]
        elif platform == "android":
            app = f"android/{rng.choice([3, 4, 5])}"
        else:
            app = "ios/1"
        for d in range(rng.choices([1, 2, 3], weights=[80, 16, 4])[0]):
            seen = max(created, now - int(rng.expovariate(1 / (2.5 * DAY))))
            push = 3 if platform == "android" else rng.choices([1, 2, 0], weights=[85, 5, 10])[0]
            db.execute("INSERT INTO devices (id,user_id,device_key,auth_key,push_platform,push_token,created_at,"
                       "last_seen_at,app,registration_id,number,prekey_max) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                       (rid(rng), uid, rid(rng, 33), rid(rng, 32), push, rid(rng, 32) if push else b"",
                        created + d * rng.randint(0, DAY), seen, app if d == 0 else rng.choice(builds),
                        rng.randint(1, 16000), d + 1, rng.randint(20, 300)))
        if rng.random() < 0.35:
            db.execute("INSERT INTO backups VALUES (?,?,?,?,?,?)", (uid, rid(rng), rid(rng, 120),
                       created + rng.randint(60_000, max(120_000, now - created)), rid(rng, 32), None))
        users.append(uid)
    # The bots.
    for name, app in (("YoGPT", "yogpt/" + time.strftime("%Y%m%d%H%M", time.gmtime((now - 26 * HOUR) / 1000))), ("STATUS", "cli/7")):
        uid = rid(rng)
        db.execute("INSERT INTO users VALUES (?,?,?)", (uid, rid(rng, 32), now - 28 * DAY))
        db.execute("INSERT INTO handles VALUES (2,?,?,?)", (skeleton(name).encode(), uid, name))
        db.execute("INSERT INTO devices (id,user_id,device_key,auth_key,push_platform,push_token,created_at,last_seen_at,app)"
                   " VALUES (?,?,?,?,0,X'',?,?,?)", (rid(rng), uid, rid(rng, 33), rid(rng, 32), now - 28 * DAY, now - 600_000, app))
        db.execute("INSERT INTO backups VALUES (?,?,?,?,NULL,NULL)", (uid, rid(rng), rid(rng, 120), now - 20 * DAY))
    devices = [r[0] for r in db.execute("SELECT id FROM devices")]
    # Yos waiting for their device; photos and videos until their last fetch.
    for _ in range(rng.randint(30, 55) if live else 4):
        db.execute("INSERT INTO messages (device_id,envelope,received_at) VALUES (?,?,?)",
                   (rng.choice(devices), rid(rng, 180), now - int(rng.expovariate(1 / (6 * HOUR)))))
    for _ in range(rng.randint(6, 14) if live else 1):
        db.execute("INSERT INTO blobs VALUES (?,?,?,?,?,0,192,1)", (rid(rng), rng.choice(users), rng.randint(80_000, 9_000_000),
                   rng.randint(1, 4), now - rng.randint(0, 20 * HOUR)))
    db.execute("INSERT INTO gone VALUES (?,?)", (rid(rng), now - 3 * DAY))
    for dev in devices[:40]:
        for k in range(3):
            db.execute("INSERT INTO prekeys VALUES (?,?,?,?,?)", (dev, k, rng.randint(1, 999), rid(rng, 33), rid(rng, 64)))
    # Groups: some made today and not changed since.
    for g in range(rng.randint(22, 30) if live else 2):
        fresh = g < 4 and live
        db.execute("INSERT INTO zk_groups VALUES (?,?,?,?,?)", (rid(rng, 32), rid(rng, 97), 0 if fresh else rng.randint(1, 40),
                   rid(rng, 300), now - rng.randint(0, 6 * HOUR) if fresh else now - rng.randint(DAY, 12 * DAY)))
    # Deliveries per hour (8 days), a day curve with an evening peak, more today.
    hour0 = now // HOUR
    for h in range(hour0 - 8 * 24 + 1, hour0 + 1):
        local = (h + 1) % 24  # Berlin, roughly
        base = (8 if live else 1) + (34 if live else 3) * max(0, math.sin((local - 6) / 17 * math.pi))
        boost = 1.6 if h >= hour0 - 12 else 1.0
        n = int(rng.gauss(base * boost, base * 0.2))
        if h == hour0:
            n = n * (int(now % HOUR) // 60_000) // 60
        db.execute("INSERT INTO deliveries_hourly VALUES (?,?)", (h, max(0, n)))
    # Photos and videos today, per uploader.
    media_hours = {}
    for h in range(midnight // HOUR, hour0 + 1):
        local = (h + 1) % 24
        media_hours[h] = max(0, int(rng.gauss((2 if live else 0.2) + (7 if live else 0.5) * max(0, math.sin((local - 7) / 16 * math.pi)), 1.5)))
    total = sum(media_hours.values())
    uploaders = rng.sample(users, k=min(len(users), 18 if live else 2))
    for k, u in enumerate(uploaders):
        c = total // len(uploaders) + (1 if k < total % len(uploaders) else 0)
        if c:
            db.execute("INSERT INTO blob_quota VALUES (?,?,?,?)", (u, now // DAY, c, c * rng.randint(400_000, 3_000_000)))
        db.execute("INSERT INTO blob_quota VALUES (?,?,?,?)", (u, now // DAY - 1, rng.randint(1, 6), rng.randint(1, 9) * 1_000_000))
    # Reports.
    for reason, age in [(1, 2 * HOUR), (2, 9 * HOUR), (1, DAY + 3 * HOUR), (3, 3 * DAY), (2, 6 * DAY), (1, 10 * DAY)][: 6 if live else 1]:
        a, b = rng.sample(users, 2)
        db.execute("INSERT OR IGNORE INTO reports VALUES (?,?,?,?)", (a, b, reason, now - age))
    # Old feedback (before the support chat).
    db.execute("INSERT INTO feedback (user_id,text,app_version,created_at,type) VALUES (?,?,?,?,'feedback')",
               (users[1], "Love it, but the icon should be bigger", builds[3], now - 20 * DAY))
    # Support threads: (u)ser and (t)eam lines, the newest open ones unread.
    threads = rng.sample(users[8:] if live else users, k=len(SUPPORT) if live else 2)
    photo_user = None
    for k, (uid, lines) in enumerate(zip(threads, SUPPORT)):
        t = now - int((k + rng.random()) * (11 * HOUR if k < 5 else DAY))
        if "photo" in lines[0][1]:
            photo_user = uid
        for j, (who, text) in enumerate(lines):
            t += rng.randint(3, 90) * 60_000
            t = min(t, now - 60_000)
            team = who == "t"
            answered = any(w == "t" for w, _ in lines[j + 1:])
            done = None if (not team and not answered and k % 3 != 2) else (t + 600_000 if not team else None)
            read = t + rng.randint(5, 120) * 60_000 if team and j < len(lines) - 1 or (team and rng.random() < 0.5) else None
            db.execute("INSERT INTO support_messages (user_id,from_team,text,created_at,read_at,app_version,done_at) VALUES (?,?,?,?,?,?,?)",
                       (uid, int(team), text, t, read if read and read < now else None, newest if not team else "", done))
    todo_ids = {}
    for k, (text, flag) in enumerate(TODOS if live else TODOS[:2]):
        at = now - (k + 1) * rng.randint(3 * HOUR, 20 * HOUR)
        cur = db.execute("INSERT INTO support_messages (user_id,from_team,text,created_at,done_at) VALUES (X'',1,?,?,?)",
                         (text, at, at + 5 * HOUR if flag == "done" else None))
        if flag == "photo":
            todo_ids["photo"] = cur.lastrowid
        if k == 1:
            todo_ids["star"] = cur.lastrowid
    # Crashes and errors.
    n_open = 0
    for k, (kind, payload) in enumerate(CRASHES if live else CRASHES[:3]):
        first = now - rng.randint(1, 13) * DAY - rng.randint(0, DAY)
        last = min(now - 120_000, first + rng.randint(0, now - first))
        done = int(k in (3, 9))
        n_open += not done
        db.execute("INSERT INTO diagnostics (kind,signature,count,first_seen,last_seen,app,os,model,payload,done) VALUES (?,?,?,?,?,?,?,?,?,?)",
                   (kind, hashlib.sha256(json.dumps(payload).encode()).hexdigest()[:24],
                    rng.choice([1, 1, 2, 3, 5, 8, 14, 41, 230]) if payload.get("severity") != "expected" else rng.randint(80, 900),
                    first, last, rng.choice(builds[:3]), rng.choice(["iOS 26.0.1", "iOS 26.1", "iOS 26.2"]),
                    rng.choice(["iPhone15,2", "iPhone14,4", "iPhone16,1", "iPhone17,3", "iPhone13,2"]), json.dumps(payload), done))
    for d in range(now // DAY - 13, now // DAY + 1):
        db.execute("INSERT INTO diagnostic_days VALUES (?,?,?)", (d, "crash", rng.randint(0, 4 if live else 1)))
        db.execute("INSERT INTO diagnostic_days VALUES (?,?,?)", (d, "error", rng.randint(2, 30 if live else 3)))
    db.execute("INSERT INTO diagnostic_seen VALUES (?,1)", ("550e8400-e29b-41d4-a716-446655440000",))
    # Reserved names.
    db.execute("INSERT INTO settings VALUES ('reserved_enabled', 1)")
    for name, note in RESERVED if live else RESERVED[:4]:
        db.execute("INSERT INTO reserved VALUES (?,?,?,?)", (skeleton(name), name, note, now - 25 * DAY))
    db.commit()
    db.close()
    return {"threads": threads, "photo_user": photo_user, "todo_ids": todo_ids, "media_hours": media_hours,
            "newest": newest, "internal": users[0]}


# ---- the fake hub admin API (what admin.py asks the hubs), on the fake databases -----------------------------------

class FakeHubs:
    def __init__(self, paths, on_change):
        self.paths, self.on_change, self.lock = paths, on_change, threading.Lock()

    def db(self, hub):
        return sqlite3.connect(self.paths[hub])

    def support(self, hub, suffix="", body=None):
        now = int(time.time() * 1000)
        with self.lock, self.db(hub) as db:
            parts = suffix.split("/")
            if body is None and suffix == "messages":
                at, open_ = db.execute("SELECT coalesce(max(created_at),0),coalesce(sum(done_at IS NULL),0) FROM support_messages WHERE user_id=X''").fetchone()
                rows = [{"user_id": "team", "name": "Team To-Dos", "unread": 0, "open": open_, "at": at}]
                rows += [{"user_id": u, "at": a, "unread": n, "name": name} for u, a, n, name in db.execute(
                    "SELECT hex(s.user_id),max(s.created_at),sum(s.from_team=0 AND s.done_at IS NULL),"
                    "(SELECT coalesce(spelling,value) FROM handles WHERE user_id=s.user_id AND kind=2 LIMIT 1) "
                    "FROM support_messages s WHERE length(s.user_id)>0 GROUP BY s.user_id "
                    "ORDER BY (sum(s.from_team=0 AND s.done_at IS NULL)>0) DESC,max(s.created_at) DESC")]
                return rows
            if body is None and parts[0] == "messages" and len(parts) == 2:
                user = b"" if parts[1] == "team" else bytes.fromhex(parts[1])
                return {"messages": [{"id": i, "from_team": bool(t), "text": x, "created_at": c, "read_at": r, "done_at": d}
                                     for i, t, x, c, r, d in db.execute("SELECT id,from_team,text,created_at,read_at,done_at FROM "
                                     "support_messages WHERE user_id=? ORDER BY created_at,id", (user,))]}
            if parts[0] == "messages" and len(parts) == 2:  # a reply
                user = bytes.fromhex(parts[1])
                if not db.execute("SELECT 1 FROM support_messages WHERE user_id=?", (user,)).fetchone():
                    raise OSError("409: no conversation")
                db.execute("INSERT INTO support_messages (user_id,from_team,text,created_at) VALUES (?,1,?,?)", (user, body, now))
            elif parts[0] == "messages" and parts[-1] == "handled":
                db.execute("UPDATE support_messages SET done_at=? WHERE user_id=? AND from_team=0 AND id<=? AND done_at IS NULL",
                           (now, bytes.fromhex(parts[1]), int(body)))
            elif parts[0] in ("messages", "todos"):  # messages/done/<id>, todos/<id>
                db.execute("UPDATE support_messages SET done_at=? WHERE id=?", (now if body == "1" else None, int(parts[-1])))
            else:
                raise ValueError(suffix)
        self.on_change()
        return {}

    def todo(self, hub, text):
        with self.lock, self.db(hub) as db:
            cur = db.execute("INSERT INTO support_messages (user_id,from_team,text,created_at) VALUES (X'',1,?,?)",
                             (text, int(time.time() * 1000)))
        self.on_change()
        return {"id": cur.lastrowid}

    def reserved(self, hub, method="GET", data=None, name=None):
        with self.lock, self.db(hub) as db:
            if method == "POST":
                for n in data:
                    db.execute("INSERT OR IGNORE INTO reserved VALUES (?,?,NULL,?)", (skeleton(n.strip()), n.strip(), int(time.time() * 1000)))
            elif method == "PUT":
                db.execute("UPDATE settings SET value=? WHERE key='reserved_enabled'", (int(bool(data["enabled"])),))
            elif method == "DELETE":
                db.execute("DELETE FROM reserved WHERE skeleton=?", (skeleton(name),))
            out = {"enabled": bool(db.execute("SELECT value FROM settings WHERE key='reserved_enabled'").fetchone()[0]),
                   "names": [r[0] for r in db.execute("SELECT spelling FROM reserved ORDER BY skeleton")],
                   "name_min": 3, "name_max": 32, "name_comparison": "skeleton",
                   "name_form": "NFC Latin letters, digits, underscore or emoji; starts with a letter or emoji"}
        if method != "GET":
            self.on_change()
        return out if method != "DELETE" else {}

    def diagnostics(self, hub, data):
        with self.lock, self.db(hub) as db:
            db.execute("UPDATE diagnostics SET done=? WHERE id=?", (int(data["done"]), data["id"]))
        self.on_change()
        return []


# ---- the made-up server --------------------------------------------------------------------------------------------

class FakeServer:
    def __init__(self, rng):
        self.rng, self.cpu, self.rx, self.tx, self.boot = rng, 9.0, 41_822_331_904, 12_904_118_272, time.time() - 6 * 86400 - 5 * 3600

    def stats(self, reserved):
        r = self.rng
        self.cpu = min(95, max(2, self.cpu + r.gauss(0, 4)))
        rx_s, tx_s = abs(r.gauss(180_000, 90_000)), abs(r.gauss(95_000, 50_000))
        self.rx += int(rx_s * 3)
        self.tx += int(tx_s * 3)
        unit = lambda since, mem, restarts=0: {"state": "active", "sub": "running", "since": int(since), "restarts": restarts,  # noqa: E731
                                                "memory": mem, "cpu_ns": 9_812_000_000, "tasks": 14}
        return {
            "host": "yo-hub-demo", "kernel": "6.8.0-88-generic", "uptime": time.time() - self.boot,
            "cpu": {"percent": round(self.cpu, 1), "cores": 4, "load": [round(self.cpu / 25, 2), 0.41, 0.37]},
            "memory": {"total": 8 << 30, "available": int((5.1 + r.random() * 0.2) * (1 << 30)), "swap_total": 2 << 30, "swap_free": int(1.9 * (1 << 30))},
            "disk": {"total": 160 << 30, "used": int(38.4 * (1 << 30)), "free": int(121.6 * (1 << 30))},
            "network": {"rx_per_s": rx_s, "tx_per_s": tx_s, "rx": self.rx, "tx": self.tx},
            "reboot": {"needed": False, "packages": []},
            "hub": {**unit(time.time() - 4 * 3600 - 1200, 61 << 20), "version": "3cf0c99 2026-11-19T13:02:00Z"},
            "hub_dev": unit(time.time() - 26 * 3600, 23 << 20),
            "tunnel": unit(self.boot, 38 << 20, 1),
            "yogpt": unit(time.time() - 26 * 3600, 112 << 20),
            "reserved": reserved,
        }

    def updates(self):
        now = time.time()
        return {"at": now, "errors": [], "status": "warn",
                "apt": {"packages": 5, "security": 1, "security_since": now - 9 * 3600, "checked": now - 420},
                "reboot": False, "reboot_packages": [], "running_kernel": "6.8.0-88-generic", "installed_kernel": "6.8.0-88-generic",
                "unattended": "2026-11-19 06:14:02,118 INFO Starting unattended upgrades script\n2026-11-19 06:14:40,903 INFO All upgrades installed",
                "last_upgrade_trigger": "Thu 2026-11-19 06:14:01 UTC", "next_reboot": "Sun 2026-11-22 04:00:00 UTC",
                "current_release": "Ubuntu 24.04.3 LTS", "release": {"checked": now - 3600, "result": "No new release found"}}


# ---- wiring ---------------------------------------------------------------------------------------------------------

BADGE_CSS = """.demo-badge { margin-left: 8px; padding: 3px 8px; border-radius: 6px; background: #f5c542; color: #000;
  font-size: 11px; font-weight: 800; letter-spacing: .12em; text-transform: uppercase; align-self: center; }
"""


def loopback(host: str) -> bool:
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return host == "localhost"


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--demo", action="store_true", help="required: serve without Access or login, fake data only")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=8099)
    ap.add_argument("--users", type=int, default=120)
    ap.add_argument("--seed", type=int, default=None, help="same world on every start")
    args = ap.parse_args()
    if not (args.demo or os.environ.get("YO_ADMIN_DEMO") == "1"):
        sys.exit("demo.py serves the admin without authentication: pass --demo (or YO_ADMIN_DEMO=1) to start it")
    if not loopback(args.host):
        sys.exit(f"refusing {args.host}: the demo listens on a loopback address only")

    rng = random.Random(args.seed)
    now = int(time.time() * 1000)
    state = tempfile.mkdtemp(prefix="yo-admin-demo-")
    atexit.register(shutil.rmtree, state, True)
    paths = {"live": os.path.join(state, "hub.sqlite"), "dev": os.path.join(state, "hub-dev.sqlite")}
    live = make_hub(paths["live"], rng, now, args.users, True)
    make_hub(paths["dev"], rng, now, 9, False)

    def write(name, data):
        with open(os.path.join(state, name), "w") as f:
            f.write(data if isinstance(data, str) else json.dumps(data))

    os.makedirs(os.path.join(state, "replies"))
    write("hub.env", "YO_MIN_APP=" + ios(now - 7 * DAY) + "\n")
    write("hub-dev.env", "")
    write("version-yogpt", "a1b2c3d " + time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime((now - 26 * HOUR + 120_000) / 1000)) + "\n")
    os.makedirs(os.path.join(state, "yogpt", "live"))
    write("yogpt/live/stats.json", {"contacts": 87})
    hub_hex = lambda b: b.hex().upper()  # noqa: E731
    write("support-marks.json", {"stars": [f"live:u:{hub_hex(live['threads'][1])}", f"live:u:{hub_hex(live['threads'][4])}",
                                           f"live:t:{live['todo_ids']['star']}"],
                                 "todo_user": {f"live:{live['todo_ids']['photo']}": hub_hex(live["photo_user"] or live["threads"][1])}})
    write("feedback-done.json", [])
    write("status-bot.json", {"every_minutes": 60, "baseline_users": args.users - 14})
    today = now // DAY
    write("active-days.json", {"live": {str(d): max(3, int(args.users * (0.18 + 0.32 * (1 - (today - d) / 30)) * rng.uniform(0.8, 1.15)) - (today - d))
                                        for d in range(today - 29, today)},
                               "dev": {str(d): rng.randint(1, 4) for d in range(today - 20, today)}})
    hours = {str(h): n for h, n in live["media_hours"].items()}
    for h in range(now // HOUR - 47, min(live["media_hours"])):
        hours[str(h)] = max(0, int(rng.gauss(5, 3)))
    write("media-hours.json", {"live": {"day": today, "count": sum(live["media_hours"].values()), "at": now, "hours": hours}})

    env = {
        # admin.py's required settings: unusable values, never a real secret; authentication is replaced below.
        "ADMIN_USER": "demo", "ADMIN_PASSWORD_HASH": "disabled$0$0$0$AA==$AA==", "ADMIN_SESSION_KEY": os.urandom(32).hex(),
        "ACCESS_TEAM": "demo.invalid", "ACCESS_AUD": "demo", "ADMIN_ORIGIN": f"http://{args.host}:{args.port}",
        "ADMIN_LISTEN": args.host, "ADMIN_PORT": str(args.port), "YO_ADMIN_TOKEN": "",
        "YO_HUB_URL": "http://demo.invalid", "YO_HUB_DEV_URL": "http://demo.invalid",
        "STATE_DIRECTORY": state, "ADMIN_USERS_FILE": os.path.join(state, "users.json"),
        "ADMIN_REQUEST_FILE": os.path.join(state, "want"), "STATUS_LAST": os.path.join(state, "status-last.json"),
        "YOGPT_REPLIES": os.path.join(state, "replies"), "YO_HUB_DB": paths["live"], "YO_HUB_DEV_DB": paths["dev"],
        "YO_HUB_ENV_FILE": os.path.join(state, "hub.env"), "YO_HUB_DEV_ENV_FILE": os.path.join(state, "hub-dev.env"),
        "YOGPT_STATS_PATTERN": os.path.join(state, "yogpt", "{}", "stats.json"),
        "YOGPT_VERSION_FILE": os.path.join(state, "version-yogpt"), "YO_HUB_VERSION_FILE": os.path.join(state, "version"),
        "YO_ADMIN_INTERNAL_USERS": live["internal"].hex(),
    }
    os.environ.update(env)

    # Nothing in this process may reach the network: every hub call is answered by FakeHubs.
    def no_network(*a, **k):
        raise OSError("demo: no network")
    urllib.request.urlopen = no_network

    sys.path.insert(0, ADMIN_DIR)
    import users as collector_module  # noqa: E402
    import admin  # noqa: E402

    server, lock = FakeServer(rng), threading.Lock()

    def collect(tick=False):
        """users.json from the fake databases with the real collector; on a tick the live hub moves a little."""
        with lock:
            t = int(time.time() * 1000)
            if tick:
                with sqlite3.connect(paths["live"]) as db:
                    db.execute("INSERT INTO deliveries_hourly VALUES (?,?) ON CONFLICT(hour) DO UPDATE SET n=n+?",
                               (t // HOUR, rng.randint(0, 6), rng.randint(0, 6)))
                    db.execute("UPDATE devices SET last_seen_at=? WHERE id IN (SELECT id FROM devices ORDER BY random() LIMIT ?)",
                               (t, rng.randint(0, 2)))
            out = {"at": t, "updates": server.updates()}
            for name, path in paths.items():
                out[name] = collector_module.hub(name, path)
            out["internal_users"] = collector_module.internal_users()
            out["app_builds"] = collector_module.app_builds(out, set(out["internal_users"]))
            tmp = env["ADMIN_USERS_FILE"] + ".new"
            with open(tmp, "w") as f:
                json.dump(out, f, separators=(",", ":"))
            os.replace(tmp, env["ADMIN_USERS_FILE"])
            live_out = out["live"]
            write("status-last.json", {"at": int(time.time()) - 17 * 60, "users": live_out["total"], "since": live_out["total"] - (args.users - 14),
                                       "up": 4, "total": 4, "load": 0.42, "crashes": live_out["diagnostics_open"], "sent": True})

    def ticker():
        while True:
            time.sleep(10)
            try:
                collect(tick=True)
            except Exception as e:  # noqa: BLE001
                print(f"demo: collect: {e}", flush=True)

    collect()
    threading.Thread(target=ticker, daemon=True).start()

    hubs = FakeHubs(paths, collect)
    admin.support_request = hubs.support
    admin.todo_add = hubs.todo
    admin.reserved_request = hubs.reserved
    admin.diagnostics_request = hubs.diagnostics
    admin.stats = lambda: server.stats(admin.reserved_names())
    # No Access and no login: everyone who reaches 127.0.0.1 is "demo". POSTs only from this page's own origin.
    origins = {f"http://{args.host}:{args.port}", f"http://localhost:{args.port}", f"http://127.0.0.1:{args.port}"}
    admin.access_user = lambda token: "demo@localhost"
    admin.session_valid = lambda cookie, who: True
    admin.Admin.same_origin = lambda self: self.headers.get("Origin") in origins | {None}

    def snapshot(timeout=3):
        with open(env["ADMIN_USERS_FILE"]) as f:
            return {**json.load(f), "stale": False}
    admin.collector.snapshot = snapshot
    page = admin.page

    def demo_page(name):
        if name == "demo.css":
            return BADGE_CSS.encode()
        body = page(name)
        if name == "dashboard.html":
            body = body.replace(b'<link rel="stylesheet" href="/app.css">', b'<link rel="stylesheet" href="/app.css">\n<link rel="stylesheet" href="/demo.css">')
            body = body.replace(b'<span class="logo">Yo</span>', b'<span class="logo">Yo</span><span class="demo-badge" title="made-up data, not the live hub">Demo</span>')
            body = body.replace(b"<title>Yo admin</title>", b"<title>Yo admin (demo)</title>")
        return body
    admin.page = demo_page
    admin.FILES["/demo.css"] = "text/css; charset=utf-8"
    admin.HEADERS.pop("Strict-Transport-Security", None)  # plain HTTP on loopback: no HSTS

    print(f"yo-admin demo: http://{args.host}:{args.port}  (state {state}, fake data only)", flush=True)
    admin.ThreadingHTTPServer.daemon_threads = True
    httpd = admin.ThreadingHTTPServer((args.host, args.port), admin.Admin)
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
