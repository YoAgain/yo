"""App states: current is the highest iOS build any user reports (no network)."""
import unittest

import users

NEWEST = "202611181859"


class ClassifyTests(unittest.TestCase):
    def c(self, app, newest=NEWEST, bot=None):
        return users.classify(app, newest, bot)

    def test_ios(self):
        self.assertEqual((self.c("ios/202611181859")["label"], self.c("ios/202611181859")["state"]), ("iOS", "current"))
        self.assertEqual(self.c("ios/202611191257")["state"], "current")  # newer than any user's: an internal build
        r = self.c("ios/202611171622")
        self.assertEqual(r["state"], "update")
        self.assertIn(NEWEST, r["title"])
        self.assertEqual(self.c("ios/202611171622", None)["state"], None)

    def test_dev_and_others(self):
        self.assertEqual((self.c("ios/1")["label"], self.c("ios/1")["state"]), ("iOS", None))
        self.assertEqual(self.c("yogpt/202611182101", bot="yogpt/202611182059")["state"], "current")
        r = self.c("yogpt/202611181000", bot="yogpt/202611182059")
        self.assertEqual((r["label"], r["state"]), ("Bot", "update"))
        self.assertEqual((self.c("android/202611160001")["label"], self.c("android/202611160001")["state"]), ("Android", None))
        self.assertEqual(self.c(None)["label"], "unknown")

    def test_the_highest_reported_build_is_current_for_everyone(self):
        out = {"live": {"apps": [["ios/202611171622", 2], ["ios/202611191257", 1]],
                        "users": [{"id": "aa", "app": "ios/202611171622", "device_list": [{"app": "ios/202611171622"}]},
                                  {"id": "bb", "app": "ios/202611181859", "device_list": []},
                                  {"id": "team", "app": "ios/202611191257", "device_list": [{"app": "ios/202611191257"}]}],
                        "yogpt_min": "yogpt/202611182059"},
               "dev": {"apps": [["yogpt/202611182101", 1], ["unknown", 3], ["ios/1", 1]], "users": []}}
        b = users.app_builds(out)
        self.assertEqual(set(b), {"ios/202611171622", "ios/202611181859", "ios/202611191257", "yogpt/202611182101", "ios/1"})
        self.assertEqual(b["ios/202611191257"]["state"], "current")
        self.assertEqual(b["ios/202611181859"]["state"], "update")
        self.assertEqual(b["ios/202611171622"]["state"], "update")

    def test_version_day_counts_todays_devices(self):
        builds = {"ios/202611181859": {"state": "current"}, "ios/202611171622": {"state": "update"},
                  "ios/202611100000": {"state": "update"}, "yogpt/202611182101": {"state": "current"}}
        apps = [["ios/202611181859", 5], ["ios/202611171622", 3], ["ios/202611100000", 2], ["ios/1", 1],
                ["android/4", 1], ["yogpt/202611182101", 1], ["cli/7", 1], ["unknown", 2]]
        self.assertEqual(users.version_day(apps, builds, "ios/202611150000"), {"current": 5, "update": 3, "must": 3, "unknown": 3})
        self.assertEqual(users.version_day(apps, builds, "android/5, ios/202611150000")["must"], 4)  # the hub refuses ios/1 too
        self.assertEqual(users.version_day(apps, builds, None)["must"], 0)


if __name__ == "__main__":
    unittest.main()
