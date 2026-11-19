#!/usr/bin/env python3
"""STATUS: the server's numbers as a picture, sent as a Yo to one user at a fixed rhythm. Runs on the server itself:
users, sign-ups since a baseline and open crash reports from the live hub's database (read-only), services from
systemd, the load from /proc. Rhythm and recipient come from the admin page's config, read again before every slot:

  {"every_minutes": 30, "to": "NAME", "baseline_users": 30}     every_minutes: 0 (off), 15, 30, 60, 120, 1440 (daily)

Slots are whole multiples of every_minutes from Berlin midnight; daily at 09:00 Berlin. After each send it writes
the numbers (no names) to STATUS_LAST for the admin page. Rendering needs Pillow (python3-pil) and the Montserrat
Bold next to this file.

  YO_STATE=… YO_HUB=… status.py [--once]     (once before: yo register NAME)
"""

import argparse
import datetime
import json
import os
import re
import sqlite3
import subprocess
import sys
import tempfile
import time
from zoneinfo import ZoneInfo

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import yocli  # noqa: E402

CONFIG = os.environ.get("STATUS_CONFIG", "/var/lib/yo-admin/status-bot.json")
LAST = os.environ.get("STATUS_LAST", os.path.join(os.environ.get("RUNTIME_DIRECTORY", "."), "last.json"))
HUB_DB = os.environ.get("STATUS_HUB_DB", "/var/lib/private/yo/yo-hub.sqlite")
SERVICES = os.environ.get("STATUS_SERVICES", "yo-hub yo-hub-dev yo-ping@live yo-yogpt yo-admin cloudflared").split()
FONT = os.path.join(HERE, "Montserrat-Bold.ttf")
BERLIN = ZoneInfo("Europe/Berlin")
EVERY = (0, 15, 30, 60, 120, 1440)
DAILY_AT = 9 * 60  # minutes after Berlin midnight
NAME = re.compile(r"@?[A-Za-z0-9][A-Za-z0-9._-]{0,31}")


def config(path=CONFIG):
    """The admin's config, checked; off (every 0) when missing, unreadable or malformed."""
    try:
        with open(path) as f:
            data = json.load(f)
        every, to, base = data.get("every_minutes"), data.get("to"), data.get("baseline_users", 0)
        if every not in EVERY or type(every) is not int or type(base) is not int or base < 0:
            raise ValueError()
        if every and not (isinstance(to, str) and NAME.fullmatch(to)):
            raise ValueError()
        return {"every_minutes": every, "to": to, "baseline_users": base}
    except (OSError, ValueError, AttributeError, TypeError):
        return {"every_minutes": 0, "to": None, "baseline_users": 0}


def next_slot(now, every):
    """The next slot after `now` (an aware datetime), in Berlin time; None when off."""
    if not every:
        return None
    local = now.astimezone(BERLIN)
    midnight = local.replace(hour=0, minute=0, second=0, microsecond=0)
    minutes = (local - midnight).total_seconds() / 60
    if every == 1440:
        day = midnight if minutes < DAILY_AT else midnight + datetime.timedelta(days=1)
        return day.replace(hour=DAILY_AT // 60, minute=DAILY_AT % 60)
    step = int(minutes // every) + 1
    slot = midnight + datetime.timedelta(minutes=step * every)
    return slot  # wall-clock arithmetic: through a DST change the next whole slot by the Berlin clock


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
        yocli.log(f"database: {type(error).__name__}")
    try:
        done = subprocess.run(["systemctl", "is-active", *services], capture_output=True, text=True, timeout=10)
        out["up"] = done.stdout.split().count("active")
    except (OSError, subprocess.TimeoutExpired) as error:
        yocli.log(f"systemctl: {type(error).__name__}")
    try:
        with open("/proc/loadavg") as f:
            out["load"] = f.read().split()[0]
    except (OSError, IndexError):
        pass
    return out


def rows(n, base):
    """The card's four rows, as (label, value)."""
    q = lambda v: "?" if v is None else str(v)  # noqa: E731
    since = "?" if n["users"] is None else f"{n['users'] - base:+d}"
    return [("USERS", q(n["users"])), ("SINCE THE POST", since),
            ("SERVICES", f"{q(n['up'])}/{n['total']} up"), ("LOAD · CRASHES", f"{q(n['load'])} · {q(n['crashes'])}")]


def render(path, n, base, when):
    """A 1080×1080 JPEG: amethyst top with "Yo" and the time, four Flat UI rows below."""
    from PIL import Image, ImageDraw, ImageFont

    image = Image.new("RGB", (1080, 1080), "#9B59B6")
    draw = ImageDraw.Draw(image)
    draw.text((80, 70), "Yo", font=ImageFont.truetype(FONT, 120), fill="white")
    draw.text((80, 214), f"Server · {when:%H:%M} Berlin", font=ImageFont.truetype(FONT, 40), fill="#ecdff2")
    font = ImageFont.truetype(FONT, 56)
    for i, ((label, value), colour) in enumerate(zip(rows(n, base), ("#1ABC9C", "#2ECC71", "#3498DB", "#34495E"))):
        top = 1080 - (4 - i) * 150
        draw.rectangle((0, top, 1080, top + 150), fill=colour)
        draw.text((80, top + 75), label, font=font, fill="white", anchor="lm")
        draw.text((1000, top + 75), value, font=font, fill="white", anchor="rm")
    image.save(path, "JPEG", quality=88)


def report(cfg):
    when = datetime.datetime.now(BERLIN)
    n = numbers()
    last = {**n, "at": int(time.time()), "since": None if n["users"] is None else n["users"] - cfg["baseline_users"],
            "sent": False}
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "status.jpg")
        try:
            render(path, n, cfg["baseline_users"], when)
            yocli.yo("send", cfg["to"].lstrip("@"), "--photo", path)
            last["sent"] = True
            yocli.log("sent")
        except yocli.Failed as failed:
            last["error"] = yocli.reason(failed)
            yocli.log(f"not sent: {last['error']}")
        except (OSError, ImportError) as error:
            last["error"] = type(error).__name__
            yocli.log(f"not rendered: {last['error']}")
    try:
        tmp = LAST + ".new"
        with open(tmp, "w") as f:
            json.dump(last, f)
        os.chmod(tmp, 0o644)
        os.replace(tmp, LAST)
    except OSError as error:
        yocli.log(f"last report not kept: {type(error).__name__}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--once", action="store_true", help="one report now (to the configured user), then end")
    args = parser.parse_args()
    yocli.registered()
    if args.once:
        cfg = config()
        if not cfg["to"]:
            sys.exit("status: no recipient configured")
        return report(cfg)
    shown = None
    while True:
        cfg = config()
        slot = next_slot(datetime.datetime.now(BERLIN), cfg["every_minutes"])
        if (cfg["every_minutes"], cfg["to"]) != shown:
            shown = (cfg["every_minutes"], cfg["to"])
            yocli.log(f"every {cfg['every_minutes']} min" if slot else "off")
        # Asleep in steps of a minute at most: a changed config counts from the next step on.
        while True:
            wait = None if slot is None else slot.timestamp() - time.time()
            if wait is not None and wait <= 0:
                report(cfg)
                break
            time.sleep(60 if wait is None else min(wait, 60))
            fresh = config()
            if fresh != cfg:
                break


if __name__ == "__main__":
    main()
