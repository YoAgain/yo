#!/usr/bin/env python3
"""Owner-run, offline only: import old dashboard done flags after the hub schema migration.
Usage: python3 server/admin/migrate_support_done.py HUB DB FEEDBACK_DONE_JSON
Stop that hub and back up its DB and admin state first. Safe to repeat; leaves the JSON untouched.
"""
import json
import sqlite3
import sys
import time


def migrate(hub, path, done_path):
    if hub not in ("live", "dev"):
        raise ValueError("hub must be live or dev")
    with open(done_path) as source:
        keys = json.load(source)
    now = int(time.time() * 1000)
    # mode=rw refuses to accidentally create a database at the wrong path.
    with sqlite3.connect("file:" + path + "?mode=rw", uri=True) as db:
        for key in keys:
            prefix, _, number = key.partition(":")
            if prefix != hub or not number.isdecimal():
                continue
            fid = int(number)
            db.execute("UPDATE todos SET done_at=? WHERE id=? AND done_at IS NULL", (now, fid))
            db.execute("UPDATE support_messages SET read_at=? WHERE id=? AND from_team=0 AND read_at IS NULL", (now, fid))


if __name__ == "__main__":
    migrate(*sys.argv[1:])
