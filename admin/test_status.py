"""The Status tab: the STATUS bot's config (written here) and its last report (written by the bot)."""
import http.client
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch

from test_help import load


class StatusTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.last = Path(self.temp.name) / "last.json"
        with patch.dict(os.environ, {"ADMIN_USER": "test", "ADMIN_PASSWORD_HASH": "unused", "ADMIN_SESSION_KEY": "00",
                                     "ACCESS_TEAM": "invalid", "ACCESS_AUD": "test", "STATE_DIRECTORY": self.temp.name,
                                     "STATUS_LAST": str(self.last)}):
            self.admin = load("admin")
        self.config = Path(self.temp.name) / "status-bot.json"

    def test_off_until_saved_and_the_baseline_kept(self):
        admin = self.admin
        self.assertEqual(admin.status_read()["config"], {"every_minutes": 0, "baseline_users": 0})
        self.assertIsNone(admin.status_read()["last"])
        self.config.write_text(json.dumps({"every_minutes": 30, "to": "Lina", "baseline_users": 30}))
        out = admin.status_save({"every_minutes": 1440})
        self.assertEqual(out["config"], {"every_minutes": 1440, "baseline_users": 30})
        self.assertEqual(json.loads(self.config.read_text()), {"every_minutes": 1440, "baseline_users": 30}, "no recipient kept")
        self.assertEqual(admin.status_save({"every_minutes": 0})["config"]["every_minutes"], 0)
        self.last.write_text(json.dumps({"at": 1, "users": 42, "sent": True}))
        self.assertEqual(admin.status_read()["last"]["users"], 42)

    def test_malformed_settings_and_any_recipient_are_refused(self):
        for bad in [{"every_minutes": 7}, {"every_minutes": "30"}, {"every_minutes": True}, {},
                    {"every_minutes": 30, "to": "Lina"}, {"to": "Lina"}, {"every_minutes": 30, "baseline_users": 1}]:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                self.admin.status_save(bad)
        self.assertFalse(self.config.exists())

    def test_status_routes_need_login_and_origin(self):
        admin = self.admin
        with patch.object(admin.Admin, "who", return_value="test"), \
                patch.object(admin, "session_valid", return_value=False) as login:
            server = admin.ThreadingHTTPServer(("127.0.0.1", 0), admin.Admin)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                def call(method, path, data=None, origin=None):
                    conn = http.client.HTTPConnection(*server.server_address)
                    headers = {"Content-Type": "application/json", **({"Origin": origin} if origin else {})}
                    conn.request(method, path, None if data is None else json.dumps(data), headers)
                    response = conn.getresponse()
                    body = response.read()
                    conn.close()
                    return response.status, body
                good = {"every_minutes": 60}
                self.assertEqual(call("GET", "/status.json")[0], 401)
                self.assertEqual(call("POST", "/status", good)[0], 401)
                login.return_value = True
                self.assertEqual(call("POST", "/status", good, "https://invalid.example")[0], 403)
                self.assertEqual(call("POST", "/status", {**good, "every_minutes": 5})[0], 400)
                status, body = call("POST", "/status", good)
                self.assertEqual((status, json.loads(body)["config"]["every_minutes"]), (200, 60))
                self.assertEqual(json.loads(call("GET", "/status.json")[1])["config"], {"every_minutes": 60, "baseline_users": 0})
                self.assertEqual(call("POST", "/status", {**good, "to": "Lina"})[0], 400)
            finally:
                server.shutdown()
                server.server_close()
                thread.join()


class GoneClientTests(unittest.TestCase):
    """A page that went before its answer (reloaded, a poll replaced) costs no traceback in the journal."""

    def test_a_closed_connection_is_no_error(self):
        with patch.dict(os.environ, {"ADMIN_USER": "test", "ADMIN_PASSWORD_HASH": "unused", "ADMIN_SESSION_KEY": "00",
                                     "ACCESS_TEAM": "invalid", "ACCESS_AUD": "test",
                                     "STATE_DIRECTORY": tempfile.gettempdir()}):
            admin = load("admin")

        for error in (BrokenPipeError, ConnectionResetError):
            class Gone:
                def write(self, _, error=error):
                    raise error("the page went")

            handler = admin.Admin.__new__(admin.Admin)
            handler.command, handler.path, handler.request_version = "GET", "/api/stats", "HTTP/1.1"
            handler.requestline = "GET /api/stats HTTP/1.1"
            handler.wfile = Gone()
            with patch("builtins.print"):
                handler.send(200, b"{}", "application/json")
            self.assertTrue(handler.close_connection)


if __name__ == "__main__":
    unittest.main()
