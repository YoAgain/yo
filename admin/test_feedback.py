"""Run: python3 -m unittest discover -s admin -p 'test_*.py'."""
import importlib.util
import io
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).parent


def load(name):
    spec = importlib.util.spec_from_file_location(name, HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class FeedbackTests(unittest.TestCase):
    def test_types_for_old_and_new_schemas(self):
        users = load("users")
        for columns in ["", ", auto INTEGER DEFAULT 0", ", auto INTEGER DEFAULT 0, type TEXT"]:
            with self.subTest(columns=columns), tempfile.TemporaryDirectory() as directory:
                path = str(Path(directory) / "hub.sqlite")
                with sqlite3.connect(path) as db:
                    db.execute("CREATE TABLE handles (user_id BLOB, spelling TEXT, value TEXT, kind INTEGER)")
                    db.execute("CREATE TABLE feedback (id INTEGER PRIMARY KEY, user_id BLOB, text TEXT, app_version TEXT, created_at INTEGER" + columns + ")")
                    db.execute("INSERT INTO feedback (id, user_id, text, app_version, created_at) VALUES (1, X'', 'plain', '1', 1), (2, X'', 'task', 'admin', 2)")
                    if 'auto' in columns:
                        db.execute("INSERT INTO feedback (id, text, app_version, created_at, auto) VALUES (3, 'trace', 'admin', 3, 1)")
                    if 'type' in columns:
                        # Explicit type wins even when legacy fields disagree.
                        db.execute("UPDATE feedback SET type = 'feedback'")
                rows = users.hub("live", path)["feedback"]
                kinds = {r["id"]: r["type"] for r in rows}
                self.assertEqual(kinds[1], "feedback")
                self.assertEqual(kinds[2], "feedback" if 'type' in columns else "todo")
                if 'auto' in columns:
                    self.assertEqual(kinds[3], "feedback" if 'type' in columns else "crash")

    def test_migration_resumes_and_remaps_done(self):
        import json
        migration = load("migrate_todos")
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory)
            (state / "todos.json").write_text(json.dumps([
                {"id": "t1", "hub": "live", "text": "already moved"},
                {"id": "t2", "hub": "dev", "text": "next"}]))
            (state / "todos-migrated.json").write_text('{"live:t1": "live:11"}')
            (state / "feedback-done.json").write_text('["live:t1", "dev:t2", "live:7"]')
            with patch.dict(os.environ, {"STATE_DIRECTORY": directory, "YO_ADMIN_TOKEN": "test"}), \
                    patch.object(migration.urllib.request, "urlopen", return_value=io.BytesIO(b'{"id": 22}')) as send:
                migration.main()
            self.assertEqual(send.call_count, 1)
            self.assertEqual(json.loads((state / "feedback-done.json").read_text()), ["dev:22", "live:11", "live:7"])
            self.assertFalse((state / "todos.json").exists())
            self.assertTrue((state / "todos.json.migrated").exists())

    def test_todo_uses_selected_hub_and_utf8(self):
        with patch.dict(os.environ, {"ADMIN_USER": "test", "ADMIN_PASSWORD_HASH": "unused",
                "ADMIN_SESSION_KEY": "00", "ACCESS_TEAM": "invalid", "ACCESS_AUD": "test",
                "YO_ADMIN_TOKEN": "test-token", "YO_HUB_DEV_URL": "http://localhost:19090"}):
            admin = load("admin")
        with patch.object(admin.urllib.request, "urlopen", return_value=io.BytesIO(b'{"id": 42}')) as send:
            self.assertEqual(admin.todo_add("dev", "fix 🎨"), {"id": 42})
            request = send.call_args.args[0]
            self.assertEqual(request.full_url, "http://localhost:19090/v1/admin/todos")
            self.assertEqual(request.data, "fix 🎨".encode())
            self.assertEqual(request.get_header("Yo-admin-token"), "test-token")


class ReservedTests(unittest.TestCase):
    def setUp(self):
        with patch.dict(os.environ, {"ADMIN_USER": "test", "ADMIN_PASSWORD_HASH": "unused",
                "ADMIN_SESSION_KEY": "00", "ACCESS_TEAM": "invalid", "ACCESS_AUD": "test",
                "YO_ADMIN_TOKEN": "test-token", "YO_HUB_DEV_URL": "http://localhost:19090"}):
            self.admin = load("admin")

    def test_proxy_methods_bodies_and_skeleton_name_encoding(self):
        import json
        for method, data, name, suffix in [
                ("GET", None, None, ""),
                ("POST", ["Admin", "Support"], None, ""),
                ("PUT", {"enabled": False}, None, ""),
                ("DELETE", None, "Ädm1n/a", "/%C3%84dm1n%2Fa")]:
            with self.subTest(method=method), patch.object(self.admin.urllib.request, "urlopen",
                    return_value=io.BytesIO(b'{"enabled": false, "names": []}')) as send:
                self.admin.reserved_request("dev", method, data, name)
                request = send.call_args.args[0]
                self.assertEqual(request.full_url, "http://localhost:19090/v1/admin/reserved" + suffix)
                self.assertEqual(request.method, method)
                self.assertEqual(request.get_header("Yo-admin-token"), "test-token")
                self.assertEqual(None if request.data is None else json.loads(request.data), data)

    def test_writes_require_login_origin_and_valid_hub(self):
        import http.client
        import json
        import threading
        admin = self.admin
        with patch.object(admin.Admin, "who", return_value="test"), \
                patch.object(admin, "session_valid", return_value=True) as logged_in, \
                patch.object(admin, "reserved_request", return_value={}) as request:
            server = admin.ThreadingHTTPServer(("127.0.0.1", 0), admin.Admin)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                def post(data, origin=None):
                    conn = http.client.HTTPConnection(*server.server_address)
                    headers = {"Content-Type": "application/json"}
                    if origin is not None:
                        headers["Origin"] = origin
                    conn.request("POST", "/reserved", json.dumps(data), headers)
                    response = conn.getresponse()
                    response.read()
                    conn.close()
                    return response.status
                data = {"hub": "dev", "action": "switch", "enabled": False}
                self.assertEqual(post(data, "https://invalid.example"), 403)
                logged_in.return_value = False
                self.assertEqual(post(data), 401)
                logged_in.return_value = True
                self.assertEqual(post({**data, "hub": "other"}), 400)
                request.assert_not_called()
                self.assertEqual(post(data), 200)
                request.assert_called_once_with("dev", "PUT", {"enabled": False}, None)
            finally:
                server.shutdown()
                server.server_close()
                thread.join()


if __name__ == "__main__":
    unittest.main()
