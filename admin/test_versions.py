"""App states: current is the highest iOS build two users report (no network)."""
import unittest

import users

NEWEST = "202611181859"


class ClassifyTests(unittest.TestCase):
    def c(self, app, newest=NEWEST, bot=None):
        return users.classify(app, newest, bot)

    def test_classify(self):
        bot = "yogpt/202611182059"
        for app, newest, bot_min, label, state in [
            ("ios/202611181859", NEWEST, None, "iOS", "current"),
            ("ios/202611191257", NEWEST, None, "iOS", "fresh"),  # newer than the baseline: one user so far
            ("ios/202611171622", NEWEST, None, "iOS", "update"),
            ("ios/202611171622", None, None, "iOS", None),
            ("ios/1", NEWEST, None, "iOS", None),
            ("yogpt/202611182101", NEWEST, bot, "Bot", "current"),
            ("yogpt/202611181000", NEWEST, bot, "Bot", "update"),
            ("android/202611181859", NEWEST, None, "Android", "current"),
            ("android/202611191257", NEWEST, None, "Android", "fresh"),
            ("android/202611160001", NEWEST, None, "Android", "update"),  # RC8 and older: a fixed build
            ("android/202611160001", None, None, "Android", None),
            ("android/29919800", NEWEST, None, "Android", None),  # not a minute: a dev build
        ]:
            r = self.c(app, newest, bot_min)
            self.assertEqual((r["label"], r["state"]), (label, state), app)
        self.assertIn(NEWEST, self.c("ios/202611171622")["title"])
        self.assertIn("fresh: only 1 user so far", self.c("ios/202611191257")["title"])
        self.assertEqual(self.c(None)["label"], "unknown")

    def test_baseline_is_the_highest_build_two_users_report(self):
        out = {"live": {"apps": [["ios/202611171622", 2], ["ios/202611191257", 1]],
                        "users": [{"id": "aa", "app": "ios/202611171622", "device_list": [{"app": "ios/202611171622"}]},
                                  {"id": "bb", "app": "ios/202611181859", "device_list": []},
                                  {"id": "cc", "app": "ios/202611181859", "device_list": []},
                                  {"id": "team", "app": "ios/202611191257", "device_list": [{"app": "ios/202611191257"}]}],
                        "yogpt_min": "yogpt/202611182059"},
               "dev": {"apps": [["yogpt/202611182101", 1], ["unknown", 3], ["ios/1", 1]], "users": []}}
        b = users.app_builds(out)
        self.assertEqual(set(b), {"ios/202611171622", "ios/202611181859", "ios/202611191257", "yogpt/202611182101", "ios/1"})
        self.assertEqual(b["ios/202611191257"]["state"], "fresh")
        self.assertEqual(b["ios/202611181859"]["state"], "current")
        self.assertEqual(b["ios/202611171622"]["state"], "update")
        # a second user (even on the other hub, with several devices counting once) makes the newest the baseline
        out["dev"]["users"] = [{"id": "team", "app": "ios/202611191257", "device_list": []},
                               {"id": "dd", "app": "ios/202611191257", "device_list": []}]
        b = users.app_builds(out)
        self.assertEqual(b["ios/202611191257"]["state"], "current")
        self.assertEqual(b["ios/202611181859"]["state"], "update")

    def test_android_builds_are_compared_only_with_android(self):
        us = lambda *p: [{"id": i, "app": a, "device_list": []} for i, a in p]
        out = {"live": {"users": us(("a", "android/202611160001"), ("b", "android/202611200930"), ("c", "android/202611200930"),
                                    ("d", "android/202611201200"), ("e", "android/1"), ("f", "ios/202611181859"))}}
        b = users.app_builds(out)
        self.assertEqual([b[f"android/{n}"]["state"] for n in (202611160001, 202611200930, 202611201200)], ["update", "current", "fresh"])
        self.assertIsNone(b["android/1"]["state"])
        self.assertEqual(b["android/1"]["title"], "Android dev build 1")
        self.assertEqual(b["ios/202611181859"]["state"], "current")  # alone on iOS, not judged against Android

    def test_without_any_build_of_two_users_the_highest_is_current(self):
        out = {"live": {"users": [{"id": "aa", "app": "ios/202611171622"}, {"id": "bb", "app": "ios/202611181859"}]}}
        b = users.app_builds(out)
        self.assertEqual(b["ios/202611181859"]["state"], "current")
        self.assertEqual(b["ios/202611171622"]["state"], "update")

    def test_version_day_counts_todays_devices(self):
        builds = {"ios/202611181859": {"state": "current"}, "ios/202611171622": {"state": "update"},
                  "ios/202611100000": {"state": "update"}, "yogpt/202611182101": {"state": "current"},
                  "ios/202611191257": {"state": "fresh"}}
        apps = [["ios/202611181859", 5], ["ios/202611171622", 3], ["ios/202611100000", 2], ["ios/1", 1],
                ["android/4", 1], ["yogpt/202611182101", 1], ["cli/7", 1], ["unknown", 2], ["ios/202611191257", 2]]
        self.assertEqual(users.version_day(apps, builds, "ios/202611150000"), {"current": 7, "update": 3, "must": 3, "unknown": 3})
        self.assertEqual(users.version_day(apps, builds, "android/5, ios/202611150000")["must"], 4)  # the hub refuses ios/1 too
        self.assertEqual(users.version_day(apps, builds, None)["must"], 0)


if __name__ == "__main__":
    unittest.main()
