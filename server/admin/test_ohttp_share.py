"""Issue #8: the share of phones seen in 30 days that came inside OHTTP, per platform."""
import contextlib
import sqlite3
import unittest

from users import DAY_MS, ohttp_share


class OhttpShareTests(unittest.TestCase):
    def test_share_per_platform_of_devices_seen_in_30_days(self):
        now = 100 * DAY_MS
        with contextlib.closing(sqlite3.connect(":memory:")) as db:
            db.executescript("CREATE TABLE account_device(id, user_id, last_seen_at, app);")
            self.assertIsNone(ohttp_share(db, now), "a hub before the column")
            db.executescript("ALTER TABLE account_device ADD COLUMN ohttp_at INTEGER;")
            db.executemany("INSERT INTO account_device VALUES(?,?,?,?,?)", [
                (1, 1, now, "ios/1", now), (2, 2, now, "ios/1", None),
                (3, 3, now, "ios/1", now - 31 * DAY_MS), (4, 4, now - 31 * DAY_MS, "ios/1", now - 31 * DAY_MS),
                (5, 5, now, "android/120", now - DAY_MS), (6, 6, now, "cli/1", now)])
            self.assertEqual(ohttp_share(db, now), {"ios": {"seen": 3, "ohttp": 1}, "android": {"seen": 1, "ohttp": 1}})


if __name__ == "__main__":
    unittest.main()
