import sqlite3
import unittest

from users import DAY_MS, attestation


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
