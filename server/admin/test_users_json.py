"""A snapshot's data, its collector time and the dev hub's own collection time."""
import contextlib
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

import users


class SnapshotTests(unittest.TestCase):
    def test_queue_counts_per_user(self):
        with contextlib.closing(sqlite3.connect(":memory:")) as db:
            db.executescript("""
                CREATE TABLE users(id, created_at);
                CREATE TABLE handles(user_id, kind, spelling, value);
                CREATE TABLE devices(id, user_id, last_seen_at, push_platform, push_token, app);
                CREATE TABLE backups(user_id, updated_at);
                CREATE TABLE messages(device_id);
                INSERT INTO users VALUES(1, 1), (2, 2);
                INSERT INTO devices VALUES(10, 1, 5, 0, '', ''), (11, 1, 4, 0, '', '');
                INSERT INTO messages VALUES(10), (10), (11);
            """)
            self.assertEqual([(r[0], r[3], r[8]) for r in db.execute(users.USERS, (10,))], [(1, 2, 3), (2, 0, 0)])

    def test_sent_history_comes_from_stats_not_the_minute_buffer(self):
        """Schema 10: events keeps 2 hours; the totals and days must come from stats (the live "Sent" chart)."""
        now = 498800 * 3600000
        with contextlib.closing(sqlite3.connect(":memory:")) as db:
            db.executescript("""
                CREATE TABLE stats(metric TEXT, key TEXT NOT NULL DEFAULT '', hour INTEGER, value INTEGER,
                    PRIMARY KEY(metric, key, hour));
                CREATE TABLE events(kind TEXT, minute INTEGER, n INTEGER, PRIMARY KEY(kind, minute));
                CREATE TABLE levels(kind, hour, value);
            """)
            db.executemany("INSERT INTO stats VALUES ('yo-out', '', ?, 800)", [(498800 - h,) for h in range(164)])
            db.execute("INSERT INTO events VALUES ('yo-out', ?, 3)", (now // 60000,))
            sent = users.events(db, now)["yo-out"]
            self.assertEqual(sent["total"], 164 * 800)
            self.assertEqual(sum(sent["year"]), 164 * 800)
            self.assertEqual(sent["hours"][-1], 800)
            self.assertEqual(sent["minutes"][-1], 3)

    def test_dev_collection_and_tag(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "users.json"
            want = path.with_name("want-dev")
            with patch.object(users, "OUT", str(path)), patch.object(users, "updates", return_value={"at": 1}), \
                    patch.object(users, "hub", side_effect=lambda name, _: {"name": name}) as collect, \
                    patch.dict(os.environ, {"ADMIN_REQUEST_FILE": str(path.with_name("want"))}):
                users.main()
                self.assertEqual([c.args[0] for c in collect.call_args_list], ["live"])
                want.touch()
                users.main()
                fresh = json.loads(path.read_bytes())
                self.assertEqual(fresh["dev_at"], fresh["at"])
                self.assertEqual(fresh["tag"], users.snapshot_tag({**fresh, "at": 2, "updates": {"at": 3}}))
                os.utime(want, (1, 1))
                collect.reset_mock()
                users.main()
                cached = json.loads(path.read_bytes())
                self.assertEqual([c.args[0] for c in collect.call_args_list], ["live"])
                self.assertEqual((cached["dev"], cached["dev_at"]), (fresh["dev"], fresh["dev_at"]))

    def test_hourly_history_and_minute_buffer(self):
        with contextlib.closing(sqlite3.connect(":memory:")) as db:
            db.executescript("""
                CREATE TABLE stats(metric, key, hour, value);
                INSERT INTO stats VALUES('yo-in', '', 24, 17), ('yo-in', '', 1, 9);
                CREATE TABLE events(kind, minute, n);
                INSERT INTO events VALUES('yo-in', 1440, 3), ('yo-in', 1380, 5);
            """)
            counts = users.events(db, users.DAY_MS)["yo-in"]
            self.assertEqual((counts["total"], counts["today"], counts["hours"][-1]), (26, 17, 17))
            self.assertEqual(counts["year"][-2:], [9, 17])
            self.assertEqual(counts["minutes"], [0] * 59 + [3])

    def test_hourly_stats_and_minute_buffer_are_independent(self):
        with contextlib.closing(sqlite3.connect(":memory:")) as db:
            db.executescript("""
                CREATE TABLE stats(metric, key, hour INTEGER, value INTEGER);
                CREATE TABLE events(kind, minute INTEGER, n INTEGER);
                INSERT INTO stats VALUES('yo-in','',2400,40), ('yo-in','',1920,7),
                                        ('deliveries','',2100,8), ('diag','7',2400,19);
                INSERT INTO events VALUES('yo-in',144000,3);
            """)
            event = users.events(db, 2400 * 3600000)["yo-in"]
            self.assertEqual((event["hours"][-1], event["today"], event["total"]), (40, 40, 47))
            self.assertEqual(sum(event["minutes"]), 3)
            self.assertEqual(users.deliveries(db, 2400)["days"][0], 8)
