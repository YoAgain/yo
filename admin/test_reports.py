"""The Reports view's grouping (app.js's pure part, run in node): one row per reported user, most reports first,
reasons counted, reporters distinct install ids, open until every report of the user is ticked."""
import json
from pathlib import Path
import shutil
import subprocess
import unittest

SOURCE = (Path(__file__).with_name("app.js")).read_text()
FN = SOURCE[SOURCE.index("// <report-groups>"):SOURCE.index("// </report-groups>")]


def run(case: dict):
    script = FN + ("\nconst c = JSON.parse(process.argv[1]);\nconst done = new Set(c.done || []);\n"
                   "console.log(JSON.stringify(reportGroups({ reports: c.reports, users: c.users, isDone: (r) => done.has(r.id) })));\n")
    return json.loads(subprocess.run(["node", "-e", script, json.dumps(case)], capture_output=True, text=True, check=True).stdout)


def rep(i, reporter, reported, reason, at, name=None):
    return {"id": i, "reporter": reporter, "reported": reported, "reason": reason, "at": at, "reported_name": name}


@unittest.skipUnless(shutil.which("node"), "node not installed")
class ReportGroupTests(unittest.TestCase):
    CASE = {
        "reports": [rep("1", "a" * 32, "u1", "spam", 100, "alice"), rep("2", "b" * 32, "u1", "spam", 300, "alice"),
                    rep("3", "c" * 32, "u1", "abuse", 200, "alice"), rep("4", "a" * 32, "u2", "other", 400),
                    rep("5", "a" * 32, "u1", "spam", 250, "alice")],
        "users": [{"id": "u2", "name": "bob", "created": 5, "last_seen": 9, "devices": 2}],
    }

    def test_groups_and_order(self):
        g = run(self.CASE)
        self.assertEqual([x["id"] for x in g], ["u1", "u2"])
        self.assertEqual((g[0]["count"], g[0]["reporters"], g[0]["name"]), (4, 3, "alice"))
        self.assertEqual(g[0]["reasons"], [["spam", 3], ["abuse", 1]])
        self.assertEqual((g[0]["first"], g[0]["last"]), (100, 300))
        self.assertEqual([r["at"] for r in g[0]["rows"]], [300, 250, 200, 100])

    def test_user_status_and_name_fallback(self):
        g = run(self.CASE)
        self.assertIsNone(g[0]["user"])
        self.assertEqual((g[1]["name"], g[1]["user"]["devices"]), ("bob", 2))
        self.assertEqual(run({"reports": [rep("9", "a" * 32, "ffeeddccbbaa", "spam", 1)]})[0]["name"], "ffeeddcc")

    def test_open_until_all_ticked(self):
        self.assertEqual([x["open"] for x in run({**self.CASE, "done": ["1", "2", "3"]})], [True, True])
        self.assertEqual([x["open"] for x in run({**self.CASE, "done": ["1", "2", "3", "5"]})], [False, True])

    def test_unknown_reason_kept(self):
        self.assertEqual(run({"reports": [rep("1", "a" * 32, "u1", "7", 1)]})[0]["reasons"], [["7", 1]])


if __name__ == "__main__":
    unittest.main()
