"""The services' book as the collector reads it."""
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

from test_help import load

NOW = 1_800_000_000


class ServicesTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.book = str(Path(temp.name) / "book.sqlite")
        with patch.dict(os.environ, {"YO_SERVICES_DB": self.book, "YO_SERVICES_DEV_DB": self.book + "-none"}):
            self.users = load("users")

    def test_bots_and_reminders(self):
        self.assertIsNone(self.users.services("live", NOW * 1000))
        db = sqlite3.connect(self.book)
        db.execute("CREATE TABLE services_yos (id INTEGER PRIMARY KEY, bot, way, at, user, cron_due, cron_done)")
        db.executemany("INSERT INTO services_yos (bot, way, at, user, cron_due, cron_done) VALUES (?,?,?,?,?,?)", [
            ("PING", "received", NOW - 90_000, None, None, None), ("PING", "sent", NOW, None, None, None),
            ("DST", "received", NOW, "a" * 32, NOW + 60, None),
            ("DST", "received", NOW, "b" * 32, NOW - 60, NOW - 50),
            ("DST", "received", NOW, None, NOW - 90_000, None)])
        db.commit()
        db.close()
        out = self.users.services("live", NOW * 1000)
        dst, ping = out["bots"]
        self.assertEqual((ping["received"], ping["received_day"], ping["sent"], ping["waiting"], ping["next"]), (1, 0, 1, 0, None))
        self.assertEqual((dst["waiting"], dst["next"], dst["done"], dst["missed"]), (1, NOW + 60, 1, 1))
        self.assertEqual([r["done"] for r in out["reminders"]], [None, None, NOW - 50])
        self.assertNotIn("a" * 32, str(out["table"]), "the raw table has nobody in it")
        self.assertIsNone(self.users.services("dev", NOW * 1000))


if __name__ == "__main__":
    unittest.main()
