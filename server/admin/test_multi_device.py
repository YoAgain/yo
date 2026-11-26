"""Issue #4's gate: the share of phones seen in 30 days on a multi-device build, per platform."""
import contextlib
import sqlite3
import unittest

from users import DAY_MS, multi_device


class MultiDeviceTests(unittest.TestCase):
    def test_share_per_platform_of_devices_seen_in_30_days(self):
        now = 100 * DAY_MS
        with contextlib.closing(sqlite3.connect(":memory:")) as db:
            db.executescript("CREATE TABLE account_device(id, user_id, last_seen_at, app);")
            db.executemany("INSERT INTO account_device VALUES(?,?,?,?)", [
                (1, 1, now, "ios/202612010000"), (2, 2, now, "ios/202611010000"),
                (3, 3, now - 31 * DAY_MS, "ios/202612010000"), (4, 4, now, "android/120"),
                (5, 5, now, "cli/1"), (6, 6, now, None)])
            out = multi_device(db, now, "ios/202612010000, android/121")
            self.assertEqual(out["ios"], {"seen": 2, "ready": 1, "min": 202612010000})
            self.assertEqual(out["android"], {"seen": 1, "ready": 0, "min": 121})
            self.assertEqual(multi_device(db, now, None)["ios"], {"seen": 2, "ready": 0, "min": None})


if __name__ == "__main__":
    unittest.main()
