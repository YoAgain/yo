"""Problems: each group's builds and its reports of the last 24 hours, per hour (spec 4.2), for "still reported"."""
import sqlite3
import time
import unittest

import users


class ProblemTests(unittest.TestCase):
    def test_builds_and_the_last_24_hours(self):
        db = sqlite3.connect(":memory:")
        h = int(time.time()) // 3600
        db.executescript(f"""
        CREATE TABLE diag (id INTEGER PRIMARY KEY, kind TEXT, signature TEXT, count INTEGER, first_seen INTEGER,
            last_seen INTEGER, app TEXT, os TEXT, model TEXT, payload TEXT, done INTEGER, done_builds TEXT, done_at INTEGER);
        CREATE TABLE diag_build (diagnostic_id INTEGER, app TEXT, count INTEGER, last_seen INTEGER);
        CREATE TABLE diagnostic_hours (diagnostic_id INTEGER, hour INTEGER, count INTEGER);
        INSERT INTO diag VALUES (1, 'error', 's', 5, 1, 2, 'ios/2', '26', 'x', '{{}}', 1, 'ios/2', 2);
        INSERT INTO diag_build VALUES (1, 'ios/2', 3, 2), (1, 'ios/1', 2, 1);
        INSERT INTO diagnostic_hours VALUES (1, {h}, 4), (1, {h - 3}, 1), (1, {h - 30}, 9);
        """)
        users.prepare_stats(db)
        row = users.diagnostics(db)[0]
        self.assertEqual(row["builds"], [["ios/1", 2, 1], ["ios/2", 3, 2]])
        self.assertEqual(sum(row["hours_24h"]), 5)
        self.assertEqual((row["hours_24h"][-1], row["hours_24h"][-4]), (4, 1))

    def test_an_older_hub_has_no_builds(self):
        db = sqlite3.connect(":memory:")
        db.execute("CREATE TABLE diag (id INTEGER PRIMARY KEY, kind TEXT, count INTEGER, last_seen INTEGER, payload TEXT)")
        db.execute("INSERT INTO diag VALUES (1, 'error', 1, 1, '{}')")
        users.prepare_stats(db)
        row = users.diagnostics(db)[0]
        self.assertEqual((row["builds"], sum(row["hours_24h"])), ([], 0))


if __name__ == "__main__":
    unittest.main()
