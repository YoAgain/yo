"""The header's live strip: Yos per minute for the last hour, bots left out, counts only."""
import sqlite3
import unittest

import users

NOW = 29_000_000 * 60000 + 30_000  # half a minute into minute 29,000,000


class LiveTests(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(":memory:")
        self.db.execute("CREATE TABLE events (kind TEXT NOT NULL, minute INTEGER NOT NULL, n INTEGER NOT NULL, PRIMARY KEY (kind, minute)) WITHOUT ROWID")
        m = NOW // 60000
        self.db.executemany("INSERT INTO events VALUES (?,?,?)", [
            ("yo-in", m, 3), ("media-in", m, 2), ("yo-in-bot", m, 50), ("yo-out", m, 9),
            ("yo-in", m - 1, 4), ("yo-in", m - 59, 7), ("yo-in", m - 60, 100), ("yo-in", m + 1, 100)])

    def test_last_hour_per_minute_without_bots(self):
        out = users.live(self.db, NOW)
        self.assertEqual(out["minute"], NOW // 60000)
        self.assertEqual(len(out["minutes"]), 60)
        self.assertEqual(out["minutes"][-1], 5)  # yo-in + media-in; yo-in-bot and yo-out left out
        self.assertEqual(out["minutes"][-2], 4)
        self.assertEqual(out["minutes"][0], 7)  # 59 minutes ago is the first; 60 ago and the future are out
        self.assertEqual(sum(out["minutes"]), 16)

    def test_empty(self):
        self.db.execute("DELETE FROM events")
        self.assertEqual(users.live(self.db, NOW)["minutes"], [0] * 60)


if __name__ == "__main__":
    unittest.main()
