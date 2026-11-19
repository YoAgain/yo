"""App states from the build number's channel (no network)."""
import unittest

import users

NEWEST = {"public": "202611181859.1", "main": "202611191257"}


class ClassifyTests(unittest.TestCase):
    def c(self, app, newest=NEWEST, bot=None):
        return users.classify(app, newest, bot)

    def test_channels(self):
        self.assertEqual(users.ios_channel("202611181859.1"), "public")
        self.assertEqual(users.ios_channel("202611181859"), "main")
        self.assertIsNone(users.ios_channel("1"))
        self.assertIsNone(users.ios_channel("202611181859.2"))

    def test_public(self):
        self.assertEqual((self.c("ios/202611181859.1")["label"], self.c("ios/202611181859.1")["state"]), ("iOS", "current"))
        r = self.c("ios/202611171622.1")
        self.assertEqual(r["state"], "update")
        self.assertIn("TestFlight public", r["title"])
        self.assertIn("202611181859.1", r["title"])

    def test_main_never_update_against_public(self):
        self.assertEqual(self.c("ios/202611191257")["state"], "current")
        self.assertEqual(self.c("ios/202611180000")["state"], "update")
        self.assertEqual(self.c("ios/202611180000", {"public": "202611191300.1"})["state"], None)

    def test_dev_and_others(self):
        self.assertEqual((self.c("ios/1")["label"], self.c("ios/1")["state"]), ("iOS", None))
        self.assertEqual(self.c("yogpt/202611182101", bot="yogpt/202611182059")["state"], "current")
        self.assertEqual((self.c("yogpt/202611181000", bot="yogpt/202611182059")["label"],
                          self.c("yogpt/202611181000", bot="yogpt/202611182059")["state"]), ("Bot", "update"))
        self.assertEqual((self.c("android/202611160001")["label"], self.c("android/202611160001")["state"]), ("Android", None))
        self.assertEqual(self.c(None)["label"], "unknown")

    def test_app_builds_newest_per_channel_from_both_hubs(self):
        out = {"live": {"apps": [["ios/202611171622.1", 2], ["ios/202611191257", 1]],
                        "users": [{"app": "ios/1", "device_list": [{"app": "ios/202611181859.1"}]}],
                        "yogpt_min": "yogpt/202611182059"},
               "dev": {"apps": [["yogpt/202611182101", 1], ["unknown", 3], ["ios/202611180000", 1]], "users": []}}
        b = users.app_builds(out)
        self.assertEqual(set(b), {"ios/202611171622.1", "ios/202611191257", "ios/1", "ios/202611181859.1",
                                  "yogpt/202611182101", "ios/202611180000"})
        self.assertEqual(b["ios/202611171622.1"]["state"], "update")
        self.assertEqual(b["ios/202611181859.1"]["state"], "current")
        self.assertEqual(b["ios/202611191257"]["state"], "current")
        self.assertEqual(b["ios/202611180000"]["state"], "update")
        self.assertEqual(b["yogpt/202611182101"]["state"], "current")


if __name__ == "__main__":
    unittest.main()
