"""The kept active-user counts and server history; the collector's numbers."""
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

from test_help import load



class MarksTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        with patch.dict(os.environ, {"ADMIN_USER": "test", "ADMIN_PASSWORD_HASH": "unused", "ADMIN_SESSION_KEY": "00",
                                     "ACCESS_TEAM": "invalid", "ACCESS_AUD": "test", "STATE_DIRECTORY": self.temp.name}):
            self.admin = load("admin")

    def test_histories_come_from_the_hubs_levels(self):
        users, hour, day = load("users"), 3600000, 20400
        db = sqlite3.connect(":memory:")
        db.execute("CREATE TABLE stat (metric TEXT, key TEXT DEFAULT '', hour INTEGER, value INTEGER, PRIMARY KEY (metric,key,hour))")
        h0 = day * 24  # midnight starting `day`: the levels at the end of day - 1
        db.executemany("INSERT INTO stat(metric,hour,value) VALUES (?, ?, ?)", [
            ("active-24h", h0 - 24, 4), ("active-24h", h0, 5), ("active-24h", h0 + 3, 9),
            ("queue-envelopes", h0, 2), ("queue-media-bytes", h0, 70), ("queue-envelopes", h0 + 3, 1),
            ("open-errors", h0, 8), ("open-crashes", h0, 2), ("open-errors", h0 + 3, 3),
            ("build:ios/202611191626", h0, 4), ("build:unknown", h0, 1), ("build:ios/202611191626", h0 + 3, 6),
            ("build:android/202611180900", h0 + 3, 0), ("phones-24h", h0 + 3, 10), ("problem-installs-24h", h0 + 3, 3), ("phones-24h:ios", h0 + 3, 6), ("problem-installs-24h:ios", h0 + 3, 1)])
        now = (h0 + 3) * hour + 600_000
        out = users.levels(db, now, 6, (1, 900))
        self.assertEqual(out["active_days"], {str(day - 2): 4, str(day - 1): 5}, "today's from active_day")
        self.assertEqual(out["queue_hours"], {str(h0): [2, 70], str(h0 + 3): [6, 900]}, "this hour: now")
        self.assertEqual(out["queue_days"], {str(day - 1): [2, 70], str(day): [6, 900]})
        self.assertEqual(out["open_report_history"], [
            {"t": h0 * hour - 1, "errors_open": 8, "crashes_open": 2},
            {"t": (h0 + 3) * hour - 1, "errors_open": 3, "crashes_open": 0}])
        self.assertEqual(users.levels(db, now + 48 * hour)["queue_hours"], {})
        self.assertEqual(out["build_levels"], {str(h0): {"ios/202611191626": 4, "unknown": 1},
                                               str(h0 + 3): {"ios/202611191626": 6}}, "devices per build, none at 0")
        self.assertEqual(out["error_free_levels"], {str(h0 + 3): [10, 3, 6, 1, None, None]})

    def test_version_days_keep_the_latest_per_day_for_good(self):
        admin, day = self.admin, 20400
        now = day * 86400000 + 5000
        v = lambda c, u, m, n: {"current": c, "update": u, "must": m, "unknown": n}  # noqa: E731
        out = admin.version_days({"at": now, "live": {"versions_day": v(5, 2, 1, 0)}, "dev": {"error": "x"}}, now)
        self.assertEqual(out["live"]["version_days"], {str(day): [5, 2, 1, 0]})
        self.assertEqual(out["dev"]["version_days"], {})
        admin.version_days({"at": now, "live": {"versions_day": v(9, 2, 1, 1)}}, now)  # later the same day: replaced
        later = now + 400 * 86400000
        out = admin.version_days({"at": later, "live": {"versions_day": v(1, 0, 0, 0)}}, later)
        self.assertEqual(out["live"]["version_days"], {str(day): [9, 2, 1, 1], str(day + 400): [1, 0, 0, 0]})
        # An old snapshot (from yesterday) never writes today's.
        out = admin.version_days({"at": now, "live": {"versions_day": v(7, 7, 7, 7)}}, later)
        self.assertNotIn(str(day), [k for k, r in out["live"]["version_days"].items() if r == [7, 7, 7, 7]])

    def test_build_counts_per_day_for_good_and_per_hour_for_48(self):
        admin, day = self.admin, 20400
        now = day * 86400000 + 5000
        apps = [["ios/202611191626", 4], ["ios/202611181859", 2], ["cli/7", 3], ["android/4", 1], ["bad", "x"]]
        out = admin.version_days({"at": now, "live": {"apps_today": apps}, "dev": {"error": "x"}}, now)
        counts = {"ios/202611191626": 4, "ios/202611181859": 2, "android/4": 1}  # the CLI left out, counts only
        self.assertEqual(out["live"]["build_days"], {str(day): counts})
        self.assertEqual(out["live"]["build_hours"], {str(now // 3600000): counts})
        self.assertEqual(out["dev"]["build_days"], {})
        # An hour later: a second hour, the day replaced; nothing seen (an empty list) keeps what was kept.
        h2 = now + 3600000
        out = admin.version_days({"at": h2, "live": {"apps_today": [["ios/202611191626", 6]]}}, h2)
        self.assertEqual(out["live"]["build_days"], {str(day): {"ios/202611191626": 6}})
        self.assertEqual(len(out["live"]["build_hours"]), 2)
        out = admin.version_days({"at": h2, "live": {"apps_today": []}}, h2)
        self.assertEqual(out["live"]["build_days"][str(day)], {"ios/202611191626": 6})
        # 50 hours on, the hours before are gone; the days stay for good, an old snapshot never writes today's.
        later = now + 50 * 3600000
        out = admin.version_days({"at": now, "live": {"apps_today": [["ios/202611201000", 1]]}}, later)
        self.assertEqual(out["live"]["build_hours"], {str(later // 3600000): {"ios/202611201000": 1}})
        self.assertEqual(set(out["live"]["build_days"]), {str(day)})
        self.assertEqual(out["live"]["version_days"], {})

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

    def test_server_history_a_quarter_hour_point_the_last_day(self):
        admin = self.admin
        admin.HISTORY.clear()
        admin.HISTORY_DAY.clear()
        out = lambda cpu: {"cpu": {"percent": cpu}, "memory": {"total": 100, "available": 60}, "disk": {"total": 200, "used": 50},  # noqa: E731
                           "network": {"rx_per_s": 5, "tx_per_s": 7}}
        admin.remember(out(10.0), 900.0)
        admin.remember(out(20.0), 960.0)  # the same quarter: averaged
        self.assertEqual(admin.HISTORY_DAY, [[900_000, 15.0, 40.0, 25.0, 5, 7, 2]])
        for k in range(30 * 60):
            admin.remember(out(30.0), 2000.0 + 60 * k)
        self.assertEqual(len(admin.HISTORY_DAY), 96)
        self.assertEqual(admin.HISTORY_DAY[-1][1], 30.0)


class UsersTabCollectorTests(unittest.TestCase):
    def test_signups_30_days_and_active_since_midnight(self):
        users = load("users")
        now = int(users.time.time() * 1000)
        midnight = now - now % users.DAY_MS
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "hub.sqlite")
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE account(id BLOB, created_at INTEGER)")
                db.execute("CREATE TABLE account_device(id INTEGER, user_id BLOB, last_seen_at INTEGER)")
                db.executemany("INSERT INTO account VALUES(?,?)", [(b"\x01", midnight + 1), (b"\x02", midnight - 20 * users.DAY_MS), (b"\x03", midnight - 40 * users.DAY_MS)])
                db.executemany("INSERT INTO account_device VALUES(?,?,?)", [(1, b"\x01", midnight), (2, b"\x01", midnight + 5), (3, b"\x02", midnight - 1)])
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

    def test_retention_and_active_month(self):
        users = load("users")
        now, day = int(users.time.time() * 1000), users.DAY_MS
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "hub.sqlite")
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE account(id BLOB, created_at INTEGER)")
                db.execute("CREATE TABLE account_device(id INTEGER, user_id BLOB, last_seen_at INTEGER, app TEXT)")
                # Signed up 40 days ago: one seen 35 days later, one the day after, one never again; a bot left out;
                # one 3 days ago, seen 2 days later; one 100 days ago (out of the cohorts, but seen this month).
                rows = [(1, now - 40 * day, now - 5 * day, "ios/1"), (2, now - 40 * day, now - 39 * day + 1, "ios/1"),
                        (3, now - 40 * day, now - 40 * day, "android/1"), (4, now - 40 * day, now, "yogpt/1"),
                        (5, now - 3 * day, now - day, "ios/1"), (6, now - 100 * day, now - 2 * day, "ios/1")]
                for i, created, seen, app in rows:
                    db.execute("INSERT INTO account VALUES(?,?)", (bytes([i]), created))
                    db.execute("INSERT INTO account_device VALUES(?,?,?,?)", (i, bytes([i]), seen, app))
            snapshot = users.hub("live", path)
            self.assertEqual(snapshot["retention"], {"d1": [3, 4], "d7": [1, 3], "d30": [1, 3]})
            self.assertEqual(snapshot["active_month"], 4)  # 1, the bot, 5 and 6

    def test_events_per_hour_day_and_today_by_kind(self):
        users = load("users")
        now = int(users.time.time() * 1000)
        minute, hour, day = now // 60000, now // 3600000, now // users.DAY_MS
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "hub.sqlite")
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE account(id BLOB, created_at INTEGER)")
                db.execute("CREATE TABLE account_device(id INTEGER, user_id BLOB, last_seen_at INTEGER)")
                db.execute("CREATE TABLE stat_minute(kind TEXT, minute INTEGER, n INTEGER, PRIMARY KEY(kind, minute))")
                db.executemany("INSERT INTO stat_minute VALUES(?,?,?)", [
                    ("yo-in", minute, 3), ("yo-in", minute - 1, 2) if minute % 60 else ("yo-in", minute + 1, 2),
                    ("yo-out", minute - 60, 4), ("group-create", minute - 2 * 24 * 60, 1),
                    ("media-in", (day - 20) * 24 * 60, 9),  # older than 14 days: only in the total
                    ("media-in-bytes", minute, 3_000_000), ("media-in-bytes", (day - 20) * 24 * 60, 9_000_000),
                    ("bot-yo", minute, 7), ("bot-media:YoGPT", minute, 2)])
            e = users.hub("live", path)["events"]
            self.assertEqual(e["yo-in"]["hours"][-1], 5)
            self.assertEqual(e["yo-out"]["hours"][-2], 4)
            self.assertEqual(len(e["yo-in"]["hours"]), 24)
            self.assertEqual(e["group-create"]["days"], [0] * 11 + [1, 0, 0])
            media = [0] * 365
            media[-21] = 9  # 20 days ago: in the year, not in the 14 days
            self.assertEqual(e["media-in"], {"hours": [0] * 24, "days": [0] * 14, "today": 0, "minutes": [0] * 60, "total": 9, "year": media})
            self.assertEqual(len(e["yo-in"]["minutes"]), 60)
            self.assertEqual(e["yo-in"]["minutes"][-1], 3)  # the current minute last
            self.assertEqual(len(e["yo-in-bot"]["year"]), 365)
            # The bytes uploaded, kept for good like the counts: the Media row's hours, days and total.
            self.assertEqual(e["media-in-bytes"]["hours"][-1], 3_000_000)
            self.assertEqual(e["media-in-bytes"]["today"], 3_000_000)
            self.assertEqual(e["media-in-bytes"]["total"], 12_000_000)
            self.assertEqual(e["yo-in"]["total"], 5)  # a bot's sends are apart: not in the human counts
            self.assertEqual(e["bot-yo"]["hours"][-1], 7)
            self.assertEqual(e["bot-media:YoGPT"]["total"], 2)
            self.assertEqual(e["media-out"]["total"], 0)
            self.assertEqual(e["media-out"]["today"], 0)
            self.assertEqual(e["yo-in"]["today"], 5)
            self.assertEqual(e["yo-out"]["today"], 4 if (minute - 60) // 1440 == day else 0)


if __name__ == "__main__":
    unittest.main()
