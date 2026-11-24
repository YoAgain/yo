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
                CREATE TABLE devices (id BLOB, user_id BLOB, last_seen_at INTEGER, app TEXT, version TEXT);
                INSERT INTO devices VALUES (x'01', x'01', 0, 'ios/202611241600', '1.0.0-RC37');
                INSERT INTO devices VALUES (x'02', x'02', 0, 'ios/202611241700', NULL);
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


if __name__ == "__main__":
    unittest.main()
