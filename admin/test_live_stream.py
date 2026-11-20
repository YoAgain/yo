"""The header's live strip: the hub's stream of Yos handed on to the page as it comes, behind the login."""
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
import tempfile
import threading
import unittest
from unittest.mock import patch

from test_help import load


class FakeHub(BaseHTTPRequestHandler):
    """One Yo, then waits until the test has seen it: an answer held back anywhere would hang the test."""
    seen = threading.Event()
    tokens = []

    def log_message(self, *_):
        pass

    def do_GET(self):
        FakeHub.tokens.append(self.headers.get("Yo-Admin-Token"))
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        self.wfile.write(b'data: {"k":"yo"}\n\n')
        self.wfile.flush()
        FakeHub.seen.wait(5)
        self.wfile.write(b':\n\ndata: {"k":"media"}\n\n')  # the hub's keep-alive, then a Yo


def serve(server):
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return thread


class LiveStreamTests(unittest.TestCase):
    def setUp(self):
        with patch.dict(os.environ, {"ADMIN_USER": "test", "ADMIN_PASSWORD_HASH": "unused", "ADMIN_SESSION_KEY": "00",
                                     "ACCESS_TEAM": "invalid", "ACCESS_AUD": "test",
                                     "STATE_DIRECTORY": tempfile.gettempdir()}):
            self.admin = load("admin")
        self.hub = ThreadingHTTPServer(("127.0.0.1", 0), FakeHub)
        self.addCleanup(self.hub.server_close)
        self.addCleanup(self.hub.shutdown)
        serve(self.hub)

    def test_streamed_at_once_behind_the_login(self):
        admin = self.admin
        hub_url = "http://%s:%d" % self.hub.server_address
        with patch.object(admin.Admin, "who", return_value="test"), \
                patch.object(admin, "session_valid", return_value=False) as login, \
                patch.object(admin, "ADMIN_TOKEN", "secret"), \
                patch.dict(admin.HUB_URLS, {"live": hub_url, "dev": "http://127.0.0.1:9"}):
            server = ThreadingHTTPServer(("127.0.0.1", 0), admin.Admin)
            thread = serve(server)
            try:
                def get(path):
                    conn = http.client.HTTPConnection(*server.server_address, timeout=5)
                    conn.request("GET", path)
                    return conn, conn.getresponse()
                conn, response = get("/live?hub=live")
                self.assertEqual(response.status, 401)
                conn.close()
                login.return_value = True
                for path, status in (("/live?hub=x", 400), ("/live?hub=dev", 502)):
                    conn, response = get(path)
                    self.assertEqual(response.status, status)
                    conn.close()
                conn, response = get("/live?hub=live")
                self.assertEqual(response.status, 200)
                self.assertEqual(response.getheader("Content-Type"), "text/event-stream")
                self.assertEqual(response.getheader("Cache-Control"), "no-cache, no-transform")
                self.assertEqual(response.getheader("X-Accel-Buffering"), "no")
                # The hello: a quick retry, and a ping the page's watchdog can see (a comment it could not).
                self.assertEqual(response.fp.readline(), b"retry: 2000\n")
                self.assertEqual(response.fp.readline(), b"event: ping\n")
                lines, pings = [], 0
                while len(lines) < 1:
                    line = response.fp.readline()
                    if line.startswith(b"data: {"):
                        lines.append(line)
                FakeHub.seen.set()  # the first Yo came before the hub's answer ended
                while len(lines) < 2:
                    line = response.fp.readline()
                    self.assertFalse(line.startswith(b":"))  # the keep-alive comes on as a ping event
                    pings += line == b"event: ping\n"
                    if line.startswith(b"data: {"):
                        lines.append(line)
                self.assertEqual(pings, 1)
                self.assertEqual(lines, [b'data: {"k":"yo"}\n', b'data: {"k":"media"}\n'])
                self.assertEqual(FakeHub.tokens, ["secret"])
                conn.close()
            finally:
                server.shutdown()
                server.server_close()
                thread.join()


if __name__ == "__main__":
    unittest.main()
