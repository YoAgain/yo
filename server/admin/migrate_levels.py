#!/usr/bin/env python3
"""Once, after the hub keeps its own levels: the admin's JSON histories of before into the hub's `levels`, so none of
it is lost. Usage: migrate_levels.py HUB DB STATE_DIR, HUB "live" or "dev" (the key in the JSON files), DB the hub's
database (/var/lib/yo/yo-hub.sqlite, /var/lib/yo-dev/yo-hub.sqlite), STATE_DIR the admin's (with active-days.json,
queue.json, problems-open.json). INSERT OR IGNORE: what the hub sampled itself wins, a second run adds nothing.
The hub may run meanwhile (WAL, one short transaction). A sample at an hour's start is the level at the end of the hour
before: a value of hour h goes to h + 1, a day's to the next midnight."""
import json
import os
import sqlite3
import sys
import time

H, D = 3600000, 24


def load(path):
    try:
        with open(path) as f:
            data = json.load(f)
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def rows(hub, state, now):
    """(kind, hour, value) from the JSON files of `state` for `hub`; nothing past the current hour."""
    out = []
    for day, n in (load(os.path.join(state, "active-days.json")).get(hub) or {}).items():
        if day.isdigit() and isinstance(n, int):
            out.append(("active-24h", (int(day) + 1) * D, n))
    queue = load(os.path.join(state, "queue.json")).get(hub) or {}
    for slots, at in ((queue.get("hours") or {}, lambda k: k + 1), (queue.get("days") or {}, lambda k: (k + 1) * D)):
        for key, value in slots.items():
            if key.isdigit() and isinstance(value, list) and len(value) == 2:
                out += [("queue-envelopes", at(int(key)), value[0]), ("queue-media-bytes", at(int(key)), value[1])]
    for point in load(os.path.join(state, "problems-open.json")).get(hub) or []:
        if isinstance(point, dict) and isinstance(point.get("t"), int):
            hour = point["t"] // H + 1
            out += [("open-errors", hour, point.get("errors_open", 0)), ("open-crashes", hour, point.get("crashes_open", 0))]
    return [(k, h, int(v or 0)) for k, h, v in out if h <= now // H]


def main():
    hub, path, state = sys.argv[1:4]
    found = rows(hub, state, int(time.time() * 1000))
    db = sqlite3.connect(path, timeout=30)
    try:
        if not db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='levels'").fetchone():
            sys.exit(f"{path}: no levels table (start the new hub first)")
        with db:
            before = db.total_changes
            db.executemany("INSERT OR IGNORE INTO levels (kind, hour, value) VALUES (?, ?, ?)", found)
            added = db.total_changes - before
    finally:
        db.close()
    print(f"{hub}: {added} of {len(found)} levels added")


if __name__ == "__main__":
    main()
