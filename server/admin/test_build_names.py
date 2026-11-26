"""A release's build is named by its version and build ("1.0.0-RC36 (202611241344)"), any other by its date."""
import json
import re
import sqlite3
import subprocess
import tempfile
import unittest
from pathlib import Path

import users


class BuildNameTests(unittest.TestCase):
    def test_versions_come_from_the_map_and_the_devices(self):
        self.assertEqual(users.RELEASES["ios/202611241344"], "1.0.0-RC35")
        self.assertEqual(users.RELEASES["android/202611241341"], "1.0.0-RC35")
        out = {"live": {"app_versions": {"ios/202611241600": "1.0.0-RC37"}, "apps": [["ios/202611241600", 1],
                                                                                       ["ios/202611240754", 1]]}}
        versions = users.app_versions(out)
        self.assertEqual(versions["ios/202611241600"], "1.0.0-RC37")
        self.assertEqual(versions["ios/202611240754"], "1.0.0-RC34")
        builds = users.app_builds(out)
        self.assertEqual(builds["ios/202611240754"]["version"], "1.0.0-RC34")
        self.assertTrue(builds["ios/202611240754"]["title"].startswith("1.0.0-RC34 · TestFlight build"))

    def test_the_hub_keeps_the_version_per_device(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "hub.sqlite"
            with sqlite3.connect(path) as db:
                db.executescript("""
                CREATE TABLE account_device (id BLOB, user_id BLOB, last_seen_at INTEGER, app TEXT, version TEXT);
                INSERT INTO account_device VALUES (x'01', x'01', 0, 'ios/202611241600', '1.0.0-RC37');
                INSERT INTO account_device VALUES (x'02', x'02', 0, 'ios/202611241700', NULL);
                """)
            h = users.hub("live", str(path))
            self.assertEqual(h["app_versions"], {"ios/202611241600": "1.0.0-RC37"})

    def test_labels(self):
        src = Path(__file__).with_name("app.js").read_text()
        fn = re.search(r"// @build-label\n(.*?)// @end", src, re.S).group(1)
        stubs = ('const build = (app) => (app || "").split("/").pop(); const appInfo = () => null;'
                 'const last = { app_versions: { "ios/202611241344": "1.0.0-RC35", "android/202611241341": "1.0.0-RC35",'
                 ' "ios/202612010000": "1.0.0" } };')
        apps = ["ios/202611241344", "android/202611241341", "ios/202612010000", "ios/202611240800",
                "android/202611231451", "ios/1"]
        out = subprocess.run(["node", "-e", stubs + fn + f"console.log(JSON.stringify({json.dumps(apps)}.map(buildLabel)))"],
                             capture_output=True, text=True, check=True).stdout
        self.assertEqual(json.loads(out), ["1.0.0-RC35 (202611241344)", "Android 1.0.0-RC35 (202611241341)",
                                           "1.0.0 (202612010000)", "Nov 24 08:00", "Android Nov 23 14:51", "iOS 1"])

    def test_groups(self):
        src = Path(__file__).with_name("app.js").read_text()
        fn = "".join(re.search(rf"// @{k}\n(.*?)// @end", src, re.S).group(1) for k in ("release-color", "release-groups"))
        versions = {f"ios/202611{d}0000": f"1.0.0-RC{d}" for d in range(10, 18)}
        versions["android/202611170001"] = "1.0.0-RC17"
        apps = list(versions) + ["ios/202611171200", "ios/202611160500", "ios/202611010000", "unknown"]
        js = (fn + f"const v = {json.dumps(versions)};"
              f"const g = releaseGroups({json.dumps(apps)}, (a) => v[a], (a) => a.split('/')[1] < '202611050000');"
              "const rows = g.map((x) => ({ kind: x.kind, name: x.name, color: x.color, n: x.apps.length }));"
              "console.log(JSON.stringify([g.map((x) => [x.name, x.apps.length, x.color]),"
              " foldRows(rows, 8).map((x) => [x.name, x.n]), foldRows(rows, 20).length]))")
        out, folded, unfolded = json.loads(subprocess.run(["node", "-e", js], capture_output=True, text=True, check=True).stdout)
        # A build of main counts with the release it builds on ("+"), none is a band of its own.
        self.assertEqual([n for n, _, _ in out], ["1.0.0-RC17 +", "1.0.0-RC16 +"] + [f"1.0.0-RC{d}" for d in range(15, 9, -1)]
                         + ["Unknown", "Below minimum"])
        self.assertEqual(out[0][1], 3)  # RC17 on iOS and Android, and the build of main after it
        self.assertEqual(out[1][1], 2)  # RC16 and the build of main after it
        hue = [int(re.search(r"hsl\((\d+)", c).group(1)) for _, _, c in out if c.startswith("hsl")]
        self.assertEqual(hue[0], 135); self.assertEqual(hue[-1], 4)
        self.assertTrue(all(a > b for a, b in zip(hue[:8], hue[1:8])), hue)  # each band its own shade
        # The hover: past 8 rows the oldest fold into "Older"; "Below minimum" stays last.
        self.assertEqual([n for n, _ in folded], ["1.0.0-RC17 +", "1.0.0-RC16 +"] + [f"1.0.0-RC{d}" for d in range(15, 10, -1)]
                         + ["Older", "Below minimum"])
        self.assertEqual(folded[-2][1], 1 + 1)  # RC10 and Unknown
        self.assertEqual(unfolded, 10)

if __name__ == "__main__":
    unittest.main()
