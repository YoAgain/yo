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
        temp.write_text(json.dumps({"live": {"total": 7}}))
        os.utime(temp, ns=(timestamp, timestamp))
        temp.replace(self.output)

    def test_coalesces_concurrent_requests_and_retriggers_after_five_seconds(self):
        with patch.object(self.admin.time, 'monotonic', return_value=10):
            threads = [threading.Thread(target=self.collector.request) for _ in range(8)]
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join()
            first = self.want.stat().st_mtime_ns
            os.utime(self.want, ns=(1, 1))
            self.assertEqual(self.collector.request(), first)
            self.assertEqual(self.want.stat().st_mtime_ns, 1)
        with patch.object(self.admin.time, 'monotonic', return_value=15):
            self.assertGreater(self.collector.request(), 1)

    def test_waits_for_atomic_publication(self):
        requested = self.collector.request()
        self.publish(requested - 1)
        timer = threading.Timer(.05, self.publish, args=(requested + 1,))
        timer.start()
        self.addCleanup(timer.join)
        result = self.collector.snapshot(timeout=.5)
        self.assertFalse(result['stale'])
        self.assertEqual(result['live']['total'], 7)

    def test_timeout_returns_old_data_even_at_equal_mtime(self):
        requested = self.collector.request()
        self.publish(requested)
        self.assertEqual(self.collector.snapshot(timeout=.01),
                         {'live': {'total': 7}, 'stale': True})

    def test_missing_and_invalid_snapshot(self):
        self.assertEqual(self.collector.snapshot(timeout=0), {'stale': True})
        self.output.write_text('{')
        self.assertEqual(self.collector.snapshot(timeout=0), {'stale': True})

    def test_unwritable_request_falls_back_to_stale(self):
        self.publish(1)
        self.collector.request_file = self.want / 'missing' / 'want'
        self.assertTrue(self.collector.snapshot(timeout=0)['stale'])
