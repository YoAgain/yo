#!/usr/bin/env python3
"""The collector: run as a small root service with read access to the hub's database, never near a Yo. Writes numbers
only (no names) to /run/yo-status/numbers.json, with the admin's rhythm, for the STATUS script."""
import json
import os
import sqlite3
import subprocess
import sys

CONFIG = os.environ.get("STATUS_CONFIG", "/var/lib/yo-admin/status-bot.json")
HUB_DB = os.environ.get("STATUS_HUB_DB", "/var/lib/private/yo/yo-hub.sqlite")
SERVICES = os.environ.get("STATUS_SERVICES", "yo-hub yo-hub-dev yo-service@ping yo-service@yogpt yo-admin cloudflared").split()
EVERY = (0, 15, 30, 60, 120, 1440)
OUT = os.environ.get("STATUS_NUMBERS", "/run/yo-status/numbers.json")


def log(what):
    print(what, file=sys.stderr, flush=True)


def config(path=CONFIG):
    """The admin's config, checked; off (every 0) when missing, unreadable or malformed."""
    try:
        with open(path) as f:
            data = json.load(f)
        every, base = data.get("every_minutes"), data.get("baseline_users", 0)
        if every not in EVERY or type(every) is not int or type(base) is not int or base < 0:
            raise ValueError()
        return {"every_minutes": every, "baseline_users": base}
    except (OSError, ValueError, AttributeError, TypeError):
        return {"every_minutes": 0, "baseline_users": 0}


def numbers(db=HUB_DB, services=SERVICES):
    """{users, crashes, up, total, load}: None for what could not be read."""
    out = {"users": None, "crashes": None, "up": None, "total": len(services), "load": None}
    try:
        with sqlite3.connect(f"file:{db}?mode=ro", uri=True, timeout=5) as c:
            out["users"] = c.execute("SELECT count(*) FROM users").fetchone()[0]
            if c.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='diagnostics'").fetchone():
                out["crashes"] = c.execute("SELECT count(*) FROM diagnostics WHERE done=0").fetchone()[0]
            else:
                out["crashes"] = 0
        c.close()
    except sqlite3.Error as error:
        log(f"database: {type(error).__name__}")
    try:
        done = subprocess.run(["systemctl", "is-active", *services], capture_output=True, text=True, timeout=10)
        out["up"] = done.stdout.split().count("active")
    except (OSError, subprocess.TimeoutExpired) as error:
        log(f"systemctl: {type(error).__name__}")
    try:
        with open("/proc/loadavg") as f:
            out["load"] = f.read().split()[0]
    except (OSError, IndexError):
        pass
    return out


if __name__ == "__main__":
    with open(OUT + ".new", "w") as f:
        json.dump({**numbers(), **config()}, f)
    os.chmod(OUT + ".new", 0o644)
    os.replace(OUT + ".new", OUT)
