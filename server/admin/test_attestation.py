import sqlite3
import unittest

from users import DAY_MS, attestation, attested_days, levels


class AttestationTest(unittest.TestCase):
    def test_account_counts_and_old_schema(self):
        db = sqlite3.connect(":memory:")
        self.addCleanup(db.close)
        now = 3 * DAY_MS
        self.assertIsNone(attestation(db, now))
        db.executescript("""
            CREATE TABLE devices(user_id INTEGER, app TEXT, last_seen_at INTEGER);
            CREATE TABLE attestation(user_id INTEGER PRIMARY KEY, platform TEXT, state TEXT, reason TEXT, level TEXT, at_ms INTEGER);
            INSERT INTO attestation VALUES(1,'android','attested',NULL,'strong',20),
                                          (2,'android','failed','device_integrity','basic',30);
        """)
        db.executemany("INSERT INTO devices VALUES(?,?,?)", [
            (1, "ios/1", now), (3, "ios/2", now), (3, "ios/3", now),
            (4, "android/1", now), (5, "android/1", DAY_MS), (6, "cli/2", now)])
        rows = attestation(db, now)
        self.assertEqual(rows["ios"]["unknown"], 1)
        self.assertEqual(rows["android"], dict(attested=1, failed=1, unknown=1,
                         reasons={"device_integrity": 1}, levels={"strong": 1, "basic": 1}, last=30))

    def test_attested_days_per_signup_day(self):
        db = sqlite3.connect(":memory:")
        self.addCleanup(db.close)
        midnight = 20 * DAY_MS
        self.assertIsNone(attested_days(db, midnight))
        db.executescript("""
            CREATE TABLE users(id INTEGER PRIMARY KEY, created_at INTEGER);
            CREATE TABLE attestation(user_id INTEGER PRIMARY KEY, platform TEXT, state TEXT, reason TEXT, level TEXT, at_ms INTEGER);
        """)
        # Today: three accounts, one attested, one failed, one not measured; yesterday: one attested.
        db.executemany("INSERT INTO users VALUES(?,?)", [(1, midnight + 1), (2, midnight + 2), (3, midnight + 3), (4, midnight - 5)])
        db.executemany("INSERT INTO attestation VALUES(?,?,?,?,?,?)", [
            (1, "ios", "attested", None, None, midnight + 9), (2, "android", "failed", "device_integrity", "basic", midnight + 9),
            (4, "android", "attested", None, "strong", midnight + 9)])
        days = attested_days(db, midnight)
        self.assertEqual(len(days), 14)
        self.assertEqual(days[-1], [3, 1])
        self.assertEqual(days[-2], [1, 1])
        self.assertEqual(days[0], [0, 0])

    def test_attested_level_once_sampled(self):
        db = sqlite3.connect(":memory:")
        self.addCleanup(db.close)
        db.execute("CREATE TABLE levels(kind TEXT, hour INTEGER, value INTEGER)")
        self.assertEqual(levels(db, 3 * DAY_MS)["attested_levels"], {})
        db.execute("INSERT INTO levels VALUES('attested', 70, 5)")
        self.assertEqual(levels(db, 3 * DAY_MS)["attested_levels"], {"70": 5})
