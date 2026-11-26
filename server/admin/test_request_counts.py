"""Requests 24h: signed requests each user sent in the last 24 hours, from the hub's request_hours (no network)."""
import os
import sqlite3
import tempfile
import time
import unittest

import users


class RequestCountTests(unittest.TestCase):
    def test_requests_of_the_last_24_hours_per_sender(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "hub.sqlite")
            db = sqlite3.connect(path)
            db.executescript("""
                CREATE TABLE account(id BLOB, created_at INTEGER);
                CREATE TABLE account_device(id BLOB, user_id BLOB, last_seen_at INTEGER, push_platform INTEGER,
                    push_token BLOB, app TEXT, created_at INTEGER);
                CREATE TABLE account_backup(user_id BLOB, updated_at INTEGER);
                CREATE TABLE account_name(value TEXT, spelling TEXT, user_id BLOB, kind INTEGER);
                CREATE TABLE message(device_id BLOB);
                CREATE TABLE stat_request(user_id BLOB, hour INTEGER, n INTEGER, PRIMARY KEY(user_id, hour));
            """)
            hour = int(time.time() * 1000) // 3_600_000
            a, b, c = b"\x01" * 16, b"\x02" * 16, b"\x03" * 16
            db.executemany("INSERT INTO account VALUES (?, 0)", [(a,), (b,), (c,)])
            db.executemany("INSERT INTO stat_request VALUES (?, ?, ?)",
                           [(a, hour, 500), (a, hour - 23, 20), (a, hour - 24, 1000), (b, hour - 3, 2)])
            db.commit()
            db.close()
            got = {u["id"]: u["requests_24h"] for u in users.hub("live", path)["users"]}
            self.assertEqual(got, {a.hex(): 520, b.hex(): 2, c.hex(): 0})

    def test_a_hub_without_the_table_counts_none(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "hub.sqlite")
            db = sqlite3.connect(path)
            db.executescript("CREATE TABLE account(id BLOB, created_at INTEGER); CREATE TABLE account_device(id BLOB, user_id BLOB,"
                             " last_seen_at INTEGER, push_platform INTEGER, push_token BLOB, app TEXT, created_at INTEGER);"
                             " CREATE TABLE account_backup(user_id BLOB, updated_at INTEGER);"
                             " CREATE TABLE account_name(value TEXT, spelling TEXT, user_id BLOB, kind INTEGER);"
                             " CREATE TABLE message(device_id BLOB); INSERT INTO account VALUES (x'01', 0);")
            db.commit()
            db.close()
            self.assertEqual([u["requests_24h"] for u in users.hub("live", path)["users"]], [0])


if __name__ == "__main__":
    unittest.main()
