"""App labels from GitHub release fixtures (no network)."""
import unittest
from unittest.mock import patch

import users

RELEASES = [
    {"tag_name": "v1.2", "draft": True, "prerelease": False, "published_at": None, "body": "Build: 202611201200"},
    {"tag_name": "v1.1", "draft": False, "prerelease": True, "published_at": "2026-11-18T19:30:00Z",
     "body": "Fixes.\r\n\r\nBuild: 202611181859\r\n"},
    {"tag_name": "v1.0", "draft": False, "prerelease": False, "published_at": "2026-11-17T17:00:00Z",
     "body": "First.\n\nBuild: 202611171622\n"},
    {"tag_name": "v0.9", "draft": False, "prerelease": False, "published_at": "2026-11-19T08:00:00Z",
     "body": "No build line."},
]
NEWEST = {"build": "202611181859", "tag": "v1.1", "prerelease": True}


class ReleaseTests(unittest.TestCase):
    def test_newest_non_draft_with_build(self):
        self.assertEqual(users.newest_release(RELEASES), NEWEST)

    def test_none(self):
        self.assertEqual(users.newest_release([]), {})
        self.assertEqual(users.newest_release([RELEASES[0], RELEASES[3]]), {})

    def test_cache_and_failure_keep_value(self):
        with patch.object(users, "releases_fetch", side_effect=OSError("down")) as fetch:
            fresh = {**NEWEST, "checked": 1000}
            self.assertIs(users.releases(fresh, now=1000 + 60), fresh)
            fetch.assert_not_called()
            failed = users.releases(fresh, now=1000 + users.RELEASES_EVERY_S)
            self.assertEqual(failed["build"], NEWEST["build"])
            self.assertIn("down", failed["error"])
        with patch.object(users, "releases_fetch", return_value=RELEASES):
            ok = users.releases(failed, now=5000)
            self.assertNotIn("error", ok)
            self.assertEqual((ok["build"], ok["checked"]), (NEWEST["build"], 5000))


class ClassifyTests(unittest.TestCase):
    def c(self, app, release=NEWEST, bot=None):
        return users.classify(app, release, bot)

    def test_ios_against_newest_release(self):
        self.assertEqual((self.c("ios/202611181859")["label"], self.c("ios/202611181859")["state"]),
                         ("TestFlight · newest", "newest"))
        self.assertEqual((self.c("ios/202611171622")["label"], self.c("ios/202611171622")["state"]),
                         ("TestFlight · update", "older"))
        self.assertEqual((self.c("ios/202611191257")["label"], self.c("ios/202611191257")["state"]),
                         ("TestFlight (main)", None))

    def test_no_release_known(self):
        for release in (None, {}, {"error": "URLError", "checked": 1}):
            r = self.c("ios/202611171622", release)
            self.assertEqual((r["label"], r["state"]), ("iOS 1622", None))

    def test_dev_builds(self):
        self.assertEqual(self.c("ios/1")["channel"], "dev")
        self.assertIsNone(self.c("ios/1")["state"])

    def test_other_platforms(self):
        self.assertEqual(self.c("yogpt/202611182101", bot="yogpt/202611182059")["state"], "newest")
        self.assertEqual(self.c("yogpt/202611181000", bot="yogpt/202611182059")["state"], "older")
        self.assertEqual(self.c("yogpt/202611181000")["state"], None)
        self.assertEqual(self.c("android/202611160001")["state"], None)
        self.assertEqual(self.c(None)["channel"], "unknown")

    def test_app_builds_from_both_hubs(self):
        out = {"releases": NEWEST,
               "live": {"apps": [["ios/202611171622", 2]], "users": [{"app": "ios/1", "device_list": []}],
                        "yogpt_min": "yogpt/202611182059"},
               "dev": {"apps": [["yogpt/202611182101", 1], ["unknown", 3]], "users": []}}
        b = users.app_builds(out)
        self.assertEqual(set(b), {"ios/202611171622", "ios/1", "yogpt/202611182101"})
        self.assertEqual(b["ios/202611171622"]["label"], "TestFlight · update")
        self.assertEqual(b["yogpt/202611182101"]["state"], "newest")


if __name__ == "__main__":
    unittest.main()
