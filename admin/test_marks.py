"""Stars, to-do owners and the kept active-user counts; the Users tab's collector numbers."""
import http.client
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import threading
import unittest
from unittest.mock import patch

from test_feedback import load

HEX = "AB" * 16


class MarksTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        with patch.dict(os.environ, {"ADMIN_USER": "test", "ADMIN_PASSWORD_HASH": "unused", "ADMIN_SESSION_KEY": "00",
                                     "ACCESS_TEAM": "invalid", "ACCESS_AUD": "test", "STATE_DIRECTORY": self.temp.name}):
            self.admin = load("admin")

    def test_stars_and_todo_owners_per_hub(self):
        admin = self.admin
        self.assertEqual(admin.marks_read(), {"stars": [], "todo_user": {}})
        admin.marks_change({"hub": "live", "star": "u:" + HEX, "on": True})
        admin.marks_change({"hub": "dev", "star": "t:7", "on": True})
        admin.marks_change({"hub": "live", "todo": 7, "user": HEX})
        marks = admin.marks_change({"hub": "live", "star": "u:" + HEX, "on": True})  # twice: still once
        self.assertEqual(marks, {"stars": ["dev:t:7", "live:u:" + HEX], "todo_user": {"live:7": HEX}})
        admin.marks_change({"hub": "dev", "star": "t:7", "on": False})
        admin.marks_change({"hub": "live", "todo": 7, "user": None})
        self.assertEqual(admin.marks_read(), {"stars": ["live:u:" + HEX], "todo_user": {}})

    def test_malformed_marks_are_refused(self):
        for bad in [{"hub": "other", "star": "t:1", "on": True}, {"hub": "live", "star": "u:ab", "on": True},
                    {"hub": "live", "star": "t:0", "on": True}, {"hub": "live", "star": "t:1", "on": 1},
                    {"hub": "live", "star": "x:../", "on": True}, {"hub": "live", "todo": "7", "user": HEX},
                    {"hub": "live", "todo": 7, "user": HEX.lower()}, {"hub": "live", "todo": -1, "user": None}, {"hub": "live"}]:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                self.admin.marks_change(bad)
        self.assertFalse((Path(self.temp.name) / "support-marks.json").exists())

    def test_marks_routes_need_login_and_origin(self):
        admin = self.admin
        with patch.object(admin.Admin, "who", return_value="test"), \
                patch.object(admin, "session_valid", return_value=False) as login:
            server = admin.ThreadingHTTPServer(("127.0.0.1", 0), admin.Admin)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                def call(method, path, data=None, origin=None):
                    conn = http.client.HTTPConnection(*server.server_address)
                    headers = {"Content-Type": "application/json", **({"Origin": origin} if origin else {})}
                    conn.request(method, path, None if data is None else json.dumps(data), headers)
                    response = conn.getresponse()
                    body = response.read()
                    conn.close()
                    return response.status, body
                star = {"hub": "live", "star": "t:3", "on": True}
                self.assertEqual(call("GET", "/marks.json")[0], 401)
                self.assertEqual(call("POST", "/marks", star)[0], 401)
                login.return_value = True
                self.assertEqual(call("POST", "/marks", star, "https://invalid.example")[0], 403)
                self.assertEqual(call("POST", "/marks", {**star, "star": "t:x"})[0], 400)
                status, body = call("POST", "/marks", star)
                self.assertEqual((status, json.loads(body)["stars"]), (200, ["live:t:3"]))
                self.assertEqual(json.loads(call("GET", "/marks.json")[1])["stars"], ["live:t:3"])
            finally:
                server.shutdown()
                server.server_close()
                thread.join()

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


if __name__ == "__main__":
    unittest.main()
