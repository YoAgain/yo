"""Collector signaling and waiting use temporary files, never systemd."""
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch

from test_help import load


class RequestTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        with patch.dict(os.environ, {"ADMIN_USER": "test", "ADMIN_PASSWORD_HASH": "unused",
                                  "ADMIN_SESSION_KEY": "00" * 32,
                                  "ACCESS_TEAM": "test", "ACCESS_AUD": "test"}):
            cls.admin = load("admin")

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.want = Path(self.temp.name) / "want"
        self.output = Path(self.temp.name) / "users.json"
        self.collector = self.admin.CollectorRequest(self.want, self.output)

    def publish(self, timestamp):
        temp = self.output.with_suffix('.tmp')
        temp.write_text(json.dumps({"at": 2, "live": {"total": 7}}))
        os.utime(temp, ns=(timestamp, timestamp))
        temp.replace(self.output)

    def test_plain_read_and_cache(self):
        self.publish(1)
        with patch.object(self.admin.gzip, "compress", wraps=self.admin.gzip.compress) as compress:
            first, stale = self.collector.snapshot()
            self.assertFalse(stale)
            self.assertFalse(self.want.exists())
            self.assertIs(self.collector.snapshot()[0], first)
            compress.assert_called_once()
            self.publish(1)  # the inode changed, even with the same timestamp
            self.assertIsNot(self.collector.snapshot()[0], first)
        self.collector.want_dev()
        self.assertTrue(self.want.with_name("want-dev").exists())
        self.assertFalse(self.want.exists())

    def test_waits_for_atomic_publication(self):
        self.publish(1)
        def publish():
            while not self.want.exists():
                if stop.wait(.005):
                    return
            self.publish(self.want.stat().st_mtime_ns + 1)
        stop = threading.Event()
        worker = threading.Thread(target=publish)
        worker.start()
        try:
            result, stale = self.collector.snapshot(timeout=1, after=2)
        finally:
            stop.set()
            worker.join()
        self.assertFalse(stale)
        self.assertEqual(json.loads(result[0])["live"]["total"], 7)

    def test_timeout_returns_old_data(self):
        self.publish(1)
        result, stale = self.collector.snapshot(timeout=.01, after=3)
        self.assertTrue(stale)
        self.assertEqual(result[2], 2)

    def test_missing_and_invalid_snapshot(self):
        self.assertTrue(self.collector.snapshot()[1])
        self.output.write_text('{')
        self.assertTrue(self.collector.snapshot()[1])

    def test_unwritable_request_falls_back_to_stale(self):
        self.publish(1)
        self.collector.request_file = self.want / 'missing' / 'want'
        self.assertTrue(self.collector.snapshot(after=2)[1])
