"""The kept active-user counts and server history; the collector's numbers."""
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

from test_feedback import load



class MarksTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        with patch.dict(os.environ, {"ADMIN_USER": "test", "ADMIN_PASSWORD_HASH": "unused", "ADMIN_SESSION_KEY": "00",
                                     "ACCESS_TEAM": "invalid", "ACCESS_AUD": "test", "STATE_DIRECTORY": self.temp.name}):
            self.admin = load("admin")

    def test_active_days_keep_the_highest_count_per_day(self):
        admin, day = self.admin, 20400
        now = day * 86400000 + 5000
        out = admin.active_days({"at": now, "live": {"active_day": 3}, "dev": {"error": "x"}}, now)
        self.assertEqual(out["live"]["active_days"], {str(day): 3})
        self.assertEqual(out["dev"]["active_days"], {})
        admin.active_days({"at": now, "live": {"active_day": 2}}, now)  # lower later in the day: kept at 3
        out = admin.active_days({"at": now + 86400000, "live": {"active_day": 1}}, now + 86400000)
        self.assertEqual(out["live"]["active_days"], {str(day): 3, str(day + 1): 1})
        # An old snapshot (from yesterday) never writes today's value; 60 days later the old ones are gone.
        out = admin.active_days({"at": now, "live": {"active_day": 9}}, now + 86400000)
        self.assertEqual(out["live"]["active_days"][str(day + 1)], 1)
        later = now + 61 * 86400000
        self.assertEqual(admin.active_days({"at": later, "live": {"active_day": 4}}, later)["live"]["active_days"], {str(day + 61): 4})
        self.assertEqual(admin.active_days({"stale": True})["stale"], True)

    def test_server_history_a_point_a_minute_the_last_hour(self):
        admin = self.admin
        admin.HISTORY.clear()
        out = {"cpu": {"percent": 12.0}, "memory": {"total": 100, "available": 60}, "disk": {"total": 200, "used": 50},
               "network": {"rx_per_s": 5, "tx_per_s": 7}}
        admin.remember(out, 1000.0)
        admin.remember(out, 1030.0)  # within the minute: not again
        self.assertEqual(admin.remember(out, 1000.0 + 60), [[1_000_000, 12.0, 40.0, 25.0, 5, 7], [1_060_000, 12.0, 40.0, 25.0, 5, 7]])
        for k in range(100):
            admin.remember(out, 2000.0 + 60 * k)
        self.assertEqual(len(admin.HISTORY), 60)


class UsersTabCollectorTests(unittest.TestCase):
    def test_signups_30_days_and_active_since_midnight(self):
        users = load("users")
        now = int(users.time.time() * 1000)
        midnight = now - now % users.DAY_MS
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "hub.sqlite")
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE users(id BLOB, created_at INTEGER)")
                db.execute("CREATE TABLE devices(id INTEGER, user_id BLOB, last_seen_at INTEGER)")
                db.executemany("INSERT INTO users VALUES(?,?)", [(b"\x01", midnight + 1), (b"\x02", midnight - 20 * users.DAY_MS), (b"\x03", midnight - 40 * users.DAY_MS)])
                db.executemany("INSERT INTO devices VALUES(?,?,?)", [(1, b"\x01", midnight), (2, b"\x01", midnight + 5), (3, b"\x02", midnight - 1)])
            snapshot = users.hub("live", path)
            self.assertEqual(len(snapshot["signups"]), 30)
            self.assertEqual(snapshot["signups"][-1], 1)
            self.assertEqual(snapshot["signups"][-21], 1)
            self.assertEqual(sum(snapshot["signups"]), 2)
            self.assertEqual(snapshot["active_day"], 1)
            self.assertEqual(len(snapshot["signups_hours"]), 24)
            self.assertIsNone(snapshot["events"])  # an older hub without the tables
            self.assertIsNone(snapshot["groups"])
            self.assertEqual(sum(snapshot["signups_hours"]), 1)
            if now // 3600000 == (midnight + 1) // 3600000:
                self.assertEqual(snapshot["signups_hours"][-1], 1)

    def test_events_per_hour_day_and_today_by_kind(self):
        users = load("users")
        now = int(users.time.time() * 1000)
        minute, hour, day = now // 60000, now // 3600000, now // users.DAY_MS
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "hub.sqlite")
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE users(id BLOB, created_at INTEGER)")
                db.execute("CREATE TABLE devices(id INTEGER, user_id BLOB, last_seen_at INTEGER)")
                db.execute("CREATE TABLE events(kind TEXT, minute INTEGER, n INTEGER, PRIMARY KEY(kind, minute))")
                db.executemany("INSERT INTO events VALUES(?,?,?)", [
                    ("yo-in", minute, 3), ("yo-in", minute - 1, 2) if minute % 60 else ("yo-in", minute + 1, 2),
                    ("yo-out", minute - 60, 4), ("group-create", minute - 2 * 24 * 60, 1),
                    ("media-in", (day - 20) * 24 * 60, 9),  # older than 14 days: only in the total
                    ("media-in-bytes", minute, 3_000_000), ("media-in-bytes", (day - 20) * 24 * 60, 9_000_000)])
            e = users.hub("live", path)["events"]
            self.assertEqual(e["yo-in"]["hours"][-1], 5)
            self.assertEqual(e["yo-out"]["hours"][-2], 4)
            self.assertEqual(len(e["yo-in"]["hours"]), 24)
            self.assertEqual(e["group-create"]["days"], [0] * 11 + [1, 0, 0])
            media = [0] * 365
            media[-21] = 9  # 20 days ago: in the year, not in the 14 days
            self.assertEqual(e["media-in"], {"hours": [0] * 24, "days": [0] * 14, "today": 0, "total": 9, "year": media})
            self.assertEqual(len(e["yo-in-bot"]["year"]), 365)
            # The bytes uploaded, kept for good like the counts: the Media row's hours, days and total.
            self.assertEqual(e["media-in-bytes"]["hours"][-1], 3_000_000)
            self.assertEqual(e["media-in-bytes"]["today"], 3_000_000)
            self.assertEqual(e["media-in-bytes"]["total"], 12_000_000)
            self.assertEqual(e["yo-in"]["total"], 5)
            self.assertEqual(e["media-out"]["total"], 0)
            self.assertEqual(e["media-out"]["today"], 0)
            self.assertEqual(e["yo-in"]["today"], 5)
            self.assertEqual(e["yo-out"]["today"], 4 if (minute - 60) // 1440 == day else 0)


if __name__ == "__main__":
    unittest.main()
