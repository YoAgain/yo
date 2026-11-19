"""The support chat list (app.js's supportList, run in node): who wrote, open or answered, newest first."""
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
const rows = supportList({ users: c.users, threads: c.threads, filter: c.filter });
console.log(JSON.stringify(rows.map((r) => [r.id, r.name])));
"""
    return json.loads(subprocess.run(["node", "-e", script, json.dumps(case)], capture_output=True, text=True, check=True).stdout)


@unittest.skipUnless(shutil.which("node"), "node not installed")
class SupportListTests(unittest.TestCase):
    CASE = {
        "users": [{"id": "aa", "name": "Ann"}, {"id": "bb", "name": "Ben"}, {"id": "cc", "name": "Cid"}],
        "threads": [{"user_id": "AA", "at": 100, "unread": 0}, {"user_id": "DD", "at": 300, "unread": 1, "name": "Dee"},
                    {"user_id": "BB", "at": 200, "unread": 2}, {"user_id": "team", "at": 400, "unread": 0}],
    }

    def test_open_by_default_newest_first(self):
        self.assertEqual(run({**self.CASE, "filter": "open"}), [["DD", "Dee"], ["BB", "Ben"]])

    def test_answered_and_never_the_team_or_silent_users(self):
        self.assertEqual(run({**self.CASE, "filter": "answered"}), [["AA", "Ann"]])


if __name__ == "__main__":
    unittest.main()
