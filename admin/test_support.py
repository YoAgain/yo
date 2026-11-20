"""The support chat list and crash-free (app.js's pure part, run in node): installs that wrote, open or answered,
newest first, by their short id; crash-free (est.) from installs with a crash against active users."""
import json
from pathlib import Path
import shutil
import subprocess
import unittest

SOURCE = (Path(__file__).with_name("app.js")).read_text()
FN = SOURCE[SOURCE.index("// <support-list>"):SOURCE.index("// </support-list>")]


def node(expression: str, case):
    script = FN + f"\nconst c = JSON.parse(process.argv[1]);\nconsole.log(JSON.stringify({expression}));\n"
    return json.loads(subprocess.run(["node", "-e", script, json.dumps(case)], capture_output=True, text=True, check=True).stdout)


def run(case: dict):
    return node("supportList({ threads: c.threads, filter: c.filter }).map((r) => [r.id, r.name])", case)


A, B, D = "A3F09C" + "0" * 26, "b" * 32, "d4e5f6" + "1" * 26


@unittest.skipUnless(shutil.which("node"), "node not installed")
class SupportListTests(unittest.TestCase):
    CASE = {"threads": [{"install": A, "at": 100, "unread": 0}, {"install": D, "at": 300, "unread": 1},
                        {"install": B, "at": 200, "unread": 2}, {"install": "team", "at": 400, "unread": 1},
                        {"user_id": "c" * 32, "at": 500, "unread": 1}]}

    def test_open_by_default_newest_first_by_short_install_id(self):
        self.assertEqual(run({**self.CASE, "filter": "open"}), [[D, "d4e5f6"], [B, "bbbbbb"]])

    def test_answered_lower_case_and_never_anything_but_an_install(self):
        self.assertEqual(run({**self.CASE, "filter": "answered"}), [[A.lower(), "a3f09c"]])

    def test_install_short_form(self):
        self.assertEqual(node("[installShort(c[0]), installShort(c[1]), installShort(null)]", [A, "a3f09c"]), ["a3f09c", "a3f09c", ""])


@unittest.skipUnless(shutil.which("node"), "node not installed")
class CrashFreeTests(unittest.TestCase):
    def free(self, installs, users):
        return node("crashFree(c[0], c[1])", [installs, users])

    def test_installs_with_a_crash_against_active_users(self):
        self.assertEqual(self.free(5, 200), 97.5)
        self.assertEqual(self.free(0, 10), 100)

    def test_never_below_zero_and_none_without_data(self):
        self.assertEqual(self.free(30, 10), 0)  # installs are not users: more installs than users clamps at 0
        self.assertIsNone(self.free(None, 10))  # an older hub without diagnostic_installs
        self.assertIsNone(self.free(3, 0))


if __name__ == "__main__":
    unittest.main()
