"""The Support list (app.js's supportList, run in node): one To-dos row, writers by default, starred on top."""
import json
from pathlib import Path
import shutil
import subprocess
import unittest

SOURCE = (Path(__file__).with_name("app.js")).read_text()
FN = SOURCE[SOURCE.index("// <support-list>"):SOURCE.index("// </support-list>")]


def run(case: dict):
    script = FN + """
const c = JSON.parse(process.argv[1]);
const rows = supportList({ users: c.users, threads: c.threads, todos: c.todos, todoAbout: (id) => c.about[id] || null,
  starred: (k) => c.stars.includes(k), query: c.query || "", filter: c.filter });
console.log(JSON.stringify(rows.map((r) => [r.kind, r.id, r.name])));
"""
    return json.loads(subprocess.run(["node", "-e", script, json.dumps(case)], capture_output=True, text=True, check=True).stdout)


@unittest.skipUnless(shutil.which("node"), "node not installed")
class SupportListTests(unittest.TestCase):
    CASE = {
        "users": [{"id": "aa", "name": "Ann", "last_seen": 5}, {"id": "bb", "name": "Ben", "last_seen": 9},
                  {"id": "cc", "name": "Cid", "last_seen": 1}],
        "threads": [{"user_id": "AA", "at": 100, "unread": 0}, {"user_id": "DD", "at": 300, "unread": 1, "name": "Dee"}],
        "todos": [{"id": 1, "text": "one", "created_at": 200, "done_at": None},
                  {"id": 2, "text": "two", "created_at": 50, "done_at": None},
                  {"id": 3, "text": "done", "created_at": 10, "done_at": 20}],
        "about": {"2": "CC"}, "stars": [],
    }

    def test_default_writers_and_one_todo_row(self):
        rows = run({**self.CASE, "filter": "chats"})
        self.assertEqual(rows, [["user", "DD", "Dee"], ["todo", "team", "To-dos · 2"], ["user", "AA", "Ann"]])

    def test_all_shows_everyone(self):
        ids = [r[1] for r in run({**self.CASE, "filter": "all"})]
        self.assertEqual(sorted(ids), ["AA", "BB", "CC", "DD", "team"])
        self.assertEqual(ids.count("team"), 1)

    def test_starred_on_top_and_filters(self):
        rows = run({**self.CASE, "filter": "chats", "stars": ["u:AA"]})
        self.assertEqual(rows[0][1], "AA")
        self.assertEqual([r[1] for r in run({**self.CASE, "filter": "star", "stars": ["u:AA"]})], ["AA"])
        # Open: unread messages, open to-dos (Cid has one about him, without a chat).
        self.assertEqual([r[1] for r in run({**self.CASE, "filter": "open"})], ["DD", "team", "CC"])

    def test_no_todos(self):
        rows = run({**self.CASE, "todos": [], "filter": "chats"})
        self.assertIn(["todo", "team", "To-dos"], rows)


if __name__ == "__main__":
    unittest.main()
