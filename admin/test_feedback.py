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
                    if 'type' in columns:
                        self.assertEqual(kinds[3], "feedback")
                    else:
                        self.assertNotIn(3, kinds)

    def test_diagnostics_collector_and_done_proxy(self):
        users = load("users")
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "hub.sqlite")
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE diagnostics(id INTEGER, kind TEXT, count INTEGER, first_seen INTEGER, last_seen INTEGER, app TEXT, os TEXT, model TEXT, payload TEXT, done INTEGER)")
                db.execute("INSERT INTO diagnostics VALUES(7,'error',20,1,3,'ios/1','26','iPhone','{}',1)")
                db.execute("INSERT INTO diagnostics VALUES(8,'crash',2,2,4,'ios/1','26','iPhone','{}',0)")
                db.execute("CREATE TABLE diagnostic_days(day INTEGER,kind TEXT,count INTEGER)")
                db.execute("INSERT INTO diagnostic_days VALUES(20000,'crash',2),(20000,'error',20)")
            snapshot = users.hub("live", path)
            self.assertEqual(snapshot["diagnostics_open"], 1)
            self.assertEqual(snapshot["diagnostic_days"], [{"day":20000,"kind":"crash","count":2},{"day":20000,"kind":"error","count":20}])
            rows = snapshot["diagnostics"]
            self.assertEqual(rows[1]["count"], 20)
            self.assertEqual(rows[1]["done"], 1)
        with patch.dict(os.environ, {"ADMIN_USER":"test", "ADMIN_PASSWORD_HASH":"unused", "ADMIN_SESSION_KEY":"00",
                "ACCESS_TEAM":"invalid", "ACCESS_AUD":"test", "YO_ADMIN_TOKEN":"test-token"}):
            admin = load("admin")
        with patch.object(admin.urllib.request, "urlopen", return_value=io.BytesIO(b'[]')) as send:
            admin.diagnostics_request("dev", {"id":7, "done":True})
            request = send.call_args.args[0]
            self.assertTrue(request.full_url.endswith("/v1/admin/diagnostics"))
            self.assertEqual(request.method, "PUT")
            self.assertEqual(request.get_header("Yo-admin-token"), "test-token")

    def test_support_unread_days_and_last_line(self):
        users = load("users")
        now = users.time.time() * 1000
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "hub.sqlite")
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE support_messages(id INTEGER PRIMARY KEY, user_id BLOB, from_team INTEGER, text TEXT, created_at INTEGER, read_at INTEGER, done_at INTEGER)")
                day = users.DAY_MS
                db.executemany("INSERT INTO support_messages VALUES(?,?,?,?,?,?,?)", [
                    (1, b"\x01" * 16, 0, "old, read yesterday", now - 5 * day, now - day, now - day),
                    (2, b"\x01" * 16, 1, "team answer", now - day + 1000, None, None),
                    (3, b"\x02" * 16, 0, "still open", now - 3 * day, now - day, None),
                    (4, b"", 1, "team task", now - 3 * day, None, None)])
            snapshot = users.hub("live", path)
            days = snapshot["support_unread_days"]
            self.assertEqual(len(days), 14)
            self.assertEqual(days[-1], 1)
            self.assertEqual(max(days), 2)
            self.assertEqual(snapshot["support_last"]["01" * 16], {"text": "team answer", "team": True})
            self.assertEqual(snapshot["support_last"]["02" * 16]["text"], "still open")

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

    def test_team_todos_snapshot(self):
        users = load("users")
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "hub.sqlite")
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE support_messages(id INTEGER PRIMARY KEY,text TEXT,created_at INTEGER,done_at INTEGER,user_id BLOB)")
                db.execute("INSERT INTO support_messages VALUES(3,'task',4,NULL,X''),(4,'done',5,6,X''),(5,'user',6,NULL,X'01')")
            rows = users.hub("dev", path)["todos"]
            self.assertEqual(rows, [{"id":3,"text":"task","at":4,"done_at":None},{"id":4,"text":"done","at":5,"done_at":6}])
            # From the hub's schema 7 on: their own table.
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE todos(id INTEGER PRIMARY KEY,text TEXT,created_at INTEGER,done_at INTEGER)")
                db.execute("INSERT INTO todos VALUES(7,'own',8,NULL)")
            self.assertEqual(users.hub("dev", path)["todos"], [{"id":7,"text":"own","at":8,"done_at":None}])

    def test_support_proxy_plain_text_and_no_content(self):
        with patch.dict(os.environ, {"ADMIN_USER":"test", "ADMIN_PASSWORD_HASH":"unused",
                "ADMIN_SESSION_KEY":"00", "ACCESS_TEAM":"invalid", "ACCESS_AUD":"test",
                "YO_ADMIN_TOKEN":"test-token", "YO_HUB_DEV_URL":"http://localhost:19090"}):
            admin = load("admin")
        with patch.object(admin.urllib.request,"urlopen",return_value=io.BytesIO(b"")) as send:
            self.assertEqual(admin.support_request("dev","messages/" + "a"*32,"reply 🎨"),{})
            request = send.call_args.args[0]
            self.assertEqual(request.full_url,"http://localhost:19090/v1/admin/messages/" + "a"*32)
            self.assertEqual(request.data,"reply 🎨".encode())
            self.assertEqual(request.get_header("Yo-admin-token"),"test-token")

    def test_done_import_keeps_ids_and_is_repeatable(self):
        import json
        migration = load("migrate_support_done")
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "hub.sqlite")
            done = Path(directory) / "done.json"
            done.write_text(json.dumps(["dev:3","dev:4","live:5","dev:r123"]))
            with sqlite3.connect(path) as db:
                db.executescript("CREATE TABLE todos(id INTEGER,text TEXT,done_at INTEGER);"
                    "CREATE TABLE support_messages(id INTEGER,from_team INTEGER,read_at INTEGER);"
                    "INSERT INTO todos VALUES(3,'task',NULL),(5,'other hub',NULL);"
                    "INSERT INTO support_messages VALUES(4,0,NULL),(4,1,NULL);")
            migration.migrate("dev",path,str(done))
            with sqlite3.connect(path) as db:
                first = db.execute("SELECT done_at FROM todos WHERE id=3").fetchone()[0]
                self.assertIsNotNone(first)
                self.assertIsNone(db.execute("SELECT done_at FROM todos WHERE id=5").fetchone()[0])
                self.assertIsNotNone(db.execute("SELECT read_at FROM support_messages WHERE from_team=0").fetchone()[0])
                self.assertIsNone(db.execute("SELECT read_at FROM support_messages WHERE from_team=1").fetchone()[0])
            migration.migrate("dev",path,str(done))
            with sqlite3.connect(path) as db:
                self.assertEqual(db.execute("SELECT done_at FROM todos WHERE id=3").fetchone()[0],first)


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
                ("POST", ["Admin", {"name": "Open", "note": "App command: yo://open"}], None, ""),
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

    def test_support_requires_login_origin_and_valid_input(self):
        import http.client
        import json
        import threading
        admin = self.admin
        with patch.object(admin.Admin,"who",return_value="test"), \
                patch.object(admin,"session_valid",return_value=True) as login, \
                patch.object(admin,"support_request",return_value={}) as proxy:
            server = admin.ThreadingHTTPServer(("127.0.0.1",0),admin.Admin)
            thread = threading.Thread(target=server.serve_forever,daemon=True)
            thread.start()
            try:
                def call(method,path,data=None,origin=None):
                    conn=http.client.HTTPConnection(*server.server_address)
                    headers={"Content-Type":"application/json"}
                    if origin: headers["Origin"]=origin
                    conn.request(method,path,None if data is None else json.dumps(data),headers)
                    response=conn.getresponse(); response.read(); conn.close()
                    return response.status
                data={"hub":"dev","user":"a"*32,"action":"reply","text":"hello"}
                self.assertEqual(call("POST","/support",data,"https://invalid.example"),403)
                login.return_value=False
                self.assertEqual(call("POST","/support",data),401)
                self.assertEqual(call("GET","/support.json?hub=dev"),401)
                login.return_value=True
                for invalid in [{**data,"hub":"other"},{**data,"user":"../x"},{**data,"text":"x"*2001},{**data,"action":"erase"}]:
                    self.assertEqual(call("POST","/support",invalid),400)
                proxy.assert_not_called()
                self.assertEqual(call("POST","/support",data),200)
                proxy.assert_called_with("dev","messages/"+"a"*32,"hello")
                self.assertEqual(call("GET","/support.json?hub=dev&user="+"a"*32),200)
                proxy.assert_called_with("dev","messages/"+"a"*32)
                self.assertEqual(call("POST","/support/done",{"hub":"dev","id":3,"done":False}),200)
                proxy.assert_called_with("dev","messages/done/3","0")
                self.assertEqual(call("GET","/support.json?hub=dev&user=team"),400)  # the to-dos are no thread any more
                self.assertEqual(call("POST","/todos/done",{"hub":"dev","id":3,"done":True}),200)
                proxy.assert_called_with("dev","todos/3","1")
                self.assertEqual(call("POST","/todos/delete",{"hub":"dev","id":3}),200)
                proxy.assert_called_with("dev","todos/3",None,"DELETE")
                self.assertEqual(call("POST","/todos/delete",{"hub":"dev","id":"3"}),400)
            finally:
                server.shutdown(); server.server_close(); thread.join()

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
